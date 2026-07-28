use crate::*;

pub(crate) const TRACKING_CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrackingConfig {
    pub(crate) schema_version: u32,
    pub(crate) installed: bool,
    pub(crate) active: bool,
    pub(crate) sources: Vec<TrackingSource>,
    pub(crate) schedule: TrackingSchedule,
    pub(crate) submission: TrackingSubmission,
    pub(crate) scheduler_target: Option<PathBuf>,
    pub(crate) hooks_installed: bool,
    #[serde(default)]
    pub(crate) provider_fixture: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrackingSource {
    pub(crate) kind: TrackingSourceKind,
    pub(crate) path: PathBuf,
    pub(crate) enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TrackingSourceKind {
    Git,
    Calendar,
    ClaudeCode,
}

impl TrackingSourceKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Calendar => "calendar",
            Self::ClaudeCode => "claude-code",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrackingSchedule {
    pub(crate) weekdays: bool,
    pub(crate) at: String,
    pub(crate) timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrackingSubmission {
    pub(crate) mode: SubmissionMode,
    pub(crate) automatic_submission_authorized: bool,
}

impl Default for TrackingConfig {
    fn default() -> Self {
        Self {
            schema_version: TRACKING_CONFIG_SCHEMA_VERSION,
            installed: false,
            active: false,
            sources: Vec::new(),
            schedule: TrackingSchedule {
                weekdays: true,
                at: DEFAULT_SCHEDULE_TIME.to_owned(),
                timezone: DEFAULT_SCHEDULE_TIMEZONE.to_owned(),
            },
            submission: TrackingSubmission {
                mode: SubmissionMode::Draft,
                automatic_submission_authorized: false,
            },
            scheduler_target: None,
            hooks_installed: false,
            provider_fixture: None,
        }
    }
}

pub(crate) fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

pub(crate) fn load_tracking_config(
    data_dir: &Path,
) -> Result<Option<TrackingConfig>, CompanionError> {
    let path = config_path(data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let body = fs::read_to_string(&path).map_err(|source| CompanionError::Read {
        path: path.clone(),
        source,
    })?;
    let config: TrackingConfig = serde_json::from_str(&body)
        .map_err(|error| CompanionError::Proposal(format!("tracking config schema: {error}")))?;
    if config.schema_version != TRACKING_CONFIG_SCHEMA_VERSION {
        return Err(CompanionError::Proposal(format!(
            "unsupported tracking config schema {}; expected {}",
            config.schema_version, TRACKING_CONFIG_SCHEMA_VERSION
        )));
    }
    Ok(Some(config))
}

pub(crate) fn save_tracking_config(
    data_dir: &Path,
    config: &TrackingConfig,
) -> Result<(), CompanionError> {
    fs::create_dir_all(data_dir).map_err(|source| CompanionError::CreateDir {
        path: data_dir.to_path_buf(),
        source,
    })?;
    ensure_companion_sentinel(data_dir)?;
    let body = serde_json::to_vec_pretty(config).map_err(CompanionError::Serialize)?;
    atomic_write(&config_path(data_dir), &body)
}

pub(crate) fn configured_sources(
    repos: Vec<PathBuf>,
    ics_files: Vec<PathBuf>,
) -> Result<Vec<TrackingSource>, CompanionError> {
    repos
        .into_iter()
        .map(|path| {
            Ok(TrackingSource {
                kind: TrackingSourceKind::Git,
                path: stable_source_path(path)?,
                enabled: true,
            })
        })
        .chain(ics_files.into_iter().map(|path| {
            Ok(TrackingSource {
                kind: TrackingSourceKind::Calendar,
                path: stable_source_path(path)?,
                enabled: true,
            })
        }))
        .collect()
}

pub(crate) fn stable_source_path(path: PathBuf) -> Result<PathBuf, CompanionError> {
    let absolute = std::path::absolute(&path).map_err(|error| {
        CompanionError::Proposal(format!(
            "could not make configured source path {} absolute: {error}",
            path.display()
        ))
    })?;
    absolute.canonicalize().map_err(|error| {
        CompanionError::Proposal(format!(
            "could not canonicalize configured source path {}: {error}",
            path.display()
        ))
    })
}

pub(crate) fn claude_code_source() -> Result<TrackingSource, CompanionError> {
    Ok(TrackingSource {
        kind: TrackingSourceKind::ClaudeCode,
        path: stable_source_path(default_claude_settings_path())?,
        enabled: true,
    })
}

pub(crate) fn resolve_data_dir(explicit: Option<PathBuf>) -> Result<PathBuf, CompanionError> {
    validate_tracking_environment()?;
    if let Some(path) = explicit {
        finalize_migration_record(&path)?;
        return Ok(path);
    }
    let current = std::env::var_os("DRAG_TRACKING_DATA").map(PathBuf::from);
    let legacy = std::env::var_os("DRAG_COMPANION_DATA").map(PathBuf::from);
    if let (Some(current), Some(legacy)) = (&current, &legacy) {
        if current != legacy {
            return Err(CompanionError::Proposal(
                "DRAG_TRACKING_DATA and deprecated DRAG_COMPANION_DATA conflict; remove one or set both to the same directory"
                    .to_owned(),
            ));
        }
    }
    if let Some(path) = current.or(legacy) {
        finalize_migration_record(&path)?;
        return Ok(path);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let target = home.join(".drag/tracking");
    finalize_migration_record(&target)?;
    let legacy =
        migration_source_path(&target)?.unwrap_or_else(|| PathBuf::from(".drag-companion"));
    migrate_legacy_data_dir(&target, &legacy)?;
    Ok(target)
}

pub(crate) fn migrate_legacy_data_dir(target: &Path, legacy: &Path) -> Result<(), CompanionError> {
    if !legacy.exists() {
        return Ok(());
    }
    if target.exists() {
        return Err(CompanionError::Proposal(format!(
            "both legacy tracking state {} and new state {} exist; disable scheduled tracking and resolve the duplicate stores before continuing",
            legacy.display(),
            target.display()
        )));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|source| CompanionError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let source = if legacy.is_absolute() {
        legacy.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| CompanionError::Read {
                path: legacy.to_path_buf(),
                source,
            })?
            .join(legacy)
    };
    atomic_write(
        &migration_source_record_path(target),
        source.as_os_str().as_encoded_bytes(),
    )?;
    atomic_write(
        &legacy.join("migration.json"),
        br#"{"schemaVersion":1,"source":".drag-companion","status":"inProgress","recoveryAction":"rerun drag tracking status"}"#,
    )?;
    if std::env::var_os("DRAG_TRACKING_TEST_INTERRUPT_MIGRATION").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        return Err(CompanionError::Proposal(
            "tracking state migration interrupted; rerun drag tracking status to resume".to_owned(),
        ));
    }
    match fs::rename(legacy, target) {
        Ok(()) => {}
        Err(error) if is_cross_device_error(&error) => {
            let staging = target.with_extension(format!("migration-{}", std::process::id()));
            if staging.exists() {
                fs::remove_dir_all(&staging).map_err(|source| CompanionError::Write {
                    path: staging.clone(),
                    source,
                })?;
            }
            if let Err(copy_error) = copy_directory_durable(legacy, &staging) {
                let _ = fs::remove_dir_all(&staging);
                return Err(copy_error);
            }
            fs::rename(&staging, target).map_err(|source| CompanionError::Write {
                path: target.to_path_buf(),
                source,
            })?;
            if let Some(parent) = target.parent() {
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|source| CompanionError::Write {
                        path: parent.to_path_buf(),
                        source,
                    })?;
            }
            fs::remove_dir_all(legacy).map_err(|source| CompanionError::Write {
                path: legacy.to_path_buf(),
                source,
            })?;
        }
        Err(source) => {
            return Err(CompanionError::Write {
                path: target.to_path_buf(),
                source,
            });
        }
    }
    if std::env::var_os("DRAG_TRACKING_TEST_INTERRUPT_MIGRATION").as_deref()
        == Some(std::ffi::OsStr::new("after-move"))
    {
        return Err(CompanionError::Proposal(
            "tracking state migration interrupted; rerun drag tracking status to resume".to_owned(),
        ));
    }
    complete_migration_record(target)
}

#[cfg(unix)]
fn is_cross_device_error(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(18)
}

#[cfg(not(unix))]
fn is_cross_device_error(_error: &std::io::Error) -> bool {
    false
}

fn copy_directory_durable(source: &Path, target: &Path) -> Result<(), CompanionError> {
    fs::create_dir(target).map_err(|source_error| CompanionError::CreateDir {
        path: target.to_path_buf(),
        source: source_error,
    })?;
    for entry in fs::read_dir(source).map_err(|source_error| CompanionError::Read {
        path: source.to_path_buf(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| CompanionError::Read {
            path: source.to_path_buf(),
            source: source_error,
        })?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|source_error| CompanionError::Read {
                path: from.clone(),
                source: source_error,
            })?;
        if file_type.is_dir() {
            copy_directory_durable(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to).map_err(|source_error| CompanionError::Write {
                path: to.clone(),
                source: source_error,
            })?;
            File::open(&to)
                .and_then(|file| file.sync_all())
                .map_err(|source_error| CompanionError::Write {
                    path: to,
                    source: source_error,
                })?;
        } else {
            return Err(CompanionError::Proposal(format!(
                "legacy tracking state contains unsupported file type: {}",
                from.display()
            )));
        }
    }
    File::open(target)
        .and_then(|directory| directory.sync_all())
        .map_err(|source_error| CompanionError::Write {
            path: target.to_path_buf(),
            source: source_error,
        })
}

fn finalize_migration_record(target: &Path) -> Result<(), CompanionError> {
    let path = target.join("migration.json");
    let in_progress = fs::read_to_string(&path)
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .and_then(|record| record["status"].as_str().map(str::to_owned))
        .as_deref()
        == Some("inProgress");
    if in_progress {
        complete_migration_record(target)?;
    }
    if target.exists() {
        remove_migration_source_record(target)?;
    }
    Ok(())
}

fn complete_migration_record(target: &Path) -> Result<(), CompanionError> {
    atomic_write(
        &target.join("migration.json"),
        br#"{"schemaVersion":1,"source":".drag-companion","status":"completed","recoveryAction":"pause tracking, move the directory back to .drag-companion, and reinstall the previous release"}"#,
    )?;
    remove_migration_source_record(target)
}

fn migration_source_record_path(target: &Path) -> PathBuf {
    target.with_extension("migration-source")
}

fn migration_source_path(target: &Path) -> Result<Option<PathBuf>, CompanionError> {
    let path = migration_source_record_path(target);
    if !path.exists() {
        return Ok(None);
    }
    let source = fs::read_to_string(&path).map_err(|source| CompanionError::Read {
        path: path.clone(),
        source,
    })?;
    Ok(Some(PathBuf::from(source)))
}

fn remove_migration_source_record(target: &Path) -> Result<(), CompanionError> {
    let path = migration_source_record_path(target);
    if path.exists() {
        fs::remove_file(&path).map_err(|source| CompanionError::Write { path, source })?;
    }
    Ok(())
}

fn validate_tracking_environment() -> Result<(), CompanionError> {
    for suffix in [
        "DATA",
        "KILL_SWITCH",
        "LIVE_MUTATION_ROLLOUT",
        "RETENTION_NOW",
        "RETENTION_RAW_DAYS",
        "RETENTION_NORMALIZED_DAYS",
        "RETENTION_REPORT_LEDGER_DAYS",
        "TEMPO_WORK_ATTRIBUTES",
    ] {
        let current = format!("DRAG_TRACKING_{suffix}");
        let legacy = format!("DRAG_COMPANION_{suffix}");
        let current_value = std::env::var_os(&current);
        let legacy_value = std::env::var_os(&legacy);
        if matches!((&current_value, &legacy_value), (Some(left), Some(right)) if left != right) {
            return Err(CompanionError::Proposal(format!(
                "{current} and deprecated {legacy} conflict; remove one or set both to the same value"
            )));
        }
    }
    Ok(())
}

pub(crate) fn environment_enabled(current: &str, legacy: &str) -> Result<bool, CompanionError> {
    let current_value = std::env::var_os(current);
    let legacy_value = std::env::var_os(legacy);
    if let (Some(current_value), Some(legacy_value)) = (&current_value, &legacy_value) {
        if current_value != legacy_value {
            return Err(CompanionError::Proposal(format!(
                "{current} and deprecated {legacy} conflict; remove one or set both to the same value"
            )));
        }
    }
    Ok(current_value.or(legacy_value).is_some())
}
