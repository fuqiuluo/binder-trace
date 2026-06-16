//! 仅供 JSONL 后端使用的记录 DTO。
//!
//! # 职责
//! - 固定对外 JSON 字段名和格式细节。
//! - 通过私有 DTO 隔离行式导出格式，让后续数据库 schema 可以独立演进。

use std::fmt;

use bt_decoder::{DecodedEvent, DecodedTransaction};
use serde::{Serialize, Serializer};

#[derive(Debug, Serialize)]
pub(super) struct JsonEnvelope<'a, T> {
    pub(super) device_id: &'a str,
    pub(super) seq: u64,
    pub(super) timestamp_ns: u64,
    pub(super) object: &'a str,
    pub(super) data: T,
}

#[derive(Debug, Serialize)]
struct Process {
    pid: u32,
    tid: u32,
    uid: u32,
}

#[derive(Debug, Serialize)]
pub(super) struct DecodedEventData<'a> {
    kind: &'static str,
    binder_device: &'static str,
    process: Process,
    flags: u32,
    sequence: u64,
    transaction: Option<DecodedTransactionData<'a>>,
}

impl<'a> DecodedEventData<'a> {
    pub(super) fn new(event: &'a DecodedEvent) -> Self {
        Self {
            kind: event.kind.name(),
            binder_device: event.device.name(),
            process: Process {
                pid: event.pid,
                tid: event.tid,
                uid: event.uid,
            },
            flags: event.flags,
            sequence: event.sequence,
            transaction: event.transaction.as_ref().map(DecodedTransactionData::new),
        }
    }
}

#[derive(Debug, Serialize)]
struct DecodedTransactionData<'a> {
    code: u32,
    flags: u32,
    data_size: u64,
    offsets_size: u64,
    target_handle: u32,
    sender_pid: u32,
    sender_euid: u32,
    payload_truncated: bool,
    payload_hex: HexBytes<'a>,
}

impl<'a> DecodedTransactionData<'a> {
    fn new(transaction: &'a DecodedTransaction) -> Self {
        Self {
            code: transaction.code,
            flags: transaction.flags,
            data_size: transaction.data_size,
            offsets_size: transaction.offsets_size,
            target_handle: transaction.target_handle,
            sender_pid: transaction.sender_pid,
            sender_euid: transaction.sender_euid,
            payload_truncated: transaction.payload_truncated,
            payload_hex: HexBytes(&transaction.payload),
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ProgramVersionData<'a> {
    pub(super) program: &'a str,
    pub(super) version: &'a str,
}

#[derive(Debug, Clone, Copy)]
struct HexBytes<'a>(&'a [u8]);

impl fmt::Display for HexBytes<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }

        Ok(())
    }
}

impl Serialize for HexBytes<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}
