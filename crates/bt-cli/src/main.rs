//! `binder-trace` 命令行入口。
//!
//! # 职责
//! - 把命令行参数转换成 `bt-agent` 的运行配置。
//! - 保持 CLI 层只做参数解析和错误展示，采集能力判断和事件输出由 `bt-agent` 负责。
//!
//! # 不变量
//! - 新增采集行为时优先扩展 `AgentConfig`，这里只暴露必要的用户入口。

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, ExitCode};
use std::time::Duration;

use bt_agent::{
    Agent, AgentConfig, AgentError, CaptureConfig, CaptureHistory, DriverFeature, OutputConfig,
    SocketIpcClient, SocketIpcError,
};
use bt_decoder::{AndroidPlatformMethodsPathError, set_android_platform_methods_tsv_path};
use bt_mcp::{McpServerConfig, McpServerError};
use bt_webui::{WebuiError, WebuiEventsConfig, WebuiServerConfig};
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod startup_marker;
mod tui;

fn main() -> ExitCode {
    init_tracing();

    match Cli::parse().run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "binder-trace",
    version,
    about = "Android Binder trace 采集工具"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(
        short,
        long,
        value_name = "path",
        help = "将 JSONL 输出写入文件，默认写到 stdout"
    )]
    output: Option<PathBuf>,

    #[arg(long, value_name = "id", help = "覆盖消息信封中的 device_id")]
    device_id: Option<String>,

    #[arg(
        long,
        global = true,
        value_name = "path",
        help = "平台 Binder method TSV 释放/覆盖路径，也可用 BINDER_TRACE_ANDROID_PLATFORM_METHODS_TSV"
    )]
    platform_methods_tsv: Option<PathBuf>,
}

impl Cli {
    fn run(self) -> Result<(), CliError> {
        let Self {
            command,
            output,
            device_id,
            platform_methods_tsv,
        } = self;

        if let Some(path) = platform_methods_tsv {
            set_android_platform_methods_tsv_path(path)?;
        }

        if let Err(error) = startup_marker::write_default() {
            eprintln!(
                "warning: failed to write startup marker {}: {error}",
                startup_marker::DEFAULT_MARKER_PATH
            );
        }

        match command {
            Some(Command::Ipc(args)) => args.command.run(),
            Some(Command::Tui(args)) => args.run(),
            Some(Command::Webui(args)) => args.run(),
            Some(Command::Mcp(args)) => args.run(),
            None => {
                let config = Self::agent_config(output, device_id);
                Agent::new(config).run().map_err(CliError::Agent)
            }
        }
    }

    fn agent_config(output: Option<PathBuf>, device_id: Option<String>) -> AgentConfig {
        let mut config = AgentConfig::default();

        if let Some(path) = output {
            config.output = OutputConfig::JsonlFile(path);
        }
        config.device_id = device_id;

        config
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "通过自定义 socket 协议族控制内核模块")]
    Ipc(IpcCommand),
    #[command(about = "启动实时 Binder transaction 跟踪 TUI")]
    Tui(TuiCommand),
    #[command(about = "启动内嵌 Binder Trace WebUI")]
    Webui(WebuiCommand),
    #[command(about = "启动在线 Binder Trace MCP Streamable HTTP 服务")]
    Mcp(McpCommand),
}

