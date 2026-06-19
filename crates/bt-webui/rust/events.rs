//! WebUI 生产事件源与 JSON DTO。
//!
//! # 职责
//! - 后台读取内核 socket 事件流，并维护一个后端历史窗口。
//! - 把 `bt-agent` 的固定 UAPI 事件转换成前端现有 normalized model 可消费的 JSON。
//! - 在 Rust 层完成筛选与窗口裁剪，避免浏览器承担后端职责。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use bt_agent::{BinderEvent, CaptureConfig, SocketIpcClient};
use bt_decoder::{AndroidPlatformMethods, parse_interface_token};
use serde::{Deserialize, Serialize};

const DEFAULT_MAX_EVENTS: usize = 65_536;
const DEFAULT_QUERY_LIMIT: usize = 256;
const MAX_QUERY_LIMIT: usize = 4_096;
const MAX_INTERFACE_OPTIONS: usize = 512;
const TF_ONE_WAY: u32 = 0x01;
const POLL_TIMEOUT: Duration = Duration::from_millis(250);
const SLOW_TRANSACTION_US: u64 = 10_000;

/// WebUI 事件源配置。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebuiEventsConfig {
    /// 是否启动生产事件源；测试静态资源服务时可关闭。
    pub enabled: bool,
    /// WebUI 是否负责开启内核捕获配置；`None` 表示只读现有事件流。
    pub capture_config: Option<CaptureConfig>,
    /// Android SDK 版本，用于平台 Binder 方法名解析。
    pub android_sdk: Option<u16>,
    /// WebUI 内存中保留的最近事件数。
    pub max_events: usize,
}

impl Default for WebuiEventsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            capture_config: None,
            android_sdk: None,
            max_events: DEFAULT_MAX_EVENTS,
        }
    }
}

/// 共享事件窗口。
#[derive(Clone, Debug)]
pub(crate) struct WebuiEventHub {
    state: Arc<Mutex<EventState>>,
}

impl WebuiEventHub {
    pub(crate) fn new(config: WebuiEventsConfig) -> Self {
        let state = Arc::new(Mutex::new(EventState::new(config.max_events)));
        let hub = Self {
            state: Arc::clone(&state),
        };

        if config.enabled {
            spawn_collector(state, config);
        } else {
            hub.set_source_state(SourceState::Disabled, None);
        }

        hub
    }

    pub(crate) fn snapshot(&self, query: &EventsQuery) -> EventsResponse {
        let state = self
            .state
            .lock()
            .expect("event state mutex should not poison");
        state.snapshot(query)
    }

    pub(crate) fn clear(&self) {
        let mut state = self
            .state
            .lock()
            .expect("event state mutex should not poison");
        state.clear();
    }

    fn set_source_state(&self, source_state: SourceState, error: Option<String>) {
        let mut state = self
            .state
            .lock()
            .expect("event state mutex should not poison");
        state.source_state = source_state;
        state.error = error;
    }
}

#[derive(Debug)]
struct EventState {
    events: VecDeque<ApiTraceEvent>,
    call_summaries: BTreeMap<u64, CallSummary>,
    process_labels: HashMap<u32, String>,
    max_events: usize,
    total_count: u64,
    kernel_lost_count: u64,
    backend_evicted_count: u64,
    source_state: SourceState,
    error: Option<String>,
}

