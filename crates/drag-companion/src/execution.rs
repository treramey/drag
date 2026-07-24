use crate::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExecuteResult {
    pub(crate) status: &'static str,
    pub(crate) selected_date: NaiveDate,
    pub(crate) submitted: usize,
    pub(crate) skipped: usize,
    pub(crate) uncertain: bool,
    pub(crate) network_access: bool,
    pub(crate) live_mutation_allowed: bool,
}

pub(crate) fn live_rollout_enabled() -> bool {
    std::env::var("DRAG_COMPANION_LIVE_MUTATION_ROLLOUT")
        .ok()
        .as_deref()
        == Some("1")
}

pub(crate) fn execute_drag_worklogs(
    data_dir: &Path,
    drag_bin: &Path,
    date: NaiveDate,
    authorize_live: bool,
) -> Result<ExecuteResult, CompanionError> {
    if !authorize_live
        || !live_rollout_enabled()
        || !persisted_live_mutation_allowed(data_dir)?
        || scheduler_kill_switch_path(data_dir).exists()
        || std::env::var_os("DRAG_COMPANION_KILL_SWITCH").is_some()
    {
        return Ok(ExecuteResult {
            status: "gated",
            selected_date: date,
            submitted: 0,
            skipped: 0,
            uncertain: false,
            network_access: false,
            live_mutation_allowed: false,
        });
    }
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    let mut conn = Connection::open(store_path(data_dir))?;
    migrate(&mut conn)?;
    migrate_run_coordination(&conn)?;
    let account = execution_tempo_account(data_dir, date)?;
    let _lock = acquire_advisory_lock(data_dir, date, &account)?;
    let owner_id = format!("execute:{}:{}", std::process::id(), now_string());
    acquire_sqlite_lease(&conn, date, &account, &owner_id)?;
    let result = execute_drag_worklogs_locked(data_dir, drag_bin, date, &account, &conn);
    let release = release_sqlite_lease(&conn, date, &account, &owner_id);
    match (result, release) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(result), Ok(())) => Ok(result),
    }
}

pub(crate) fn execute_drag_worklogs_locked(
    data_dir: &Path,
    drag_bin: &Path,
    date: NaiveDate,
    account: &str,
    conn: &Connection,
) -> Result<ExecuteResult, CompanionError> {
    reconcile_complete_day_and_ledger(conn, drag_bin, date, account)?;
    let approved = approved_payloads_for_account(data_dir, date, account)?;
    let mut submitted = 0;
    let mut skipped = 0;
    let mut blocked_accounts = std::collections::BTreeSet::new();
    for record in approved {
        let proposal_id = record.proposal_id;
        let account = record.tempo_account;
        let payload = record.payload;
        if blocked_accounts.contains(&account) {
            continue;
        }
        let key = operation_key(&account, date, &payload)?;
        match operation_state(conn, &key)?.as_deref() {
            Some("confirmed" | "failed") => {
                skipped += 1;
                continue;
            }
            Some("submitting" | "uncertain") => {
                blocked_accounts.insert(account);
                continue;
            }
            Some(_) | None => {}
        }
        if date_has_unresolved_operation(conn, date, &account)? {
            blocked_accounts.insert(account);
            continue;
        }
        let latest = read_drag_day(drag_bin, date)?;
        let candidate = normalize_payload_worklog(&payload, &proposal_id)?;
        if latest
            .worklogs
            .iter()
            .any(|existing| same_worklog(existing, &candidate))
        {
            persist_submitting_operation(conn, date, &account, &proposal_id, &key, &payload)?;
            persist_confirmed_operation(conn, &key, "reconciled-existing")?;
            skipped += 1;
            continue;
        }
        persist_submitting_operation(conn, date, &account, &proposal_id, &key, &payload)?;
        let response = drag_json(
            drag_bin,
            &[
                "--output".into(),
                "json".into(),
                "log".into(),
                "--json".into(),
                "-".into(),
            ],
            Some(&payload),
            false,
        );
        match response {
            Ok(value) => {
                let Some(id) = value
                    .get("tempoWorklogId")
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                else {
                    mark_operation_uncertain(conn, date, &account, &key)?;
                    return Ok(uncertain_execute_result(date));
                };
                persist_confirmed_operation(conn, &key, id)?;
                submitted += 1;
            }
            Err(error) if unverifiable_after_live_spawn(&error) => {
                mark_operation_uncertain(conn, date, &account, &key)?;
                return Ok(uncertain_execute_result(date));
            }
            Err(error) => {
                mark_operation_failed(conn, &key)?;
                return Err(error);
            }
        }
    }
    let uncertain = submitted == 0 && skipped == 0 && !blocked_accounts.is_empty();
    Ok(ExecuteResult {
        status: if uncertain { "uncertain" } else { "executed" },
        selected_date: date,
        submitted,
        skipped,
        uncertain,
        network_access: true,
        live_mutation_allowed: true,
    })
}

