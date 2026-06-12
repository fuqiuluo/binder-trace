use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bt_common::{BinderDevice, EventKind, RawBinderEvent};
use bt_decoder::{DecodeError, Decoder};
use bt_storage::{JsonlSink, StorageError};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AgentConfig {
    pub output: OutputConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            output: OutputConfig::Stdout,
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
        let decoder = Decoder::default();
        let event = decoder.decode(&startup_event())?;

        match &self.config.output {
            OutputConfig::Stdout => {
                let stdout = io::stdout();
                let mut sink = JsonlSink::new(stdout.lock());
                sink.write_event(&event)?;
                sink.flush()?;
            }
            OutputConfig::JsonlFile(path) => {
                let file = File::create(path)?;
                let mut sink = JsonlSink::new(BufWriter::new(file));
                sink.write_event(&event)?;
                sink.flush()?;
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum AgentError {
    Decode(DecodeError),
    Io(io::Error),
    Storage(StorageError),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => write!(f, "failed to decode event: {error}"),
            Self::Io(error) => write!(f, "agent I/O failed: {error}"),
            Self::Storage(error) => write!(f, "failed to persist event: {error}"),
        }
    }
}

impl std::error::Error for AgentError {}

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
