//! 自定义 socket 协议族控制客户端。
//!
//! # 职责
//! - 探测内核模块动态注册的控制协议族。
//! - 通过 socket ioctl 设置捕获开关、基础过滤和读取统计。
//! - 通过 recvmsg/poll 读取内核模块推送的 Binder 事件流。
//!
//! # 不变量
//! - 事件流只承载固定布局的 UAPI 结构，不在用户态猜测内核私有结构布局。
//! - 协议族编号由内核模块动态分配，用户态不能写死 family。

use std::ffi::c_void;
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr;
use std::time::Duration;

use bt_common::MAX_INLINE_PAYLOAD;
use thiserror::Error;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

const BT_ABI_VERSION: u32 = 2;
const BT_IOC_MAGIC: u32 = b'B' as u32;
const BT_DRIVER_FEATURE_MAGIC: u64 = 0x4254_5241_4345_3031;
const BT_FEATURE_EVENT_STREAM: u32 = 1 << 1;
const BT_EVENT_KIND_BINDER_TRANSACTION: u32 = 1;
const BT_FIRST_FAMILY: libc::c_int = libc::AF_DECnet;
const BT_LAST_FAMILY: libc::c_int = 46;

const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;

const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;

const IOC_NONE: u32 = 0;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const BT_CAPTURE_POINT_IOCTL: u32 = 1 << 0;
const BT_CAPTURE_POINT_COPY_TO_USER: u32 = 1 << 1;
const BT_CAPTURE_POINT_TRANSACTION: u32 = 1 << 2;
const BT_CAPTURE_POINT_ALL: u32 =
    BT_CAPTURE_POINT_IOCTL | BT_CAPTURE_POINT_COPY_TO_USER | BT_CAPTURE_POINT_TRANSACTION;

const fn ioc(dir: u32, ty: u32, nr: u32, size: u32) -> u32 {
    (dir << IOC_DIRSHIFT) | (ty << IOC_TYPESHIFT) | (nr << IOC_NRSHIFT) | (size << IOC_SIZESHIFT)
}

const fn io(nr: u32) -> u32 {
    ioc(IOC_NONE, BT_IOC_MAGIC, nr, 0)
}

const fn ior<T>(nr: u32) -> u32 {
    ioc(IOC_READ, BT_IOC_MAGIC, nr, size_of::<T>() as u32)
}

const fn iow<T>(nr: u32) -> u32 {
    ioc(IOC_WRITE, BT_IOC_MAGIC, nr, size_of::<T>() as u32)
}

const BT_IOC_GET_ABI_VERSION: u32 = ior::<AbiVersion>(0x00);
const BT_IOC_SET_CONFIG: u32 = iow::<CaptureConfig>(0x01);
const BT_IOC_GET_CONFIG: u32 = ior::<CaptureConfig>(0x02);
const BT_IOC_GET_STATS: u32 = ior::<CaptureStats>(0x03);
const BT_IOC_CLEAR_STATS: u32 = io(0x04);
const BT_IOC_GET_FEATURE: u32 = ior::<DriverFeature>(0x05);

/// socket IPC 连接或请求失败。
#[derive(Debug, Error)]
pub enum SocketIpcError {
    /// 未找到 binder-trace 动态协议族。
    #[error("未找到 binder-trace 控制协议族")]
    NotFound,
    /// socket 或 ioctl 系统调用失败。
    #[error("socket IPC 系统调用失败: {0}")]
    Io(#[from] io::Error),
    /// 内核模块和用户态 ABI 版本不一致。
    #[error("socket IPC ABI 不匹配: 期望 {expected}, 实际 {actual}")]
    AbiMismatch { expected: u32, actual: u32 },
    /// recvmsg 返回了非完整事件。
    #[error("socket IPC 事件读取长度错误: 期望 {expected}, 实际 {actual}")]
    ShortRead { expected: usize, actual: usize },
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct AbiVersion {
    version: u32,
    reserved: u32,
}

/// 内核驱动特征，布局必须和 `struct bt_driver_feature` 保持一致。
#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DriverFeature {
    pub magic: u64,
    pub abi_version: u32,
    pub feature_flags: u32,
    pub name: [u8; 16],
}

impl DriverFeature {
    /// 判断当前协议族是否确认为 binder-trace 驱动。
    pub const fn is_binder_trace(&self) -> bool {
        self.magic == BT_DRIVER_FEATURE_MAGIC
    }

