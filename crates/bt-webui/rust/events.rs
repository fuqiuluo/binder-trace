//! WebUI 生产事件源与 JSON DTO。
//!
//! # 职责
//! - 后台读取内核 socket 事件流，并维护一个后端历史窗口。
//! - 把 `bt-agent` 的固定 UAPI 事件转换成前端现有 normalized model 可消费的 JSON。
//! - 在 Rust 层完成筛选与窗口裁剪，避免浏览器承担后端职责。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use bt_agent::{BinderEvent, CaptureConfig, CaptureHistory, HistoryError, SocketIpcClient};
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebuiEventsConfig {
    /// 是否启动生产事件源；测试静态资源服务时可关闭。
    pub enabled: bool,
    /// WebUI 是否负责开启内核捕获配置；`None` 表示只读现有事件流。
    pub capture_config: Option<CaptureConfig>,
    /// Android SDK 版本，用于平台 Binder 方法名解析。
    pub android_sdk: Option<u16>,
    /// btcap 历史文件初始事件容量，满后按需扩容。
    pub max_events: usize,
    /// WebUI btcap 历史文件路径。
    pub history_path: Option<PathBuf>,
    /// WebUI btcap 历史文件最大字节数。
    pub max_history_bytes: u64,
}

impl Default for WebuiEventsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            capture_config: None,
            android_sdk: None,
            max_events: DEFAULT_MAX_EVENTS,
            history_path: None,
            max_history_bytes: CaptureHistory::DEFAULT_MAX_FILE_BYTES,
        }
    }
}

/// 共享事件窗口。
#[derive(Clone)]
pub(crate) struct WebuiEventHub {
    state: Arc<Mutex<EventState>>,
}

impl WebuiEventHub {
    pub(crate) fn new(config: WebuiEventsConfig) -> Self {
        let event_state = EventState::new(config.clone());
        let should_spawn_collector = event_state.can_collect();
        let state = Arc::new(Mutex::new(event_state));
        let hub = Self {
            state: Arc::clone(&state),
        };

        if should_spawn_collector {
            spawn_collector(state, config);
        }

        hub
    }

    pub(crate) fn snapshot(&self, query: &EventsQuery) -> EventsResponse {
        let mut state = self
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
}

struct EventState {
    history: Option<CaptureHistory>,
    call_summaries: BTreeMap<u64, CallSummary>,
    reply_latencies: HashMap<u64, u64>,
    process_labels: HashMap<u32, String>,
    android_methods: Option<AndroidPlatformMethods>,
    kernel_lost_count: u64,
    backend_evicted_count: u64,
    source_state: SourceState,
    error: Option<String>,
}

impl EventState {
    fn new(config: WebuiEventsConfig) -> Self {
        let mut state = Self {
            history: None,
            call_summaries: BTreeMap::new(),
            reply_latencies: HashMap::new(),
            process_labels: HashMap::new(),
            android_methods: config.android_sdk.map(AndroidPlatformMethods::new),
            kernel_lost_count: 0,
            backend_evicted_count: 0,
            source_state: if config.enabled {
                SourceState::Connecting
            } else {
                SourceState::Disabled
            },
            error: None,
        };

        if config.enabled {
            let history_path = config
                .history_path
                .unwrap_or_else(default_webui_history_path);
            match CaptureHistory::create_with_max_file_bytes(
                history_path,
                config.max_events.max(1),
                config.max_history_bytes,
            ) {
                Ok(history) => {
                    state.history = Some(history);
                }
                Err(error) => {
                    state.source_state = SourceState::Error;
                    state.error = Some(format!("创建 WebUI btcap 历史失败: {error}"));
                }
            }
        }

        state
    }

    fn can_collect(&self) -> bool {
        self.source_state == SourceState::Connecting && self.history.is_some()
    }

    fn snapshot(&mut self, query: &EventsQuery) -> EventsResponse {
        let filter = EventFilter::from(query);
        let limit = query.limit();
        let selected = match self.select_events(&filter, query.page_request(), limit) {
            Ok(selected) => selected,
            Err(error) => {
                self.source_state = SourceState::Error;
                self.error = Some(format!("读取 WebUI btcap 历史失败: {error}"));
                SelectedPage::default()
            }
        };
        let event_count = self.event_count();
        let retained_count = usize::try_from(event_count).unwrap_or(usize::MAX);
        let oldest_seq = selected.events.first().map(|event| event.raw.seq);
        let newest_seq = selected.events.last().map(|event| event.raw.seq);
        let matched_count = selected.matched_count;
        let interfaces = self.interface_options();

        EventsResponse {
            events: selected.events,
            total_count: event_count,
            retained_count,
            matched_count,
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
                .is_some_and(|end_index| end_index < matched_count),
            interfaces,
            source_state: self.source_state,
            error: self.error.clone(),
            device_context: "production · binder transaction stream",
        }
    }

