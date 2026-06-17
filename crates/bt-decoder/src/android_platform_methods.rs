//! Android 平台 Binder 方法表加载器。
//!
//! 表数据源文件放在 `crates/bt-decoder/data/android_platform_methods.tsv`，编译时压缩
//! 进二进制。运行时首次查询会把内置 TSV 释放到磁盘；如果目标文件已经存在，则直接
//! 读取该文件，方便用户用自定义表覆盖内置数据。
//!
//! 数据由本地 AOSP release 分支 AIDL 流式生成，并保留少量非 AIDL Binder 接口。
//! 厂商接口、应用接口和未收录接口返回空方法名。
//!
//! SDK mask 从 Android 11/API 30 开始：bit0=30，bit1=31，依次到 bit6=36。
//! 生成范围：
//! - frameworks/base: android11-release 到 android16-release，排除 tests/tools/aidl_api。
//! - frameworks/libs/net: android14-release common/netd/binder，用于 SDK 34 的 netd 接口。
//! - packages/modules/Connectivity: android14-release framework/framework-t，用于 SDK 34 网络服务接口。
//! - 少量非 AIDL 接口手工维护：IContentProvider、IBulkCursor、IServiceManager。
//!
//! 维护时对照 AOSP 源码：
//! - https://android.googlesource.com/platform/frameworks/base/+/refs/heads/android14-release/
//! - https://android.googlesource.com/platform/frameworks/libs/net/+/refs/heads/android14-release/common/netd/binder/
//! - https://android.googlesource.com/platform/packages/modules/Connectivity/+/refs/heads/android14-release/

use std::env;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::OnceLock;

use csv::{ReaderBuilder, StringRecord};
use flate2::read::GzDecoder;

use super::PlatformMethodEntry;

pub const ANDROID_PLATFORM_METHODS_TSV_ENV: &str = "BINDER_TRACE_ANDROID_PLATFORM_METHODS_TSV";

#[cfg(not(target_os = "android"))]
const ANDROID_PLATFORM_METHODS_HASH: &str = env!("BT_DECODER_ANDROID_PLATFORM_METHODS_HASH");
const ANDROID_PLATFORM_METHODS_TSV_GZ: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/android_platform_methods.tsv.gz"));

static ANDROID_PLATFORM_METHODS: OnceLock<Vec<PlatformMethodEntry>> = OnceLock::new();
static ANDROID_PLATFORM_METHODS_TSV_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 平台方法表 TSV 路径配置失败原因。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AndroidPlatformMethodsPathError {
    /// 路径已经被显式配置过。
    AlreadyConfigured,
    /// 方法表已经加载进内存，后续配置不会再生效。
    AlreadyLoaded,
}

impl fmt::Display for AndroidPlatformMethodsPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyConfigured => write!(f, "Android 平台方法表路径已经配置"),
            Self::AlreadyLoaded => write!(f, "Android 平台方法表已经加载，不能再修改路径"),
        }
    }
}

impl std::error::Error for AndroidPlatformMethodsPathError {}

/// 配置平台方法表 TSV 的释放/读取路径。
///
/// 必须在首次查询 `AndroidPlatformMethods` 前调用。路径存在时直接读取该文件；路径
/// 不存在时会把内置压缩 TSV 释放到这个路径后再读取。
pub fn set_android_platform_methods_tsv_path(
    path: impl Into<PathBuf>,
) -> Result<(), AndroidPlatformMethodsPathError> {
    if ANDROID_PLATFORM_METHODS.get().is_some() {
        return Err(AndroidPlatformMethodsPathError::AlreadyLoaded);
    }

    ANDROID_PLATFORM_METHODS_TSV_PATH
        .set(path.into())
        .map_err(|_| AndroidPlatformMethodsPathError::AlreadyConfigured)
}

pub(super) fn android_platform_methods() -> &'static [PlatformMethodEntry] {
    ANDROID_PLATFORM_METHODS
        .get_or_init(load_android_platform_methods)
        .as_slice()
}

