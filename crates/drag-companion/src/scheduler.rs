use crate::*;

pub(crate) fn scheduler_state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("scheduler.json")
}

pub(crate) fn scheduler_kill_switch_path(data_dir: &Path) -> PathBuf {
    data_dir.join("scheduler.kill")
}

pub(crate) fn scheduler_status(data_dir: &Path) -> Result<Value, CompanionError> {
    let state_path = scheduler_state_path(data_dir);
    let state = if state_path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&state_path).map_err(|source| {
            CompanionError::Read {
                path: state_path.clone(),
                source,
            }
        })?)
        .map_err(|error| CompanionError::Proposal(format!("scheduler state schema: {error}")))?
    } else {
        serde_json::json!({})
    };
    Ok(serde_json::json!({
        "status": "ok",
        "schemaVersion": SCHEDULER_SCHEMA_VERSION,
        "enabled": state.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        "killSwitchActive": scheduler_kill_switch_path(data_dir).exists() || std::env::var_os("DRAG_COMPANION_KILL_SWITCH").is_some(),
        "mode": DEFAULT_MODE,
        "shadowModeForced": scheduler_kill_switch_path(data_dir).exists() || std::env::var_os("DRAG_COMPANION_KILL_SWITCH").is_some(),
        "dragMachineContract": { "requiredVersion": DRAG_MACHINE_CONTRACT_VERSION, "compatible": true },
        "package": { "name": "drag-companion", "independent": true },
        "state": state,
    }))
}

pub(crate) fn install_scheduler(
    data_dir: &Path,
    drag_bin: &Path,
    args: &SchedulerInstallArgs,
) -> Result<(), CompanionError> {
    fs::create_dir_all(&args.target_dir).map_err(|source| CompanionError::CreateDir {
        path: args.target_dir.clone(),
        source,
    })?;
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    validate_time_and_timezone(&args.at, &args.timezone)?;
    let timezone_prefix = if args.timezone == "local" {
        String::new()
    } else {
        format!("TZ={} ", shell_quote(&args.timezone))
    };
    let companion = shell_quote(
        &std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("drag-companion"))
            .to_string_lossy(),
    );
    let command = format!(
        "{}{} --data-dir {} --drag-bin {} scheduler run --date \"$({}date +%F)\"",
        timezone_prefix,
        companion,
        shell_quote(&data_dir.to_string_lossy()),
        shell_quote(&drag_bin.to_string_lossy()),
        timezone_prefix,
    );
    let catch_up_command = format!(
        "{}{} --data-dir {} --drag-bin {} scheduler catch-up",
        timezone_prefix,
        companion,
        shell_quote(&data_dir.to_string_lossy()),
        shell_quote(&drag_bin.to_string_lossy()),
    );
    let installed = if args.platform == "launchd" {
        if args.timezone != "local" {
            return Err(CompanionError::Proposal(
                "launchd calendar intervals use the system timezone; configure local or use systemd for an explicit IANA timezone"
                    .to_owned(),
            ));
        }
        let plist = args.target_dir.join("email.trevors.drag-companion.plist");
        let catch_up_plist = args
            .target_dir
            .join("email.trevors.drag-companion.catch-up.plist");
        write_owned_file(&plist, &render_launchd(&command, &args.at, &args.timezone)?)?;
        write_owned_file(
            &catch_up_plist,
            &render_launchd_catch_up(&catch_up_command)?,
        )?;
        vec![plist, catch_up_plist]
    } else {
        let service = args.target_dir.join("drag-companion.service");
        let timer = args.target_dir.join("drag-companion.timer");
        let catch_up_service = args.target_dir.join("drag-companion-catch-up.service");
        write_owned_file(&service, &render_systemd_service(&command))?;
        write_owned_file(&timer, &render_systemd_timer(&args.at, &args.timezone)?)?;
        write_owned_file(
            &catch_up_service,
            &render_systemd_catch_up_service(&catch_up_command),
        )?;
        vec![service, timer, catch_up_service]
    };
    write_scheduler_state(
        data_dir,
        serde_json::json!({
            "schemaVersion": SCHEDULER_SCHEMA_VERSION,
            "enabled": true,
            "platform": args.platform,
            "at": args.at,
            "timezone": args.timezone,
            "installedFiles": installed,
            "operationKeys": [],
        }),
    )?;
    print_json(
        &serde_json::json!({ "status": "installed", "hostSchedulerMutated": false, "installedFiles": installed }),
    )
}

