use crate::*;

pub(crate) fn setup_tracking(
    data_dir: &Path,
    drag_bin: &Path,
    args: PublicSetupArgs,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    validate_time_and_timezone(&args.at, &args.schedule_timezone)?;
    if args.authorize_automatic && args.mode != SubmissionMode::Automatic {
        return Err(CompanionError::Proposal(
            "--authorize-automatic requires --mode automatic".to_owned(),
        ));
    }
    if args.mode == SubmissionMode::Automatic && !args.authorize_automatic {
        return Err(CompanionError::Proposal(
            "automatic mode requires separate --authorize-automatic consent".to_owned(),
        ));
    }
    if args.install_scheduler && args.scheduler_target.is_none() {
        return Err(CompanionError::Proposal(
            "--install-scheduler requires --scheduler-target DIR".to_owned(),
        ));
    }

    let mut config = load_tracking_config(data_dir)?.unwrap_or_default();
    let sources = configured_sources(args.repos, args.ics_files)?;
    validate_source_configuration(&sources)?;
    config.installed = true;
    config.sources = sources;
    let schedule_was_explicit =
        args.at != DEFAULT_SCHEDULE_TIME || args.schedule_timezone != DEFAULT_SCHEDULE_TIMEZONE;
    if config.scheduler_target.is_none() || schedule_was_explicit {
        config.schedule = TrackingSchedule {
            weekdays: true,
            at: args.at.clone(),
            timezone: args.schedule_timezone.clone(),
        };
    }
    config.submission = TrackingSubmission {
        mode: args.mode,
        automatic_submission_authorized: args.authorize_automatic,
    };

    let scheduler = if let Some(target_dir) = args.scheduler_target {
        let target_dir = absolute_path(&target_dir)?;
        let install = SchedulerInstallArgs {
            platform: default_scheduler_platform().to_owned(),
            target_dir: target_dir.clone(),
            at: args.at,
            timezone: args.schedule_timezone,
        };
        config.scheduler_target = Some(target_dir);
        Some(install_scheduler_files(data_dir, drag_bin, &install)?)
    } else {
        None
    };
    if args.install_hooks {
        install_claude_hooks(&default_claude_settings_path())?;
        config.hooks_installed = true;
    }
    if config.hooks_installed {
        config
            .sources
            .retain(|source| source.kind != TrackingSourceKind::ClaudeCode);
        config.sources.push(claude_code_source()?);
    }
    validate_tracking_configuration(&config)?;
    let scheduler_state = scheduler_status(data_dir)?;
    config.active = config.scheduler_target.is_some()
        && scheduler_state["enabled"].as_bool().unwrap_or(false)
        && scheduler_files_healthy(&scheduler_state["state"]);
    save_tracking_config(data_dir, &config)?;
    let result = serde_json::json!({
        "schemaVersion": TRACKING_MACHINE_CONTRACT_VERSION,
        "status": "configured",
        "installed": config.installed,
        "active": config.active,
        "privacy": {
            "evidenceAccess": "only explicitly configured local sources",
            "rawEvidenceRemainsLocal": true,
            "networkAccess": "only Drag read, preview, and submission boundaries during a run"
        },
        "sources": source_statuses(&config),
        "schedule": config.schedule,
        "submission": {
            "mode": config.submission.mode,
            "automaticSubmissionAuthorized": config.submission.automatic_submission_authorized,
            "effectiveMutationPermission": effective_mutation_permission(data_dir, &config)?
        },
        "effects": {
            "schedulerInstalled": scheduler.is_some(),
            "hooksInstalled": config.hooks_installed,
            "automaticSubmissionAuthorized": config.submission.automatic_submission_authorized
        },
        "scheduler": scheduler,
        "networkAccess": false,
        "liveMutationAllowed": false,
        "nextSafeAction": if config.active { "inspect tracking status before the first scheduled run" } else { "resume tracking after installing its scheduler" }
    });
    print_public(
        output,
        &result,
        "Automatic time tracking configured. Local sources and each installation or submission effect remain independently authorized.",
    )
}

