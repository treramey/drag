use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use chrono::{
    DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc,
};
use chrono_tz::Tz;
use clap::{Args, Parser, Subcommand};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

const DEFAULT_MODE: &str = "capture-only";
const COLLECTOR_ADAPTER: &str = "fake";
const MUTATOR_ADAPTER: &str = "disabled";
const JOURNAL_SCHEMA_VERSION: u32 = 1;
const STORE_SCHEMA_VERSION: i64 = 1;
const CLAUDE_HOOK_SCHEMA_VERSION: u32 = 1;
const CLAUDE_COLLECTOR: &str = "claude-code-session-hook";
const CLAUDE_HOOK_COMMAND: &str = "drag-companion claude-hook capture";

#[derive(Debug, Parser)]
#[command(
    name = "drag-companion",
    version,
    about = "Safe capture-only companion for explicit-date Drag reconciliation",
    propagate_version = true
)]
struct Cli {
    /// Directory for companion state. Defaults to .drag-companion in the current directory.
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
    /// Print a byte-stable minimized evidence bundle for one explicit local date.
    Bundle(DateArgs),
    /// Remove persisted capture-only companion state.
    Purge,
    /// Inspect scheduler lifecycle operations. These do not install anything yet.
    Scheduler(SchedulerArgs),
    /// Install, remove, or capture Claude Code SessionStart/SessionEnd hooks.
    ClaudeHook(ClaudeHookArgs),
    /// Print the machine-readable command and side-effect contract.
    Contract,
}

#[derive(Debug, Args)]
struct DateArgs {
    /// Explicit reconciliation date in YYYY-MM-DD format.
    #[arg(long, value_parser = parse_date)]
    date: NaiveDate,
}

#[derive(Debug, Args)]
struct CollectArgs {
    /// Local Git repository to scan. Repeat for each configured repository.
    #[arg(long = "repo", value_name = "DIR")]
    repos: Vec<PathBuf>,
    /// Explicit selected day for bounded local ICS expansion.
    #[arg(long, value_parser = parse_date)]
    date: Option<NaiveDate>,
    /// Local RFC 5545 .ics file to import. Repeat for each configured calendar file.
    #[arg(long = "ics", value_name = "FILE")]
    ics_files: Vec<PathBuf>,
}

#[derive(Debug, Args)]
struct SchedulerArgs {
    #[command(subcommand)]
    operation: SchedulerOperation,
}

#[derive(Debug, Args)]
struct ClaudeHookArgs {
    #[command(subcommand)]
    operation: ClaudeHookOperation,
}

#[derive(Debug, Subcommand)]
enum ClaudeHookOperation {
    /// Install SessionStart and SessionEnd capture hooks in a Claude settings JSON file.
    Install(ClaudeHookSettingsArgs),
    /// Remove only drag-companion Claude hook commands from a Claude settings JSON file.
    Remove(ClaudeHookSettingsArgs),
    /// Capture one Claude hook payload from stdin into the local journal.
    Capture,
}

#[derive(Debug, Args)]
struct ClaudeHookSettingsArgs {
    /// Claude settings JSON path to update.
    #[arg(long, value_name = "FILE")]
    settings: PathBuf,
}

#[derive(Debug, Subcommand)]
enum SchedulerOperation {
    /// Describe scheduler installation without mutating host scheduler state.
    Install,
    /// Describe scheduler enablement without mutating host scheduler state.
    Enable,
    /// Describe scheduler disablement without mutating host scheduler state.
    Disable,
    /// Describe scheduler removal without mutating host scheduler state.
    Uninstall,
    /// Show scheduler status from companion state only.
    Status,
}