#[derive(Debug, Args)]
struct McpCommand {
    #[arg(
        long,
        default_value = "127.0.0.1:5174",
        value_name = "addr",
        help = "MCP HTTP 监听地址"
    )]
    listen: SocketAddr,

    #[arg(
        long,
        default_value_t = 65536,
        help = "btcap 历史文件初始事件容量，满后自动扩容"
    )]
    rows: usize,

    #[arg(
        long,
        value_name = "path",
        help = "MCP btcap 历史文件路径，默认使用 /data/local/tmp/binder-trace/mcp-events.btcap"
    )]
    history_path: Option<PathBuf>,

    #[arg(
        long,
        default_value_t = CaptureHistory::DEFAULT_MAX_FILE_BYTES,
        value_name = "bytes",
        help = "MCP btcap 历史文件最大字节数，默认 8GiB"
    )]
    max_history_bytes: u64,

    #[arg(long, help = "允许 MCP tool 修改内核捕获配置")]
    allow_control: bool,

    #[arg(long, help = "MCP 服务启动时立即开启 Binder transaction 捕获")]
    enable: bool,

    #[arg(long, value_name = "tgid", help = "开启捕获时只捕获指定进程组")]
    tgid: Option<i32>,

    #[arg(long, value_name = "pid", help = "开启捕获时只捕获指定线程")]
    pid: Option<i32>,

    #[arg(long, value_name = "uid", help = "开启捕获时只捕获指定 uid")]
    uid: Option<u32>,

    #[arg(
        long,
        value_name = "bytes",
        help = "开启捕获时只捕获大于等于该 data size 的事件"
    )]
    min_size: Option<u64>,

    #[arg(
        long,
        value_name = "bytes",
        help = "开启捕获时只捕获小于等于该 data size 的事件"
    )]
    max_size: Option<u64>,

    #[arg(
        long,
        value_name = "sdk",
        help = "Android SDK 版本；未指定时尝试读取 ro.build.version.sdk"
    )]
    android_sdk: Option<u16>,
}

impl McpCommand {
    fn run(self) -> Result<(), CliError> {
        let capture_config = self.capture_config();
        let android_sdk = self.android_sdk.or_else(detect_android_sdk);
        let config = McpServerConfig {
            listen: self.listen,
            initial_events: self.rows,
            history_path: self.history_path,
            max_history_bytes: self.max_history_bytes,
            allow_control: self.allow_control,
            auto_enable: self.enable,
            capture_config,
            android_sdk,
        };
        eprintln!("Binder Trace MCP listening on http://{}/mcp", config.listen);
        bt_mcp::serve_http_blocking(config).map_err(CliError::Mcp)
    }

    fn capture_config(&self) -> CaptureConfig {
        let mut config = CaptureConfig::binder_transaction_enabled();

        if let Some(tgid) = self.tgid {
            config.tgid = tgid;
        }
        if let Some(pid) = self.pid {
            config.pid = pid;
        }
        if let Some(uid) = self.uid {
            config.uid = uid;
            config.uid_enabled = 1;
        }
        if let Some(min_size) = self.min_size {
            config.min_size = min_size;
        }
        if let Some(max_size) = self.max_size {
            config.max_size = max_size;
        }

        config
    }
}

#[derive(Debug, Args)]
struct WebuiCommand {
    #[arg(
        long,
        default_value = "127.0.0.1:5173",
        value_name = "addr",
        help = "WebUI 监听地址"
    )]
    listen: SocketAddr,

    #[arg(
        long,
        default_value_t = 65536,
        help = "WebUI btcap 历史文件初始事件容量，满后自动扩容"
    )]
    rows: usize,

    #[arg(
        long,
        value_name = "path",
        help = "WebUI btcap 历史文件路径，默认使用 /data/local/tmp/binder-trace/webui-events.btcap"
    )]
    history_path: Option<PathBuf>,

    #[arg(
        long,
        default_value_t = CaptureHistory::DEFAULT_MAX_FILE_BYTES,
        value_name = "bytes",
        help = "WebUI btcap 历史文件最大字节数，默认 8GiB"
    )]
    max_history_bytes: u64,

    #[arg(long, value_name = "tgid", help = "只捕获指定进程组")]
    tgid: Option<i32>,

    #[arg(long, value_name = "pid", help = "只捕获指定线程")]
    pid: Option<i32>,

    #[arg(long, value_name = "uid", help = "只捕获指定 uid")]
    uid: Option<u32>,

    #[arg(long, help = "只读事件流，不自动更新内核捕获配置")]
    no_enable: bool,

    #[arg(
        long,
        value_name = "sdk",
        help = "Android SDK 版本；未指定时尝试读取 ro.build.version.sdk"
    )]
    android_sdk: Option<u16>,
}

