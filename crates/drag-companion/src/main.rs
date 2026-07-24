use std::fs;
use std::path::PathBuf;

use chrono::NaiveDate;
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use thiserror::Error;

const DEFAULT_MODE: &str = "capture-only";
const COLLECTOR_ADAPTER: &str = "fake";
const MUTATOR_ADAPTER: &str = "disabled";

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
            "status": "ready",
            "mode": DEFAULT_MODE,
            "networkAccess": false,
            "liveMutationAllowed": false,
        })),
        Command::Collect => print_json(&serde_json::json!({
            "status": "collected",
            "mode": DEFAULT_MODE,
            "adapter": COLLECTOR_ADAPTER,
            "networkAccess": false,
        })),
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
            "status": "described",
            "operation": format!("{:?}", args.operation).to_lowercase(),
            "mode": DEFAULT_MODE,
            "hostSchedulerMutated": false,
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

fn persist_result(data_dir: &std::path::Path, result: &RunResult) -> Result<(), CompanionError> {
    let runs_dir = data_dir.join("runs");
    fs::create_dir_all(&runs_dir).map_err(|source| CompanionError::CreateDir {
        path: runs_dir.clone(),
        source,
    })?;
    let path = run_path(data_dir, result.date);
    let body = serde_json::to_vec_pretty(result).map_err(CompanionError::Serialize)?;
    fs::write(&path, body).map_err(|source| CompanionError::Write { path, source })
}

fn run_path(data_dir: &std::path::Path, date: NaiveDate) -> PathBuf {
    data_dir.join("runs").join(format!("{date}.json"))
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
