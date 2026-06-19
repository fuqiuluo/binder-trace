use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::Duration,
};

use bt_agent::{
    BinderEvent, CaptureConfig, CaptureHistory, CaptureStats, DriverFeature, HistoryError,
    SocketIpcClient,
};
use bt_decoder::{AndroidPlatformMethods, parse_interface_token};
use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::{Json, Parameters},
    schemars, tool, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::McpServerError;

const DEFAULT_INITIAL_EVENTS: usize = 65_536;
const DEFAULT_QUERY_LIMIT: usize = 64;
const MAX_QUERY_LIMIT: usize = 512;
const POLL_TIMEOUT: Duration = Duration::from_millis(100);
const SLOW_TRANSACTION_US: u64 = 16_000;
const TF_ONE_WAY: u32 = 0x01;
const MCP_ENDPOINT_PATH: &str = "/mcp";

/// MCP 在线服务配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpServerConfig {
    /// HTTP 监听地址。
    pub listen: SocketAddr,
    /// btcap 历史文件初始事件容量；满后会自动扩容。
    pub initial_events: usize,
    /// btcap 历史文件路径；未指定时使用 MCP 专用默认路径。
    pub history_path: Option<PathBuf>,
    /// btcap 历史文件最大字节数。
    pub max_history_bytes: u64,
    /// 是否允许 MCP tool 修改内核捕获配置。
    pub allow_control: bool,
    /// 启动 MCP 服务时是否立即开启 Binder transaction 捕获。
    pub auto_enable: bool,
    /// 自动开启和 `binder_trace_enable` 默认使用的捕获配置。
    pub capture_config: CaptureConfig,
    /// Android SDK 版本，用于把平台 Binder code 映射成方法名。
    pub android_sdk: Option<u16>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 5174)),
            initial_events: DEFAULT_INITIAL_EVENTS,
            history_path: None,
            max_history_bytes: CaptureHistory::DEFAULT_MAX_FILE_BYTES,
            allow_control: false,
            auto_enable: false,
            capture_config: CaptureConfig::binder_transaction_enabled(),
            android_sdk: None,
        }
    }
}

/// 启动 Streamable HTTP MCP 服务并阻塞当前线程。
pub fn serve_http_blocking(config: McpServerConfig) -> Result<(), McpServerError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    runtime.block_on(serve_http(config))
}

async fn serve_http(config: McpServerConfig) -> Result<(), McpServerError> {
    let listen = config.listen;
    let server = LiveMcpServer::connect(config)?;
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service(MCP_ENDPOINT_PATH, service);
    let listener = tokio::net::TcpListener::bind(listen).await?;

    axum::serve(listener, router).await?;
    Ok(())
}

#[derive(Clone)]
struct LiveMcpServer {
    state: Arc<LiveState>,
}

#[tool_router(server_handler)]
impl LiveMcpServer {
    fn connect(config: McpServerConfig) -> Result<Self, McpServerError> {
        let client = SocketIpcClient::connect()?;
        let feature = client.get_feature()?;
        if !feature.has_event_stream() {
            return Err(McpServerError::EventStreamUnsupported);
        }

        if config.auto_enable {
            client.set_config(config.capture_config)?;
            client.clear_stats()?;
        }

        let state = Arc::new(LiveState::new(client, feature, config)?);
        spawn_collector(Arc::clone(&state));
        Ok(Self { state })
    }