pub(crate) fn tracking_status(
    data_dir: &Path,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let mut status = status_payload(data_dir)?;
    if let Some(config) = load_tracking_config(data_dir)? {
        status["configuration"]["configured"] = Value::Bool(config.installed);
        status["configuration"]["migration"] = migration_status(data_dir)?;
        status["scheduler"]["active"] = Value::Bool(config.active);
        status["scheduler"]["nextRun"] = next_run_description(&config);
        status["submission"]["mode"] =
            serde_json::to_value(config.submission.mode).map_err(CompanionError::Serialize)?;
        status["submission"]["automaticSubmissionAuthorized"] =
            Value::Bool(config.submission.automatic_submission_authorized);
        status["submission"]["effectiveMutationPermission"] =
            Value::Bool(effective_mutation_permission(data_dir, &config)?);
        status["sources"] = Value::Array(source_statuses(&config));
        status["timezone"] = Value::String(config.schedule.timezone.clone());
        status["pendingAction"] = Value::String(
            if status["scheduler"]["healthy"] == Value::Bool(false) {
                "repair the missing or modified tracking scheduler files"
            } else if !config.active {
                "resume tracking after validating the configured schedule and sources"
            } else if status["scheduler"]["killSwitchActive"] == Value::Bool(true) {
                "remove the tracking kill switch only after reviewing recovery state"
            } else {
                "no action required"
            }
            .to_owned(),
        );
    } else {
        status["configuration"]["migration"] = migration_status(data_dir)?;
        status["sources"] = Value::Array(Vec::new());
        status["timezone"] = Value::String("local".to_owned());
    }
    match output {
        Some(TrackingOutputMode::Human) => print_tracking_status(&status),
        Some(TrackingOutputMode::Json) => print_json(&serde_json::json!({
            "ok": true,
            "data": public_tracking_status(&status)
        })),
        None => print_json(&status),
    }
}

pub(crate) fn run_tracking(
    data_dir: &Path,
    drag_bin: &Path,
    args: PublicDateArgs,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let config = require_config(data_dir)?;
    let date = select_public_date(args.when.as_deref(), &config.schedule.timezone)?;
    let result = run_tracking_for_date(data_dir, drag_bin, date)?;
    let status = result["status"].as_str().unwrap_or("unknown");
    print_public(
        output,
        &result,
        &format!(
            "Tracking run for {date}: {status}. Next safe action: {}.",
            next_safe_action(status)
        ),
    )
}

pub(crate) fn run_tracking_for_date(
    data_dir: &Path,
    drag_bin: &Path,
    date: NaiveDate,
) -> Result<Value, CompanionError> {
    let config = require_config(data_dir)?;
    let collect = CollectArgs {
        repos: config
            .sources
            .iter()
            .filter(|source| source.enabled && source.kind == TrackingSourceKind::Git)
            .map(|source| source.path.clone())
            .collect(),
        date: Some(date),
        ics_files: config
            .sources
            .iter()
            .filter(|source| source.enabled && source.kind == TrackingSourceKind::Calendar)
            .map(|source| source.path.clone())
            .collect(),
    };
    let collected = collect_activity(data_dir, &collect)?;
    let imported = if journal_path(data_dir).exists() {
        import_journal(data_dir)?
    } else {
        let mut conn = Connection::open(store_path(data_dir))?;
        migrate(&mut conn)?;
        0
    };
    let bundle = build_bundle(data_dir, date)?;
    let run = coordinated_run(data_dir, drag_bin, date, true)?;
    let before_audit = proposal_counts(data_dir, date)?;
    let approved_review =
        config.submission.mode == SubmissionMode::Review && approval_matches(data_dir, date)?;
    let audit = if before_audit.proposals > 0 {
        Some(audit_drag_day(
            data_dir,
            drag_bin,
            date,
            config.submission.mode == SubmissionMode::Automatic || approved_review,
        )?)
    } else {
        None
    };
    let execution = if config.submission.mode == SubmissionMode::Automatic
        && config.submission.automatic_submission_authorized
        || approved_review
    {
        Some(execute_drag_worklogs(data_dir, drag_bin, date, true)?)
    } else {
        None
    };
    let counts = proposal_counts(data_dir, date)?;
    let status = execution
        .as_ref()
        .map(|value| value.status)
        .filter(|status| *status != "executed")
        .unwrap_or(run.status);
    let warnings = match status {
        "gated" => vec!["submission was blocked by runtime safety gates"],
        "uncertain" => vec!["submission outcome is uncertain; reconcile before retrying"],
        _ => Vec::new(),
    };
    let result = serde_json::json!({
        "schemaVersion": TRACKING_MACHINE_CONTRACT_VERSION,
        "selectedDate": date,
        "status": status,
        "resumed": run.resumed,
        "sourceHealth": source_statuses(&config),
        "observations": collected.git.commits.len() + collected.calendar.events.len(),
        "importedEvidence": imported,
        "evidenceBundle": {
            "items": bundle.evidence.len(),
            "contradictions": bundle.contradictions.len()
        },
        "proposals": counts.proposals,
        "accepted": counts.accepted,
        "rejected": counts.rejected,
        "submitted": execution.as_ref().map_or(0, |value| value.submitted),
        "skipped": execution.as_ref().map_or(0, |value| value.skipped),
        "networkAccess": audit.is_some()
            || execution.as_ref().is_some_and(|value| value.network_access),
        "liveMutationAllowed": execution.as_ref().is_some_and(|value| value.live_mutation_allowed),
        "warnings": warnings,
        "nextSafeAction": next_safe_action(status),
        "phases": run.phases
    });
    Ok(result)
}

