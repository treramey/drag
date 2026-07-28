use crate::*;

pub(crate) fn scheduler_state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("scheduler.json")
}

pub(crate) fn scheduler_kill_switch_path(data_dir: &Path) -> PathBuf {
    data_dir.join("scheduler.kill")
}

pub(crate) fn tracking_kill_switch_active() -> bool {
    std::env::var_os("DRAG_TRACKING_KILL_SWITCH").is_some()
        || std::env::var_os("DRAG_COMPANION_KILL_SWITCH").is_some()
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
        "killSwitchActive": scheduler_kill_switch_path(data_dir).exists() || tracking_kill_switch_active(),
        "mode": DEFAULT_MODE,
        "shadowModeForced": scheduler_kill_switch_path(data_dir).exists() || tracking_kill_switch_active(),
        "dragMachineContract": { "requiredVersion": DRAG_MACHINE_CONTRACT_VERSION, "compatible": true },
        "package": { "name": "drag-tracking", "independent": true },
        "state": state,
    }))
}

pub(crate) fn install_scheduler(
    data_dir: &Path,
    drag_bin: &Path,
    args: &SchedulerInstallArgs,
) -> Result<(), CompanionError> {
    print_json(&install_scheduler_files(data_dir, drag_bin, args)?)
}

pub(crate) fn install_scheduler_files(
    data_dir: &Path,
    drag_bin: &Path,
    args: &SchedulerInstallArgs,
) -> Result<Value, CompanionError> {
    reject_unsupported_scheduler_host()?;
    validate_time_and_timezone(&args.at, &args.timezone)?;
    if args.platform == "launchd" && args.timezone != "local" {
        return Err(CompanionError::Proposal(
            "launchd calendar intervals use the system timezone; configure local or use systemd for an explicit IANA timezone"
                .to_owned(),
        ));
    }
    let mut state = scheduler_status(data_dir)?["state"].clone();
    let installed = if args.platform == "launchd" {
        vec![
            args.target_dir.join("email.trevors.drag-tracking.plist"),
            args.target_dir
                .join("email.trevors.drag-tracking.catch-up.plist"),
        ]
    } else {
        vec![
            args.target_dir.join("drag-tracking.service"),
            args.target_dir.join("drag-tracking.timer"),
            args.target_dir.join("drag-tracking-catch-up.service"),
        ]
    };
    preflight_scheduler_destinations(&installed)?;
    let timezone_prefix = if args.timezone == "local" {
        String::new()
    } else {
        format!("TZ={} ", shell_quote(&args.timezone))
    };
    let config_prefix = std::env::var_os("DRAG_CONFIG").map_or_else(String::new, |path| {
        format!("DRAG_CONFIG={} ", shell_quote(&path.to_string_lossy()))
    });
    let companion = shell_quote(
        &std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("drag-tracking"))
            .to_string_lossy(),
    );
    let command = format!(
        "{}{}{} --data-dir {} --drag-bin {} internal scheduler run --date \"$({}date +%F)\"",
        timezone_prefix,
        config_prefix,
        companion,
        shell_quote(&data_dir.to_string_lossy()),
        shell_quote(&drag_bin.to_string_lossy()),
        timezone_prefix,
    );
    let catch_up_command = format!(
        "{}{}{} --data-dir {} --drag-bin {} internal scheduler catch-up",
        timezone_prefix,
        config_prefix,
        companion,
        shell_quote(&data_dir.to_string_lossy()),
        shell_quote(&drag_bin.to_string_lossy()),
    );
    let rendered = if args.platform == "launchd" {
        vec![
            render_launchd(&command, &args.at, &args.timezone)?,
            render_launchd_catch_up(&catch_up_command)?,
        ]
    } else {
        vec![
            render_systemd_service(&command),
            render_systemd_timer(&args.at, &args.timezone)?,
            render_systemd_catch_up_service(&catch_up_command),
        ]
    };
    fs::create_dir_all(&args.target_dir).map_err(|source| CompanionError::CreateDir {
        path: args.target_dir.clone(),
        source,
    })?;
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    remove_owned_legacy_scheduler_files(&args.target_dir)?;
    for (path, content) in installed.iter().zip(rendered) {
        write_owned_file(path, &content)?;
    }
    let installed_file_hashes = installed
        .iter()
        .map(|path| {
            let content = fs::read_to_string(path).map_err(|source| CompanionError::Read {
                path: path.clone(),
                source,
            })?;
            Ok((
                path.to_string_lossy().into_owned(),
                Value::String(sha256_str(&content)),
            ))
        })
        .collect::<Result<serde_json::Map<String, Value>, CompanionError>>()?;
    let desired_enabled = state
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let host_scheduler_mutated =
        mutate_host_scheduler(&args.platform, &args.target_dir, desired_enabled)?;
    state["schemaVersion"] = serde_json::json!(SCHEDULER_SCHEMA_VERSION);
    state["enabled"] = serde_json::json!(desired_enabled && host_scheduler_mutated);
    if state.get("operationKeys").is_none() {
        state["operationKeys"] = serde_json::json!([]);
    }
    state["platform"] = serde_json::json!(args.platform);
    state["at"] = serde_json::json!(args.at);
    state["timezone"] = serde_json::json!(args.timezone);
    state["installedFiles"] = serde_json::json!(installed);
    state["installedFileHashes"] = Value::Object(installed_file_hashes);
    write_scheduler_state(data_dir, state)?;
    Ok(serde_json::json!({
        "status": "installed",
        "hostSchedulerMutated": host_scheduler_mutated,
        "active": desired_enabled && host_scheduler_mutated,
        "installedFiles": installed,
        "activationRequired": !host_scheduler_mutated
    }))
}

