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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrackingSource {
    pub(crate) kind: String,
    pub(crate) path: PathBuf,
    pub(crate) enabled: bool,
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
) -> Vec<TrackingSource> {
    repos
        .into_iter()
        .map(|path| TrackingSource {
            kind: "git".to_owned(),
            path,
            enabled: true,
        })
        .chain(ics_files.into_iter().map(|path| TrackingSource {
            kind: "calendar".to_owned(),
            path,
            enabled: true,
        }))
        .collect()
}

pub(crate) fn resolve_data_dir(explicit: Option<PathBuf>) -> Result<PathBuf, CompanionError> {
    if let Some(path) = explicit {
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
        return Ok(path);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let target = home.join(".drag/tracking");
    migrate_legacy_data_dir(&target, &PathBuf::from(".drag-companion"))?;
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
    fs::rename(legacy, target).map_err(|source| CompanionError::Write {
        path: target.to_path_buf(),
        source,
    })?;
    atomic_write(
        &target.join("migration.json"),
        br#"{"schemaVersion":1,"source":".drag-companion","status":"completed"}"#,
    )
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
