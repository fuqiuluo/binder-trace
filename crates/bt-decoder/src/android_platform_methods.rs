//! Android 平台 Binder 方法表。
//!
//! 这里先维护 TUI 首屏常见的 framework Binder 接口。第三方接口、厂商接口、
//! 未收录的平台接口都返回空方法名。
//!
//! SDK mask 从 Android 11/API 30 开始：bit0=30，bit1=31，依次到 bit6=36。
//! 维护时对照 AOSP release 分支源码：
//! - https://android.googlesource.com/platform/frameworks/base/+/refs/heads/android11-release/
//! - https://android.googlesource.com/platform/frameworks/base/+/refs/heads/android12-release/
//! - https://android.googlesource.com/platform/frameworks/base/+/refs/heads/android12L-release/
//! - https://android.googlesource.com/platform/frameworks/base/+/refs/heads/android13-release/
//! - https://android.googlesource.com/platform/frameworks/base/+/refs/heads/android14-release/
//! - https://android.googlesource.com/platform/frameworks/base/+/refs/heads/android15-release/
//! - https://android.googlesource.com/platform/frameworks/base/+/refs/heads/android16-release/

use super::PlatformMethodEntry;

const SDK_30_36: u16 = 0x7f;
const SDK_31_36: u16 = 0x7e;
const SDK_34_36: u16 = 0x70;

pub(super) const ANDROID_PLATFORM_METHODS: &[PlatformMethodEntry] = &[
    // core/java/android/content/IContentProvider.java
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 1,
        method: "query",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 2,
        method: "getType",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 3,
        method: "insert",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 4,
        method: "delete",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 10,
        method: "update",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 13,
        method: "bulkInsert",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 14,
        method: "openFile",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 15,
        method: "openAssetFile",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 20,
        method: "applyBatch",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 21,
        method: "call",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 22,
        method: "getStreamTypes",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 23,
        method: "openTypedAssetFile",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 24,
        method: "createCancellationSignal",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 25,
        method: "canonicalize",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 26,
        method: "uncanonicalize",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 27,
        method: "refresh",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 28,
        method: "checkUriPermission",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 29,
        method: "getTypeAsync",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IContentProvider",
        code: 30,
        method: "canonicalizeAsync",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_31_36,
        interface: "android.content.IContentProvider",
        code: 31,
        method: "uncanonicalizeAsync",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_34_36,
        interface: "android.content.IContentProvider",
        code: 32,
        method: "getTypeAnonymousAsync",
    },
    // core/java/android/database/IBulkCursor.java
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IBulkCursor",
        code: 1,
        method: "getWindow",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IBulkCursor",
        code: 2,
        method: "deactivate",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IBulkCursor",
        code: 3,
        method: "requery",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IBulkCursor",
        code: 4,
        method: "onMove",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IBulkCursor",
        code: 5,
        method: "getExtras",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IBulkCursor",
        code: 6,
        method: "respond",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.content.IBulkCursor",
        code: 7,
        method: "close",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.database.IBulkCursor",
        code: 1,
        method: "getWindow",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.database.IBulkCursor",
        code: 2,
        method: "deactivate",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.database.IBulkCursor",
        code: 3,
        method: "requery",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.database.IBulkCursor",
        code: 4,
        method: "onMove",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.database.IBulkCursor",
        code: 5,
        method: "getExtras",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.database.IBulkCursor",
        code: 6,
        method: "respond",
    },
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.database.IBulkCursor",
        code: 7,
        method: "close",
    },
    // media/java/android/media/IAudioService.aidl
    PlatformMethodEntry {
        sdk_mask: 0x06,
        interface: "android.media.IAudioService",
        code: 47,
        method: "getSurroundFormats",
    },
    PlatformMethodEntry {
        sdk_mask: 0x08,
        interface: "android.media.IAudioService",
        code: 49,
        method: "getSurroundFormats",
    },
    PlatformMethodEntry {
        sdk_mask: 0x30,
        interface: "android.media.IAudioService",
        code: 56,
        method: "getSurroundFormats",
    },
    PlatformMethodEntry {
        sdk_mask: 0x40,
        interface: "android.media.IAudioService",
        code: 63,
        method: "getSurroundFormats",
    },
    // core/java/android/view/autofill/IAutoFillManager.aidl
    PlatformMethodEntry {
        sdk_mask: SDK_30_36,
        interface: "android.view.autofill.IAutoFillManager",
        code: 6,
        method: "updateSession",
    },
];
