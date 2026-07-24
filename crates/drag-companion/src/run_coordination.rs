use crate::*;

pub(crate) fn run(cli: Cli) -> Result<(), CompanionError> {
    let drag_bin = cli.drag_bin.clone();
    let data_dir = cli
        .data_dir
        .unwrap_or_else(|| PathBuf::from(".drag-companion"));
    let _state_lock = match &cli.command {
        Command::Contract => None,
        Command::Purge(_) => Some(acquire_companion_state_lock(&data_dir, true)?),
        _ => Some(acquire_companion_state_lock(&data_dir, false)?),
    };

    match cli.command {
        Command::Status => print_json(&status_payload(&data_dir)?),
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
            let result = coordinated_run(&data_dir, &drag_bin, args.date, false)?;
            print_json(&result)
        }
        Command::Resume(args) => {
            let result = coordinated_run(&data_dir, &drag_bin, args.date, true)?;
            print_json(&result)
        }
        Command::Report(args) => println_safe_markdown(&daily_report(&data_dir, args.date)?),
        Command::Log(args) => print_json(&operator_log(&data_dir, args.date)?),
        Command::Bundle(args) => {
            let bundle = build_bundle(&data_dir, args.date)?;
            print_json(&bundle)
        }
        Command::Propose(args) => {
            let result = propose_from_fixture(&data_dir, args.date, &args.fixture)?;
            print_json(&result)
        }
        Command::Read(args) => print_json(&read_drag_day(&drag_bin, args.date)?),
        Command::Audit(args) => print_json(&audit_drag_day(
            &data_dir,
            &drag_bin,
            args.date,
            args.authorize_unattended,
        )?),
        Command::Preview(args) => print_json(&preview_drag_payload(
            &data_dir,
            &drag_bin,
            args.date,
            args.proposal.as_deref(),
        )?),
        Command::Execute(args) => print_json(&execute_drag_worklogs(
            &data_dir,
            &drag_bin,
            args.date,
            args.authorize_live,
        )?),
        Command::Rollout(args) => handle_rollout(&data_dir, args),
        Command::Replay(args) => {
            print_json(&run_replay(&args.fixtures, args.artifacts.as_deref())?)
        }
        Command::ProcessSpy(args) => print_json(&process_spy(&data_dir, args.date)?),
        Command::Purge(args) => {
            print_json(&purge_state(&data_dir, args.acknowledge_lost_recovery)?)
        }
        Command::Retention(args) => match args.operation {
            RetentionOperation::Enforce => {
                print_json(&enforce_retention(&data_dir, RetentionTrigger::Operator)?)
            }
        },
        Command::Scheduler(args) => handle_scheduler(&data_dir, &drag_bin, args),
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

pub(crate) fn handle_scheduler(
    data_dir: &Path,
    drag_bin: &Path,
    args: SchedulerArgs,
) -> Result<(), CompanionError> {
    migrate_scheduler_state(data_dir)?;
    match args.operation {
        SchedulerOperation::Install(args) => install_scheduler(data_dir, drag_bin, &args),
        SchedulerOperation::Uninstall(args) => uninstall_scheduler(data_dir, &args),
        SchedulerOperation::Enable => set_scheduler_enabled(data_dir, true),
        SchedulerOperation::Disable => set_scheduler_enabled(data_dir, false),
        SchedulerOperation::Status => print_json(&scheduler_status(data_dir)?),
        SchedulerOperation::CatchUp(args) => scheduler_catch_up(data_dir, drag_bin, args),
        SchedulerOperation::Run(args) => scheduler_run_date(data_dir, drag_bin, args.date),
    }
}

pub(crate) const TEMPO_ACCOUNT: &str = "default";
pub(crate) const LEASE_TTL_MS: i64 = 30_000;
pub(crate) const READ_ONLY_RETRIES: u32 = 2;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CoordinatedRunResult {
    pub(crate) date: NaiveDate,
    pub(crate) status: &'static str,
    pub(crate) mode: &'static str,
    pub(crate) owner: RunOwner,
    pub(crate) resumed: bool,
    pub(crate) recovered_lease: bool,
    pub(crate) skipped_confirmed_work: bool,
    pub(crate) submission_entered: bool,
    pub(crate) network_access: bool,
    pub(crate) live_mutation_allowed: bool,
    pub(crate) phases: Vec<RunPhaseRecord>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunOwner {
    pub(crate) tempo_account: &'static str,
    pub(crate) local_date: NaiveDate,
    pub(crate) owner_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunPhaseRecord {
    pub(crate) phase: String,
    pub(crate) state: String,
    pub(crate) attempt: u32,
    pub(crate) started_at: String,
    pub(crate) finished_at: Option<String>,
}

pub(crate) struct AdvisoryRunLock {
    pub(crate) _file: File,
}

pub(crate) struct CompanionStateLock {
    pub(crate) _file: File,
}

pub(crate) fn acquire_companion_state_lock(
    data_dir: &Path,
    exclusive: bool,
) -> Result<CompanionStateLock, CompanionError> {
    let identity = if data_dir.is_absolute() {
        data_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| CompanionError::Read {
                path: PathBuf::from("."),
                source,
            })?
            .join(data_dir)
    };
    let lock_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("drag-companion-locks");
    fs::create_dir_all(&lock_dir).map_err(|source| CompanionError::CreateDir {
        path: lock_dir.clone(),
        source,
    })?;
    let digest = Sha256::digest(identity.to_string_lossy().as_bytes());
    let path = lock_dir.join(format!("state-{digest:x}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| CompanionError::Open { path, source })?;
    let lock_result = if exclusive {
        FileExt::try_lock_exclusive(&file)
    } else {
        FileExt::try_lock_shared(&file)
    };
    lock_result.map_err(|_| {
        CompanionError::Proposal(
            "companion state is busy; retry after the active command completes".to_owned(),
        )
    })?;
    Ok(CompanionStateLock { _file: file })
}

pub(crate) fn coordinated_run(
    data_dir: &Path,
    drag_bin: &Path,
    date: NaiveDate,
    resume: bool,
) -> Result<CoordinatedRunResult, CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    let _lock = acquire_advisory_lock(data_dir, date)?;
    let mut conn = Connection::open(store_path(data_dir))?;
    migrate(&mut conn)?;
    migrate_run_coordination(&conn)?;
    drop(conn);
    let _retention = enforce_retention(data_dir, RetentionTrigger::Lifecycle)?;
    let conn = Connection::open(store_path(data_dir))?;
    if resume && date_has_mutation_operations(&conn, date)? {
        reconcile_complete_day_and_ledger(&conn, drag_bin, date)?;
    }
    let owner_id = format!("{}:{}", std::process::id(), now_string());
    let (recovered_lease, skipped_confirmed_work) = acquire_sqlite_lease(&conn, date, &owner_id)?;

    if let Some(status) = terminal_run_status(&conn, date)? {
        release_sqlite_lease(&conn, date, &owner_id)?;
        return Ok(CoordinatedRunResult {
            date,
            status,
            mode: DEFAULT_MODE,
            owner: RunOwner {
                tempo_account: TEMPO_ACCOUNT,
                local_date: date,
                owner_id,
            },
            resumed: resume,
            recovered_lease,
            skipped_confirmed_work: true,
            submission_entered: status != "blocked",
            network_access: false,
            live_mutation_allowed: false,
            phases: load_phase_records(&conn, date)?,
        });
    }

    let mut submission_entered = false;
    let phases = [
        "collecting",
        "model",
        "tempo_read",
        "pre_mutation",
        "submitting",
        "completed",
    ];
    for phase in phases {
        if phase_completed(&conn, date, phase)? {
            continue;
        }
        if phase == "submitting" {
            submission_entered = true;
        }
        if let Err(error) = run_phase(&conn, date, &owner_id, phase) {
            let _ = release_sqlite_lease(&conn, date, &owner_id);
            return Err(error);
        }
        if let Ok(ms) = std::env::var("DRAG_COMPANION_TEST_HOLD_MS")
            .unwrap_or_default()
            .parse::<u64>()
        {
            if ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
        heartbeat_lease(&conn, date, &owner_id)?;
    }
    finish_run(&conn, date, "completed")?;
    release_sqlite_lease(&conn, date, &owner_id)?;
    let result = CoordinatedRunResult {
        date,
        status: "completed",
        mode: DEFAULT_MODE,
        owner: RunOwner {
            tempo_account: TEMPO_ACCOUNT,
            local_date: date,
            owner_id,
        },
        resumed: resume,
        recovered_lease,
        skipped_confirmed_work,
        submission_entered,
        network_access: false,
        live_mutation_allowed: false,
        phases: load_phase_records(&conn, date)?,
    };
    persist_result(data_dir, &terminal_result(date))?;
    Ok(result)
}

pub(crate) fn acquire_advisory_lock(
    data_dir: &Path,
    date: NaiveDate,
) -> Result<AdvisoryRunLock, CompanionError> {
    let lock_dir = data_dir.join("locks");
    fs::create_dir_all(&lock_dir).map_err(|source| CompanionError::CreateDir {
        path: lock_dir.clone(),
        source,
    })?;
    let path = lock_dir.join(format!("{TEMPO_ACCOUNT}-{date}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| CompanionError::Open { path, source })?;
    file.try_lock_exclusive()
        .map_err(|_| CompanionError::RunOwned {
            account: TEMPO_ACCOUNT.to_owned(),
            date,
            owner: "os-lock".to_owned(),
            expires_at: "unknown".to_owned(),
        })?;
    Ok(AdvisoryRunLock { _file: file })
}

pub(crate) fn migrate_run_coordination(conn: &Connection) -> Result<(), CompanionError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS run_leases (tempo_account TEXT NOT NULL, local_date TEXT NOT NULL, owner_id TEXT NOT NULL, heartbeat_at TEXT NOT NULL, expires_at_ms INTEGER NOT NULL, recovered_from TEXT, PRIMARY KEY (tempo_account, local_date));
         CREATE TABLE IF NOT EXISTS run_phases (tempo_account TEXT NOT NULL, local_date TEXT NOT NULL, phase TEXT NOT NULL, state TEXT NOT NULL, attempt INTEGER NOT NULL, started_at TEXT NOT NULL, finished_at TEXT, error TEXT, PRIMARY KEY (tempo_account, local_date, phase, attempt));
         CREATE TABLE IF NOT EXISTS coordinated_runs (tempo_account TEXT NOT NULL, local_date TEXT NOT NULL, state TEXT NOT NULL, started_at TEXT NOT NULL, finished_at TEXT, PRIMARY KEY (tempo_account, local_date));"
    )?;
    for ddl in [
        "ALTER TABLE mutation_operations ADD COLUMN local_date TEXT",
        "ALTER TABLE mutation_operations ADD COLUMN tempo_account TEXT",
        "ALTER TABLE mutation_operations ADD COLUMN payload_json TEXT",
        "ALTER TABLE mutation_operations ADD COLUMN submitting_intent_json TEXT",
        "ALTER TABLE mutation_operations ADD COLUMN tempo_worklog_id TEXT",
        "ALTER TABLE mutation_operations ADD COLUMN policy_schema_version INTEGER",
        "ALTER TABLE mutation_operations ADD COLUMN payload_schema_version INTEGER",
    ] {
        if let Err(error) = conn.execute(ddl, []) {
            if !error.to_string().contains("duplicate column name") {
                return Err(error.into());
            }
        }
    }
    Ok(())
}
