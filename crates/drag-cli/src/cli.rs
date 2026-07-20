use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(
    name = "drag",
    version,
    about = "Log time in Tempo Cloud from the command line",
    propagate_version = true
)]
pub struct Cli {
    /// Output mode. Auto uses text in a terminal and JSON when redirected; NDJSON is list-only.
    #[arg(long, global = true, value_enum, default_value_t = OutputMode::Auto)]
    pub output: OutputMode,

    /// Print request diagnostics to stderr in human output (credentials are redacted).
    #[arg(long, global = true)]
    pub debug: bool,

    /// Override the config file (also available as DRAG_CONFIG).
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Override the local IANA time zone, for example Europe/Warsaw.
    #[arg(long, global = true, value_name = "ZONE")]
    pub timezone: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputMode {
    Auto,
    Human,
    Json,
    Ndjson,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Add a worklog using a duration or interval.
    ///
    /// WHEN defaults to today in the configured local time zone and accepts
    /// YYYY-MM-DD, y, yesterday, t±N, or today±N. Durations use --start when
    /// supplied; intervals include their own start time.
    #[command(
        visible_alias = "l",
        after_help = "Aliases:\n  drag l\n\nExamples:\n  drag log ABC-123 1h\n  drag l ABC-123 11:35-14:20 yesterday -d \"review\"\n  drag log ABC-123 11.35-14.20 2026-07-14\n  drag log ABC-123 1h15m 2026-07-14 --start 09:30 --remaining-estimate 2h\n  drag log --json '{\"issueKey\":\"ABC-123\",\"durationOrInterval\":\"30m\"}' --dry-run\n  printf '%s' '{\"issueKey\":\"ABC-123\",\"durationOrInterval\":\"30m\"}' | drag log --json - --dry-run"
    )]
    Log(LogArgs),
    /// List worklogs for a date without changing Jira or Tempo.
    ///
    /// DATE defaults to today in the configured local time zone and accepts
    /// YYYY-MM-DD, y, yesterday, t±N, or today±N. Add --verbose to include
    /// descriptions and Jira URLs in human output. Human output becomes an
    /// interactive stderr report only when stdin, stdout, and stderr are all
    /// terminals; otherwise it falls back to plain text. Use h/l for adjacent
    /// dates, Up/Down or j/k for rows, and o to open the focused Jira URL in
    /// the local default browser without changing Jira or Tempo. Quit with q,
    /// Escape, or Ctrl-C. Automation should pass --output json explicitly.
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// Delete one or more worklogs.
    #[command(visible_alias = "d")]
    Delete(DeleteArgs),
    /// Connect Jira and Tempo, verify both connections, then save.
    ///
    /// Interactive setup requires terminal-capable stdin and stderr and opens
    /// Ratatui for Jira account details, Atlassian API token, Tempo account,
    /// and Review & save. Use Tab and Shift-Tab to move and Enter to continue.
    /// No browser opens while entering Jira details; each token settings page
    /// opens only after you explicitly enter its token stage. Escape goes back,
    /// or cancels from Jira account details; Ctrl-C cancels from any stage. Use
    /// --from-env for unattended setup or --no-open
    /// to keep token URLs in the terminal without launching a browser. Set
    /// DRAG_REDUCED_MOTION=1 for a gentler color-only brand transition.
    Setup(SetupArgs),
    /// Report local diagnostics without network access.
    ///
    /// Add --remote to run opt-in, read-only Jira and Tempo connection checks.
    Doctor(DoctorArgs),
    /// Call Tempo operations generated from the official OpenAPI document.
    Tempo(TempoArgs),
    /// Describe the CLI contract or one Tempo OpenAPI operation.
    Schema(SchemaArgs),
}