pub(crate) fn review_tracking(
    data_dir: &Path,
    mut args: PublicReviewArgs,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let config = require_config(data_dir)?;
    if let Some(PublicReviewOperation::Approve(date)) = args.operation {
        args.when = date.when;
        args.approve = true;
    }
    let date = select_public_date(args.when.as_deref(), &config.schedule.timezone)?;
    let digest = proposal_set_digest(data_dir, date)?;
    if args.approve {
        if config.submission.mode != SubmissionMode::Review {
            return Err(CompanionError::Proposal(
                "proposal approval is available only in review mode".to_owned(),
            ));
        }
        persist_approval(data_dir, date, &digest)?;
    }
    let approval = load_approval(data_dir, date)?;
    let proposals = review_proposals(data_dir, date)?;
    let policy_inputs = if store_path(data_dir).exists() {
        proposal_policy_inputs(data_dir, date)?
    } else {
        Vec::new()
    };
    let evidence = policy_inputs
        .into_iter()
        .map(|value| {
            serde_json::json!({
                "proposalId": value.id,
                "references": value.evidence_refs
            })
        })
        .collect::<Vec<_>>();
    let result = serde_json::json!({
        "schemaVersion": TRACKING_MACHINE_CONTRACT_VERSION,
        "selectedDate": date,
        "proposals": proposals.into_iter().map(|(id, payload)| serde_json::json!({"id": id, "payload": payload})).collect::<Vec<_>>(),
        "evidenceReferences": evidence,
        "policyDecisions": review_decisions(data_dir, date)?,
        "existingWorklogConflicts": [],
        "proposalSetDigest": digest,
        "approval": {
            "approved": approval.as_ref().is_some_and(|value| value["digest"] == digest),
            "record": approval
        },
        "submissionState": terminal_report_status(data_dir, date).unwrap_or("pending"),
        "networkAccess": false,
        "liveMutationAllowed": false
    });
    print_public(
        output,
        &result,
        if args.approve {
            "Approved the current immutable proposal set. Runtime safety gates still apply."
        } else {
            "Tracking review loaded. Raw evidence remains local."
        },
    )
}

pub(crate) fn list_sources(
    data_dir: &Path,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let config = load_tracking_config(data_dir)?.unwrap_or_default();
    let result = serde_json::json!({
        "schemaVersion": TRACKING_MACHINE_CONTRACT_VERSION,
        "supported": [
            {"kind": "git", "description": "local Git commit metadata"},
            {"kind": "calendar", "description": "local RFC 5545 ICS events"},
            {"kind": "claude-code", "description": "local session lifecycle metadata"}
        ],
        "configured": source_statuses(&config),
        "effects": {
            "localSourceDataRead": true,
            "configurationPersisted": false
        },
        "networkAccess": false,
        "liveMutationAllowed": false
    });
    print_public(
        output,
        &result,
        "Listed local evidence sources without loading Drag credentials.",
    )
}

