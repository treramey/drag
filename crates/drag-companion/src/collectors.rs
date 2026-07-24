use crate::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollectResult {
    pub(crate) status: &'static str,
    pub(crate) mode: &'static str,
    pub(crate) adapter: &'static str,
    pub(crate) network_access: bool,
    pub(crate) git: GitCollectOutput,
    pub(crate) calendar: CalendarCollectOutput,
    pub(crate) failures: Vec<CollectFailure>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarCollectOutput {
    pub(crate) events: Vec<CalendarEvidence>,
    pub(crate) failures: Vec<CollectFailure>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarEvidence {
    pub(crate) uid: String,
    pub(crate) occurrence_date: NaiveDate,
    pub(crate) status: String,
    pub(crate) recurrence_id: Option<String>,
    pub(crate) last_modified: Option<String>,
    pub(crate) timezone: String,
    pub(crate) all_day: bool,
    pub(crate) interval_start: Option<String>,
    pub(crate) interval_end: Option<String>,
    pub(crate) summary: String,
    pub(crate) source_file: String,
    pub(crate) sequence: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GitCollectOutput {
    pub(crate) commits: Vec<GitCommitEvidence>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GitCommitEvidence {
    pub(crate) commit: String,
    pub(crate) author: GitIdentity,
    pub(crate) committer: GitIdentity,
    pub(crate) author_timestamp: String,
    pub(crate) committer_timestamp: String,
    pub(crate) repository: GitRepositoryIdentity,
    pub(crate) branch: String,
    pub(crate) ref_name: String,
    pub(crate) subject: String,
    pub(crate) issue_candidates: Vec<IssueCandidate>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitIdentity {
    pub(crate) name: String,
    pub(crate) email: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitRepositoryIdentity {
    pub(crate) path: String,
    pub(crate) worktree: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IssueCandidate {
    pub(crate) key: String,
    pub(crate) origin: &'static str,
    pub(crate) confidence: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct CollectFailure {
    pub(crate) repository: String,
    pub(crate) error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FakeObservation {
    pub(crate) source: &'static str,
    pub(crate) summary: &'static str,
}

pub(crate) fn install_claude_hooks(settings_path: &Path) -> Result<(), CompanionError> {
    let mut settings = read_settings(settings_path)?;
    if !settings.is_object() {
        return Err(CompanionError::InvalidClaudeHook(
            "settings must be a JSON object".to_owned(),
        ));
    }
    let Some(settings_object) = settings.as_object_mut() else {
        return Err(CompanionError::InvalidClaudeHook(
            "settings must be a JSON object".to_owned(),
        ));
    };
    let hooks = settings_object
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        return Err(CompanionError::InvalidClaudeHook(
            "hooks must be a JSON object".to_owned(),
        ));
    }
    let Some(hooks_object) = hooks.as_object_mut() else {
        return Err(CompanionError::InvalidClaudeHook(
            "hooks must be a JSON object".to_owned(),
        ));
    };
    for event in ["SessionStart", "SessionEnd"] {
        let list = hooks_object
            .entry(event)
            .or_insert_with(|| serde_json::json!([]));
        if !list.is_array() {
            return Err(CompanionError::InvalidClaudeHook(format!(
                "{event} hooks must be an array"
            )));
        }
        let Some(arr) = list.as_array_mut() else {
            return Err(CompanionError::InvalidClaudeHook(format!(
                "{event} hooks must be an array"
            )));
        };
        if !arr.iter().any(is_our_hook_entry) {
            arr.push(serde_json::json!({
                "matcher": "*",
                "hooks": [{ "type": "command", "command": CLAUDE_HOOK_COMMAND }]
            }));
        }
    }
    write_settings(settings_path, &settings)
}

pub(crate) fn remove_claude_hooks(settings_path: &Path) -> Result<(), CompanionError> {
    let mut settings = read_settings(settings_path)?;
    if let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) {
        for event in ["SessionStart", "SessionEnd"] {
            if let Some(entries) = hooks.get_mut(event).and_then(Value::as_array_mut) {
                for entry in entries.iter_mut() {
                    if let Some(commands) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                        commands.retain(|command| !is_our_command(command));
                    }
                }
                entries.retain(|entry| {
                    entry
                        .get("hooks")
                        .and_then(Value::as_array)
                        .is_none_or(|commands| !commands.is_empty())
                        || !is_our_hook_entry(entry)
                });
            }
        }
    }
    write_settings(settings_path, &settings)
}

pub(crate) fn collect_activity(
    data_dir: &Path,
    args: &CollectArgs,
) -> Result<CollectResult, CompanionError> {
    let mut commits = Vec::new();
    let mut failures = Vec::new();
    let mut calendar_events = Vec::new();
    let mut calendar_failures = Vec::new();

    for repo in &args.repos {
        match scan_git_repo(repo) {
            Ok(repo_commits) => {
                for commit in repo_commits {
                    append_journal_event(data_dir, &git_commit_event(&commit)?)?;
                    commits.push(commit);
                }
            }
            Err(error) => failures.push(CollectFailure {
                repository: repo.display().to_string(),
                error,
            }),
        }
    }

    if let Some(date) = args.date {
        for path in &args.ics_files {
            match scan_ics_file(path, date) {
                Ok(events) => {
                    for event in events {
                        append_journal_event(data_dir, &calendar_event(&event)?)?;
                        calendar_events.push(event);
                    }
                }
                Err(errors) => {
                    calendar_failures.extend(errors.into_iter().map(|error| CollectFailure {
                        repository: path.display().to_string(),
                        error,
                    }))
                }
            }
        }
    }

    Ok(CollectResult {
        status: "collected",
        mode: DEFAULT_MODE,
        adapter: if args.ics_files.is_empty() {
            "git-local"
        } else {
            "local"
        },
        network_access: false,
        git: GitCollectOutput { commits },
        calendar: CalendarCollectOutput {
            events: calendar_events,
            failures: calendar_failures,
        },
        failures,
    })
}

pub(crate) fn scan_ics_file(
    path: &Path,
    date: NaiveDate,
) -> Result<Vec<CalendarEvidence>, Vec<String>> {
    let body = fs::read_to_string(path).map_err(|error| vec![error.to_string()])?;
    let lines = unfold_ics_lines(&body);
    let mut events = Vec::new();
    let mut current = Vec::new();
    let mut in_event = false;
    let mut errors = Vec::new();
    for line in lines {
        match line.as_str() {
            "BEGIN:VEVENT" => {
                if in_event {
                    errors.push("nested VEVENT".to_owned());
                }
                in_event = true;
                current.clear();
            }
            "END:VEVENT" => {
                if in_event {
                    parse_ics_event(&current, path, date, &mut events, &mut errors);
                    in_event = false;
                    current.clear();
                }
            }
            _ if in_event => current.push(line),
            _ => {}
        }
    }
    if in_event {
        errors.push("unterminated VEVENT".to_owned());
    }
    if !errors.is_empty() {
        Err(errors)
    } else {
        Ok(events)
    }
}

pub(crate) fn unfold_ics_lines(body: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in body.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        if raw.starts_with(' ') || raw.starts_with('\t') {
            if let Some(last) = lines.last_mut() {
                last.push_str(raw.trim_start());
            }
        } else if !raw.is_empty() {
            lines.push(raw.to_owned());
        }
    }
    lines
}

pub(crate) fn parse_ics_event(
    lines: &[String],
    path: &Path,
    date: NaiveDate,
    out: &mut Vec<CalendarEvidence>,
    errors: &mut Vec<String>,
) {
    let mut uid = None;
    let mut dtstart = None;
    let mut dtend = None;
    let mut status = "CONFIRMED".to_owned();
    let mut last_modified = None;
    let mut summary = String::new();
    let mut rrule = None;
    let mut recurrence_id = None;
    let mut exdates = Vec::new();
    let mut sequence = 0;
    for line in lines {
        let Some((name_params, value)) = line.split_once(':') else {
            errors.push(format!("malformed property {line}"));
            continue;
        };
        let (name, params) = name_params.split_once(';').unwrap_or((name_params, ""));
        match name {
            "UID" => uid = Some(value.to_owned()),
            "DTSTART" => dtstart = Some((params.to_owned(), value.to_owned())),
            "DTEND" => dtend = Some((params.to_owned(), value.to_owned())),
            "STATUS" => status = value.to_owned(),
            "LAST-MODIFIED" => last_modified = normalize_ics_utc(value),
            "SUMMARY" => summary = value.to_owned(),
            "RRULE" => rrule = Some(value.to_owned()),
            "RECURRENCE-ID" => recurrence_id = Some(value.to_owned()),
            "EXDATE" => exdates.extend(value.split(',').map(ToOwned::to_owned)),
            "SEQUENCE" => sequence = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    if status == "CANCELLED" {
        return;
    }
    let Some(uid) = uid else {
        errors.push("missing UID".to_owned());
        return;
    };
    let Some((start_params, start_value)) = dtstart else {
        errors.push(format!("{uid}: missing DTSTART"));
        return;
    };
    let all_day = start_params.contains("VALUE=DATE");
    let timezone = if all_day {
        "all-day".to_owned()
    } else if start_value.ends_with('Z') {
        "UTC".to_owned()
    } else if let Some(tzid) = param_value(&start_params, "TZID") {
        tzid
    } else {
        errors.push("floating time requires explicit timezone".to_owned());
        return;
    };
    let duration = dtend
        .as_ref()
        .and_then(|(params, value)| {
            event_duration(
                &start_params,
                &start_value,
                params,
                value,
                all_day,
                &timezone,
            )
        })
        .unwrap_or_else(|| Duration::hours(1));
    let starts = occurrence_starts(
        &start_value,
        all_day,
        &timezone,
        rrule.as_deref(),
        &exdates,
        date,
        errors,
    );
    for (occurrence_date, start_utc) in starts {
        let (interval_start, interval_end) = if all_day {
            (None, None)
        } else {
            (
                Some(start_utc.to_rfc3339_opts(SecondsFormat::Secs, true)),
                Some((start_utc + duration).to_rfc3339_opts(SecondsFormat::Secs, true)),
            )
        };
        out.push(CalendarEvidence {
            uid: uid.clone(),
            occurrence_date,
            status: status.clone(),
            recurrence_id: recurrence_id
                .clone()
                .or_else(|| rrule.clone().map(|_| occurrence_date.to_string())),
            last_modified: last_modified.clone(),
            timezone: timezone.clone(),
            all_day,
            interval_start,
            interval_end,
            summary: summary.clone(),
            source_file: path.display().to_string(),
            sequence,
        });
    }
    out.sort_by(|a, b| {
        (&a.uid, a.sequence, &a.last_modified).cmp(&(&b.uid, b.sequence, &b.last_modified))
    });
}

pub(crate) fn param_value(params: &str, key: &str) -> Option<String> {
    params
        .split(';')
        .find_map(|part| part.strip_prefix(&format!("{key}=")).map(ToOwned::to_owned))
}

pub(crate) fn occurrence_starts(
    raw: &str,
    all_day: bool,
    timezone: &str,
    rrule: Option<&str>,
    exdates: &[String],
    date: NaiveDate,
    errors: &mut Vec<String>,
) -> Vec<(NaiveDate, DateTime<Utc>)> {
    let Some(first) = parse_ics_start(raw, all_day, timezone, errors) else {
        return Vec::new();
    };
    let explicit_count = rrule.and_then(|rule| {
        rule.split(';')
            .find_map(|part| part.strip_prefix("COUNT=")?.parse::<usize>().ok())
    });
    let daily = rrule.is_some_and(|rule| rule.contains("FREQ=DAILY"));
    let count = explicit_count
        .unwrap_or_else(|| {
            if daily {
                let first_local_date = if all_day {
                    first.date_naive()
                } else {
                    first
                        .with_timezone(&timezone.parse::<Tz>().unwrap_or(chrono_tz::UTC))
                        .date_naive()
                };
                date.signed_duration_since(first_local_date)
                    .num_days()
                    .max(0) as usize
                    + 1
            } else {
                1
            }
        })
        .min(400);
    let local_base = (!all_day && daily)
        .then(|| NaiveDateTime::parse_from_str(raw.trim_end_matches('Z'), "%Y%m%dT%H%M%S").ok())
        .flatten();
    let recurrence_tz = timezone.parse::<Tz>().unwrap_or(chrono_tz::UTC);
    let mut starts = Vec::new();
    for index in 0..count {
        let candidate = if let Some(base) = local_base {
            let local = base + Duration::days(index as i64);
            match recurrence_tz.from_local_datetime(&local) {
                LocalResult::Single(value) => value.with_timezone(&Utc),
                LocalResult::Ambiguous(early, _) => early.with_timezone(&Utc),
                LocalResult::None => {
                    errors.push(format!(
                        "nonexistent local recurrence {local} in {timezone}"
                    ));
                    continue;
                }
            }
        } else if daily {
            first + Duration::days(index as i64)
        } else {
            first
        };
        if exdates.iter().any(|exdate| {
            parse_ics_start(exdate, all_day, timezone, &mut Vec::new()) == Some(candidate)
        }) {
            continue;
        }
        let local_date = if all_day {
            candidate.date_naive()
        } else {
            candidate
                .with_timezone(&timezone.parse::<Tz>().unwrap_or(chrono_tz::UTC))
                .date_naive()
        };
        if local_date == date {
            starts.push((local_date, candidate));
        }
        if !daily {
            break;
        }
    }
    starts
}

pub(crate) fn parse_ics_start(
    raw: &str,
    all_day: bool,
    timezone: &str,
    errors: &mut Vec<String>,
) -> Option<DateTime<Utc>> {
    if all_day {
        return NaiveDate::parse_from_str(raw, "%Y%m%d")
            .ok()?
            .and_hms_opt(0, 0, 0)
            .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
    }
    if let Some(raw) = raw.strip_suffix('Z') {
        return NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S")
            .ok()
            .map(|value| DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
    }
    let tz: Tz = match timezone.parse() {
        Ok(tz) => tz,
        Err(_) => {
            errors.push(format!("unknown timezone {timezone}"));
            return None;
        }
    };
    let naive = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S").ok()?;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
        LocalResult::Ambiguous(early, _) => Some(early.with_timezone(&Utc)),
        LocalResult::None => {
            errors.push(format!("nonexistent local time {naive} in {timezone}"));
            None
        }
    }
}

pub(crate) fn event_duration(
    _start_params: &str,
    start: &str,
    end_params: &str,
    end: &str,
    all_day: bool,
    timezone: &str,
) -> Option<Duration> {
    if !all_day {
        let s = NaiveDateTime::parse_from_str(start.trim_end_matches('Z'), "%Y%m%dT%H%M%S").ok()?;
        let e = NaiveDateTime::parse_from_str(end.trim_end_matches('Z'), "%Y%m%dT%H%M%S").ok()?;
        return Some(e - s);
    }
    let mut errors = Vec::new();
    let s = parse_ics_start(start, all_day, timezone, &mut errors)?;
    let end_tz = param_value(end_params, "TZID").unwrap_or_else(|| timezone.to_owned());
    let e = parse_ics_start(
        end,
        all_day || end_params.contains("VALUE=DATE"),
        &end_tz,
        &mut errors,
    )?;
    Some(e - s)
}

pub(crate) fn normalize_ics_utc(raw: &str) -> Option<String> {
    if let Some(stripped) = raw.strip_suffix('Z') {
        NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S")
            .ok()
            .map(|dt| {
                DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc)
                    .to_rfc3339_opts(SecondsFormat::Secs, true)
            })
    } else {
        None
    }
}

pub(crate) fn calendar_event(calendar: &CalendarEvidence) -> Result<JournalEvent, CompanionError> {
    let occurrence = calendar
        .recurrence_id
        .as_deref()
        .unwrap_or(&calendar.occurrence_date.to_string())
        .replace(':', "");
    let event_id = format!(
        "evidence.ics.{}.{}.{}",
        calendar.uid.replace(['/', '#', ' '], "_"),
        occurrence,
        calendar.sequence
    );
    let supersedes = (calendar.sequence > 1).then(|| {
        format!(
            "evidence.ics.{}.{}.{}",
            calendar.uid.replace(['/', '#', ' '], "_"),
            occurrence,
            calendar.sequence - 1
        )
    });
    let mut event = JournalEvent {
        schema_version: JOURNAL_SCHEMA_VERSION,
        event_id,
        event_type: "calendar.ics.event".to_owned(),
        observed_at: calendar
            .last_modified
            .clone()
            .or_else(|| calendar.interval_start.clone())
            .unwrap_or_else(now_string),
        source: SourceProvenance {
            kind: "calendar".to_owned(),
            adapter: "ics-local".to_owned(),
            reference: format!("{}#{}", calendar.uid, calendar.occurrence_date),
        },
        collector: CollectorProvenance {
            name: "ics-local".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        timestamp_semantics: TimestampSemantics {
            observed_at_source: "ics-dtstart".to_owned(),
            timezone: calendar.timezone.clone(),
            explicit_date: calendar.occurrence_date,
        },
        privacy: PrivacyState {
            classification: "local-calendar-metadata".to_owned(),
            redacted: false,
        },
        retention: RetentionMetadata {
            policy: "retain-until-user-purge".to_owned(),
            retain_until: None,
        },
        supersedes,
        payload: serde_json::to_value(calendar).map_err(CompanionError::Serialize)?,
        integrity_hash: String::new(),
    };
    event.payload["intervalStart"] = calendar.interval_start.clone().into();
    event.payload["intervalEnd"] = calendar.interval_end.clone().into();
    event.payload["summary"] = serde_json::json!(calendar.summary);
    event.integrity_hash = event_hash(&event).map_err(CompanionError::Serialize)?;
    Ok(event)
}

pub(crate) fn scan_git_repo(repo: &Path) -> Result<Vec<GitCommitEvidence>, String> {
    if !repo.exists() {
        return Err("repository path does not exist".to_owned());
    }
    let worktree = git_stdout(repo, ["rev-parse", "--show-toplevel"])?;
    let branch = git_stdout(repo, ["branch", "--show-current"])
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "DETACHED".to_owned());
    let ref_name = git_stdout(
        repo,
        ["rev-parse", "--symbolic-full-name", "--quiet", "HEAD"],
    )
    .ok()
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "HEAD".to_owned());
    let output = git_stdout(
        repo,
        [
            "log",
            "--all",
            "--max-count=200",
            "--date=iso-strict",
            "--format=%H%x1f%an%x1f%ae%x1f%cn%x1f%ce%x1f%aI%x1f%cI%x1f%s%x1e",
        ],
    )?;
    let mut commits = Vec::new();
    for record in output
        .split('\u{1e}')
        .filter(|record| !record.trim().is_empty())
    {
        let fields: Vec<&str> = record.trim_matches('\n').split('\u{1f}').collect();
        if fields.len() != 8 {
            return Err("unexpected git log format".to_owned());
        }
        let subject = minimize_subject(fields[7]);
        commits.push(GitCommitEvidence {
            commit: fields[0].to_owned(),
            author: GitIdentity {
                name: fields[1].to_owned(),
                email: fields[2].to_owned(),
            },
            committer: GitIdentity {
                name: fields[3].to_owned(),
                email: fields[4].to_owned(),
            },
            author_timestamp: fields[5].to_owned(),
            committer_timestamp: fields[6].to_owned(),
            repository: GitRepositoryIdentity {
                path: repo.display().to_string(),
                worktree: worktree.clone(),
            },
            branch: branch.clone(),
            ref_name: ref_name.clone(),
            issue_candidates: issue_candidates(&subject),
            subject,
        });
    }
    Ok(commits)
}

pub(crate) fn git_stdout<const N: usize>(repo: &Path, args: [&str; N]) -> Result<String, String> {
    let mut command = ProcessCommand::new("git");
    clear_git_repository_environment(&mut command);
    let output = command
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn clear_git_repository_environment(command: &mut ProcessCommand) {
    // Git hooks export repository-local variables. Without clearing them, `git -C`
    // can still target the hook's repository instead of the configured evidence source.
    for name in [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_DIR",
        "GIT_GRAFT_FILE",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_INTERNAL_SUPER_PREFIX",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_OBJECT_DIRECTORY",
        "GIT_PREFIX",
        "GIT_REPLACE_REF_BASE",
        "GIT_SHALLOW_FILE",
        "GIT_WORK_TREE",
    ] {
        command.env_remove(name);
    }
}

pub(crate) fn minimize_subject(subject: &str) -> String {
    const MAX: usize = 72;
    let clean = subject.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.len() <= MAX {
        clean
    } else {
        let mut minimized = String::new();
        for ch in clean.chars() {
            if minimized.len() + ch.len_utf8() + 3 > MAX {
                break;
            }
            minimized.push(ch);
        }
        minimized.push('…');
        minimized
    }
}

pub(crate) fn issue_candidates(subject: &str) -> Vec<IssueCandidate> {
    subject
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .filter(|part| {
            let Some((project, number)) = part.split_once('-') else {
                return false;
            };
            project.len() >= 2
                && project.chars().all(|ch| ch.is_ascii_uppercase())
                && number.chars().all(|ch| ch.is_ascii_digit())
        })
        .map(|key| IssueCandidate {
            key: key.to_owned(),
            origin: "commit-subject",
            confidence: "candidate",
        })
        .collect()
}

pub(crate) fn git_commit_event(commit: &GitCommitEvidence) -> Result<JournalEvent, CompanionError> {
    let explicit_date = commit
        .author_timestamp
        .get(..10)
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .unwrap_or_else(|| Utc::now().date_naive());
    let mut event = JournalEvent {
        schema_version: JOURNAL_SCHEMA_VERSION,
        event_id: format!("evidence.git.{}", commit.commit),
        event_type: "git.commit".to_owned(),
        observed_at: commit.author_timestamp.clone(),
        source: SourceProvenance {
            kind: "git".to_owned(),
            adapter: "git-local".to_owned(),
            reference: format!("{}@{}", commit.repository.worktree, commit.commit),
        },
        collector: CollectorProvenance {
            name: "git-local".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        timestamp_semantics: TimestampSemantics {
            observed_at_source: "git-author-timestamp".to_owned(),
            timezone: "from-git-offset".to_owned(),
            explicit_date,
        },
        privacy: PrivacyState {
            classification: "local-git-metadata".to_owned(),
            redacted: false,
        },
        retention: RetentionMetadata {
            policy: "retain-until-user-purge".to_owned(),
            retain_until: None,
        },
        supersedes: None,
        payload: serde_json::to_value(commit).map_err(CompanionError::Serialize)?,
        integrity_hash: String::new(),
    };
    event.integrity_hash = event_hash(&event).map_err(CompanionError::Serialize)?;
    Ok(event)
}

pub(crate) fn read_settings(path: &Path) -> Result<Value, CompanionError> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let body = fs::read_to_string(path).map_err(|source| CompanionError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&body)
        .map_err(|error| CompanionError::InvalidClaudeHook(error.to_string()))
}

pub(crate) fn write_settings(path: &Path, settings: &Value) -> Result<(), CompanionError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CompanionError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let body = serde_json::to_vec_pretty(settings).map_err(CompanionError::Serialize)?;
    atomic_write(path, &body)
}

pub(crate) fn is_our_hook_entry(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|commands| commands.iter().any(is_our_command))
}

pub(crate) fn is_our_command(command: &Value) -> bool {
    command
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(CLAUDE_HOOK_COMMAND))
}

pub(crate) fn read_claude_hook_event(data_dir: &Path) -> Result<JournalEvent, CompanionError> {
    let mut stdin = String::new();
    io::stdin()
        .read_to_string(&mut stdin)
        .map_err(|source| CompanionError::Read {
            path: PathBuf::from("<stdin>"),
            source,
        })?;
    let payload: Value = serde_json::from_str(&stdin)
        .map_err(|error| CompanionError::InvalidClaudeHook(error.to_string()))?;
    claude_hook_event_from_payload(data_dir, &payload)
}

pub(crate) fn claude_hook_event_from_payload(
    _data_dir: &Path,
    payload: &Value,
) -> Result<JournalEvent, CompanionError> {
    let kind = payload
        .get("hook_event_name")
        .or_else(|| payload.get("event"))
        .or_else(|| payload.get("hookEventName"))
        .and_then(Value::as_str)
        .ok_or_else(|| CompanionError::InvalidClaudeHook("missing lifecycle event".to_owned()))?;
    if !matches!(kind, "SessionStart" | "SessionEnd") {
        return Err(CompanionError::InvalidClaudeHook(format!(
            "unsupported lifecycle event {kind}"
        )));
    }
    let session_id = payload
        .get("session_id")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CompanionError::InvalidClaudeHook("missing session id".to_owned()))?;
    let observed_at = payload
        .get("timestamp")
        .or_else(|| payload.get("observed_at"))
        .or_else(|| payload.get("observedAt"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(now_string);
    let explicit_date = normalize_timestamp(&observed_at)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(&timestamp).ok())
        .map(|timestamp| timestamp.date_naive())
        .unwrap_or_else(|| Utc::now().date_naive());
    let cwd = payload
        .get("cwd")
        .or_else(|| payload.get("workspace"))
        .and_then(Value::as_str);
    let repo = find_repo_link(cwd).unwrap_or_else(|| "unknown".to_owned());
    let mut lifecycle_payload = serde_json::json!({
        "schemaVersion": CLAUDE_HOOK_SCHEMA_VERSION,
        "lifecycleKind": kind,
        "sessionId": session_id,
        "observedAt": observed_at,
        "repository": repo,
        "summary": format!("Claude Code {kind} captured locally for repository {repo}"),
        "networkAccess": false,
        "transcriptCaptured": false,
    });
    if kind == "SessionStart" {
        lifecycle_payload["intervalStart"] = serde_json::json!(observed_at);
    } else {
        lifecycle_payload["intervalEnd"] = serde_json::json!(observed_at);
    }
    let mut event = JournalEvent {
        schema_version: JOURNAL_SCHEMA_VERSION,
        event_id: format!("evidence.claude.{session_id}.{kind}"),
        event_type: "evidence.claude.lifecycle".to_owned(),
        observed_at: observed_at.clone(),
        source: SourceProvenance {
            kind: "claude-code".to_owned(),
            adapter: CLAUDE_COLLECTOR.to_owned(),
            reference: format!("{repo}#{session_id}"),
        },
        collector: CollectorProvenance {
            name: CLAUDE_COLLECTOR.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        timestamp_semantics: TimestampSemantics {
            observed_at_source: observed_at,
            timezone: "UTC".to_owned(),
            explicit_date,
        },
        privacy: PrivacyState {
            classification: "local-metadata".to_owned(),
            redacted: true,
        },
        retention: RetentionMetadata {
            policy: "retain-until-user-purge".to_owned(),
            retain_until: None,
        },
        supersedes: None,
        payload: lifecycle_payload,
        integrity_hash: String::new(),
    };
    event.integrity_hash = event_hash(&event).map_err(CompanionError::Serialize)?;
    Ok(event)
}

pub(crate) fn find_repo_link(cwd: Option<&str>) -> Option<String> {
    let cwd = Path::new(cwd?);
    cwd.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}
