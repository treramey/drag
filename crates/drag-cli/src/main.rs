mod alias;
mod api;
mod app;
mod browser;
mod cli;
mod config;
mod delete;
mod doctor;
mod error;
mod list;
mod list_tui;
mod log;
mod output;
mod schema;
mod setup;
mod setup_tui;
mod transport;
mod tui_theme;

use std::ffi::OsString;
use std::io;
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::{AliasCommand, Cli, Command};
use serde_json::json;

use crate::app::{default_timezone, App};
pub(crate) use crate::error::{CliError, RemoteError, RemoteErrorKind, RemoteService, EXIT_USAGE};
use crate::output::{
    emit_error, emit_result, handle_parse_error, output_mode_from_args, resolve_mode,
};
pub(crate) use crate::output::{Rendered, ResolvedOutputMode};
use crate::schema::schema;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let requested_mode = output_mode_from_args(&args);
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => return handle_parse_error(error, requested_mode, &args),
    };
    let mode = resolve_mode(cli.output);
    match run(cli, mode).await {
        Ok(RunResult::Rendered(result)) => {
            let exit_code = result.exit_code();
            match emit_result(result, mode) {
                Ok(()) => ExitCode::from(exit_code),
                Err(error) => {
                    emit_error(&error, mode);
                    ExitCode::from(error.exit_code())
                }
            }
        }
        Ok(RunResult::Streamed) => ExitCode::SUCCESS,
        Err(error) => {
            emit_error(&error, mode);
            ExitCode::from(error.exit_code())
        }
    }
}

enum RunResult {
    Rendered(Rendered),
    Streamed,
}

async fn run(cli: Cli, mode: ResolvedOutputMode) -> Result<RunResult, CliError> {
    if mode == ResolvedOutputMode::Ndjson && !matches!(&cli.command, Command::List(_)) {
        return Err(CliError::InvalidInput(
            "NDJSON output is supported only for list".to_owned(),
        ));
    }
    let timezone = default_timezone(cli.timezone.as_deref())?;
    let path = cli.config.unwrap_or(config::config_path()?);
    let debug = request_debug_enabled(cli.debug, mode);
    let app = App::new(path.clone(), timezone, debug);
    let rendered = match cli.command {
        Command::Log(args) => app.log(args).await?,
        Command::List(args) if mode == ResolvedOutputMode::Ndjson => {
            app.list_stream(args, &mut io::stdout().lock()).await?;
            return Ok(RunResult::Streamed);
        }
        Command::List(args) => match app.list(args, mode == ResolvedOutputMode::Human).await? {
            Some(rendered) => rendered,
            None => return Ok(RunResult::Streamed),
        },
        Command::Delete(args) => app.delete(args).await?,
        Command::Setup(args) => app.setup(args).await?,
        Command::Alias { command } => match command {
            AliasCommand::Set(args) => app.alias_set(args)?,
            AliasCommand::List => app.alias_list()?,
            AliasCommand::Delete(args) => app.alias_delete(args)?,
        },
        Command::LegacyAliasSet(args) => app.alias_set(args)?,
        Command::LegacyAliasList => app.alias_list()?,
        Command::LegacyAliasDelete(args) => app.alias_delete(args)?,
        Command::Completions { shell } => {
            let shell = shell.unwrap_or_else(detect_shell);
            let mut bytes = Vec::new();
            generate(shell, &mut Cli::command(), "drag", &mut bytes);
            let script = String::from_utf8(bytes)?;
            Rendered::new(
                json!({"shell": shell.to_string(), "script": script}),
                script,
            )
        }
        Command::Doctor(args) => app.doctor(args).await?,
        Command::Schema => schema(),
    };
    Ok(RunResult::Rendered(rendered))
}

fn request_debug_enabled(requested: bool, mode: ResolvedOutputMode) -> bool {
    requested && mode == ResolvedOutputMode::Human
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

#[cfg(test)]
mod tests {
    use super::{request_debug_enabled, ResolvedOutputMode};

    #[test]
    fn request_diagnostics_are_enabled_only_for_human_output() {
        assert!(request_debug_enabled(true, ResolvedOutputMode::Human));
        assert!(!request_debug_enabled(true, ResolvedOutputMode::Json));
        assert!(!request_debug_enabled(true, ResolvedOutputMode::Ndjson));
        assert!(!request_debug_enabled(false, ResolvedOutputMode::Human));
    }
}