#[derive(Debug, Args)]
#[command(disable_help_flag = true)]
pub struct TempoArgs {
    /// OpenAPI-generated resource, method, and method flags.
    #[arg(
        value_name = "ARGUMENT",
        num_args = 0..,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub arguments: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SchemaArgs {
    /// Optional dotted Tempo operation, for example tempo.worklogs.create.
    #[arg(value_name = "PATH")]
    pub path: Option<String>,
    /// Resolve local OpenAPI references in the selected operation.
    #[arg(long, requires = "path")]
    pub resolve_refs: bool,
}

#[derive(Debug, Args)]
pub struct LogArgs {
    /// Jira issue key.
    #[arg(required_unless_present = "json")]
    pub issue_key: Option<String>,
    /// Duration (`1h15m`) or interval (`11-12:30` or `11.35-14.20`).
    #[arg(required_unless_present = "json")]
    pub duration_or_interval: Option<String>,
    /// Date: YYYY-MM-DD, y, yesterday, t±N, or today±N.
    pub when: Option<String>,
    /// Worklog description.
    #[arg(short, long)]
    pub description: Option<String>,
    /// Start time for duration input (HH:mm).
    #[arg(short, long)]
    pub start: Option<String>,
    /// Remaining estimate as a duration, such as 2h.
    #[arg(short = 'r', long)]
    pub remaining_estimate: Option<String>,
    /// Raw input JSON, or '-' to read it from stdin.
    #[arg(long, conflicts_with_all = ["issue_key", "duration_or_interval", "when", "description", "start", "remaining_estimate"])]
    pub json: Option<String>,
    /// Validate and print the Tempo request without sending it.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogInput {
    pub issue_key: String,
    pub duration_or_interval: String,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub remaining_estimate: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ListArgs {
    /// Optional date (defaults to today): YYYY-MM-DD, y, yesterday, t±N, or today±N.
    #[arg(value_name = "DATE")]
    pub when: Option<String>,
    /// Include descriptions and Jira URLs.
    #[arg(short, long)]
    pub verbose: bool,
    /// Comma-delimited result fields to include in structured output.
    #[arg(long, value_name = "MASK")]
    pub fields: Option<String>,
    /// Maximum worklogs to retrieve and return (1-1000; default: 100).
    #[arg(
        long,
        value_parser = clap::value_parser!(u16).range(1..=1000),
        conflicts_with = "all_pages"
    )]
    pub limit: Option<u16>,
    /// Maximum Tempo pages to retrieve (1-100; default: 1).
    #[arg(
        long,
        value_parser = clap::value_parser!(u16).range(1..=100),
        conflicts_with = "all_pages"
    )]
    pub page_limit: Option<u16>,
    /// Resume from the opaque continuation token returned by a prior list result.
    #[arg(long, value_name = "TOKEN")]
    pub continue_from: Option<String>,
    /// Retrieve every page, subject to the 100-page safety ceiling.
    #[arg(long)]
    pub all_pages: bool,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Numeric Tempo worklog IDs.
    #[arg(required_unless_present = "json", num_args = 1..)]
    pub worklog_ids: Vec<u64>,
    /// Raw input JSON, or '-' to read it from stdin.
    #[arg(long, conflicts_with = "worklog_ids")]
    pub json: Option<String>,
    /// Show what would be deleted without changing Tempo.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteInput {
    #[schemars(length(min = 1))]
    pub worklog_ids: Vec<u64>,
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Verify and save the four required environment values without prompting.
    #[arg(long)]
    pub from_env: bool,

    /// Print token URLs without launching a browser.
    #[arg(long)]
    pub no_open: bool,

    /// Validate unattended setup and report planned effects without saving.
    #[arg(long, requires = "from_env")]
    pub dry_run: bool,

    /// Perform read-only Jira and Tempo checks during an unattended dry-run.
    #[arg(long, requires_all = ["from_env", "dry_run"])]
    pub verify: bool,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Opt in to read-only Jira and Tempo connection checks.
    #[arg(long)]
    pub remote: bool,
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::Parser;

    use super::{Cli, Command};

    #[test]
    fn list_and_ls_parse_to_the_same_command_arguments() -> Result<(), String> {
        for command in ["list", "ls"] {
            let cli = Cli::try_parse_from([
                "drag",
                command,
                "yesterday",
                "--verbose",
                "--fields",
                "worklogs.id,pagination.next",
            ])
            .map_err(|error| error.to_string())?;
            let args = match cli.command {
                Command::List(args) => args,
                _ => return Err(format!("{command} did not dispatch to list")),
            };
            assert_eq!(args.when.as_deref(), Some("yesterday"));
            assert!(args.verbose);
            assert_eq!(args.fields.as_deref(), Some("worklogs.id,pagination.next"));
        }
        Ok(())
    }

    #[test]
    fn removed_commands_are_not_available() -> Result<(), String> {
        for arguments in [
            &["tracker", "start", "ABC-1"][..],
            &["tracker", "pause", "ABC-1"],
            &["tracker", "resume", "ABC-1"],
            &["tracker", "stop", "ABC-1"],
            &["tracker", "delete", "ABC-1"],
            &["tracker", "list"],
            &["start", "ABC-1"],
            &["pause", "ABC-1"],
            &["resume", "ABC-1"],
            &["stop", "ABC-1"],
            &["tracker:start", "ABC-1"],
            &["tracker:pause", "ABC-1"],
            &["tracker:resume", "ABC-1"],
            &["tracker:stop", "ABC-1"],
            &["tracker:list"],
            &["tracker:delete", "ABC-1"],
            &["alias", "list"],
            &["alias", "set", "lunch", "ABC-1"],
            &["alias", "delete", "lunch"],
            &["alias:list"],
            &["alias:set", "lunch", "ABC-1"],
            &["alias:delete", "lunch"],
            &["completions"],
            &["autocomplete"],
        ] {
            let Err(error) =
                Cli::try_parse_from(std::iter::once("drag").chain(arguments.iter().copied()))
            else {
                return Err(format!(
                    "removed command unexpectedly parsed: {arguments:?}"
                ));
            };
            assert_eq!(error.kind(), ErrorKind::InvalidSubcommand, "{arguments:?}");
        }
        Ok(())
    }
}
