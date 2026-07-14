//! agent 运行编排。
//!
//! # 职责
//! - 固定高层运行顺序：启动诊断事件、后续采集入口和最终 flush。
//! - 让 CLI 只负责参数转换，不直接接触输出格式和设备身份解析。

use std::fs::File;
use std::io::{self, BufWriter};
use std::time::Duration;

use bt_decoder::{DecodedEvent, Decoder};
use bt_storage::{EventEnvelope, JsonlSink, ProgramVersion};
use tracing::{debug, info, warn};

use crate::config::{AgentConfig, OutputConfig};
use crate::device::resolve_device_id;
use crate::diagnostic::startup_event;
use crate::error::AgentError;
use crate::socket_ipc::{BinderEvent, CaptureConfig, SocketIpcClient, SocketIpcError};

const EVENT_POLL_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_EVENTS_PER_BATCH: usize = 512;

trait EventSource {
    fn poll_event(&self, timeout: Duration) -> Result<bool, SocketIpcError>;
    fn try_recv_event(&self) -> Result<Option<BinderEvent>, SocketIpcError>;
}

impl EventSource for SocketIpcClient {
    fn poll_event(&self, timeout: Duration) -> Result<bool, SocketIpcError> {
        SocketIpcClient::poll_event(self, timeout)
    }

    fn try_recv_event(&self) -> Result<Option<BinderEvent>, SocketIpcError> {
        SocketIpcClient::try_recv_event(self)
    }
}

struct EventCollector<'a, S> {
    source: &'a S,
    decoder: &'a Decoder,
    device_id: &'a str,
}

impl<'a, S> EventCollector<'a, S>
where
    S: EventSource,
{
    const fn new(source: &'a S, decoder: &'a Decoder, device_id: &'a str) -> Self {
        Self {
            source,
            decoder,
            device_id,
        }
    }

    fn collect_batch<W>(&self, sink: &mut JsonlSink<W>) -> Result<usize, AgentError>
    where
        W: io::Write,
    {
        if !self.source.poll_event(EVENT_POLL_TIMEOUT)? {
            return Ok(0);
        }

        let mut written = 0;
        for _ in 0..MAX_EVENTS_PER_BATCH {
            let Some(event) = self.source.try_recv_event()? else {
                break;
            };
            let Some(raw) = event.to_raw_event() else {
                warn!(
                    kind = event.kind,
                    sequence = event.sequence,
                    "跳过未知 socket 事件类型"
                );
                continue;
            };
            let decoded = self.decoder.decode(&raw)?;
            let object = if event.is_reply() {
                "binder.reply"
            } else {
                "binder.transaction"
            };
            sink.write_event(
                EventEnvelope {
                    device_id: self.device_id,
                    timestamp_ns: decoded.timestamp_ns,
                    object,
                },
                &decoded,
            )?;
            written += 1;
        }

        if written > 0 {
            sink.flush()?;
        }
        Ok(written)
    }
}

struct CaptureConfigGuard<'a> {
    client: &'a SocketIpcClient,
    previous: CaptureConfig,
}

impl<'a> CaptureConfigGuard<'a> {
    fn enable(client: &'a SocketIpcClient) -> Result<Self, SocketIpcError> {
        let previous = client.get_config()?;
        client.set_config(CaptureConfig::binder_transaction_enabled())?;
        Ok(Self { client, previous })
    }
}

impl Drop for CaptureConfigGuard<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.client.set_config(self.previous) {
            warn!(%error, "恢复原始内核捕获配置失败");
        }
    }
}

/// 使用一份确定的 [`AgentConfig`] 运行 Binder trace。
///
/// `Agent` 只负责协调阻塞式 I/O；具体采集、解码和输出格式由下层模块维护。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Agent {
    config: AgentConfig,
}

impl Agent {
    /// 从已经解析完成的配置创建 agent。
    pub const fn new(config: AgentConfig) -> Self {
        Self { config }
    }

    /// 执行启动检查并持续写入 Binder transaction 事件。
    ///
    /// # 错误
    /// 当内核能力不满足、socket 事件流中断、事件解码失败或输出失败时返回
    /// [`AgentError`]。成功启动后会阻塞读取，直到发生错误或进程被外部终止。
    pub fn run(&self) -> Result<(), AgentError> {
        info!("启动 binder-trace agent");

        let client = SocketIpcClient::connect()?;
        if !client.get_feature()?.has_event_stream() {
            return Err(AgentError::EventStreamUnsupported);
        }
        let capture_guard = CaptureConfigGuard::enable(&client)?;
        let device_id = resolve_device_id(self.config.device_id.as_deref());
        debug!(device_id, "设备标识解析完成");

        let result = match &self.config.output {
            OutputConfig::Stdout => {
                debug!("JSONL 输出目标为 stdout");
                let stdout = io::stdout();
                let mut sink = JsonlSink::new(stdout.lock());
                self.run_with_sink(&mut sink, &device_id, &client)
            }
            OutputConfig::JsonlFile(path) => {
                debug!(path = %path.display(), "JSONL 输出目标为文件");
                let file = File::create(path)?;
                let mut sink = JsonlSink::new(BufWriter::new(file));
                self.run_with_sink(&mut sink, &device_id, &client)
            }
        };
        drop(capture_guard);
        result
    }

