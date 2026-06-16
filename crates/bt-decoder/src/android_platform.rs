//! Android 平台 Binder 语义解析。
//!
//! # 职责
//! - 按 Android SDK 版本把平台自带 Binder interface/code 映射成方法名。
//! - 解析 Binder Parcel 开头的 interface token。
//!
//! # 约束
//! - 只覆盖 AOSP frameworks/base 中的平台接口；厂商和应用自定义接口返回空。
//! - 方法表由脚本生成，源码来自 AOSP release 分支，不在运行时访问网络。

#[path = "android_platform_methods.rs"]
mod android_platform_methods;

const MIN_ANDROID_SDK: u16 = 30;
const MAX_ANDROID_SDK: u16 = 36;

/// Android 平台 Binder 方法信息。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AndroidPlatformMethod {
    /// Binder interface descriptor。
    pub interface: &'static str,
    /// Binder transaction code。
    pub code: u32,
    /// Android 平台源码中的方法名。
    pub method: &'static str,
}

/// 指定 SDK 版本的平台方法表视图。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AndroidPlatformMethods {
    sdk: u16,
}

impl AndroidPlatformMethods {
    /// 创建指定 Android SDK 版本的方法表视图。
    pub const fn new(sdk: u16) -> Self {
        Self { sdk }
    }

    /// 查询平台 Binder 方法；非平台接口、未知 code 或不支持的 SDK 返回 `None`。
    pub fn lookup(self, interface: &str, code: u32) -> Option<AndroidPlatformMethod> {
        let sdk_mask = sdk_mask(self.sdk)?;

        android_platform_methods::ANDROID_PLATFORM_METHODS
            .iter()
            .find(|entry| entry.matches(interface, code, sdk_mask))
            .map(|entry| AndroidPlatformMethod {
                interface: entry.interface,
                code: entry.code,
                method: entry.method,
            })
    }

    /// 查询方法名；没有命中时返回空字符串，方便 TUI 直接展示。
    pub fn method_name_or_empty(self, interface: &str, code: u32) -> &'static str {
        self.lookup(interface, code)
            .map(|method| method.method)
            .unwrap_or("")
    }
}

/// 从 Binder Parcel payload 中读取 interface token。
///
/// Java `Parcel.writeInterfaceToken()` 会先写入 Binder 调用头，再写入 UTF-16
/// descriptor。不同 Android 版本头部字段有演进，这里按现代和旧布局都尝试一次。
pub fn parse_interface_token(payload: &[u8]) -> Option<String> {
    [12, 8, 4, 0]
        .into_iter()
        .find_map(|offset| read_string16(payload, offset))
        .filter(|token| is_interface_descriptor(token))
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct PlatformMethodEntry {
    pub(super) sdk_mask: u16,
    pub(super) interface: &'static str,
    pub(super) code: u32,
    pub(super) method: &'static str,
}

impl PlatformMethodEntry {
    fn matches(&self, interface: &str, code: u32, sdk_mask: u16) -> bool {
        self.interface == interface && self.code == code && (self.sdk_mask & sdk_mask) != 0
    }
}

const fn sdk_mask(sdk: u16) -> Option<u16> {
    if sdk < MIN_ANDROID_SDK || sdk > MAX_ANDROID_SDK {
        None
    } else {
        Some(1 << (sdk - MIN_ANDROID_SDK))
    }
}

fn read_string16(payload: &[u8], offset: usize) -> Option<String> {
    let length = read_i32_le(payload, offset)?;
    if length < 0 {
        return None;
    }

    let length = usize::try_from(length).ok()?;
    if length == 0 || length > 512 {
        return None;
    }

    let data_offset = offset.checked_add(4)?;
    let byte_len = length.checked_mul(2)?;
    let nul_offset = data_offset.checked_add(byte_len)?;
    let end = nul_offset.checked_add(2)?;
    if end > payload.len() {
        return None;
    }

    let units = payload[data_offset..nul_offset]
        .chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&units).ok()
}

fn read_i32_le(payload: &[u8], offset: usize) -> Option<i32> {
    let bytes = payload.get(offset..offset.checked_add(4)?)?;
    Some(i32::from_le_bytes(bytes.try_into().ok()?))
}

fn is_interface_descriptor(token: &str) -> bool {
    token.contains('.')
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'$'))
}

#[cfg(test)]
mod tests {
    use super::{AndroidPlatformMethods, parse_interface_token};

    #[test]
    fn lookup_uses_sdk_specific_platform_table() {
        let sdk34 = AndroidPlatformMethods::new(34);

        assert_eq!(
            sdk34.method_name_or_empty("android.content.IContentProvider", 24),
            "createCancellationSignal"
        );
        assert_eq!(
            sdk34.method_name_or_empty("android.view.autofill.IAutoFillManager", 6),
            "updateSession"
        );
        assert_eq!(
            sdk34.method_name_or_empty("android.database.IBulkCursor", 7),
            "close"
        );
    }

    #[test]
    fn lookup_returns_empty_for_unknown_or_out_of_range_sdk() {
        assert_eq!(
            AndroidPlatformMethods::new(34).method_name_or_empty("com.example.IFoo", 1),
            ""
        );
        assert_eq!(
            AndroidPlatformMethods::new(29)
                .method_name_or_empty("android.content.IContentProvider", 24),
            ""
        );
    }

    #[test]
    fn parses_modern_interface_token_header() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0_i32.to_le_bytes());
        payload.extend_from_slice(&(-1_i32).to_le_bytes());
        payload.extend_from_slice(b"SYST");
        write_string16(&mut payload, "android.content.IContentProvider");

        assert_eq!(
            parse_interface_token(&payload).as_deref(),
            Some("android.content.IContentProvider")
        );
    }

    #[test]
    fn rejects_non_descriptor_strings() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0_i32.to_le_bytes());
        payload.extend_from_slice("android.content.pm.ResolveInfo|乱码".as_bytes());

        assert_eq!(parse_interface_token(&payload), None);
    }

    fn write_string16(output: &mut Vec<u8>, value: &str) {
        let units = value.encode_utf16().collect::<Vec<_>>();
        output.extend_from_slice(&(units.len() as i32).to_le_bytes());
        for unit in units {
            output.extend_from_slice(&unit.to_le_bytes());
        }
        output.extend_from_slice(&0_u16.to_le_bytes());
        while !output.len().is_multiple_of(4) {
            output.push(0);
        }
    }
}
