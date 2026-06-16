#![no_std]

//! 内核采集侧与用户态共享的固定布局数据结构。
//!
//! # 职责
//! - 定义跨 crate、跨内核模块/用户态边界传递的 raw event ABI。
//! - 保持结构体布局简单稳定，避免在内核态依赖堆分配或复杂类型。

mod constants;
mod device;
mod event;
mod kind;

pub use constants::MAX_INLINE_PAYLOAD;
pub use device::BinderDevice;
pub use event::{RawBinderEvent, RawEventHeader, RawTransaction};
pub use kind::{EventKind, UnknownEventKind};
