//! agent 运行编排。
//!
//! # 职责
//! - 固定高层运行顺序：启动诊断事件、后续采集入口和最终 flush。
//! - 让 CLI 只负责参数转换，不直接接触输出格式和设备身份解析。

use std::fs::File;
use std::io::{self, BufWriter};

use bt_decoder::{DecodedEvent, Decoder};
use bt_storage::{EventEnvelope, JsonlSink, ProgramVersion};
use tracing::{debug, info};

use crate::config::{AgentConfig, OutputConfig};
use crate::device::resolve_device_id;
use crate::diagnostic::startup_event;
use crate::error::AgentError;

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

    /// 执行启动检查并运行配置指定的采集器。
    ///
    /// # 错误
    /// 当内核能力不满足、启动事件解码失败、输出 I/O 失败或事件写入失败时返回
    /// [`AgentError`]。
    pub fn run(&self) -> Result<(), AgentError> {
        info!("启动 binder-trace agent");

        let decoder = Decoder::default();
        let event = decoder.decode(&startup_event())?;
        let device_id = resolve_device_id(self.config.device_id.as_deref());
        debug!(device_id, "设备标识解析完成");

        match &self.config.output {
            OutputConfig::Stdout => {
                debug!("JSONL 输出目标为 stdout");
                let stdout = io::stdout();
                let mut sink = JsonlSink::new(stdout.lock());
                self.run_with_sink(&mut sink, &device_id, &event)
            }
            OutputConfig::JsonlFile(path) => {
                debug!(path = %path.display(), "JSONL 输出目标为文件");
                let file = File::create(path)?;
                let mut sink = JsonlSink::new(BufWriter::new(file));
                self.run_with_sink(&mut sink, &device_id, &event)
            }
        }
    }

    fn run_with_sink<W: io::Write>(
        &self,
        sink: &mut JsonlSink<W>,
        device_id: &str,
        diagnostic_event: &DecodedEvent,
    ) -> Result<(), AgentError> {
        self.write_startup_events(sink, device_id, diagnostic_event)?;
        sink.flush()?;
        info!("binder-trace agent 已完成输出 flush");
        Ok(())
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