fn preflight_scheduler_destinations(paths: &[PathBuf]) -> Result<(), CompanionError> {
    for path in paths {
        if path.exists() && !is_owned_scheduler_file(path)? {
            return Err(CompanionError::Proposal(format!(
                "refusing to overwrite unrelated file {}",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn reject_unsupported_scheduler_host() -> Result<(), CompanionError> {
    Err(CompanionError::Proposal(
        "scheduler installation is not supported on Windows".to_owned(),
    ))
}

#[cfg(not(target_os = "windows"))]
fn reject_unsupported_scheduler_host() -> Result<(), CompanionError> {
    Ok(())
}

fn remove_owned_legacy_scheduler_files(target_dir: &Path) -> Result<(), CompanionError> {
    for name in [
        "drag-companion.service",
        "drag-companion.timer",
        "drag-companion-catch-up.service",
        "email.trevors.drag-companion.plist",
        "email.trevors.drag-companion.catch-up.plist",
    ] {
        let path = target_dir.join(name);
        if path.exists() && is_owned_scheduler_file(&path)? {
            fs::remove_file(&path).map_err(|source| CompanionError::Write {
                path: path.clone(),
                source,
            })?;
        }
    }
    Ok(())
}

pub(crate) fn uninstall_scheduler(
    data_dir: &Path,
    args: &SchedulerInstallArgs,
) -> Result<(), CompanionError> {
    print_json(&uninstall_scheduler_files(data_dir, args)?)
}

pub(crate) fn uninstall_scheduler_files(
    data_dir: &Path,
    args: &SchedulerInstallArgs,
) -> Result<Value, CompanionError> {
    let host_scheduler_mutated = mutate_host_scheduler(&args.platform, &args.target_dir, false)?;
    let names = [
        "drag-tracking.service",
        "drag-tracking.timer",
        "drag-tracking-catch-up.service",
        "email.trevors.drag-tracking.plist",
        "email.trevors.drag-tracking.catch-up.plist",
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
    Ok(
        serde_json::json!({ "status": "uninstalled", "hostSchedulerMutated": host_scheduler_mutated, "removedFiles": removed }),
    )
}

pub(crate) fn set_scheduler_enabled(data_dir: &Path, enabled: bool) -> Result<(), CompanionError> {
    print_json(&set_scheduler_enabled_state(data_dir, enabled)?)
}

pub(crate) fn set_scheduler_enabled_state(
    data_dir: &Path,
    enabled: bool,
) -> Result<Value, CompanionError> {
    let mut state = scheduler_status(data_dir)?["state"].clone();
    let has_installed_files = state["installedFiles"].as_array().is_some();
    let host_scheduler_mutated = if let (Some(platform), Some(installed_files)) = (
        state["platform"].as_str(),
        state["installedFiles"].as_array(),
    ) {
        let first = installed_files
            .first()
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| {
                CompanionError::Proposal("scheduler state has invalid installed files".to_owned())
            })?;
        let target_dir = first.parent().ok_or_else(|| {
            CompanionError::Proposal("scheduler target directory is invalid".to_owned())
        })?;
        validate_owned_scheduler_files(&state)?;
        mutate_host_scheduler(platform, target_dir, enabled)?
    } else {
        false
    };
    state["schemaVersion"] = serde_json::json!(SCHEDULER_SCHEMA_VERSION);
    state["enabled"] =
        serde_json::json!(enabled && (!has_installed_files || host_scheduler_mutated));
    if state.get("operationKeys").is_none() {
        state["operationKeys"] = serde_json::json!([]);
    }
    if state.get("resumable").is_none() {
        state["resumable"] = serde_json::json!(true);
    }
    write_scheduler_state(data_dir, state)?;
    Ok(serde_json::json!({
        "status": if enabled && host_scheduler_mutated { "enabled" } else if enabled { "activation-required" } else { "disabled" },
        "hostSchedulerMutated": host_scheduler_mutated
    }))
}

pub(crate) fn validate_owned_scheduler_files(state: &Value) -> Result<(), CompanionError> {
    let files = state["installedFiles"].as_array().ok_or_else(|| {
        CompanionError::Proposal("scheduler state has no installed files".to_owned())
    })?;
    for file in files {
        let path = file.as_str().map(PathBuf::from).ok_or_else(|| {
            CompanionError::Proposal("scheduler state has an invalid installed file".to_owned())
        })?;
        if !path.exists() || !is_owned_scheduler_file(&path)? {
            return Err(CompanionError::Proposal(format!(
                "tracking-owned scheduler file is missing or modified: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

pub(crate) fn scheduler_files_healthy(state: &Value) -> bool {
    let Some(files) = state.get("installedFiles").and_then(Value::as_array) else {
        return true;
    };
    let hashes = state.get("installedFileHashes").and_then(Value::as_object);
    files.iter().all(|file| {
        file.as_str().is_some_and(|path| {
            let path = Path::new(path);
            path.exists()
                && is_owned_scheduler_file(path).unwrap_or(false)
                && hashes
                    .and_then(|hashes| hashes.get(path.to_string_lossy().as_ref()))
                    .and_then(Value::as_str)
                    .is_some_and(|expected| {
                        fs::read_to_string(path)
                            .is_ok_and(|content| sha256_str(&content) == expected)
                    })
        })
    })
}

pub(crate) fn scheduler_files_installed(state: &Value) -> bool {
    state
        .get("installedFiles")
        .and_then(Value::as_array)
        .is_some_and(|files| !files.is_empty())
}

fn mutate_host_scheduler(
    platform: &str,
    target_dir: &Path,
    enabled: bool,
) -> Result<bool, CompanionError> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(false);
    };
    let expected = if platform == "launchd" {
        home.join("Library/LaunchAgents")
    } else {
        home.join(".config/systemd/user")
    };
    let target = std::path::absolute(target_dir).map_err(|source| CompanionError::Read {
        path: target_dir.to_path_buf(),
        source,
    })?;
    if target != expected {
        return Ok(false);
    }
    if platform == "launchd" {
        for name in [
            "email.trevors.drag-tracking.plist",
            "email.trevors.drag-tracking.catch-up.plist",
        ] {
            let mut command = ProcessCommand::new("launchctl");
            command.arg(if enabled { "load" } else { "unload" });
            if enabled {
                command.arg("-w");
            }
            run_scheduler_command(command.arg(target.join(name)))?;
        }
    } else {
        let mut reload = ProcessCommand::new("systemctl");
        reload.args(["--user", "daemon-reload"]);
        run_scheduler_command(&mut reload)?;
        let mut command = ProcessCommand::new("systemctl");
        command.args(if enabled {
            ["--user", "enable", "--now"]
        } else {
            ["--user", "disable", "--now"]
        });
        command.args(["drag-tracking.timer", "drag-tracking-catch-up.service"]);
        run_scheduler_command(&mut command)?;
    }
    Ok(true)
}

fn run_scheduler_command(command: &mut ProcessCommand) -> Result<(), CompanionError> {
    let output = command.output().map_err(|source| {
        CompanionError::Proposal(format!("could not invoke host scheduler: {source}"))
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(CompanionError::Proposal(format!(
            "host scheduler rejected activation: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
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
    let already_succeeded = state["successfulOperationKeys"]
        .as_array()
        .is_some_and(|keys| keys.iter().any(|key| key == &serde_json::json!(op_key)))
        || (!config_path(data_dir).exists() && existing_key && run_path(data_dir, date).exists());
    if existing_key && already_succeeded {
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
    let result = if let Some(config) = load_tracking_config(data_dir)? {
        run_tracking_for_date(data_dir, drag_bin, &config, date)?
    } else {
        serde_json::to_value(coordinated_run(data_dir, drag_bin, date, existing_key)?)
            .map_err(CompanionError::Serialize)?
    };
    let mut state = scheduler_status(data_dir)?["state"].clone();
    if result["status"] == "completed" {
        state["lastSuccessfulDate"] = serde_json::json!(date.to_string());
        let mut successful = state["successfulOperationKeys"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !successful
            .iter()
            .any(|key| key == &serde_json::json!(op_key))
        {
            successful.push(serde_json::json!(op_key));
        }
        state["successfulOperationKeys"] = Value::Array(successful);
        write_scheduler_state(data_dir, state)?;
    }
    let mutation_allowed = result["liveMutationAllowed"].as_bool().unwrap_or(false);
    print_json(
        &serde_json::json!({ "status": "ran", "date": date, "operationKey": op_key, "mutationAllowed": mutation_allowed, "result": result }),
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
    format!("# managed-by=drag-tracking\n[Unit]\nDescription=Drag automatic tracking\n[Service]\nType=oneshot\nExecStart=/bin/sh -c {}\n", shell_quote(&command))
}

pub(crate) fn render_systemd_catch_up_service(command: &str) -> String {
    let command = command.replace('%', "%%");
    format!("# managed-by=drag-tracking\n[Unit]\nDescription=Catch up missed Drag tracking after startup\n[Service]\nType=oneshot\nExecStart=/bin/sh -c {}\n[Install]\nWantedBy=default.target\n", shell_quote(&command))
}

pub(crate) fn render_systemd_timer(at: &str, timezone: &str) -> Result<String, CompanionError> {
    validate_time_and_timezone(at, timezone)?;
    let timezone_suffix = if timezone == "local" {
        String::new()
    } else {
        format!(" {timezone}")
    };
    Ok(format!("# managed-by=drag-tracking\n[Unit]\nDescription=Run Drag automatic tracking at {at} {timezone}\n[Timer]\nOnCalendar=Mon..Fri *-*-* {at}:00{timezone_suffix}\nPersistent=true\n[Install]\nWantedBy=timers.target\n"))
}

pub(crate) fn render_launchd(
    command: &str,
    at: &str,
    timezone: &str,
) -> Result<String, CompanionError> {
    validate_time_and_timezone(at, timezone)?;
    if timezone != "local" {
        return Err(CompanionError::Proposal(
            "launchd calendar intervals use the system timezone; configure local or use systemd for an explicit IANA timezone"
                .to_owned(),
        ));
    }
    let (hour, minute) = at.split_once(':').unwrap_or(("18", "45"));
    let weekdays = (1..=5)
        .map(|weekday| format!("<dict><key>Weekday</key><integer>{weekday}</integer><key>Hour</key><integer>{hour}</integer><key>Minute</key><integer>{minute}</integer></dict>"))
        .collect::<String>();
    Ok(format!("<!-- managed-by=drag-tracking timezone=local -->\n<plist version=\"1.0\"><dict><key>Label</key><string>email.trevors.drag-tracking</string><key>ProgramArguments</key><array><string>/bin/sh</string><string>-lc</string><string>{}</string></array><key>StartCalendarInterval</key><array>{weekdays}</array></dict></plist>\n", xml_escape(command)))
}

pub(crate) fn render_launchd_catch_up(command: &str) -> Result<String, CompanionError> {
    Ok(format!("<!-- managed-by=drag-tracking timezone=local -->\n<plist version=\"1.0\"><dict><key>Label</key><string>email.trevors.drag-tracking.catch-up</string><key>ProgramArguments</key><array><string>/bin/sh</string><string>-lc</string><string>{}</string></array><key>RunAtLoad</key><true/></dict></plist>\n", xml_escape(command)))
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
    Ok(content.contains("managed-by=drag-tracking")
        || content.contains("managed-by=drag-companion"))
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
