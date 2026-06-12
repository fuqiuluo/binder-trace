#![no_std]

use core::convert::TryFrom;

pub const MAX_INLINE_PAYLOAD: usize = 256;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct UnknownEventKind {
    raw: u16,
}

impl UnknownEventKind {
    pub const fn raw(self) -> u16 {
        self.raw
    }
}

#[repr(u16)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum EventKind {
    Diagnostic = 0,
    IoctlEnter = 1,
    IoctlExit = 2,
    Transaction = 3,
    Reply = 4,
}

impl EventKind {
    pub const fn as_raw(self) -> u16 {
        self as u16
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Diagnostic => "diagnostic",
            Self::IoctlEnter => "ioctl_enter",
            Self::IoctlExit => "ioctl_exit",
            Self::Transaction => "transaction",
            Self::Reply => "reply",
        }
    }
}

impl TryFrom<u16> for EventKind {
    type Error = UnknownEventKind;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Diagnostic),
            1 => Ok(Self::IoctlEnter),
            2 => Ok(Self::IoctlExit),
            3 => Ok(Self::Transaction),
            4 => Ok(Self::Reply),
            raw => Err(UnknownEventKind { raw }),
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct BinderDevice(u16);

impl BinderDevice {
    pub const BINDER: Self = Self(0);
    pub const HWBINDER: Self = Self(1);
    pub const VNDBINDER: Self = Self(2);
    pub const BINDERFS: Self = Self(3);
    pub const UNKNOWN: Self = Self(u16::MAX);

    pub const fn as_raw(self) -> u16 {
        self.0
    }

    pub const fn name(self) -> &'static str {
        match self.0 {
            0 => "binder",
            1 => "hwbinder",
            2 => "vndbinder",
            3 => "binderfs",
            u16::MAX => "unknown",
            _ => "custom",
        }
    }
}

impl From<u16> for BinderDevice {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RawEventHeader {
    pub kind: u16,
    pub device: u16,
    pub pid: u32,
    pub tid: u32,
    pub uid: u32,
    pub flags: u32,
    pub timestamp_ns: u64,
    pub sequence: u64,
}

impl RawEventHeader {
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

#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RawTransaction {
    pub code: u32,
    pub flags: u32,
    pub data_size: u64,
    pub offsets_size: u64,
    pub target_handle: u32,
    pub sender_pid: u32,
    pub sender_euid: u32,
    pub payload_len: u32,
    pub payload_truncated: u8,
    pub reserved: [u8; 7],
    pub payload: [u8; MAX_INLINE_PAYLOAD],
}

impl RawTransaction {
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

#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RawBinderEvent {
    pub header: RawEventHeader,
    pub transaction: RawTransaction,
}

impl RawBinderEvent {
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
