//! 共享 raw event 类型编号。

use core::convert::TryFrom;

/// raw event kind 无法映射到已知 [`EventKind`] 时的错误。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct UnknownEventKind {
    raw: u16,
}

impl UnknownEventKind {
    /// 返回原始 kind 数值，用于诊断或兼容输出。
    pub const fn raw(self) -> u16 {
        self.raw
    }
}

/// raw Binder 事件类型。
#[repr(u16)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum EventKind {
    /// agent 自身启动或诊断事件。
    Diagnostic = 0,
    /// ioctl 进入事件。
    IoctlEnter = 1,
    /// ioctl 返回事件。
    IoctlExit = 2,
    /// Binder transaction 请求事件。
    Transaction = 3,
    /// Binder reply 响应事件。
    Reply = 4,
}

impl EventKind {
    /// 返回写入 raw event 的数值表示。
    pub const fn as_raw(self) -> u16 {
        self as u16
    }

    /// 返回用于日志和 JSON 输出的稳定短名称。
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
