//! 用户态 agent 的公开配置类型。
//!
//! # 职责
//! - 表达 agent 启动所需的纯数据配置。
//! - 让 CLI、测试、配置文件或未来 service API 可以构造同一套配置，而不引入运行时初始化逻辑。

use std::path::PathBuf;

/// [`crate::Agent`] 的完整运行配置。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AgentConfig {
    /// 启动事件和采集事件的 JSONL 输出目的地。
    pub output: OutputConfig,
    /// 事件信封中的设备标识覆盖值；为空时由 agent 自动解析。
    pub device_id: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            output: OutputConfig::Stdout,
            device_id: None,
        }
    }
}

/// agent 事件的 JSONL 输出目的地。
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum OutputConfig {
    /// 写入标准输出，适合 adb shell 或管道调试。
    Stdout,
    /// 写入指定文件路径；文件已存在时会被覆盖。
    JsonlFile(PathBuf),
}
