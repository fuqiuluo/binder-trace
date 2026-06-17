use std::fmt;

use anstyle::{Ansi256Color, AnsiColor, Color, RgbColor, Style};

pub(super) const BORDER: Style = ansi256_fg(15);
pub(super) const FOCUSED_BORDER: Style = ansi256_fg(120);
pub(super) const FREQUENCY: Style = rgb_fg(176, 223, 226);
pub(super) const FREQUENCY_CODE: Style = ansi256_fg(226);
pub(super) const FREQUENCY_SELECTED: Style = black_on_rgb(176, 223, 226);
pub(super) const MUTED: Style = Style::new().dimmed();
pub(super) const TITLE: Style = Style::new().bold();
pub(super) const SELECTED: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Black)))
    .bg_color(Some(Color::Ansi(AnsiColor::Cyan)));
pub(super) const STATUS: Style = SELECTED;

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

pub(super) fn ansi256_fg_style(index: u8) -> Style {
    ansi256_fg(index)
}

pub(super) fn black_on_ansi256(index: u8) -> Style {
    Style::new()
        .fg_color(Some(Color::Ansi(AnsiColor::Black)))
        .bg_color(Some(Color::Ansi256(Ansi256Color(index))))
}

pub(super) fn paint(style: Style, text: impl fmt::Display) -> String {
    format!("{style}{text}{style:#}")
}