impl WebuiCommand {
    fn run(self) -> Result<(), CliError> {
        let capture_config = self.capture_config();
        let config = WebuiServerConfig {
            listen: self.listen,
            events: WebuiEventsConfig {
                enabled: true,
                capture_config: (!self.no_enable).then_some(capture_config),
                android_sdk: self.android_sdk.or_else(detect_android_sdk),
                max_events: self.rows,
                history_path: self.history_path,
                max_history_bytes: self.max_history_bytes,
                ..WebuiEventsConfig::default()
            },
        };
        println!("Binder Trace WebUI listening on {config}");
        bt_webui::serve_blocking(config).map_err(CliError::Webui)
    }

    fn capture_config(&self) -> CaptureConfig {
        let mut config = CaptureConfig::binder_transaction_enabled();

        if let Some(tgid) = self.tgid {
            config.tgid = tgid;
        }
        if let Some(pid) = self.pid {
            config.pid = pid;
        }
        if let Some(uid) = self.uid {
            config.uid = uid;
            config.uid_enabled = 1;
        }

        config
    }
}

#[derive(Debug, Args)]
struct TuiCommand {
    #[arg(long, default_value_t = 512, help = "内存中保留的最近事件行数")]
    rows: usize,

    #[arg(long, default_value_t = 250, help = "界面刷新间隔，单位毫秒")]
    refresh_ms: u64,

    #[arg(
        long,
        value_name = "path",
        help = "二进制事件历史文件路径，默认自动选择"
    )]
    history_path: Option<PathBuf>,

    #[arg(long, value_name = "tgid", help = "只捕获指定进程组")]
    tgid: Option<i32>,

    #[arg(long, value_name = "pid", help = "只捕获指定线程")]
    pid: Option<i32>,

    #[arg(long, value_name = "uid", help = "只捕获指定 uid")]
    uid: Option<u32>,

    #[arg(long, help = "只读事件流，不自动更新内核捕获配置")]
    no_enable: bool,

    #[arg(
        long,
        value_name = "sdk",
        help = "Android SDK 版本；未指定时尝试读取 ro.build.version.sdk"
    )]
    android_sdk: Option<u16>,
}

impl TuiCommand {
    fn run(self) -> Result<(), CliError> {
        let client = SocketIpcClient::connect()?;
        let feature = client.get_feature()?;
        if !feature.has_event_stream() {
            return Err(CliError::EventStreamUnsupported);
        }

        let capture_config = self.capture_config();

        if !self.no_enable {
            client.set_config(capture_config)?;
            client.clear_stats()?;
        }

        tui::run_tui(
            &client,
            client.family(),
            tui::TuiConfig {
                rows: self.rows,
                refresh: Duration::from_millis(self.refresh_ms),
                capture_config: (!self.no_enable).then_some(capture_config),
                android_sdk: self.android_sdk.or_else(detect_android_sdk),
                history_path: self.history_path,
            },
        )
        .map_err(CliError::Tui)
    }

    fn capture_config(&self) -> CaptureConfig {
        let mut config = CaptureConfig::binder_transaction_enabled();

        if let Some(tgid) = self.tgid {
            config.tgid = tgid;
        }
        if let Some(pid) = self.pid {
            config.pid = pid;
        }
        if let Some(uid) = self.uid {
            config.uid = uid;
            config.uid_enabled = 1;
        }

        config
    }
}

#[derive(Debug, Args)]
#[command(arg_required_else_help = true)]
struct IpcCommand {
    #[command(subcommand)]
    command: IpcAction,
}

#[derive(Debug, Subcommand)]
enum IpcAction {
    #[command(about = "探测并打印内核模块控制协议族特征")]
    Feature,
    #[command(about = "打印当前捕获配置")]
    Config,
    #[command(about = "开启默认捕获配置")]
    Enable,
    #[command(about = "关闭捕获")]
    Disable,
    #[command(about = "打印内核控制面统计")]
    Stats,
    #[command(name = "clear-stats", about = "清空内核控制面统计")]
    ClearStats,
}

