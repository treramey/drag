use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::Instant;

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone,
    Timelike, Utc,
};
use chrono_tz::Tz;
use clap::{Args, Parser, Subcommand, ValueEnum};
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

const DEFAULT_MODE: &str = "capture-only";
const COLLECTOR_ADAPTER: &str = "fake";
const MUTATOR_ADAPTER: &str = "disabled";
const JOURNAL_SCHEMA_VERSION: u32 = 1;
const STORE_SCHEMA_VERSION: i64 = 2;
const CLAUDE_HOOK_SCHEMA_VERSION: u32 = 1;
const CLAUDE_COLLECTOR: &str = "claude-code-session-hook";
const PROPOSAL_SCHEMA_VERSION: u32 = 1;
const POLICY_SCHEMA_VERSION: u32 = 1;
const PROPOSAL_ADAPTER: &str = "provider-fixture";
const MAX_BUNDLE_BYTES: usize = 128 * 1024;
const MAX_PROVIDER_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_PROVIDER_ATTEMPTS: u32 = 2;
const CLAUDE_HOOK_COMMAND: &str = "drag-tracking internal claude-hook capture";
const LEGACY_CLAUDE_HOOK_COMMAND: &str = "drag-companion claude-hook capture";
const RAW_EVIDENCE_RETENTION_DAYS: u32 = 30;
const NORMALIZED_EVIDENCE_RETENTION_DAYS: u32 = 90;
const REPORT_LEDGER_RETENTION_DAYS: u32 = 365;
const SCHEDULER_SCHEMA_VERSION: u32 = 2;
const DRAG_MACHINE_CONTRACT_VERSION: u32 = 13;
const TRACKING_MACHINE_CONTRACT_VERSION: u32 = 3;
const TEMPO_WORK_ATTRIBUTES_ENV: &str = "DRAG_TRACKING_TEMPO_WORK_ATTRIBUTES";
const LEGACY_TEMPO_WORK_ATTRIBUTES_ENV: &str = "DRAG_COMPANION_TEMPO_WORK_ATTRIBUTES";
const DEFAULT_SCHEDULE_TIME: &str = "18:45";
const DEFAULT_SCHEDULE_TIMEZONE: &str = "local";

#[cfg(debug_assertions)]
pub(crate) fn test_env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(not(debug_assertions))]
pub(crate) fn test_env_var(_name: &str) -> Option<String> {
    None
}

mod cli_contract;
mod collectors;
mod contract;
mod drag_gateway;
mod errors;
mod evidence_bundles;
mod evidence_sources;
mod execution;
mod operator_retention;
mod persistence_journal;
mod provider_proposals;
mod public_tracking;
mod replay;
mod rollout;
mod run_coordination;
mod scheduler;
mod tracking_config;

pub(crate) use cli_contract::*;
pub(crate) use collectors::*;
pub(crate) use contract::*;
pub(crate) use drag_gateway::*;
pub(crate) use errors::*;
pub(crate) use evidence_bundles::*;
pub(crate) use evidence_sources::*;
pub(crate) use execution::*;
pub(crate) use operator_retention::*;
pub(crate) use persistence_journal::*;
pub(crate) use provider_proposals::*;
pub(crate) use public_tracking::*;
pub(crate) use replay::*;
pub(crate) use rollout::*;
pub(crate) use run_coordination::*;
pub(crate) use scheduler::*;
pub(crate) use tracking_config::*;

fn main() {
    let cli = Cli::parse();
    let output = cli.output;
    if let Err(error) = run(cli) {
        if output == Some(TrackingOutputMode::Json) {
            let body = match &error {
                CompanionError::TrackingRun { details, .. } => serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": "tracking_run_failed",
                        "message": error.to_string(),
                        "details": details
                    }
                }),
                _ => serde_json::json!({
                    "ok": false,
                    "error": {"code": "tracking_error", "message": error.to_string()}
                }),
            };
            let _ = print_error_json(&body);
        } else {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }
}
