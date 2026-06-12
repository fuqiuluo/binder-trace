use std::fmt;
use std::io::{self, Write};

use bt_decoder::{DecodedEvent, DecodedTransaction};

pub struct JsonlSink<W> {
    writer: W,
}

impl<W> JsonlSink<W>
where
    W: Write,
{
    pub const fn new(writer: W) -> Self {
        Self { writer }
    }

    pub fn write_event(&mut self, event: &DecodedEvent) -> Result<(), StorageError> {
        write!(
            self.writer,
            "{{\"kind\":\"{}\",\"device\":\"{}\",\"pid\":{},\"tid\":{},\"uid\":{},\"flags\":{},\"timestamp_ns\":{},\"sequence\":{}",
            event.kind.name(),
            event.device.name(),
            event.pid,
            event.tid,
            event.uid,
            event.flags,
            event.timestamp_ns,
            event.sequence
        )?;

        match &event.transaction {
            Some(transaction) => {
                write!(self.writer, ",\"transaction\":")?;
                write_transaction(&mut self.writer, transaction)?;
            }
            None => write!(self.writer, ",\"transaction\":null")?,
        }

        writeln!(self.writer, "}}")?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), StorageError> {
        self.writer.flush().map_err(StorageError::Io)
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}

#[derive(Debug)]
pub enum StorageError {
    Io(io::Error),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "storage I/O failed: {error}"),
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
