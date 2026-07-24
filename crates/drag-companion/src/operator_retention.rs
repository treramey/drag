use crate::*;

pub(crate) const COMPANION_SENTINEL: &str = ".drag-companion-owned";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OperatorLog<'a> {
    pub(crate) event: &'a str,
    pub(crate) run_id: Option<String>,
    pub(crate) status: &'a str,
    pub(crate) next_safe_action: &'a str,
    pub(crate) recovery: &'a str,
}

#[derive(Clone, Copy)]
pub(crate) enum RetentionTrigger {
    Lifecycle,
    Operator,
}

impl RetentionTrigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Lifecycle => "lifecycle",
            Self::Operator => "operator",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RetentionConfig {
    pub(crate) raw_days: u32,
    pub(crate) normalized_days: u32,
    pub(crate) report_ledger_days: u32,
}

pub(crate) fn retention_config_values() -> RetentionConfig {
    RetentionConfig {
        raw_days: retention_days(
            "DRAG_COMPANION_RETENTION_RAW_DAYS",
            RAW_EVIDENCE_RETENTION_DAYS,
        ),
        normalized_days: retention_days(
            "DRAG_COMPANION_RETENTION_NORMALIZED_DAYS",
            NORMALIZED_EVIDENCE_RETENTION_DAYS,
        ),
        report_ledger_days: retention_days(
            "DRAG_COMPANION_RETENTION_REPORT_LEDGER_DAYS",
            REPORT_LEDGER_RETENTION_DAYS,
        ),
    }
}

pub(crate) fn enforce_retention(
    data_dir: &Path,
    trigger: RetentionTrigger,
) -> Result<Value, CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    let config = retention_config_values();
    let now = retention_now()?;
    let raw_cutoff = (now - Duration::days(i64::from(config.raw_days))).date_naive();
    let normalized_cutoff = (now - Duration::days(i64::from(config.normalized_days))).date_naive();
    let report_cutoff = (now - Duration::days(i64::from(config.report_ledger_days))).date_naive();
    let journal = compact_journal(data_dir, raw_cutoff)?;
    let mut conn = Connection::open(store_path(data_dir))?;
    migrate(&mut conn)?;
    migrate_run_coordination(&conn)?;
    let store = compact_store(&mut conn, raw_cutoff, normalized_cutoff, report_cutoff)?;
    let runs = compact_run_files(data_dir, report_cutoff)?;
    Ok(serde_json::json!({
        "status": "retention-enforced",
        "trigger": trigger.as_str(),
        "now": now.to_rfc3339_opts(SecondsFormat::Secs, true),
        "retention": {
            "rawEvidenceDays": config.raw_days,
            "normalizedEvidenceDays": config.normalized_days,
            "reportsAndLedgerDays": config.report_ledger_days
        },
        "classes": {
            "raw": {"expired": journal.expired + store.raw_redacted, "journalRemoved": journal.expired, "storeRedacted": store.raw_redacted, "protected": journal.protected},
            "normalized": {"expired": store.normalized_deleted, "protected": store.normalized_protected},
            "reportsAndLedger": {"expired": store.report_ledger_deleted + runs.deleted, "protected": store.report_ledger_protected, "runFilesDeleted": runs.deleted}
        },
        "journal": {"path": journal_path(data_dir), "retained": journal.retained, "removed": journal.expired, "recoveredTempFiles": journal.recovered_temp_files, "crashSafe": "atomic-tempfile-rename"},
        "store": {"path": store_path(data_dir), "crashSafe": "sqlite-transaction"},
        "privacy": {"rawPayloadsRedacted": store.raw_redacted, "operatorOutputContainsRawPayloads": false},
        "liveMutationAllowed": false,
        "nextSafeAction": "review protected counts; uncertain or submitting records remain available for recovery"
    }))
}

pub(crate) struct JournalCompaction {
    pub(crate) retained: u64,
    pub(crate) expired: u64,
    pub(crate) protected: u64,
    pub(crate) recovered_temp_files: u64,
}

