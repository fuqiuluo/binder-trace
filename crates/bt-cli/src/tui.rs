//! 终端实时界面。
//!
//! # 职责
//! - 用固定布局展示 Binder transaction 事件、频率统计和当前事件详情。
//! - 通过 alternate screen 和原地覆盖降低 adb shell 下的闪烁。
//! - 处理捕获开关、清空、滚动和退出等键盘交互。

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Stdout, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

use bt_agent::{BinderEvent, CaptureConfig, CaptureStats, SocketIpcClient, SocketIpcError};
use bt_decoder::{AndroidPlatformMethods, parse_interface_token};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};

mod theme {
    use std::fmt;

    use anstyle::{Ansi256Color, AnsiColor, Color, Style};

    pub const BORDER: Style = ansi256_fg(120);
    pub const TRANSACTION_SEND: Style = ansi256_fg(51);
    pub const TRANSACTION_REPLY: Style = ansi256_fg(226);
    pub const FREQUENCY: Style = ansi256_fg(120);
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

#[derive(Debug, Clone, Copy)]
pub struct TuiConfig {
    pub rows: usize,
    pub refresh: Duration,
    pub capture_config: Option<CaptureConfig>,
    pub android_sdk: Option<u16>,
}

#[derive(Debug)]
pub enum TuiError {
    Socket(SocketIpcError),
    Io(io::Error),
}

impl fmt::Display for TuiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
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
    selected: usize,
    stats: CaptureStats,
    recording: bool,
    help_visible: bool,
    input_available: bool,
    android_sdk: Option<u16>,
    platform_methods: Option<AndroidPlatformMethods>,
    start: Instant,
}

impl TuiState {
    fn new(
        family: i32,
        capacity: usize,
        stats: CaptureStats,
        input_available: bool,
        android_sdk: Option<u16>,
    ) -> Self {
        Self {
            family,
            capacity: capacity.max(1),
            events: VecDeque::with_capacity(capacity.max(1)),
            selected: 0,
            stats,
            recording: true,
            help_visible: false,
            input_available,
            android_sdk,
            platform_methods: android_sdk.map(AndroidPlatformMethods::new),
            start: Instant::now(),
        }
    }

    fn push_event(&mut self, event: BinderEvent) {
        let follows_tail = self.events.is_empty() || self.selected + 1 >= self.events.len();

        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.selected = self.selected.saturating_sub(1);
        }

        self.events.push_back(event);
        if follows_tail {
            self.selected = self.events.len().saturating_sub(1);
        }
    }

    fn clear(&mut self) {
        self.events.clear();
        self.selected = 0;
    }

    fn selected_event(&self) -> Option<&BinderEvent> {
        self.events.get(self.selected)
    }

    fn move_selection(&mut self, command: UiCommand, page_size: usize) {
        if self.events.is_empty() {
            self.selected = 0;
            return;
        }

        let last = self.events.len() - 1;
        self.selected = match command {
            UiCommand::Up => self.selected.saturating_sub(1),
            UiCommand::Down => (self.selected + 1).min(last),
            UiCommand::PageUp => self.selected.saturating_sub(page_size.max(1)),
            UiCommand::PageDown => (self.selected + page_size.max(1)).min(last),
            UiCommand::Home => 0,
            UiCommand::End => last,
            UiCommand::Quit | UiCommand::ToggleRecording | UiCommand::Clear | UiCommand::Help => {
                self.selected
            }
        };
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
    let mut terminal = TerminalSession::enter()?;
    let capture_guard = CaptureGuard::new(client, config.capture_config);
    let mut state = TuiState::new(
        family,
        config.rows.max(1),
        client.get_stats()?,
        terminal.input.is_some(),
        config.android_sdk,
    );
    let refresh = config
        .refresh
        .clamp(Duration::from_millis(50), Duration::from_secs(5));
    let mut last_stats_at = Instant::now();
    let mut next_frame_at = Instant::now();
    let mut dirty = true;

    render(&mut terminal.stdout, &state)?;

    loop {
        if let Some(input) = terminal.input.as_mut() {
            match input.read_command() {
                Ok(Some(UiCommand::Quit)) => break,
                Ok(Some(UiCommand::ToggleRecording)) => {
                    toggle_recording(client, &mut state, config.capture_config)?;
                    dirty = true;
                }
                Ok(Some(UiCommand::Clear)) => {
                    state.clear();
                    dirty = true;
                }
                Ok(Some(UiCommand::Help)) => {
                    state.help_visible = !state.help_visible;
                    dirty = true;
                }
                Ok(Some(command)) => {
                    state.move_selection(command, 10);
                    dirty = true;
                }
                Ok(None) => {}
                Err(_) => {
                    terminal.input = None;
                    state.input_available = false;
                    dirty = true;
                }
            }
        }

        if client.poll_event(Duration::from_millis(20))? {
            while let Some(event) = client.try_recv_event()? {
                if state.recording && event.is_binder_transaction() {
                    state.push_event(event);
                    dirty = true;
                }
            }
        }

        if last_stats_at.elapsed() >= Duration::from_secs(1) {
            state.stats = client.get_stats()?;
            last_stats_at = Instant::now();
            dirty = true;
        }

        if dirty && Instant::now() >= next_frame_at {
            render(&mut terminal.stdout, &state)?;
            next_frame_at = Instant::now() + refresh;
            dirty = false;
        }
    }

    drop(capture_guard);
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
    let (width, height) = terminal::size().unwrap_or((120, 36));
    let width = usize::from(width).max(72);
    let height = usize::from(height).max(20);
    let content_height = height.saturating_sub(1);
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

    queue!(out, MoveTo(0, content_height as u16))?;
    write!(out, "{status}")?;
    out.flush()
}