pub(crate) fn uncertain_execute_result(date: NaiveDate) -> ExecuteResult {
    ExecuteResult {
        status: "uncertain",
        selected_date: date,
        submitted: 0,
        skipped: 0,
        uncertain: true,
        network_access: true,
        live_mutation_allowed: true,
    }
}

pub(crate) fn unverifiable_after_live_spawn(error: &CompanionError) -> bool {
    matches!(
        error,
        CompanionError::DragReconcile {
            kind: ReconcileErrorKind::TransportAmbiguity
                | ReconcileErrorKind::SchemaIncompatibility
                | ReconcileErrorKind::IncompleteRead,
            ..
        }
    )
}

pub(crate) fn operation_key(
    account: &str,
    date: NaiveDate,
    payload: &Value,
) -> Result<String, CompanionError> {
    let canonical = serde_json::to_vec(payload).map_err(CompanionError::Serialize)?;
    let digest = Sha256::digest(canonical);
    Ok(format!(
        "op.v{POLICY_SCHEMA_VERSION}.{account}.{date}.{digest:x}"
    ))
}

pub(crate) fn approved_payloads(
    data_dir: &Path,
    date: NaiveDate,
) -> Result<Vec<ProposalPayloadRecord>, CompanionError> {
    let approved = {
        let conn = Connection::open(store_path(data_dir))?;
        let mut stmt = conn.prepare("SELECT proposal_id FROM policy_decisions WHERE decision = 'approved' ORDER BY proposal_id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<std::collections::BTreeSet<_>, _>>()?
    };
    Ok(proposal_payload_records(data_dir, date, None)?
        .into_iter()
        .filter(|record| approved.contains(&record.proposal_id))
        .collect())
}

pub(crate) fn execution_tempo_account(
    data_dir: &Path,
    date: NaiveDate,
) -> Result<String, CompanionError> {
    let accounts = approved_payloads(data_dir, date)?
        .into_iter()
        .map(|record| record.tempo_account)
        .collect::<std::collections::BTreeSet<_>>();
    match accounts.len() {
        0 => Ok(TEMPO_ACCOUNT.to_owned()),
        1 => Ok(accounts.into_iter().next().unwrap_or_default()),
        _ => Err(reconcile_error(
            ReconcileErrorKind::SchemaIncompatibility,
            format!("approved proposals span multiple Tempo accounts for {date}"),
        )),
    }
}

pub(crate) fn approved_payloads_for_account(
    data_dir: &Path,
    date: NaiveDate,
    account: &str,
) -> Result<Vec<ProposalPayloadRecord>, CompanionError> {
    Ok(approved_payloads(data_dir, date)?
        .into_iter()
        .filter(|record| record.tempo_account == account)
        .collect())
}

pub(crate) fn persist_submitting_operation(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
    proposal_id: &str,
    key: &str,
    payload: &Value,
) -> Result<(), CompanionError> {
    let intent =
        serde_json::json!({"intent":"submit-worklog","persistedBeforeDrag":true,"at":now_string()});
    let tx = conn.unchecked_transaction()?;
    tx.execute("INSERT INTO mutation_operations (id, proposal_id, state, idempotency_key, local_date, tempo_account, payload_json, submitting_intent_json, policy_schema_version, payload_schema_version) VALUES (?1, ?2, 'submitting', ?1, ?3, ?4, ?5, ?6, ?7, 1) ON CONFLICT(id) DO NOTHING", params![key, proposal_id, date.to_string(), account, payload.to_string(), intent.to_string(), POLICY_SCHEMA_VERSION])?;
    tx.execute("INSERT INTO mutation_attempts (id, operation_id, state, attempted_at) VALUES (?1, ?1, 'submitting', ?2) ON CONFLICT(id) DO NOTHING", params![key, now_string()])?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn persist_confirmed_operation(
    conn: &Connection,
    key: &str,
    tempo_id: &str,
) -> Result<(), CompanionError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE mutation_operations SET state = 'confirmed', tempo_worklog_id = ?1 WHERE id = ?2",
        params![tempo_id, key],
    )?;
    tx.execute(
        "UPDATE mutation_attempts SET state = 'confirmed' WHERE operation_id = ?1",
        params![key],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn mark_operation_uncertain(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
    key: &str,
) -> Result<(), CompanionError> {
    conn.execute(
        "UPDATE mutation_operations SET state = 'uncertain' WHERE id = ?1",
        params![key],
    )?;
    conn.execute(
        "UPDATE mutation_attempts SET state = 'uncertain' WHERE operation_id = ?1",
        params![key],
    )?;
    finish_run(conn, date, account, "uncertain")?;
    Ok(())
}

pub(crate) fn mark_operation_failed(conn: &Connection, key: &str) -> Result<(), CompanionError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE mutation_operations SET state = 'failed' WHERE id = ?1",
        params![key],
    )?;
    tx.execute(
        "UPDATE mutation_attempts SET state = 'failed' WHERE operation_id = ?1",
        params![key],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn operation_state(
    conn: &Connection,
    key: &str,
) -> Result<Option<String>, CompanionError> {
    Ok(conn
        .query_row(
            "SELECT state FROM mutation_operations WHERE id = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

pub(crate) fn date_has_unresolved_operation(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
) -> Result<bool, CompanionError> {
    Ok(conn.query_row("SELECT 1 FROM mutation_operations WHERE tempo_account = ?1 AND local_date = ?2 AND state IN ('submitting','uncertain') LIMIT 1", params![account, date.to_string()], |row| row.get::<_, i64>(0)).optional()?.is_some())
}

pub(crate) fn date_has_mutation_operations(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
) -> Result<bool, CompanionError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM mutation_operations WHERE tempo_account = ?1 AND local_date = ?2 LIMIT 1",
            params![account, date.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some())
}

pub(crate) fn reconcile_complete_day_and_ledger(
    conn: &Connection,
    drag_bin: &Path,
    date: NaiveDate,
    account: &str,
) -> Result<(), CompanionError> {
    let read = read_drag_day(drag_bin, date)?;
    let mut stmt = conn.prepare("SELECT id, payload_json FROM mutation_operations WHERE tempo_account = ?1 AND local_date = ?2 AND state IN ('submitting','uncertain') ORDER BY id")?;
    let rows = stmt.query_map(params![account, date.to_string()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (key, payload_json) = row?;
        let payload: Value =
            serde_json::from_str(&payload_json).map_err(CompanionError::Serialize)?;
        let candidate = normalize_payload_worklog(&payload, &key)?;
        if let Some(existing) = read
            .worklogs
            .iter()
            .find(|existing| same_worklog(existing, &candidate))
        {
            persist_confirmed_operation(conn, &key, &existing.tempo_worklog_id)?;
        }
    }
    Ok(())
}

pub(crate) fn process_spy(data_dir: &Path, date: NaiveDate) -> Result<Value, CompanionError> {
    let mut conn = Connection::open(store_path(data_dir))?;
    migrate(&mut conn)?;
    migrate_run_coordination(&conn)?;
    let mut stmt = conn.prepare("SELECT id, state, tempo_account, payload_json, submitting_intent_json, tempo_worklog_id FROM mutation_operations WHERE local_date = ?1 ORDER BY tempo_account, id")?;
    let rows = stmt.query_map([date.to_string()], |row| Ok(serde_json::json!({"operationKey": row.get::<_, String>(0)?, "state": row.get::<_, String>(1)?, "tempoAccount": row.get::<_, Option<String>>(2)?, "payload": row.get::<_, Option<String>>(3)?.and_then(|s| serde_json::from_str::<Value>(&s).ok()), "submittingIntent": row.get::<_, Option<String>>(4)?.and_then(|s| serde_json::from_str::<Value>(&s).ok()), "tempoWorklogId": row.get::<_, Option<String>>(5)?})))?.collect::<Result<Vec<_>, _>>()?;
    Ok(serde_json::json!({"selectedDate": date, "operations": rows}))
}

pub(crate) fn acquire_sqlite_lease(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
    owner_id: &str,
) -> Result<(bool, bool), CompanionError> {
    let now = epoch_ms();
    let ttl = std::env::var("DRAG_COMPANION_TEST_LEASE_TTL_MS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(LEASE_TTL_MS);
    let expires = now + ttl;
    let existing: Option<(String, i64)> = conn.query_row(
        "SELECT owner_id, expires_at_ms FROM run_leases WHERE tempo_account = ?1 AND local_date = ?2",
        params![account, date.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let mut recovered = false;
    if let Some((owner, expiry)) = existing {
        if expiry > now {
            return Err(CompanionError::RunOwned {
                account: account.to_owned(),
                date,
                owner,
                expires_at: expiry.to_string(),
            });
        }
        recovered = true;
        conn.execute(
            "DELETE FROM run_leases WHERE tempo_account = ?1 AND local_date = ?2",
            params![account, date.to_string()],
        )?;
    }
    let skipped = terminal_run_status(conn, date, account)?.is_some();
    conn.execute("INSERT OR IGNORE INTO coordinated_runs (tempo_account, local_date, state, started_at) VALUES (?1, ?2, 'running', ?3)", params![account, date.to_string(), now_string()])?;
    conn.execute("INSERT INTO run_leases (tempo_account, local_date, owner_id, heartbeat_at, expires_at_ms, recovered_from) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![account, date.to_string(), owner_id, now_string(), expires, if recovered { Some("expired") } else { None }])?;
    Ok((recovered, skipped))
}

pub(crate) fn heartbeat_lease(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
    owner_id: &str,
) -> Result<(), CompanionError> {
    conn.execute("UPDATE run_leases SET heartbeat_at = ?1, expires_at_ms = ?2 WHERE tempo_account = ?3 AND local_date = ?4 AND owner_id = ?5", params![now_string(), epoch_ms() + LEASE_TTL_MS, account, date.to_string(), owner_id])?;
    Ok(())
}

pub(crate) fn release_sqlite_lease(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
    owner_id: &str,
) -> Result<(), CompanionError> {
    conn.execute(
        "DELETE FROM run_leases WHERE tempo_account = ?1 AND local_date = ?2 AND owner_id = ?3",
        params![account, date.to_string(), owner_id],
    )?;
    Ok(())
}

pub(crate) fn run_phase(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
    owner_id: &str,
    phase: &'static str,
) -> Result<(), CompanionError> {
    let retryable = matches!(phase, "collecting" | "model" | "tempo_read");
    let transient = std::env::var("DRAG_COMPANION_TEST_TRANSIENT_PHASE")
        .ok()
        .as_deref()
        == Some(phase);
    let max_attempts = if retryable { READ_ONLY_RETRIES } else { 1 };
    for attempt in 1..=max_attempts {
        persist_phase_start(conn, date, account, phase, attempt)?;
        if std::env::var("DRAG_COMPANION_TEST_CRASH_AFTER_PHASE")
            .ok()
            .as_deref()
            == Some(phase)
        {
            std::process::exit(42);
        }
        if phase == "pre_mutation"
            && std::env::var("DRAG_COMPANION_TEST_BLOCK_BEFORE_MUTATION").is_ok()
        {
            finish_phase(
                conn,
                date,
                account,
                phase,
                attempt,
                "blocked",
                Some("blocked before mutation"),
            )?;
            finish_run(conn, date, account, "blocked")?;
            return Err(CompanionError::BlockedBeforeMutation);
        }
        if transient && attempt == 1 {
            finish_phase(
                conn,
                date,
                account,
                phase,
                attempt,
                "failed",
                Some("transient fixture"),
            )?;
            if !retryable {
                return Err(CompanionError::NotRetryable(phase));
            }
            continue;
        }
        if transient && !retryable {
            return Err(CompanionError::NotRetryable(phase));
        }
        finish_phase(conn, date, account, phase, attempt, "completed", None)?;
        heartbeat_lease(conn, date, account, owner_id)?;
        return Ok(());
    }
    Err(CompanionError::DragReconcile {
        kind: ReconcileErrorKind::DefiniteFailure,
        message: format!("phase {phase} exhausted retries"),
    })
}

pub(crate) fn persist_phase_start(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
    phase: &str,
    attempt: u32,
) -> Result<(), CompanionError> {
    conn.execute("INSERT OR IGNORE INTO run_phases (tempo_account, local_date, phase, state, attempt, started_at) VALUES (?1, ?2, ?3, 'running', ?4, ?5)", params![account, date.to_string(), phase, attempt, now_string()])?;
    Ok(())
}

pub(crate) fn finish_phase(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
    phase: &str,
    attempt: u32,
    state: &str,
    error: Option<&str>,
) -> Result<(), CompanionError> {
    conn.execute("UPDATE run_phases SET state = ?1, finished_at = ?2, error = ?3 WHERE tempo_account = ?4 AND local_date = ?5 AND phase = ?6 AND attempt = ?7", params![state, now_string(), error, account, date.to_string(), phase, attempt])?;
    Ok(())
}

pub(crate) fn finish_run(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
    state: &str,
) -> Result<(), CompanionError> {
    conn.execute("INSERT INTO coordinated_runs (tempo_account, local_date, state, started_at, finished_at) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(tempo_account, local_date) DO UPDATE SET state = excluded.state, finished_at = excluded.finished_at", params![account, date.to_string(), state, now_string(), now_string()])?;
    Ok(())
}

pub(crate) fn terminal_run_status(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
) -> Result<Option<&'static str>, CompanionError> {
    let state: Option<String> = conn.query_row("SELECT state FROM coordinated_runs WHERE tempo_account = ?1 AND local_date = ?2 AND state IN ('completed','partial','blocked','failed')", params![account, date.to_string()], |row| row.get(0)).optional()?;
    Ok(match state.as_deref() {
        Some("completed") => Some("completed"),
        Some("partial") => Some("partial"),
        Some("blocked") => Some("blocked"),
        Some("failed") => Some("failed"),
        _ => None,
    })
}

pub(crate) fn phase_completed(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
    phase: &str,
) -> Result<bool, CompanionError> {
    let done: Option<i64> = conn.query_row("SELECT 1 FROM run_phases WHERE tempo_account = ?1 AND local_date = ?2 AND phase = ?3 AND state = 'completed' LIMIT 1", params![account, date.to_string(), phase], |row| row.get(0)).optional()?;
    Ok(done.is_some())
}

pub(crate) fn load_phase_records(
    conn: &Connection,
    date: NaiveDate,
    account: &str,
) -> Result<Vec<RunPhaseRecord>, CompanionError> {
    let mut stmt = conn.prepare("SELECT phase, state, attempt, started_at, finished_at FROM run_phases WHERE tempo_account = ?1 AND local_date = ?2 ORDER BY rowid")?;
    let rows = stmt.query_map(params![account, date.to_string()], |row| {
        Ok(RunPhaseRecord {
            phase: row.get(0)?,
            state: row.get(1)?,
            attempt: row.get::<_, i64>(2)? as u32,
            started_at: row.get(3)?,
            finished_at: row.get(4)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(CompanionError::Store)
}
