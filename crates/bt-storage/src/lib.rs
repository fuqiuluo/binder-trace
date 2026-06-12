use std::fmt;
use std::io::{self, Write};

use bt_decoder::{DecodedEvent, DecodedTransaction};

/// 写入消息队列或 JSONL 文件前的统一事件信封。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct EventEnvelope<'a> {
    pub device_id: &'a str,
    pub timestamp_ns: u64,
    pub object: &'a str,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ProgramVersion<'a> {
    pub program: &'a str,
    pub version: &'a str,
}

pub struct JsonlSink<W> {
    writer: W,
    next_sequence: u64,
}

impl<W> JsonlSink<W>
where
    W: Write,
{
    pub const fn new(writer: W) -> Self {
        Self {
            writer,
            next_sequence: 0,
        }
    }

    pub const fn with_initial_sequence(writer: W, next_sequence: u64) -> Self {
        Self {
            writer,
            next_sequence,
        }
    }

    pub fn write_event(
        &mut self,
        envelope: EventEnvelope<'_>,
        event: &DecodedEvent,
    ) -> Result<(), StorageError> {
        let sequence = self.allocate_sequence()?;

        self.write_envelope_prefix(envelope, sequence)?;
        write!(
            self.writer,
            ",\"data\":{{\"kind\":\"{}\",\"binder_device\":\"{}\",\"process\":{{\"pid\":{},\"tid\":{},\"uid\":{}}},\"flags\":{},\"sequence\":{}",
            event.kind.name(),
            event.device.name(),
            event.pid,
            event.tid,
            event.uid,
            event.flags,
            event.sequence
        )?;

        match &event.transaction {
            Some(transaction) => {
                write!(self.writer, ",\"transaction\":")?;
                write_transaction(&mut self.writer, transaction)?;
            }
            None => write!(self.writer, ",\"transaction\":null")?,
        }

        writeln!(self.writer, "}}}}")?;
        Ok(())
    }

    pub fn write_program_version(
        &mut self,
        envelope: EventEnvelope<'_>,
        version: ProgramVersion<'_>,
    ) -> Result<(), StorageError> {
        let sequence = self.allocate_sequence()?;

        self.write_envelope_prefix(envelope, sequence)?;
        write!(self.writer, ",\"data\":{{\"program\":")?;
        write_json_string(&mut self.writer, version.program)?;
        write!(self.writer, ",\"version\":")?;
        write_json_string(&mut self.writer, version.version)?;
        writeln!(self.writer, "}}}}")?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), StorageError> {
        self.writer.flush().map_err(StorageError::Io)
    }

    pub fn into_inner(self) -> W {
        self.writer
    }

    fn allocate_sequence(&mut self) -> Result<u64, StorageError> {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(StorageError::SequenceOverflow)?;
        Ok(sequence)
    }

    fn write_envelope_prefix(
        &mut self,
        envelope: EventEnvelope<'_>,
        sequence: u64,
    ) -> Result<(), StorageError> {
        write!(self.writer, "{{\"device_id\":")?;
        write_json_string(&mut self.writer, envelope.device_id)?;
        write!(
            self.writer,
            ",\"seq\":{},\"timestamp_ns\":{},\"object\":",
            sequence, envelope.timestamp_ns
        )?;
        write_json_string(&mut self.writer, envelope.object)?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum StorageError {
    Io(io::Error),
    SequenceOverflow,
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "storage I/O failed: {error}"),
            Self::SequenceOverflow => write!(f, "message sequence overflow"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<io::Error> for StorageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn write_transaction(
    writer: &mut impl Write,
    transaction: &DecodedTransaction,
) -> Result<(), StorageError> {
    write!(
        writer,
        "{{\"code\":{},\"flags\":{},\"data_size\":{},\"offsets_size\":{},\"target_handle\":{},\"sender_pid\":{},\"sender_euid\":{},\"payload_truncated\":{},\"payload_hex\":\"",
        transaction.code,
        transaction.flags,
        transaction.data_size,
        transaction.offsets_size,
        transaction.target_handle,
        transaction.sender_pid,
        transaction.sender_euid,
        transaction.payload_truncated
    )?;
    write_hex(writer, &transaction.payload)?;
    write!(writer, "\"}}")?;
    Ok(())
}

fn write_hex(writer: &mut impl Write, bytes: &[u8]) -> Result<(), StorageError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    for byte in bytes {
        writer.write_all(&[HEX[(byte >> 4) as usize], HEX[(byte & 0x0f) as usize]])?;
    }

    Ok(())
}

fn write_json_string(writer: &mut impl Write, value: &str) -> Result<(), StorageError> {
    writer.write_all(b"\"")?;

    for byte in value.bytes() {
        match byte {
            b'"' => writer.write_all(br#"\""#)?,
            b'\\' => writer.write_all(br#"\\"#)?,
            b'\n' => writer.write_all(br#"\n"#)?,
            b'\r' => writer.write_all(br#"\r"#)?,
            b'\t' => writer.write_all(br#"\t"#)?,
            0x00..=0x1f => write!(writer, "\\u{byte:04x}")?,
            _ => writer.write_all(&[byte])?,
        }
    }

    writer.write_all(b"\"")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use bt_common::{BinderDevice, EventKind};
    use bt_decoder::DecodedEvent;

    use super::{EventEnvelope, JsonlSink, ProgramVersion};

    #[test]
    fn writes_message_envelope() {
        let event = DecodedEvent {
            kind: EventKind::Diagnostic,
            device: BinderDevice::UNKNOWN,
            pid: 123,
            tid: 124,
            uid: 2000,
            flags: 0,
            timestamp_ns: 9,
            sequence: 7,
            transaction: None,
        };
        let envelope = EventEnvelope {
            device_id: "device-1",
            timestamp_ns: event.timestamp_ns,
            object: "agent.diagnostic",
        };
        let mut output = Vec::new();
        let mut sink = JsonlSink::new(&mut output);

        sink.write_event(envelope, &event)
            .expect("event should be written");

        let json = String::from_utf8(output).expect("json should be utf-8");
        assert!(json.contains("\"device_id\":\"device-1\""));
        assert!(json.contains("\"seq\":0"));
        assert!(json.contains("\"timestamp_ns\":9"));
        assert!(json.contains("\"object\":\"agent.diagnostic\""));
        assert!(json.contains("\"data\":{\"kind\":\"diagnostic\""));
        assert!(json.contains("\"process\":{\"pid\":123,\"tid\":124,\"uid\":2000}"));
        assert!(json.ends_with("\"transaction\":null}}\n"));
    }

    #[test]
    fn increments_message_sequence() {
        let event = DecodedEvent {
            kind: EventKind::Diagnostic,
            device: BinderDevice::UNKNOWN,
            pid: 123,
            tid: 124,
            uid: 2000,
            flags: 0,
            timestamp_ns: 9,
            sequence: 7,
            transaction: None,
        };
        let envelope = EventEnvelope {
            device_id: "device-1",
            timestamp_ns: event.timestamp_ns,
            object: "agent.diagnostic",
        };
        let mut output = Vec::new();
        let mut sink = JsonlSink::with_initial_sequence(&mut output, 41);

        sink.write_event(envelope, &event)
            .expect("first event should be written");
        sink.write_event(envelope, &event)
            .expect("second event should be written");

        let json = String::from_utf8(output).expect("json should be utf-8");
        assert!(json.contains("\"seq\":41"));
        assert!(json.contains("\"seq\":42"));
    }

    #[test]
    fn writes_program_version_envelope() {
        let envelope = EventEnvelope {
            device_id: "device-1",
            timestamp_ns: 11,
            object: "program.version",
        };
        let version = ProgramVersion {
            program: "binder-trace",
            version: "0.1.0",
        };
        let mut output = Vec::new();
        let mut sink = JsonlSink::new(&mut output);

        sink.write_program_version(envelope, version)
            .expect("version event should be written");

        let json = String::from_utf8(output).expect("json should be utf-8");
        assert!(json.contains("\"device_id\":\"device-1\""));
        assert!(json.contains("\"seq\":0"));
        assert!(json.contains("\"object\":\"program.version\""));
        assert!(json.contains("\"data\":{\"program\":\"binder-trace\",\"version\":\"0.1.0\"}"));
    }
}
