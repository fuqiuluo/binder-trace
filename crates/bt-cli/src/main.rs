use std::env;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

use bt_agent::{Agent, AgentConfig, AgentError, KernelConfigSource, OutputConfig};

fn main() -> ExitCode {
    match Command::from_args(env::args().skip(1)).and_then(Command::run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Help,
    Run(AgentConfig),
}

impl Command {
    fn from_args(mut args: impl Iterator<Item = String>) -> Result<Self, CliError> {
        let mut config = AgentConfig::default();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => return Ok(Self::Help),
                "-o" | "--output" => {
                    let Some(path) = args.next() else {
                        return Err(CliError::MissingValue(arg));
                    };
                    config.output = OutputConfig::JsonlFile(PathBuf::from(path));
                }
                "--kernel-config-gz" => {
                    let Some(path) = args.next() else {
                        return Err(CliError::MissingValue(arg));
                    };
                    config.kernel_config_source = KernelConfigSource::GzipFile(PathBuf::from(path));
                }
                "--kernel-config-text" => {
                    let Some(path) = args.next() else {
                        return Err(CliError::MissingValue(arg));
                    };
                    config.kernel_config_source = KernelConfigSource::TextFile(PathBuf::from(path));
                }
                "--device-id" => {
                    let Some(device_id) = args.next() else {
                        return Err(CliError::MissingValue(arg));
                    };
                    config.device_id = Some(device_id);
                }
                other => return Err(CliError::UnknownArgument(other.to_owned())),
            }
        }

        Ok(Self::Run(config))
    }

    fn run(self) -> Result<(), CliError> {
        match self {
            Self::Help => {
                print_help();
                Ok(())
            }
            Self::Run(config) => Agent::new(config).run().map_err(CliError::Agent),
        }
    }
}

#[derive(Debug)]
enum CliError {
    Agent(AgentError),
    MissingValue(String),
    UnknownArgument(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent(error) => write!(f, "{error}"),
            Self::MissingValue(flag) => write!(f, "missing value for `{flag}`"),
            Self::UnknownArgument(argument) => write!(f, "unknown argument: `{argument}`"),
        }
    }
}

impl std::error::Error for CliError {}

fn print_help() {
    println!(
        "\
binder-trace

Usage:
  binder-trace [--output <path>] [--kernel-config-gz <path>] [--device-id <id>]

Options:
  -o, --output <path>       Write JSONL output to a file instead of stdout
      --kernel-config-gz    Read gzip kernel config from a custom path
      --kernel-config-text  Read plain text kernel config from a custom path
      --device-id <id>      Override emitted message envelope device_id
  -h, --help                Print help
"
    );
}