fn load_android_platform_methods() -> Vec<PlatformMethodEntry> {
    let path = android_platform_methods_tsv_path();
    ensure_android_platform_methods_tsv(&path).unwrap_or_else(|error| {
        panic!(
            "释放 Android 平台 Binder 方法表到 {} 失败: {error}",
            path.display()
        )
    });
    let tsv = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "读取 Android 平台 Binder 方法表 {} 失败: {error}",
            path.display()
        )
    });

    let mut reader = ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(false)
        .comment(Some(b'#'))
        .flexible(false)
        .from_reader(tsv.as_bytes());
    let mut entries = Vec::new();

    for (index, record) in reader.records().enumerate() {
        let record = record.unwrap_or_else(|error| {
            panic!(
                "Android 平台 Binder 方法表第 {} 条记录无效: {error}",
                index + 1
            )
        });
        let entry = parse_entry(&record).unwrap_or_else(|error| {
            panic!(
                "Android 平台 Binder 方法表第 {} 条记录无效: {error}",
                index + 1
            )
        });
        entries.push(entry);
    }

    entries
}

fn android_platform_methods_tsv_path() -> PathBuf {
    if let Some(path) = ANDROID_PLATFORM_METHODS_TSV_PATH.get() {
        return path.clone();
    }

    if let Some(path) = non_empty_env_path(ANDROID_PLATFORM_METHODS_TSV_ENV) {
        return path;
    }

    default_android_platform_methods_tsv_path()
}

fn default_android_platform_methods_tsv_path() -> PathBuf {
    #[cfg(target_os = "android")]
    {
        PathBuf::from("/data/local/tmp")
            .join("binder-trace")
            .join("android_platform_methods.tsv")
    }

    #[cfg(not(target_os = "android"))]
    {
        if let Some(path) = non_empty_env_path("XDG_CACHE_HOME") {
            return path
                .join("binder-trace")
                .join(android_platform_methods_tsv_file_name());
        }
        if let Some(path) = non_empty_env_path("HOME") {
            return path
                .join(".cache")
                .join("binder-trace")
                .join(android_platform_methods_tsv_file_name());
        }

        env::temp_dir()
            .join("binder-trace")
            .join(android_platform_methods_tsv_file_name())
    }
}

#[cfg(not(target_os = "android"))]
fn android_platform_methods_tsv_file_name() -> String {
    format!("android_platform_methods-{ANDROID_PLATFORM_METHODS_HASH}.tsv")
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.as_os_str().is_empty())
        .map(PathBuf::from)
}

fn ensure_android_platform_methods_tsv(path: &Path) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let temp_path = extraction_temp_path(path);
    let result =
        write_embedded_tsv_to_temp(&temp_path).and_then(|()| publish_temp_tsv(&temp_path, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn write_embedded_tsv_to_temp(path: &Path) -> io::Result<()> {
    let _ = fs::remove_file(path);
    let mut decoder = GzDecoder::new(ANDROID_PLATFORM_METHODS_TSV_GZ);
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    io::copy(&mut decoder, &mut file)?;
    file.flush()
}

fn publish_temp_tsv(temp_path: &Path, path: &Path) -> io::Result<()> {
    match fs::hard_link(temp_path, path) {
        Ok(()) => {
            let _ = fs::remove_file(temp_path);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(temp_path);
            Ok(())
        }
        Err(_) if path.exists() => {
            let _ = fs::remove_file(temp_path);
            Ok(())
        }
        Err(_) => fs::rename(temp_path, path),
    }
}

fn extraction_temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "android_platform_methods.tsv".into());
    path.with_file_name(format!(".{file_name}.{}.tmp", process::id()))
}

fn parse_entry(record: &StringRecord) -> Result<PlatformMethodEntry, String> {
    if record.len() != 4 {
        return Err(format!("字段数量应为 4 个，实际为 {}", record.len()));
    }

    let sdk_mask = field(record, 0, "sdk_mask")?;
    let code = field(record, 1, "code")?;
    let interface = field(record, 2, "interface")?;
    let method = field(record, 3, "method")?;
    let sdk_mask = sdk_mask
        .strip_prefix("0x")
        .ok_or_else(|| format!("sdk_mask 不是十六进制: {sdk_mask}"))
        .and_then(|raw| {
            u16::from_str_radix(raw, 16).map_err(|_| format!("sdk_mask 数值无效: {sdk_mask}"))
        })?;
    let code = code
        .parse::<u32>()
        .map_err(|_| format!("code 数值无效: {code}"))?;

    Ok(PlatformMethodEntry {
        sdk_mask,
        interface: interface.to_owned(),
        code,
        method: method.to_owned(),
    })
}

fn field<'a>(record: &'a StringRecord, index: usize, name: &str) -> Result<&'a str, String> {
    record
        .get(index)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("缺少 {name} 字段"))
}
