//! 基于内核 `.config` 判断当前设备可用的采集能力。
//!
//! # References
//! - Android common kernel `android-mainline`，`kernel/bpf/Kconfig`:
//!   <https://android.googlesource.com/kernel/common/+/refs/heads/android-mainline/kernel/bpf/Kconfig>
//! - Android common kernel `android-mainline`，`kernel/trace/Kconfig`:
//!   <https://android.googlesource.com/kernel/common/+/refs/heads/android-mainline/kernel/trace/Kconfig>
//! - Android 设备常见入口：`/proc/config.gz`

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::process::Command;
use std::string::FromUtf8Error;

use bitflags::bitflags;

bitflags! {
    /// Agent 启动前需要验证的采集 source 集合。
    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    pub struct CaptureMode: u64 {
        /// 不启用任何采集 source。
        const EMPTY = 0;
        /// 通过 `BPF_PROG_TYPE_KPROBE` 挂内核函数。
        const EBPF_KPROBE = 1 << 0;
        /// 通过 eBPF tracepoint 读取内核 tracepoint 事件。
        const EBPF_TRACEPOINT = 1 << 1;
        // 暂不开放 `EBPF_UPROBE` capture source。
        // 原因：uprobe 会进入目标用户态进程的地址空间和符号解析边界，
        // 容易扩大隐私与稳定性风险；当前阶段只验证 Binder 内核路径采集。
        // const EBPF_UPROBE = 1 << 2;
        /// 通过 tracefs `kprobe_events` 做非 eBPF kprobe 实验。
        const TRACEFS_KPROBE = 1 << 3;
    }
}

const CAPTURE_MODE_NAMES: &[(CaptureMode, &str)] = &[
    (CaptureMode::EBPF_KPROBE, "ebpf-kprobe"),
    (CaptureMode::EBPF_TRACEPOINT, "ebpf-tracepoint"),
    (CaptureMode::TRACEFS_KPROBE, "tracefs-kprobe"),
];

impl CaptureMode {
    /// 返回启用的 source 名称。
    pub fn names(self) -> Vec<&'static str> {
        CAPTURE_MODE_NAMES
            .iter()
            .filter_map(|(mode, name)| self.contains(*mode).then_some(*name))
            .collect()
    }

    fn required_capabilities(self) -> Vec<KernelCapability> {
        CAPTURE_MODE_NAMES
            .iter()
            .map(|(mode, _)| *mode)
            .filter(|mode| self.contains(*mode))
            .flat_map(Self::required_capabilities_for_one)
            .copied()
            .fold(Vec::new(), |mut capabilities, capability| {
                if !capabilities.contains(&capability) {
                    capabilities.push(capability);
                }
                capabilities
            })
    }

    fn required_capabilities_for_one(mode: Self) -> &'static [KernelCapability] {
        if mode == Self::EBPF_KPROBE {
            &[
                KernelCapability::Bpf,
                KernelCapability::BpfSyscall,
                KernelCapability::BpfEvents,
                KernelCapability::Kprobes,
            ]
        } else if mode == Self::EBPF_TRACEPOINT {
            &[
                KernelCapability::Bpf,
                KernelCapability::BpfSyscall,
                KernelCapability::BpfEvents,
            ]
        } else if mode == Self::TRACEFS_KPROBE {
            &[
                KernelCapability::Ftrace,
                KernelCapability::Kprobes,
                KernelCapability::KprobeEvents,
            ]
        } else {
            &[]
        }
    }
}

impl Default for CaptureMode {
    fn default() -> Self {
        Self::EBPF_KPROBE
    }
}

impl fmt::Display for CaptureMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = self.names();
        if names.is_empty() {
            f.write_str("none")
        } else {
            f.write_str(&names.join(", "))
        }
    }
}

/// 读取内核配置的来源。
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum KernelConfigSource {
    /// 使用 `zcat /proc/config.gz` 读取当前设备的内核配置。
    ProcConfigGz,
    /// 使用 `zcat <path>` 读取指定的 gzip 内核配置，主要用于设备实验和测试夹具。
    GzipFile(PathBuf),
    /// 直接读取纯文本内核配置，便于本地构造测试输入。
    TextFile(PathBuf),
}

impl KernelConfigSource {
    /// 读取并解压内核配置文本。
    ///
    /// # Errors
    /// 当 `zcat` 执行失败、文件不可读或输出不是 UTF-8 时返回错误。
    pub fn read_to_string(&self) -> Result<String, KernelConfigError> {
        match self {
            Self::ProcConfigGz => read_gzip_config("/proc/config.gz"),
            Self::GzipFile(path) => read_gzip_config(path),
            Self::TextFile(path) => std::fs::read_to_string(path).map_err(KernelConfigError::Io),
        }
    }
}

