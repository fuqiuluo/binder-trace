//! Binder trace 的用户态 agent。
//!
//! # 职责
//! - 为 CLI 和后续 service 入口提供小而稳定的公开接口。
//! - 把运行编排、采集后端、启动诊断、设备身份和错误建模拆到内部模块。
//!
//! # 不变量
//! - 外部调用者只通过这里 re-export 的类型组合 agent，不直接依赖内部模块布局。

mod agent;
mod capture_history;
mod config;
mod device;
mod diagnostic;
mod error;
mod socket_ipc;

pub use agent::Agent;
pub use capture_history::{CaptureHistory, HistoryError};
pub use config::{AgentConfig, OutputConfig};
pub use error::AgentError;
pub use socket_ipc::{
    BinderEvent, CaptureConfig, CaptureStats, DriverFeature, SocketIpcClient, SocketIpcError,
};