impl EventState {
    fn new(max_events: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(max_events.max(1)),
            call_summaries: BTreeMap::new(),
            process_labels: HashMap::new(),
            max_events: max_events.max(1),
            total_count: 0,
            kernel_lost_count: 0,
            backend_evicted_count: 0,
            source_state: SourceState::Connecting,
            error: None,
        }
    }

    fn snapshot(&self, query: &EventsQuery) -> EventsResponse {
        let filter = EventFilter::from(query);
        let limit = query.limit();
        let matched = self
            .events
            .iter()
            .filter(|event| filter.matches(event))
            .collect::<Vec<_>>();
        let selected = select_page(&matched, query.page_request(), limit);
        let oldest_seq = selected.events.first().map(|event| event.raw.seq);
        let newest_seq = selected.events.last().map(|event| event.raw.seq);
        let events = selected.events.into_iter().cloned().collect();

        EventsResponse {
            events,
            total_count: self.total_count,
            retained_count: self.events.len(),
            matched_count: matched.len() as u64,
            dropped_count: self.kernel_lost_count,
            backend_evicted_count: self.backend_evicted_count,
            oldest_seq,
            newest_seq,
            page_start_index: selected.start_index,
            page_end_index: selected.end_index,
            has_more_before: selected
                .start_index
                .is_some_and(|start_index| start_index > 1),
            has_more_after: selected
                .end_index
                .is_some_and(|end_index| end_index < matched.len() as u64),
            interfaces: self.interface_options(),
            source_state: self.source_state,
            error: self.error.clone(),
            device_context: "production · binder transaction stream",
        }
    }

    fn clear(&mut self) {
        self.events.clear();
        self.call_summaries.clear();
        self.total_count = 0;
        self.kernel_lost_count = 0;
        self.backend_evicted_count = 0;
    }

    fn push_event(&mut self, event: ApiTraceEvent, lost_before: u32) {
        self.total_count += 1;
        self.kernel_lost_count = self
            .kernel_lost_count
            .saturating_add(u64::from(lost_before));
        self.events.push_back(event);
        while self.events.len() > self.max_events {
            if let Some(evicted) = self.events.pop_front() {
                self.call_summaries.remove(&evicted.enrichment.debug_id);
            }
            self.backend_evicted_count += 1;
        }
    }

    fn process_label(&mut self, tgid: u32) -> String {
        if let Some(label) = self.process_labels.get(&tgid) {
            return label.clone();
        }

        let label = read_process_label(tgid).unwrap_or_else(|| format!("pid:{tgid}"));
        self.process_labels.insert(tgid, label.clone());
        label
    }

    fn interface_options(&self) -> Vec<String> {
        let mut counts = HashMap::<String, usize>::new();
        for event in &self.events {
            *counts
                .entry(event.enrichment.interface.clone())
                .or_default() += 1;
        }

        let mut entries = counts.into_iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        entries
            .into_iter()
            .take(MAX_INTERFACE_OPTIONS)
            .map(|entry| entry.0)
            .collect()
    }
}

#[derive(Debug, Clone)]
struct CallSummary {
    interface: String,
    method: String,
    timestamp_ns: u64,
}

/// `/api/events` 响应。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct EventsResponse {
    pub(crate) events: Vec<ApiTraceEvent>,
    pub(crate) total_count: u64,
    pub(crate) retained_count: usize,
    pub(crate) matched_count: u64,
    pub(crate) dropped_count: u64,
    pub(crate) backend_evicted_count: u64,
    pub(crate) oldest_seq: Option<u64>,
    pub(crate) newest_seq: Option<u64>,
    pub(crate) page_start_index: Option<u64>,
    pub(crate) page_end_index: Option<u64>,
    pub(crate) has_more_before: bool,
    pub(crate) has_more_after: bool,
    pub(crate) interfaces: Vec<String>,
    pub(crate) source_state: SourceState,
    pub(crate) error: Option<String>,
    pub(crate) device_context: &'static str,
}

/// `/api/events` 查询参数。
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct EventsQuery {
    #[serde(default)]
    query: String,
    #[serde(default)]
    direction: DirectionFilter,
    #[serde(default, rename = "interface")]
    interface_name: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    before_seq: Option<u64>,
    #[serde(default)]
    after_seq: Option<u64>,
}

impl EventsQuery {
    fn limit(&self) -> usize {
        self.limit
            .unwrap_or(DEFAULT_QUERY_LIMIT)
            .clamp(1, MAX_QUERY_LIMIT)
    }

    fn page_request(&self) -> PageRequest {
        if let Some(seq) = self.before_seq {
            return PageRequest::Before(seq);
        }
        if let Some(seq) = self.after_seq {
            return PageRequest::After(seq);
        }
        PageRequest::Latest
    }
}

#[derive(Debug, Clone, Copy)]
enum PageRequest {
    Latest,
    Before(u64),
    After(u64),
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DirectionFilter {
    #[default]
    All,
    Call,
    Reply,
    Oneway,
}

#[derive(Debug, Clone)]
struct EventFilter {
    needle: String,
    direction: DirectionFilter,
    interface_name: Option<String>,
}

impl EventFilter {
    fn matches(&self, event: &ApiTraceEvent) -> bool {
        if !self.matches_direction(event) {
            return false;
        }
        if self
            .interface_name
            .as_deref()
            .is_some_and(|interface_name| event.enrichment.interface != interface_name)
        {
            return false;
        }
        self.needle.is_empty() || event_matches_needle(event, &self.needle)
    }

