//! agent 对外暴露的错误面。
//!
//! # 职责
//! - 保留具体错误变体，让调用方不需要解析展示字符串就能区分解码、I/O 或写入失败。

use std::io;

use bt_decoder::DecodeError;
use bt_storage::StorageError;
use thiserror::Error;

use crate::socket_ipc::SocketIpcError;

/// 配置或运行 agent 时可能返回的错误。
#[derive(Debug, Error)]
pub enum AgentError {
    /// raw 事件解码失败。
    #[error("failed to decode event: {0}")]
    Decode(#[from] DecodeError),
    /// 内核 socket 控制或事件读取失败。
    #[error("failed to access socket event stream: {0}")]
    Socket(#[from] SocketIpcError),
    /// 内核模块没有声明事件流能力。
    #[error("binder-trace kernel module does not support the socket event stream")]
    EventStreamUnsupported,
    /// agent 自己负责的文件或进程 I/O 失败。
    #[error("agent I/O failed: {0}")]
    Io(#[from] io::Error),
    /// 事件序列化或 sink flush 失败。
    #[error("failed to persist event: {0}")]
    Storage(#[from] StorageError),
}
