//! 存储层共享的事件信封类型。
//!
//! # 职责
//! - 定义 JSONL 输出使用的信封元数据。
//! - 保持启动事件和后续采集事件的路由字段一致。

/// 每条持久化事件都会携带的信封元数据。
///
/// 信封借用由生产者在一次写调用期间持有；后端如果需要在返回后保留字段，必须自行拷贝。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct EventEnvelope<'a> {
    /// 稳定设备标识，用于关联同一台 Android 设备产生的事件。
    pub device_id: &'a str,
    /// 事件时间戳，单位为 Unix epoch 以来的纳秒。
    pub timestamp_ns: u64,
    /// 逻辑事件类型，例如 `agent.diagnostic` 或 `binder.transaction`。
    pub object: &'a str,
}

/// 启动阶段以普通事件形式输出的程序元数据。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ProgramVersion<'a> {
    /// 程序或组件名称。
    pub program: &'a str,
    /// 组件上报的版本字符串。
    pub version: &'a str,
}