    #[tool(
        description = "Return live binder-trace MCP status, driver feature, capture config, stats, and btcap history metadata."
    )]
    fn binder_trace_status(&self) -> Result<Json<StatusResponse>, McpError> {
        let (family, config, stats) = {
            let client = self.client()?;
            let config = client.get_config().map_err(socket_tool_error)?;
            let stats = client.get_stats().map_err(socket_tool_error)?;
            (client.family(), config, stats)
        };
        let source = self.source_snapshot()?;
        let history = self.store()?.history_response();

        Ok(Json(StatusResponse {
            family,
            allow_control: self.state.allow_control,
            feature: FeatureResponse::from(self.state.feature),
            capture_config: CaptureConfigResponse::from(config),
            stats: CaptureStatsResponse::from(stats),
            source_state: source.state,
            error: source.error,
            history,
        }))
    }

    #[tool(
        description = "Enable Binder transaction tracing with optional tgid, pid, uid, and payload-size filters. Requires --allow-control."
    )]
    fn binder_trace_enable(
        &self,
        Parameters(params): Parameters<EnableCaptureParams>,
    ) -> Result<Json<ControlResponse>, McpError> {
        self.ensure_control()?;
        let config = params.capture_config(self.state.default_capture_config);
        let (config, stats) = {
            let client = self.client()?;
            client.set_config(config).map_err(socket_tool_error)?;
            client.clear_stats().map_err(socket_tool_error)?;
            let config = client.get_config().map_err(socket_tool_error)?;
            let stats = client.get_stats().map_err(socket_tool_error)?;
            (config, stats)
        };

        Ok(Json(ControlResponse {
            capture_config: CaptureConfigResponse::from(config),
            stats: CaptureStatsResponse::from(stats),
        }))
    }

    #[tool(description = "Disable Binder tracing in the kernel module. Requires --allow-control.")]
    fn binder_trace_disable(&self) -> Result<Json<ControlResponse>, McpError> {
        self.ensure_control()?;
        let (config, stats) = {
            let client = self.client()?;
            client
                .set_config(CaptureConfig::disabled())
                .map_err(socket_tool_error)?;
            let config = client.get_config().map_err(socket_tool_error)?;
            let stats = client.get_stats().map_err(socket_tool_error)?;
            (config, stats)
        };

        Ok(Json(ControlResponse {
            capture_config: CaptureConfigResponse::from(config),
            stats: CaptureStatsResponse::from(stats),
        }))
    }

    #[tool(description = "Clear kernel-side binder-trace counters. Requires --allow-control.")]
    fn binder_trace_clear_stats(&self) -> Result<Json<CaptureStatsResponse>, McpError> {
        self.ensure_control()?;
        let stats = {
            let client = self.client()?;
            client.clear_stats().map_err(socket_tool_error)?;
            client.get_stats().map_err(socket_tool_error)?
        };

        Ok(Json(CaptureStatsResponse::from(stats)))
    }

    #[tool(
        description = "Return Binder events from the btcap history with optional direction, interface, text query, and sequence pagination filters."
    )]
    fn binder_trace_events(
        &self,
        Parameters(params): Parameters<EventsParams>,
    ) -> Result<Json<EventsResponse>, McpError> {
        let response = self.store()?.events(&params).map_err(history_tool_error)?;
        Ok(Json(response))
    }

    #[tool(
        description = "Return one Binder event from the btcap history by kernel sequence number."
    )]
    fn binder_trace_event(
        &self,
        Parameters(params): Parameters<EventParams>,
    ) -> Result<Json<EventLookupResponse>, McpError> {
        let event = self
            .store()?
            .event(params.seq)
            .map_err(history_tool_error)?;
        Ok(Json(EventLookupResponse { event }))
    }

    #[tool(description = "Return top Binder interfaces in the current btcap history.")]
    fn binder_trace_top_interfaces(
        &self,
        Parameters(params): Parameters<TopInterfacesParams>,
    ) -> Result<Json<TopInterfacesResponse>, McpError> {
        let response = self
            .store()?
            .top_interfaces(params.limit())
            .map_err(history_tool_error)?;
        Ok(Json(response))
    }

    #[tool(
        description = "Clear the MCP server's btcap history without touching kernel capture state."
    )]
    fn binder_trace_clear_history(&self) -> Result<Json<HistoryResponse>, McpError> {
        let mut store = self.store()?;
        store.clear().map_err(history_tool_error)?;
        Ok(Json(store.history_response()))
    }

    fn client(&self) -> Result<MutexGuard<'_, SocketIpcClient>, McpError> {
        self.state
            .client
            .lock()
            .map_err(|_| state_lock_error("client"))
    }

    fn store(&self) -> Result<MutexGuard<'_, EventStore>, McpError> {
        self.state
            .store
            .lock()
            .map_err(|_| state_lock_error("events"))
    }

    fn source_snapshot(&self) -> Result<SourceSnapshot, McpError> {
        self.state
            .source
            .lock()
            .map(|source| source.clone())
            .map_err(|_| state_lock_error("source"))
    }

    fn ensure_control(&self) -> Result<(), McpError> {
        if self.state.allow_control {
            Ok(())
        } else {
            Err(McpError::invalid_params(
                "binder-trace control tools are disabled; restart mcp with --allow-control",
                None,
            ))
        }
    }
}

struct LiveState {
    client: Mutex<SocketIpcClient>,
    feature: DriverFeature,
    allow_control: bool,
    default_capture_config: CaptureConfig,
    store: Mutex<EventStore>,
    source: Mutex<SourceSnapshot>,
}