    /// 判断当前驱动是否支持 socket 事件流。
    pub const fn has_event_stream(&self) -> bool {
        (self.feature_flags & BT_FEATURE_EVENT_STREAM) != 0
    }
}

/// 内核捕获配置，布局必须和 `kernel/src/ipc/bt_ipc_uapi.h` 保持一致。
#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CaptureConfig {
    pub enabled: u32,
    pub point_mask: u32,
    pub tgid: i32,
    pub pid: i32,
    pub uid: u32,
    pub uid_enabled: u32,
    pub ioctl_cmd: u32,
    pub ioctl_cmd_enabled: u32,
    pub min_size: u64,
    pub max_size: u64,
}

impl CaptureConfig {
    /// 默认开启所有 hook 点，不设置进程、uid、cmd 或大小过滤。
    pub const fn enabled() -> Self {
        Self {
            enabled: 1,
            point_mask: BT_CAPTURE_POINT_ALL,
            tgid: 0,
            pid: 0,
            uid: 0,
            uid_enabled: 0,
            ioctl_cmd: 0,
            ioctl_cmd_enabled: 0,
            min_size: 0,
            max_size: 0,
        }
    }

    /// 只开启 Binder transaction hook，用于跟踪发送/回复路径。
    pub const fn binder_transaction_enabled() -> Self {
        Self {
            point_mask: BT_CAPTURE_POINT_TRANSACTION,
            ..Self::enabled()
        }
    }

