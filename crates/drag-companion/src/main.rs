use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, SecondsFormat, Utc};
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
    Collect,
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
    /// Remove persisted capture-only companion state.
    Purge,
    /// Inspect scheduler lifecycle operations. These do not install anything yet.
    Scheduler(SchedulerArgs),
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
struct SchedulerArgs {
    #[command(subcommand)]
    operation: SchedulerOperation,
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
        Command::Collect => print_json(&serde_json::json!({
            "status": "collected", "mode": DEFAULT_MODE, "adapter": COLLECTOR_ADAPTER, "networkAccess": false,
        })),
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
        Command::Purge => {
            let _ = fs::remove_dir_all(&data_dir);
            print_json(&serde_json::json!({ "status": "purged", "dataDir": data_dir }))
        }
        Command::Scheduler(args) => print_json(&serde_json::json!({
            "status": "described", "operation": format!("{:?}", args.operation).to_lowercase(),
            "mode": DEFAULT_MODE, "hostSchedulerMutated": false,
        })),
        Command::Contract => print_json(&contract()),
    }
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