impl LiveState {
    fn new(
        client: SocketIpcClient,
        feature: DriverFeature,
        config: McpServerConfig,
    ) -> Result<Self, McpServerError> {
        let history_path = config.history_path.unwrap_or_else(default_mcp_history_path);
        let store = EventStore::new(
            history_path,
            config.initial_events.max(1),
            config.max_history_bytes,
            config.android_sdk.map(AndroidPlatformMethods::new),
        )?;

        Ok(Self {
            client: Mutex::new(client),
            feature,
            allow_control: config.allow_control,
            default_capture_config: config.capture_config,
            store: Mutex::new(store),
            source: Mutex::new(SourceSnapshot {
                state: SourceState::Connecting,
                error: None,
            }),
        })
    }

    fn set_source_state(&self, state: SourceState, error: Option<String>) {
        match self.source.lock() {
            Ok(mut source) => {
                source.state = state;
                source.error = error;
            }
            Err(_) => {
                eprintln!("binder-trace mcp source state lock poisoned");
            }
        }
    }
}

fn default_mcp_history_path() -> PathBuf {
    let android_tmp = Path::new("/data/local/tmp");
    if android_tmp.is_dir() {
        android_tmp.join("binder-trace/mcp-events.btcap")
    } else {
        PathBuf::from("binder-trace-mcp.btcap")
    }
}

fn spawn_collector(state: Arc<LiveState>) {
    let thread_state = Arc::clone(&state);
    if let Err(error) = thread::Builder::new()
        .name("bt-mcp-events".to_owned())
        .spawn(move || collect_events(thread_state))
    {
        state.set_source_state(
            SourceState::Error,
            Some(format!("启动 MCP 事件线程失败: {error}")),
        );
    }
}

fn collect_events(state: Arc<LiveState>) {
    state.set_source_state(SourceState::Connected, None);

    loop {
        match poll_events(&state) {
            Ok(true) => drain_events(&state),
            Ok(false) => {}
            Err(error) => {
                state.set_source_state(SourceState::Error, Some(error));
                return;
            }
        }
    }
}

fn poll_events(state: &LiveState) -> Result<bool, String> {
    state
        .client
        .lock()
        .map_err(|_| "MCP client lock poisoned".to_owned())?
        .poll_event(POLL_TIMEOUT)
        .map_err(|error| format!("等待内核事件失败: {error}"))
}

fn drain_events(state: &LiveState) {
    loop {
        let event = match recv_event(state) {
            Ok(Some(event)) => event,
            Ok(None) => return,
            Err(error) => {
                state.set_source_state(SourceState::Error, Some(error));
                return;
            }
        };

        if !event.is_binder_transaction() {
            continue;
        }

        match state.store.lock() {
            Ok(mut store) => {
                if let Err(error) = store.push(event) {
                    handle_history_write_error(state, error);
                    return;
                }
            }
            Err(_) => {
                state.set_source_state(
                    SourceState::Error,
                    Some("MCP events lock poisoned".to_owned()),
                );
                return;
            }
        }
    }
}

fn recv_event(state: &LiveState) -> Result<Option<BinderEvent>, String> {
    state
        .client
        .lock()
        .map_err(|_| "MCP client lock poisoned".to_owned())?
        .try_recv_event()
        .map_err(|error| format!("读取内核事件失败: {error}"))
}

fn handle_history_write_error(state: &LiveState, error: HistoryError) {
    let should_disable_capture = matches!(error, HistoryError::CapacityLimit { .. });
    let message = format!("写入 MCP btcap 历史失败: {error}");

    if should_disable_capture {
        match state.client.lock() {
            Ok(client) => {
                if let Err(disable_error) = client.set_config(CaptureConfig::disabled()) {
                    state.set_source_state(
                        SourceState::Error,
                        Some(format!("{message}; 自动关闭捕获失败: {disable_error}")),
                    );
                    return;
                }
            }
            Err(_) => {
                state.set_source_state(
                    SourceState::Error,
                    Some(format!(
                        "{message}; 自动关闭捕获失败: MCP client lock poisoned"
                    )),
                );
                return;
            }
        }
    }

    state.set_source_state(SourceState::Error, Some(message));
}

struct EventStore {
    history: CaptureHistory,
    history_path: PathBuf,
    seq_to_index: HashMap<u64, u64>,
    call_summaries: HashMap<u64, CallSummary>,
    reply_latencies: HashMap<u64, u64>,
    android_methods: Option<AndroidPlatformMethods>,
    lost_count: u64,
    oldest_seq: Option<u64>,
    newest_seq: Option<u64>,
}