    fn matches_direction(&self, event: &ApiTraceEvent) -> bool {
        match self.direction {
            DirectionFilter::All => true,
            DirectionFilter::Call => event_direction(event) == EventDirection::Call,
            DirectionFilter::Reply => event_direction(event) == EventDirection::Reply,
            DirectionFilter::Oneway => event_direction(event) == EventDirection::Oneway,
        }
    }
}

impl From<&EventsQuery> for EventFilter {
    fn from(query: &EventsQuery) -> Self {
        let interface_name = normalized_scalar_filter(&query.interface_name);
        Self {
            needle: query.query.trim().to_lowercase(),
            direction: query.direction,
            interface_name,
        }
    }
}

#[derive(Debug)]
struct SelectedPage<'a> {
    events: Vec<&'a ApiTraceEvent>,
    start_index: Option<u64>,
    end_index: Option<u64>,
}

fn select_page<'a>(
    matched: &[&'a ApiTraceEvent],
    request: PageRequest,
    limit: usize,
) -> SelectedPage<'a> {
    let (start, end) = match request {
        PageRequest::Latest => {
            let end = matched.len();
            (end.saturating_sub(limit), end)
        }
        PageRequest::Before(seq) => {
            let end = matched.partition_point(|event| event.raw.seq < seq);
            (end.saturating_sub(limit), end)
        }
        PageRequest::After(seq) => {
            let start = matched.partition_point(|event| event.raw.seq <= seq);
            (start, start.saturating_add(limit).min(matched.len()))
        }
    };

    let events = matched[start..end].to_vec();
    if events.is_empty() {
        return SelectedPage {
            events,
            start_index: None,
            end_index: None,
        };
    }

    SelectedPage {
        events,
        start_index: Some(start as u64 + 1),
        end_index: Some(end as u64),
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceState {
    Connecting,
    Connected,
    Disabled,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ApiTraceEvent {
    raw: RawTraceRecord,
    enrichment: ApiTraceEnrichment,
}

#[derive(Debug, Clone, Serialize)]
struct RawTraceRecord {
    device_id: &'static str,
    seq: u64,
    timestamp_ns: u64,
    object: &'static str,
    data: RawRecordData,
}

#[derive(Debug, Clone, Serialize)]
struct RawRecordData {
    kind: &'static str,
    binder_device: &'static str,
    process: RawRecordProcess,
    flags: u32,
    sequence: u64,
    transaction: RawRecordTransaction,
}

#[derive(Debug, Clone, Serialize)]
struct RawRecordProcess {
    pid: u32,
    tid: u32,
    uid: u32,
}

#[derive(Debug, Clone, Serialize)]
struct RawRecordTransaction {
    code: u32,
    flags: u32,
    data_size: u64,
    offsets_size: u64,
    target_handle: u32,
    sender_pid: u32,
    sender_euid: u32,
    payload_truncated: bool,
    payload_hex: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiTraceEnrichment {
    interface: String,
    method: String,
    process_label: String,
    debug_id: u64,
    reply_to_debug_id: Option<u64>,
    latency_us: Option<u64>,
    status: &'static str,
}

fn spawn_collector(state: Arc<Mutex<EventState>>, config: WebuiEventsConfig) {
    let thread_state = Arc::clone(&state);
    if let Err(error) = thread::Builder::new()
        .name("bt-webui-events".to_owned())
        .spawn(move || collect_events(thread_state, config))
    {
        let message = format!("启动 WebUI 事件线程失败: {error}");
        set_error(&state, message);
    }
}

fn collect_events(state: Arc<Mutex<EventState>>, config: WebuiEventsConfig) {
    let client = match SocketIpcClient::connect() {
        Ok(client) => client,
        Err(error) => {
            set_error(&state, format!("连接 binder-trace 内核事件流失败: {error}"));
            return;
        }
    };

    match client.get_feature() {
        Ok(feature) if feature.has_event_stream() => {}
        Ok(_) => {
            set_error(&state, "当前内核模块不支持 socket 事件流".to_owned());
            return;
        }
        Err(error) => {
            set_error(&state, format!("读取内核模块特性失败: {error}"));
            return;
        }
    }

    if let Some(capture_config) = config.capture_config {
        if let Err(error) = client.set_config(capture_config) {
            set_error(&state, format!("设置 WebUI 捕获配置失败: {error}"));
            return;
        }
        let _ = client.clear_stats();
    }

    {
        let mut state = state.lock().expect("event state mutex should not poison");
        state.source_state = SourceState::Connected;
        state.error = None;
    }

    let platform_methods = config.android_sdk.map(AndroidPlatformMethods::new);
    loop {
        match client.poll_event(POLL_TIMEOUT) {
            Ok(true) => drain_events(&client, &state, platform_methods),
            Ok(false) => {}
            Err(error) => {
                set_error(&state, format!("等待内核事件失败: {error}"));
                return;
            }
        }
    }
}

fn drain_events(
    client: &SocketIpcClient,
    state: &Arc<Mutex<EventState>>,
    platform_methods: Option<AndroidPlatformMethods>,
) {
    loop {
        let event = match client.try_recv_event() {
            Ok(Some(event)) => event,
            Ok(None) => return,
            Err(error) => {
                set_error(state, format!("读取内核事件失败: {error}"));
                return;
            }
        };

        if !event.is_binder_transaction() {
            continue;
        }

        let mut state = state.lock().expect("event state mutex should not poison");
        let api_event = build_api_event(&event, &mut state, platform_methods);
        state.push_event(api_event, event.lost_before);
    }
}

fn build_api_event(
    event: &BinderEvent,
    state: &mut EventState,
    platform_methods: Option<AndroidPlatformMethods>,
) -> ApiTraceEvent {
    let debug_id = debug_id(event);
    let reply_to_debug_id =
        (event.reply_to_debug_id != 0).then_some(u64::from(event.reply_to_debug_id));
    let process_label = state.process_label(event.tgid);
    let (interface, method, latency_us) = if event.is_reply() {
        reply_enrichment(event, state)
    } else {
        call_enrichment(event, state, platform_methods)
    };
    let status = classify_status(event, latency_us);

    if let Some(reply_to_debug_id) = reply_to_debug_id {
        update_call_latency(state, reply_to_debug_id, latency_us, status);
    }

    ApiTraceEvent {
        raw: RawTraceRecord {
            device_id: "android",
            seq: event.sequence,
            timestamp_ns: event.timestamp_ns,
            object: if event.is_reply() {
                "binder.reply"
            } else {
                "binder.transaction"
            },
            data: RawRecordData {
                kind: if event.is_reply() {
                    "reply"
                } else {
                    "transaction"
                },
                binder_device: "binder",
                process: RawRecordProcess {
                    pid: event.tgid,
                    tid: event.pid,
                    uid: event.uid,
                },
                flags: 0,
                sequence: event.sequence,
                transaction: RawRecordTransaction {
                    code: event.code,
                    flags: event.flags,
                    data_size: event.data_size,
                    offsets_size: event.offsets_size,
                    target_handle: event.target_handle,
                    sender_pid: event.sender_pid,
                    sender_euid: event.sender_euid,
                    payload_truncated: event.payload_truncated != 0,
                    payload_hex: payload_hex(event.payload_bytes()),
                },
            },
        },
        enrichment: ApiTraceEnrichment {
            interface,
            method,
            process_label,
            debug_id,
            reply_to_debug_id,
            latency_us,
            status,
        },
    }
}

fn call_enrichment(
    event: &BinderEvent,
    state: &mut EventState,
    platform_methods: Option<AndroidPlatformMethods>,
) -> (String, String, Option<u64>) {
    let interface = parse_interface_token(event.payload_bytes())
        .unwrap_or_else(|| format!("handle#{}", event.target_handle));
    let method = platform_methods
        .and_then(|methods| methods.lookup(&interface, event.code))
        .map(|method| method.method.to_owned())
        .unwrap_or_else(|| format!("code_{}", event.code));

    state.call_summaries.insert(
        debug_id(event),
        CallSummary {
            interface: interface.clone(),
            method: method.clone(),
            timestamp_ns: event.timestamp_ns,
        },
    );

    (interface, method, None)
}

fn reply_enrichment(event: &BinderEvent, state: &EventState) -> (String, String, Option<u64>) {
    let Some(summary) = state
        .call_summaries
        .get(&u64::from(event.reply_to_debug_id))
    else {
        return (
            format!("reply_to#{}", event.reply_to_debug_id),
            format!("code_{}", event.code),
            None,
        );
    };

    let latency_us = event
        .timestamp_ns
        .saturating_sub(summary.timestamp_ns)
        .checked_div(1_000);

    (
        summary.interface.clone(),
        summary.method.clone(),
        latency_us,
    )
}

fn update_call_latency(
    state: &mut EventState,
    debug_id: u64,
    latency_us: Option<u64>,
    status: &'static str,
) {
    let Some(latency_us) = latency_us else {
        return;
    };

    for event in state.events.iter_mut().rev() {
        if event.enrichment.debug_id == debug_id {
            event.enrichment.latency_us = Some(latency_us);
            event.enrichment.status = status;
            return;
        }
    }
}

fn classify_status(event: &BinderEvent, latency_us: Option<u64>) -> &'static str {
    if event.lost_before > 0 {
        return "error";
    }
    if latency_us.is_some_and(|latency_us| latency_us >= SLOW_TRANSACTION_US) {
        return "slow";
    }

    "ok"
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum EventDirection {
    Call,
    Reply,
    Oneway,
}

fn event_direction(event: &ApiTraceEvent) -> EventDirection {
    if event.raw.data.kind == "reply" {
        return EventDirection::Reply;
    }
    if (event.raw.data.transaction.flags & TF_ONE_WAY) == TF_ONE_WAY {
        return EventDirection::Oneway;
    }
    EventDirection::Call
}

fn normalized_scalar_filter(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value != "all").then(|| value.to_owned())
}

fn event_matches_needle(event: &ApiTraceEvent, needle: &str) -> bool {
    number_contains(event.raw.seq, needle)
        || number_contains(event.enrichment.debug_id, needle)
        || number_contains(event.raw.data.process.pid, needle)
        || number_contains(event.raw.data.process.tid, needle)
        || number_contains(event.raw.data.process.uid, needle)
        || text_contains(&event.enrichment.process_label, needle)
        || text_contains(&event.enrichment.interface, needle)
        || text_contains(&event.enrichment.method, needle)
        || number_contains(event.raw.data.transaction.code, needle)
        || text_contains(event.raw.data.binder_device, needle)
        || text_contains(event_direction_label(event), needle)
        || text_contains(event.enrichment.status, needle)
        || flags_contain(event.raw.data.transaction.flags, needle)
}

fn number_contains(value: impl ToString, needle: &str) -> bool {
    value.to_string().contains(needle)
}

fn text_contains(value: &str, needle: &str) -> bool {
    value.to_lowercase().contains(needle)
}

fn event_direction_label(event: &ApiTraceEvent) -> &'static str {
    match event_direction(event) {
        EventDirection::Call => "call",
        EventDirection::Reply => "reply",
        EventDirection::Oneway => "oneway",
    }
}

fn flags_contain(flags: u32, needle: &str) -> bool {
    const FLAG_TABLE: &[(u32, &str)] = &[
        (TF_ONE_WAY, "one_way"),
        (0x04, "root_object"),
        (0x08, "status_code_pending"),
        (0x10, "accept_fds"),
        (0x20, "clear_buf"),
    ];

    FLAG_TABLE
        .iter()
        .any(|(bit, label)| (flags & bit) == *bit && label.contains(needle))
}

fn debug_id(event: &BinderEvent) -> u64 {
    if event.transaction_debug_id == 0 {
        event.sequence
    } else {
        u64::from(event.transaction_debug_id)
    }
}

fn payload_hex(payload: &[u8]) -> String {
    let mut out = String::with_capacity(payload.len() * 2);
    for byte in payload {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn read_process_label(tgid: u32) -> Option<String> {
    let bytes = fs::read(format!("/proc/{tgid}/cmdline")).ok()?;
    let name = bytes
        .split(|byte| *byte == 0)
        .find(|part| !part.is_empty())
        .and_then(|part| std::str::from_utf8(part).ok())?
        .trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn set_error(state: &Arc<Mutex<EventState>>, message: String) {
    let mut state = state.lock().expect("event state mutex should not poison");
    state.source_state = SourceState::Error;
    state.error = Some(message);
}
