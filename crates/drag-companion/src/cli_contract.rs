use crate::*;

#[derive(Debug, Parser)]
#[command(
    name = "drag-companion",
    version,
    about = "Safe capture-only companion for explicit-date Drag reconciliation",
    propagate_version = true
)]
pub(crate) struct Cli {
    /// Directory for companion state. Defaults to .drag-companion in the current directory.
    #[arg(long, global = true, value_name = "DIR")]
    pub(crate) data_dir: Option<PathBuf>,

    /// Drag executable used for public gateway/process-boundary operations.
    #[arg(long, global = true, default_value = "drag", value_name = "EXE")]
    pub(crate) drag_bin: PathBuf,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Show companion state and safety posture.
    Status,
    /// Collect fake adapter observations without network access.
    Collect(CollectArgs),
    /// Capture one explicit-date fake evidence event in the append-only journal.
    Capture(DateArgs),
    /// Import append-only journal events into the canonical SQLite store.
    Import,
    /// Run an explicit-date fake reconciliation and persist a terminal result.
    Reconcile(DateArgs),
    /// Resume a previously captured explicit-date run without live mutation.
    Resume(DateArgs),
    /// Print a persisted explicit-date terminal report.
    Report(DateArgs),
    /// Print a secret-safe structured JSON operator log for one explicit local date.
    Log(DateArgs),
    /// Print a byte-stable minimized evidence bundle for one explicit local date.
    Bundle(DateArgs),
    /// Generate schema-constrained worklog proposals from a minimized bundle and offline provider fixture.
    Propose(ProposeArgs),
    /// Read the complete selected Tempo day through Drag without mutation.
    Read(DateArgs),
    /// Audit proposals against existing Tempo worklogs through Drag without mutation.
    Audit(AuditArgs),
    /// Preview exact structured Drag worklog payloads through dry-run only.
    Preview(PreviewArgs),
    /// Execute approved payloads through Drag with an idempotent operation ledger.
    Execute(ExecuteArgs),
    /// Inspect and advance persisted staged autonomy rollout gates.
    Rollout(RolloutArgs),
    /// Replay recorded historical workday fixtures without external services.
    Replay(ReplayArgs),
    /// Inspect the durable mutation operation ledger for tests and operators.
    ProcessSpy(DateArgs),
    /// Remove persisted capture-only companion state while protecting recovery records by default.
    Purge(PurgeArgs),
    /// Enforce age-based privacy retention safely and report compacted classes.
    Retention(RetentionArgs),
    /// Install, inspect, remove, catch up, or run scheduler-safe explicit-date reconciliation.
    Scheduler(SchedulerArgs),
    /// Install, remove, or capture Claude Code SessionStart/SessionEnd hooks.
    ClaudeHook(ClaudeHookArgs),
    /// Print the machine-readable command and side-effect contract.
    Contract,
}

#[derive(Debug, Args)]
pub(crate) struct DateArgs {
    /// Explicit reconciliation date in YYYY-MM-DD format.
    #[arg(long, value_parser = parse_date)]
    pub(crate) date: NaiveDate,
}