pub(crate) fn uninstall_scheduler(
    data_dir: &Path,
    args: &SchedulerInstallArgs,
) -> Result<(), CompanionError> {
    let names = [
        "drag-companion.service",
        "drag-companion.timer",
        "drag-companion-catch-up.service",
        "email.trevors.drag-companion.plist",
        "email.trevors.drag-companion.catch-up.plist",
    ];
    let mut removed = Vec::new();
    for name in names {
        let path = args.target_dir.join(name);
        if path.exists() && is_owned_scheduler_file(&path)? {
            fs::remove_file(&path).map_err(|source| CompanionError::Write {
                path: path.clone(),
                source,
            })?;
            removed.push(path);
        }
    }
    write_scheduler_state(
        data_dir,
        serde_json::json!({
            "schemaVersion": SCHEDULER_SCHEMA_VERSION,
            "enabled": false,
            "removedFiles": removed,
            "operationKeys": scheduler_status(data_dir)?.get("state").and_then(|s| s.get("operationKeys")).cloned().unwrap_or_else(|| serde_json::json!([])),
        }),
    )?;
    print_json(
        &serde_json::json!({ "status": "uninstalled", "hostSchedulerMutated": false, "removedFiles": removed }),
    )
}

pub(crate) fn set_scheduler_enabled(data_dir: &Path, enabled: bool) -> Result<(), CompanionError> {
    let mut state = scheduler_status(data_dir)?["state"].clone();
    state["schemaVersion"] = serde_json::json!(SCHEDULER_SCHEMA_VERSION);
    state["enabled"] = serde_json::json!(enabled);
    if state.get("operationKeys").is_none() {
        state["operationKeys"] = serde_json::json!([]);
    }
    if state.get("resumable").is_none() {
        state["resumable"] = serde_json::json!(true);
    }
    write_scheduler_state(data_dir, state)?;
    print_json(
        &serde_json::json!({ "status": if enabled { "enabled" } else { "disabled" }, "hostSchedulerMutated": false }),
    )
}

pub(crate) fn scheduler_catch_up(
    data_dir: &Path,
    drag_bin: &Path,
    args: SchedulerCatchUpArgs,
) -> Result<(), CompanionError> {
    let status = scheduler_status(data_dir)?;
    if status["killSwitchActive"].as_bool().unwrap_or(false)
        || !status["enabled"].as_bool().unwrap_or(true)
    {
        return print_json(
            &serde_json::json!({ "status": "shadow", "selectedDate": null, "mutationAllowed": false }),
        );
    }
    let today = args
        .today
        .unwrap_or_else(|| chrono::Local::now().date_naive());
    let state_last_success = status["state"]
        .get("lastSuccessfulDate")
        .and_then(Value::as_str)
        .and_then(|raw| NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok());
    let selected = latest_eligible_missed_workday(today, args.last_success.or(state_last_success));
    if let Some(date) = selected {
        scheduler_run_date(data_dir, drag_bin, date)
    } else {
        print_json(
            &serde_json::json!({ "status": "no-op", "selectedDate": null, "mutationAllowed": false }),
        )
    }
}

pub(crate) fn scheduler_run_date(
    data_dir: &Path,
    drag_bin: &Path,
    date: NaiveDate,
) -> Result<(), CompanionError> {
    let status = scheduler_status(data_dir)?;
    if status["killSwitchActive"].as_bool().unwrap_or(false) {
        return print_json(
            &serde_json::json!({ "status": "shadow", "date": date, "mutationAllowed": false, "reason": "kill-switch" }),
        );
    }
    if !status["enabled"].as_bool().unwrap_or(true) {
        return print_json(
            &serde_json::json!({ "status": "disabled", "date": date, "mutationAllowed": false }),
        );
    }
    let op_key = format!("scheduler.run.{date}");
    let mut state = status["state"].clone();
    let mut keys = state["operationKeys"].as_array().cloned().ok_or_else(|| {
        CompanionError::Proposal(
            "scheduler state schema: operationKeys must be an array of strings".to_owned(),
        )
    })?;
    let existing_key = keys.iter().any(|key| key == &serde_json::json!(op_key));
    if existing_key && run_path(data_dir, date).exists() {
        return print_json(
            &serde_json::json!({ "status": "duplicate", "date": date, "operationKey": op_key, "mutationAllowed": false }),
        );
    }
    if !existing_key {
        keys.push(serde_json::json!(op_key));
        state["operationKeys"] = Value::Array(keys);
        state["lastAttemptedDate"] = serde_json::json!(date.to_string());
        write_scheduler_state(data_dir, state)?;
    }
    let result = coordinated_run(data_dir, drag_bin, date, existing_key)?;
    let mut state = scheduler_status(data_dir)?["state"].clone();
    if result.status == "completed" {
        state["lastSuccessfulDate"] = serde_json::json!(date.to_string());
        write_scheduler_state(data_dir, state)?;
    }
    print_json(
        &serde_json::json!({ "status": "ran", "date": date, "operationKey": op_key, "mutationAllowed": false, "result": result }),
    )
}