#[derive(Debug, Error)]
enum CompanionError {
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to create {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to serialize result: {0}")]
    Serialize(serde_json::Error),
    #[error("invalid journal event on line {line}: {reason}")]
    InvalidJournal { line: usize, reason: String },
    #[error("sqlite store error: {0}")]
    Store(#[from] rusqlite::Error),
    #[error("failed to open {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid Claude hook payload: {0}")]
    InvalidClaudeHook(String),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Contract {
    binary: &'static str,
    default_mode: &'static str,
    config_dir: &'static str,
    data_dir: &'static str,
    adapters: Adapters,
    network_access: bool,
    live_mutation_allowed: bool,
    drag_boundary: DragBoundary,
    commands: Vec<CommandContract>,
}

#[derive(Debug, Serialize)]
struct Adapters {
    collector: &'static str,
    mutator: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DragBoundary {
    invocation: &'static str,
    schema_contract: &'static str,
    process_boundary: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandContract {
    name: &'static str,
    requires_explicit_date: bool,
    side_effects: Vec<&'static str>,
    network_access: bool,
    live_mutation_allowed: bool,
    operations: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunResult {
    date: NaiveDate,
    status: &'static str,
    mode: &'static str,
    adapters: Adapters,
    network_access: bool,
    live_mutation_allowed: bool,
    drag_boundary: DragBoundary,
    observations: Vec<FakeObservation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectResult {
    status: &'static str,
    mode: &'static str,
    adapter: &'static str,
    network_access: bool,
    git: GitCollectOutput,
    calendar: CalendarCollectOutput,
    failures: Vec<CollectFailure>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CalendarCollectOutput {
    events: Vec<CalendarEvidence>,
    failures: Vec<CollectFailure>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CalendarEvidence {
    uid: String,
    occurrence_date: NaiveDate,
    status: String,
    recurrence_id: Option<String>,
    last_modified: Option<String>,
    timezone: String,
    all_day: bool,
    interval_start: Option<String>,
    interval_end: Option<String>,
    summary: String,
    source_file: String,
    sequence: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitCollectOutput {
    commits: Vec<GitCommitEvidence>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitCommitEvidence {
    commit: String,
    author: GitIdentity,
    committer: GitIdentity,
    author_timestamp: String,
    committer_timestamp: String,
    repository: GitRepositoryIdentity,
    branch: String,
    ref_name: String,
    subject: String,
    issue_candidates: Vec<IssueCandidate>,
}

#[derive(Debug, Serialize)]
struct GitIdentity {
    name: String,
    email: String,
}

#[derive(Debug, Serialize)]
struct GitRepositoryIdentity {
    path: String,
    worktree: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueCandidate {
    key: String,
    origin: &'static str,
    confidence: &'static str,
}

#[derive(Debug, Serialize)]
struct CollectFailure {
    repository: String,
    error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FakeObservation {
    source: &'static str,
    summary: &'static str,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalEvent {
    schema_version: u32,
    event_id: String,
    event_type: String,
    observed_at: String,
    source: SourceProvenance,
    collector: CollectorProvenance,
    timestamp_semantics: TimestampSemantics,
    privacy: PrivacyState,
    retention: RetentionMetadata,
    supersedes: Option<String>,
    payload: Value,
    integrity_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceProvenance {
    kind: String,
    adapter: String,
    reference: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectorProvenance {
    name: String,
    version: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimestampSemantics {
    observed_at_source: String,
    timezone: String,
    explicit_date: NaiveDate,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrivacyState {
    classification: String,
    redacted: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetentionMetadata {
    policy: String,
    retain_until: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceBundle {
    schema_version: u32,
    explicit_date: NaiveDate,
    mode: &'static str,
    network_access: bool,
    live_mutation_allowed: bool,
    unsupported_gaps: Vec<&'static str>,
    source_health: Vec<BundleSourceHealth>,
    evidence: Vec<BundleEvidence>,
    contradictions: Vec<BundleContradiction>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BundleSourceHealth {
    source: String,
    events: usize,
    abandoned_sessions: usize,
    health: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BundleEvidence {
    id: String,
    source: String,
    reference: String,
    original_timestamp: String,
    original_timezone: String,
    observed_at_utc: Option<String>,
    interval_start_utc: Option<String>,
    interval_end_utc: Option<String>,
    elapsed_seconds: Option<i64>,
    summary: String,
    supersedes: Option<String>,
    superseded_by: Option<String>,
    contradicted_by: Vec<String>,
    abandoned_session: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BundleContradiction {
    key: String,
    evidence_ids: Vec<String>,
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("drag-companion: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), CompanionError> {
    let data_dir = cli
        .data_dir
        .unwrap_or_else(|| PathBuf::from(".drag-companion"));

    match cli.command {
        Command::Status => print_json(&serde_json::json!({
            "status": "ready", "mode": DEFAULT_MODE, "networkAccess": false,
            "liveMutationAllowed": false, "journal": journal_path(&data_dir), "store": store_path(&data_dir),
        })),
        Command::Collect(args) => {
            let result = collect_activity(&data_dir, &args)?;
            print_json(&result)
        }
        Command::Capture(args) => {
            let event = evidence_event(args.date);
            append_journal_event(&data_dir, &event)?;
            print_json(
                &serde_json::json!({ "status": "captured", "eventId": event.event_id, "journal": journal_path(&data_dir) }),
            )
        }
        Command::Import => {
            let imported = import_journal(&data_dir)?;
            print_json(
                &serde_json::json!({ "status": "imported", "imported": imported, "store": store_path(&data_dir) }),
            )
        }
        Command::Reconcile(args) => {
            let result = terminal_result(args.date);
            persist_result(&data_dir, &result)?;
            print_json(&result)
        }
        Command::Resume(args) => {
            let result = terminal_result(args.date);
            persist_result(&data_dir, &result)?;
            print_json(&result)
        }
        Command::Report(args) => {
            let path = run_path(&data_dir, args.date);
            let report = fs::read_to_string(&path).map_err(|source| CompanionError::Read {
                path: path.clone(),
                source,
            })?;
            println!("{report}");
            Ok(())
        }
        Command::Bundle(args) => {
            let bundle = build_bundle(&data_dir, args.date)?;
            print_json(&bundle)
        }
        Command::Purge => {
            let _ = fs::remove_dir_all(&data_dir);
            print_json(&serde_json::json!({ "status": "purged", "dataDir": data_dir }))
        }
        Command::Scheduler(args) => print_json(&serde_json::json!({
            "status": "described", "operation": format!("{:?}", args.operation).to_lowercase(),
            "mode": DEFAULT_MODE, "hostSchedulerMutated": false,
        })),
        Command::ClaudeHook(args) => match args.operation {
            ClaudeHookOperation::Install(args) => {
                install_claude_hooks(&args.settings)?;
                print_json(
                    &serde_json::json!({ "status": "installed", "settings": args.settings, "events": ["SessionStart", "SessionEnd"] }),
                )
            }
            ClaudeHookOperation::Remove(args) => {
                remove_claude_hooks(&args.settings)?;
                print_json(&serde_json::json!({ "status": "removed", "settings": args.settings }))
            }
            ClaudeHookOperation::Capture => {
                let event = read_claude_hook_event(&data_dir)?;
                append_journal_event(&data_dir, &event)?;
                print_json(
                    &serde_json::json!({ "status": "captured", "eventId": event.event_id, "journal": journal_path(&data_dir), "networkAccess": false }),
                )
            }
        },
        Command::Contract => print_json(&contract()),
    }
}

fn install_claude_hooks(settings_path: &Path) -> Result<(), CompanionError> {
    let mut settings = read_settings(settings_path)?;
    if !settings.is_object() {
        settings = serde_json::json!({});
    }
    let Some(settings_object) = settings.as_object_mut() else {
        return Err(CompanionError::InvalidClaudeHook(
            "settings must be a JSON object".to_owned(),
        ));
    };
    let hooks = settings_object
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let Some(hooks_object) = hooks.as_object_mut() else {
        return Err(CompanionError::InvalidClaudeHook(
            "hooks must be a JSON object".to_owned(),
        ));
    };
    for event in ["SessionStart", "SessionEnd"] {
        let list = hooks_object
            .entry(event)
            .or_insert_with(|| serde_json::json!([]));
        if !list.is_array() {
            *list = serde_json::json!([]);
        }
        let Some(arr) = list.as_array_mut() else {
            return Err(CompanionError::InvalidClaudeHook(format!(
                "{event} hooks must be an array"
            )));
        };
        if !arr.iter().any(is_our_hook_entry) {
            arr.push(serde_json::json!({
                "matcher": "*",
                "hooks": [{ "type": "command", "command": CLAUDE_HOOK_COMMAND }]
            }));
        }
    }
    write_settings(settings_path, &settings)
}

fn remove_claude_hooks(settings_path: &Path) -> Result<(), CompanionError> {
    let mut settings = read_settings(settings_path)?;
    if let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) {
        for event in ["SessionStart", "SessionEnd"] {
            if let Some(entries) = hooks.get_mut(event).and_then(Value::as_array_mut) {
                for entry in entries.iter_mut() {
                    if let Some(commands) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                        commands.retain(|command| !is_our_command(command));
                    }
                }
                entries.retain(|entry| {
                    entry
                        .get("hooks")
                        .and_then(Value::as_array)
                        .is_none_or(|commands| !commands.is_empty())
                        || !is_our_hook_entry(entry)
                });
            }
        }
    }
    write_settings(settings_path, &settings)
}

fn collect_activity(data_dir: &Path, args: &CollectArgs) -> Result<CollectResult, CompanionError> {
    let mut commits = Vec::new();
    let mut failures = Vec::new();
    let mut calendar_events = Vec::new();
    let mut calendar_failures = Vec::new();

    for repo in &args.repos {
        match scan_git_repo(repo) {
            Ok(repo_commits) => {
                for commit in repo_commits {
                    append_journal_event(data_dir, &git_commit_event(&commit)?)?;
                    commits.push(commit);
                }
            }
            Err(error) => failures.push(CollectFailure {
                repository: repo.display().to_string(),
                error,
            }),
        }
    }

    if let Some(date) = args.date {
        for path in &args.ics_files {
            match scan_ics_file(path, date) {
                Ok(events) => {
                    for event in events {
                        append_journal_event(data_dir, &calendar_event(&event)?)?;
                        calendar_events.push(event);
                    }
                }
                Err(errors) => {
                    calendar_failures.extend(errors.into_iter().map(|error| CollectFailure {
                        repository: path.display().to_string(),
                        error,
                    }))
                }
            }
        }
    }

    Ok(CollectResult {
        status: "collected",
        mode: DEFAULT_MODE,
        adapter: if args.ics_files.is_empty() {
            "git-local"
        } else {
            "local"
        },
        network_access: false,
        git: GitCollectOutput { commits },
        calendar: CalendarCollectOutput {
            events: calendar_events,
            failures: calendar_failures,
        },
        failures,
    })
}

fn scan_ics_file(path: &Path, date: NaiveDate) -> Result<Vec<CalendarEvidence>, Vec<String>> {
    let body = fs::read_to_string(path).map_err(|error| vec![error.to_string()])?;
    let lines = unfold_ics_lines(&body);
    let mut events = Vec::new();
    let mut current = Vec::new();
    let mut in_event = false;
    let mut errors = Vec::new();
    for line in lines {
        match line.as_str() {
            "BEGIN:VEVENT" => {
                if in_event {
                    errors.push("nested VEVENT".to_owned());
                }
                in_event = true;
                current.clear();
            }
            "END:VEVENT" => {
                if in_event {
                    parse_ics_event(&current, path, date, &mut events, &mut errors);
                    in_event = false;
                    current.clear();
                }
            }
            _ if in_event => current.push(line),
            _ => {}
        }
    }
    if in_event {
        errors.push("unterminated VEVENT".to_owned());
    }
    if !errors.is_empty() {
        Err(errors)
    } else {
        Ok(events)
    }
}

fn unfold_ics_lines(body: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in body.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        if raw.starts_with(' ') || raw.starts_with('\t') {
            if let Some(last) = lines.last_mut() {
                last.push_str(raw.trim_start());
            }
        } else if !raw.is_empty() {
            lines.push(raw.to_owned());
        }
    }
    lines
}

fn parse_ics_event(
    lines: &[String],
    path: &Path,
    date: NaiveDate,
    out: &mut Vec<CalendarEvidence>,
    errors: &mut Vec<String>,
) {
    let mut uid = None;
    let mut dtstart = None;
    let mut dtend = None;
    let mut status = "CONFIRMED".to_owned();
    let mut last_modified = None;
    let mut summary = String::new();
    let mut rrule = None;
    let mut recurrence_id = None;
    let mut exdates = Vec::new();
    let mut sequence = 0;
    for line in lines {
        let Some((name_params, value)) = line.split_once(':') else {
            errors.push(format!("malformed property {line}"));
            continue;
        };
        let (name, params) = name_params.split_once(';').unwrap_or((name_params, ""));
        match name {
            "UID" => uid = Some(value.to_owned()),
            "DTSTART" => dtstart = Some((params.to_owned(), value.to_owned())),
            "DTEND" => dtend = Some((params.to_owned(), value.to_owned())),
            "STATUS" => status = value.to_owned(),
            "LAST-MODIFIED" => last_modified = normalize_ics_utc(value),
            "SUMMARY" => summary = value.to_owned(),
            "RRULE" => rrule = Some(value.to_owned()),
            "RECURRENCE-ID" => recurrence_id = Some(value.to_owned()),
            "EXDATE" => exdates.extend(value.split(',').map(ToOwned::to_owned)),
            "SEQUENCE" => sequence = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    if status == "CANCELLED" {
        return;
    }
    let Some(uid) = uid else {
        errors.push("missing UID".to_owned());
        return;
    };
    let Some((start_params, start_value)) = dtstart else {
        errors.push(format!("{uid}: missing DTSTART"));
        return;
    };
    let all_day = start_params.contains("VALUE=DATE");
    let timezone = if all_day {
        "all-day".to_owned()
    } else if let Some(tzid) = param_value(&start_params, "TZID") {
        tzid
    } else {
        errors.push("floating time requires explicit timezone".to_owned());
        return;
    };
    let duration = dtend
        .as_ref()
        .and_then(|(params, value)| {
            event_duration(
                &start_params,
                &start_value,
                params,
                value,
                all_day,
                &timezone,
            )
        })
        .unwrap_or_else(|| Duration::hours(1));
    let starts = occurrence_starts(
        &start_value,
        all_day,
        &timezone,
        rrule.as_deref(),
        &exdates,
        date,
        errors,
    );
    for (occurrence_date, start_utc) in starts {
        let (interval_start, interval_end) = if all_day {
            (None, None)
        } else {
            (
                Some(start_utc.to_rfc3339_opts(SecondsFormat::Secs, true)),
                Some((start_utc + duration).to_rfc3339_opts(SecondsFormat::Secs, true)),
            )
        };
        out.push(CalendarEvidence {
            uid: uid.clone(),
            occurrence_date,
            status: status.clone(),
            recurrence_id: recurrence_id
                .clone()
                .or_else(|| rrule.clone().map(|_| occurrence_date.to_string())),
            last_modified: last_modified.clone(),
            timezone: timezone.clone(),
            all_day,
            interval_start,
            interval_end,
            summary: summary.clone(),
            source_file: path.display().to_string(),
            sequence,
        });
    }
    out.sort_by(|a, b| {
        (&a.uid, a.sequence, &a.last_modified).cmp(&(&b.uid, b.sequence, &b.last_modified))
    });
}

fn param_value(params: &str, key: &str) -> Option<String> {
    params
        .split(';')
        .find_map(|part| part.strip_prefix(&format!("{key}=")).map(ToOwned::to_owned))
}

fn occurrence_starts(
    raw: &str,
    all_day: bool,
    timezone: &str,
    rrule: Option<&str>,
    exdates: &[String],
    date: NaiveDate,
    errors: &mut Vec<String>,
) -> Vec<(NaiveDate, DateTime<Utc>)> {
    let Some(first) = parse_ics_start(raw, all_day, timezone, errors) else {
        return Vec::new();
    };
    let count = rrule
        .and_then(|rule| {
            rule.split(';')
                .find_map(|part| part.strip_prefix("COUNT=")?.parse::<usize>().ok())
        })
        .unwrap_or(1)
        .min(400);
    let daily = rrule.is_some_and(|rule| rule.contains("FREQ=DAILY"));
    let mut starts = Vec::new();
    for index in 0..count {
        let candidate = if daily {
            first + Duration::days(index as i64)
        } else {
            first
        };
        if exdates.iter().any(|exdate| {
            parse_ics_start(exdate, all_day, timezone, &mut Vec::new()) == Some(candidate)
        }) {
            continue;
        }
        let local_date = if all_day {
            candidate.date_naive()
        } else {
            candidate
                .with_timezone(&timezone.parse::<Tz>().unwrap_or(chrono_tz::UTC))
                .date_naive()
        };
        if local_date == date {
            starts.push((local_date, candidate));
        }
        if !daily {
            break;
        }
    }
    starts
}

fn parse_ics_start(
    raw: &str,
    all_day: bool,
    timezone: &str,
    errors: &mut Vec<String>,
) -> Option<DateTime<Utc>> {
    if all_day {
        return NaiveDate::parse_from_str(raw, "%Y%m%d")
            .ok()?
            .and_hms_opt(0, 0, 0)
            .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
    }
    let tz: Tz = match timezone.parse() {
        Ok(tz) => tz,
        Err(_) => {
            errors.push(format!("unknown timezone {timezone}"));
            return None;
        }
    };
    let naive = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S").ok()?;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
        LocalResult::Ambiguous(early, _) => Some(early.with_timezone(&Utc)),
        LocalResult::None => Some(tz.from_utc_datetime(&naive).with_timezone(&Utc)),
    }
}

fn event_duration(
    _start_params: &str,
    start: &str,
    end_params: &str,
    end: &str,
    all_day: bool,
    timezone: &str,
) -> Option<Duration> {
    if !all_day {
        let s = NaiveDateTime::parse_from_str(start, "%Y%m%dT%H%M%S").ok()?;
        let e = NaiveDateTime::parse_from_str(end, "%Y%m%dT%H%M%S").ok()?;
        return Some(e - s);
    }
    let mut errors = Vec::new();
    let s = parse_ics_start(start, all_day, timezone, &mut errors)?;
    let end_tz = param_value(end_params, "TZID").unwrap_or_else(|| timezone.to_owned());
    let e = parse_ics_start(
        end,
        all_day || end_params.contains("VALUE=DATE"),
        &end_tz,
        &mut errors,
    )?;
    Some(e - s)
}

fn normalize_ics_utc(raw: &str) -> Option<String> {
    if let Some(stripped) = raw.strip_suffix('Z') {
        NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S")
            .ok()
            .map(|dt| {
                DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc)
                    .to_rfc3339_opts(SecondsFormat::Secs, true)
            })
    } else {
        None
    }
}

fn calendar_event(calendar: &CalendarEvidence) -> Result<JournalEvent, CompanionError> {
    let occurrence = calendar
        .recurrence_id
        .as_deref()
        .unwrap_or(&calendar.occurrence_date.to_string())
        .replace(':', "");
    let event_id = format!(
        "evidence.ics.{}.{}.{}",
        calendar.uid.replace(['/', '#', ' '], "_"),
        occurrence,
        calendar.sequence
    );
    let supersedes = (calendar.sequence > 1).then(|| {
        format!(
            "evidence.ics.{}.{}.{}",
            calendar.uid.replace(['/', '#', ' '], "_"),
            occurrence,
            calendar.sequence - 1
        )
    });
    let mut event = JournalEvent {
        schema_version: JOURNAL_SCHEMA_VERSION,
        event_id,
        event_type: "calendar.ics.event".to_owned(),
        observed_at: calendar
            .last_modified
            .clone()
            .or_else(|| calendar.interval_start.clone())
            .unwrap_or_else(now_string),
        source: SourceProvenance {
            kind: "calendar".to_owned(),
            adapter: "ics-local".to_owned(),
            reference: format!("{}#{}", calendar.uid, calendar.occurrence_date),
        },
        collector: CollectorProvenance {
            name: "ics-local".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        timestamp_semantics: TimestampSemantics {
            observed_at_source: "ics-dtstart".to_owned(),
            timezone: calendar.timezone.clone(),
            explicit_date: calendar.occurrence_date,
        },
        privacy: PrivacyState {
            classification: "local-calendar-metadata".to_owned(),
            redacted: false,
        },
        retention: RetentionMetadata {
            policy: "retain-until-user-purge".to_owned(),
            retain_until: None,
        },
        supersedes,
        payload: serde_json::to_value(calendar).map_err(CompanionError::Serialize)?,
        integrity_hash: String::new(),
    };
    event.payload["intervalStart"] = calendar.interval_start.clone().into();
    event.payload["intervalEnd"] = calendar.interval_end.clone().into();
    event.payload["summary"] = serde_json::json!(calendar.summary);
    event.integrity_hash = event_hash(&event).map_err(CompanionError::Serialize)?;
    Ok(event)
}

fn scan_git_repo(repo: &Path) -> Result<Vec<GitCommitEvidence>, String> {
    if !repo.exists() {
        return Err("repository path does not exist".to_owned());
    }
    let worktree = git_stdout(repo, ["rev-parse", "--show-toplevel"])?;
    let branch = git_stdout(repo, ["branch", "--show-current"])
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "DETACHED".to_owned());
    let ref_name = git_stdout(
        repo,
        ["rev-parse", "--symbolic-full-name", "--quiet", "HEAD"],
    )
    .ok()
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "HEAD".to_owned());
    let output = git_stdout(
        repo,
        [
            "log",
            "--all",
            "--max-count=200",
            "--date=iso-strict",
            "--format=%H%x1f%an%x1f%ae%x1f%cn%x1f%ce%x1f%aI%x1f%cI%x1f%s%x1e",
        ],
    )?;
    let mut commits = Vec::new();
    for record in output
        .split('\u{1e}')
        .filter(|record| !record.trim().is_empty())
    {
        let fields: Vec<&str> = record.trim_matches('\n').split('\u{1f}').collect();
        if fields.len() != 8 {
            return Err("unexpected git log format".to_owned());
        }
        let subject = minimize_subject(fields[7]);
        commits.push(GitCommitEvidence {
            commit: fields[0].to_owned(),
            author: GitIdentity {
                name: fields[1].to_owned(),
                email: fields[2].to_owned(),
            },
            committer: GitIdentity {
                name: fields[3].to_owned(),
                email: fields[4].to_owned(),
            },
            author_timestamp: fields[5].to_owned(),
            committer_timestamp: fields[6].to_owned(),
            repository: GitRepositoryIdentity {
                path: repo.display().to_string(),
                worktree: worktree.clone(),
            },
            branch: branch.clone(),
            ref_name: ref_name.clone(),
            issue_candidates: issue_candidates(&subject),
            subject,
        });
    }
    Ok(commits)
}

fn git_stdout<const N: usize>(repo: &Path, args: [&str; N]) -> Result<String, String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn minimize_subject(subject: &str) -> String {
    const MAX: usize = 72;
    let clean = subject.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.len() <= MAX {
        clean
    } else {
        let mut minimized = String::new();
        for ch in clean.chars() {
            if minimized.len() + ch.len_utf8() + 3 > MAX {
                break;
            }
            minimized.push(ch);
        }
        minimized.push('…');
        minimized
    }
}

fn issue_candidates(subject: &str) -> Vec<IssueCandidate> {
    subject
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .filter(|part| {
            let Some((project, number)) = part.split_once('-') else {
                return false;
            };
            project.len() >= 2
                && project.chars().all(|ch| ch.is_ascii_uppercase())
                && number.chars().all(|ch| ch.is_ascii_digit())
        })
        .map(|key| IssueCandidate {
            key: key.to_owned(),
            origin: "commit-subject",
            confidence: "candidate",
        })
        .collect()
}

fn git_commit_event(commit: &GitCommitEvidence) -> Result<JournalEvent, CompanionError> {
    let explicit_date = commit
        .author_timestamp
        .get(..10)
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .unwrap_or_else(|| Utc::now().date_naive());
    let mut event = JournalEvent {
        schema_version: JOURNAL_SCHEMA_VERSION,
        event_id: format!("evidence.git.{}", commit.commit),
        event_type: "git.commit".to_owned(),
        observed_at: commit.author_timestamp.clone(),
        source: SourceProvenance {
            kind: "git".to_owned(),
            adapter: "git-local".to_owned(),
            reference: format!("{}@{}", commit.repository.worktree, commit.commit),
        },
        collector: CollectorProvenance {
            name: "git-local".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        timestamp_semantics: TimestampSemantics {
            observed_at_source: "git-author-timestamp".to_owned(),
            timezone: "from-git-offset".to_owned(),
            explicit_date,
        },
        privacy: PrivacyState {
            classification: "local-git-metadata".to_owned(),
            redacted: false,
        },
        retention: RetentionMetadata {
            policy: "retain-until-user-purge".to_owned(),
            retain_until: None,
        },
        supersedes: None,
        payload: serde_json::to_value(commit).map_err(CompanionError::Serialize)?,
        integrity_hash: String::new(),
    };
    event.integrity_hash = event_hash(&event).map_err(CompanionError::Serialize)?;
    Ok(event)
}

fn read_settings(path: &Path) -> Result<Value, CompanionError> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let body = fs::read_to_string(path).map_err(|source| CompanionError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&body)
        .map_err(|error| CompanionError::InvalidClaudeHook(error.to_string()))
}

fn write_settings(path: &Path, settings: &Value) -> Result<(), CompanionError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CompanionError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let body = serde_json::to_vec_pretty(settings).map_err(CompanionError::Serialize)?;
    fs::write(path, body).map_err(|source| CompanionError::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn is_our_hook_entry(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|commands| commands.iter().any(is_our_command))
}

fn is_our_command(command: &Value) -> bool {
    command
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(CLAUDE_HOOK_COMMAND))
}

fn read_claude_hook_event(data_dir: &Path) -> Result<JournalEvent, CompanionError> {
    let mut stdin = String::new();
    io::stdin()
        .read_to_string(&mut stdin)
        .map_err(|source| CompanionError::Read {
            path: PathBuf::from("<stdin>"),
            source,
        })?;
    let payload: Value = serde_json::from_str(&stdin)
        .map_err(|error| CompanionError::InvalidClaudeHook(error.to_string()))?;
    claude_hook_event_from_payload(data_dir, &payload)
}

fn claude_hook_event_from_payload(
    _data_dir: &Path,
    payload: &Value,
) -> Result<JournalEvent, CompanionError> {
    let kind = payload
        .get("hook_event_name")
        .or_else(|| payload.get("event"))
        .or_else(|| payload.get("hookEventName"))
        .and_then(Value::as_str)
        .ok_or_else(|| CompanionError::InvalidClaudeHook("missing lifecycle event".to_owned()))?;
    if !matches!(kind, "SessionStart" | "SessionEnd") {
        return Err(CompanionError::InvalidClaudeHook(format!(
            "unsupported lifecycle event {kind}"
        )));
    }
    let session_id = payload
        .get("session_id")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CompanionError::InvalidClaudeHook("missing session id".to_owned()))?;
    let observed_at = payload
        .get("timestamp")
        .or_else(|| payload.get("observed_at"))
        .or_else(|| payload.get("observedAt"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(now_string);
    let explicit_date = normalize_timestamp(&observed_at)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(&timestamp).ok())
        .map(|timestamp| timestamp.date_naive())
        .unwrap_or_else(|| Utc::now().date_naive());
    let cwd = payload
        .get("cwd")
        .or_else(|| payload.get("workspace"))
        .and_then(Value::as_str);
    let repo = find_repo_link(cwd).unwrap_or_else(|| "unknown".to_owned());
    let mut lifecycle_payload = serde_json::json!({
        "schemaVersion": CLAUDE_HOOK_SCHEMA_VERSION,
        "lifecycleKind": kind,
        "sessionId": session_id,
        "observedAt": observed_at,
        "repository": repo,
        "summary": format!("Claude Code {kind} captured locally for repository {repo}"),
        "networkAccess": false,
        "transcriptCaptured": false,
    });
    if kind == "SessionStart" {
        lifecycle_payload["intervalStart"] = serde_json::json!(observed_at);
    } else {
        lifecycle_payload["intervalEnd"] = serde_json::json!(observed_at);
    }
    let mut event = JournalEvent {
        schema_version: JOURNAL_SCHEMA_VERSION,
        event_id: format!("evidence.claude.{session_id}.{kind}"),
        event_type: "evidence.claude.lifecycle".to_owned(),
        observed_at: observed_at.clone(),
        source: SourceProvenance {
            kind: "claude-code".to_owned(),
            adapter: CLAUDE_COLLECTOR.to_owned(),
            reference: format!("{repo}#{session_id}"),
        },
        collector: CollectorProvenance {
            name: CLAUDE_COLLECTOR.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        timestamp_semantics: TimestampSemantics {
            observed_at_source: observed_at,
            timezone: "UTC".to_owned(),
            explicit_date,
        },
        privacy: PrivacyState {
            classification: "local-metadata".to_owned(),
            redacted: true,
        },
        retention: RetentionMetadata {
            policy: "retain-until-user-purge".to_owned(),
            retain_until: None,
        },
        supersedes: None,
        payload: lifecycle_payload,
        integrity_hash: String::new(),
    };
    event.integrity_hash = event_hash(&event).unwrap_or_default();
    Ok(event)
}

fn find_repo_link(cwd: Option<&str>) -> Option<String> {
    let cwd = Path::new(cwd?);
    cwd.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_date(raw: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| "date must use YYYY-MM-DD".to_owned())
}

fn print_json<T: Serialize>(value: &T) -> Result<(), CompanionError> {
    let body = serde_json::to_string_pretty(value).map_err(CompanionError::Serialize)?;
    println!("{body}");
    Ok(())
}

fn append_journal_event(data_dir: &Path, event: &JournalEvent) -> Result<(), CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    let path = journal_path(data_dir);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|source| CompanionError::Open {
            path: path.clone(),
            source,
        })?;
    let mut body = serde_json::to_vec(event).map_err(CompanionError::Serialize)?;
    body.push(b'\n');
    file.write_all(&body)
        .map_err(|source| CompanionError::Write {
            path: path.clone(),
            source,
        })?;
    file.sync_data()
        .map_err(|source| CompanionError::Write { path, source })
}

fn import_journal(data_dir: &Path) -> Result<usize, CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    let mut conn = Connection::open(store_path(data_dir))?;
    migrate(&mut conn)?;
    let path = journal_path(data_dir);
    if !path.exists() {
        return Ok(0);
    }
    let file = File::open(&path).map_err(|source| CompanionError::Open { path, source })?;
    let tx = conn.transaction()?;
    let mut imported = 0;
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line_number = index + 1;
        let line = line.map_err(|source| CompanionError::Read {
            path: journal_path(data_dir),
            source,
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let event: JournalEvent =
            serde_json::from_str(&line).map_err(|error| CompanionError::InvalidJournal {
                line: line_number,
                reason: error.to_string(),
            })?;
        validate_event(&tx, &event, line_number)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO evidence_events (event_id, event_type, observed_at, source_kind, source_adapter, source_reference, collector_name, collector_version, timestamp_source, timezone, explicit_date, privacy_classification, privacy_redacted, retention_policy, retain_until, supersedes, payload_json, integrity_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![event.event_id, event.event_type, event.observed_at, event.source.kind, event.source.adapter, event.source.reference, event.collector.name, event.collector.version, event.timestamp_semantics.observed_at_source, event.timestamp_semantics.timezone, event.timestamp_semantics.explicit_date.to_string(), event.privacy.classification, event.privacy.redacted, event.retention.policy, event.retention.retain_until, event.supersedes, event.payload.to_string(), event.integrity_hash],
        )?;
        imported += inserted;
    }
    tx.commit()?;
    Ok(imported)
}

fn validate_event(
    conn: &Connection,
    event: &JournalEvent,
    line: usize,
) -> Result<(), CompanionError> {
    if event.schema_version != JOURNAL_SCHEMA_VERSION {
        return Err(CompanionError::InvalidJournal {
            line,
            reason: format!("unsupported schemaVersion {}", event.schema_version),
        });
    }
    let expected = event_hash(event).map_err(CompanionError::Serialize)?;
    if event.integrity_hash != expected {
        return Err(CompanionError::InvalidJournal {
            line,
            reason: "integrity hash mismatch".to_owned(),
        });
    }
    let existing_hash: Option<String> = conn
        .query_row(
            "SELECT integrity_hash FROM evidence_events WHERE event_id = ?1",
            [&event.event_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing_hash) = existing_hash {
        if existing_hash != event.integrity_hash {
            return Err(CompanionError::InvalidJournal {
                line,
                reason: format!(
                    "duplicate eventId {} has different integrity hash",
                    event.event_id
                ),
            });
        }
    }
    if let Some(supersedes) = &event.supersedes {
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM evidence_events WHERE event_id = ?1",
                [supersedes],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(CompanionError::InvalidJournal {
                line,
                reason: format!("supersedes unknown event {supersedes}"),
            });
        }
    }
    Ok(())
}

fn migrate(conn: &mut Connection) -> Result<(), CompanionError> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\
         CREATE TABLE IF NOT EXISTS evidence_events (event_id TEXT PRIMARY KEY, event_type TEXT NOT NULL, observed_at TEXT NOT NULL, source_kind TEXT NOT NULL, source_adapter TEXT NOT NULL, source_reference TEXT NOT NULL, collector_name TEXT NOT NULL, collector_version TEXT NOT NULL, timestamp_source TEXT NOT NULL, timezone TEXT NOT NULL, explicit_date TEXT NOT NULL, privacy_classification TEXT NOT NULL, privacy_redacted INTEGER NOT NULL CHECK (privacy_redacted IN (0, 1)), retention_policy TEXT NOT NULL, retain_until TEXT, supersedes TEXT REFERENCES evidence_events(event_id), payload_json TEXT NOT NULL, integrity_hash TEXT NOT NULL UNIQUE);\
         CREATE TABLE IF NOT EXISTS issue_candidates (id TEXT PRIMARY KEY, evidence_event_id TEXT NOT NULL REFERENCES evidence_events(event_id), issue_key TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('proposed','approved','rejected','skipped','failed','uncertain')));\
         CREATE TABLE IF NOT EXISTS daily_bundles (id TEXT PRIMARY KEY, explicit_date TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('proposed','approved','submitting','confirmed','rejected','skipped','failed','uncertain')));\
         CREATE TABLE IF NOT EXISTS proposals (id TEXT PRIMARY KEY, bundle_id TEXT NOT NULL REFERENCES daily_bundles(id), state TEXT NOT NULL CHECK (state IN ('proposed','approved','submitting','confirmed','rejected','skipped','failed','uncertain')));\
         CREATE TABLE IF NOT EXISTS unsupported_periods (id TEXT PRIMARY KEY, explicit_date TEXT NOT NULL, reason TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('proposed','confirmed','skipped','failed','uncertain')));\
         CREATE TABLE IF NOT EXISTS policy_decisions (id TEXT PRIMARY KEY, proposal_id TEXT REFERENCES proposals(id), decision TEXT NOT NULL CHECK (decision IN ('approved','rejected','skipped','uncertain')), decided_at TEXT NOT NULL);\
         CREATE TABLE IF NOT EXISTS runs (id TEXT PRIMARY KEY, explicit_date TEXT NOT NULL, state TEXT NOT NULL CHECK (state IN ('proposed','approved','submitting','confirmed','rejected','skipped','failed','uncertain')), started_at TEXT NOT NULL, finished_at TEXT);\
         CREATE TABLE IF NOT EXISTS leases (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), state TEXT NOT NULL CHECK (state IN ('proposed','approved','confirmed','rejected','skipped','failed','uncertain')), expires_at TEXT NOT NULL);\
         CREATE TABLE IF NOT EXISTS mutation_operations (id TEXT PRIMARY KEY, proposal_id TEXT REFERENCES proposals(id), state TEXT NOT NULL CHECK (state IN ('proposed','approved','submitting','confirmed','rejected','skipped','failed','uncertain')), idempotency_key TEXT NOT NULL UNIQUE);\
         CREATE TABLE IF NOT EXISTS mutation_attempts (id TEXT PRIMARY KEY, operation_id TEXT NOT NULL REFERENCES mutation_operations(id), state TEXT NOT NULL CHECK (state IN ('proposed','approved','submitting','confirmed','rejected','skipped','failed','uncertain')), attempted_at TEXT NOT NULL);\
         CREATE TABLE IF NOT EXISTS reports (id TEXT PRIMARY KEY, run_id TEXT REFERENCES runs(id), state TEXT NOT NULL CHECK (state IN ('proposed','approved','confirmed','rejected','skipped','failed','uncertain')), body_json TEXT NOT NULL);"
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
        params![STORE_SCHEMA_VERSION, now_string()],
    )?;
    tx.commit()?;
    Ok(())
}

fn evidence_event(date: NaiveDate) -> JournalEvent {
    let mut event = JournalEvent {
        schema_version: JOURNAL_SCHEMA_VERSION,
        event_id: format!("evidence.fake.{date}"),
        event_type: "evidence.captured".to_owned(),
        observed_at: now_string(),
        source: SourceProvenance {
            kind: "fixture".to_owned(),
            adapter: COLLECTOR_ADAPTER.to_owned(),
            reference: date.to_string(),
        },
        collector: CollectorProvenance {
            name: COLLECTOR_ADAPTER.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        timestamp_semantics: TimestampSemantics {
            observed_at_source: "collector-clock".to_owned(),
            timezone: "UTC".to_owned(),
            explicit_date: date,
        },
        privacy: PrivacyState {
            classification: "local-fixture".to_owned(),
            redacted: false,
        },
        retention: RetentionMetadata {
            policy: "retain-until-user-purge".to_owned(),
            retain_until: None,
        },
        supersedes: None,
        payload: serde_json::json!({ "summary": "fake explicit-date capture completed without network or live mutation", "networkAccess": false, "liveMutationAllowed": false }),
        integrity_hash: String::new(),
    };
    event.integrity_hash = event_hash(&event).unwrap_or_default();
    event
}

fn event_hash(event: &JournalEvent) -> Result<String, serde_json::Error> {
    let hash_body = serde_json::json!({
        "schemaVersion": event.schema_version, "eventId": event.event_id, "eventType": event.event_type,
        "observedAt": event.observed_at, "source": event.source, "collector": event.collector,
        "timestampSemantics": event.timestamp_semantics, "privacy": event.privacy,
        "retention": event.retention, "supersedes": event.supersedes, "payload": event.payload,
    });
    let encoded = serde_json::to_vec(&hash_body)?;
    Ok(format!("sha256:{:x}", Sha256::digest(encoded)))
}

fn build_bundle(data_dir: &Path, date: NaiveDate) -> Result<EvidenceBundle, CompanionError> {
    let mut conn = Connection::open(store_path(data_dir))?;
    migrate(&mut conn)?;
    let mut stmt = conn.prepare(
        "SELECT event_id, source_adapter, source_reference, timestamp_source, timezone, supersedes, payload_json \
         FROM evidence_events WHERE explicit_date = ?1 ORDER BY event_id ASC",
    )?;
    let rows = stmt.query_map([date.to_string()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;

    let mut evidence = Vec::new();
    for row in rows {
        let (
            id,
            source,
            reference,
            original_timestamp,
            original_timezone,
            supersedes,
            payload_json,
        ) = row?;
        let payload: Value =
            serde_json::from_str(&payload_json).map_err(CompanionError::Serialize)?;
        let interval_start = payload.get("intervalStart").and_then(Value::as_str);
        let interval_end = payload.get("intervalEnd").and_then(Value::as_str);
        let point = payload
            .get("observedAt")
            .and_then(Value::as_str)
            .unwrap_or(&original_timestamp)
            .to_owned();
        let summary = payload.get("summary").and_then(Value::as_str).unwrap_or("");
        let start_utc = interval_start.and_then(normalize_timestamp);
        let end_utc = interval_end.and_then(normalize_timestamp);
        let elapsed_seconds = match (interval_start, interval_end) {
            (Some(start), Some(end)) => elapsed(start, end),
            _ => None,
        };
        evidence.push(BundleEvidence {
            id,
            source,
            reference,
            original_timestamp,
            original_timezone,
            observed_at_utc: normalize_timestamp(&point),
            interval_start_utc: start_utc,
            interval_end_utc: end_utc,
            elapsed_seconds,
            summary: redact(summary),
            supersedes,
            superseded_by: None,
            contradicted_by: Vec::new(),
            abandoned_session: interval_start.is_some() && interval_end.is_none(),
        });
    }
    evidence.sort_by(|left, right| left.id.cmp(&right.id));

    for index in 0..evidence.len() {
        let replacement_id = evidence[index].id.clone();
        if let Some(supersedes) = evidence[index].supersedes.clone() {
            if let Some(target) = evidence.iter_mut().find(|item| item.id == supersedes) {
                target.superseded_by = Some(replacement_id);
            }
        }
    }

    let mut contradictions = Vec::new();
    let mut by_key = std::collections::BTreeMap::<String, Vec<String>>::new();
    for item in &evidence {
        if let Some(key) = item.reference.split('#').next() {
            by_key
                .entry(key.to_owned())
                .or_default()
                .push(item.id.clone());
        }
    }
    for (key, ids) in by_key.into_iter().filter(|(_, ids)| ids.len() > 1) {
        for id in &ids {
            if let Some(item) = evidence.iter_mut().find(|item| &item.id == id) {
                item.contradicted_by = ids.iter().filter(|other| *other != id).cloned().collect();
            }
        }
        contradictions.push(BundleContradiction {
            key,
            evidence_ids: ids,
        });
    }

    let mut health = std::collections::BTreeMap::<String, (usize, usize)>::new();
    for item in &evidence {
        let entry = health.entry(item.source.clone()).or_default();
        entry.0 += 1;
        if item.abandoned_session {
            entry.1 += 1;
        }
    }
    let source_health = health
        .into_iter()
        .map(
            |(source, (events, abandoned_sessions))| BundleSourceHealth {
                source,
                events,
                abandoned_sessions,
                health: if abandoned_sessions > 0 {
                    "degraded"
                } else {
                    "healthy"
                },
            },
        )
        .collect();

    Ok(EvidenceBundle {
        schema_version: 1,
        explicit_date: date,
        mode: DEFAULT_MODE,
        network_access: false,
        live_mutation_allowed: false,
        unsupported_gaps: vec!["collectors-deferred", "model-export-only"],
        source_health,
        evidence,
        contradictions,
    })
}

fn normalize_timestamp(raw: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(raw).ok().map(|timestamp| {
        timestamp
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    })
}

fn elapsed(start: &str, end: &str) -> Option<i64> {
    let start = DateTime::parse_from_rfc3339(start).ok()?;
    let end = DateTime::parse_from_rfc3339(end).ok()?;
    Some((end - start).num_seconds())
}

fn redact(raw: &str) -> String {
    raw.split_whitespace()
        .filter(|word| {
            let lower = word.to_ascii_lowercase();
            !(lower.contains("token=")
                || lower.contains("password=")
                || lower.contains("secret")
                || lower.contains("/home/")
                || lower.contains("transcript")
                || lower.contains("ignore")
                || lower.contains("instruction"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn persist_result(data_dir: &Path, result: &RunResult) -> Result<(), CompanionError> {
    let runs_dir = data_dir.join("runs");
    fs::create_dir_all(&runs_dir).map_err(|source| CompanionError::CreateDir {
        path: runs_dir.clone(),
        source,
    })?;
    let path = run_path(data_dir, result.date);
    let body = serde_json::to_vec_pretty(result).map_err(CompanionError::Serialize)?;
    fs::write(&path, body).map_err(|source| CompanionError::Write { path, source })
}

fn journal_path(data_dir: &Path) -> PathBuf {
    data_dir.join("journal.jsonl")
}
fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("companion.sqlite3")
}
fn run_path(data_dir: &Path, date: NaiveDate) -> PathBuf {
    data_dir.join("runs").join(format!("{date}.json"))
}
fn now_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn terminal_result(date: NaiveDate) -> RunResult {
    RunResult {
        date,
        status: "terminal",
        mode: DEFAULT_MODE,
        adapters: adapters(),
        network_access: false,
        live_mutation_allowed: false,
        drag_boundary: drag_boundary(),
        observations: vec![FakeObservation {
            source: COLLECTOR_ADAPTER,
            summary: "fake explicit-date capture completed without network or live mutation",
        }],
    }
}

fn contract() -> Contract {
    Contract {
        binary: "drag-companion",
        default_mode: DEFAULT_MODE,
        config_dir: "$DRAG_COMPANION_CONFIG or .drag-companion/config.json",
        data_dir: "$DRAG_COMPANION_DATA or .drag-companion",
        adapters: adapters(),
        network_access: false,
        live_mutation_allowed: false,
        drag_boundary: drag_boundary(),
        commands: vec![
            command("status", false, vec![], vec![]),
            command("collect", false, vec!["capture fake observations"], vec![]),
            command(
                "capture",
                true,
                vec!["append one immutable evidence event to journal"],
                vec![],
            ),
            command(
                "import",
                false,
                vec!["migrate sqlite store", "import journal events idempotently"],
                vec![],
            ),
            command("reconcile", true, vec!["write terminal run result"], vec![]),
            command("resume", true, vec!["write terminal run result"], vec![]),
            command("report", true, vec![], vec![]),
            command(
                "bundle",
                true,
                vec!["read imported evidence and print minimized daily bundle"],
                vec![],
            ),
            command(
                "purge",
                false,
                vec!["delete companion data directory"],
                vec![],
            ),
            command(
                "scheduler",
                false,
                vec![],
                vec!["install", "enable", "disable", "uninstall", "status"],
            ),
            command(
                "claude-hook",
                false,
                vec![
                    "install SessionStart and SessionEnd capture hooks while preserving unrelated Claude settings",
                    "remove only drag-companion Claude hook commands",
                    "append local Claude lifecycle metadata from stdin without transcript capture",
                ],
                vec!["install", "remove", "capture"],
            ),
        ],
    }
}

fn command(
    name: &'static str,
    requires_explicit_date: bool,
    side_effects: Vec<&'static str>,
    operations: Vec<&'static str>,
) -> CommandContract {
    CommandContract {
        name,
        requires_explicit_date,
        side_effects,
        network_access: false,
        live_mutation_allowed: false,
        operations,
    }
}

fn adapters() -> Adapters {
    Adapters {
        collector: COLLECTOR_ADAPTER,
        mutator: MUTATOR_ADAPTER,
    }
}
fn drag_boundary() -> DragBoundary {
    DragBoundary {
        invocation: "drag public CLI process",
        schema_contract: "drag schema",
        process_boundary: true,
    }
}
