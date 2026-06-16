//! 事件信封使用的设备标识解析。
//!
//! # 职责
//! - 按配置、环境变量、Android property 的优先级解析设备标识。
//! - 在非 Android 环境保持可测试：`getprop` 不可用时安静降级到 boot id 或 `unknown`。

use std::process::Command;

use tracing::debug;

const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";
const UNKNOWN_DEVICE_ID: &str = "unknown";

pub(crate) fn resolve_device_id(configured: Option<&str>) -> String {
    if let Some(device_id) = configured.and_then(non_empty_str) {
        debug!(source = "configured", "使用配置中的 device_id");
        return device_id.to_owned();
    }

    if let Some(device_id) = device_id_from_env() {
        debug!(source = "env", "使用环境变量中的 device_id");
        return device_id;
    }

    if let Some(device_id) = android_property("ro.serialno") {
        debug!(
            source = "ro.serialno",
            "使用 Android property 中的 device_id"
        );
        return device_id;
    }

    if let Some(device_id) = android_property("ro.boot.serialno") {
        debug!(
            source = "ro.boot.serialno",
            "使用 Android boot property 中的 device_id"
        );
        return device_id;
    }

    if let Some(device_id) = boot_id() {
        debug!(
            source = "boot_id",
            "使用 boot_id 作为本次启动内的 device_id"
        );
        return device_id;
    }

    debug!(source = "unknown", "无法解析 device_id，使用 unknown");
    UNKNOWN_DEVICE_ID.to_owned()
}

fn device_id_from_env() -> Option<String> {
    std::env::var("BINDER_TRACE_DEVICE_ID")
        .ok()
        .and_then(non_empty_string)
}

fn android_property(name: &str) -> Option<String> {
    // `getprop` 只在 Android 设备上可靠存在；host 测试环境失败时按缺失处理。
    let output = Command::new("getprop").arg(name).output().ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout)
        .ok()
        .and_then(non_empty_string)
}

fn boot_id() -> Option<String> {
    // boot_id 只能保证本次启动内稳定，不能替代跨重启稳定的设备序列号。
    std::fs::read_to_string(BOOT_ID_PATH)
        .ok()
        .and_then(non_empty_string)
}

fn non_empty_string(value: String) -> Option<String> {
    non_empty_str(&value).map(str::to_owned)
}

fn non_empty_str(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}