    fn clear(&mut self) {
        if let Some(history) = self.history.as_mut() {
            if let Err(error) = history.clear() {
                self.source_state = SourceState::Error;
                self.error = Some(format!("清空 WebUI btcap 历史失败: {error}"));
            }
        }
        self.call_summaries.clear();
        self.reply_latencies.clear();
        self.kernel_lost_count = 0;
        self.backend_evicted_count = 0;
    }

    fn push_event(&mut self, event: BinderEvent) -> Result<(), HistoryError> {
        let Some(history) = self.history.as_mut() else {
            return Ok(());
        };

        history.append_event(event)?;
        self.kernel_lost_count = self
            .kernel_lost_count
            .saturating_add(u64::from(event.lost_before));
        self.observe_event(&event);
        Ok(())
    }

    fn observe_event(&mut self, event: &BinderEvent) {
        let debug_id = debug_id(event);
        if event.is_reply() {
            if let Some(latency_us) = self.reply_latency(event) {
                self.reply_latencies
                    .insert(u64::from(event.reply_to_debug_id), latency_us);
            }
            return;
        }

        let (interface, method) = self.call_target(event);
        self.call_summaries.insert(
            debug_id,
            CallSummary {
                interface,
                method,
                timestamp_ns: event.timestamp_ns,
            },
        );
    }

    fn event_count(&self) -> u64 {
        self.history.as_ref().map_or(0, CaptureHistory::event_count)
    }

    fn process_label(&mut self, tgid: u32) -> String {
        if let Some(label) = self.process_labels.get(&tgid) {
            return label.clone();
        }

        let label = read_process_label(tgid).unwrap_or_else(|| format!("pid:{tgid}"));
        self.process_labels.insert(tgid, label.clone());
        label
    }

    fn interface_options(&mut self) -> Vec<String> {
        let mut counts = HashMap::<String, usize>::new();
        for index in 0..self.event_count() {
            let event = match self.event_at(index) {
                Ok(event) => event,
                Err(error) => {
                    self.source_state = SourceState::Error;
                    self.error = Some(format!("读取 WebUI interface 列表失败: {error}"));
                    break;
                }
            };
            let interface = if event.is_reply() {
                self.reply_enrichment(&event).0
            } else {
                self.call_target(&event).0
            };
            *counts.entry(interface).or_default() += 1;
        }

        let mut entries = counts.into_iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        entries
            .into_iter()
            .take(MAX_INTERFACE_OPTIONS)
            .map(|entry| entry.0)
            .collect()
    }

    fn select_events(
        &mut self,
        filter: &EventFilter,
        request: PageRequest,
        limit: usize,
    ) -> Result<SelectedPage, HistoryError> {
        let mut selected = VecDeque::<SelectedEvent>::with_capacity(limit);
        let mut matched_count = 0_u64;
        let event_count = self.event_count();

        for index in 0..event_count {
            let event = self.event_at(index)?;
            let api_event = self.api_event(&event);
            if !filter.matches(&api_event) {
                continue;
            }

            matched_count += 1;
            if !request.matches(api_event.raw.seq) {
                continue;
            }

            match request {
                PageRequest::Latest | PageRequest::Before(_) => {
                    selected.push_back(SelectedEvent {
                        ordinal: matched_count,
                        event: api_event,
                    });
                    while selected.len() > limit {
                        selected.pop_front();
                    }
                }
                PageRequest::After(_) => {
                    if selected.len() < limit {
                        selected.push_back(SelectedEvent {
                            ordinal: matched_count,
                            event: api_event,
                        });
                    }
                }
            }
        }

        let start_index = selected.front().map(|selected| selected.ordinal);
        let end_index = selected.back().map(|selected| selected.ordinal);
        let events = selected
            .into_iter()
            .map(|selected| selected.event)
            .collect::<Vec<_>>();

        Ok(SelectedPage {
            events,
            start_index,
            end_index,
            matched_count,
        })
    }

    fn event_at(&self, index: u64) -> Result<BinderEvent, HistoryError> {
        self.history
            .as_ref()
            .ok_or_else(|| HistoryError::Io(std::io::Error::other("WebUI btcap 历史未启用")))?
            .event_at(index)
            .map_err(HistoryError::Io)
    }

    fn api_event(&mut self, event: &BinderEvent) -> ApiTraceEvent {
        let debug_id = debug_id(event);
        let reply_to_debug_id =
            (event.reply_to_debug_id != 0).then_some(u64::from(event.reply_to_debug_id));
        let process_label = self.process_label(event.tgid);
        let (interface, method, latency_us) = if event.is_reply() {
            let (interface, method) = self.reply_enrichment(event);
            (interface, method, self.reply_latency(event))
        } else {
            let (interface, method) = self.call_target(event);
            let latency_us = self.reply_latencies.get(&debug_id).copied();
            (interface, method, latency_us)
        };
        let status = classify_status(event, latency_us);

        api_trace_event(
            event,
            interface,
            method,
            process_label,
            debug_id,
            reply_to_debug_id,
            latency_us,
            status,
        )
    }