impl EventStore {
    fn new(
        history_path: PathBuf,
        initial_events: usize,
        max_history_bytes: u64,
        android_methods: Option<AndroidPlatformMethods>,
    ) -> Result<Self, HistoryError> {
        Ok(Self {
            history: CaptureHistory::create_with_max_file_bytes(
                history_path.clone(),
                initial_events,
                max_history_bytes,
            )?,
            history_path,
            seq_to_index: HashMap::new(),
            call_summaries: HashMap::new(),
            reply_latencies: HashMap::new(),
            android_methods,
            lost_count: 0,
            oldest_seq: None,
            newest_seq: None,
        })
    }

    fn push(&mut self, event: BinderEvent) -> Result<(), HistoryError> {
        let index = self.history.append_event(event)?;
        let debug_id = debug_id(&event);
        self.seq_to_index.insert(event.sequence, index);
        self.lost_count = self.lost_count.saturating_add(u64::from(event.lost_before));
        self.oldest_seq.get_or_insert(event.sequence);
        self.newest_seq = Some(event.sequence);

        match EventDirection::from_event(&event) {
            EventDirection::Call | EventDirection::Oneway => {
                let (interface, method) = self.call_summary_for_event(&event);
                self.call_summaries.insert(
                    debug_id,
                    CallSummary {
                        interface,
                        method,
                        timestamp_ns: event.timestamp_ns,
                    },
                );
            }
            EventDirection::Reply => {
                if let Some(latency_us) = self.reply_latency(&event) {
                    self.reply_latencies
                        .insert(u64::from(event.reply_to_debug_id), latency_us);
                }
            }
        }

        Ok(())
    }

    fn clear(&mut self) -> Result<(), HistoryError> {
        self.history.clear()?;
        self.seq_to_index.clear();
        self.call_summaries.clear();
        self.reply_latencies.clear();
        self.lost_count = 0;
        self.oldest_seq = None;
        self.newest_seq = None;
        Ok(())
    }

    fn trace_event(&self, event: BinderEvent) -> TraceEvent {
        let debug_id = debug_id(&event);
        let reply_to_debug_id =
            (event.reply_to_debug_id != 0).then_some(u64::from(event.reply_to_debug_id));
        let direction = EventDirection::from_event(&event);
        let (interface, method) = match direction {
            EventDirection::Reply => self.reply_enrichment(&event),
            EventDirection::Call | EventDirection::Oneway => self.call_summary_for_event(&event),
        };
        let latency_us = match direction {
            EventDirection::Call | EventDirection::Oneway => {
                self.reply_latencies.get(&debug_id).copied()
            }
            EventDirection::Reply => self.reply_latency(&event),
        };

        TraceEvent {
            seq: event.sequence,
            timestamp_ns: event.timestamp_ns,
            direction,
            pid: event.pid,
            tgid: event.tgid,
            uid: event.uid,
            interface,
            method,
            code: event.code,
            flags: event.flags,
            target_handle: event.target_handle,
            sender_pid: event.sender_pid,
            sender_euid: event.sender_euid,
            data_size: event.data_size,
            offsets_size: event.offsets_size,
            payload_truncated: event.payload_truncated != 0,
            payload_hex: payload_hex(event.payload_bytes()),
            debug_id,
            reply_to_debug_id,
            latency_us,
            status: status_for(event.lost_before, latency_us),
            lost_before: event.lost_before,
        }
    }

