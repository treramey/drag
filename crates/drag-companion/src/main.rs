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
use clap::{Args, Parser, Subcommand};
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
const CLAUDE_HOOK_COMMAND: &str = "drag-companion claude-hook capture";
const RAW_EVIDENCE_RETENTION_DAYS: u32 = 30;
const NORMALIZED_EVIDENCE_RETENTION_DAYS: u32 = 90;
const REPORT_LEDGER_RETENTION_DAYS: u32 = 365;
const SCHEDULER_SCHEMA_VERSION: u32 = 2;
const DRAG_MACHINE_CONTRACT_VERSION: u32 = 10;
const TEMPO_WORK_ATTRIBUTES_ENV: &str = "DRAG_COMPANION_TEMPO_WORK_ATTRIBUTES";
const DEFAULT_SCHEDULE_TIME: &str = "18:45";
const DEFAULT_SCHEDULE_TIMEZONE: &str = "local";

mod cli_contract;
mod collectors;
mod contract;
mod drag_gateway;
mod errors;
mod evidence_bundles;
mod execution;
mod operator_retention;
mod persistence_journal;
mod provider_proposals;
mod replay;
mod rollout;
mod run_coordination;
mod scheduler;

pub(crate) use cli_contract::*;
pub(crate) use collectors::*;
pub(crate) use contract::*;
pub(crate) use drag_gateway::*;
pub(crate) use errors::*;
pub(crate) use evidence_bundles::*;
pub(crate) use execution::*;
pub(crate) use operator_retention::*;
pub(crate) use persistence_journal::*;
pub(crate) use provider_proposals::*;
pub(crate) use replay::*;
pub(crate) use rollout::*;
pub(crate) use run_coordination::*;
pub(crate) use scheduler::*;

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
