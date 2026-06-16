//! Binder 设备编号。

/// Binder 设备编号。
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct BinderDevice(u16);

impl BinderDevice {
    /// `/dev/binder`。
    pub const BINDER: Self = Self(0);
    /// `/dev/hwbinder`。
    pub const HWBINDER: Self = Self(1);
    /// `/dev/vndbinder`。
    pub const VNDBINDER: Self = Self(2);
    /// binderfs 挂载下的 Binder 设备。
    pub const BINDERFS: Self = Self(3);
    /// 未知或暂未解析的设备。
    pub const UNKNOWN: Self = Self(u16::MAX);

    /// 返回 raw 设备编号。
    pub const fn as_raw(self) -> u16 {
        self.0
    }

    /// 返回用于输出的设备名称。
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