    fn call_target(&self, event: &BinderEvent) -> (String, String) {
        let interface = parse_interface_token(event.payload_bytes())
            .unwrap_or_else(|| format!("handle#{}", event.target_handle));
        let method = self
            .android_methods
            .and_then(|methods| methods.lookup(&interface, event.code))
            .map(|method| method.method.to_owned())
            .unwrap_or_else(|| format!("code_{}", event.code));

        (interface, method)
    }

    fn reply_enrichment(&self, event: &BinderEvent) -> (String, String) {
        let Some(summary) = self.call_summaries.get(&u64::from(event.reply_to_debug_id)) else {
            return (
                format!("reply_to#{}", event.reply_to_debug_id),
                format!("code_{}", event.code),
            );
        };

        (summary.interface.clone(), summary.method.clone())
    }

    fn reply_latency(&self, event: &BinderEvent) -> Option<u64> {
        self.call_summaries
            .get(&u64::from(event.reply_to_debug_id))
            .and_then(|summary| {
                event
                    .timestamp_ns
                    .saturating_sub(summary.timestamp_ns)
                    .checked_div(1_000)
            })
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

impl PageRequest {
    fn matches(self, seq: u64) -> bool {
        match self {
            Self::Latest => true,
            Self::Before(before_seq) => seq < before_seq,
            Self::After(after_seq) => seq > after_seq,
        }
    }
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

#[derive(Debug, Default)]
struct SelectedPage {
    events: Vec<ApiTraceEvent>,
    start_index: Option<u64>,
    end_index: Option<u64>,
    matched_count: u64,
}

#[derive(Debug)]
struct SelectedEvent {
    ordinal: u64,
    event: ApiTraceEvent,
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

    loop {
        match client.poll_event(POLL_TIMEOUT) {
            Ok(true) => drain_events(&client, &state),
            Ok(false) => {}
            Err(error) => {
                set_error(&state, format!("等待内核事件失败: {error}"));
                return;
            }
        }
    }
}

fn drain_events(client: &SocketIpcClient, state: &Arc<Mutex<EventState>>) {
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
        if let Err(error) = state.push_event(event) {
            let message = format!("写入 WebUI btcap 历史失败: {error}");
            if matches!(error, HistoryError::CapacityLimit { .. }) {
                let _ = client.set_config(CaptureConfig::disabled());
            }
            state.source_state = SourceState::Error;
            state.error = Some(message);
            return;
        }
    }
}

fn api_trace_event(
    event: &BinderEvent,
    interface: String,
    method: String,
    process_label: String,
    debug_id: u64,
    reply_to_debug_id: Option<u64>,
    latency_us: Option<u64>,
    status: &'static str,
) -> ApiTraceEvent {
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

fn default_webui_history_path() -> PathBuf {
    let android_tmp = Path::new("/data/local/tmp");
    if android_tmp.is_dir() {
        android_tmp.join("binder-trace/webui-events.btcap")
    } else {
        PathBuf::from("binder-trace-webui.btcap")
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn snapshot_reads_old_events_from_btcap_history() {
        let path = temp_path("webui-history");
        let mut state = EventState::new(WebuiEventsConfig {
            enabled: true,
            capture_config: None,
            android_sdk: None,
            max_events: 1,
            history_path: Some(path.clone()),
            max_history_bytes: CaptureHistory::DEFAULT_MAX_FILE_BYTES,
        });

        for sequence in 1..=5 {
            state
                .push_event(test_event(sequence))
                .expect("测试事件应写入 WebUI btcap");
        }

        let latest = state.snapshot(&EventsQuery {
            limit: Some(2),
            ..EventsQuery::default()
        });
        assert_eq!(latest.total_count, 5);
        assert_eq!(latest.retained_count, 5);
        assert_eq!(latest.backend_evicted_count, 0);
        assert_eq!(event_sequences(&latest), vec![4, 5]);
        assert!(latest.has_more_before);

        let older = state.snapshot(&EventsQuery {
            limit: Some(2),
            before_seq: Some(4),
            ..EventsQuery::default()
        });
        assert_eq!(event_sequences(&older), vec![2, 3]);
        assert_eq!(older.matched_count, 5);

        let _ = fs::remove_file(path);
    }

    fn event_sequences(response: &EventsResponse) -> Vec<u64> {
        response.events.iter().map(|event| event.raw.seq).collect()
    }

    fn temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "binder-trace-{name}-{}-{nanos}.btcap",
            std::process::id()
        ))
    }

    fn test_event(sequence: u64) -> BinderEvent {
        BinderEvent {
            sequence,
            timestamp_ns: sequence * 1_000,
            kind: 1,
            pid: 100,
            tgid: 100,
            uid: 0,
            reply: 0,
            lost_before: 0,
            transaction_debug_id: sequence as u32,
            reply_to_debug_id: 0,
            transaction: 0,
            proc: 0,
            thread: 0,
            extra_buffers_size: 0,
            code: sequence as u32,
            flags: 0,
            data_size: 0,
            offsets_size: 0,
            target_handle: 1,
            sender_pid: 0,
            sender_euid: 0,
            payload_len: 0,
            payload_truncated: 0,
            reserved: [0; 7],
            payload: [0; 256],
        }
    }
}
