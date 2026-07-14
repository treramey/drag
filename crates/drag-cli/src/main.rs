mod api;
mod app;
mod cli;
mod config;

use std::error::Error as StdError;
use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::{AliasCommand, Cli, Command, OutputMode, TrackerCommand};
use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;

use crate::app::{default_timezone, App};

const EXIT_FAILURE: u8 = 1;
const EXIT_USAGE: u8 = 2;

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Core(#[from] drag::Error),
    #[error("{0}")]
    InvalidInput(String),
    #[error("drag is not configured: {0}")]
    NotConfigured(String),
    #[error("configuration error: {message}")]
    Config {
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
    #[error("API request failed: {0}")]
    Api(String),
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("generated completion output was not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

impl CliError {
    const fn code(&self) -> &'static str {
        match self {
            Self::Core(error) => error.code(),
            Self::InvalidInput(_) => "invalid_input",
            Self::NotConfigured(_) => "not_configured",
            Self::Config { .. } => "config_error",
            Self::Api(_) => "api_error",
            Self::Http(_) => "http_error",
            Self::Url(_) => "invalid_url",
            Self::Json(_) => "invalid_json",
            Self::Io(_) => "io_error",
            Self::Utf8(_) => "encoding_error",
        }
    }

    const fn exit_code(&self) -> u8 {
        match self {
            Self::Core(_)
            | Self::InvalidInput(_)
            | Self::NotConfigured(_)
            | Self::Json(_)
            | Self::Url(_) => EXIT_USAGE,
            Self::Config { .. } | Self::Api(_) | Self::Http(_) | Self::Io(_) | Self::Utf8(_) => {
                EXIT_FAILURE
            }
        }
    }
}

pub struct Rendered {
    data: Value,
    human: String,
}

impl Rendered {
    pub fn new(data: Value, human: String) -> Self {
        Self { data, human }
    }
}

#[derive(Serialize)]
struct Success<T> {
    ok: bool,
    data: T,
}

#[derive(Serialize)]
struct Failure<'a> {
    ok: bool,
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let requested_mode = output_mode_from_args(&args);
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => return handle_parse_error(error, requested_mode),
    };
    let mode = resolve_mode(cli.output);
    match run(cli).await {
        Ok(result) => match emit_result(result, mode) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                emit_error(&error, mode);
                ExitCode::from(error.exit_code())
            }
        },
        Err(error) => {
            emit_error(&error, mode);
            ExitCode::from(error.exit_code())
        }
    }
}

async fn run(cli: Cli) -> Result<Rendered, CliError> {
    let timezone = default_timezone(cli.timezone.as_deref())?;
    let path = cli.config.unwrap_or(config::config_path()?);
    let app = App::new(path.clone(), timezone, cli.debug);
    match cli.command {
        Command::Log(args) => app.log(args).await,
        Command::List(args) => app.list(args).await,
        Command::Delete(args) => app.delete(args).await,
        Command::Setup(args) => app.setup(args).await,
        Command::Alias { command } => match command {
            AliasCommand::Set(args) => app.alias_set(args),
            AliasCommand::List => app.alias_list(),
            AliasCommand::Delete(args) => app.alias_delete(args),
        },
        Command::Tracker { command } => match command {
            TrackerCommand::Start(args) => app.tracker_start(args).await,
            TrackerCommand::Pause(args) => app.tracker_pause(args),
            TrackerCommand::Resume(args) => app.tracker_resume(args),
            TrackerCommand::Stop(args) => app.tracker_stop(args).await,
            TrackerCommand::Delete(args) => app.tracker_delete(args),
            TrackerCommand::List => app.tracker_list(),
        },
        Command::Start(args) => app.tracker_start(args).await,
        Command::Pause(args) => app.tracker_pause(args),
        Command::Resume(args) => app.tracker_resume(args),
        Command::Stop(args) => app.tracker_stop(args).await,
        Command::LegacyAliasSet(args) => app.alias_set(args),
        Command::LegacyAliasList => app.alias_list(),
        Command::LegacyAliasDelete(args) => app.alias_delete(args),
        Command::LegacyTrackerList => app.tracker_list(),
        Command::LegacyTrackerDelete(args) => app.tracker_delete(args),
        Command::Completions { shell } => {
            let shell = shell.unwrap_or_else(detect_shell);
            let mut bytes = Vec::new();
            generate(shell, &mut Cli::command(), "drag", &mut bytes);
            let script = String::from_utf8(bytes)?;
            Ok(Rendered::new(
                json!({"shell": shell.to_string(), "script": script}),
                script,
            ))
        }
        Command::Doctor => {
            let config = config::Config::load(&path)?;
            let report = json!({
                "name": "drag",
                "version": env!("CARGO_PKG_VERSION"),
                "configPath": path,
                "configured": {
                    "tempoToken": config.tempo_token.is_some() || std::env::var_os("TEMPO_TOKEN").is_some(),
                    "accountId": config.account_id.is_some() || std::env::var_os("TEMPO_ACCOUNT_ID").is_some(),
                    "atlassianEmail": config.atlassian_user_email.is_some() || std::env::var_os("ATLASSIAN_EMAIL").is_some(),
                    "atlassianToken": config.atlassian_token.is_some() || std::env::var_os("ATLASSIAN_TOKEN").is_some(),
                    "atlassianHost": config.hostname.is_some() || std::env::var_os("ATLASSIAN_HOST").is_some()
                },
                "aliases": config.aliases.len(),
                "trackers": config.trackers.len(),
                "timezone": timezone.name(),
                "target": {"architecture": std::env::consts::ARCH, "operatingSystem": std::env::consts::OS}
            });
            Ok(Rendered::new(
                report,
                format!(
                    "drag {}\nconfig: {}\ntimezone: {}\naliases: {}\ntrackers: {}",
                    env!("CARGO_PKG_VERSION"),
                    path.display(),
                    timezone.name(),
                    config.aliases.len(),
                    config.trackers.len()
                ),
            ))
        }
        Command::Schema => Ok(schema()),
    }
}

