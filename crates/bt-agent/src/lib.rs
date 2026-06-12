use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use bt_common::{BinderDevice, EventKind, RawBinderEvent};
use bt_decoder::{DecodeError, Decoder};
use bt_storage::{EventEnvelope, JsonlSink, ProgramVersion, StorageError};

mod kernel_config;

pub use kernel_config::{
    CaptureMode, KernelCapabilities, KernelCapability, KernelConfig, KernelConfigError,
    KernelConfigSource,
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AgentConfig {
    pub output: OutputConfig,
    pub capture_modes: CaptureMode,
    pub kernel_config_source: KernelConfigSource,
    pub device_id: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            output: OutputConfig::Stdout,
            capture_modes: CaptureMode::EBPF_KPROBE,
            kernel_config_source: KernelConfigSource::ProcConfigGz,
            device_id: None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum OutputConfig {
    Stdout,
    JsonlFile(PathBuf),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Agent {
    config: AgentConfig,
}

impl Agent {
    pub const fn new(config: AgentConfig) -> Self {
        Self { config }
    }

    pub fn run(&self) -> Result<(), AgentError> {
        self.require_kernel_capabilities()?;

        let decoder = Decoder::default();
        let event = decoder.decode(&startup_event())?;
        let device_id = self.device_id();
        let envelope = EventEnvelope {
            device_id: &device_id,
            timestamp_ns: event.timestamp_ns,
            object: "agent.diagnostic",
        };

        match &self.config.output {
            OutputConfig::Stdout => {
                let stdout = io::stdout();
                let mut sink = JsonlSink::new(stdout.lock());
                self.write_startup_events(&mut sink, &device_id, envelope, &event)?;
                sink.flush()?;
            }
            OutputConfig::JsonlFile(path) => {
                let file = File::create(path)?;
                let mut sink = JsonlSink::new(BufWriter::new(file));
                self.write_startup_events(&mut sink, &device_id, envelope, &event)?;
                sink.flush()?;
            }
        }

        Ok(())
    }

    fn write_startup_events(
        &self,
        sink: &mut JsonlSink<impl io::Write>,
        device_id: &str,
        diagnostic_envelope: EventEnvelope<'_>,
        diagnostic_event: &bt_decoder::DecodedEvent,
    ) -> Result<(), AgentError> {
        sink.write_program_version(
            EventEnvelope {
                device_id,
                timestamp_ns: diagnostic_envelope.timestamp_ns,
                object: "program.version",
            },
            ProgramVersion {
                program: "binder-trace",
                version: env!("CARGO_PKG_VERSION"),
            },
        )?;
        sink.write_event(diagnostic_envelope, diagnostic_event)?;
        Ok(())
    }

    fn device_id(&self) -> String {
        self.config
            .device_id
            .clone()
            .or_else(device_id_from_env)
            .or_else(|| android_property("ro.serialno"))
            .or_else(|| android_property("ro.boot.serialno"))
            .unwrap_or_else(|| "unknown".to_owned())
    }

    fn require_kernel_capabilities(&self) -> Result<(), AgentError> {
        let config_text = self.config.kernel_config_source.read_to_string()?;
        let kernel_config = KernelConfig::parse(&config_text);
        let capabilities = KernelCapabilities::from_config(&kernel_config);

        if !capabilities.has_any_ebpf() {
            return Err(AgentError::NoEbpfCapabilities);
        }

        if !capabilities.supports(self.config.capture_modes) {
            return Err(AgentError::UnsupportedCaptureMode {
                modes: self.config.capture_modes,
                missing: capabilities.missing_for(self.config.capture_modes),
                detected: capabilities.names(),
            });
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum AgentError {
    KernelConfig(KernelConfigError),
    NoEbpfCapabilities,
    UnsupportedCaptureMode {
        modes: CaptureMode,
        missing: Vec<KernelCapability>,
        detected: Vec<&'static str>,
    },
    Decode(DecodeError),
    Io(io::Error),
    Storage(StorageError),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KernelConfig(error) => write!(f, "failed to inspect kernel config: {error}"),
            Self::NoEbpfCapabilities => {
                write!(
                    f,
                    "refusing to start: no eBPF kernel capabilities found in config.gz"
                )
            }
            Self::UnsupportedCaptureMode {
                modes,
                missing,
                detected,
            } => write!(
                f,
                "refusing to start: capture modes `{modes}` require {}, detected capabilities: {}",
                format_capabilities(missing),
                format_detected_capabilities(detected)
            ),
            Self::Decode(error) => write!(f, "failed to decode event: {error}"),
            Self::Io(error) => write!(f, "agent I/O failed: {error}"),
            Self::Storage(error) => write!(f, "failed to persist event: {error}"),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<KernelConfigError> for AgentError {
    fn from(error: KernelConfigError) -> Self {
        Self::KernelConfig(error)
    }
}

impl From<DecodeError> for AgentError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

impl From<io::Error> for AgentError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<StorageError> for AgentError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

fn startup_event() -> RawBinderEvent {
    let mut event = RawBinderEvent::new(EventKind::Diagnostic, BinderDevice::UNKNOWN);
    event.header.pid = std::process::id();
    event.header.timestamp_ns = timestamp_ns();
    event
}

fn timestamp_ns() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    }
}

fn format_capabilities(capabilities: &[KernelCapability]) -> String {
    capabilities
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_detected_capabilities(capabilities: &[&'static str]) -> String {
    if capabilities.is_empty() {
        "none".to_owned()
    } else {
        capabilities.join(", ")
    }
}

fn device_id_from_env() -> Option<String> {
    std::env::var("BINDER_TRACE_DEVICE_ID")
        .ok()
        .and_then(non_empty_string)
}

fn android_property(name: &str) -> Option<String> {
    let output = Command::new("getprop").arg(name).output().ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout)
        .ok()
        .and_then(non_empty_string)
}

fn non_empty_string(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Agent, AgentConfig, AgentError, KernelConfigSource, OutputConfig};

    #[test]
    fn refuses_to_start_without_ebpf_capabilities() {
        let config_path = write_temp_config(
            "no-ebpf",
            "\
# CONFIG_BPF is not set
# CONFIG_BPF_SYSCALL is not set
# CONFIG_BPF_EVENTS is not set
# CONFIG_KPROBES is not set
",
        );
        let output_path = temp_path("no-ebpf-output");
        let agent = Agent::new(AgentConfig {
            output: OutputConfig::JsonlFile(output_path),
            kernel_config_source: KernelConfigSource::TextFile(config_path.clone()),
            ..AgentConfig::default()
        });

        let error = agent.run().expect_err("agent must reject missing eBPF");

        assert!(matches!(error, AgentError::NoEbpfCapabilities));
        fs::remove_file(config_path).expect("temp config should be removable");
    }

    #[test]
    fn starts_when_ebpf_kprobe_requirements_are_enabled() {
        let config_path = write_temp_config(
            "ebpf-kprobe",
            "\
CONFIG_BPF=y
CONFIG_BPF_SYSCALL=y
CONFIG_BPF_EVENTS=y
CONFIG_KPROBES=y
",
        );
        let output_path = temp_path("ebpf-kprobe-output");
        let agent = Agent::new(AgentConfig {
            output: OutputConfig::JsonlFile(output_path.clone()),
            kernel_config_source: KernelConfigSource::TextFile(config_path.clone()),
            ..AgentConfig::default()
        });

        agent.run().expect("agent should accept eBPF kprobe config");

        let output = fs::read_to_string(&output_path).expect("agent output should be readable");
        let mut lines = output.lines();
        let version = lines.next().expect("version event should be emitted first");
        let diagnostic = lines
            .next()
            .expect("diagnostic event should follow version event");

        assert!(version.contains("\"seq\":0"));
        assert!(version.contains("\"object\":\"program.version\""));
        assert!(version.contains("\"program\":\"binder-trace\""));
        assert!(version.contains(concat!("\"version\":\"", env!("CARGO_PKG_VERSION"), "\"")));
        assert!(diagnostic.contains("\"seq\":1"));
        assert!(output.contains("\"object\":\"agent.diagnostic\""));
        assert!(output.contains("\"data\":{\"kind\":\"diagnostic\""));
        fs::remove_file(config_path).expect("temp config should be removable");
        fs::remove_file(output_path).expect("temp output should be removable");
    }

    fn write_temp_config(name: &str, text: &str) -> std::path::PathBuf {
        let path = temp_path(name);
        fs::write(&path, text).expect("temp config should be writable");
        path
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("binder-trace-{name}-{}", std::process::id()))
    }
}
