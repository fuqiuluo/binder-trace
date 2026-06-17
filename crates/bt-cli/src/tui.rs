//! 终端实时界面。
//!
//! # 职责
//! - 用固定布局展示 Binder transaction 事件、频率统计和当前事件详情。
//! - 通过 alternate screen 和原地覆盖降低 adb shell 下的闪烁。
//! - 处理捕获开关、清空、滚动和退出等键盘交互。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
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

    use anstyle::{Ansi256Color, AnsiColor, Color, RgbColor, Style};

    pub const BORDER: Style = ansi256_fg(15);
    pub const FOCUSED_BORDER: Style = ansi256_fg(120);
    pub const FREQUENCY: Style = rgb_fg(176, 223, 226);
    pub const FREQUENCY_SELECTED: Style = black_on_rgb(176, 223, 226);
    pub const MUTED: Style = Style::new().dimmed();
    pub const TITLE: Style = Style::new().bold();
    pub const SELECTED: Style = Style::new()
        .fg_color(Some(Color::Ansi(AnsiColor::Black)))
        .bg_color(Some(Color::Ansi(AnsiColor::Cyan)));
    pub const STATUS: Style = SELECTED;

    const fn ansi256_fg(index: u8) -> Style {
        Style::new().fg_color(Some(Color::Ansi256(Ansi256Color(index))))
    }

    const fn rgb_fg(red: u8, green: u8, blue: u8) -> Style {
        Style::new().fg_color(Some(Color::Rgb(RgbColor(red, green, blue))))
    }

    const fn black_on_rgb(red: u8, green: u8, blue: u8) -> Style {
        Style::new()
            .fg_color(Some(Color::Ansi(AnsiColor::Black)))
            .bg_color(Some(Color::Rgb(RgbColor(red, green, blue))))
    }

    pub fn ansi256_fg_style(index: u8) -> Style {
        ansi256_fg(index)
    }

    pub fn black_on_ansi256(index: u8) -> Style {
        Style::new()
            .fg_color(Some(Color::Ansi(AnsiColor::Black)))
            .bg_color(Some(Color::Ansi256(Ansi256Color(index))))
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
    Space,
    Clear,
    NextPane,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum FocusPane {
    Transactions,
    Frequency,
    Hexdump,
    Parsed,
}

impl FocusPane {
    const fn title(self) -> &'static str {
        match self {
            Self::Transactions => "Transactions",
            Self::Frequency => "Frequency",
            Self::Hexdump => "Hexdump",
            Self::Parsed => "Parsed Transaction",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Transactions => Self::Frequency,
            Self::Frequency => Self::Hexdump,
            Self::Hexdump => Self::Parsed,
            Self::Parsed => Self::Transactions,
        }
    }
}

#[derive(Debug)]
struct DisplayEvent {
    history_index: u64,
    event: BinderEvent,
}