impl Default for KernelConfigSource {
    fn default() -> Self {
        Self::ProcConfigGz
    }
}

/// 从内核配置解析出的能力集合。
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct KernelCapabilities {
    bits: u64,
}

impl KernelCapabilities {
    /// 从内核配置文本构造能力集合。
    pub fn from_config(config: &KernelConfig) -> Self {
        KernelCapability::ALL
            .iter()
            .copied()
            .filter(|capability| config.is_enabled(capability.config_key()))
            .fold(Self::default(), |capabilities, capability| {
                capabilities.with(capability)
            })
    }

    /// 返回能力集合是否包含指定能力。
    pub const fn contains(self, capability: KernelCapability) -> bool {
        self.bits & capability.bit() != 0
    }

    /// 返回是否包含任意 eBPF 基础能力。
    pub const fn has_any_ebpf(self) -> bool {
        self.contains(KernelCapability::Bpf)
            || self.contains(KernelCapability::BpfSyscall)
            || self.contains(KernelCapability::BpfEvents)
    }

    /// 返回当前集合是否满足指定采集模式。
    pub fn supports(self, mode: CaptureMode) -> bool {
        !mode.is_empty() && self.missing_for(mode).is_empty()
    }

    /// 返回指定采集模式缺失的能力。
    pub fn missing_for(self, mode: CaptureMode) -> Vec<KernelCapability> {
        mode.required_capabilities()
            .into_iter()
            .filter(|capability| !self.contains(*capability))
            .collect()
    }

    /// 返回能力名称列表，用于诊断输出。
    pub fn names(self) -> Vec<&'static str> {
        KernelCapability::ALL
            .iter()
            .copied()
            .filter(|capability| self.contains(*capability))
            .map(KernelCapability::name)
            .collect()
    }

    const fn with(self, capability: KernelCapability) -> Self {
        Self {
            bits: self.bits | capability.bit(),
        }
    }
}

/// 本项目关心的内核能力位。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum KernelCapability {
    Bpf,
    BpfSyscall,
    BpfEvents,
    Kprobes,
    KprobeEvents,
    /// 仅记录设备事实，不代表当前允许启用 uprobe 采集 source。
    Uprobes,
    /// 仅记录设备事实，不代表当前允许启用 uprobe 采集 source。
    UprobeEvents,
    Ftrace,
    FunctionTracer,
    FtraceSyscalls,
    DebugInfoBtf,
    Kallsyms,
    KallsymsAll,
}

impl KernelCapability {
    const ALL: &'static [Self] = &[
        Self::Bpf,
        Self::BpfSyscall,
        Self::BpfEvents,
        Self::Kprobes,
        Self::KprobeEvents,
        Self::Uprobes,
        Self::UprobeEvents,
        Self::Ftrace,
        Self::FunctionTracer,
        Self::FtraceSyscalls,
        Self::DebugInfoBtf,
        Self::Kallsyms,
        Self::KallsymsAll,
    ];

    /// 返回对应的 Kconfig key。
    pub const fn config_key(self) -> &'static str {
        match self {
            Self::Bpf => "CONFIG_BPF",
            Self::BpfSyscall => "CONFIG_BPF_SYSCALL",
            Self::BpfEvents => "CONFIG_BPF_EVENTS",
            Self::Kprobes => "CONFIG_KPROBES",
            Self::KprobeEvents => "CONFIG_KPROBE_EVENTS",
            Self::Uprobes => "CONFIG_UPROBES",
            Self::UprobeEvents => "CONFIG_UPROBE_EVENTS",
            Self::Ftrace => "CONFIG_FTRACE",
            Self::FunctionTracer => "CONFIG_FUNCTION_TRACER",
            Self::FtraceSyscalls => "CONFIG_FTRACE_SYSCALLS",
            Self::DebugInfoBtf => "CONFIG_DEBUG_INFO_BTF",
            Self::Kallsyms => "CONFIG_KALLSYMS",
            Self::KallsymsAll => "CONFIG_KALLSYMS_ALL",
        }
    }

    /// 返回诊断输出使用的短名称。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bpf => "bpf",
            Self::BpfSyscall => "bpf_syscall",
            Self::BpfEvents => "bpf_events",
            Self::Kprobes => "kprobes",
            Self::KprobeEvents => "kprobe_events",
            Self::Uprobes => "uprobes",
            Self::UprobeEvents => "uprobe_events",
            Self::Ftrace => "ftrace",
            Self::FunctionTracer => "function_tracer",
            Self::FtraceSyscalls => "ftrace_syscalls",
            Self::DebugInfoBtf => "debug_info_btf",
            Self::Kallsyms => "kallsyms",
            Self::KallsymsAll => "kallsyms_all",
        }
    }

    const fn bit(self) -> u64 {
        1 << (self as u8)
    }
}