    fn run_with_sink<W>(
        &self,
        sink: &mut JsonlSink<W>,
        device_id: &str,
        source: &impl EventSource,
    ) -> Result<(), AgentError>
    where
        W: io::Write,
    {
        let decoder = Decoder::default();
        let diagnostic_event = decoder.decode(&startup_event())?;
        self.write_startup_events(sink, device_id, &diagnostic_event)?;
        sink.flush()?;
        info!("binder-trace agent 已连接内核事件流");

        let collector = EventCollector::new(source, &decoder, device_id);
        loop {
            collector.collect_batch(sink)?;
        }
    }

    fn write_startup_events<W: io::Write>(
        &self,
        sink: &mut JsonlSink<W>,
        device_id: &str,
        diagnostic_event: &DecodedEvent,
    ) -> Result<(), AgentError> {
        debug!(
            timestamp_ns = diagnostic_event.timestamp_ns,
            "写入程序版本和启动诊断事件"
        );
        sink.write_program_version(
            EventEnvelope {
                device_id,
                timestamp_ns: diagnostic_event.timestamp_ns,
                object: "program.version",
            },
            ProgramVersion {
                program: "binder-trace",
                version: env!("CARGO_PKG_VERSION"),
            },
        )?;
        sink.write_event(
            EventEnvelope {
                device_id,
                timestamp_ns: diagnostic_event.timestamp_ns,
                object: "agent.diagnostic",
            },
            diagnostic_event,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::time::Duration;

    use bt_common::MAX_INLINE_PAYLOAD;

    use super::{Agent, Decoder, EventCollector, EventSource, JsonlSink, startup_event};
    use crate::{AgentConfig, BinderEvent, SocketIpcError};

    struct FakeEventSource {
        events: RefCell<VecDeque<BinderEvent>>,
    }

    impl FakeEventSource {
        fn new(events: impl IntoIterator<Item = BinderEvent>) -> Self {
            Self {
                events: RefCell::new(events.into_iter().collect()),
            }
        }
    }

    impl EventSource for FakeEventSource {
        fn poll_event(&self, _timeout: Duration) -> Result<bool, SocketIpcError> {
            Ok(!self.events.borrow().is_empty())
        }

        fn try_recv_event(&self) -> Result<Option<BinderEvent>, SocketIpcError> {
            Ok(self.events.borrow_mut().pop_front())
        }
    }

    fn binder_event(sequence: u64, reply: bool) -> BinderEvent {
        let mut payload = [0; MAX_INLINE_PAYLOAD];
        payload[..4].copy_from_slice(&[0x01, 0x02, 0x03, 0x04]);
        BinderEvent {
            sequence,
            timestamp_ns: 123,
            kind: 1,
            pid: 45,
            tgid: 44,
            uid: 1000,
            reply: u32::from(reply),
            lost_before: 0,
            transaction_debug_id: 7,
            reply_to_debug_id: 6,
            transaction: 0,
            proc: 0,
            thread: 0,
            extra_buffers_size: 0,
            code: 8,
            flags: 9,
            data_size: 10,
            offsets_size: 11,
            target_handle: 12,
            sender_pid: 13,
            sender_euid: 14,
            payload_len: 4,
            payload_truncated: 0,
            reserved: [0; 7],
            payload,
        }
    }

    #[test]
    fn collector_writes_transaction_and_reply_events() {
        let source = FakeEventSource::new([binder_event(1, false), binder_event(2, true)]);
        let decoder = Decoder::default();
        let collector = EventCollector::new(&source, &decoder, "device-1");
        let mut output = Vec::new();
        let mut sink = JsonlSink::new(&mut output);

        let written = collector
            .collect_batch(&mut sink)
            .expect("socket events should be written");

        let lines = String::from_utf8(output).expect("jsonl should be utf-8");
        let values = lines.lines().collect::<Vec<_>>();
        assert_eq!(written, 2);
        assert_eq!(values.len(), 2);
        assert!(values[0].contains("\"object\":\"binder.transaction\""));
        assert!(values[1].contains("\"object\":\"binder.reply\""));
        assert!(values[0].contains("\"binder_device\":\"unknown\""));
        assert!(values[0].contains("\"process\":{\"pid\":44,\"tid\":45,\"uid\":1000}"));
        assert!(values[0].contains("\"payload_hex\":\"01020304\""));
    }

    #[test]
    fn collector_skips_unknown_socket_event_kind() {
        let mut event = binder_event(1, false);
        event.kind = 99;
        let source = FakeEventSource::new([event]);
        let decoder = Decoder::default();
        let collector = EventCollector::new(&source, &decoder, "device-1");
        let mut output = Vec::new();
        let mut sink = JsonlSink::new(&mut output);

        let written = collector
            .collect_batch(&mut sink)
            .expect("unknown events should be skipped");

        assert_eq!(written, 0);
        assert!(output.is_empty());
    }

    #[test]
    fn writes_startup_events_before_collecting() {
        let agent = Agent::new(AgentConfig::default());
        let decoder = Decoder::default();
        let event = decoder
            .decode(&startup_event())
            .expect("startup event should decode");
        let mut output = Vec::new();
        let mut sink = JsonlSink::new(&mut output);

        agent
            .write_startup_events(&mut sink, "device-1", &event)
            .expect("startup events should be written");

        let lines = String::from_utf8(output).expect("jsonl should be utf-8");
        let records = lines.lines().collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert!(records[0].contains("\"object\":\"program.version\""));
        assert!(records[1].contains("\"object\":\"agent.diagnostic\""));
    }
}