pub(crate) fn compact_journal(
    data_dir: &Path,
    raw_cutoff: NaiveDate,
) -> Result<JournalCompaction, CompanionError> {
    let _journal_lock = acquire_journal_lock(data_dir)?;
    let recovered_temp_files = cleanup_stale_journal_temps(data_dir)?;
    let path = journal_path(data_dir);
    if !path.exists() {
        return Ok(JournalCompaction {
            retained: 0,
            expired: 0,
            protected: 0,
            recovered_temp_files,
        });
    }
    let file = File::open(&path).map_err(|source| CompanionError::Open {
        path: path.clone(),
        source,
    })?;
    let mut retained_lines = Vec::new();
    let mut retained = 0;
    let mut expired = 0;
    let protected = 0;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|source| CompanionError::Read {
            path: path.clone(),
            source,
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let event_date = serde_json::from_str::<JournalEvent>(&line)
            .ok()
            .map(|event| event.timestamp_semantics.explicit_date);
        if event_date.is_some_and(|date| date < raw_cutoff) {
            expired += 1;
        } else {
            retained += 1;
            retained_lines.push(line);
        }
    }
    if expired > 0 {
        let mut body = retained_lines.join("\n").into_bytes();
        if !body.is_empty() {
            body.push(b'\n');
        }
        atomic_write(&path, &body)?;
    }
    Ok(JournalCompaction {
        retained,
        expired,
        protected,
        recovered_temp_files,
    })
}