pub(crate) fn configure_sources(
    data_dir: &Path,
    args: PublicSourceConfigurationArgs,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let mut config = require_config(data_dir)?;
    let update_git = args.clear_repos || !args.repos.is_empty();
    let update_calendars = args.clear_ics || !args.ics_files.is_empty();
    let update_claude = args.claude_code || args.no_claude_code;
    if !update_git && !update_calendars && !update_claude {
        return Err(CompanionError::Proposal(
            "select a source update; pass --repo, --ics, --claude-code, or a clear option"
                .to_owned(),
        ));
    }

    let mut selected = configured_sources(args.repos, args.ics_files)?;
    if args.claude_code {
        selected.push(claude_code_source()?);
    }
    validate_source_configuration(&selected)?;
    config.sources.retain(|source| {
        !(update_git && source.kind == TrackingSourceKind::Git
            || update_calendars && source.kind == TrackingSourceKind::Calendar
            || update_claude && source.kind == TrackingSourceKind::ClaudeCode)
    });
    config.sources.extend(selected);
    if config.sources.len() > SOURCE_TEST_SOURCE_LIMIT {
        return Err(CompanionError::Proposal(format!(
            "source configuration is limited to {SOURCE_TEST_SOURCE_LIMIT} entries"
        )));
    }
    save_tracking_config(data_dir, &config)?;
    let result = serde_json::json!({
        "schemaVersion": TRACKING_MACHINE_CONTRACT_VERSION,
        "status": "configured",
        "sources": source_statuses(&config),
        "effects": {
            "localSourceDataRead": true,
            "configurationPersisted": true
        },
        "networkAccess": false,
        "liveMutationAllowed": false
    });
    print_public(
        output,
        &result,
        "Updated configured local evidence sources.",
    )
}

pub(crate) fn test_sources(
    data_dir: &Path,
    args: PublicDateArgs,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let config = require_config(data_dir)?;
    let date = select_public_date(args.when.as_deref(), &config.schedule.timezone)?;
    let sources = tested_source_statuses(&config, date);
    let result = serde_json::json!({
        "schemaVersion": TRACKING_MACHINE_CONTRACT_VERSION,
        "status": "tested",
        "selectedDate": date,
        "sources": sources,
        "redacted": true,
        "redaction": {
            "applied": true,
            "rawEvidenceIncluded": false,
            "localPathsIncluded": false
        },
        "bounds": {
            "sourceLimit": SOURCE_TEST_SOURCE_LIMIT,
            "observationLimitPerSource": SOURCE_TEST_OBSERVATION_LIMIT,
            "configuredSources": config.sources.len(),
            "testedSources": sources.len(),
            "truncated": config.sources.len() > sources.len()
        },
        "worklogsGenerated": 0,
        "effects": {
            "localSourceDataRead": true,
            "configurationPersisted": false,
            "evidencePersisted": false,
            "worklogsGenerated": 0
        },
        "networkAccess": false,
        "liveMutationAllowed": false
    });
    print_public(
        output,
        &result,
        "Tested configured sources without generating worklogs.",
    )
}

pub(crate) fn show_schedule(
    data_dir: &Path,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let config = require_config(data_dir)?;
    validate_schedule(&config.schedule)?;
    let status = scheduler_status(data_dir)?;
    let result = serde_json::json!({
        "schedule": config.schedule,
        "active": config.active,
        "nextRun": next_run_description(&config),
        "health": {
            "installed": scheduler_files_installed(&status["state"]),
            "healthy": scheduler_files_healthy(&status["state"]),
            "killSwitchActive": status["killSwitchActive"]
        },
        "networkAccess": false,
        "liveMutationAllowed": false
    });
    print_public(
        output,
        &result,
        "Displayed the automatic tracking schedule.",
    )
}

