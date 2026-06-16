//! 解码后的用户态事件模型。

use bt_common::{BinderDevice, EventKind};

/// 解码后的通用 Binder 事件。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DecodedEvent {
    /// 事件类型。
    pub kind: EventKind,
    /// Binder 设备。
    pub device: BinderDevice,
    /// 当前进程 pid。
    pub pid: u32,
    /// 当前线程 tid。
    pub tid: u32,
    /// 当前 uid。
    pub uid: u32,
    /// 事件 flags。
    pub flags: u32,
    /// 事件时间戳，单位纳秒。
    pub timestamp_ns: u64,
    /// 生产者侧序号。
    pub sequence: u64,
    /// transaction 详情；诊断事件没有 transaction。
    pub transaction: Option<DecodedTransaction>,
}

/// 解码后的 Binder transaction 信息。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DecodedTransaction {
    /// Binder transaction code。
    pub code: u32,
    /// Binder transaction flags。
    pub flags: u32,
    /// Binder data 区大小。
    pub data_size: u64,
    /// Binder offsets 区大小。
    pub offsets_size: u64,
    /// 目标 handle。
    pub target_handle: u32,
    /// 发送方 pid。
    pub sender_pid: u32,
    /// 发送方有效 uid。
    pub sender_euid: u32,
    /// payload 是否因为 inline 上限或解码配置而被截断。
    pub payload_truncated: bool,
    /// 解码后保留的 payload 字节。
    pub payload: Vec<u8>,
}
