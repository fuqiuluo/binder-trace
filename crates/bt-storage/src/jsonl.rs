//! JSON Lines 存储后端。
//!
//! # 职责
//! - 维护面向追加写入的 JSONL 序列化规则。
//! - JSONL 专用 DTO 只服务行式导出；后续存储后端接入时保持 DTO 私有。

mod record;

use std::io::Write;

use bt_decoder::DecodedEvent;
use record::{DecodedEventData, JsonEnvelope, ProgramVersionData};
use serde::Serialize;

use crate::{EventEnvelope, ProgramVersion, StorageError};

/// 每行写入一个紧凑 JSON 对象的存储 sink。
///
/// sink 自己维护单调递增的 `seq` 字段。写入是阻塞式的，目标是内部包裹的 `Write`。
pub struct JsonlSink<W> {
    writer: W,
    next_sequence: u64,
}

impl<W> JsonlSink<W>
where
    W: Write,
{
    /// 创建一个下一条序号从 0 开始的 JSONL sink。
    pub const fn new(writer: W) -> Self {
        Self {
            writer,
            next_sequence: 0,
        }
    }

    /// 把一条已解码事件写成一条 JSONL 记录。
    pub fn write_event(
        &mut self,
        envelope: EventEnvelope<'_>,
        event: &DecodedEvent,
    ) -> Result<(), StorageError> {
        self.write_json_line(envelope, DecodedEventData::new(event))
    }

    /// 把启动程序元数据写成一条 JSONL 记录。
    pub fn write_program_version(
        &mut self,
        envelope: EventEnvelope<'_>,
        version: ProgramVersion<'_>,
    ) -> Result<(), StorageError> {
        self.write_json_line(
            envelope,
            ProgramVersionData {
                program: version.program,
                version: version.version,
            },
        )
    }

    /// 刷新内部 writer。
    pub fn flush(&mut self) -> Result<(), StorageError> {
        self.writer.flush().map_err(StorageError::Io)
    }

    fn allocate_sequence(&mut self) -> Result<u64, StorageError> {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(StorageError::SequenceOverflow)?;
        Ok(sequence)
    }

    fn write_json_line<T>(
        &mut self,
        envelope: EventEnvelope<'_>,
        data: T,
    ) -> Result<(), StorageError>
    where
        T: Serialize,
    {
        let sequence = self.allocate_sequence()?;
        let message = JsonEnvelope {
            device_id: envelope.device_id,
            seq: sequence,
            timestamp_ns: envelope.timestamp_ns,
            object: envelope.object,
            data,
        };

        serde_json::to_writer(&mut self.writer, &message)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bt_common::{BinderDevice, EventKind};
    use bt_decoder::DecodedEvent;
    use serde_json::Value;

    use crate::{EventEnvelope, JsonlSink, ProgramVersion};

    fn parse_json_line(output: Vec<u8>) -> Value {
        let json = String::from_utf8(output).expect("json should be utf-8");
        assert!(json.ends_with('\n'));
        serde_json::from_str(json.trim_end()).expect("json line should parse")
    }

    fn parse_json_lines(output: Vec<u8>) -> Vec<Value> {
        let json = String::from_utf8(output).expect("json should be utf-8");
        json.lines()
            .map(|line| serde_json::from_str(line).expect("json line should parse"))
            .collect()
    }

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

        let value = parse_json_line(output);
        assert_eq!(value["device_id"].as_str(), Some("device-1"));
        assert_eq!(value["seq"].as_u64(), Some(0));
        assert_eq!(value["timestamp_ns"].as_u64(), Some(9));
        assert_eq!(value["object"].as_str(), Some("agent.diagnostic"));
        assert_eq!(value["data"]["kind"].as_str(), Some("diagnostic"));
        assert_eq!(value["data"]["binder_device"].as_str(), Some("unknown"));
        assert_eq!(value["data"]["process"]["pid"].as_u64(), Some(123));
        assert_eq!(value["data"]["process"]["tid"].as_u64(), Some(124));
        assert_eq!(value["data"]["process"]["uid"].as_u64(), Some(2000));
        assert!(value["data"]["transaction"].is_null());
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
        let mut sink = JsonlSink::new(&mut output);

        sink.write_event(envelope, &event)
            .expect("first event should be written");
        sink.write_event(envelope, &event)
            .expect("second event should be written");

        let lines = parse_json_lines(output);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["seq"].as_u64(), Some(0));
        assert_eq!(lines[1]["seq"].as_u64(), Some(1));
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

        let value = parse_json_line(output);
        assert_eq!(value["device_id"].as_str(), Some("device-1"));
        assert_eq!(value["seq"].as_u64(), Some(0));
        assert_eq!(value["object"].as_str(), Some("program.version"));
        assert_eq!(value["data"]["program"].as_str(), Some("binder-trace"));
        assert_eq!(value["data"]["version"].as_str(), Some("0.1.0"));
    }
}