impl fmt::Display for KernelCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// 解析后的内核配置。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct KernelConfig {
    enabled_keys: BTreeSet<String>,
}

impl KernelConfig {
    /// 从 `.config` 文本解析已启用的 Kconfig key。
    pub fn parse(text: &str) -> Self {
        let enabled_keys = text
            .lines()
            .filter_map(parse_enabled_key)
            .map(str::to_owned)
            .collect();

        Self { enabled_keys }
    }

    /// 判断指定 Kconfig key 是否启用。
    pub fn is_enabled(&self, key: &str) -> bool {
        self.enabled_keys.contains(key)
    }
}

/// 内核能力探测失败的原因。
#[derive(Debug)]
pub enum KernelConfigError {
    Io(std::io::Error),
    ZcatFailed { path: PathBuf, stderr: String },
    Utf8(FromUtf8Error),
}

impl fmt::Display for KernelConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "failed to read kernel config: {error}"),
            Self::ZcatFailed { path, stderr } => {
                write!(f, "failed to decompress `{}`", path.display())?;
                if stderr.trim().is_empty() {
                    Ok(())
                } else {
                    write!(f, ": {}", stderr.trim())
                }
            }
            Self::Utf8(error) => write!(f, "kernel config is not valid UTF-8: {error}"),
        }
    }
}

impl std::error::Error for KernelConfigError {}

impl From<FromUtf8Error> for KernelConfigError {
    fn from(error: FromUtf8Error) -> Self {
        Self::Utf8(error)
    }
}

fn read_gzip_config(path: impl Into<PathBuf>) -> Result<String, KernelConfigError> {
    let path = path.into();
    let output = Command::new("zcat")
        .arg(&path)
        .output()
        .map_err(KernelConfigError::Io)?;

    if !output.status.success() {
        return Err(KernelConfigError::ZcatFailed {
            path,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    String::from_utf8(output.stdout).map_err(KernelConfigError::Utf8)
}

fn parse_enabled_key(line: &str) -> Option<&str> {
    let (key, value) = line.split_once('=')?;

    match value {
        "y" | "m" => Some(key),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureMode, KernelCapabilities, KernelCapability, KernelConfig, parse_enabled_key,
    };

    #[test]
    fn parses_enabled_config_keys() {
        assert_eq!(parse_enabled_key("CONFIG_BPF=y"), Some("CONFIG_BPF"));
        assert_eq!(
            parse_enabled_key("CONFIG_KPROBES=m"),
            Some("CONFIG_KPROBES")
        );
        assert_eq!(
            parse_enabled_key("# CONFIG_DEBUG_INFO_BTF is not set"),
            None
        );
        assert_eq!(parse_enabled_key("CONFIG_HZ=300"), None);
    }

    #[test]
    fn detects_ebpf_kprobe_capability() {
        let config = KernelConfig::parse(
            "\
CONFIG_BPF=y
CONFIG_BPF_SYSCALL=y
CONFIG_BPF_EVENTS=y
CONFIG_KPROBES=y
CONFIG_KPROBE_EVENTS=y
# CONFIG_DEBUG_INFO_BTF is not set
",
        );

        let capabilities = KernelCapabilities::from_config(&config);

        assert!(capabilities.has_any_ebpf());
        assert!(capabilities.supports(CaptureMode::EBPF_KPROBE));
        assert!(capabilities.contains(KernelCapability::KprobeEvents));
        assert!(!capabilities.contains(KernelCapability::DebugInfoBtf));
    }

    #[test]
    fn reports_missing_kprobe_requirements() {
        let config = KernelConfig::parse(
            "\
CONFIG_BPF=y
CONFIG_BPF_SYSCALL=y
# CONFIG_BPF_EVENTS is not set
# CONFIG_KPROBES is not set
",
        );

        let capabilities = KernelCapabilities::from_config(&config);

        assert_eq!(
            capabilities.missing_for(CaptureMode::EBPF_KPROBE),
            vec![KernelCapability::BpfEvents, KernelCapability::Kprobes]
        );
    }

    #[test]
    fn supports_combined_capture_modes() {
        let config = KernelConfig::parse(
            "\
CONFIG_BPF=y
CONFIG_BPF_SYSCALL=y
CONFIG_BPF_EVENTS=y
CONFIG_KPROBES=y
",
        );
        let capabilities = KernelCapabilities::from_config(&config);
        let mode = CaptureMode::EBPF_KPROBE | CaptureMode::EBPF_TRACEPOINT;

        assert!(mode.contains(CaptureMode::EBPF_KPROBE));
        assert!(mode.contains(CaptureMode::EBPF_TRACEPOINT));
        assert!(capabilities.supports(mode));
    }
}