pub(crate) fn latest_eligible_missed_workday(
    today: NaiveDate,
    last_success: Option<NaiveDate>,
) -> Option<NaiveDate> {
    let start = today - Duration::days(7);
    let mut candidate = today - Duration::days(1);
    while candidate >= start {
        let weekday = candidate.weekday();
        if weekday.num_days_from_monday() < 5 && last_success.is_none_or(|last| candidate > last) {
            return Some(candidate);
        }
        candidate -= Duration::days(1);
    }
    None
}

pub(crate) fn render_systemd_service(command: &str) -> String {
    let command = command.replace('%', "%%");
    format!("# managed-by=drag-companion\n[Unit]\nDescription=Drag companion explicit-date reconciliation\n[Service]\nType=oneshot\nExecStart=/bin/sh -c {}\n", shell_quote(&command))
}

pub(crate) fn render_systemd_catch_up_service(command: &str) -> String {
    let command = command.replace('%', "%%");
    format!("# managed-by=drag-companion\n[Unit]\nDescription=Catch up missed Drag companion reconciliation after startup\n[Service]\nType=oneshot\nExecStart=/bin/sh -c {}\n[Install]\nWantedBy=default.target\n", shell_quote(&command))
}

pub(crate) fn render_systemd_timer(at: &str, timezone: &str) -> Result<String, CompanionError> {
    validate_time_and_timezone(at, timezone)?;
    let timezone_suffix = if timezone == "local" {
        String::new()
    } else {
        format!(" {timezone}")
    };
    Ok(format!("# managed-by=drag-companion\n[Unit]\nDescription=Run Drag companion at {at} {timezone}\n[Timer]\nOnCalendar=*-*-* {at}:00{timezone_suffix}\nPersistent=false\nWakeSystem=false\n[Install]\nWantedBy=timers.target\n"))
}

pub(crate) fn render_launchd(
    command: &str,
    at: &str,
    timezone: &str,
) -> Result<String, CompanionError> {
    validate_time_and_timezone(at, timezone)?;
    let (hour, minute) = at.split_once(':').unwrap_or(("18", "45"));
    Ok(format!("<!-- managed-by=drag-companion timezone={} -->\n<plist version=\"1.0\"><dict><key>Label</key><string>email.trevors.drag-companion</string><key>ProgramArguments</key><array><string>/bin/sh</string><string>-lc</string><string>{}</string></array><key>StartCalendarInterval</key><dict><key>Hour</key><integer>{hour}</integer><key>Minute</key><integer>{minute}</integer></dict></dict></plist>\n", xml_escape(timezone), xml_escape(command)))
}