pub(crate) fn cleanup_stale_journal_temps(data_dir: &Path) -> Result<u64, CompanionError> {
    if !data_dir.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in fs::read_dir(data_dir).map_err(|source| CompanionError::Read {
        path: data_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| CompanionError::Read {
            path: data_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("journal.jsonl.tmp-") && path.is_file() {
            fs::remove_file(&path).map_err(|source| CompanionError::Write {
                path: path.clone(),
                source,
            })?;
            removed += 1;
        }
    }
    Ok(removed)
}

pub(crate) struct StoreCompaction {
    pub(crate) raw_redacted: u64,
    pub(crate) normalized_deleted: u64,
    pub(crate) normalized_protected: u64,
    pub(crate) report_ledger_deleted: u64,
    pub(crate) report_ledger_protected: u64,
}

pub(crate) fn compact_store(
    conn: &mut Connection,
    raw_cutoff: NaiveDate,
    normalized_cutoff: NaiveDate,
    report_cutoff: NaiveDate,
) -> Result<StoreCompaction, CompanionError> {
    let protected_dates = protected_retention_dates(conn)?;
    let tx = conn.transaction()?;
    let raw_redacted = tx.execute(
        "UPDATE evidence_events SET payload_json = ?1, privacy_redacted = 1 WHERE explicit_date < ?2 AND privacy_redacted = 0",
        params![serde_json::json!({"retention":"redacted","class":"rawEvidence"}).to_string(), raw_cutoff.to_string()],
    )? as u64;

    let mut normalized_deleted = 0;
    let mut normalized_protected = 0;
    for date in dates_before(&tx, "daily_bundles", "explicit_date", normalized_cutoff)? {
        if protected_dates.contains(&date) {
            normalized_protected += count_date_rows(&tx, "daily_bundles", "explicit_date", date)?;
            continue;
        }
        normalized_deleted += delete_normalized_date(&tx, date)?;
    }
    for date in dates_before(&tx, "evidence_events", "explicit_date", normalized_cutoff)? {
        if protected_dates.contains(&date) {
            normalized_protected += count_date_rows(&tx, "evidence_events", "explicit_date", date)?;
            continue;
        }
        normalized_deleted += tx.execute(
            "DELETE FROM issue_candidates WHERE evidence_event_id IN (SELECT event_id FROM evidence_events WHERE explicit_date = ?1)",
            [date.to_string()],
        )? as u64;
        normalized_deleted += tx.execute(
            "UPDATE evidence_events SET supersedes = NULL WHERE supersedes IN (SELECT event_id FROM evidence_events WHERE explicit_date = ?1)",
            [date.to_string()],
        )? as u64;
        normalized_deleted += tx.execute(
            "DELETE FROM evidence_events WHERE explicit_date = ?1",
            [date.to_string()],
        )? as u64;
    }

    let mut report_ledger_deleted = 0;
    let report_ledger_protected = protected_ledger_count(&tx, report_cutoff)?;
    report_ledger_deleted += tx.execute(
        "DELETE FROM mutation_attempts WHERE operation_id IN (SELECT id FROM mutation_operations WHERE local_date < ?1 AND state IN ('confirmed','rejected','skipped','failed'))",
        [report_cutoff.to_string()],
    )? as u64;
    report_ledger_deleted += tx.execute(
        "DELETE FROM mutation_operations WHERE local_date < ?1 AND state IN ('confirmed','rejected','skipped','failed')",
        [report_cutoff.to_string()],
    )? as u64;
    report_ledger_deleted += tx.execute(
        "DELETE FROM reports WHERE run_id IN (SELECT id FROM runs WHERE explicit_date < ?1 AND state IN ('confirmed','rejected','skipped','failed'))",
        [report_cutoff.to_string()],
    )? as u64;
    report_ledger_deleted += tx.execute(
        "DELETE FROM leases WHERE run_id IN (SELECT id FROM runs WHERE explicit_date < ?1 AND state IN ('confirmed','rejected','skipped','failed'))",
        [report_cutoff.to_string()],
    )? as u64;
    report_ledger_deleted += tx.execute(
        "DELETE FROM runs WHERE explicit_date < ?1 AND state IN ('confirmed','rejected','skipped','failed')",
        [report_cutoff.to_string()],
    )? as u64;
    report_ledger_deleted += tx.execute(
        "DELETE FROM run_phases WHERE local_date < ?1 AND state IN ('completed','failed')",
        [report_cutoff.to_string()],
    )? as u64;
    report_ledger_deleted += tx.execute(
        "DELETE FROM coordinated_runs WHERE local_date < ?1 AND state IN ('completed','partial','blocked','failed')",
        [report_cutoff.to_string()],
    )? as u64;
    tx.commit()?;
    Ok(StoreCompaction {
        raw_redacted,
        normalized_deleted,
        normalized_protected,
        report_ledger_deleted,
        report_ledger_protected,
    })
}

pub(crate) fn protected_retention_dates(
    conn: &Connection,
) -> Result<std::collections::BTreeSet<NaiveDate>, CompanionError> {
    let mut dates = std::collections::BTreeSet::new();
    collect_protected_dates(
        conn,
        &mut dates,
        "SELECT explicit_date FROM daily_bundles WHERE state IN ('proposed','approved','submitting','uncertain')",
    )?;
    collect_protected_dates(
        conn,
        &mut dates,
        "SELECT b.explicit_date FROM proposals p JOIN daily_bundles b ON b.id = p.bundle_id WHERE p.state IN ('proposed','approved','submitting','uncertain')",
    )?;
    collect_protected_dates(
        conn,
        &mut dates,
        "SELECT local_date FROM mutation_operations WHERE state IN ('proposed','approved','submitting','uncertain') AND local_date IS NOT NULL",
    )?;
    collect_protected_dates(
        conn,
        &mut dates,
        "SELECT local_date FROM coordinated_runs WHERE state NOT IN ('completed','partial','blocked','failed')",
    )?;
    collect_protected_dates(
        conn,
        &mut dates,
        "SELECT local_date FROM run_phases WHERE state NOT IN ('completed','failed')",
    )?;
    Ok(dates)
}

pub(crate) fn collect_protected_dates(
    conn: &Connection,
    dates: &mut std::collections::BTreeSet<NaiveDate>,
    sql: &str,
) -> Result<(), CompanionError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        if let Ok(date) = NaiveDate::parse_from_str(&row?, "%Y-%m-%d") {
            dates.insert(date);
        }
    }
    Ok(())
}