pub(crate) fn update_schedule(
    data_dir: &Path,
    drag_bin: &Path,
    args: PublicScheduleUpdateArgs,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let mut config = require_config(data_dir)?;
    let timezone = args
        .schedule_timezone
        .unwrap_or_else(|| config.schedule.timezone.clone());
    validate_time_and_timezone(&args.at, &timezone)?;
    config.schedule.at = args.at.clone();
    config.schedule.timezone = timezone.clone();
    let scheduler = if let Some(target_dir) = &config.scheduler_target {
        Some(install_scheduler_files(
            data_dir,
            drag_bin,
            &SchedulerInstallArgs {
                platform: default_scheduler_platform().to_owned(),
                target_dir: target_dir.clone(),
                at: args.at,
                timezone,
            },
        )?)
    } else {
        None
    };
    save_tracking_config(data_dir, &config)?;
    let status = scheduler_status(data_dir)?;
    let result = serde_json::json!({
        "status": "updated",
        "schedule": config.schedule,
        "active": config.active,
        "nextRun": next_run_description(&config),
        "health": {
            "installed": scheduler_files_installed(&status["state"]),
            "healthy": scheduler_files_healthy(&status["state"]),
            "killSwitchActive": status["killSwitchActive"]
        },
        "scheduler": scheduler,
        "networkAccess": false,
        "liveMutationAllowed": false
    });
    print_public(output, &result, "Updated the automatic tracking schedule.")
}

pub(crate) fn set_tracking_active(
    data_dir: &Path,
    active: bool,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let mut config = require_config(data_dir)?;
    if active {
        validate_tracking_configuration(&config)?;
        if config.scheduler_target.is_none() {
            return Err(CompanionError::Proposal(
                "tracking cannot resume until owned scheduler files are installed".to_owned(),
            ));
        }
        let status = status_payload(data_dir)?;
        if status["scheduler"]["installed"] != Value::Bool(true)
            || status["scheduler"]["healthy"] != Value::Bool(true)
        {
            return Err(CompanionError::Proposal(
                "tracking cannot resume until owned scheduler files are installed and healthy"
                    .to_owned(),
            ));
        }
    }
    config.active = active;
    let _ = set_scheduler_enabled_state(data_dir, active)?;
    save_tracking_config(data_dir, &config)?;
    let result = serde_json::json!({
        "status": if active { "active" } else { "paused" },
        "historyPreserved": true,
        "networkAccess": false,
        "liveMutationAllowed": false
    });
    print_public(
        output,
        &result,
        if active {
            "Automatic tracking resumed."
        } else {
            "Automatic tracking paused; history and recovery state were preserved."
        },
    )
}

pub(crate) fn uninstall_tracking(
    data_dir: &Path,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let mut config = load_tracking_config(data_dir)?.ok_or_else(|| {
        CompanionError::Proposal(
            "automatic tracking is not configured; run tracking setup first".to_owned(),
        )
    })?;
    let scheduler = if let Some(target_dir) = &config.scheduler_target {
        Some(uninstall_scheduler_files(
            data_dir,
            &SchedulerInstallArgs {
                platform: default_scheduler_platform().to_owned(),
                target_dir: target_dir.clone(),
                at: config.schedule.at.clone(),
                timezone: config.schedule.timezone.clone(),
            },
        )?)
    } else {
        None
    };
    if config.hooks_installed {
        remove_claude_hooks(&default_claude_settings_path())?;
    }
    config.installed = false;
    config.active = false;
    config.hooks_installed = false;
    config.scheduler_target = None;
    config.submission.automatic_submission_authorized = false;
    save_tracking_config(data_dir, &config)?;
    let result = serde_json::json!({
        "status": "uninstalled",
        "scheduler": scheduler,
        "hooksRemoved": true,
        "historyPreserved": true,
        "dataDirectory": data_dir,
        "networkAccess": false,
        "liveMutationAllowed": false
    });
    print_public(
        output,
        &result,
        "Removed tracking-owned scheduler and hook files. History and recovery state were preserved.",
    )
}

fn require_config(data_dir: &Path) -> Result<TrackingConfig, CompanionError> {
    load_tracking_config(data_dir)?
        .filter(|config| config.installed)
        .ok_or_else(|| {
            CompanionError::Proposal(
                "automatic tracking is not configured; run tracking setup first".to_owned(),
            )
        })
}

fn validate_tracking_configuration(config: &TrackingConfig) -> Result<(), CompanionError> {
    validate_schedule(&config.schedule)?;
    validate_source_configuration(&config.sources)
}

