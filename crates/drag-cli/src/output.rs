use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use serde::Serialize;
use serde_json::Value;

use crate::cli::OutputMode;
use crate::error::{CliError, EXIT_USAGE};

#[derive(Debug)]
pub(crate) struct Rendered {
    pub(crate) data: Value,
    pub(crate) human: String,
    pub(crate) failure: Option<RenderedFailure>,
}

#[derive(Debug)]
pub(crate) struct RenderedFailure {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
    pub(crate) exit_code: u8,
}

impl Rendered {
    pub(crate) fn new(data: Value, human: String) -> Self {
        Self {
            data,
            human,
            failure: None,
        }
    }

    pub(crate) fn failed(
        data: Value,
        human: String,
        code: &'static str,
        message: &'static str,
        exit_code: u8,
    ) -> Self {
        Self {
            data,
            human,
            failure: Some(RenderedFailure {
                code,
                message,
                exit_code,
            }),
        }
    }

    pub(crate) const fn exit_code(&self) -> u8 {
        match &self.failure {
            Some(failure) => failure.exit_code,
            None => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolvedOutputMode {
    Human,
    Json,
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

#[derive(Serialize)]
struct DiagnosticFailure<T> {
    ok: bool,
    error: DiagnosticError<T>,
}

#[derive(Serialize)]
struct DiagnosticError<T> {
    code: &'static str,
    message: &'static str,
    details: T,
}

pub(crate) fn resolve_mode(mode: OutputMode) -> ResolvedOutputMode {
    match mode {
        OutputMode::Human => ResolvedOutputMode::Human,
        OutputMode::Auto if io::stdout().is_terminal() => ResolvedOutputMode::Human,
        OutputMode::Auto | OutputMode::Json => ResolvedOutputMode::Json,
    }
}

pub(crate) fn emit_result(result: Rendered, mode: ResolvedOutputMode) -> Result<(), CliError> {
    if let Some(failure) = result.failure {
        match mode {
            ResolvedOutputMode::Human => eprintln!("{}", sanitize_for_terminal(&result.human)),
            ResolvedOutputMode::Json => write_json(
                &mut io::stderr().lock(),
                &DiagnosticFailure {
                    ok: false,
                    error: DiagnosticError {
                        code: failure.code,
                        message: failure.message,
                        details: result.data,
                    },
                },
            )?,
        }
        return Ok(());
    }
    match mode {
        ResolvedOutputMode::Human => println!("{}", sanitize_for_terminal(&result.human)),
        ResolvedOutputMode::Json => write_json(
            &mut io::stdout().lock(),
            &Success {
                ok: true,
                data: result.data,
            },
        )?,
    }
    Ok(())
}

pub(crate) fn emit_error(error: &CliError, mode: ResolvedOutputMode) {
    if mode == ResolvedOutputMode::Json {
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
        eprintln!("error: {}", sanitize_for_terminal(&error.to_string()));
    }
}

pub(crate) fn handle_parse_error(error: clap::Error, requested: OutputMode) -> ExitCode {
    use clap::error::ErrorKind::{DisplayHelp, DisplayVersion};
    if matches!(error.kind(), DisplayHelp | DisplayVersion) {
        let _ = error.print();
        return ExitCode::SUCCESS;
    }
    if resolve_mode(requested) == ResolvedOutputMode::Json {
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

pub(crate) fn output_mode_from_args(args: &[OsString]) -> OutputMode {
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

pub(crate) fn sanitize_for_terminal(text: &str) -> String {
    text.chars()
        .filter(|character| {
            matches!(character, '\n' | '\t')
                || (!character.is_control() && !is_dangerous_unicode(*character))
        })
        .collect()
}

fn is_dangerous_unicode(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}'
            | '\u{feff}'
    )
}

fn parse_output_mode(value: &str) -> Option<OutputMode> {
    match value {
        "auto" => Some(OutputMode::Auto),
        "human" => Some(OutputMode::Human),
        "json" => Some(OutputMode::Json),
        _ => None,
    }
}

fn write_json(writer: &mut impl Write, value: &impl Serialize) -> Result<(), CliError> {
    serde_json::to_writer_pretty(&mut *writer, value)?;
    writeln!(writer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sanitize_for_terminal;

    #[test]
    fn terminal_sanitizer_removes_escape_and_bidi_control_characters() {
        assert_eq!(
            sanitize_for_terminal("safe\u{1b}[31mred\u{202e}hidden"),
            "safe[31mredhidden"
        );
    }

    #[test]
    fn terminal_sanitizer_preserves_readable_whitespace_and_unicode() {
        assert_eq!(
            sanitize_for_terminal("first\nsecond\t日本語"),
            "first\nsecond\t日本語"
        );
    }
}