pub(crate) fn render_launchd_catch_up(command: &str) -> Result<String, CompanionError> {
    Ok(format!("<!-- managed-by=drag-companion timezone=local -->\n<plist version=\"1.0\"><dict><key>Label</key><string>email.trevors.drag-companion.catch-up</string><key>ProgramArguments</key><array><string>/bin/sh</string><string>-lc</string><string>{}</string></array><key>RunAtLoad</key><true/></dict></plist>\n", xml_escape(command)))
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(crate) fn validate_time_and_timezone(at: &str, timezone: &str) -> Result<(), CompanionError> {
    let (hour, minute) = at
        .split_once(':')
        .ok_or_else(|| CompanionError::Proposal("invalid scheduler time".to_owned()))?;
    let hour: u32 = hour
        .parse()
        .map_err(|_| CompanionError::Proposal("invalid scheduler hour".to_owned()))?;
    let minute: u32 = minute
        .parse()
        .map_err(|_| CompanionError::Proposal("invalid scheduler minute".to_owned()))?;
    if hour > 23 || minute > 59 {
        return Err(CompanionError::Proposal(
            "invalid scheduler time".to_owned(),
        ));
    }
    if timezone != "local" {
        timezone
            .parse::<Tz>()
            .map_err(|_| CompanionError::Proposal("invalid scheduler timezone".to_owned()))?;
    }
    Ok(())
}

pub(crate) fn is_owned_scheduler_file(path: &Path) -> Result<bool, CompanionError> {
    let content = fs::read_to_string(path).map_err(|source| CompanionError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(content.contains("managed-by=drag-companion"))
}

pub(crate) fn write_owned_file(path: &Path, content: &str) -> Result<(), CompanionError> {
    if path.exists() && !is_owned_scheduler_file(path)? {
        return Err(CompanionError::Proposal(format!(
            "refusing to overwrite unrelated file {}",
            path.display()
        )));
    }
    atomic_write(path, content.as_bytes())
}

pub(crate) fn write_scheduler_state(data_dir: &Path, state: Value) -> Result<(), CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    let path = scheduler_state_path(data_dir);
    let body = serde_json::to_vec_pretty(&state).map_err(CompanionError::Serialize)?;
    if path.exists() {
        let backup = path.with_extension("json.bak");
        fs::copy(&path, &backup).map_err(|source| CompanionError::Write {
            path: backup,
            source,
        })?;
    }
    atomic_write(&path, &body)
}

pub(crate) fn migrate_scheduler_state(data_dir: &Path) -> Result<(), CompanionError> {
    let path = scheduler_state_path(data_dir);
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&path).map_err(|source| CompanionError::Read {
        path: path.clone(),
        source,
    })?;
    let mut state: Value = serde_json::from_str(&raw)
        .map_err(|error| CompanionError::Proposal(format!("scheduler state schema: {error}")))?;
    let object = state.as_object_mut().ok_or_else(|| {
        CompanionError::Proposal("scheduler state schema: expected a JSON object".to_owned())
    })?;
    let version = match object.get("schemaVersion") {
        Some(value) => value.as_u64().ok_or_else(|| {
            CompanionError::Proposal(
                "scheduler state schema: schemaVersion must be an unsigned integer".to_owned(),
            )
        })?,
        None => 0,
    };
    if version > u64::from(SCHEDULER_SCHEMA_VERSION) {
        return Err(CompanionError::Proposal(format!(
            "scheduler state schema version {version} is newer than supported version {SCHEDULER_SCHEMA_VERSION}"
        )));
    }
    if version < u64::from(SCHEDULER_SCHEMA_VERSION) {
        object.insert(
            "schemaVersion".to_owned(),
            serde_json::json!(SCHEDULER_SCHEMA_VERSION),
        );
        object.insert("resumable".to_owned(), serde_json::json!(true));
        if !object.contains_key("operationKeys") {
            object.insert("operationKeys".to_owned(), serde_json::json!([]));
        }
    }
    validate_scheduler_state(object)?;
    if version < u64::from(SCHEDULER_SCHEMA_VERSION) {
        write_scheduler_state(data_dir, state)?;
    }
    Ok(())
}

pub(crate) fn validate_scheduler_state(
    object: &serde_json::Map<String, Value>,
) -> Result<(), CompanionError> {
    if object
        .get("operationKeys")
        .and_then(Value::as_array)
        .is_none_or(|keys| !keys.iter().all(Value::is_string))
    {
        return Err(CompanionError::Proposal(
            "scheduler state schema: operationKeys must be an array of strings".to_owned(),
        ));
    }
    for field in ["platform", "at", "timezone", "lastAttemptedDate"] {
        if object.get(field).is_some_and(|value| !value.is_string()) {
            return Err(CompanionError::Proposal(format!(
                "scheduler state schema: {field} must be a string"
            )));
        }
    }
    if object.get("installedFiles").is_some_and(|value| {
        value
            .as_array()
            .is_none_or(|items| !items.iter().all(Value::is_string))
    }) {
        return Err(CompanionError::Proposal(
            "scheduler state schema: installedFiles must be an array of strings".to_owned(),
        ));
    }
    for field in ["enabled", "resumable"] {
        if object.get(field).is_some_and(|value| !value.is_boolean()) {
            return Err(CompanionError::Proposal(format!(
                "scheduler state schema: {field} must be a boolean"
            )));
        }
    }
    Ok(())
}