pub(crate) fn dates_before(
    conn: &Connection,
    table: &str,
    column: &str,
    cutoff: NaiveDate,
) -> Result<Vec<NaiveDate>, CompanionError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT {column} FROM {table} WHERE {column} < ?1 ORDER BY {column}"
    ))?;
    let rows = stmt.query_map([cutoff.to_string()], |row| row.get::<_, String>(0))?;
    let mut dates = Vec::new();
    for row in rows {
        if let Ok(date) = NaiveDate::parse_from_str(&row?, "%Y-%m-%d") {
            dates.push(date);
        }
    }
    Ok(dates)
}

pub(crate) fn count_date_rows(
    conn: &Connection,
    table: &str,
    column: &str,
    date: NaiveDate,
) -> Result<u64, CompanionError> {
    let count: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
        [date.to_string()],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as u64)
}

pub(crate) fn delete_normalized_date(
    conn: &Connection,
    date: NaiveDate,
) -> Result<u64, CompanionError> {
    let date = date.to_string();
    let mut deleted = 0;
    deleted += conn.execute(
        "DELETE FROM policy_decisions WHERE proposal_id IN (SELECT p.id FROM proposals p JOIN daily_bundles b ON b.id = p.bundle_id WHERE b.explicit_date = ?1)",
        [&date],
    )? as u64;
    deleted += conn.execute(
        "DELETE FROM proposal_drag_resolutions WHERE proposal_id IN (SELECT p.id FROM proposals p JOIN daily_bundles b ON b.id = p.bundle_id WHERE b.explicit_date = ?1)",
        [&date],
    )? as u64;
    deleted += conn.execute(
        "DELETE FROM proposal_policy_fields WHERE proposal_id IN (SELECT p.id FROM proposals p JOIN daily_bundles b ON b.id = p.bundle_id WHERE b.explicit_date = ?1)",
        [&date],
    )? as u64;
    deleted += conn.execute(
        "UPDATE mutation_operations SET proposal_id = NULL WHERE proposal_id IN (SELECT p.id FROM proposals p JOIN daily_bundles b ON b.id = p.bundle_id WHERE b.explicit_date = ?1)",
        [&date],
    )? as u64;
    deleted += conn.execute(
        "DELETE FROM proposals WHERE bundle_id IN (SELECT id FROM daily_bundles WHERE explicit_date = ?1)",
        [&date],
    )? as u64;
    deleted += conn.execute(
        "DELETE FROM daily_bundles WHERE explicit_date = ?1",
        [&date],
    )? as u64;
    deleted += conn.execute(
        "DELETE FROM unsupported_periods WHERE explicit_date = ?1",
        [&date],
    )? as u64;
    deleted += conn.execute(
        "DELETE FROM provider_requests WHERE explicit_date = ?1",
        [&date],
    )? as u64;
    Ok(deleted)
}

pub(crate) fn protected_ledger_count(
    conn: &Connection,
    cutoff: NaiveDate,
) -> Result<u64, CompanionError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM mutation_operations WHERE local_date < ?1 AND state IN ('proposed','approved','submitting','uncertain')",
        [cutoff.to_string()],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as u64)
}

pub(crate) struct RunFileCompaction {
    pub(crate) deleted: u64,
}

pub(crate) fn compact_run_files(
    data_dir: &Path,
    report_cutoff: NaiveDate,
) -> Result<RunFileCompaction, CompanionError> {
    let runs_dir = data_dir.join("runs");
    if !runs_dir.exists() {
        return Ok(RunFileCompaction { deleted: 0 });
    }
    let mut deleted = 0;
    for entry in fs::read_dir(&runs_dir).map_err(|source| CompanionError::Read {
        path: runs_dir.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| CompanionError::Read {
            path: runs_dir.clone(),
            source,
        })?;
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(date) = NaiveDate::parse_from_str(stem, "%Y-%m-%d") else {
            continue;
        };
        if date >= report_cutoff || run_file_is_protected(&path) {
            continue;
        }
        fs::remove_file(&path).map_err(|source| CompanionError::Write {
            path: path.clone(),
            source,
        })?;
        deleted += 1;
    }
    Ok(RunFileCompaction { deleted })
}