#[derive(Debug, Args)]
pub(crate) struct ProposeArgs {
    /// Explicit reconciliation date in YYYY-MM-DD format.
    #[arg(long, value_parser = parse_date)]
    pub(crate) date: NaiveDate,
    /// Offline recorded provider fixture JSON. No network or tools are available.
    #[arg(long, value_name = "FILE")]
    pub(crate) fixture: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct AuditArgs {
    /// Explicit reconciliation date in YYYY-MM-DD format.
    #[arg(long, value_parser = parse_date)]
    pub(crate) date: NaiveDate,
    /// Explicitly authorize unattended approval decisions. Still never permits mutation.
    #[arg(long)]
    pub(crate) authorize_unattended: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PreviewArgs {
    /// Explicit reconciliation date in YYYY-MM-DD format.
    #[arg(long, value_parser = parse_date)]
    pub(crate) date: NaiveDate,
    /// Proposal id to preview. Defaults to the first persisted proposal for the date.
    #[arg(long)]
    pub(crate) proposal: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ExecuteArgs {
    /// Explicit reconciliation date in YYYY-MM-DD format.
    #[arg(long, value_parser = parse_date)]
    pub(crate) date: NaiveDate,
    /// Explicitly authorize live Drag mutation. Rollout env must also be enabled.
    #[arg(long)]
    pub(crate) authorize_live: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RolloutArgs {
    #[command(subcommand)]
    pub(crate) operation: RolloutOperation,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RolloutOperation {
    /// Show persisted rollout stage, effective mutation mode, and gates.
    Status,
    /// Record promotion evidence or a safety failure.
    Record(RolloutRecordArgs),
    /// Promote by at most one eligible gate.
    Promote,
    /// Show the effective mode after persisted rollout state and safety prerequisites.
    EffectiveMode(RolloutEffectiveModeArgs),
}

#[derive(Debug, Args)]
pub(crate) struct RolloutRecordArgs {
    /// Gate/evidence class to record: fixture, replay, shadow, reviewed, restricted, general.
    #[arg(long)]
    pub(crate) gate: Option<String>,
    #[arg(long, default_value_t = 0)]
    pub(crate) eligible_days: u64,
    #[arg(long, default_value_t = 0)]
    pub(crate) proposals: u64,
    #[arg(long, default_value_t = 1.0)]
    pub(crate) issue_attribution_precision: f64,
    #[arg(long, default_value_t = 1.0)]
    pub(crate) supported_duration_precision: f64,
    #[arg(long, default_value_t = true)]
    pub(crate) schema_valid: bool,
    #[arg(long, default_value_t = true)]
    pub(crate) provenance_retained: bool,
    #[arg(long, default_value_t = true)]
    pub(crate) secrets_redacted: bool,
    #[arg(long, default_value_t = 0)]
    pub(crate) reviewed_batches: u64,
    #[arg(long, default_value_t = 0)]
    pub(crate) incorrect_creates: u64,
    #[arg(long, default_value_t = 0)]
    pub(crate) duplicates: u64,
    #[arg(long, default_value_t = 0)]
    pub(crate) overlap_violations: u64,
    #[arg(long, default_value_t = 0)]
    pub(crate) uncertain_outcome_retries: u64,
    #[arg(long, default_value_t = 0)]
    pub(crate) privacy_incidents: u64,
    #[arg(long, default_value_t = 0)]
    pub(crate) fabricated_material_fields: u64,
    #[arg(long, default_value_t = 0)]
    pub(crate) unsafe_retries: u64,
    /// Unsafe proposal reason. Resets the applicable gate.
    #[arg(long)]
    pub(crate) unsafe_reason: Option<String>,
    /// General autonomy expansion token. One evidence class or policy rule per promotion.
    #[arg(long)]
    pub(crate) expansion: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct RolloutEffectiveModeArgs {
    #[arg(long)]
    pub(crate) collector_health_failure: bool,
    #[arg(long)]
    pub(crate) schema_compatibility_failure: bool,
    #[arg(long)]
    pub(crate) lock_failure: bool,
    #[arg(long)]
    pub(crate) incomplete_day: bool,
    #[arg(long)]
    pub(crate) mutation_uncertainty: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ReplayArgs {
    /// Directory containing recorded replay day fixture JSON files.
    #[arg(long, value_name = "DIR")]
    pub(crate) fixtures: PathBuf,
    /// Optional directory to write secret-safe replay artifacts.
    #[arg(long, value_name = "DIR")]
    pub(crate) artifacts: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct PurgeArgs {
    /// Also delete idempotency records, acknowledging automated recovery guarantees are lost.
    #[arg(long)]
    pub(crate) acknowledge_lost_recovery: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RetentionArgs {
    #[command(subcommand)]
    pub(crate) operation: RetentionOperation,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RetentionOperation {
    /// Apply configured raw, normalized, and report/ledger retention windows now.
    Enforce,
}

#[derive(Debug, Args)]
pub(crate) struct CollectArgs {
    /// Local Git repository to scan. Repeat for each configured repository.
    #[arg(long = "repo", value_name = "DIR")]
    pub(crate) repos: Vec<PathBuf>,
    /// Explicit selected day for bounded local ICS expansion.
    #[arg(long, value_parser = parse_date)]
    pub(crate) date: Option<NaiveDate>,
    /// Local RFC 5545 .ics file to import. Repeat for each configured calendar file.
    #[arg(long = "ics", value_name = "FILE")]
    pub(crate) ics_files: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct SchedulerArgs {
    #[command(subcommand)]
    pub(crate) operation: SchedulerOperation,
}

#[derive(Debug, Args)]
pub(crate) struct ClaudeHookArgs {
    #[command(subcommand)]
    pub(crate) operation: ClaudeHookOperation,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ClaudeHookOperation {
    /// Install SessionStart and SessionEnd capture hooks in a Claude settings JSON file.
    Install(ClaudeHookSettingsArgs),
    /// Remove only drag-companion Claude hook commands from a Claude settings JSON file.
    Remove(ClaudeHookSettingsArgs),
    /// Capture one Claude hook payload from stdin into the local journal.
    Capture,
}

#[derive(Debug, Args)]
pub(crate) struct ClaudeHookSettingsArgs {
    /// Claude settings JSON path to update.
    #[arg(long, value_name = "FILE")]
    pub(crate) settings: PathBuf,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SchedulerOperation {
    /// Install scheduler files into an explicit directory without touching unrelated config.
    Install(SchedulerInstallArgs),
    /// Mark the companion scheduler enabled in companion state.
    Enable,
    /// Mark the companion scheduler disabled in companion state.
    Disable,
    /// Remove only files previously installed by drag-companion.
    Uninstall(SchedulerInstallArgs),
    /// Show scheduler status from companion state only.
    Status,
    /// Select and run the latest eligible missed workday, if any.
    CatchUp(SchedulerCatchUpArgs),
    /// Scheduler-safe explicit-date command invoked by host schedulers.
    Run(SchedulerRunArgs),
}

#[derive(Debug, Args, Clone)]
pub(crate) struct SchedulerInstallArgs {
    /// Host scheduler platform to render. Defaults to the current OS.
    #[arg(long, value_parser = ["systemd", "launchd"], default_value = default_scheduler_platform())]
    pub(crate) platform: String,
    /// Directory containing user scheduler units/agents. Required for non-destructive installs.
    #[arg(long, value_name = "DIR")]
    pub(crate) target_dir: PathBuf,
    /// Local time to run in HH:MM.
    #[arg(long, default_value = DEFAULT_SCHEDULE_TIME)]
    pub(crate) at: String,
    /// IANA timezone or 'local'. Defaults to configured local time.
    #[arg(long, default_value = DEFAULT_SCHEDULE_TIMEZONE)]
    pub(crate) timezone: String,
}

#[derive(Debug, Args)]
pub(crate) struct SchedulerCatchUpArgs {
    /// Current local date used by tests and startup/wake reconciliation.
    #[arg(long, value_parser = parse_date)]
    pub(crate) today: Option<NaiveDate>,
    /// Last successfully reconciled local date.
    #[arg(long, value_parser = parse_date)]
    pub(crate) last_success: Option<NaiveDate>,
}

#[derive(Debug, Args)]
pub(crate) struct SchedulerRunArgs {
    /// Explicit reconciliation date in YYYY-MM-DD format.
    #[arg(long, value_parser = parse_date)]
    pub(crate) date: NaiveDate,
}

pub(crate) fn default_scheduler_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "launchd"
    } else {
        "systemd"
    }
}

pub(crate) fn parse_date(raw: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| "date must use YYYY-MM-DD".to_owned())
}

pub(crate) fn print_json<T: Serialize>(value: &T) -> Result<(), CompanionError> {
    let body = serde_json::to_string_pretty(value).map_err(CompanionError::Serialize)?;
    println!("{body}");
    Ok(())
}

pub(crate) fn println_safe_markdown(markdown: &str) -> Result<(), CompanionError> {
    println!("{markdown}");
    Ok(())
}
