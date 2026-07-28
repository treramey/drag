use crate::*;

#[derive(Debug, Parser)]
#[command(
    name = "drag-tracking",
    version,
    about = "Automatic time tracking for Drag",
    propagate_version = true
)]
pub(crate) struct Cli {
    /// Output format for public tracking commands.
    #[arg(long, global = true, value_enum)]
    pub(crate) output: Option<TrackingOutputMode>,

    /// Tracking state directory. Defaults to ~/.drag/tracking.
    #[arg(long, global = true, value_name = "DIR")]
    pub(crate) data_dir: Option<PathBuf>,

    /// Drag executable used for public gateway/process-boundary operations.
    #[arg(long, global = true, default_value = "drag", value_name = "EXE")]
    pub(crate) drag_bin: PathBuf,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum TrackingOutputMode {
    Human,
    Json,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Configure sources, schedule, and submission policy.
    Setup(PublicSetupArgs),
    /// Show tracking state and safety posture.
    Status,
    /// Run the complete tracking workflow for one selected date.
    Run(PublicDateArgs),
    /// Inspect or approve the immutable proposal set for one selected date.
    Review(PublicReviewArgs),
    /// Pause scheduled tracking without deleting history.
    Pause,
    /// Resume scheduled tracking after validating configuration.
    #[command(name = "resume")]
    ResumeTracking,
    /// Remove tracking-owned scheduler and hook files while preserving history.
    Uninstall,
    /// Discover, configure, or test local evidence sources.
    Sources(PublicSourcesArgs),
    /// Inspect or update the tracking schedule.
    Schedule(PublicScheduleArgs),
    /// Low-level diagnostics and recovery operations.
    #[command(hide = true)]
    Internal(InternalArgs),
    /// Collect fake adapter observations without network access.
    #[command(hide = true)]
    Collect(CollectArgs),
    /// Capture one explicit-date fake evidence event in the append-only journal.
    #[command(hide = true)]
    Capture(DateArgs),
    /// Import append-only journal events into the canonical SQLite store.
    #[command(hide = true)]
    Import,
    /// Run an explicit-date fake reconciliation and persist a terminal result.
    #[command(hide = true)]
    Reconcile(DateArgs),
    /// Print a persisted explicit-date terminal report.
    #[command(hide = true)]
    Report(DateArgs),
    /// Print a secret-safe structured JSON operator log for one explicit local date.
    #[command(hide = true)]
    Log(DateArgs),
    /// Print a byte-stable minimized evidence bundle for one explicit local date.
    #[command(hide = true)]
    Bundle(DateArgs),
    /// Generate schema-constrained worklog proposals from a minimized bundle and offline provider fixture.
    #[command(hide = true)]
    Propose(ProposeArgs),
    /// Read the complete selected Tempo day through Drag without mutation.
    #[command(hide = true)]
    Read(DateArgs),
    /// Audit proposals against existing Tempo worklogs through Drag without mutation.
    #[command(hide = true)]
    Audit(AuditArgs),
    /// Preview exact structured Drag worklog payloads through dry-run only.
    #[command(hide = true)]
    Preview(PreviewArgs),
    /// Execute approved payloads through Drag with an idempotent operation ledger.
    #[command(hide = true)]
    Execute(ExecuteArgs),
    /// Inspect and advance persisted staged autonomy rollout gates.
    #[command(hide = true)]
    Rollout(RolloutArgs),
    /// Replay recorded historical workday fixtures without external services.
    #[command(hide = true)]
    Replay(ReplayArgs),
    /// Inspect the durable mutation operation ledger for tests and operators.
    #[command(hide = true)]
    ProcessSpy(DateArgs),
    /// Remove persisted capture-only companion state while protecting recovery records by default.
    #[command(hide = true)]
    Purge(PurgeArgs),
    /// Enforce age-based privacy retention safely and report compacted classes.
    #[command(hide = true)]
    Retention(RetentionArgs),
    /// Install, inspect, remove, catch up, or run scheduler-safe explicit-date reconciliation.
    #[command(hide = true)]
    Scheduler(SchedulerArgs),
    /// Install, remove, or capture Claude Code SessionStart/SessionEnd hooks.
    #[command(hide = true)]
    ClaudeHook(ClaudeHookArgs),
    /// Print the machine-readable command and side-effect contract.
    #[command(hide = true)]
    Contract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SubmissionMode {
    Draft,
    Review,
    Automatic,
}

#[derive(Debug, Args)]
pub(crate) struct PublicSetupArgs {
    #[arg(long, value_enum, default_value_t = SubmissionMode::Draft)]
    pub(crate) mode: SubmissionMode,
    /// Separately authorize automatic worklog submission.
    #[arg(long)]
    pub(crate) authorize_automatic: bool,
    /// Confirm installation of tracking-owned scheduler files.
    #[arg(long)]
    pub(crate) install_scheduler: bool,
    /// Confirm installation of tracking-owned Claude Code hooks.
    #[arg(long)]
    pub(crate) install_hooks: bool,
    #[arg(long, value_name = "DIR", requires = "install_scheduler")]
    pub(crate) scheduler_target: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_SCHEDULE_TIME)]
    pub(crate) at: String,
    #[arg(long, default_value = DEFAULT_SCHEDULE_TIMEZONE)]
    pub(crate) schedule_timezone: String,
    #[arg(long = "repo", value_name = "DIR")]
    pub(crate) repos: Vec<PathBuf>,
    #[arg(long = "ics", value_name = "FILE")]
    pub(crate) ics_files: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct PublicDateArgs {
    /// Drag-style date selector; defaults to today.
    pub(crate) when: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct PublicReviewArgs {
    /// Drag-style date selector; defaults to today.
    pub(crate) when: Option<String>,
    /// Approve the current immutable proposal set.
    #[arg(long)]
    pub(crate) approve: bool,
    #[command(subcommand)]
    pub(crate) operation: Option<PublicReviewOperation>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PublicReviewOperation {
    Approve(PublicDateArgs),
}

#[derive(Debug, Args)]
pub(crate) struct PublicSourcesArgs {
    #[command(subcommand)]
    pub(crate) operation: PublicSourcesOperation,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PublicSourcesOperation {
    List,
    Configure(PublicSourceConfigurationArgs),
    Test(PublicDateArgs),
}

#[derive(Debug, Args)]
pub(crate) struct PublicSourceConfigurationArgs {
    #[arg(long = "repo", value_name = "DIR", conflicts_with = "clear_repos")]
    pub(crate) repos: Vec<PathBuf>,
    /// Remove every configured Git repository without changing other sources.
    #[arg(long, conflicts_with = "repos")]
    pub(crate) clear_repos: bool,
    #[arg(long = "ics", value_name = "FILE", conflicts_with = "clear_ics")]
    pub(crate) ics_files: Vec<PathBuf>,
    /// Remove every configured calendar without changing other sources.
    #[arg(long, conflicts_with = "ics_files")]
    pub(crate) clear_ics: bool,
    /// Select the installed Claude Code lifecycle hook as an evidence source.
    #[arg(long, conflicts_with = "no_claude_code")]
    pub(crate) claude_code: bool,
    /// Stop selecting Claude Code without removing its separately installed hooks.
    #[arg(long, conflicts_with = "claude_code")]
    pub(crate) no_claude_code: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PublicScheduleArgs {
    #[command(subcommand)]
    pub(crate) operation: PublicScheduleOperation,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PublicScheduleOperation {
    Show,
    Update(PublicScheduleUpdateArgs),
    Pause,
    Resume,
}

#[derive(Debug, Args)]
pub(crate) struct PublicScheduleUpdateArgs {
    #[arg(long)]
    pub(crate) at: String,
    #[arg(long)]
    pub(crate) schedule_timezone: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct InternalArgs {
    #[command(subcommand)]
    pub(crate) command: InternalCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum InternalCommand {
    Collect(CollectArgs),
    Capture(DateArgs),
    Import,
    Reconcile(DateArgs),
    Resume(DateArgs),
    Report(DateArgs),
    Log(DateArgs),
    Bundle(DateArgs),
    Propose(ProposeArgs),
    Read(DateArgs),
    Audit(AuditArgs),
    Preview(PreviewArgs),
    Execute(ExecuteArgs),
    Rollout(RolloutArgs),
    Replay(ReplayArgs),
    ProcessSpy(DateArgs),
    Purge(PurgeArgs),
    Retention(RetentionArgs),
    Scheduler(SchedulerArgs),
    ClaudeHook(ClaudeHookArgs),
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
    /// Claude settings JSON path to update. Defaults to ~/.claude/settings.json.
    #[arg(
        long,
        value_name = "FILE",
        default_value_os_t = default_claude_settings_path()
    )]
    pub(crate) settings: PathBuf,
}

pub(crate) fn default_claude_settings_path() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude/settings.json")
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

pub(crate) fn print_error_json<T: Serialize>(value: &T) -> Result<(), CompanionError> {
    let body = serde_json::to_string(value).map_err(CompanionError::Serialize)?;
    eprintln!("{body}");
    Ok(())
}

pub(crate) fn println_safe_markdown(markdown: &str) -> Result<(), CompanionError> {
    println!("{markdown}");
    Ok(())
}
