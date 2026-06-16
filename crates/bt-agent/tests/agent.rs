//! `bt-agent` 公共 API 的集成测试。
//!
//! 这里不访问 crate 私有模块，只验证外部调用者能观察到的启动事件输出契约。

use std::fs;

use bt_agent::{Agent, AgentConfig, OutputConfig};

#[test]
fn writes_startup_events() {
    let output_path = temp_path("startup-output");
    let agent = Agent::new(AgentConfig {
        output: OutputConfig::JsonlFile(output_path.clone()),
        ..AgentConfig::default()
    });

    agent.run().expect("agent 应输出启动事件");

    let output = fs::read_to_string(&output_path).expect("agent output should be readable");
    assert!(output.contains("\"object\":\"program.version\""));
    assert!(output.contains("\"object\":\"agent.diagnostic\""));
    fs::remove_file(output_path).expect("temp output should be removable");
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("binder-trace-{name}-{}", std::process::id()))
}
