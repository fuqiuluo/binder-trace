use std::env;
use std::process::Command;
use std::sync::OnceLock;

use serde::Deserialize;

use super::{FocusPane, TuiState, render::visible_window_bounds};

const EN_US_MESSAGES: &[u8] = include_bytes!("locales/en-US.json");
const ZH_CN_MESSAGES: &[u8] = include_bytes!("locales/zh-CN.json");
const JA_JP_MESSAGES: &[u8] = include_bytes!("locales/ja-JP.json");

static EN_US: OnceLock<Messages> = OnceLock::new();
static ZH_CN: OnceLock<Messages> = OnceLock::new();
static JA_JP: OnceLock<Messages> = OnceLock::new();

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum UiLanguage {
    English,
    Chinese,
    Japanese,
}

impl UiLanguage {
    pub(super) fn detect() -> Self {
        Self::from_android_system()
            .or_else(Self::from_env)
            .unwrap_or(Self::English)
    }

    fn from_env() -> Option<Self> {
        ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"]
            .into_iter()
            .filter_map(|name| env::var(name).ok())
            .find_map(|value| Self::from_locale_list(&value))
    }

    fn from_android_system() -> Option<Self> {
        [
            "persist.sys.locale",
            "ro.product.locale",
            "ro.product.locale.language",
        ]
        .into_iter()
        .filter_map(android_property)
        .find_map(|value| Self::from_locale_list(&value))
    }

    pub(super) fn from_locale_list(value: &str) -> Option<Self> {
        value.split(':').find_map(Self::from_locale)
    }

    pub(super) fn from_locale(value: &str) -> Option<Self> {
        let locale = value.trim().to_ascii_lowercase().replace('-', "_");
        if locale.starts_with("zh") {
            Some(Self::Chinese)
        } else if locale.starts_with("ja") {
            Some(Self::Japanese)
        } else if locale.starts_with("en") {
            Some(Self::English)
        } else {
            None
        }
    }

    pub(super) fn unknown(self) -> &'static str {
        &self.messages().unknown
    }

    pub(super) fn status_text(
        self,
        state: &TuiState,
        selected: &str,
        sdk: &str,
        uptime: u64,
    ) -> String {
        let messages = self.messages();
        let (window_start, window_end) = visible_window_bounds(state);
        let family = state.family.to_string();
        let focus = self.focus_title(state.focus);
        let transactions = state.stats.captured.to_string();
        let saved = state.total_events.to_string();
        let window_start = window_start.to_string();
        let window_end = window_end.to_string();
        let history = state.history_path.display().to_string();
        let recording = self.bool_text(state.recording);
        let input = self.bool_text(state.input_available);
        let uptime = uptime.to_string();

        render_template(
            &messages.status,
            &[
                ("family", &family),
                ("sdk", sdk),
                ("focus", focus),
                ("transactions", &transactions),
                ("saved", &saved),
                ("window_start", &window_start),
                ("window_end", &window_end),
                ("history", &history),
                ("recording", recording),
                ("input", input),
                ("selected", selected),
                ("uptime", &uptime),
            ],
        )
    }

    pub(super) fn key_hints(self, state: &TuiState) -> String {
        let messages = self.messages();
        match state.focus {
            FocusPane::Transactions => {
                let space = if state.recording {
                    &messages.keys.transactions.recording_space
                } else {
                    &messages.keys.transactions.paused_space
                };
                render_template(&messages.keys.transactions.template, &[("space", space)])
            }
            FocusPane::Frequency => messages.keys.frequency.clone(),
            FocusPane::Hexdump => messages.keys.hexdump.clone(),
            FocusPane::Parsed => messages.keys.parsed.clone(),
        }
    }

    fn messages(self) -> &'static Messages {
        match self {
            Self::English => EN_US.get_or_init(|| parse_messages("en-US", EN_US_MESSAGES)),
            Self::Chinese => ZH_CN.get_or_init(|| parse_messages("zh-CN", ZH_CN_MESSAGES)),
            Self::Japanese => JA_JP.get_or_init(|| parse_messages("ja-JP", JA_JP_MESSAGES)),
        }
    }

    fn bool_text(self, value: bool) -> &'static str {
        let messages = self.messages();
        if value {
            &messages.boolean.enabled
        } else {
            &messages.boolean.disabled
        }
    }

    fn focus_title(self, pane: FocusPane) -> &'static str {
        let messages = self.messages();
        match pane {
            FocusPane::Transactions => &messages.focus.transactions,
            FocusPane::Frequency => &messages.focus.frequency,
            FocusPane::Hexdump => &messages.focus.hexdump,
            FocusPane::Parsed => &messages.focus.parsed,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Messages {
    unknown: String,
    boolean: BooleanMessages,
    focus: FocusMessages,
    status: String,
    keys: KeyMessages,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct BooleanMessages {
    enabled: String,
    disabled: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FocusMessages {
    transactions: String,
    frequency: String,
    hexdump: String,
    parsed: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyMessages {
    transactions: TransactionKeyMessages,
    frequency: String,
    hexdump: String,
    parsed: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionKeyMessages {
    template: String,
    recording_space: String,
    paused_space: String,
}

fn parse_messages(language: &str, resource: &[u8]) -> Messages {
    match serde_json::from_slice(resource) {
        Ok(messages) => messages,
        Err(error) => panic!("{language} locale resource is invalid: {error}"),
    }
}

fn render_template(template: &str, replacements: &[(&str, &str)]) -> String {
    replacements
        .iter()
        .fold(template.to_owned(), |rendered, (key, value)| {
            rendered.replace(&format!("{{{key}}}"), value)
        })
}

fn android_property(name: &str) -> Option<String> {
    let output = Command::new("getprop").arg(name).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{EN_US_MESSAGES, JA_JP_MESSAGES, Messages, ZH_CN_MESSAGES};

    #[test]
    fn bundled_messages_are_valid_json() {
        for (name, resource) in [
            ("en-US", EN_US_MESSAGES),
            ("zh-CN", ZH_CN_MESSAGES),
            ("ja-JP", JA_JP_MESSAGES),
        ] {
            if let Err(error) = serde_json::from_slice::<Messages>(resource) {
                panic!("{name} locale resource is invalid: {error}");
            }
        }
    }
}
