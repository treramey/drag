use crate::*;

pub(crate) const SOURCE_TEST_SOURCE_LIMIT: usize = 64;
pub(crate) const SOURCE_TEST_OBSERVATION_LIMIT: usize = 200;
pub(crate) const MAX_SOURCE_SETTINGS_BYTES: u64 = 1024 * 1024;

pub(crate) fn source_statuses(config: &TrackingConfig) -> Vec<Value> {
    config.sources.iter().map(source_status).collect()
}

pub(crate) fn tested_source_statuses(config: &TrackingConfig, date: NaiveDate) -> Vec<Value> {
    config
        .sources
        .iter()
        .take(SOURCE_TEST_SOURCE_LIMIT)
        .map(|source| tested_source_status(source, date))
        .collect()
}

pub(crate) fn validate_source_configuration(
    sources: &[TrackingSource],
) -> Result<(), CompanionError> {
    if sources.len() > SOURCE_TEST_SOURCE_LIMIT {
        return Err(CompanionError::Proposal(format!(
            "source configuration is limited to {SOURCE_TEST_SOURCE_LIMIT} entries"
        )));
    }
    let mut references = Vec::with_capacity(sources.len());
    for source in sources {
        let reference = source_reference(source);
        if references.contains(&reference) {
            return Err(CompanionError::Proposal(
                "source configuration contains a duplicate selection".to_owned(),
            ));
        }
        references.push(reference);
        let inspection = inspect_source(source, None);
        if inspection.health != "healthy" {
            let kind = match source.kind {
                TrackingSourceKind::Git => "Git",
                TrackingSourceKind::Calendar => "calendar",
                TrackingSourceKind::ClaudeCode => "Claude Code",
            };
            return Err(CompanionError::Proposal(format!(
                "{kind} source is invalid or unavailable; {}",
                inspection
                    .reason
                    .unwrap_or("select an accessible supported local source")
            )));
        }
    }
    Ok(())
}

struct SourceInspection {
    available: bool,
    health: &'static str,
    reason: Option<&'static str>,
    observations: Option<usize>,
    truncated: bool,
}

fn source_status(source: &TrackingSource) -> Value {
    source_status_value(source, inspect_source(source, None))
}

fn tested_source_status(source: &TrackingSource, date: NaiveDate) -> Value {
    let inspection = inspect_source(source, Some(date));
    let observations = inspection.observations.unwrap_or(0);
    let truncated = inspection.truncated;
    let mut status = source_status_value(source, inspection);
    status["check"] = serde_json::json!({
        "bounded": true,
        "observationLimit": SOURCE_TEST_OBSERVATION_LIMIT,
        "observations": observations,
        "truncated": truncated
    });
    status
}

fn source_status_value(source: &TrackingSource, inspection: SourceInspection) -> Value {
    serde_json::json!({
        "kind": source.kind,
        "reference": source_reference(source),
        "configured": true,
        "enabled": source.enabled,
        "available": inspection.available,
        "health": inspection.health,
        "reason": inspection.reason
    })
}

fn inspect_source(source: &TrackingSource, date: Option<NaiveDate>) -> SourceInspection {
    let unavailable = || SourceInspection {
        available: false,
        health: "unavailable",
        reason: Some("configured path does not exist or cannot be read"),
        observations: date.map(|_| 0),
        truncated: false,
    };
    let unhealthy = |reason| SourceInspection {
        available: true,
        health: "unhealthy",
        reason: Some(reason),
        observations: date.map(|_| 0),
        truncated: false,
    };
    let healthy = |observations: Option<usize>, truncated| SourceInspection {
        available: true,
        health: "healthy",
        reason: None,
        observations,
        truncated,
    };

    let Ok(metadata) = fs::metadata(&source.path) else {
        return unavailable();
    };
    match source.kind {
        TrackingSourceKind::Git => {
            if !metadata.is_dir() {
                return unhealthy("configured Git source is not a directory");
            }
            if date.is_some() {
                match scan_git_repo_for_date(&source.path, date) {
                    Ok(observations) => {
                        let count = observations.len();
                        healthy(
                            Some(count.min(SOURCE_TEST_OBSERVATION_LIMIT)),
                            count == SOURCE_TEST_OBSERVATION_LIMIT,
                        )
                    }
                    Err(_) => unhealthy("configured directory is not a readable Git repository"),
                }
            } else if git_stdout(&source.path, ["rev-parse", "--is-inside-work-tree"])
                .is_ok_and(|value| value == "true")
            {
                healthy(None, false)
            } else {
                unhealthy("configured directory is not a readable Git repository")
            }
        }
        TrackingSourceKind::Calendar => {
            if !metadata.is_file() {
                return unhealthy("configured calendar source is not a file");
            }
            if metadata.len() > MAX_SOURCE_SETTINGS_BYTES {
                return unhealthy("configured calendar exceeds the 1 MiB safety limit");
            }
            let Ok(body) = fs::read_to_string(&source.path) else {
                return unavailable();
            };
            let has_calendar = body.lines().any(|line| line.trim() == "BEGIN:VCALENDAR")
                && body.lines().any(|line| line.trim() == "END:VCALENDAR");
            if !has_calendar {
                return unhealthy("configured calendar is invalid RFC 5545 data");
            }
            match scan_ics_file(
                &source.path,
                date.unwrap_or_else(|| Utc::now().date_naive()),
            ) {
                Ok(observations) => {
                    let count = observations.len();
                    healthy(
                        date.map(|_| count.min(SOURCE_TEST_OBSERVATION_LIMIT)),
                        date.is_some() && count > SOURCE_TEST_OBSERVATION_LIMIT,
                    )
                }
                Err(_) => unhealthy("configured calendar is invalid RFC 5545 data"),
            }
        }
        TrackingSourceKind::ClaudeCode => {
            if !metadata.is_file() {
                return unhealthy("configured Claude Code settings source is not a file");
            }
            if metadata.len() > MAX_SOURCE_SETTINGS_BYTES {
                return unhealthy("configured Claude Code settings exceed the 1 MiB safety limit");
            }
            let Ok(body) = fs::read_to_string(&source.path) else {
                return unavailable();
            };
            let Ok(settings) = serde_json::from_str::<Value>(&body) else {
                return unhealthy("configured Claude Code settings are invalid JSON");
            };
            let hooks_installed = ["SessionStart", "SessionEnd"].into_iter().all(|event| {
                settings["hooks"][event]
                    .as_array()
                    .is_some_and(|entries| entries.iter().any(is_our_hook_entry))
            });
            if hooks_installed {
                healthy(date.map(|_| 0), false)
            } else {
                unhealthy(
                    "tracking-owned Claude Code lifecycle hooks are not installed; run tracking setup --install-hooks",
                )
            }
        }
    }
}

fn source_reference(source: &TrackingSource) -> String {
    let canonical = source
        .path
        .canonicalize()
        .unwrap_or_else(|_| source.path.clone());
    let mut digest = Sha256::new();
    digest.update(source.kind.as_str().as_bytes());
    digest.update([0]);
    update_path_digest(&mut digest, &canonical);
    format!("local-source:sha256:{:x}", digest.finalize())
}

#[cfg(unix)]
fn update_path_digest(digest: &mut Sha256, path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    digest.update(path.as_os_str().as_bytes());
}

#[cfg(windows)]
fn update_path_digest(digest: &mut Sha256, path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    for unit in path.as_os_str().encode_wide() {
        digest.update(unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, windows)))]
fn update_path_digest(digest: &mut Sha256, path: &Path) {
    digest.update(path.to_string_lossy().as_bytes());
}
