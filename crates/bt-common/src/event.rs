//! raw Binder event ABI。

use crate::{BinderDevice, EventKind, MAX_INLINE_PAYLOAD};

/// raw Binder event 的通用头部。
#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RawEventHeader {
    /// [`EventKind`] 的 raw 数值。
    pub kind: u16,
    /// [`BinderDevice`] 的 raw 数值。
    pub device: u16,
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
}

impl RawEventHeader {
    /// 创建一个指定 kind 和设备、其余字段清零的 raw event 头部。
    pub const fn new(kind: EventKind, device: BinderDevice) -> Self {
        Self {
            kind: kind.as_raw(),
            device: device.as_raw(),
            pid: 0,
            tid: 0,
            uid: 0,
            flags: 0,
            timestamp_ns: 0,
            sequence: 0,
        }
    }
}

/// raw Binder transaction 数据。
#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RawTransaction {
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
    /// `payload` 中有效字节数。
    pub payload_len: u32,
    /// payload 是否被 inline 表达截断。
    pub payload_truncated: u8,
    /// 结构体显式 padding，保持 C layout 稳定。
    pub reserved: [u8; 7],
    /// inline payload 缓冲区。
    pub payload: [u8; MAX_INLINE_PAYLOAD],
}

impl RawTransaction {
    /// 创建一个所有字段清零的 transaction。
    pub const fn empty() -> Self {
        Self {
            code: 0,
            flags: 0,
            data_size: 0,
            offsets_size: 0,
            target_handle: 0,
            sender_pid: 0,
            sender_euid: 0,
            payload_len: 0,
            payload_truncated: 0,
            reserved: [0; 7],
            payload: [0; MAX_INLINE_PAYLOAD],
        }
    }
}

impl Default for RawTransaction {
    fn default() -> Self {
        Self::empty()
    }
}

/// raw Binder event。
#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RawBinderEvent {
    /// 通用事件头部。
    pub header: RawEventHeader,
    /// transaction 事件详情；诊断事件中为空值。
    pub transaction: RawTransaction,
}

impl RawBinderEvent {
    /// 创建一个指定 kind 和设备、其余字段清零的 raw Binder event。
    pub const fn new(kind: EventKind, device: BinderDevice) -> Self {
        Self {
            header: RawEventHeader::new(kind, device),
            transaction: RawTransaction::empty(),
        }
    }
}

impl Default for RawBinderEvent {
    fn default() -> Self {
        Self::new(EventKind::Diagnostic, BinderDevice::UNKNOWN)
    }
}
