//! 启动诊断事件构造。
//!
//! # 职责
//! - 在采集器加载前生成诊断事件，让下游能区分“agent 已启动”和“采集器已挂载”。
//! - 为启动事件填入进程号和当前时间，供消息队列侧做基础排序与排障。

use std::time::{SystemTime, UNIX_EPOCH};

use bt_common::{BinderDevice, EventKind, RawBinderEvent};

pub(crate) fn startup_event() -> RawBinderEvent {
    let mut event = RawBinderEvent::new(EventKind::Diagnostic, BinderDevice::UNKNOWN);
    event.header.pid = std::process::id();
    event.header.timestamp_ns = timestamp_ns();
    event
}

fn timestamp_ns() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos().min(u128::from(u64::MAX)) as u64,
        // 系统时间异常时仍然要发出启动事件；0 明确表示时间不可用。
        Err(_) => 0,
    }
}
