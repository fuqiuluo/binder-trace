//! Binder trace 的 MCP 在线服务入口。
//!
//! # 职责
//! - 通过 MCP Streamable HTTP transport 暴露实时 Binder trace 查询工具。
//! - 在显式授权后允许 MCP tool 修改内核捕获配置。
//! - 复用 `bt-agent` 的 btcap 历史文件，按需查询当前会话完整事件。

mod error;
mod server;

pub use error::McpServerError;
pub use server::{McpServerConfig, serve_http_blocking};