impl IpcAction {
    fn run(self) -> Result<(), CliError> {
        match self {
            Self::Feature => {
                let client = SocketIpcClient::connect()?;
                print_feature(client.family(), client.get_feature()?);
                Ok(())
            }
            Self::Config => {
                let client = SocketIpcClient::connect()?;
                print_config(client.get_config()?);
                Ok(())
            }
            Self::Enable => {
                let client = SocketIpcClient::connect()?;
                client.set_config(CaptureConfig::enabled())?;
                print_config(client.get_config()?);
                Ok(())
            }
            Self::Disable => {
                let client = SocketIpcClient::connect()?;
                client.set_config(CaptureConfig::disabled())?;
                print_config(client.get_config()?);
                Ok(())
            }
            Self::Stats => {
                let client = SocketIpcClient::connect()?;
                let stats = client.get_stats()?;
                println!("ioctl_hits={}", stats.ioctl_hits);
                println!("copy_to_user_hits={}", stats.copy_to_user_hits);
                println!("transaction_hits={}", stats.transaction_hits);
                println!("captured={}", stats.captured);
                println!("filtered={}", stats.filtered);
                Ok(())
            }
            Self::ClearStats => {
                let client = SocketIpcClient::connect()?;
                client.clear_stats()?;
                Ok(())
            }
        }
    }
}

#[derive(Debug)]
enum CliError {
    Agent(AgentError),
    SocketIpc(SocketIpcError),
    Io(io::Error),
    Tui(tui::TuiError),
    Webui(WebuiError),
    Mcp(McpServerError),
    PlatformMethods(AndroidPlatformMethodsPathError),
    EventStreamUnsupported,
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent(error) => write!(f, "{error}"),
            Self::SocketIpc(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Tui(error) => write!(f, "{error}"),
            Self::Webui(error) => write!(f, "{error}"),
            Self::Mcp(error) => write!(f, "{error}"),
            Self::PlatformMethods(error) => write!(f, "{error}"),
            Self::EventStreamUnsupported => {
                write!(
                    f,
                    "当前内核模块不支持 socket 事件流，请重新加载新版 bt-kmod"
                )
            }
        }
    }
}

impl std::error::Error for CliError {}

