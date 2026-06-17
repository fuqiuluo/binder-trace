//! Android 平台 Binder 方法表加载器。
//!
//! 表数据放在 `crates/bt-decoder/data/android_platform_methods.tsv`，避免让 LSP
//! 解析上万行 Rust 常量。运行时首次查询时解析一次，之后复用内存中的排序表。
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

use std::sync::OnceLock;

use csv::{ReaderBuilder, StringRecord};

use super::PlatformMethodEntry;

const ANDROID_PLATFORM_METHODS_TSV: &str = include_str!("../data/android_platform_methods.tsv");

static ANDROID_PLATFORM_METHODS: OnceLock<Vec<PlatformMethodEntry>> = OnceLock::new();

pub(super) fn android_platform_methods() -> &'static [PlatformMethodEntry] {
    ANDROID_PLATFORM_METHODS
        .get_or_init(load_android_platform_methods)
        .as_slice()
}

fn load_android_platform_methods() -> Vec<PlatformMethodEntry> {
    let mut reader = ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(false)
        .comment(Some(b'#'))
        .flexible(false)
        .from_reader(ANDROID_PLATFORM_METHODS_TSV.as_bytes());
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