#[derive(Debug)]
struct TuiState {
    family: i32,
    capacity: usize,
    events: VecDeque<DisplayEvent>,
    selected: u64,
    total_events: u64,
    frequency_counts: BTreeMap<FrequencyKey, u64>,
    frequency_counted_events: u64,
    disabled_frequency: BTreeSet<FrequencyKey>,
    frequency_selected: usize,
    hexdump_scroll: usize,
    parsed_scroll: usize,
    stats: CaptureStats,
    recording: bool,
    focus: FocusPane,
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
            selected: 0,
            total_events: 0,
            frequency_counts: BTreeMap::new(),
            frequency_counted_events: 0,
            disabled_frequency: BTreeSet::new(),
            frequency_selected: 0,
            hexdump_scroll: 0,
            parsed_scroll: 0,
            stats,
            recording: true,
            focus: FocusPane::Transactions,
            input_available,
            android_sdk,
            platform_methods: android_sdk.map(AndroidPlatformMethods::new),
            history_path,
            start: Instant::now(),
        }
    }

    fn push_event(&mut self, history_index: u64, event: BinderEvent) {
        let follows_tail = self
            .events
            .back()
            .map(|entry| self.selected == entry.history_index)
            .unwrap_or(true);
        self.total_events = history_index + 1;

        if !follows_tail || !self.displays_event(&event) {
            return;
        }

        if self.events.len() == self.capacity {
            self.events.pop_front();
        }

        self.events.push_back(DisplayEvent {
            history_index,
            event,
        });
        self.selected = history_index;
        self.hexdump_scroll = 0;
        self.parsed_scroll = 0;
    }

    fn focus_next(&mut self) {
        self.focus = self.focus.next();
    }

    fn disable_selected_frequency(&mut self) -> bool {
        let entries = frequency_entries(self);
        let Some(entry) = entries
            .get(self.frequency_selected.min(entries.len().saturating_sub(1)))
            .cloned()
        else {
            return false;
        };

        self.disabled_frequency.insert(entry.key())
    }

    fn displays_event(&self, event: &BinderEvent) -> bool {
        let Some(key) = FrequencyKey::from_event(event, self.platform_methods) else {
            return true;
        };
        !self.disabled_frequency.contains(&key)
    }

    fn move_focused(
        &mut self,
        command: UiCommand,
        page_size: usize,
        history: &CaptureHistory,
    ) -> Result<(), TuiError> {
        match self.focus {
            FocusPane::Transactions => {
                let previous = self.selected;
                self.move_selection(command, page_size, history)?;
                self.ensure_selected_loaded(history)?;
                if self.selected != previous {
                    self.hexdump_scroll = 0;
                    self.parsed_scroll = 0;
                }
            }
            FocusPane::Frequency => {
                self.frequency_selected = move_index(
                    self.frequency_selected,
                    command,
                    page_size,
                    self.frequency_counts.len(),
                );
            }
            FocusPane::Hexdump => {
                let lines = self
                    .selected_event()
                    .map(|event| event.payload_bytes().len().div_ceil(16))
                    .unwrap_or_default();
                self.hexdump_scroll = move_index(self.hexdump_scroll, command, page_size, lines);
            }
            FocusPane::Parsed => {
                let lines = self
                    .selected_event()
                    .map(|_| PARSED_LINE_COUNT)
                    .unwrap_or_default();
                self.parsed_scroll = move_index(self.parsed_scroll, command, page_size, lines);
            }
        }

        Ok(())
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
        self.selected = self.total_events.saturating_sub(1);
        self.frequency_selected = 0;
        self.hexdump_scroll = 0;
        self.parsed_scroll = 0;
    }

    fn selected_event(&self) -> Option<&BinderEvent> {
        self.events
            .iter()
            .find(|entry| entry.history_index == self.selected)
            .map(|entry| &entry.event)
    }

    fn move_selection(
        &mut self,
        command: UiCommand,
        page_size: usize,
        history: &CaptureHistory,
    ) -> Result<(), TuiError> {
        if history.event_count() == 0 {
            self.selected = 0;
            return Ok(());
        }

        let page_size = page_size.max(1);
        match command {
            UiCommand::Up => {
                if let Some(index) =
                    self.previous_visible(history, self.selected.saturating_sub(1))?
                {
                    self.selected = index;
                }
            }
            UiCommand::Down => {
                if let Some(index) = self.next_visible(history, self.selected.saturating_add(1))? {
                    self.selected = index;
                }
            }
            UiCommand::PageUp => {
                for _ in 0..page_size {
                    let Some(index) =
                        self.previous_visible(history, self.selected.saturating_sub(1))?
                    else {
                        break;
                    };
                    self.selected = index;
                }
            }
            UiCommand::PageDown => {
                for _ in 0..page_size {
                    let Some(index) =
                        self.next_visible(history, self.selected.saturating_add(1))?
                    else {
                        break;
                    };
                    self.selected = index;
                }
            }
            UiCommand::Home => {
                if let Some(index) = self.next_visible(history, 0)? {
                    self.selected = index;
                }
            }
            UiCommand::End => {
                if let Some(index) =
                    self.previous_visible(history, history.event_count().saturating_sub(1))?
                {
                    self.selected = index;
                }
            }
            UiCommand::Quit | UiCommand::Space | UiCommand::Clear | UiCommand::NextPane => {}
        }

        Ok(())
    }

    fn ensure_selected_loaded(&mut self, history: &CaptureHistory) -> Result<(), TuiError> {
        if self.total_events == 0 || self.selection_is_loaded() {
            return Ok(());
        }

        self.reload_visible_window(history)?;
        Ok(())
    }

    fn selection_is_loaded(&self) -> bool {
        self.events
            .iter()
            .any(|entry| entry.history_index == self.selected)
    }

    fn reload_visible_window(&mut self, history: &CaptureHistory) -> Result<(), TuiError> {
        self.events.clear();
        if history.event_count() == 0 {
            return Ok(());
        }

        let start = self.selected.saturating_sub((self.capacity / 2) as u64);
        for index in start..history.event_count() {
            let event = history.event_at(index)?;
            if self.displays_event(&event) {
                self.events.push_back(DisplayEvent {
                    history_index: index,
                    event,
                });
                if self.events.len() == self.capacity {
                    break;
                }
            }
        }

        Ok(())
    }

    fn select_nearest_visible(&mut self, history: &CaptureHistory) -> Result<(), TuiError> {
        if history.event_count() == 0 {
            self.selected = 0;
            self.events.clear();
            return Ok(());
        }

        if self.selected < history.event_count()
            && self.displays_event(&history.event_at(self.selected)?)
        {
            return Ok(());
        }

        if let Some(index) = self.next_visible(history, self.selected.saturating_add(1))? {
            self.selected = index;
        } else if let Some(index) =
            self.previous_visible(history, self.selected.saturating_sub(1))?
        {
            self.selected = index;
        } else {
            self.selected = 0;
        }

        self.hexdump_scroll = 0;
        self.parsed_scroll = 0;
        Ok(())
    }

    fn next_visible(&self, history: &CaptureHistory, start: u64) -> Result<Option<u64>, TuiError> {
        for index in start..history.event_count() {
            let event = history.event_at(index)?;
            if self.displays_event(&event) {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    fn previous_visible(
        &self,
        history: &CaptureHistory,
        start: u64,
    ) -> Result<Option<u64>, TuiError> {
        if history.event_count() == 0 {
            return Ok(None);
        }

        let start = start.min(history.event_count() - 1);
        for index in (0..=start).rev() {
            let event = history.event_at(index)?;
            if self.displays_event(&event) {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }
}

fn move_index(current: usize, command: UiCommand, page_size: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }

    let last = len - 1;
    match command {
        UiCommand::Up => current.saturating_sub(1),
        UiCommand::Down => (current + 1).min(last),
        UiCommand::PageUp => current.saturating_sub(page_size.max(1)),
        UiCommand::PageDown => (current + page_size.max(1)).min(last),
        UiCommand::Home => 0,
        UiCommand::End => last,
        UiCommand::Quit | UiCommand::Space | UiCommand::Clear | UiCommand::NextPane => {
            current.min(last)
        }
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
                Ok(Some(UiCommand::Space)) => {
                    handle_space(client, state, capture_config, history)?;
                    *dirty = true;
                }
                Ok(Some(UiCommand::Clear)) => {
                    state.clear();
                    *dirty = true;
                }
                Ok(Some(UiCommand::NextPane)) => {
                    state.focus_next();
                    *dirty = true;
                }
                Ok(Some(command)) => {
                    state.move_focused(command, 10, history)?;
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

fn handle_space(
    client: &SocketIpcClient,
    state: &mut TuiState,
    capture_config: Option<CaptureConfig>,
    history: &CaptureHistory,
) -> Result<(), TuiError> {
    match state.focus {
        FocusPane::Transactions => toggle_recording(client, state, capture_config),
        FocusPane::Frequency => {
            if state.disable_selected_frequency() {
                state.select_nearest_visible(history)?;
                state.reload_visible_window(history)?;
            }
            Ok(())
        }
        FocusPane::Hexdump | FocusPane::Parsed => Ok(()),
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
                b' ' => Some(UiCommand::Space),
                b'c' => Some(UiCommand::Clear),
                b'\t' => Some(UiCommand::NextPane),
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
        FocusPane::Transactions.title(),
        state.focus == FocusPane::Transactions,
        width,
        height,
        |inner_width, inner_height| {
            let mut lines = Vec::with_capacity(inner_height);
            let visible_rows = inner_height.saturating_sub(1);
            let selected_offset = state
                .events
                .iter()
                .position(|entry| entry.history_index == state.selected)
                .unwrap_or_default();
            let start = if selected_offset >= visible_rows {
                selected_offset + 1 - visible_rows
            } else {
                0
            };
            let rows = (start..state.events.len().min(start + visible_rows))
                .map(|index| {
                    let entry = &state.events[index];
                    let event = &entry.event;
                    let summary = TransactionSummary::new(event, state.platform_methods);
                    (entry.history_index, event, summary)
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
                    direction(event),
                    summary.interface.as_str(),
                    &event.code.to_string(),
                    summary.method,
                    &format!("0x{:x}", event.data_size),
                );
                let color = transaction_color_index(&summary, event);
                if history_index == state.selected {
                    lines.push(theme::paint(theme::black_on_ansi256(color), fitted));
                } else {
                    lines.push(theme::paint(theme::ansi256_fg_style(color), fitted));
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
    direction: usize,
    interface: usize,
    code: usize,
    method: usize,
    len: usize,
}

impl TransactionColumns {
    fn new(width: usize, has_method: bool) -> Self {
        const SEQUENCE_WIDTH: usize = 8;
        const DIRECTION_WIDTH: usize = 5;
        const CODE_WIDTH: usize = 10;
        const LEN_WIDTH: usize = 8;
        const MIN_INTERFACE_WIDTH: usize = 18;
        const MAX_INTERFACE_WIDTH: usize = 56;
        const MIN_METHOD_WIDTH: usize = 12;
        const MAX_METHOD_WIDTH: usize = 28;

        let gap_width = if has_method { 5 } else { 4 };
        let available = width.saturating_sub(gap_width);
        let sequence = SEQUENCE_WIDTH.min(available);
        let remaining = available.saturating_sub(sequence);
        let direction = DIRECTION_WIDTH.min(remaining);
        let remaining = remaining.saturating_sub(direction);
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
            direction,
            interface,
            code,
            method,
            len,
        }
    }

    fn header(self) -> String {
        self.row("Seq", "Dir", "Interface", "#", "Method", "Len")
    }

    fn row(
        self,
        sequence: &str,
        direction: &str,
        interface: &str,
        code: &str,
        method: &str,
        len: &str,
    ) -> String {
        let line = if self.method == 0 {
            format!(
                "{} {} {} {} {}",
                fit_right(sequence, self.sequence),
                fit(direction, self.direction),
                fit(interface, self.interface),
                fit_right(code, self.code),
                fit_right(len, self.len),
            )
        } else {
            format!(
                "{} {} {} {} {} {}",
                fit_right(sequence, self.sequence),
                fit(direction, self.direction),
                fit(interface, self.interface),
                fit_right(code, self.code),
                fit(method, self.method),
                fit_right(len, self.len),
            )
        };
        fit(&line, self.width)
    }
}

const TRANSACTION_COLORS: &[u8] = &[
    39, 45, 51, 81, 87, 111, 117, 123, 159, 177, 183, 189, 204, 207, 213, 219, 222, 228,
];

fn transaction_color_index(summary: &TransactionSummary, event: &BinderEvent) -> u8 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    if summary.interface.is_empty() {
        hash = fnv1a(hash, b"<reply>");
        hash = fnv1a(hash, &event.code.to_le_bytes());
    } else {
        hash = fnv1a(hash, summary.interface.as_bytes());
    }

    TRANSACTION_COLORS[(hash as usize) % TRANSACTION_COLORS.len()]
}

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn render_frequency(state: &TuiState, width: usize, height: usize) -> Vec<String> {
    render_panel(
        FocusPane::Frequency.title(),
        state.focus == FocusPane::Frequency,
        width,
        height,
        |inner_width, inner_height| {
            let columns = FrequencyColumns::new(inner_width);
            let entries = frequency_entries(state);
            let visible_rows = inner_height.saturating_sub(1);
            let selected = state
                .frequency_selected
                .min(entries.len().saturating_sub(1));
            let start = visible_start(selected, visible_rows, entries.len());
            let mut lines = Vec::with_capacity(inner_height);
            lines.push(theme::paint(theme::MUTED, columns.header()));

            for (index, entry) in entries
                .into_iter()
                .enumerate()
                .skip(start)
                .take(inner_height.saturating_sub(1))
            {
                let filter = if state.disabled_frequency.contains(&entry.key()) {
                    "off"
                } else {
                    "on"
                };
                lines.push(columns.styled_row(&entry, filter, index == selected));
            }

            lines
        },
    )
}

fn visible_start(selected: usize, visible_rows: usize, len: usize) -> usize {
    if visible_rows == 0 || len <= visible_rows {
        return 0;
    }
    if selected >= visible_rows {
        (selected + 1 - visible_rows).min(len - visible_rows)
    } else {
        0
    }
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

    fn styled_row(self, entry: &FrequencyEntry, filter: &str, selected: bool) -> String {
        let count = fit_right(&entry.count.to_string(), self.count);
        let filter = fit_right(filter, self.filter);
        if selected {
            let separator = theme::paint(theme::FREQUENCY_SELECTED, " ");
            return format!(
                "{}{}{}{}{}",
                self.styled_label(entry, true),
                separator,
                theme::paint(theme::FREQUENCY_SELECTED, count),
                separator,
                theme::paint(theme::FREQUENCY_SELECTED, filter),
            );
        }

        format!(
            "{} {} {}",
            self.styled_label(entry, false),
            theme::paint(theme::FREQUENCY, count),
            theme::paint(theme::FREQUENCY, filter),
        )
    }

    fn styled_label(self, entry: &FrequencyEntry, selected: bool) -> String {
        let code = format!("#{}", entry.code);
        let code_width = code.chars().count();
        let label_style = if selected {
            theme::FREQUENCY_SELECTED
        } else {
            theme::FREQUENCY
        };
        if self.label == 0 {
            return String::new();
        }
        if code_width >= self.label {
            return theme::paint(label_style, fit(&code, self.label));
        }

        let interface = truncate_with_ellipsis(&entry.interface, self.label - code_width);
        let padding = self
            .label
            .saturating_sub(interface.chars().count() + code_width);
        format!(
            "{}{}{}",
            theme::paint(label_style, interface),
            theme::paint(label_style, code),
            theme::paint(label_style, " ".repeat(padding)),
        )
    }
}

fn render_hexdump(state: &TuiState, width: usize, height: usize) -> Vec<String> {
    render_panel(
        FocusPane::Hexdump.title(),
        state.focus == FocusPane::Hexdump,
        width,
        height,
        |inner_width, inner_height| {
            let Some(event) = state.selected_event() else {
                return vec![dim_line("等待 binder_transaction 事件", inner_width)];
            };

            let bytes = event.payload_bytes();
            if bytes.is_empty() {
                return vec![dim_line("payload 为空或未捕获", inner_width)];
            }

            bytes
                .chunks(16)
                .skip(state.hexdump_scroll)
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
                    let line = format!(
                        "{:04x}  {:<48}  {}",
                        (state.hexdump_scroll + row) * 16,
                        hex,
                        ascii
                    );
                    fit(&line, inner_width)
                })
                .collect()
        },
    )
}

const PARSED_LINE_COUNT: usize = 20;

fn render_parsed(state: &TuiState, width: usize, height: usize) -> Vec<String> {
    render_panel(
        FocusPane::Parsed.title(),
        state.focus == FocusPane::Parsed,
        width,
        height,
        |inner_width, inner_height| {
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
                .enumerate()
                .skip(state.parsed_scroll)
                .take(inner_height)
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

fn render_status(state: &TuiState, width: usize) -> [String; 2] {
    let selected = if state.total_events == 0 {
        "0/0".to_owned()
    } else {
        format!("{}/{}", state.selected + 1, state.total_events)
    };
    let (window_start, window_end) = visible_window_bounds(state);
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
        "Family: {}  SDK: {}  Focus: {}  Transactions: {}  Saved: {}  Window: {}-{}  History: {}  Recording: {}  Input: {}  Selected: {}  Uptime: {}s",
        state.family,
        sdk,
        state.focus.title(),
        state.stats.captured,
        state.total_events,
        window_start,
        window_end,
        state.history_path.display(),
        recording,
        input,
        selected,
        uptime
    );
    let key_text = key_hints(state);

    [
        theme::paint(theme::STATUS, fit(&status_text, width)),
        theme::paint(theme::MUTED, fit(&key_text, width)),
    ]
}

fn visible_window_bounds(state: &TuiState) -> (u64, u64) {
    let start = state
        .events
        .front()
        .map(|entry| entry.history_index)
        .unwrap_or_default();
    let end = state
        .events
        .back()
        .map(|entry| entry.history_index + 1)
        .unwrap_or(start);
    (start, end)
}

fn key_hints(state: &TuiState) -> String {
    match state.focus {
        FocusPane::Transactions => {
            let space = if state.recording {
                "space=pause capture"
            } else {
                "space=resume capture"
            };
            format!(
                "Keys: tab=focus  q=quit  {space}  c=clear  up/down=move transaction  page up/down=page  home/end=jump"
            )
        }
        FocusPane::Frequency => {
            "Keys: tab=focus  q=quit  space=disable interface/code  c=clear  up/down=move frequency  page up/down=page  home/end=jump".to_owned()
        }
        FocusPane::Hexdump => {
            "Keys: tab=focus  q=quit  c=clear  up/down=scroll hexdump  page up/down=page  home/end=jump".to_owned()
        }
        FocusPane::Parsed => {
            "Keys: tab=focus  q=quit  c=clear  up/down=scroll parsed  page up/down=page  home/end=jump".to_owned()
        }
    }
}

fn render_panel(
    title: &str,
    focused: bool,
    width: usize,
    height: usize,
    body: impl FnOnce(usize, usize) -> Vec<String>,
) -> Vec<String> {
    let width = width.max(8);
    let height = height.max(3);
    let inner_width = width.saturating_sub(2);
    let inner_height = height.saturating_sub(2);
    let mut lines = Vec::with_capacity(height);
    let border_style = if focused {
        theme::FOCUSED_BORDER
    } else {
        theme::BORDER
    };

    lines.push(top_border(title, width, focused, border_style));
    for line in body(inner_width, inner_height)
        .into_iter()
        .take(inner_height)
    {
        lines.push(format!(
            "{}{}{}",
            theme::paint(border_style, "│"),
            line,
            theme::paint(border_style, "│")
        ));
    }

    while lines.len() + 1 < height {
        lines.push(format!(
            "{}{}{}",
            theme::paint(border_style, "│"),
            " ".repeat(inner_width),
            theme::paint(border_style, "│")
        ));
    }

    lines.push(theme::paint(
        border_style,
        format!("└{}┘", "─".repeat(inner_width)),
    ));
    lines
}

fn top_border(title: &str, width: usize, focused: bool, style: anstyle::Style) -> String {
    let inner_width = width.saturating_sub(2);
    let title = if focused {
        format!(" [{title}] ")
    } else {
        format!(" {title} ")
    };
    if title.len() >= inner_width {
        return theme::paint(style, format!("┌{}┐", fit(&title, inner_width)));
    }

    let left = (inner_width - title.len()) / 2;
    let right = inner_width - title.len() - left;
    theme::paint(
        style,
        format!("┌{}{}{}┐", "─".repeat(left), title, "─".repeat(right)),
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

impl FrequencyEntry {
    fn key(&self) -> FrequencyKey {
        FrequencyKey {
            interface: self.interface.clone(),
            code: self.code,
        }
    }
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
        FocusPane, FrequencyColumns, FrequencyEntry, FrequencyKey, TransactionColumns,
        TransactionSummary, TuiState, UiCommand, fit, frequency_entries, render_panel,
        render_status, transaction_color_index, truncate_with_ellipsis, visible_window_bounds,
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
                "send",
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
            "reply",
            "android.content.pm.IPackageManager",
            "4294967295",
            "method",
            "0x10",
        );

        assert!(row.contains("4294967295"));
        assert!(row.contains("reply"));
    }

    #[test]
    fn transaction_columns_include_direction_header() {
        let columns = TransactionColumns::new(80, true);
        let header = columns.header();
        let row = columns.row("1", "send", "android.os.IMessenger", "1", "send", "0x20");

        assert!(header.contains("Dir"));
        assert!(row.contains("send"));
    }

    #[test]
    fn transaction_color_is_derived_from_interface_name() {
        let left_event = binder_event("android.os.IMessenger", 1, false);
        let right_event = binder_event("android.os.IMessenger", 42, false);
        let other_event = binder_event("android.app.IActivityManager", 1, false);
        let left = TransactionSummary::new(&left_event, Some(AndroidPlatformMethods::new(34)));
        let right = TransactionSummary::new(&right_event, Some(AndroidPlatformMethods::new(34)));
        let other = TransactionSummary::new(&other_event, Some(AndroidPlatformMethods::new(34)));

        assert_eq!(
            transaction_color_index(&left, &left_event),
            transaction_color_index(&right, &right_event)
        );
        assert_ne!(
            transaction_color_index(&left, &left_event),
            transaction_color_index(&other, &other_event)
        );
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
    fn frequency_disable_filters_display_without_dropping_history() {
        let path = temp_path("frequency-disable");
        let mut history = CaptureHistory::create(path.clone(), 4).expect("历史文件应可创建");
        let mut state = TuiState::new(12, 16, empty_stats(), true, Some(34), path.clone());
        let hidden = history
            .append_for_test(binder_event("android.os.IMessenger", 1, false))
            .expect("测试事件应可追加");
        let visible = history
            .append_for_test(binder_event("android.os.IMessenger", 2, false))
            .expect("测试事件应可追加");
        state.push_event(
            hidden,
            history.event_at(hidden).expect("隐藏事件应可从历史读取"),
        );
        state.push_event(
            visible,
            history.event_at(visible).expect("可见事件应可从历史读取"),
        );
        state.frequency_counts.insert(
            FrequencyKey {
                interface: "android.os.IMessenger".to_owned(),
                code: 1,
            },
            4,
        );

        assert!(state.disable_selected_frequency());
        state
            .select_nearest_visible(&history)
            .expect("过滤后应可重新选择可见事件");
        state
            .reload_visible_window(&history)
            .expect("过滤后应可重载可见窗口");

        assert_eq!(history.event_count(), 2);
        assert_eq!(state.selected, visible);
        assert_eq!(
            state
                .events
                .iter()
                .map(|entry| entry.event.code)
                .collect::<Vec<_>>(),
            vec![2]
        );

        let _ = fs::remove_file(path);
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
    fn frequency_styled_row_uses_srgb_color() {
        let columns = FrequencyColumns::new(40);
        let entry = FrequencyEntry {
            label: "android.os.IFoo#18".to_owned(),
            interface: "android.os.IFoo".to_owned(),
            code: 18,
            count: 7,
        };
        let row = columns.styled_row(&entry, "[+]", false);
        let plain = strip_ansi(&row);

        assert!(row.contains("\x1b[38;2;176;223;226mandroid.os.IFoo"));
        assert!(row.contains("\x1b[38;2;176;223;226m#18"));
        assert!(row.contains("\x1b[38;2;176;223;226m      7"));
        assert!(!row.contains("38;5;176"));
        assert_eq!(plain.chars().count(), 40);
        assert!(plain.contains("android.os.IFoo#18"));
    }

    #[test]
    fn frequency_selected_row_changes_text_colors() {
        let columns = FrequencyColumns::new(40);
        let entry = FrequencyEntry {
            label: "android.os.IFoo#18".to_owned(),
            interface: "android.os.IFoo".to_owned(),
            code: 18,
            count: 7,
        };
        let row = columns.styled_row(&entry, "[+]", true);
        let plain = strip_ansi(&row);

        assert!(row.contains("48;2;176;223;226"));
        assert!(row.contains("38;5;0"));
        assert!(!row.contains("48;5;176"));
        assert_eq!(plain.chars().count(), 40);
    }

    #[test]
    fn status_bar_separates_state_from_key_hints() {
        let mut state = TuiState::new(12, 16, empty_stats(), true, Some(34), temp_path("status"));
        let [status, keys] = render_status(&state, 96);
        let status = strip_ansi(&status);
        let keys = strip_ansi(&keys);

        assert_eq!(status.chars().count(), 96);
        assert_eq!(keys.chars().count(), 96);
        assert!(status.contains("Family: 12"));
        assert!(!status.contains("q=quit"));
        assert!(status.contains("Focus: Transactions"));
        assert!(keys.contains("Keys: tab=focus"));
        assert!(keys.contains("q=quit"));
        assert!(keys.contains("space=pause capture"));
        assert!(!keys.contains("h=help"));

        state.focus = FocusPane::Frequency;
        let keys = strip_ansi(&render_status(&state, 120)[1]);
        assert!(keys.contains("space=disable interface/code"));
        assert!(keys.contains("up/down=move frequency"));
        assert!(!keys.contains("space=pause capture"));

        state.focus = FocusPane::Hexdump;
        let keys = strip_ansi(&render_status(&state, 120)[1]);
        assert!(keys.contains("up/down=scroll hexdump"));
        assert!(!keys.contains("space="));
    }

    #[test]
    fn tab_focus_cycles_through_panels() {
        let mut state = TuiState::new(12, 16, empty_stats(), true, Some(34), temp_path("focus"));

        assert_eq!(state.focus, FocusPane::Transactions);
        state.focus_next();
        assert_eq!(state.focus, FocusPane::Frequency);
        state.focus_next();
        assert_eq!(state.focus, FocusPane::Hexdump);
        state.focus_next();
        assert_eq!(state.focus, FocusPane::Parsed);
        state.focus_next();
        assert_eq!(state.focus, FocusPane::Transactions);
    }

    #[test]
    fn focused_frequency_navigation_does_not_move_transaction_selection() {
        let path = temp_path("frequency-focus");
        let history = CaptureHistory::create(path.clone(), 2).expect("历史文件应可创建");
        let mut state = TuiState::new(12, 16, empty_stats(), true, Some(34), path.clone());
        state.total_events = 8;
        state.selected = 5;
        state.focus = FocusPane::Frequency;
        state.frequency_counts.insert(
            FrequencyKey {
                interface: "android.os.IFoo".to_owned(),
                code: 1,
            },
            3,
        );
        state.frequency_counts.insert(
            FrequencyKey {
                interface: "android.os.IBar".to_owned(),
                code: 2,
            },
            2,
        );

        state
            .move_focused(UiCommand::Down, 10, &history)
            .expect("频率焦点导航应成功");

        assert_eq!(state.selected, 5);
        assert_eq!(state.frequency_selected, 1);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn focused_detail_navigation_scrolls_without_moving_transaction_selection() {
        let path = temp_path("detail-focus");
        let history = CaptureHistory::create(path.clone(), 2).expect("历史文件应可创建");
        let mut state = TuiState::new(12, 16, empty_stats(), true, Some(34), path.clone());
        let event = binder_event("android.os.IMessenger", 1, false);
        state.push_event(0, event);

        state.focus = FocusPane::Hexdump;
        state
            .move_focused(UiCommand::Down, 10, &history)
            .expect("hexdump 焦点导航应成功");
        assert_eq!(state.selected, 0);
        assert_eq!(state.hexdump_scroll, 1);

        state.focus = FocusPane::Parsed;
        state
            .move_focused(UiCommand::Down, 10, &history)
            .expect("parsed 焦点导航应成功");
        assert_eq!(state.selected, 0);
        assert_eq!(state.parsed_scroll, 1);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn focused_panel_uses_solid_green_border() {
        let focused = render_panel("Frequency", true, 24, 3, |width, _| {
            vec![fit("body", width)]
        });
        let unfocused = render_panel("Frequency", false, 24, 3, |width, _| {
            vec![fit("body", width)]
        });
        let focused_top = strip_ansi(&focused[0]);
        let unfocused_top = strip_ansi(&unfocused[0]);

        assert!(focused[0].contains("\x1b[38;5;120m"));
        assert!(unfocused[0].contains("\x1b[38;5;15m"));
        assert!(focused_top.starts_with("┌"));
        assert!(focused_top.contains("[Frequency]"));
        assert!(focused_top.contains("─"));
        assert!(focused[1].contains("│"));
        assert!(focused[2].contains("└"));
        assert!(!unfocused_top.contains("[Frequency]"));
        assert!(unfocused_top.contains(" Frequency "));
        assert!(!focused_top.contains("="));
        assert!(!unfocused_top.contains("-"));
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
                .map(|entry| entry.event.sequence)
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

        assert_eq!(visible_window_bounds(&state), (3, 6));
        assert_eq!(state.selected, 5);
        assert_eq!(
            state
                .events
                .iter()
                .map(|entry| entry.event.sequence)
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
                .map(|entry| entry.event.sequence)
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
                .map(|entry| entry.event.sequence)
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
                .map(|entry| entry.event.sequence)
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
