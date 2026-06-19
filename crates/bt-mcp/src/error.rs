use std::io;

use bt_agent::HistoryError;
use bt_agent::SocketIpcError;
use thiserror::Error;

/// MCP 服务启动或运行失败。
#[derive(Debug, Error)]
pub enum McpServerError {
    /// socket IPC 连接、控制或事件读取失败。
    #[error("{0}")]
    SocketIpc(#[from] SocketIpcError),
    /// 当前内核模块没有事件流能力。
    #[error("当前内核模块不支持 socket 事件流，请重新加载新版 bt-kmod")]
    EventStreamUnsupported,
    /// btcap 历史文件读写失败。
    #[error("{0}")]
    History(#[from] HistoryError),
    /// MCP runtime 或 HTTP transport 发生 IO 错误。
    #[error("{0}")]
    Io(#[from] io::Error),
    /// MCP 初始化握手失败。
    #[error("MCP 初始化失败: {0}")]
    Initialize(String),
    /// MCP 服务任务异常退出。
    #[error("MCP 服务退出失败: {0}")]
    Join(String),
}