    /// 关闭捕获，保留默认 hook 点掩码。
    pub const fn disabled() -> Self {
        Self {
            enabled: 0,
            ..Self::enabled()
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

/// 内核控制面统计。
#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CaptureStats {
    pub ioctl_hits: u64,
    pub copy_to_user_hits: u64,
    pub transaction_hits: u64,
    pub captured: u64,
    pub filtered: u64,
}

/// 内核通过 recvmsg 推送的 Binder 事件。
#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, FromBytes, Immutable, IntoBytes, KnownLayout)]
pub struct BinderEvent {
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub kind: u32,
    pub pid: u32,
    pub tgid: u32,
    pub uid: u32,
    pub reply: u32,
    pub lost_before: u32,
    pub transaction: u64,
    pub proc: u64,
    pub thread: u64,
    pub extra_buffers_size: u64,
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

impl BinderEvent {
    pub const fn is_binder_transaction(&self) -> bool {
        self.kind == BT_EVENT_KIND_BINDER_TRANSACTION
    }

    pub const fn is_reply(&self) -> bool {
        self.reply != 0
    }

    pub fn payload_bytes(&self) -> &[u8] {
        let payload_len = (self.payload_len as usize).min(MAX_INLINE_PAYLOAD);
        &self.payload[..payload_len]
    }
}

/// 自定义协议族控制客户端。
#[derive(Debug)]
pub struct SocketIpcClient {
    fd: OwnedFd,
    family: libc::c_int,
}

impl SocketIpcClient {
    /// 扫描并连接内核模块动态注册的协议族。
    pub fn connect() -> Result<Self, SocketIpcError> {
        for family in BT_FIRST_FAMILY..BT_LAST_FAMILY {
            if !Self::family_looks_like_binder_trace(family) {
                continue;
            }

            let Some(fd) = Self::open_raw_socket(family)? else {
                continue;
            };

            let client = Self { fd, family };
            match client.get_feature() {
                Ok(feature) if !feature.is_binder_trace() => continue,
                Ok(feature) if feature.abi_version == BT_ABI_VERSION => return Ok(client),
                Ok(feature) => {
                    return Err(SocketIpcError::AbiMismatch {
                        expected: BT_ABI_VERSION,
                        actual: feature.abi_version,
                    });
                }
                Err(SocketIpcError::Io(error)) if error.raw_os_error() == Some(libc::ENOTTY) => {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }

        Err(SocketIpcError::NotFound)
    }

    pub const fn family(&self) -> libc::c_int {
        self.family
    }

    /// 返回底层控制 socket fd，供外部事件循环等待可读状态。
    pub fn raw_fd(&self) -> std::os::fd::RawFd {
        self.fd.as_raw_fd()
    }

    pub fn get_feature(&self) -> Result<DriverFeature, SocketIpcError> {
        let mut feature = DriverFeature {
            magic: 0,
            abi_version: 0,
            feature_flags: 0,
            name: [0; 16],
        };
        self.ioctl(
            BT_IOC_GET_FEATURE,
            ptr::from_mut(&mut feature).cast::<c_void>(),
        )?;
        Ok(feature)
    }

    pub fn get_abi_version(&self) -> Result<u32, SocketIpcError> {
        let mut version = AbiVersion {
            version: 0,
            reserved: 0,
        };
        self.ioctl(
            BT_IOC_GET_ABI_VERSION,
            ptr::from_mut(&mut version).cast::<c_void>(),
        )?;
        Ok(version.version)
    }

    pub fn set_config(&self, mut config: CaptureConfig) -> Result<(), SocketIpcError> {
        self.ioctl(
            BT_IOC_SET_CONFIG,
            ptr::from_mut(&mut config).cast::<c_void>(),
        )
    }

    pub fn get_config(&self) -> Result<CaptureConfig, SocketIpcError> {
        let mut config = CaptureConfig::default();
        self.ioctl(
            BT_IOC_GET_CONFIG,
            ptr::from_mut(&mut config).cast::<c_void>(),
        )?;
        Ok(config)
    }

    pub fn get_stats(&self) -> Result<CaptureStats, SocketIpcError> {
        let mut stats = CaptureStats {
            ioctl_hits: 0,
            copy_to_user_hits: 0,
            transaction_hits: 0,
            captured: 0,
            filtered: 0,
        };
        self.ioctl(BT_IOC_GET_STATS, ptr::from_mut(&mut stats).cast::<c_void>())?;
        Ok(stats)
    }

    pub fn clear_stats(&self) -> Result<(), SocketIpcError> {
        self.ioctl(BT_IOC_CLEAR_STATS, ptr::null_mut())
    }

    pub fn poll_event(&self, timeout: Duration) -> Result<bool, SocketIpcError> {
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        let mut pollfd = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };

        loop {
            // SAFETY: `pollfd` 指向栈上有效内存，fd 由 `OwnedFd` 持有且在调用期间有效。
            let ret = unsafe { libc::poll(ptr::from_mut(&mut pollfd), 1, timeout_ms) };
            if ret > 0 {
                return Ok((pollfd.revents & libc::POLLIN) != 0);
            }
            if ret == 0 {
                return Ok(false);
            }

            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINTR) {
                return Err(SocketIpcError::Io(error));
            }
        }
    }

    pub fn try_recv_event(&self) -> Result<Option<BinderEvent>, SocketIpcError> {
        self.recv_event_with_flags(libc::MSG_DONTWAIT)
    }

    /// 尝试把下一条事件直接读入调用方提供的事件槽位。
    ///
    /// 这个接口用于 TUI 的 mmap 历史文件：`recv` 的目标缓冲区可以直接是文件映射中的
    /// 一个 `BinderEvent` 槽位，避免先读到临时对象再复制到磁盘缓存。
    pub fn try_recv_event_into(&self, event: &mut BinderEvent) -> Result<bool, SocketIpcError> {
        self.recv_event_into_with_flags(event, libc::MSG_DONTWAIT)
    }

    fn family_looks_like_binder_trace(family: libc::c_int) -> bool {
        // SAFETY: `socket` 不持有 Rust 引用；参数是按 libc ABI 传递的整数，返回值立即检查。
        let fd = unsafe { libc::socket(family, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd >= 0 {
            // SAFETY: `fd` 来自刚成功返回的 `socket`，尚未转移给其他 owner。
            let owned = unsafe { OwnedFd::from_raw_fd(fd) };
            drop(owned);
            return false;
        }

        io::Error::last_os_error().raw_os_error() == Some(libc::ENOKEY)
    }

    fn open_raw_socket(family: libc::c_int) -> Result<Option<OwnedFd>, SocketIpcError> {
        // SAFETY: `socket` 不持有 Rust 引用；参数是按 libc ABI 传递的整数，返回值立即检查。
        let fd = unsafe { libc::socket(family, libc::SOCK_RAW | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EAFNOSUPPORT) {
                return Ok(None);
            }
            return Err(SocketIpcError::Io(error));
        }

        // SAFETY: `fd` 来自刚成功返回的 `socket`，尚未转移给其他 owner。
        Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    fn ioctl(&self, request: u32, arg: *mut c_void) -> Result<(), SocketIpcError> {
        // Android bionic 和 glibc 对可变参数 ioctl 的 request 类型声明不同，这里只保持编号按位不变。
        #[cfg(target_os = "android")]
        let request = request as libc::c_int;
        #[cfg(not(target_os = "android"))]
        let request = request as libc::c_ulong;

        // SAFETY: fd 由 `OwnedFd` 持有且有效；`arg` 指向调用方在 ioctl 返回前保持存活的 UAPI 结构。
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), request, arg) };
        if ret < 0 {
            return Err(SocketIpcError::Io(io::Error::last_os_error()));
        }

        Ok(())
    }

    fn recv_event_with_flags(
        &self,
        flags: libc::c_int,
    ) -> Result<Option<BinderEvent>, SocketIpcError> {
        let mut event = MaybeUninit::<BinderEvent>::uninit();
        // SAFETY: 目标缓冲区足够容纳一个 `BinderEvent`，fd 有效，flags 由调用方固定传入。
        let ret = unsafe {
            libc::recv(
                self.fd.as_raw_fd(),
                event.as_mut_ptr().cast::<c_void>(),
                size_of::<BinderEvent>(),
                flags,
            )
        };
        if ret < 0 {
            let error = io::Error::last_os_error();
            let raw_error = error.raw_os_error();
            if raw_error == Some(libc::EAGAIN) || raw_error == Some(libc::EWOULDBLOCK) {
                return Ok(None);
            }
            return Err(SocketIpcError::Io(error));
        }

        let actual = ret as usize;
        if actual != size_of::<BinderEvent>() {
            return Err(SocketIpcError::ShortRead {
                expected: size_of::<BinderEvent>(),
                actual,
            });
        }

        // SAFETY: 内核已经完整写入 `BinderEvent` 大小的数据。
        Ok(Some(unsafe { event.assume_init() }))
    }

    fn recv_event_into_with_flags(
        &self,
        event: &mut BinderEvent,
        flags: libc::c_int,
    ) -> Result<bool, SocketIpcError> {
        let bytes = event.as_mut_bytes();
        // SAFETY: `bytes` 是调用方提供的完整 `BinderEvent` 可写字节视图，fd 有效。
        let ret = unsafe {
            libc::recv(
                self.fd.as_raw_fd(),
                bytes.as_mut_ptr().cast::<c_void>(),
                bytes.len(),
                flags,
            )
        };
        if ret < 0 {
            let error = io::Error::last_os_error();
            let raw_error = error.raw_os_error();
            if raw_error == Some(libc::EAGAIN) || raw_error == Some(libc::EWOULDBLOCK) {
                return Ok(false);
            }
            return Err(SocketIpcError::Io(error));
        }

        let actual = ret as usize;
        if actual != bytes.len() {
            return Err(SocketIpcError::ShortRead {
                expected: bytes.len(),
                actual,
            });
        }

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{AbiVersion, BinderEvent, CaptureConfig, CaptureStats, DriverFeature};

    #[test]
    fn uapi_layout_sizes_are_stable() {
        assert_eq!(std::mem::size_of::<AbiVersion>(), 8);
        assert_eq!(std::mem::size_of::<DriverFeature>(), 32);
        assert_eq!(std::mem::size_of::<CaptureConfig>(), 48);
        assert_eq!(std::mem::size_of::<CaptureStats>(), 40);
        assert_eq!(std::mem::size_of::<BinderEvent>(), 376);
    }

    #[test]
    fn default_capture_config_is_disabled() {
        let config = CaptureConfig::default();

        assert_eq!(config.enabled, 0);
        assert_ne!(config.point_mask, 0);
    }
}