    fn call_summary_for_event(&self, event: &BinderEvent) -> (String, String) {
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

    fn event(&self, seq: u64) -> Result<Option<TraceEvent>, HistoryError> {
        if let Some(index) = self.seq_to_index.get(&seq).copied() {
            return self
                .history
                .event_at(index)
                .map(|event| Some(self.trace_event(event)))
                .map_err(HistoryError::Io);
        }

        let mut found = None;
        self.history.for_each_event(|_, event| {
            if event.sequence == seq {
                found = Some(self.trace_event(event));
            }
            Ok(())
        })?;

        Ok(found)
    }

    fn events(&self, params: &EventsParams) -> Result<EventsResponse, HistoryError> {
        let limit = params.limit();
        let filter = EventFilter::from(params);
        let selected = self.select_events(&filter, params.page_request(), limit)?;

        Ok(EventsResponse {
            events: selected.events,
            matched_count: selected.matched_count,
            page_start_index: selected.start_index,
            page_end_index: selected.end_index,
            has_more_before: selected.start_index.is_some_and(|index| index > 1),
            has_more_after: selected
                .end_index
                .is_some_and(|index| index < selected.matched_count),
            history: self.history_response(),
        })
    }

    fn top_interfaces(&self, limit: usize) -> Result<TopInterfacesResponse, HistoryError> {
        let mut counts = HashMap::<String, u64>::new();
        self.history.for_each_event(|_, event| {
            let event = self.trace_event(event);
            *counts.entry(event.interface.clone()).or_default() += 1;
            Ok(())
        })?;
        let mut interfaces = counts
            .into_iter()
            .map(|(interface, count)| InterfaceCount { interface, count })
            .collect::<Vec<_>>();
        interfaces.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.interface.cmp(&right.interface))
        });
        interfaces.truncate(limit);

        Ok(TopInterfacesResponse {
            interfaces,
            history: self.history_response(),
        })
    }

    fn select_events(
        &self,
        filter: &EventFilter,
        request: PageRequest,
        limit: usize,
    ) -> Result<SelectedEvents, HistoryError> {
        let mut selected = VecDeque::<SelectedEvent>::with_capacity(limit);
        let mut matched_count = 0_u64;

        self.history.for_each_event(|_, event| {
            let event = self.trace_event(event);
            if !filter.matches(&event) {
                return Ok(());
            }

            matched_count += 1;
            if !request.matches(event.seq) {
                return Ok(());
            }

            match request {
                PageRequest::Latest | PageRequest::Before(_) => {
                    selected.push_back(SelectedEvent {
                        ordinal: matched_count,
                        event,
                    });
                    while selected.len() > limit {
                        selected.pop_front();
                    }
                }
                PageRequest::After(_) => {
                    if selected.len() < limit {
                        selected.push_back(SelectedEvent {
                            ordinal: matched_count,
                            event,
                        });
                    }
                }
            }

            Ok(())
        })?;

        let start_index = selected.front().map(|selected| selected.ordinal);
        let end_index = selected.back().map(|selected| selected.ordinal);
        let events = selected
            .into_iter()
            .map(|selected| selected.event)
            .collect::<Vec<_>>();

        Ok(SelectedEvents {
            events,
            matched_count,
            start_index,
            end_index,
        })
    }

    fn history_response(&self) -> HistoryResponse {
        HistoryResponse {
            history_path: self.history_path.to_string_lossy().into_owned(),
            event_count: self.history.event_count(),
            capacity: self.history.capacity(),
            max_events: self.history.max_events(),
            max_file_bytes: self.history.max_file_bytes(),
            lost_count: self.lost_count,
            oldest_seq: self.oldest_seq,
            newest_seq: self.newest_seq,
        }
    }
}

#[derive(Debug, Clone)]
struct CallSummary {
    interface: String,
    method: String,
    timestamp_ns: u64,
}

#[derive(Debug, Clone)]
struct SelectedEvent {
    ordinal: u64,
    event: TraceEvent,
}

#[derive(Debug, Clone)]
struct EventFilter {
    direction: DirectionFilter,
    interface: Option<String>,
    since_ns: Option<u64>,
    until_ns: Option<u64>,
    query: String,
}

impl EventFilter {
    fn matches(&self, event: &TraceEvent) -> bool {
        if !self.direction.matches(event.direction) {
            return false;
        }
        if self
            .interface
            .as_deref()
            .is_some_and(|interface| interface != event.interface)
        {
            return false;
        }
        if self
            .since_ns
            .is_some_and(|since_ns| event.timestamp_ns < since_ns)
        {
            return false;
        }
        if self
            .until_ns
            .is_some_and(|until_ns| event.timestamp_ns > until_ns)
        {
            return false;
        }
        self.query.is_empty() || event_matches_query(event, &self.query)
    }
}

impl From<&EventsParams> for EventFilter {
    fn from(params: &EventsParams) -> Self {
        Self {
            direction: params.direction.unwrap_or_default(),
            interface: params
                .interface
                .as_deref()
                .map(str::trim)
                .filter(|interface| !interface.is_empty() && *interface != "all")
                .map(ToOwned::to_owned),
            since_ns: params.since_ns,
            until_ns: params.until_ns,
            query: params.query.as_deref().unwrap_or("").trim().to_lowercase(),
        }
    }
}

#[derive(Debug, Clone)]
struct SelectedEvents {
    events: Vec<TraceEvent>,
    matched_count: u64,
    start_index: Option<u64>,
    end_index: Option<u64>,
}

