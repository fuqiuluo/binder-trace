//! 终端实时界面。
//!
//! # 职责
//! - 用固定布局展示 Binder transaction 事件、频率统计和当前事件详情。
//! - 通过 alternate screen 和原地覆盖降低 adb shell 下的闪烁。
//! - 处理捕获开关、清空、滚动和退出等键盘交互。

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::future;
use std::io::{self, Read, Stdout, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bt_agent::{BinderEvent, CaptureConfig, CaptureStats, SocketIpcClient, SocketIpcError};
use bt_decoder::{AndroidPlatformMethods, parse_interface_token};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use tokio::io::unix::AsyncFd;
use tokio::time::{self, MissedTickBehavior};

use crate::tui_history::{CaptureHistory, HistoryError};

mod theme {
    use std::fmt;

    use anstyle::{Ansi256Color, AnsiColor, Color, Style};

    pub const BORDER: Style = ansi256_fg(120);
    pub const TRANSACTION_SEND: Style = ansi256_fg(51);
    pub const TRANSACTION_REPLY: Style = ansi256_fg(226);
    pub const FREQUENCY: Style = ansi256_fg(120);
    pub const FREQUENCY_CODE: Style = ansi256_fg(226);
    pub const MUTED: Style = Style::new().dimmed();
    pub const TITLE: Style = Style::new().bold();
    pub const SELECTED: Style = Style::new()
        .fg_color(Some(Color::Ansi(AnsiColor::Black)))
        .bg_color(Some(Color::Ansi(AnsiColor::Cyan)));
    pub const STATUS: Style = SELECTED;

    const fn ansi256_fg(index: u8) -> Style {
        Style::new().fg_color(Some(Color::Ansi256(Ansi256Color(index))))
    }

    pub fn paint(style: Style, text: impl fmt::Display) -> String {
        format!("{style}{text}{style:#}")
    }
}

#[derive(Debug, Clone)]
pub struct TuiConfig {
    pub rows: usize,
    pub refresh: Duration,
    pub capture_config: Option<CaptureConfig>,
    pub android_sdk: Option<u16>,
    pub history_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum TuiError {
    Socket(SocketIpcError),
    Io(io::Error),
    History(HistoryError),
}

impl fmt::Display for TuiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::History(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TuiError {}

impl From<SocketIpcError> for TuiError {
    fn from(error: SocketIpcError) -> Self {
        Self::Socket(error)
    }
}

impl From<io::Error> for TuiError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<HistoryError> for TuiError {
    fn from(error: HistoryError) -> Self {
        Self::History(error)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum UiCommand {
    Quit,
    ToggleRecording,
    Clear,
    Help,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

#[derive(Debug)]
struct TuiState {
    family: i32,
    capacity: usize,
    events: VecDeque<BinderEvent>,
    window_start: u64,
    selected: u64,
    total_events: u64,
    frequency_counts: BTreeMap<FrequencyKey, u64>,
    frequency_counted_events: u64,
    stats: CaptureStats,
    recording: bool,
    help_visible: bool,
    input_available: bool,
    android_sdk: Option<u16>,
    platform_methods: Option<AndroidPlatformMethods>,
    history_path: PathBuf,
    start: Instant,
}

impl TuiState {
    fn new(
        family: i32,
        capacity: usize,
        stats: CaptureStats,
        input_available: bool,
        android_sdk: Option<u16>,
        history_path: PathBuf,
    ) -> Self {
        Self {
            family,
            capacity: capacity.max(1),
            events: VecDeque::with_capacity(capacity.max(1)),
            window_start: 0,
            selected: 0,
            total_events: 0,
            frequency_counts: BTreeMap::new(),
            frequency_counted_events: 0,
            stats,
            recording: true,
            help_visible: false,
            input_available,
            android_sdk,
            platform_methods: android_sdk.map(AndroidPlatformMethods::new),
            history_path,
            start: Instant::now(),
        }
    }

    fn push_event(&mut self, history_index: u64, event: BinderEvent) {
        let follows_tail = self.total_events == 0 || self.selected + 1 >= self.total_events;
        self.total_events = history_index + 1;

        if !follows_tail {
            return;
        }

        if self.events.is_empty() {
            self.window_start = history_index;
        }

        if self.window_start + self.events.len() as u64 != history_index {
            self.events.clear();
            self.window_start = history_index;
        }

        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.window_start += 1;
        }

        self.events.push_back(event);
        self.selected = history_index;
    }

    fn sync_frequency_counts(&mut self, history: &CaptureHistory) -> Result<(), TuiError> {
        while self.frequency_counted_events < history.event_count() {
            let event = history.event_at(self.frequency_counted_events)?;
            if let Some(key) = FrequencyKey::from_event(&event, self.platform_methods) {
                *self.frequency_counts.entry(key).or_default() += 1;
            }
            self.frequency_counted_events += 1;
        }

        Ok(())
    }

    fn clear(&mut self) {
        self.events.clear();
        self.window_start = self.total_events;
        self.selected = self.total_events.saturating_sub(1);
    }

    fn selected_event(&self) -> Option<&BinderEvent> {
        let offset = self.selected.checked_sub(self.window_start)?;
        self.events.get(offset as usize)
    }

    fn move_selection(&mut self, command: UiCommand, page_size: usize) {
        if self.total_events == 0 {
            self.selected = 0;
            return;
        }

        let last = self.total_events - 1;
        self.selected = match command {
            UiCommand::Up => self.selected.saturating_sub(1),
            UiCommand::Down => (self.selected + 1).min(last),
            UiCommand::PageUp => self.selected.saturating_sub(page_size.max(1) as u64),
            UiCommand::PageDown => (self.selected + page_size.max(1) as u64).min(last),
            UiCommand::Home => 0,
            UiCommand::End => last,
            UiCommand::Quit | UiCommand::ToggleRecording | UiCommand::Clear | UiCommand::Help => {
                self.selected
            }
        };
    }

    fn ensure_selected_loaded(&mut self, history: &CaptureHistory) -> Result<(), TuiError> {
        if self.total_events == 0 || self.selection_is_loaded() {
            return Ok(());
        }

        let capacity = self.capacity as u64;
        let half_window = capacity / 2;
        let max_start = self.total_events.saturating_sub(capacity);
        let start = self.selected.saturating_sub(half_window).min(max_start);
        let events = history.load_window(start, self.capacity)?;

        self.window_start = start;
        self.events = VecDeque::from(events);
        Ok(())
    }

    fn selection_is_loaded(&self) -> bool {
        let window_end = self.window_start + self.events.len() as u64;
        self.selected >= self.window_start && self.selected < window_end
    }
}

struct TerminalSession {
    stdout: Stdout,
    input: Option<TtyInput>,
}

impl TerminalSession {
    fn enter() -> Result<Self, TuiError> {
        let input = TtyInput::open().ok();
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide, Clear(ClearType::All))?;

        Ok(Self { stdout, input })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = execute!(self.stdout, Show, LeaveAlternateScreen);
    }
}

pub fn run_tui(client: &SocketIpcClient, family: i32, config: TuiConfig) -> Result<(), TuiError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?
        .block_on(run_tui_async(client, family, config))
}

async fn run_tui_async(
    client: &SocketIpcClient,
    family: i32,
    config: TuiConfig,
) -> Result<(), TuiError> {
    const MAX_EVENTS_PER_TICK: usize = 512;

    let TuiConfig {
        rows,
        refresh,
        capture_config,
        android_sdk,
        history_path,
    } = config;

    let history_path = history_path.unwrap_or_else(CaptureHistory::default_path);
    let mut history = CaptureHistory::create(history_path, rows.max(1))?;
    let mut terminal = TerminalSession::enter()?;
    let capture_guard = CaptureGuard::new(client, capture_config);
    let mut state = TuiState::new(
        family,
        rows.max(1),
        client.get_stats()?,
        terminal.input.is_some(),
        android_sdk,
        history.path().to_path_buf(),
    );
    let refresh = refresh.clamp(Duration::from_millis(50), Duration::from_secs(5));
    let socket_ready = AsyncFd::new(RawFdSource::new(client.raw_fd()))?;
    let mut input_ready = terminal
        .input
        .as_ref()
        .map(|input| AsyncFd::new(RawFdSource::new(input.raw_fd())))
        .transpose()?;
    let mut stats_interval = time::interval_at(
        time::Instant::now() + Duration::from_secs(1),
        Duration::from_secs(1),
    );
    stats_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut frame_interval = time::interval_at(time::Instant::now() + refresh, refresh);
    frame_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;

    render_if_dirty(&mut terminal.stdout, &state, &mut dirty)?;

    loop {
        tokio::select! {
            input_result = wait_optional_readable(input_ready.as_ref()) => {
                input_result?;
                if drain_input_commands(
                    &mut terminal,
                    &mut input_ready,
                    client,
                    &mut state,
                    capture_config,
                    &history,
                    &mut dirty,
                )? {
                    break;
                }
                render_if_dirty(&mut terminal.stdout, &state, &mut dirty)?;
            }
            socket_result = wait_readable(&socket_ready) => {
                socket_result?;
                if drain_socket_events(client, &mut history, &mut state, MAX_EVENTS_PER_TICK)? {
                    dirty = true;
                    render_if_dirty(&mut terminal.stdout, &state, &mut dirty)?;
                }
            }
            _ = stats_interval.tick() => {
                state.stats = client.get_stats()?;
                history.flush_async()?;
                dirty = true;
                render_if_dirty(&mut terminal.stdout, &state, &mut dirty)?;
            }
            _ = frame_interval.tick() => {
                render_if_dirty(&mut terminal.stdout, &state, &mut dirty)?;
            }
        }
    }

    drop(capture_guard);
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct RawFdSource {
    fd: RawFd,
}

impl RawFdSource {
    const fn new(fd: RawFd) -> Self {
        Self { fd }
    }
}

impl AsRawFd for RawFdSource {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

async fn wait_readable(fd: &AsyncFd<RawFdSource>) -> io::Result<()> {
    let mut guard = fd.readable().await?;
    guard.clear_ready();
    Ok(())
}

async fn wait_optional_readable(fd: Option<&AsyncFd<RawFdSource>>) -> io::Result<()> {
    match fd {
        Some(fd) => wait_readable(fd).await,
        None => future::pending().await,
    }
}

fn drain_input_commands(
    terminal: &mut TerminalSession,
    input_ready: &mut Option<AsyncFd<RawFdSource>>,
    client: &SocketIpcClient,
    state: &mut TuiState,
    capture_config: Option<CaptureConfig>,
    history: &CaptureHistory,
    dirty: &mut bool,
) -> Result<bool, TuiError> {
    let mut input_failed = false;

    if let Some(input) = terminal.input.as_mut() {
        loop {
            match input.read_command() {
                Ok(Some(UiCommand::Quit)) => return Ok(true),
                Ok(Some(UiCommand::ToggleRecording)) => {
                    toggle_recording(client, state, capture_config)?;
                    *dirty = true;
                }
                Ok(Some(UiCommand::Clear)) => {
                    state.clear();
                    *dirty = true;
                }
                Ok(Some(UiCommand::Help)) => {
                    state.help_visible = !state.help_visible;
                    *dirty = true;
                }
                Ok(Some(command)) => {
                    state.move_selection(command, 10);
                    state.ensure_selected_loaded(history)?;
                    *dirty = true;
                }
                Ok(None) => break,
                Err(_) => {
                    input_failed = true;
                    break;
                }
            }
        }
    }

    if input_failed {
        terminal.input = None;
        *input_ready = None;
        state.input_available = false;
        *dirty = true;
    }

    Ok(false)
}

fn drain_socket_events(
    client: &SocketIpcClient,
    history: &mut CaptureHistory,
    state: &mut TuiState,
    max_events: usize,
) -> Result<bool, TuiError> {
    let mut changed = false;

    if state.recording {
        let mut appended = false;
        for _ in 0..max_events {
            let Some(index) =
                history.recv_next_matching(client, BinderEvent::is_binder_transaction)?
            else {
                break;
            };
            let event = history.event_at(index)?;
            state.push_event(index, event);
            appended = true;
            changed = true;
        }
        if appended {
            state.sync_frequency_counts(history)?;
        }
    } else {
        for _ in 0..max_events {
            if client.try_recv_event()?.is_none() {
                break;
            }
            changed = true;
        }
    }

    Ok(changed)
}

fn render_if_dirty(out: &mut Stdout, state: &TuiState, dirty: &mut bool) -> Result<(), TuiError> {
    if *dirty {
        render(out, state)?;
        *dirty = false;
    }

    Ok(())
}

struct CaptureGuard<'a> {
    client: &'a SocketIpcClient,
    owns_capture: bool,
}

impl<'a> CaptureGuard<'a> {
    fn new(client: &'a SocketIpcClient, capture_config: Option<CaptureConfig>) -> Self {
        Self {
            client,
            owns_capture: capture_config.is_some(),
        }
    }
}

impl Drop for CaptureGuard<'_> {
    fn drop(&mut self) {
        if self.owns_capture {
            let _ = self.client.set_config(CaptureConfig::disabled());
        }
    }
}

fn toggle_recording(
    client: &SocketIpcClient,
    state: &mut TuiState,
    capture_config: Option<CaptureConfig>,
) -> Result<(), TuiError> {
    if state.recording {
        if capture_config.is_some() {
            client.set_config(CaptureConfig::disabled())?;
        }
        state.recording = false;
    } else {
        if let Some(capture_config) = capture_config {
            client.set_config(capture_config)?;
        }
        state.recording = true;
    }

    Ok(())
}

struct TtyInput {
    file: File,
    original_termios: libc::termios,
    parser: KeyParser,
}

impl TtyInput {
    fn open() -> io::Result<Self> {
        let file = OpenOptions::new().read(true).open("/dev/tty")?;
        let original_termios = get_termios(file.as_raw_fd())?;
        let mut raw_termios = original_termios;

        raw_termios.c_iflag &=
            !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        raw_termios.c_oflag &= !libc::OPOST;
        raw_termios.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        raw_termios.c_cc[libc::VMIN] = 0;
        raw_termios.c_cc[libc::VTIME] = 0;
        set_termios(file.as_raw_fd(), &raw_termios)?;

        Ok(Self {
            file,
            original_termios,
            parser: KeyParser::default(),
        })
    }

    fn read_command(&mut self) -> io::Result<Option<UiCommand>> {
        while poll_fd(self.file.as_raw_fd())? {
            let mut bytes = [0_u8; 32];
            let read = match self.file.read(&mut bytes) {
                Ok(0) => return Ok(None),
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };

            for byte in bytes.into_iter().take(read) {
                if let Some(command) = self.parser.push(byte) {
                    return Ok(Some(command));
                }
            }
        }

        Ok(None)
    }

    fn raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

impl Drop for TtyInput {
    fn drop(&mut self) {
        let _ = set_termios(self.file.as_raw_fd(), &self.original_termios);
    }
}

#[derive(Debug, Default)]
struct KeyParser {
    escape: Vec<u8>,
}

impl KeyParser {
    fn push(&mut self, byte: u8) -> Option<UiCommand> {
        if self.escape.is_empty() {
            return match byte {
                b'q' | 0x03 => Some(UiCommand::Quit),
                b' ' => Some(UiCommand::ToggleRecording),
                b'c' => Some(UiCommand::Clear),
                b'h' => Some(UiCommand::Help),
                0x1b => {
                    self.escape.push(byte);
                    None
                }
                _ => None,
            };
        }

        self.escape.push(byte);
        if let Some(command) = escape_to_command(&self.escape) {
            self.escape.clear();
            return Some(command);
        }
        if !is_escape_prefix(&self.escape) {
            self.escape.clear();
        }

        None
    }
}

const ESCAPE_COMMANDS: &[(&[u8], UiCommand)] = &[
    (b"\x1b[A", UiCommand::Up),
    (b"\x1b[B", UiCommand::Down),
    (b"\x1b[5~", UiCommand::PageUp),
    (b"\x1b[6~", UiCommand::PageDown),
    (b"\x1b[H", UiCommand::Home),
    (b"\x1b[1~", UiCommand::Home),
    (b"\x1bOH", UiCommand::Home),
    (b"\x1b[F", UiCommand::End),
    (b"\x1b[4~", UiCommand::End),
    (b"\x1bOF", UiCommand::End),
];

fn escape_to_command(sequence: &[u8]) -> Option<UiCommand> {
    ESCAPE_COMMANDS
        .iter()
        .find_map(|(candidate, command)| (*candidate == sequence).then_some(*command))
}

fn is_escape_prefix(sequence: &[u8]) -> bool {
    ESCAPE_COMMANDS
        .iter()
        .any(|(candidate, _)| candidate.starts_with(sequence))
}

fn get_termios(fd: RawFd) -> io::Result<libc::termios> {
    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: `termios` 指向未初始化但足够大的栈内存，`fd` 是当前进程打开的 tty fd。
    let ret = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
    if ret == 0 {
        // SAFETY: `tcgetattr` 成功后已经完整初始化 `termios`。
        Ok(unsafe { termios.assume_init() })
    } else {
        Err(io::Error::last_os_error())
    }
}

fn set_termios(fd: RawFd, termios: &libc::termios) -> io::Result<()> {
    // SAFETY: `termios` 是有效引用，`fd` 是当前进程打开的 tty fd。
    let ret = unsafe { libc::tcsetattr(fd, libc::TCSANOW, termios) };
    if ret == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn poll_fd(fd: RawFd) -> io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };

    loop {
        // SAFETY: `pollfd` 指向栈上有效内存，长度参数与实际元素数量一致。
        let ret = unsafe { libc::poll(&mut pollfd, 1, 0) };
        if ret > 0 {
            return Ok((pollfd.revents & libc::POLLIN) != 0);
        }
        if ret == 0 {
            return Ok(false);
        }

        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn render(out: &mut Stdout, state: &TuiState) -> io::Result<()> {
    const STATUS_HEIGHT: usize = 2;

    let (width, height) = terminal::size().unwrap_or((120, 36));
    let width = usize::from(width).max(72);
    let height = usize::from(height).max(20);
    let content_height = height.saturating_sub(STATUS_HEIGHT);
    let top_height = ((content_height * 52) / 100).clamp(8, content_height.saturating_sub(6));
    let bottom_height = content_height.saturating_sub(top_height).max(6);
    let left_width = ((width * 56) / 100).clamp(38, width.saturating_sub(28));
    let right_width = width.saturating_sub(left_width);

    let transactions = render_transactions(state, left_width, top_height);
    let frequency = render_frequency(state, right_width, top_height);
    let hexdump = render_hexdump(state, left_width, bottom_height);
    let parsed = render_parsed(state, right_width, bottom_height);
    let status = render_status(state, width);

    queue!(out, MoveTo(0, 0))?;

    for row in 0..top_height {
        queue!(out, MoveTo(0, row as u16))?;
        write!(out, "{}{}", transactions[row], frequency[row])?;
    }

    for row in 0..bottom_height {
        queue!(out, MoveTo(0, (top_height + row) as u16))?;
        write!(out, "{}{}", hexdump[row], parsed[row])?;
    }

    for (offset, line) in status.into_iter().enumerate() {
        queue!(out, MoveTo(0, (content_height + offset) as u16))?;
        write!(out, "{line}")?;
    }
    out.flush()
}

fn render_transactions(state: &TuiState, width: usize, height: usize) -> Vec<String> {
    render_panel(
        "Transactions",
        width,
        height,
        |inner_width, inner_height| {
            let mut lines = Vec::with_capacity(inner_height);
            let visible_rows = inner_height.saturating_sub(1);
            let selected_offset = state
                .selected
                .checked_sub(state.window_start)
                .map(|offset| offset as usize)
                .unwrap_or_default();
            let start = if selected_offset >= visible_rows {
                selected_offset + 1 - visible_rows
            } else {
                0
            };
            let rows = (start..state.events.len().min(start + visible_rows))
                .map(|index| {
                    let event = &state.events[index];
                    let summary = TransactionSummary::new(event, state.platform_methods);
                    (state.window_start + index as u64, event, summary)
                })
                .collect::<Vec<_>>();
            let columns = TransactionColumns::new(
                inner_width,
                rows.iter()
                    .any(|(_, _, summary)| !summary.method.is_empty()),
            );
            lines.push(theme::paint(theme::MUTED, columns.header()));

            for (history_index, event, summary) in rows {
                let fitted = columns.row(
                    &event.sequence.to_string(),
                    summary.interface.as_str(),
                    &event.code.to_string(),
                    summary.method,
                    &format!("0x{:x}", event.data_size),
                );
                if history_index == state.selected {
                    lines.push(theme::paint(theme::SELECTED, fitted));
                } else if event.is_reply() {
                    lines.push(theme::paint(theme::TRANSACTION_REPLY, fitted));
                } else {
                    lines.push(theme::paint(theme::TRANSACTION_SEND, fitted));
                }
            }

            lines
        },
    )
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct TransactionColumns {
    width: usize,
    sequence: usize,
    interface: usize,
    code: usize,
    method: usize,
    len: usize,
}

impl TransactionColumns {
    fn new(width: usize, has_method: bool) -> Self {
        const SEQUENCE_WIDTH: usize = 8;
        const CODE_WIDTH: usize = 10;
        const LEN_WIDTH: usize = 8;
        const MIN_INTERFACE_WIDTH: usize = 18;
        const MAX_INTERFACE_WIDTH: usize = 56;
        const MIN_METHOD_WIDTH: usize = 12;
        const MAX_METHOD_WIDTH: usize = 28;

        let gap_width = if has_method { 4 } else { 3 };
        let available = width.saturating_sub(gap_width);
        let sequence = SEQUENCE_WIDTH.min(available);
        let remaining = available.saturating_sub(sequence);
        let code = CODE_WIDTH.min(remaining);
        let remaining = remaining.saturating_sub(code);
        let len = LEN_WIDTH.min(remaining);
        let remaining = remaining.saturating_sub(len);
        let method = if has_method && remaining > MIN_INTERFACE_WIDTH {
            let available_for_method = remaining.saturating_sub(MIN_INTERFACE_WIDTH);
            if available_for_method >= MIN_METHOD_WIDTH {
                MAX_METHOD_WIDTH.min(available_for_method)
            } else {
                0
            }
        } else {
            0
        };
        let interface = remaining.saturating_sub(method).min(MAX_INTERFACE_WIDTH);

        Self {
            width,
            sequence,
            interface,
            code,
            method,
            len,
        }
    }

    fn header(self) -> String {
        self.row("Seq", "Interface", "#", "Method", "Len")
    }

    fn row(self, sequence: &str, interface: &str, code: &str, method: &str, len: &str) -> String {
        let line = if self.method == 0 {
            format!(
                "{} {} {} {}",
                fit_right(sequence, self.sequence),
                fit(interface, self.interface),
                fit_right(code, self.code),
                fit_right(len, self.len),
            )
        } else {
            format!(
                "{} {} {} {} {}",
                fit_right(sequence, self.sequence),
                fit(interface, self.interface),
                fit_right(code, self.code),
                fit(method, self.method),
                fit_right(len, self.len),
            )
        };
        fit(&line, self.width)
    }
}

fn render_frequency(state: &TuiState, width: usize, height: usize) -> Vec<String> {
    render_panel("Frequency", width, height, |inner_width, inner_height| {
        let columns = FrequencyColumns::new(inner_width);
        let mut lines = Vec::with_capacity(inner_height);
        lines.push(theme::paint(theme::MUTED, columns.header()));

        for entry in frequency_entries(state)
            .into_iter()
            .take(inner_height.saturating_sub(1))
        {
            lines.push(columns.styled_row(&entry, "[+]"));
        }

        lines
    })
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct FrequencyColumns {
    width: usize,
    label: usize,
    count: usize,
    filter: usize,
}

impl FrequencyColumns {
    fn new(width: usize) -> Self {
        const GAP_WIDTH: usize = 2;
        const COUNT_WIDTH: usize = 9;
        const FILTER_WIDTH: usize = 6;

        let available = width.saturating_sub(GAP_WIDTH);
        let filter = FILTER_WIDTH.min(available);
        let remaining = available.saturating_sub(filter);
        let count = COUNT_WIDTH.min(remaining);
        let label = remaining.saturating_sub(count);

        Self {
            width,
            label,
            count,
            filter,
        }
    }

    fn header(self) -> String {
        self.row("Interface/Code", "Frequency", "Filter")
    }

    fn row(self, label: &str, count: &str, filter: &str) -> String {
        let line = format!(
            "{} {} {}",
            fit(label, self.label),
            fit_right(count, self.count),
            fit_right(filter, self.filter),
        );
        fit(&line, self.width)
    }

    fn styled_row(self, entry: &FrequencyEntry, filter: &str) -> String {
        format!(
            "{} {} {}",
            self.styled_label(entry),
            theme::paint(
                theme::FREQUENCY,
                fit_right(&entry.count.to_string(), self.count)
            ),
            theme::paint(theme::FREQUENCY, fit_right(filter, self.filter)),
        )
    }

    fn styled_label(self, entry: &FrequencyEntry) -> String {
        let code = format!("#{}", entry.code);
        let code_width = code.chars().count();
        if self.label == 0 {
            return String::new();
        }
        if code_width >= self.label {
            return theme::paint(theme::FREQUENCY_CODE, fit(&code, self.label));
        }

        let interface = truncate_with_ellipsis(&entry.interface, self.label - code_width);
        let padding = self
            .label
            .saturating_sub(interface.chars().count() + code_width);
        format!(
            "{}{}{}",
            theme::paint(theme::FREQUENCY, interface),
            theme::paint(theme::FREQUENCY_CODE, code),
            theme::paint(theme::FREQUENCY, " ".repeat(padding)),
        )
    }
}

fn render_hexdump(state: &TuiState, width: usize, height: usize) -> Vec<String> {
    render_panel("Hexdump", width, height, |inner_width, inner_height| {
        let Some(event) = state.selected_event() else {
            return vec![dim_line("等待 binder_transaction 事件", inner_width)];
        };

        let bytes = event.payload_bytes();
        if bytes.is_empty() {
            return vec![dim_line("payload 为空或未捕获", inner_width)];
        }

        bytes
            .chunks(16)
            .take(inner_height)
            .enumerate()
            .map(|(row, chunk)| {
                let mut hex = String::new();
                let mut ascii = String::new();
                for byte in chunk {
                    hex.push_str(&format!("{byte:02x} "));
                    ascii.push(if byte.is_ascii_graphic() {
                        char::from(*byte)
                    } else {
                        '.'
                    });
                }
                let line = format!("{:04x}  {:<48}  {}", row * 16, hex, ascii);
                fit(&line, inner_width)
            })
            .collect()
    })
}

fn render_parsed(state: &TuiState, width: usize, height: usize) -> Vec<String> {
    render_panel(
        "Parsed Transaction",
        width,
        height,
        |inner_width, inner_height| {
            if state.help_visible {
                return help_lines(inner_width, inner_height);
            }

            let Some(event) = state.selected_event() else {
                return vec![dim_line("选择事件后显示解析结果", inner_width)];
            };
            let summary = TransactionSummary::new(event, state.platform_methods);

            let lines = [
                theme::paint(theme::TITLE, fit("binder_transaction", inner_width)),
                format!("interface: {}", summary.interface.as_str()),
                format!("code: {}", event.code),
                format!("method: {}", summary.method),
                format!("flags: 0x{:x}", event.flags),
                format!("data_size: 0x{:x}", event.data_size),
                format!("offsets_size: 0x{:x}", event.offsets_size),
                format!("target_handle: {}", event.target_handle),
                format!(
                    "sender_pid/euid: {}/{}",
                    event.sender_pid, event.sender_euid
                ),
                format!(
                    "payload: {} bytes{}",
                    event.payload_bytes().len(),
                    if event.payload_truncated != 0 {
                        " truncated"
                    } else {
                        ""
                    }
                ),
                format!("direction: {}", direction(event)),
                format!("sequence: {}", event.sequence),
                format!("timestamp_ns: {}", event.timestamp_ns),
                format!("tgid/pid: {}/{}", event.tgid, event.pid),
                format!("uid: {}", event.uid),
                format!("transaction: 0x{:016x}", event.transaction),
                format!("proc: 0x{:016x}", event.proc),
                format!("thread: 0x{:016x}", event.thread),
                format!("extra_buffers_size: 0x{:x}", event.extra_buffers_size),
                format!("lost_before: {}", event.lost_before),
            ];

            lines
                .into_iter()
                .take(inner_height)
                .enumerate()
                .map(|(index, line)| {
                    if index == 0 {
                        line
                    } else {
                        fit(&line, inner_width)
                    }
                })
                .collect()
        },
    )
}

fn help_lines(width: usize, height: usize) -> Vec<String> {
    [
        "q / Ctrl-C     quit",
        "space          toggle kernel capture",
        "c              clear captured rows",
        "h              toggle this help pane",
        "up/down        move selection",
        "page up/down   scroll selection faster",
        "home/end       jump to first/last row",
    ]
    .into_iter()
    .take(height)
    .map(|line| fit(line, width))
    .collect()
}

fn render_status(state: &TuiState, width: usize) -> [String; 2] {
    let selected = if state.total_events == 0 {
        "0/0".to_owned()
    } else {
        format!("{}/{}", state.selected + 1, state.total_events)
    };
    let window_end = state.window_start + state.events.len() as u64;
    let uptime = state.start.elapsed().as_secs();
    let recording = if state.recording { "True" } else { "False" };
    let input = if state.input_available {
        "True"
    } else {
        "False"
    };
    let sdk = state
        .android_sdk
        .map(|sdk| sdk.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let status_text = format!(
        "Family: {}  SDK: {}  Transactions: {}  Saved: {}  Window: {}-{}  History: {}  Recording: {}  Input: {}  Selected: {}  Uptime: {}s",
        state.family,
        sdk,
        state.stats.captured,
        state.total_events,
        state.window_start,
        window_end,
        state.history_path.display(),
        recording,
        input,
        selected,
        uptime
    );
    let key_text = "Keys: q=quit  h=help  space=toggle  c=clear  up/down=move  page up/down=page  home/end=jump";

    [
        theme::paint(theme::STATUS, fit(&status_text, width)),
        theme::paint(theme::MUTED, fit(key_text, width)),
    ]
}

fn render_panel(
    title: &str,
    width: usize,
    height: usize,
    body: impl FnOnce(usize, usize) -> Vec<String>,
) -> Vec<String> {
    let width = width.max(8);
    let height = height.max(3);
    let inner_width = width.saturating_sub(2);
    let inner_height = height.saturating_sub(2);
    let mut lines = Vec::with_capacity(height);

    lines.push(top_border(title, width));
    for line in body(inner_width, inner_height)
        .into_iter()
        .take(inner_height)
    {
        lines.push(format!(
            "{}{}{}",
            theme::paint(theme::BORDER, "|"),
            line,
            theme::paint(theme::BORDER, "|")
        ));
    }

    while lines.len() + 1 < height {
        lines.push(format!(
            "{}{}{}",
            theme::paint(theme::BORDER, "|"),
            " ".repeat(inner_width),
            theme::paint(theme::BORDER, "|")
        ));
    }

    lines.push(theme::paint(
        theme::BORDER,
        format!("+{}+", "-".repeat(inner_width)),
    ));
    lines
}

fn top_border(title: &str, width: usize) -> String {
    let inner_width = width.saturating_sub(2);
    let title = format!(" {title} ");
    if title.len() >= inner_width {
        return theme::paint(theme::BORDER, format!("+{}+", fit(&title, inner_width)));
    }

    let left = (inner_width - title.len()) / 2;
    let right = inner_width - title.len() - left;
    theme::paint(
        theme::BORDER,
        format!("+{}{}{}+", "-".repeat(left), title, "-".repeat(right)),
    )
}

fn dim_line(text: &str, width: usize) -> String {
    theme::paint(theme::MUTED, fit(text, width))
}

fn fit(text: &str, width: usize) -> String {
    let mut result = truncate_with_ellipsis(text, width);
    let visible = result.chars().count();
    if visible < width {
        result.push_str(&" ".repeat(width - visible));
    }
    result
}

fn fit_right(text: &str, width: usize) -> String {
    let result = truncate_with_ellipsis(text, width);
    let visible = result.chars().count();
    if visible < width {
        format!("{}{}", " ".repeat(width - visible), result)
    } else {
        result
    }
}

fn truncate_with_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    if text.chars().count() <= width {
        return text.to_owned();
    }

    if width <= 3 {
        return ".".repeat(width);
    }

    format!("{}...", text.chars().take(width - 3).collect::<String>())
}

fn direction(event: &BinderEvent) -> &'static str {
    if event.is_reply() { "reply" } else { "send" }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct TransactionSummary {
    interface: String,
    method: &'static str,
}

impl TransactionSummary {
    fn new(event: &BinderEvent, platform_methods: Option<AndroidPlatformMethods>) -> Self {
        if event.is_reply() {
            return Self {
                interface: String::new(),
                method: "",
            };
        }

        let interface = parse_interface_token(event.payload_bytes()).unwrap_or_default();
        let method = if interface.is_empty() {
            ""
        } else {
            platform_methods
                .map(|methods| methods.method_name_or_empty(&interface, event.code))
                .unwrap_or("")
        };

        Self { interface, method }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct FrequencyEntry {
    label: String,
    interface: String,
    code: u32,
    count: u64,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct FrequencyKey {
    interface: String,
    code: u32,
}

impl FrequencyKey {
    fn from_event(
        event: &BinderEvent,
        platform_methods: Option<AndroidPlatformMethods>,
    ) -> Option<Self> {
        let summary = TransactionSummary::new(event, platform_methods);
        if summary.interface.is_empty() {
            return None;
        }

        Some(Self {
            interface: summary.interface,
            code: event.code,
        })
    }

    fn label(&self) -> String {
        format!("{}#{}", self.interface, self.code)
    }
}

fn frequency_entries(state: &TuiState) -> Vec<FrequencyEntry> {
    let mut entries = state
        .frequency_counts
        .iter()
        .map(|(key, count)| FrequencyEntry {
            label: key.label(),
            interface: key.interface.clone(),
            code: key.code,
            count: *count,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.label.cmp(&right.label))
    });
    entries
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use bt_agent::BinderEvent;
    use bt_agent::CaptureStats;
    use bt_decoder::AndroidPlatformMethods;

    use super::{
        FrequencyColumns, FrequencyEntry, FrequencyKey, TransactionColumns, TuiState,
        frequency_entries, render_status, truncate_with_ellipsis,
    };
    use crate::tui_history::CaptureHistory;

    #[test]
    fn truncation_marks_omitted_text() {
        assert_eq!(
            truncate_with_ellipsis("android.app.IActivityManager", 14),
            "android.app..."
        );
        assert_eq!(truncate_with_ellipsis("abcdef", 3), "...");
        assert_eq!(truncate_with_ellipsis("abcdef", 0), "");
    }

    #[test]
    fn transaction_columns_keep_row_width() {
        for width in [6, 24, 65, 100] {
            let row = TransactionColumns::new(width, true).row(
                "123456789",
                "android.app.IActivityManager",
                "1000000",
                "someExtremelyLongMethodName",
                "0x123456789",
            );

            assert_eq!(row.chars().count(), width);
        }
    }

    #[test]
    fn transaction_columns_drop_method_when_empty() {
        let header = TransactionColumns::new(80, false).header();

        assert!(!header.contains("Method"));
    }

    #[test]
    fn transaction_columns_do_not_stretch_interface_on_wide_terminals() {
        let columns = TransactionColumns::new(180, true);

        assert_eq!(columns.interface, 56);
        assert_eq!(columns.method, 28);
    }

    #[test]
    fn transaction_columns_keep_full_u32_code_width() {
        let columns = TransactionColumns::new(100, true);
        let row = columns.row(
            "1",
            "android.content.pm.IPackageManager",
            "4294967295",
            "method",
            "0x10",
        );

        assert!(row.contains("4294967295"));
    }

    #[test]
    fn frequency_key_uses_interface_and_code() {
        let event = binder_event("android.net.INetworkStatsService", 13, false);
        let key = FrequencyKey::from_event(&event, Some(AndroidPlatformMethods::new(34)));

        assert_eq!(
            key.map(|key| key.label()).as_deref(),
            Some("android.net.INetworkStatsService#13")
        );
    }

    #[test]
    fn frequency_key_ignores_replies_without_interface() {
        let event = binder_event("android.net.INetworkStatsService", 13, true);

        assert_eq!(
            FrequencyKey::from_event(&event, Some(AndroidPlatformMethods::new(34))),
            None
        );
    }

    #[test]
    fn frequency_columns_keep_count_and_filter_aligned() {
        let columns = FrequencyColumns::new(52);
        let row = columns.row(
            "android.content.pm.IPackageInstallerSessionFileSystemConnector#4294967295",
            "12345",
            "[+]",
        );

        assert_eq!(row.chars().count(), 52);
        assert!(row.contains("..."));
        assert!(row.ends_with(" 12345    [+]"));
    }

    #[test]
    fn frequency_styled_row_colors_code_separately() {
        let columns = FrequencyColumns::new(40);
        let entry = FrequencyEntry {
            label: "android.os.IFoo#18".to_owned(),
            interface: "android.os.IFoo".to_owned(),
            code: 18,
            count: 7,
        };
        let row = columns.styled_row(&entry, "[+]");
        let plain = strip_ansi(&row);

        assert!(row.contains("\x1b[38;5;226m#18"));
        assert_eq!(plain.chars().count(), 40);
        assert!(plain.contains("android.os.IFoo#18"));
    }

    #[test]
    fn status_bar_separates_state_from_key_hints() {
        let state = TuiState::new(12, 16, empty_stats(), true, Some(34), temp_path("status"));
        let [status, keys] = render_status(&state, 96);
        let status = strip_ansi(&status);
        let keys = strip_ansi(&keys);

        assert_eq!(status.chars().count(), 96);
        assert_eq!(keys.chars().count(), 96);
        assert!(status.contains("Family: 12"));
        assert!(!status.contains("q=quit"));
        assert!(keys.contains("Keys: q=quit"));
    }

    #[test]
    fn selection_loads_missing_window_from_history() {
        let path = temp_path("tui-window");
        let mut history = CaptureHistory::create(path.clone(), 2).expect("历史文件应可创建");
        for sequence in 0..5 {
            let mut event = binder_event("android.os.IMessenger", sequence as u32, false);
            event.sequence = sequence;
            history.append_for_test(event).expect("测试事件应可追加");
        }

        let mut state = TuiState::new(0, 2, empty_stats(), true, Some(34), path.clone());
        state.total_events = 5;
        state.selected = 4;
        state
            .ensure_selected_loaded(&history)
            .expect("缺失窗口应可从历史文件加载");

        assert_eq!(
            state
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(
            state.selected_event().map(|event| event.sequence).as_ref(),
            Some(&4)
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn tail_window_preserves_append_order() {
        let path = temp_path("tail-order");
        let mut state = TuiState::new(0, 3, empty_stats(), true, Some(34), path.clone());

        for sequence in 0..6 {
            let mut event = binder_event("android.os.IMessenger", sequence as u32, false);
            event.sequence = sequence;
            state.push_event(sequence, event);
        }

        assert_eq!(state.window_start, 3);
        assert_eq!(state.selected, 5);
        assert_eq!(
            state
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
    }

    #[test]
    fn backscroll_keeps_window_until_tail_reload() {
        let path = temp_path("backscroll-order");
        let mut history = CaptureHistory::create(path.clone(), 2).expect("历史文件应可创建");
        let mut state = TuiState::new(0, 2, empty_stats(), true, Some(34), path.clone());

        for sequence in 0..3 {
            let mut event = binder_event("android.os.IMessenger", sequence as u32, false);
            event.sequence = sequence;
            let index = history.append_for_test(event).expect("测试事件应可追加");
            state.push_event(index, event);
        }

        state.selected = 0;
        state
            .ensure_selected_loaded(&history)
            .expect("旧窗口应可从历史文件加载");
        assert_eq!(
            state
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );

        for sequence in 3..5 {
            let mut event = binder_event("android.os.IMessenger", sequence as u32, false);
            event.sequence = sequence;
            let index = history.append_for_test(event).expect("测试事件应可追加");
            state.push_event(index, event);
        }

        assert_eq!(
            state
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );

        state.selected = state.total_events - 1;
        state
            .ensure_selected_loaded(&history)
            .expect("尾部窗口应可从历史文件加载");
        assert_eq!(
            state
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn frequency_counts_follow_full_history_after_window_eviction() {
        let path = temp_path("frequency-history");
        let mut history = CaptureHistory::create(path.clone(), 2).expect("历史文件应可创建");
        let mut state = TuiState::new(0, 2, empty_stats(), true, Some(34), path.clone());

        for sequence in 0..128 {
            let mut event = binder_event("android.os.IMessenger", 1, false);
            event.sequence = sequence;
            let index = history.append_for_test(event).expect("测试事件应可追加");
            state.push_event(index, event);
        }
        state
            .sync_frequency_counts(&history)
            .expect("频率统计应可从完整历史同步");

        assert_eq!(state.events.len(), 2);
        assert_eq!(
            frequency_entries(&state)
                .first()
                .map(|entry| (entry.label.as_str(), entry.count)),
            Some(("android.os.IMessenger#1", 128))
        );

        let _ = fs::remove_file(path);
    }

    fn binder_event(interface: &str, code: u32, reply: bool) -> BinderEvent {
        let payload = parcel_payload(interface);
        let mut inline_payload = [0_u8; 256];
        inline_payload[..payload.len()].copy_from_slice(&payload);

        BinderEvent {
            sequence: 1,
            timestamp_ns: 0,
            kind: 1,
            pid: 0,
            tgid: 0,
            uid: 0,
            reply: u32::from(reply),
            lost_before: 0,
            transaction: 0,
            proc: 0,
            thread: 0,
            extra_buffers_size: 0,
            code,
            flags: 0,
            data_size: payload.len() as u64,
            offsets_size: 0,
            target_handle: 0,
            sender_pid: 0,
            sender_euid: 0,
            payload_len: payload.len() as u32,
            payload_truncated: 0,
            reserved: [0; 7],
            payload: inline_payload,
        }
    }

    fn parcel_payload(interface: &str) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0_i32.to_le_bytes());
        payload.extend_from_slice(&(-1_i32).to_le_bytes());
        payload.extend_from_slice(b"SYST");
        write_string16(&mut payload, interface);
        payload
    }

    fn write_string16(output: &mut Vec<u8>, value: &str) {
        let units = value.encode_utf16().collect::<Vec<_>>();
        output.extend_from_slice(&(units.len() as i32).to_le_bytes());
        for unit in units {
            output.extend_from_slice(&unit.to_le_bytes());
        }
        output.extend_from_slice(&0_u16.to_le_bytes());
        while !output.len().is_multiple_of(4) {
            output.push(0);
        }
    }

    fn empty_stats() -> CaptureStats {
        CaptureStats {
            ioctl_hits: 0,
            copy_to_user_hits: 0,
            transaction_hits: 0,
            captured: 0,
            filtered: 0,
        }
    }

    fn strip_ansi(text: &str) -> String {
        let mut stripped = String::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                for escaped in chars.by_ref() {
                    if escaped == 'm' {
                        break;
                    }
                }
            } else {
                stripped.push(ch);
            }
        }
        stripped
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
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