impl From<SocketIpcError> for CliError {
    fn from(error: SocketIpcError) -> Self {
        Self::SocketIpc(error)
    }
}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<AndroidPlatformMethodsPathError> for CliError {
    fn from(error: AndroidPlatformMethodsPathError) -> Self {
        Self::PlatformMethods(error)
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .compact()
        .init();
}

fn detect_android_sdk() -> Option<u16> {
    let output = ProcessCommand::new("getprop")
        .arg("ro.build.version.sdk")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn print_feature(family: i32, feature: DriverFeature) {
    println!("family={family}");
    println!("name={}", c_string_lossy(&feature.name));
    println!("magic=0x{:016x}", feature.magic);
    println!("abi_version={}", feature.abi_version);
    println!("feature_flags=0x{:08x}", feature.feature_flags);
}

fn print_config(config: CaptureConfig) {
    println!("enabled={}", config.enabled);
    println!("point_mask=0x{:08x}", config.point_mask);
    println!("tgid={}", config.tgid);
    println!("pid={}", config.pid);
    println!("uid={}", config.uid);
    println!("uid_enabled={}", config.uid_enabled);
    println!("ioctl_cmd=0x{:08x}", config.ioctl_cmd);
    println!("ioctl_cmd_enabled={}", config.ioctl_cmd_enabled);
    println!("min_size={}", config.min_size);
    println!("max_size={}", config.max_size);
}

fn c_string_lossy(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, IpcAction};
    use clap::{Parser, error::ErrorKind};

    #[test]
    fn parses_output_and_device_id() {
        let cli = Cli::try_parse_from([
            "binder-trace",
            "--output",
            "trace.jsonl",
            "--device-id",
            "device-1",
        ])
        .expect("参数应可解析");
        let config = Cli::agent_config(cli.output, cli.device_id);

        assert_eq!(config.device_id.as_deref(), Some("device-1"));
        assert!(matches!(
            config.output,
            bt_agent::OutputConfig::JsonlFile(_)
        ));
    }

    #[test]
    fn parses_platform_methods_tsv_path() {
        let cli = Cli::try_parse_from([
            "binder-trace",
            "--platform-methods-tsv",
            "/data/local/tmp/custom-methods.tsv",
            "tui",
        ])
        .expect("method 表路径参数应可解析");

        assert_eq!(
            cli.platform_methods_tsv.as_deref(),
            Some(std::path::Path::new("/data/local/tmp/custom-methods.tsv"))
        );
    }

    #[test]
    fn rejects_unknown_arguments() {
        let error = Cli::try_parse_from(["binder-trace", "--bad-option"])
            .expect_err("未知参数应被 clap 拒绝");

        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn parses_ipc_feature_command() {
        let cli =
            Cli::try_parse_from(["binder-trace", "ipc", "feature"]).expect("ipc 子命令应可解析");

        let Some(Command::Ipc(ipc)) = cli.command else {
            panic!("expected ipc command");
        };
        assert!(matches!(ipc.command, IpcAction::Feature));
    }

    #[test]
    fn parses_tui_filters() {
        let cli = Cli::try_parse_from([
            "binder-trace",
            "tui",
            "--rows",
            "8",
            "--history-path",
            "/data/local/tmp/custom.btcap",
            "--tgid",
            "123",
            "--uid",
            "2000",
        ])
        .expect("tui 子命令应可解析");

        let Some(Command::Tui(tui)) = cli.command else {
            panic!("expected tui command");
        };
        assert_eq!(tui.rows, 8);
        assert_eq!(
            tui.history_path.as_deref(),
            Some(std::path::Path::new("/data/local/tmp/custom.btcap"))
        );
        assert_eq!(tui.tgid, Some(123));
        assert_eq!(tui.uid, Some(2000));
    }

    #[test]
    fn parses_webui_listen_addr() {
        let cli = Cli::try_parse_from([
            "binder-trace",
            "webui",
            "--listen",
            "127.0.0.1:9080",
            "--history-path",
            "/data/local/tmp/binder-trace/test-webui.btcap",
            "--max-history-bytes",
            "1048576",
            "--rows",
            "128",
        ])
        .expect("webui 子命令应可解析");

        let Some(Command::Webui(webui)) = cli.command else {
            panic!("expected webui command");
        };
        assert_eq!(
            webui.listen,
            std::net::SocketAddr::from(([127, 0, 0, 1], 9080))
        );
        assert_eq!(
            webui.history_path.as_deref(),
            Some(std::path::Path::new(
                "/data/local/tmp/binder-trace/test-webui.btcap"
            ))
        );
        assert_eq!(webui.rows, 128);
        assert_eq!(webui.max_history_bytes, 1_048_576);
    }

    #[test]
    fn parses_mcp_control_filters() {
        let cli = Cli::try_parse_from([
            "binder-trace",
            "mcp",
            "--listen",
            "127.0.0.1:9000",
            "--history-path",
            "/data/local/tmp/binder-trace/test-mcp.btcap",
            "--max-history-bytes",
            "1048576",
            "--rows",
            "128",
            "--allow-control",
            "--enable",
            "--tgid",
            "123",
            "--uid",
            "2000",
            "--min-size",
            "16",
            "--max-size",
            "4096",
        ])
        .expect("mcp 子命令应可解析");

        let Some(Command::Mcp(mcp)) = cli.command else {
            panic!("expected mcp command");
        };
        let config = mcp.capture_config();
        assert_eq!(
            mcp.listen,
            std::net::SocketAddr::from(([127, 0, 0, 1], 9000))
        );
        assert_eq!(
            mcp.history_path.as_deref(),
            Some(std::path::Path::new(
                "/data/local/tmp/binder-trace/test-mcp.btcap"
            ))
        );
        assert_eq!(mcp.rows, 128);
        assert_eq!(mcp.max_history_bytes, 1_048_576);
        assert!(mcp.allow_control);
        assert!(mcp.enable);
        assert_eq!(config.tgid, 123);
        assert_eq!(config.uid, 2000);
        assert_eq!(config.uid_enabled, 1);
        assert_eq!(config.min_size, 16);
        assert_eq!(config.max_size, 4096);
    }

    #[test]
    fn shows_ipc_help_without_subcommand() {
        let error =
            Cli::try_parse_from(["binder-trace", "ipc"]).expect_err("ipc 缺少子命令时应显示帮助");

        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }
}