fn render_transactions(state: &TuiState, width: usize, height: usize) -> Vec<String> {
    render_panel(
        "Transactions",
        width,
        height,
        |inner_width, inner_height| {
            let columns = TransactionColumns::new(inner_width);
            let mut lines = Vec::with_capacity(inner_height);
            lines.push(theme::paint(theme::MUTED, columns.header()));

            let visible_rows = inner_height.saturating_sub(1);
            let start = if state.selected >= visible_rows {
                state.selected + 1 - visible_rows
            } else {
                0
            };

            for index in start..state.events.len().min(start + visible_rows) {
                let event = &state.events[index];
                let summary = TransactionSummary::new(event, state.platform_methods);
                let fitted = columns.row(
                    summary.interface.as_str(),
                    &event.code.to_string(),
                    summary.method,
                    &format!("0x{:x}", event.data_size),
                );
                if index == state.selected {
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
    interface: usize,
    code: usize,
    method: usize,
    len: usize,
}

impl TransactionColumns {
    fn new(width: usize) -> Self {
        const GAP_WIDTH: usize = 3;
        const CODE_WIDTH: usize = 10;
        const LEN_WIDTH: usize = 10;
        const MIN_INTERFACE_WIDTH: usize = 10;
        const PREFERRED_METHOD_WIDTH: usize = 22;

        let available = width.saturating_sub(GAP_WIDTH);
        let code = CODE_WIDTH.min(available);
        let remaining = available.saturating_sub(code);
        let len = LEN_WIDTH.min(remaining);
        let remaining = remaining.saturating_sub(len);
        let method = if remaining > MIN_INTERFACE_WIDTH {
            PREFERRED_METHOD_WIDTH.min(remaining - MIN_INTERFACE_WIDTH)
        } else {
            remaining / 3
        };
        let interface = remaining.saturating_sub(method);

        Self {
            interface,
            code,
            method,
            len,
        }
    }

    fn header(self) -> String {
        self.row("Interface", "#", "Method", "Len")
    }

    fn row(self, interface: &str, code: &str, method: &str, len: &str) -> String {
        format!(
            "{} {} {} {}",
            fit(interface, self.interface),
            fit_right(code, self.code),
            fit(method, self.method),
            fit_right(len, self.len),
        )
    }
}

fn render_frequency(state: &TuiState, width: usize, height: usize) -> Vec<String> {
    render_panel("Frequency", width, height, |inner_width, inner_height| {
        let mut lines = Vec::with_capacity(inner_height);
        lines.push(dim_line(
            "Process/Direction            Frequency Filter",
            inner_width,
        ));

        for entry in frequency_entries(state)
            .into_iter()
            .take(inner_height.saturating_sub(1))
        {
            let label_width = inner_width.saturating_sub(16).max(8);
            let line = format!(
                "{:<label_width$} {:>8}   [+]",
                entry.label,
                entry.count,
                label_width = label_width
            );
            lines.push(theme::paint(theme::FREQUENCY, fit(&line, inner_width)));
        }

        lines
    })
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

fn render_status(state: &TuiState, width: usize) -> String {
    let selected = if state.events.is_empty() {
        "0/0".to_owned()
    } else {
        format!("{}/{}", state.selected + 1, state.events.len())
    };
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
    let text = format!(
        "Family: {}  SDK: {}  Transactions: {}  Filter: [tgid=*, pid=*, uid=*]  Recording: {}  Input: {}  Selected: {}  Uptime: {}s  q=quit h=help space=toggle c=clear",
        state.family, sdk, state.stats.captured, recording, input, selected, uptime
    );
    theme::paint(theme::STATUS, fit(&text, width))
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
    let mut result: String = text.chars().take(width).collect();
    let visible = result.chars().count();
    if visible < width {
        result.push_str(&" ".repeat(width - visible));
    }
    result
}

fn fit_right(text: &str, width: usize) -> String {
    let result: String = text.chars().take(width).collect();
    let visible = result.chars().count();
    if visible < width {
        format!("{}{}", " ".repeat(width - visible), result)
    } else {
        result
    }
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
    count: u64,
}

fn frequency_entries(state: &TuiState) -> Vec<FrequencyEntry> {
    let mut counts = BTreeMap::<String, u64>::new();
    for event in &state.events {
        let label = format!("uid={} tgid={} {}", event.uid, event.tgid, direction(event));
        *counts.entry(label).or_default() += 1;
    }

    let mut entries = counts
        .into_iter()
        .map(|(label, count)| FrequencyEntry { label, count })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.label.cmp(&right.label))
    });
    entries
}