pub(crate) fn run_file_is_protected(path: &Path) -> bool {
    let Ok(body) = fs::read_to_string(path) else {
        return true;
    };
    let Ok(json) = serde_json::from_str::<Value>(&body) else {
        return true;
    };
    matches!(
        json.get("status").and_then(Value::as_str),
        Some("uncertain" | "submitting" | "proposed" | "approved")
    )
}

pub(crate) fn status_payload(data_dir: &Path) -> Result<Value, CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    ensure_companion_sentinel(data_dir)?;
    let mut conn = Connection::open(store_path(data_dir))?;
    migrate(&mut conn)?;
    migrate_run_coordination(&conn)?;
    let now = epoch_ms();
    let mut stmt = conn.prepare("SELECT tempo_account, local_date, owner_id, heartbeat_at, expires_at_ms FROM run_leases WHERE expires_at_ms > ?1 ORDER BY local_date")?;
    let leases = stmt.query_map([now], |row| Ok(serde_json::json!({"tempoAccount": row.get::<_, String>(0)?, "localDate": row.get::<_, String>(1)?, "ownerId": row.get::<_, String>(2)?, "heartbeatAt": row.get::<_, String>(3)?, "expiresAtMs": row.get::<_, i64>(4)?})))?.collect::<Result<Vec<_>, _>>()?;
    Ok(
        serde_json::json!({ "status": "ready", "mode": DEFAULT_MODE, "networkAccess": false, "liveMutationAllowed": false, "rollout": rollout_status_value(&load_rollout_state(data_dir)?, None), "retention": retention_config(), "nextSafeAction": "run reconcile for an explicit date, or resume only after checking status and report output", "journal": journal_path(data_dir), "store": store_path(data_dir), "activeLeases": leases }),
    )
}

pub(crate) fn run_id(date: NaiveDate) -> String {
    format!("{TEMPO_ACCOUNT}:{date}")
}

pub(crate) fn operator_log(
    data_dir: &Path,
    date: NaiveDate,
) -> Result<OperatorLog<'static>, CompanionError> {
    let status = terminal_report_status(data_dir, date).unwrap_or("unknown");
    Ok(OperatorLog {
        event: "daily_audit_status",
        run_id: Some(run_id(date)),
        status,
        next_safe_action: next_safe_action(status),
        recovery: recovery_instructions(status),
    })
}

pub(crate) fn daily_report(data_dir: &Path, date: NaiveDate) -> Result<String, CompanionError> {
    let status = terminal_report_status(data_dir, date).unwrap_or("unknown");
    let created = created_ids(data_dir, date)?;
    Ok(format!(
        "# Drag Companion Daily Audit Report\n\n- Run ID: {}\n- Status: {}\n- Source health: local capture-only sources checked; network access disabled; live mutation disabled\n- Evidence summary: normalized evidence and mutation ledger inspected for the explicit local date\n- Gaps: unsupported or missing evidence remains operator-reviewed only\n- Proposals: persisted proposal decisions are summarized by the audit and preview commands\n- Policy decisions: deterministic policy output is preserved; unattended approval requires explicit authorization\n- Created IDs: {}\n- Skips: duplicate, unsupported, or unsafe periods are skipped rather than mutated blindly\n- Failures: see status and structured log output for bounded failure details\n- Uncertain outcomes: uncertain mutation operations require exact-ID day reconciliation before any further mutation\n- Recovery instructions: {}\n- Next safe action: {}\n- Retention: raw evidence {} days; normalized evidence {} days; reports and mutation ledger {} days\n",
        run_id(date),
        status,
        if created.is_empty() { "none".to_owned() } else { created.join(", ") },
        recovery_instructions(status),
        next_safe_action(status),
        retention_config()["rawEvidenceDays"],
        retention_config()["normalizedEvidenceDays"],
        retention_config()["reportsAndLedgerDays"],
    ))
}

