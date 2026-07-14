use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(
    name = "drag",
    version,
    about = "Log and track time in Tempo Cloud from the command line",
    propagate_version = true
)]
pub struct Cli {
    /// Output mode. Auto uses text in a terminal and JSON when redirected.
    #[arg(long, global = true, value_enum, default_value_t = OutputMode::Auto)]
    pub output: OutputMode,

    /// Print request diagnostics to stderr (credentials are always redacted).
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
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Add a worklog using a duration or interval.
    #[command(visible_alias = "l")]
    Log(LogArgs),
    /// List worklogs for a date.
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// Delete one or more worklogs.
    #[command(visible_alias = "d")]
    Delete(DeleteArgs),
    /// Connect Jira and Tempo, verify both connections, then save.
    Setup(SetupArgs),
    /// Manage issue-key aliases.
    Alias {
        #[command(subcommand)]
        command: AliasCommand,
    },
    /// Manage local issue timers.
    Tracker {
        #[command(subcommand)]
        command: TrackerCommand,
    },
    /// Start a tracker (compatibility alias for `tracker start`).
    #[command(alias = "tracker:start")]
    Start(TrackerStartArgs),
    /// Pause a tracker (compatibility alias for `tracker pause`).
    #[command(alias = "tracker:pause")]
    Pause(TrackerIssueArgs),
    /// Resume a tracker (compatibility alias for `tracker resume`).
    #[command(alias = "tracker:resume")]
    Resume(TrackerIssueArgs),
    /// Stop a tracker and upload its completed intervals.
    #[command(alias = "tracker:stop")]
    Stop(TrackerStopArgs),
    /// Generate shell completions.
    #[command(visible_alias = "autocomplete")]
    Completions {
        #[arg(value_enum)]
        shell: Option<Shell>,
    },
    /// Report local diagnostics and optionally run read-only connection checks.
    Doctor(DoctorArgs),
    /// Describe the machine-readable CLI contract.
    Schema,
    /// Compatibility form for `alias set`.
    #[command(name = "alias:set", hide = true)]
    LegacyAliasSet(AliasSetArgs),
    /// Compatibility form for `alias list`.
    #[command(name = "alias:list", hide = true)]
    LegacyAliasList,
    /// Compatibility form for `alias delete`.
    #[command(name = "alias:delete", hide = true)]
    LegacyAliasDelete(AliasDeleteArgs),
    /// Compatibility form for `tracker list`.
    #[command(name = "tracker:list", hide = true)]
    LegacyTrackerList,
    /// Compatibility form for `tracker delete`.
    #[command(name = "tracker:delete", hide = true)]
    LegacyTrackerDelete(TrackerIssueArgs),
}

#[derive(Debug, Args)]
pub struct LogArgs {
    /// Jira issue key or configured alias.
    #[arg(required_unless_present = "json")]
    pub issue_key_or_alias: Option<String>,
    /// Duration (`1h15m`) or interval (`11-12:30`).
    #[arg(required_unless_present = "json")]
    pub duration_or_interval: Option<String>,
    /// Date: YYYY-MM-DD, y, yesterday, t±N, or today±N.
    pub when: Option<String>,
    /// Worklog description.
    #[arg(short, long)]
    pub description: Option<String>,
    /// Start time for duration input.
    #[arg(short, long)]
    pub start: Option<String>,
    /// Remaining estimate such as 2h.
    #[arg(short = 'r', long)]
    pub remaining_estimate: Option<String>,
    /// Raw input JSON, or '-' to read it from stdin.
    #[arg(long, conflicts_with_all = ["issue_key_or_alias", "duration_or_interval", "when", "description", "start", "remaining_estimate"])]
    pub json: Option<String>,
    /// Validate and print the Tempo request without sending it.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogInput {
    pub issue_key_or_alias: String,
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

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Date: YYYY-MM-DD, y, yesterday, t±N, or today±N.
    pub when: Option<String>,
    /// Include descriptions and Jira URLs.
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Numeric Tempo worklog IDs.
    #[arg(required = true, num_args = 1..)]
    pub worklog_ids: Vec<u64>,
    /// Show what would be deleted without changing Tempo.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Verify and save four environment-provided connection values without prompting.
    #[arg(long)]
    pub from_env: bool,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Check Jira and Tempo with read-only remote requests.
    #[arg(long)]
    pub remote: bool,
}

#[derive(Debug, Subcommand)]
pub enum AliasCommand {
    /// Set or replace an alias.
    Set(AliasSetArgs),
    /// List aliases.
    List,
    /// Delete an alias.
    Delete(AliasDeleteArgs),
}

#[derive(Debug, Args)]
pub struct AliasSetArgs {
    pub alias: String,
    pub issue_key: String,
}

#[derive(Debug, Args)]
pub struct AliasDeleteArgs {
    pub alias_name: String,
}

#[derive(Debug, Subcommand)]
pub enum TrackerCommand {
    /// Start a new tracker.
    Start(TrackerStartArgs),
    /// Pause an active tracker.
    Pause(TrackerIssueArgs),
    /// Resume a paused tracker.
    Resume(TrackerIssueArgs),
    /// Stop a tracker and upload all completed intervals.
    Stop(TrackerStopArgs),
    /// Delete a tracker without uploading it.
    Delete(TrackerIssueArgs),
    /// List all trackers.
    List,
}

#[derive(Debug, Args)]
pub struct TrackerStartArgs {
    pub issue_key_or_alias: String,
    #[arg(short, long)]
    pub description: Option<String>,
    /// Stop and upload an existing tracker for the same issue first.
    #[arg(long)]
    pub stop_previous: bool,
}

#[derive(Debug, Args)]
pub struct TrackerIssueArgs {
    pub issue_key_or_alias: String,
}

#[derive(Debug, Args)]
pub struct TrackerStopArgs {
    pub issue_key_or_alias: String,
    #[arg(short, long)]
    pub description: Option<String>,
    #[arg(short = 'r', long)]
    pub remaining_estimate: Option<String>,
    /// Build worklog requests but do not upload or remove the tracker.
    #[arg(long)]
    pub dry_run: bool,
}
