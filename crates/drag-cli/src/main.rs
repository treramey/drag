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
mod tempo_openapi;
mod transport;
mod tui_theme;

use std::ffi::OsString;
use std::io;
use std::process::ExitCode;

use clap::Parser;
use cli::{Cli, Command};

use crate::app::{default_timezone, App};
pub(crate) use crate::error::{CliError, RemoteError, RemoteErrorKind, RemoteService, EXIT_USAGE};
use crate::output::{
    emit_error, emit_result, handle_parse_error, output_mode_from_args, resolve_mode,
};
pub(crate) use crate::output::{Rendered, ResolvedOutputMode};

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
        Ok(RunResult::Plain(output)) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            emit_error(&error, mode);
            ExitCode::from(error.exit_code())
        }
    }
}

enum RunResult {
    Rendered(Rendered),
    Streamed,
    Plain(String),
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
        Command::Doctor(args) => app.doctor(args).await?,
        Command::Tempo(args) => {
            match tempo_openapi::run_command(args.arguments, &path, debug).await? {
                tempo_openapi::CommandOutput::Rendered(rendered) => rendered,
                tempo_openapi::CommandOutput::Plain(output) => return Ok(RunResult::Plain(output)),
            }
        }
        Command::Schema(args) => schema::run(args).await?,
    };
    Ok(RunResult::Rendered(rendered))
}

fn request_debug_enabled(requested: bool, mode: ResolvedOutputMode) -> bool {
    requested && mode == ResolvedOutputMode::Human
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