fn validate_schedule(schedule: &TrackingSchedule) -> Result<(), CompanionError> {
    validate_time_and_timezone(&schedule.at, &schedule.timezone)?;
    if !schedule.weekdays {
        return Err(CompanionError::Proposal(
            "tracking schedule must run on weekdays".to_owned(),
        ));
    }
    Ok(())
}

fn select_public_date(raw: Option<&str>, timezone: &str) -> Result<NaiveDate, CompanionError> {
    let today = if timezone == "local" {
        chrono::Local::now().date_naive()
    } else {
        let timezone = timezone.parse::<Tz>().map_err(|_| {
            CompanionError::Proposal("invalid configured tracking timezone".to_owned())
        })?;
        Utc::now().with_timezone(&timezone).date_naive()
    };
    let Some(raw) = raw else {
        return Ok(today);
    };
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Ok(date);
    }
    match raw.to_ascii_lowercase().as_str() {
        "t" | "today" => Ok(today),
        "y" | "yesterday" => today
            .pred_opt()
            .ok_or_else(|| CompanionError::Proposal("date is out of range".to_owned())),
        value => {
            let offset = value
                .strip_prefix("today")
                .or_else(|| value.strip_prefix('t'))
                .and_then(|offset| offset.parse::<i64>().ok())
                .ok_or_else(|| {
                    CompanionError::Proposal(
                        "date must use YYYY-MM-DD, y, yesterday, t±N, or today±N".to_owned(),
                    )
                })?;
            today
                .checked_add_signed(Duration::days(offset))
                .ok_or_else(|| CompanionError::Proposal("date is out of range".to_owned()))
        }
    }
}

fn effective_mutation_permission(
    data_dir: &Path,
    config: &TrackingConfig,
) -> Result<bool, CompanionError> {
    Ok(config.submission.mode == SubmissionMode::Automatic
        && config.submission.automatic_submission_authorized
        && live_rollout_enabled()
        && persisted_live_mutation_allowed(data_dir)?
        && !scheduler_kill_switch_path(data_dir).exists()
        && !environment_enabled("DRAG_TRACKING_KILL_SWITCH", "DRAG_COMPANION_KILL_SWITCH")?)
}

fn next_run_description(config: &TrackingConfig) -> Value {
    if config.active {
        Value::String(format!(
            "next weekday at {} {}",
            config.schedule.at, config.schedule.timezone
        ))
    } else {
        Value::Null
    }
}

fn migration_status(data_dir: &Path) -> Result<Value, CompanionError> {
    let path = data_dir.join("migration.json");
    if !path.exists() {
        return Ok(serde_json::json!({
            "status": "notRequired",
            "recoveryAction": "no migration recovery action required"
        }));
    }
    let body = fs::read_to_string(&path).map_err(|source| CompanionError::Read {
        path: path.clone(),
        source,
    })?;
    let record: Value = serde_json::from_str(&body).map_err(|error| {
        CompanionError::Proposal(format!(
            "invalid migration record {}: {error}",
            path.display()
        ))
    })?;
    let recovery = record["recoveryAction"].as_str().unwrap_or(
        "pause tracking, move the directory back to .drag-companion, and reinstall the previous release",
    );
    Ok(serde_json::json!({
        "status": record["status"],
        "legacyStatePreserved": true,
        "recoveryAction": recovery
    }))
}

fn print_public(
    output: Option<TrackingOutputMode>,
    value: &Value,
    human: &str,
) -> Result<(), CompanionError> {
    match output {
        Some(TrackingOutputMode::Human) => println_safe_markdown(human),
        Some(TrackingOutputMode::Json) => {
            print_json(&serde_json::json!({"ok": true, "data": value}))
        }
        None => print_json(value),
    }
}

struct ProposalCounts {
    proposals: u64,
    accepted: u64,
    rejected: u64,
}

