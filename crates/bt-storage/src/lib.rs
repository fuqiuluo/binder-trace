//! 采集器和具体持久化后端共享的存储 API。
//!
//! # 职责
//! - 固定当前 JSONL 输出需要的信封和程序版本记录。
//! - 让后续内核模块 reader 在接入时再引入新的事件 DTO。

mod error;
mod event;
mod jsonl;

pub use error::StorageError;
pub use event::{EventEnvelope, ProgramVersion};
pub use jsonl::JsonlSink;