pub(crate) fn terminal_report_status(data_dir: &Path, date: NaiveDate) -> Option<&'static str> {
    let path = run_path(data_dir, date);
    let json = fs::read_to_string(&path)
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .or_else(|| heal_terminal_run_file(data_dir, date).ok().flatten())?;
    match json.get("status").and_then(Value::as_str) {
        Some("completed") | Some("terminal") => Some("completed"),
        Some("partial") => Some("partial"),
        Some("blocked") => Some("blocked"),
        Some("failed") => Some("failed"),
        Some("uncertain") => Some("uncertain"),
        _ => Some("unknown"),
    }
}

pub(crate) fn created_ids(data_dir: &Path, date: NaiveDate) -> Result<Vec<String>, CompanionError> {
    let conn = Connection::open(store_path(data_dir))?;
    let mut stmt = conn.prepare("SELECT tempo_worklog_id FROM mutation_operations WHERE local_date = ?1 AND tempo_worklog_id IS NOT NULL ORDER BY tempo_worklog_id")?;
    let ids = stmt.query_map([date.to_string()], |row| row.get::<_, String>(0))?;
    ids.collect::<Result<Vec<_>, _>>()
        .map_err(CompanionError::Store)
}

pub(crate) fn next_safe_action(status: &str) -> &'static str {
    match status {
        "completed" => "review the report and keep the ledger for idempotency",
        "partial" => {
            "inspect skips and failures, then run audit or preview before any authorized execute"
        }
        "blocked" => "resolve the named blocker, then run resume for the explicit date",
        "failed" => "inspect structured log and exact recovery instructions before changing inputs",
        "uncertain" => "run resume to reconcile exact created IDs before any further mutation",
        _ => "run status, then reconcile or report for one explicit date",
    }
}

pub(crate) fn recovery_instructions(status: &str) -> &'static str {
    match status {
        "uncertain" => "read the complete Tempo day through Drag, match only exact idempotency ledger payloads, and block further mutation until reconciliation names the created IDs",
        "failed" => "fix the reported non-mutation cause, then resume only after status shows no active owner",
        "blocked" => "clear the policy or source-health blocker; resume will not enter submission until pre-mutation checks pass",
        "partial" => "review skipped and failed records; create a new explicit approval instead of reusing stale mutation intent",
        _ => "no automated recovery required; retain reports and ledger for auditability",
    }
}

pub(crate) fn purge_state(
    data_dir: &Path,
    acknowledge_lost_recovery: bool,
) -> Result<Value, CompanionError> {
    validate_purge_target(data_dir)?;
    if acknowledge_lost_recovery {
        if data_dir.exists() {
            require_companion_sentinel(data_dir)?;
            purge_allowlisted_paths(data_dir, true)?;
        }
        return Ok(
            serde_json::json!({ "status": "purged", "idempotencyRecordsProtected": false, "lostAutomatedRecoveryAcknowledged": true, "nextSafeAction": "run collect and reconcile from fresh explicit-date evidence before any mutation" }),
        );
    }
    if data_dir.exists() {
        require_companion_sentinel(data_dir)?;
    } else {
        fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
            path: data_dir.to_path_buf(),
            source,
        })?;
    }
    ensure_companion_sentinel(data_dir)?;
    let database = store_path(data_dir);
    if database.exists() {
        let mut conn = Connection::open(&database)?;
        migrate(&mut conn)?;
        migrate_run_coordination(&conn)?;
        let tx = conn.transaction()?;
        tx.execute_batch(
            "UPDATE mutation_operations SET proposal_id = NULL;
             DELETE FROM policy_decisions;
             DELETE FROM proposal_drag_resolutions;
             DELETE FROM proposal_policy_fields;
             DELETE FROM proposals;
             DELETE FROM daily_bundles;
             DELETE FROM issue_candidates;
             DELETE FROM evidence_events;
             DELETE FROM unsupported_periods;
             DELETE FROM provider_requests;
             DELETE FROM reports;
             DELETE FROM run_leases;
             DELETE FROM run_phases;
             DELETE FROM coordinated_runs;
             DELETE FROM leases;
             DELETE FROM runs;",
        )?;
        tx.commit()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    }
    purge_allowlisted_paths(data_dir, false)?;
    Ok(
        serde_json::json!({ "status": "purged", "idempotencyRecordsProtected": true, "lostAutomatedRecoveryAcknowledged": false, "nextSafeAction": "keep protected idempotency records; run status before any resume" }),
    )
}

