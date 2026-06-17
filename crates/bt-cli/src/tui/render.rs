use std::io::{self, Stdout, Write};

use bt_agent::BinderEvent;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::terminal;

use super::text::{display_width, fit, fit_right, truncate_with_ellipsis};
use super::{
    FocusPane, FrequencyEntry, TransactionSummary, TuiState, direction, frequency_entries, theme,
};

pub(super) fn render(out: &mut Stdout, state: &TuiState) -> io::Result<()> {
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
                    let summary = state.transaction_summary(event);
                    (entry.history_index, event, summary)
                })
                .collect::<Vec<_>>();
            let sequence_width = rows
                .iter()
                .map(|(_, event, _)| display_width(&event.sequence.to_string()))
                .max()
                .unwrap_or_else(|| display_width("Seq"));
            let columns = TransactionColumns::new_with_sequence_width(inner_width, sequence_width);
            lines.push(theme::paint(theme::MUTED, columns.header()));

            for (history_index, event, summary) in rows {
                let fitted = columns.row(
                    &event.sequence.to_string(),
                    direction(event),
                    summary.interface.as_str(),
                    &summary.code.to_string(),
                    &format!("0x{:x}", event.data_size),
                    summary.method,
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
pub(super) struct TransactionColumns {
    pub(super) width: usize,
    pub(super) sequence: usize,
    pub(super) direction: usize,
    pub(super) interface: usize,
    pub(super) code: usize,
    pub(super) method: usize,
    pub(super) len: usize,
}

impl TransactionColumns {
    pub(super) fn new_with_sequence_width(width: usize, sequence_width: usize) -> Self {
        const DIRECTION_WIDTH: usize = 5;
        const CODE_WIDTH: usize = 10;
        const LEN_WIDTH: usize = 8;
        const MIN_INTERFACE_WIDTH: usize = 18;
        const MAX_INTERFACE_WIDTH: usize = 56;
        const MIN_METHOD_WIDTH: usize = 6;
        const MAX_METHOD_WIDTH: usize = 28;

        let available = width.saturating_sub(5);
        let sequence = sequence_width.max(display_width("Seq")).min(available);
        let remaining = available.saturating_sub(sequence);
        let direction = DIRECTION_WIDTH.min(remaining);
        let remaining = remaining.saturating_sub(direction);
        let code = CODE_WIDTH.min(remaining);
        let remaining = remaining.saturating_sub(code);
        let len = LEN_WIDTH.min(remaining);
        let remaining = remaining.saturating_sub(len);
        let can_show_method = remaining >= MIN_INTERFACE_WIDTH + MIN_METHOD_WIDTH;

        if !can_show_method {
            let available = width.saturating_sub(4);
            let sequence = sequence_width.max(display_width("Seq")).min(available);
            let remaining = available.saturating_sub(sequence);
            let direction = DIRECTION_WIDTH.min(remaining);
            let remaining = remaining.saturating_sub(direction);
            let code = CODE_WIDTH.min(remaining);
            let remaining = remaining.saturating_sub(code);
            let len = LEN_WIDTH.min(remaining);
            let remaining = remaining.saturating_sub(len);

            return Self {
                width,
                sequence,
                direction,
                interface: remaining.min(MAX_INTERFACE_WIDTH),
                code,
                method: 0,
                len,
            };
        }

        let interface = remaining
            .saturating_sub(MIN_METHOD_WIDTH)
            .min(MAX_INTERFACE_WIDTH);
        let method = remaining.saturating_sub(interface).min(MAX_METHOD_WIDTH);

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

    pub(super) fn header(self) -> String {
        self.row("Seq", "Dir", "Interface", "#", "Len", "Method")
    }

    pub(super) fn row(
        self,
        sequence: &str,
        direction: &str,
        interface: &str,
        code: &str,
        len: &str,
        method: &str,
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
                fit_right(len, self.len),
                fit(method, self.method),
            )
        };
        fit(&line, self.width)
    }
}

const TRANSACTION_COLORS: &[u8] = &[
    39, 45, 51, 81, 87, 111, 117, 123, 159, 177, 183, 189, 204, 207, 213, 219, 222, 228,
];

pub(super) fn transaction_color_index(summary: &TransactionSummary, event: &BinderEvent) -> u8 {
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
pub(super) struct FrequencyColumns {
    width: usize,
    label: usize,
    count: usize,
    filter: usize,
}

impl FrequencyColumns {
    pub(super) fn new(width: usize) -> Self {
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

    pub(super) fn header(self) -> String {
        self.row("Interface/Code", "Frequency", "Filter")
    }

    pub(super) fn row(self, label: &str, count: &str, filter: &str) -> String {
        let line = format!(
            "{} {} {}",
            fit(label, self.label),
            fit_right(count, self.count),
            fit_right(filter, self.filter),
        );
        fit(&line, self.width)
    }

    pub(super) fn styled_row(self, entry: &FrequencyEntry, filter: &str, selected: bool) -> String {
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
        let code_style = if selected {
            theme::FREQUENCY_SELECTED
        } else {
            theme::FREQUENCY_CODE
        };
        if self.label == 0 {
            return String::new();
        }
        if code_width >= self.label {
            return theme::paint(code_style, fit(&code, self.label));
        }

        let interface = truncate_with_ellipsis(&entry.interface, self.label - code_width);
        let padding = self
            .label
            .saturating_sub(display_width(&interface) + code_width);
        format!(
            "{}{}{}",
            theme::paint(label_style, interface),
            theme::paint(code_style, code),
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

pub(super) const PARSED_LINE_COUNT: usize = 22;

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
            let summary = state.transaction_summary(event);

            let lines = [
                theme::paint(theme::TITLE, fit("binder_transaction", inner_width)),
                format!("interface: {}", summary.interface.as_str()),
                format!("code: {}", summary.code),
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
                format!("transaction_debug_id: {}", event.transaction_debug_id),
                format!("reply_to_debug_id: {}", event.reply_to_debug_id),
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

pub(super) fn render_status(state: &TuiState, width: usize) -> [String; 2] {
    let selected = if state.total_events == 0 {
        "0/0".to_owned()
    } else {
        format!("{}/{}", state.selected + 1, state.total_events)
    };
    let uptime = state.start.elapsed().as_secs();
    let sdk = state
        .android_sdk
        .map(|sdk| sdk.to_string())
        .unwrap_or_else(|| state.language.unknown().to_owned());
    let status_text = state.language.status_text(state, &selected, &sdk, uptime);
    let key_text = key_hints(state);

    [
        theme::paint(theme::STATUS, fit(&status_text, width)),
        theme::paint(theme::MUTED, fit(&key_text, width)),
    ]
}

pub(super) fn visible_window_bounds(state: &TuiState) -> (u64, u64) {
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
    state.language.key_hints(state)
}

pub(super) fn render_panel(
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
