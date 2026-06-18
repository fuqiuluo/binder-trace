use std::env;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

pub(crate) const DEFAULT_MARKER_PATH: &str = "/data/local/tmp/.fuqiuluo";

const SCHEMA_VERSION: u32 = 1;
const PROJECT: &str = "binder-trace";
const CHECK_ID: &str = "bt.startup.marker";
const PURPOSE: &str = "audit_runtime_presence";
const BUILD_SIGNATURE_COVERED: &str = "official_release_manifest";
const DEFAULT_SIGNATURE_ALGORITHM: &str = "ed25519";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum MarkerWrite {
    Written,
    SkippedMissingParent,
}

#[derive(Debug, Serialize)]
struct StartupMarker {
    schema_version: u32,
    project: &'static str,
    check_id: &'static str,
    purpose: &'static str,
    version: &'static str,
    started_at_unix_ms: u64,
    pid: u32,
    executable: Option<String>,
    build_signature: BuildSignature,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum BuildSignature {
    Unsigned,
    Present {
        algorithm: &'static str,
        key_id: &'static str,
        signature: &'static str,
        covered: &'static str,
    },
}

impl StartupMarker {
    fn current() -> io::Result<Self> {
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            project: PROJECT,
            check_id: CHECK_ID,
            purpose: PURPOSE,
            version: env!("CARGO_PKG_VERSION"),
            started_at_unix_ms: unix_time_ms()?,
            pid: process::id(),
            executable: env::current_exe()
                .ok()
                .map(|path| path.display().to_string()),
            build_signature: BuildSignature::compile_time(),
        })
    }
}

impl BuildSignature {
    fn compile_time() -> Self {
        Self::from_parts(
            option_env!("BINDER_TRACE_MARKER_KEY_ID"),
            option_env!("BINDER_TRACE_MARKER_BUILD_SIGNATURE"),
            option_env!("BINDER_TRACE_MARKER_SIGNATURE_ALGORITHM"),
        )
    }

    fn from_parts(
        key_id: Option<&'static str>,
        signature: Option<&'static str>,
        algorithm: Option<&'static str>,
    ) -> Self {
        let key_id = non_empty(key_id);
        let signature = non_empty(signature);

        match (key_id, signature) {
            (Some(key_id), Some(signature)) => Self::Present {
                algorithm: non_empty(algorithm).unwrap_or(DEFAULT_SIGNATURE_ALGORITHM),
                key_id,
                signature,
                covered: BUILD_SIGNATURE_COVERED,
            },
            (None, _) | (_, None) => Self::Unsigned,
        }
    }
}

pub(crate) fn write_default() -> io::Result<MarkerWrite> {
    let path = Path::new(DEFAULT_MARKER_PATH);
    let Some(parent) = path.parent() else {
        return write_to(path);
    };

    if !parent.is_dir() {
        return Ok(MarkerWrite::SkippedMissingParent);
    }

    write_to(path)
}

fn write_to(path: &Path) -> io::Result<MarkerWrite> {
    let marker = StartupMarker::current()?;
    let mut json = serde_json::to_vec_pretty(&marker).map_err(io::Error::other)?;
    json.push(b'\n');

    let tmp_path = temporary_path(path);
    if let Err(error) = write_atomic(path, &tmp_path, &json) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error);
    }

    Ok(MarkerWrite::Written)
}

fn write_atomic(path: &Path, tmp_path: &Path, json: &[u8]) -> io::Result<()> {
    fs::write(tmp_path, json)?;

    #[cfg(unix)]
    fs::set_permissions(tmp_path, fs::Permissions::from_mode(0o644))?;

    fs::rename(tmp_path, path)
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("startup-marker");
    path.with_file_name(format!("{file_name}.{}.tmp", process::id()))
}

fn unix_time_ms() -> io::Result<u64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?;
    Ok(duration.as_millis().min(u64::MAX as u128) as u64)
}

fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::{BuildSignature, MarkerWrite, write_to};
    use serde_json::Value;
    use std::fs;
    use std::path::PathBuf;
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn writes_startup_marker_json() {
        let dir = unique_temp_dir();
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let path = dir.join(".fuqiuluo");

        let result = write_to(&path).expect("marker should be written");
        assert_eq!(result, MarkerWrite::Written);

        let json = fs::read_to_string(&path).expect("marker should be readable");
        let value: Value = serde_json::from_str(&json).expect("marker should be valid json");

        assert_eq!(value["schema_version"].as_u64(), Some(1));
        assert_eq!(value["project"].as_str(), Some("binder-trace"));
        assert_eq!(value["check_id"].as_str(), Some("bt.startup.marker"));
        assert_eq!(value["purpose"].as_str(), Some("audit_runtime_presence"));
        assert_eq!(value["pid"].as_u64(), Some(process::id().into()));
        assert!(value["started_at_unix_ms"].as_u64().is_some());
        assert!(value["build_signature"]["state"].as_str().is_some());

        fs::remove_dir_all(dir).expect("temp dir should be removable");
    }

    #[test]
    fn build_signature_requires_key_and_signature() {
        assert!(matches!(
            BuildSignature::from_parts(Some("key-1"), None, Some("ed25519")),
            BuildSignature::Unsigned
        ));
        assert!(matches!(
            BuildSignature::from_parts(None, Some("sig-1"), Some("ed25519")),
            BuildSignature::Unsigned
        ));
        assert!(matches!(
            BuildSignature::from_parts(Some("key-1"), Some("sig-1"), None),
            BuildSignature::Present {
                algorithm: "ed25519",
                key_id: "key-1",
                signature: "sig-1",
                covered: "official_release_manifest",
            }
        ));
    }

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("binder-trace-marker-{}-{nanos}", process::id()))
    }
}