fn schema() -> Rendered {
    let data = json!({
        "schemaVersion": 1,
        "name": "drag",
        "output": {"modes": ["auto", "human", "json"], "errorsOn": "stderr"},
        "commands": {
            "setup": {
                "sideEffects": true,
                "fromEnv": true,
                "fromEnvRequired": ["ATLASSIAN_HOST", "ATLASSIAN_EMAIL", "ATLASSIAN_TOKEN", "TEMPO_TOKEN"],
                "verification": {"jira": "read-only", "tempo": "read-only"},
                "derivesAccountId": true
            },
            "log": {"aliases": ["l"], "rawJson": true, "dryRun": true},
            "list": {"aliases": ["ls"]},
            "delete": {"aliases": ["d"], "dryRun": true},
            "alias": {"subcommands": ["set", "list", "delete"]},
            "tracker": {"subcommands": ["start", "pause", "resume", "stop", "delete", "list"], "stopDryRun": true},
            "completions": {}, "doctor": {}, "schema": {}
        },
        "dateSyntax": ["YYYY-MM-DD", "y", "yesterday", "t+N", "t-N", "today+N", "today-N"],
        "durationSyntax": ["15m", "1h", "1h15m", "11-12:30", "23:30-00:30"],
        "environment": ["DRAG_CONFIG", "TEMPO_TOKEN", "TEMPO_ACCOUNT_ID", "ATLASSIAN_EMAIL", "ATLASSIAN_TOKEN", "ATLASSIAN_HOST"],
        "exitCodes": {"0": "success", "1": "runtime failure", "2": "usage or invalid input"}
    });
    Rendered::new(
        data,
        "Use `drag --output json schema` for the full CLI contract.".to_owned(),
    )
}

fn resolve_mode(mode: OutputMode) -> OutputMode {
    match mode {
        OutputMode::Auto if io::stdout().is_terminal() => OutputMode::Human,
        OutputMode::Auto => OutputMode::Json,
        mode => mode,
    }
}

fn emit_result(result: Rendered, mode: OutputMode) -> Result<(), CliError> {
    match mode {
        OutputMode::Human => println!("{}", result.human),
        OutputMode::Json | OutputMode::Auto => write_json(
            &mut io::stdout().lock(),
            &Success {
                ok: true,
                data: result.data,
            },
        )?,
    }
    Ok(())
}

fn emit_error(error: &CliError, mode: OutputMode) {
    if mode == OutputMode::Json {
        let message = error.to_string();
        let body = Failure {
            ok: false,
            error: ErrorBody {
                code: error.code(),
                message: &message,
            },
        };
        let _ = write_json(&mut io::stderr().lock(), &body);
    } else {
        eprintln!("error: {error}");
    }
}

fn write_json(writer: &mut impl Write, value: &impl Serialize) -> Result<(), CliError> {
    serde_json::to_writer_pretty(&mut *writer, value)?;
    writeln!(writer)?;
    Ok(())
}

fn handle_parse_error(error: clap::Error, requested: OutputMode) -> ExitCode {
    use clap::error::ErrorKind::{DisplayHelp, DisplayVersion};
    if matches!(error.kind(), DisplayHelp | DisplayVersion) {
        let _ = error.print();
        return ExitCode::SUCCESS;
    }
    if resolve_mode(requested) == OutputMode::Json {
        let message = error.to_string();
        let body = Failure {
            ok: false,
            error: ErrorBody {
                code: "usage",
                message: message.trim(),
            },
        };
        let _ = write_json(&mut io::stderr().lock(), &body);
    } else {
        let _ = error.print();
    }
    ExitCode::from(EXIT_USAGE)
}

fn output_mode_from_args(args: &[OsString]) -> OutputMode {
    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--output" {
            return args
                .next()
                .and_then(|value| value.to_str())
                .and_then(parse_output_mode)
                .unwrap_or(OutputMode::Auto);
        }
        if let Some(value) = arg.to_str().and_then(|arg| arg.strip_prefix("--output=")) {
            return parse_output_mode(value).unwrap_or(OutputMode::Auto);
        }
    }
    OutputMode::Auto
}

fn parse_output_mode(value: &str) -> Option<OutputMode> {
    match value {
        "auto" => Some(OutputMode::Auto),
        "human" => Some(OutputMode::Human),
        "json" => Some(OutputMode::Json),
        _ => None,
    }
}

fn detect_shell() -> clap_complete::Shell {
    let shell = std::env::var("SHELL").ok().and_then(|path| {
        std::path::PathBuf::from(path)
            .file_name()?
            .to_str()
            .map(str::to_owned)
    });
    match shell.as_deref() {
        Some("zsh") => clap_complete::Shell::Zsh,
        Some("fish") => clap_complete::Shell::Fish,
        Some("elvish") => clap_complete::Shell::Elvish,
        Some("powershell" | "pwsh") => clap_complete::Shell::PowerShell,
        _ => clap_complete::Shell::Bash,
    }
}
