use crate::*;

pub(crate) fn setup_tracking(
    data_dir: &Path,
    drag_bin: &Path,
    args: PublicSetupArgs,
    output: Option<TrackingOutputMode>,
) -> Result<(), CompanionError> {
    let previous = load_tracking_config(data_dir)?;
    let mut config = previous.clone().unwrap_or_default();
    let mode = args.mode.unwrap_or(config.submission.mode);
    let at = args.at.unwrap_or_else(|| config.schedule.at.clone());
    let timezone = args
        .schedule_timezone
        .unwrap_or_else(|| config.schedule.timezone.clone());
    validate_time_and_timezone(&at, &timezone)?;
    if args.authorize_automatic && mode != SubmissionMode::Automatic {
        return Err(CompanionError::Proposal(
            "--authorize-automatic requires --mode automatic".to_owned(),
        ));
    }
    if mode == SubmissionMode::Automatic
        && !args.authorize_automatic
        && !(config.submission.mode == SubmissionMode::Automatic
            && config.submission.automatic_submission_authorized)
    {
        return Err(CompanionError::Proposal(
            "automatic mode requires separate --authorize-automatic consent".to_owned(),
        ));
    }
    if args.install_scheduler && args.scheduler_target.is_none() {
        return Err(CompanionError::Proposal(
            "--install-scheduler requires --scheduler-target DIR".to_owned(),
        ));
    }

    let update_repos = !args.repos.is_empty();
    let update_calendars = !args.ics_files.is_empty();
    let selected = configured_sources(args.repos, args.ics_files)?;
    if update_repos {
        config
            .sources
            .retain(|source| source.kind != TrackingSourceKind::Git);
    }
    if update_calendars {
        config
            .sources
            .retain(|source| source.kind != TrackingSourceKind::Calendar);
    }
    config.sources.extend(selected);
    validate_source_configuration(&config.sources)?;
    config.schedule = TrackingSchedule {
        weekdays: true,
        at: at.clone(),
        timezone: timezone.clone(),
    };
    config.submission = TrackingSubmission {
        mode,
        automatic_submission_authorized: if mode == SubmissionMode::Automatic {
            args.authorize_automatic || config.submission.automatic_submission_authorized
        } else {
            false
        },
    };
    if let Some(path) = args.provider_fixture {
        let path = stable_source_path(path)?;
        validate_provider_fixture(&path)?;
        config.provider_fixture = Some(path);
    }

    // Persist recoverable ownership before touching scheduler or hook files. An
    // interrupted setup remains explicitly uninstalled and can be safely rerun.
    config.installed = false;
    config.active = false;
    if let Some(target) = args.scheduler_target.as_ref() {
        config.scheduler_target = Some(target.clone());
    }
    save_tracking_config(data_dir, &config)?;

    let scheduler = if let Some(target_dir) = args.scheduler_target {
        let install = SchedulerInstallArgs {
            platform: default_scheduler_platform().to_owned(),
            target_dir: target_dir.clone(),
            at,
            timezone,
        };
        Some(install_scheduler_files(data_dir, drag_bin, &install)?)
    } else {
        None
    };
    if args.install_hooks {
        install_claude_hooks(&default_claude_settings_path())?;
        config.hooks_installed = true;
        save_tracking_config(data_dir, &config)?;
    }
    if config.hooks_installed {
        config
            .sources
            .retain(|source| source.kind != TrackingSourceKind::ClaudeCode);
        config.sources.push(claude_code_source()?);
    }
    validate_source_configuration(&config.sources)?;
    config.installed = true;
    config.active = if let Some(scheduler) = &scheduler {
        scheduler["active"].as_bool().unwrap_or(false)
    } else if config.scheduler_target.is_some() {
        scheduler_status(data_dir)?["enabled"]
            .as_bool()
            .unwrap_or(false)
    } else {
        false
    };
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
        "nextSafeAction": if config.active {
            "inspect tracking status before the first scheduled run"
        } else if scheduler.is_some() {
            "reinstall scheduler files in the platform user scheduler directory, then resume tracking"
        } else {
            "install the tracking scheduler, then resume tracking"
        }
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
    if let Some(mut config) = load_tracking_config(data_dir)? {
        status["configuration"]["configured"] = Value::Bool(config.installed);
        status["configuration"]["migration"] = migration_status(data_dir)?;
        config.active = status["scheduler"]["active"].as_bool().unwrap_or(false);
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
            if !config.installed {
                "rerun tracking setup to complete or recover the interrupted installation"
            } else if status["scheduler"]["healthy"] == Value::Bool(false) {
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
    let result = run_tracking_for_date(data_dir, drag_bin, &config, date)?;
    let status = result["status"].as_str().unwrap_or("failed");
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
    config: &TrackingConfig,
    date: NaiveDate,
) -> Result<Value, CompanionError> {
    let mut progress = TrackingRunProgress::new(data_dir, config, date);
    let outcome = (|| -> Result<(), CompanionError> {
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
        progress.observations = collected.git.commits.len() + collected.calendar.events.len();
        let collection_failures = collected.failures.len() + collected.calendar.failures.len();
        if collection_failures > 0 {
            progress.collection_failures = collected
                .failures
                .iter()
                .chain(collected.calendar.failures.iter())
                .map(|failure| {
                    serde_json::json!({
                        "source": minimized_reference(&failure.repository),
                        "error": redact(&failure.error)
                    })
                })
                .collect();
            progress.warnings.push(format!(
                "{collection_failures} configured source collection(s) failed; inspect tracking sources test"
            ));
            return Ok(());
        }
        progress.imported_evidence = if journal_path(data_dir).exists() {
            import_journal(data_dir)?
        } else {
            let mut conn = Connection::open(store_path(data_dir))?;
            migrate(&mut conn)?;
            0
        };
        let bundle = build_bundle(data_dir, date)?;
        progress.bundle_items = bundle.evidence.len();
        progress.bundle_contradictions = bundle.contradictions.len();
        if proposal_counts(data_dir, date)?.proposals == 0 {
            if let Some(fixture) = &config.provider_fixture {
                propose_from_fixture(data_dir, date, fixture)?;
            } else if !bundle.evidence.is_empty() {
                progress.warnings.push(
                    "proposal provider is not configured; configure an offline provider fixture with tracking setup --provider-fixture FILE".to_owned(),
                );
            }
        }
        let before_audit = proposal_counts(data_dir, date)?;
        let approved_review =
            config.submission.mode == SubmissionMode::Review && approval_matches(data_dir, date)?;
        if before_audit.proposals > 0 {
            progress.network_access = true;
            let audit = audit_drag_day(
                data_dir,
                drag_bin,
                date,
                config.submission.mode == SubmissionMode::Automatic || approved_review,
            )?;
            progress.existing_worklogs = audit.existing_worklogs.len();
        } else if config.submission.mode != SubmissionMode::Automatic {
            progress.network_access = true;
            let read = read_drag_day(drag_bin, date)?;
            progress.existing_worklogs = read.worklogs.len();
        }
        progress.counts = proposal_counts(data_dir, date)?;

        let execution_authorized = match config.submission.mode {
            SubmissionMode::Automatic => config.submission.automatic_submission_authorized,
            SubmissionMode::Review => approved_review,
            SubmissionMode::Draft => false,
        };
        let run = coordinated_run_with_submission(
            data_dir,
            drag_bin,
            date,
            progress.resumed,
            execution_authorized,
        )?;
        progress.phases = serde_json::to_value(run.phases).map_err(CompanionError::Serialize)?;
        if run.status != "completed" {
            progress.terminal_status = Some(run.status);
        }

        if execution_authorized {
            progress.mutation_attempted = true;
            let execution = execute_drag_worklogs(data_dir, drag_bin, date, true)?;
            if execution.status != "executed" {
                progress.terminal_status = Some(execution.status);
                progress.warnings.push(match execution.status {
                    "gated" => "submission was blocked by runtime safety gates".to_owned(),
                    "uncertain" => {
                        "submission outcome is uncertain; reconcile before retrying".to_owned()
                    }
                    status => format!("submission ended with status {status}"),
                });
            }
            progress.submitted = execution.submitted;
            progress.skipped = execution.skipped;
            progress.network_access |= execution.network_access;
            progress.live_mutation_allowed = execution.live_mutation_allowed;
        }
        progress.retention = Some(enforce_retention(data_dir, RetentionTrigger::Lifecycle)?);
        Ok(())
    })();

    match outcome {
        Ok(()) => {
            let status = if let Some(status) = progress.terminal_status {
                status
            } else if !progress.collection_failures.is_empty() {
                "source-failed"
            } else if progress.warnings.is_empty() {
                "completed"
            } else {
                "partial"
            };
            let result = progress.result(status, None);
            persist_tracking_run(data_dir, date, &result)?;
            Ok(result)
        }
        Err(error) => {
            progress.counts = proposal_counts(data_dir, date).unwrap_or_default();
            if let Ok((submitted, skipped, mutation_attempted)) =
                persisted_execution_progress(data_dir, date)
            {
                progress.submitted = submitted;
                progress.skipped = skipped;
                progress.mutation_attempted |= mutation_attempted;
                progress.live_mutation_allowed |= mutation_attempted;
                progress.network_access |= mutation_attempted;
            }
            let failure = serde_json::json!({
                "kind": tracking_failure_kind(&error),
                "message": error.to_string()
            });
            let result = progress.result("failed", Some(failure));
            persist_tracking_run(data_dir, date, &result)?;
            Err(CompanionError::TrackingRun {
                message: error.to_string(),
                details: Box::new(result),
            })
        }
    }
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
    let result = serde_json::json!({
        "schedule": config.schedule,
        "active": config.active,
        "nextRun": next_run_description(&config),
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
    let result = serde_json::json!({
        "status": "updated",
        "schedule": config.schedule,
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
    if active && config.scheduler_target.is_none() {
        return Err(CompanionError::Proposal(
            "tracking cannot resume until owned scheduler files are installed".to_owned(),
        ));
    }
    if active {
        validate_source_configuration(&config.sources)?;
    }
    let scheduler = if config.scheduler_target.is_some() {
        Some(set_scheduler_enabled_state(data_dir, active)?)
    } else {
        None
    };
    if active
        && scheduler
            .as_ref()
            .is_none_or(|value| value["status"] != "enabled")
    {
        return Err(CompanionError::Proposal(
            "host scheduler activation was not completed; reinstall scheduler files in the platform user scheduler directory before resuming".to_owned(),
        ));
    }
    config.active = scheduler
        .as_ref()
        .is_some_and(|value| value["status"] == Value::String("enabled".to_owned()));
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
            "automatic tracking is not configured; there are no tracked installation effects to remove"
                .to_owned(),
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
    config.submission.mode = SubmissionMode::Draft;
    config.submission.automatic_submission_authorized = false;
    config.hooks_installed = false;
    config.scheduler_target = None;
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
    let config = load_tracking_config(data_dir)?.ok_or_else(|| {
        CompanionError::Proposal(
            "automatic tracking is not configured; run tracking setup first".to_owned(),
        )
    })?;
    if !config.installed {
        return Err(CompanionError::Proposal(
            "automatic tracking is uninstalled or setup is incomplete; run tracking setup first"
                .to_owned(),
        ));
    }
    Ok(config)
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

#[derive(Default)]
struct ProposalCounts {
    proposals: u64,
    accepted: u64,
    rejected: u64,
}

struct TrackingRunProgress {
    date: NaiveDate,
    resumed: bool,
    source_health: Vec<Value>,
    collection_failures: Vec<Value>,
    terminal_status: Option<&'static str>,
    observations: usize,
    imported_evidence: usize,
    bundle_items: usize,
    bundle_contradictions: usize,
    existing_worklogs: usize,
    counts: ProposalCounts,
    submitted: usize,
    skipped: usize,
    network_access: bool,
    mutation_attempted: bool,
    live_mutation_allowed: bool,
    warnings: Vec<String>,
    phases: Value,
    retention: Option<Value>,
}

impl TrackingRunProgress {
    fn new(data_dir: &Path, config: &TrackingConfig, date: NaiveDate) -> Self {
        Self {
            date,
            resumed: run_path(data_dir, date).exists(),
            source_health: source_statuses(config),
            collection_failures: Vec::new(),
            terminal_status: None,
            observations: 0,
            imported_evidence: 0,
            bundle_items: 0,
            bundle_contradictions: 0,
            existing_worklogs: 0,
            counts: ProposalCounts::default(),
            submitted: 0,
            skipped: 0,
            network_access: false,
            mutation_attempted: false,
            live_mutation_allowed: false,
            warnings: Vec::new(),
            phases: Value::Array(Vec::new()),
            retention: None,
        }
    }

    fn result(&self, status: &str, failure: Option<Value>) -> Value {
        serde_json::json!({
            "schemaVersion": TRACKING_MACHINE_CONTRACT_VERSION,
            "selectedDate": self.date,
            "status": status,
            "resumed": self.resumed,
            "resumable": true,
            "sourceHealth": self.source_health,
            "collectionFailures": self.collection_failures,
            "observations": self.observations,
            "importedEvidence": self.imported_evidence,
            "evidenceBundle": {
                "items": self.bundle_items,
                "contradictions": self.bundle_contradictions
            },
            "existingWorklogs": self.existing_worklogs,
            "proposals": self.counts.proposals,
            "accepted": self.counts.accepted,
            "rejected": self.counts.rejected,
            "submitted": self.submitted,
            "skipped": self.skipped,
            "networkAccess": self.network_access,
            "liveMutationAllowed": self.live_mutation_allowed,
            "effects": {
                "networkAccess": self.network_access,
                "mutationAttempted": self.mutation_attempted,
                "liveMutationAllowed": self.live_mutation_allowed,
                "retentionEnforced": self.retention.is_some(),
                "reportPersisted": true
            },
            "warnings": self.warnings,
            "failure": failure,
            "nextSafeAction": next_safe_action(status),
            "phases": self.phases,
            "retention": self.retention
        })
    }
}

fn persist_tracking_run(
    data_dir: &Path,
    date: NaiveDate,
    result: &Value,
) -> Result<(), CompanionError> {
    let path = run_path(data_dir, date);
    let parent = path.parent().unwrap_or(data_dir);
    fs::create_dir_all(parent).map_err(|source| CompanionError::CreateDir {
        path: parent.to_path_buf(),
        source,
    })?;
    let body = serde_json::to_vec_pretty(result).map_err(CompanionError::Serialize)?;
    atomic_write(&path, &body)
}

fn tracking_failure_kind(error: &CompanionError) -> &'static str {
    match error {
        CompanionError::DragReconcile { kind, .. } => match kind {
            ReconcileErrorKind::IncompleteRead => "incompleteRead",
            ReconcileErrorKind::SchemaIncompatibility => "schemaIncompatibility",
            ReconcileErrorKind::DefiniteFailure => "definiteFailure",
            ReconcileErrorKind::TransportAmbiguity => "transportAmbiguity",
        },
        CompanionError::RunOwned { .. } => "runOwned",
        CompanionError::InvalidJournal { .. } => "invalidEvidence",
        CompanionError::Proposal(_) => "invalidWorkflowState",
        _ => "localFailure",
    }
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
