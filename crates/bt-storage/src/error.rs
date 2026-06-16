//! 所有存储后端共享的错误类型。

use std::fmt;
use std::io;

/// 持久化事件时返回的失败原因。
///
/// 各后端应在模块边界把具体 I/O 或编码错误转换为该类型，让调用方统一处理存储失败。
#[derive(Debug)]
pub enum StorageError {
    /// 底层 writer 或文件操作失败。
    Io(io::Error),
    /// JSONL 序列化在事件完整写出前失败。
    Json(serde_json::Error),
    /// 后端本地消息序号已经达到 `u64::MAX`。
    SequenceOverflow,
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "storage I/O failed: {error}"),
            Self::Json(error) => write!(f, "failed to serialize JSON event: {error}"),
            Self::SequenceOverflow => write!(f, "message sequence overflow"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<io::Error> for StorageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
