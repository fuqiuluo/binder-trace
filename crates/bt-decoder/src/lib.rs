use std::fmt;

use bt_common::{BinderDevice, EventKind, MAX_INLINE_PAYLOAD, RawBinderEvent, UnknownEventKind};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Decoder {
    config: DecoderConfig,
}

impl Decoder {
    pub const fn new(config: DecoderConfig) -> Self {
        Self { config }
    }

    pub fn decode(&self, raw: &RawBinderEvent) -> Result<DecodedEvent, DecodeError> {
        let kind = EventKind::try_from(raw.header.kind)?;
        let transaction = match kind {
            EventKind::Diagnostic => None,
            EventKind::IoctlEnter
            | EventKind::IoctlExit
            | EventKind::Transaction
            | EventKind::Reply => Some(self.decode_transaction(raw)?),
        };

        Ok(DecodedEvent {
            kind,
            device: BinderDevice::from(raw.header.device),
            pid: raw.header.pid,
            tid: raw.header.tid,
            uid: raw.header.uid,
            flags: raw.header.flags,
            timestamp_ns: raw.header.timestamp_ns,
            sequence: raw.header.sequence,
            transaction,
        })
    }

    fn decode_transaction(&self, raw: &RawBinderEvent) -> Result<DecodedTransaction, DecodeError> {
        let payload_len = usize::try_from(raw.transaction.payload_len)
            .map_err(|_| DecodeError::PayloadLengthOutOfRange(raw.transaction.payload_len))?;

        if payload_len > MAX_INLINE_PAYLOAD {
            return Err(DecodeError::PayloadLengthOutOfRange(
                raw.transaction.payload_len,
            ));
        }

        let payload_len = core::cmp::min(payload_len, self.config.max_payload_bytes);

        Ok(DecodedTransaction {
            code: raw.transaction.code,
            flags: raw.transaction.flags,
            data_size: raw.transaction.data_size,
            offsets_size: raw.transaction.offsets_size,
            target_handle: raw.transaction.target_handle,
            sender_pid: raw.transaction.sender_pid,
            sender_euid: raw.transaction.sender_euid,
            payload_truncated: raw.transaction.payload_truncated != 0
                || payload_len < raw.transaction.payload_len as usize,
            payload: raw.transaction.payload[..payload_len].to_vec(),
        })
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new(DecoderConfig::default())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DecoderConfig {
    pub max_payload_bytes: usize,
}

impl Default for DecoderConfig {
    fn default() -> Self {
        Self {
            max_payload_bytes: MAX_INLINE_PAYLOAD,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DecodedEvent {
    pub kind: EventKind,
    pub device: BinderDevice,
    pub pid: u32,
    pub tid: u32,
    pub uid: u32,
    pub flags: u32,
    pub timestamp_ns: u64,
    pub sequence: u64,
    pub transaction: Option<DecodedTransaction>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DecodedTransaction {
    pub code: u32,
    pub flags: u32,
    pub data_size: u64,
    pub offsets_size: u64,
    pub target_handle: u32,
    pub sender_pid: u32,
    pub sender_euid: u32,
    pub payload_truncated: bool,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum DecodeError {
    UnknownEventKind(u16),
    PayloadLengthOutOfRange(u32),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEventKind(kind) => write!(f, "unknown raw event kind: {kind}"),
            Self::PayloadLengthOutOfRange(length) => {
                write!(
                    f,
                    "raw payload length exceeds inline event payload: {length}"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<UnknownEventKind> for DecodeError {
    fn from(error: UnknownEventKind) -> Self {
        Self::UnknownEventKind(error.raw())
    }
}

pub fn decode_raw_event(raw: &RawBinderEvent) -> Result<DecodedEvent, DecodeError> {
    Decoder::default().decode(raw)
}