pub(crate) fn ensure_companion_sentinel(data_dir: &Path) -> Result<(), CompanionError> {
    let sentinel = data_dir.join(COMPANION_SENTINEL);
    if !sentinel.exists() {
        atomic_write(&sentinel, b"drag-companion-owned-data-dir\n")?;
    }
    Ok(())
}

fn require_companion_sentinel(data_dir: &Path) -> Result<(), CompanionError> {
    if data_dir.join(COMPANION_SENTINEL).is_file() {
        Ok(())
    } else {
        Err(CompanionError::Proposal(format!(
            "refusing to purge non-companion data directory {}; missing {COMPANION_SENTINEL}",
            data_dir.display()
        )))
    }
}

fn validate_purge_target(data_dir: &Path) -> Result<(), CompanionError> {
    let identity = canonical_lock_identity(data_dir)?;
    if identity.parent().is_none()
        || identity == Path::new("/")
        || identity == std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    {
        return Err(CompanionError::Proposal(format!(
            "refusing dangerous purge target {}",
            data_dir.display()
        )));
    }
    Ok(())
}

fn purge_allowlisted_paths(data_dir: &Path, include_database: bool) -> Result<(), CompanionError> {
    for name in [
        "journal.jsonl",
        ".journal.lock",
        "runs",
        "locks",
        "scheduler.json",
        "rollout.json",
        "journal.jsonl.tmp-crash-secret",
    ] {
        remove_child_if_exists(data_dir, name)?;
    }
    cleanup_stale_journal_temps(data_dir)?;
    if include_database {
        for name in [
            "companion.sqlite3",
            "companion.sqlite3-wal",
            "companion.sqlite3-shm",
        ] {
            remove_child_if_exists(data_dir, name)?;
        }
        remove_child_if_exists(data_dir, COMPANION_SENTINEL)?;
    }
    Ok(())
}

fn remove_child_if_exists(data_dir: &Path, name: &str) -> Result<(), CompanionError> {
    let path = data_dir.join(name);
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(&path).map_err(|source| CompanionError::Write { path, source })
    } else {
        fs::remove_file(&path).map_err(|source| CompanionError::Write { path, source })
    }
}

fn heal_terminal_run_file(
    data_dir: &Path,
    date: NaiveDate,
) -> Result<Option<Value>, CompanionError> {
    let database = store_path(data_dir);
    if !database.exists() {
        return Ok(None);
    }
    let conn = Connection::open(database)?;
    let status = terminal_run_status(&conn, date, TEMPO_ACCOUNT)?;
    if let Some(status) = status {
        let json = serde_json::json!({
            "date": date,
            "status": status,
            "mode": DEFAULT_MODE,
            "adapters": adapters(),
            "networkAccess": false,
            "liveMutationAllowed": false,
            "dragBoundary": drag_boundary(),
            "observations": [{"source": COLLECTOR_ADAPTER, "summary": "terminal run file healed from durable SQLite state"}]
        });
        let body = serde_json::to_vec_pretty(&json).map_err(CompanionError::Serialize)?;
        let runs_dir = data_dir.join("runs");
        fs::create_dir_all(&runs_dir).map_err(|source| CompanionError::CreateDir {
            path: runs_dir,
            source,
        })?;
        atomic_write(&run_path(data_dir, date), &body)?;
        return Ok(Some(json));
    }
    Ok(None)
}

pub(crate) fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

pub(crate) fn terminal_result(date: NaiveDate) -> RunResult {
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