#[cfg(test)]
fn select_page(matched: &[&TraceEvent], request: PageRequest, limit: usize) -> SelectedEvents {
    let (start, end) = match request {
        PageRequest::Latest => {
            let end = matched.len();
            (end.saturating_sub(limit), end)
        }
        PageRequest::Before(seq) => {
            let end = matched.partition_point(|event| event.seq < seq);
            (end.saturating_sub(limit), end)
        }
        PageRequest::After(seq) => {
            let start = matched.partition_point(|event| event.seq <= seq);
            (start, start.saturating_add(limit).min(matched.len()))
        }
    };

    let events = matched[start..end]
        .iter()
        .map(|event| (*event).clone())
        .collect::<Vec<_>>();
    if events.is_empty() {
        return SelectedEvents {
            events,
            matched_count: matched.len() as u64,
            start_index: None,
            end_index: None,
        };
    }

    SelectedEvents {
        events,
        matched_count: matched.len() as u64,
        start_index: Some(start as u64 + 1),
        end_index: Some(end as u64),
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

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DirectionFilter {
    #[default]
    All,
    Call,
    Reply,
    Oneway,
}

impl DirectionFilter {
    fn matches(self, direction: EventDirection) -> bool {
        match self {
            Self::All => true,
            Self::Call => direction == EventDirection::Call,
            Self::Reply => direction == EventDirection::Reply,
            Self::Oneway => direction == EventDirection::Oneway,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum EventDirection {
    Call,
    Reply,
    Oneway,
}

impl EventDirection {
    fn from_event(event: &BinderEvent) -> Self {
        if event.is_reply() {
            return Self::Reply;
        }
        if (event.flags & TF_ONE_WAY) == TF_ONE_WAY {
            return Self::Oneway;
        }

        Self::Call
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct EnableCaptureParams {
    /// 只捕获指定进程组；0 表示不启用该过滤。
    tgid: Option<i32>,
    /// 只捕获指定线程；0 表示不启用该过滤。
    pid: Option<i32>,
    /// 只捕获指定 uid。
    uid: Option<u32>,
    /// 只捕获 data size 大于等于该字节数的事件；0 表示不启用该过滤。
    min_size: Option<u64>,
    /// 只捕获 data size 小于等于该字节数的事件；0 表示不启用该过滤。
    max_size: Option<u64>,
}

impl EnableCaptureParams {
    fn capture_config(&self, default_config: CaptureConfig) -> CaptureConfig {
        let mut config = default_config;
        config.enabled = 1;

        if let Some(tgid) = self.tgid {
            config.tgid = tgid;
        }
        if let Some(pid) = self.pid {
            config.pid = pid;
        }
        if let Some(uid) = self.uid {
            config.uid = uid;
            config.uid_enabled = 1;
        }
        if let Some(min_size) = self.min_size {
            config.min_size = min_size;
        }
        if let Some(max_size) = self.max_size {
            config.max_size = max_size;
        }

        config
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct EventsParams {
    /// 最大返回事件数量，会限制在 1..512。
    limit: Option<usize>,
    /// 返回 kernel sequence 大于该值的事件。
    after_seq: Option<u64>,
    /// 返回 kernel sequence 小于该值的事件。
    before_seq: Option<u64>,
    /// 返回 timestamp_ns 大于等于该值的事件。
    since_ns: Option<u64>,
    /// 返回 timestamp_ns 小于等于该值的事件。
    until_ns: Option<u64>,
    /// 方向过滤，可选 all、call、reply 或 oneway。
    direction: Option<DirectionFilter>,
    /// 精确匹配 Binder interface descriptor。
    interface: Option<String>,
    /// 在常用事件字段中做大小写不敏感的文本或数字查询。
    query: Option<String>,
}

impl EventsParams {
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

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct EventParams {
    /// Kernel event sequence 编号。
    seq: u64,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct TopInterfacesParams {
    /// 最大返回 interface 数量，会限制在 1..128。
    limit: Option<usize>,
}

impl TopInterfacesParams {
    fn limit(&self) -> usize {
        self.limit.unwrap_or(20).clamp(1, 128)
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct StatusResponse {
    family: i32,
    allow_control: bool,
    feature: FeatureResponse,
    capture_config: CaptureConfigResponse,
    stats: CaptureStatsResponse,
    source_state: SourceState,
    error: Option<String>,
    history: HistoryResponse,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct FeatureResponse {
    name: String,
    magic: u64,
    abi_version: u32,
    feature_flags: u32,
    event_stream: bool,
}

impl From<DriverFeature> for FeatureResponse {
    fn from(feature: DriverFeature) -> Self {
        Self {
            name: c_string_lossy(&feature.name),
            magic: feature.magic,
            abi_version: feature.abi_version,
            feature_flags: feature.feature_flags,
            event_stream: feature.has_event_stream(),
        }
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct CaptureConfigResponse {
    enabled: bool,
    point_mask: u32,
    tgid: i32,
    pid: i32,
    uid: u32,
    uid_enabled: bool,
    ioctl_cmd: u32,
    ioctl_cmd_enabled: bool,
    min_size: u64,
    max_size: u64,
}

impl From<CaptureConfig> for CaptureConfigResponse {
    fn from(config: CaptureConfig) -> Self {
        Self {
            enabled: config.enabled != 0,
            point_mask: config.point_mask,
            tgid: config.tgid,
            pid: config.pid,
            uid: config.uid,
            uid_enabled: config.uid_enabled != 0,
            ioctl_cmd: config.ioctl_cmd,
            ioctl_cmd_enabled: config.ioctl_cmd_enabled != 0,
            min_size: config.min_size,
            max_size: config.max_size,
        }
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct CaptureStatsResponse {
    ioctl_hits: u64,
    copy_to_user_hits: u64,
    transaction_hits: u64,
    captured: u64,
    filtered: u64,
}

impl From<CaptureStats> for CaptureStatsResponse {
    fn from(stats: CaptureStats) -> Self {
        Self {
            ioctl_hits: stats.ioctl_hits,
            copy_to_user_hits: stats.copy_to_user_hits,
            transaction_hits: stats.transaction_hits,
            captured: stats.captured,
            filtered: stats.filtered,
        }
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct ControlResponse {
    capture_config: CaptureConfigResponse,
    stats: CaptureStatsResponse,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct EventsResponse {
    events: Vec<TraceEvent>,
    matched_count: u64,
    page_start_index: Option<u64>,
    page_end_index: Option<u64>,
    has_more_before: bool,
    has_more_after: bool,
    history: HistoryResponse,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct EventLookupResponse {
    event: Option<TraceEvent>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct TopInterfacesResponse {
    interfaces: Vec<InterfaceCount>,
    history: HistoryResponse,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct InterfaceCount {
    interface: String,
    count: u64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct HistoryResponse {
    history_path: String,
    event_count: u64,
    capacity: u64,
    max_events: u64,
    max_file_bytes: u64,
    lost_count: u64,
    oldest_seq: Option<u64>,
    newest_seq: Option<u64>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct TraceEvent {
    seq: u64,
    timestamp_ns: u64,
    direction: EventDirection,
    pid: u32,
    tgid: u32,
    uid: u32,
    interface: String,
    method: String,
    code: u32,
    flags: u32,
    target_handle: u32,
    sender_pid: u32,
    sender_euid: u32,
    data_size: u64,
    offsets_size: u64,
    payload_truncated: bool,
    payload_hex: String,
    debug_id: u64,
    reply_to_debug_id: Option<u64>,
    latency_us: Option<u64>,
    status: EventStatus,
    lost_before: u32,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum EventStatus {
    Ok,
    Slow,
    Error,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct SourceSnapshot {
    state: SourceState,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SourceState {
    Connecting,
    Connected,
    Error,
}

fn status_for(lost_before: u32, latency_us: Option<u64>) -> EventStatus {
    if lost_before > 0 {
        return EventStatus::Error;
    }
    if latency_us.is_some_and(|latency_us| latency_us >= SLOW_TRANSACTION_US) {
        return EventStatus::Slow;
    }

    EventStatus::Ok
}

fn event_matches_query(event: &TraceEvent, query: &str) -> bool {
    number_contains(event.seq, query)
        || number_contains(event.debug_id, query)
        || number_contains(event.pid, query)
        || number_contains(event.tgid, query)
        || number_contains(event.uid, query)
        || text_contains(&event.interface, query)
        || text_contains(&event.method, query)
        || number_contains(event.code, query)
        || number_contains(event.target_handle, query)
        || text_contains(event.direction.label(), query)
        || text_contains(event.status.label(), query)
}

impl EventDirection {
    fn label(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Reply => "reply",
            Self::Oneway => "oneway",
        }
    }
}

impl EventStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Slow => "slow",
            Self::Error => "error",
        }
    }
}

fn number_contains(value: impl ToString, query: &str) -> bool {
    value.to_string().contains(query)
}

fn text_contains(value: &str, query: &str) -> bool {
    value.to_lowercase().contains(query)
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

fn c_string_lossy(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn socket_tool_error(error: impl ToString) -> McpError {
    McpError::internal_error(
        "binder-trace socket IPC failed",
        Some(json!({ "error": error.to_string() })),
    )
}

fn history_tool_error(error: impl ToString) -> McpError {
    McpError::internal_error(
        "binder-trace btcap history failed",
        Some(json!({ "error": error.to_string() })),
    )
}

fn state_lock_error(name: &'static str) -> McpError {
    McpError::internal_error(
        "binder-trace MCP state lock failed",
        Some(json!({ "lock": name })),
    )
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn event(sequence: u64, interface: &str, direction: EventDirection) -> TraceEvent {
        TraceEvent {
            seq: sequence,
            timestamp_ns: sequence * 1_000,
            direction,
            pid: 1,
            tgid: 1,
            uid: 1,
            interface: interface.to_owned(),
            method: "method".to_owned(),
            code: 1,
            flags: 0,
            target_handle: 1,
            sender_pid: 1,
            sender_euid: 1,
            data_size: 0,
            offsets_size: 0,
            payload_truncated: false,
            payload_hex: String::new(),
            debug_id: sequence,
            reply_to_debug_id: None,
            latency_us: None,
            status: EventStatus::Ok,
            lost_before: 0,
        }
    }

    #[test]
    fn select_latest_page_from_filtered_events() {
        let events = vec![
            event(1, "android.os.IFoo", EventDirection::Call),
            event(2, "android.os.IBar", EventDirection::Call),
            event(3, "android.os.IFoo", EventDirection::Reply),
        ];
        let matched = events
            .iter()
            .filter(|event| event.interface == "android.os.IFoo")
            .collect::<Vec<_>>();

        let selected = select_page(&matched, PageRequest::Latest, 8);

        assert_eq!(selected.events.len(), 2);
        assert_eq!(selected.events[0].seq, 1);
        assert_eq!(selected.events[1].seq, 3);
        assert_eq!(selected.start_index, Some(1));
        assert_eq!(selected.end_index, Some(2));
    }

    #[test]
    fn direction_filter_rejects_other_directions() {
        let filter = EventFilter {
            direction: DirectionFilter::Reply,
            interface: None,
            since_ns: None,
            until_ns: None,
            query: String::new(),
        };

        assert!(filter.matches(&event(1, "android.os.IFoo", EventDirection::Reply)));
        assert!(!filter.matches(&event(2, "android.os.IFoo", EventDirection::Call)));
    }

    #[test]
    fn event_store_reads_old_events_from_btcap_history() {
        let path = temp_path("mcp-retains-old-events");
        let mut store = EventStore::new(
            path.clone(),
            1,
            CaptureHistory::DEFAULT_MAX_FILE_BYTES,
            None,
        )
        .expect("MCP 历史应可创建");

        for sequence in 0..3 {
            store
                .push(raw_event(sequence))
                .expect("测试事件应写入 btcap");
        }

        let first = store
            .event(0)
            .expect("btcap 读取不应失败")
            .expect("旧事件仍应可按 seq 读取");
        assert_eq!(first.seq, 0);
        assert_eq!(store.history_response().event_count, 3);

        let response = store
            .events(&EventsParams {
                limit: Some(2),
                after_seq: Some(0),
                before_seq: None,
                since_ns: None,
                until_ns: None,
                direction: None,
                interface: None,
                query: None,
            })
            .expect("分页查询应可读取 btcap");
        assert_eq!(
            response
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn events_filter_by_timestamp_range() {
        let path = temp_path("mcp-time-range");
        let mut store = EventStore::new(
            path.clone(),
            1,
            CaptureHistory::DEFAULT_MAX_FILE_BYTES,
            None,
        )
        .expect("MCP 历史应可创建");

        for sequence in 0..5 {
            store
                .push(raw_event(sequence))
                .expect("测试事件应写入 btcap");
        }

        let response = store
            .events(&EventsParams {
                limit: Some(8),
                after_seq: None,
                before_seq: None,
                since_ns: Some(1_000),
                until_ns: Some(3_000),
                direction: None,
                interface: None,
                query: None,
            })
            .expect("时间窗口查询应可读取 btcap");

        assert_eq!(
            response
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(response.matched_count, 3);

        let _ = std::fs::remove_file(path);
    }

    fn raw_event(sequence: u64) -> BinderEvent {
        BinderEvent {
            sequence,
            timestamp_ns: sequence * 1_000,
            kind: 1,
            pid: 100,
            tgid: 100,
            uid: 2000,
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
            target_handle: 0,
            sender_pid: 0,
            sender_euid: 0,
            payload_len: 0,
            payload_truncated: 0,
            reserved: [0; 7],
            payload: [0; 256],
        }
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
}
