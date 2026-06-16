//! raw event 解码错误。

use std::fmt;

use bt_common::UnknownEventKind;

/// raw event 解码失败原因。
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum DecodeError {
    /// raw event kind 不在当前版本支持的范围内。
    UnknownEventKind(u16),
    /// raw payload 长度超过固定 inline payload 缓冲区。
    PayloadLengthOutOfRange(u32),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEventKind(kind) => write!(f, "unknown raw event kind: {kind}"),
            Self::PayloadLengthOutOfRange(length) => {
                write!(
                    f,
                    "raw payload length exceeds inline event payload: {length}"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<UnknownEventKind> for DecodeError {
    fn from(error: UnknownEventKind) -> Self {
        Self::UnknownEventKind(error.raw())
    }
}
