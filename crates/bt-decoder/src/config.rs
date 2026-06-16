//! 解码器配置。

use bt_common::MAX_INLINE_PAYLOAD;

/// 解码器配置。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DecoderConfig {
    /// decoded event 中最多保留的 payload 字节数。
    pub max_payload_bytes: usize,
}

impl Default for DecoderConfig {
    fn default() -> Self {
        Self {
            max_payload_bytes: MAX_INLINE_PAYLOAD,
        }
    }
}
