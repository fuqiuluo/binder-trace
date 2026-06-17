use std::fs;
use std::path::PathBuf;

use bt_decoder::{AndroidPlatformMethods, set_android_platform_methods_tsv_path};

#[test]
fn releases_embedded_platform_methods_to_configured_path() {
    let path = temp_path("android-platform-methods.tsv");
    let _ = fs::remove_file(&path);

    set_android_platform_methods_tsv_path(path.clone()).expect("path should be configurable");

    assert_eq!(
        AndroidPlatformMethods::new(35)
            .method_name_or_empty("android.system.keystore2.IKeystoreService", 1),
        "getSecurityLevel"
    );

    let tsv = fs::read_to_string(&path).expect("embedded method table should be released");
    assert!(tsv.contains("android.system.keystore2.IKeystoreService"));

    fs::remove_file(path).expect("released method table should be removable");
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("binder-trace-{name}-{}", std::process::id()))
}
