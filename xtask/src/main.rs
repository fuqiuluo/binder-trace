//! 项目本地开发任务入口。
//!
//! # 职责
//! - 固化 workspace 常用检查命令，避免每个开发者手写不同的 cargo 参数。
//! - 作为调试入口转发 `binder-trace` 参数，不参与业务配置解析。
//!
//! # 不变量
//! - xtask 只编排本地命令；真正的采集、解码和输出逻辑必须留在各业务 crate 中。

use std::env;
use std::fmt;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match Task::from_args(env::args().skip(1)).and_then(Task::run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Task {
    Help,
    Check,
    Fmt,
    Clippy,
    // `run` 后面的参数原样交给 binder-trace，让 CLI 自己维护参数契约。
    RunCli(Vec<String>),
}

impl Task {
    fn from_args(mut args: impl Iterator<Item = String>) -> Result<Self, XtaskError> {
        let Some(command) = args.next() else {
            return Ok(Self::Help);
        };

        match command.as_str() {
            "help" | "-h" | "--help" => Ok(Self::Help),
            "check" => Ok(Self::Check),
            "fmt" => Ok(Self::Fmt),
            "clippy" => Ok(Self::Clippy),
            "run" => Ok(Self::RunCli(args.collect())),
            other => Err(XtaskError::UnknownTask(other.to_owned())),
        }
    }

    fn run(self) -> Result<(), XtaskError> {
        match self {
            Self::Help => {
                print_help();
                Ok(())
            }
            Self::Check => run_cargo(["check", "--workspace"]),
            Self::Fmt => run_cargo(["fmt", "--all"]),
            Self::Clippy => run_cargo(["clippy", "--workspace", "--all-targets"]),
            Self::RunCli(args) => run_binder_trace(args),
        }
    }
}

#[derive(Debug)]
enum XtaskError {
    CargoFailed { command: String, code: Option<i32> },
    Io(std::io::Error),
    UnknownTask(String),
}

impl fmt::Display for XtaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CargoFailed { command, code } => {
                write!(f, "cargo command failed: `{command}`")?;
                if let Some(code) = code {
                    write!(f, " with exit code {code}")?;
                }
                Ok(())
            }
            Self::Io(error) => write!(f, "failed to run cargo: {error}"),
            Self::UnknownTask(task) => write!(f, "unknown xtask command: `{task}`"),
        }
    }
}

impl std::error::Error for XtaskError {}

impl From<std::io::Error> for XtaskError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn run_cargo<const N: usize>(args: [&'static str; N]) -> Result<(), XtaskError> {
    run_command(CommandSpec::new(args.join(" "), args))
}

fn run_binder_trace(args: Vec<String>) -> Result<(), XtaskError> {
    let mut command = Command::new("cargo");
    command.args(["run", "-p", "bt-cli", "--bin", "binder-trace", "--"]);
    command.args(&args);
    let status = command.status()?;

    if status.success() {
        Ok(())
    } else {
        Err(XtaskError::CargoFailed {
            command: format!("run -p bt-cli --bin binder-trace -- {}", args.join(" ")),
            code: status.code(),
        })
    }
}

fn run_command<const N: usize>(spec: CommandSpec<N>) -> Result<(), XtaskError> {
    let status = Command::new("cargo").args(spec.args).status()?;

    if status.success() {
        Ok(())
    } else {
        Err(XtaskError::CargoFailed {
            command: spec.display,
            code: status.code(),
        })
    }
}

struct CommandSpec<const N: usize> {
    // 失败信息里展示的是用户能直接复制运行的 cargo 子命令，而不是 Debug 格式参数数组。
    display: String,
    args: [&'static str; N],
}

impl<const N: usize> CommandSpec<N> {
    fn new(display: String, args: [&'static str; N]) -> Self {
        Self { display, args }
    }
}

fn print_help() {
    println!(
        "\
binder-trace xtask

用法:
  cargo run -p xtask -- <command>

命令:
  check       运行 workspace 检查
  fmt         格式化 workspace
  clippy      对所有 target 运行 clippy
  run [args]  带参数运行 binder-trace
"
    );
}