fn proposal_counts(data_dir: &Path, date: NaiveDate) -> Result<ProposalCounts, CompanionError> {
    let path = store_path(data_dir);
    if !path.exists() {
        return Ok(ProposalCounts {
            proposals: 0,
            accepted: 0,
            rejected: 0,
        });
    }
    let mut conn = Connection::open(path)?;
    migrate(&mut conn)?;
    let proposals = conn.query_row(
        "SELECT COUNT(*) FROM proposals p JOIN daily_bundles b ON b.id = p.bundle_id WHERE b.explicit_date = ?1",
        [date.to_string()],
        |row| row.get::<_, u64>(0),
    )?;
    let accepted = conn.query_row(
        "SELECT COUNT(*) FROM policy_decisions d JOIN proposals p ON p.id = d.proposal_id JOIN daily_bundles b ON b.id = p.bundle_id WHERE b.explicit_date = ?1 AND d.decision = 'approved'",
        [date.to_string()],
        |row| row.get::<_, u64>(0),
    )?;
    let rejected = conn.query_row(
        "SELECT COUNT(*) FROM policy_decisions d JOIN proposals p ON p.id = d.proposal_id JOIN daily_bundles b ON b.id = p.bundle_id WHERE b.explicit_date = ?1 AND d.decision != 'approved'",
        [date.to_string()],
        |row| row.get::<_, u64>(0),
    )?;
    Ok(ProposalCounts {
        proposals,
        accepted,
        rejected,
    })
}

fn proposal_set_digest(data_dir: &Path, date: NaiveDate) -> Result<String, CompanionError> {
    let proposals = review_proposals(data_dir, date)?;
    let bytes = serde_json::to_vec(&proposals).map_err(CompanionError::Serialize)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn review_proposals(
    data_dir: &Path,
    date: NaiveDate,
) -> Result<Vec<(String, Value)>, CompanionError> {
    if store_path(data_dir).exists() {
        proposal_payloads(data_dir, date, None)
    } else {
        Ok(Vec::new())
    }
}

fn approval_path(data_dir: &Path, date: NaiveDate) -> PathBuf {
    data_dir.join("approvals").join(format!("{date}.json"))
}

fn persist_approval(data_dir: &Path, date: NaiveDate, digest: &str) -> Result<(), CompanionError> {
    let path = approval_path(data_dir, date);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CompanionError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let body = serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": 1,
        "selectedDate": date,
        "digest": digest,
        "approvedAt": now_string()
    }))
    .map_err(CompanionError::Serialize)?;
    atomic_write(&path, &body)
}

fn load_approval(data_dir: &Path, date: NaiveDate) -> Result<Option<Value>, CompanionError> {
    let path = approval_path(data_dir, date);
    if !path.exists() {
        return Ok(None);
    }
    let body = fs::read_to_string(&path).map_err(|source| CompanionError::Read { path, source })?;
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|error| CompanionError::Proposal(format!("approval schema: {error}")))
}

fn approval_matches(data_dir: &Path, date: NaiveDate) -> Result<bool, CompanionError> {
    let digest = proposal_set_digest(data_dir, date)?;
    Ok(load_approval(data_dir, date)?
        .is_some_and(|approval| approval["digest"].as_str() == Some(&digest)))
}

fn review_decisions(data_dir: &Path, date: NaiveDate) -> Result<Vec<Value>, CompanionError> {
    let path = store_path(data_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut conn = Connection::open(path)?;
    migrate(&mut conn)?;
    let mut stmt = conn.prepare(
        "SELECT d.proposal_id, d.decision, d.reason_codes_json, d.evidence_trace_json FROM policy_decisions d JOIN proposals p ON p.id = d.proposal_id JOIN daily_bundles b ON b.id = p.bundle_id WHERE b.explicit_date = ?1 ORDER BY d.proposal_id",
    )?;
    let rows = stmt.query_map([date.to_string()], |row| {
        let reasons: String = row.get(2)?;
        let evidence: String = row.get(3)?;
        Ok(serde_json::json!({
            "proposalId": row.get::<_, String>(0)?,
            "decision": row.get::<_, String>(1)?,
            "reasonCodes": serde_json::from_str::<Value>(&reasons)
                .unwrap_or_else(|_| Value::Array(Vec::new())),
            "evidenceReferences": serde_json::from_str::<Value>(&evidence)
                .unwrap_or_else(|_| Value::Array(Vec::new()))
        }))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}
