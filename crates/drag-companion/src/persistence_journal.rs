use crate::*;

pub(crate) fn atomic_write(path: &Path, body: &[u8]) -> Result<(), CompanionError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| CompanionError::Open {
            path: parent.to_path_buf(),
            source,
        })?;
    file.write_all(body)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|source| CompanionError::Write {
            path: file.path().to_path_buf(),
            source,
        })?;
    file.persist(path).map_err(|error| CompanionError::Write {
        path: path.to_path_buf(),
        source: error.error,
    })?;
    sync_parent_directory(parent)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<(), CompanionError> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| CompanionError::Write {
            path: parent.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Result<(), CompanionError> {
    Ok(())
}

pub(crate) fn persist_result(data_dir: &Path, result: &RunResult) -> Result<(), CompanionError> {
    let runs_dir = data_dir.join("runs");
    fs::create_dir_all(&runs_dir).map_err(|source| CompanionError::CreateDir {
        path: runs_dir.clone(),
        source,
    })?;
    let path = run_path(data_dir, result.date);
    let body = serde_json::to_vec_pretty(result).map_err(CompanionError::Serialize)?;
    atomic_write(&path, &body)
}

pub(crate) fn journal_path(data_dir: &Path) -> PathBuf {
    data_dir.join("journal.jsonl")
}
pub(crate) fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("companion.sqlite3")
}
pub(crate) fn run_path(data_dir: &Path, date: NaiveDate) -> PathBuf {
    data_dir.join("runs").join(format!("{date}.json"))
}
pub(crate) fn now_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(crate) fn retention_now() -> Result<DateTime<Utc>, CompanionError> {
    match std::env::var("DRAG_COMPANION_RETENTION_NOW") {
        Ok(value) => DateTime::parse_from_rfc3339(&value)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|error| {
                CompanionError::Proposal(format!(
                    "DRAG_COMPANION_RETENTION_NOW must be RFC3339: {error}"
                ))
            }),
        Err(_) => Ok(Utc::now()),
    }
}
