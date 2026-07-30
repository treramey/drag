use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{CliError, Rendered};

const MAX_HOOK_INPUT_BYTES: u64 = 64 * 1024;
const MANAGED_COMMAND_PREFIX: &str = "drag tracking capture --state-dir-base64 ";

pub(crate) fn install_default() -> Result<Value, CliError> {
    let settings_path = settings_path()?;
    let state_dir = state_dir()?;
    install(&settings_path, &state_dir)
}

pub(crate) fn plan() -> Result<Value, CliError> {
    let settings_path = settings_path()?;
    let state_dir = state_dir()?;
    prepared_settings(&settings_path, &state_dir)?;
    Ok(json!({
        "status": "planned", "settingsPath": settings_path, "statePath": state_dir,
        "settingsWillChange": true, "stateWillInitialize": true, "networkAccess": false
    }))
}

fn install(settings_path: &Path, state_dir: &Path) -> Result<Value, CliError> {
    let settings = prepared_settings(settings_path, state_dir)?;
    // Hooks must never become active before their destination can be initialized.
    fs::create_dir_all(state_dir)?;
    atomic_json(settings_path, &settings)?;
    Ok(json!({
        "status": "enabled", "settingsPath": settings_path, "statePath": state_dir,
        "hooks": ["SessionStart", "SessionEnd"], "networkAccess": false,
        "worklogsCreated": false
    }))
}

fn prepared_settings(settings_path: &Path, state_dir: &Path) -> Result<Value, CliError> {
    let mut settings = if settings_path.exists() {
        serde_json::from_slice::<Value>(&fs::read(settings_path)?)?
    } else {
        json!({})
    };
    let object = settings.as_object_mut().ok_or_else(|| {
        CliError::InvalidInput("Claude settings must be a JSON object".to_owned())
    })?;
    let hooks = object.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or_else(|| {
        CliError::InvalidInput("Claude settings hooks must be a JSON object".to_owned())
    })?;
    let state_dir = state_dir.to_str().ok_or_else(|| {
        CliError::InvalidInput("Claude tracking state path must be valid UTF-8".to_owned())
    })?;
    let managed_command = format!(
        "{MANAGED_COMMAND_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(state_dir.as_bytes())
    );
    for event in ["SessionStart", "SessionEnd"] {
        let entries = hooks.entry(event).or_insert_with(|| json!([]));
        let entries = entries.as_array_mut().ok_or_else(|| {
            CliError::InvalidInput(format!("Claude {event} hooks must be an array"))
        })?;
        let mut found = false;
        for entry in entries.iter_mut() {
            let Some(commands) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            for command in commands {
                if command
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(is_managed_command)
                {
                    command["command"] = json!(managed_command);
                    command["type"] = json!("command");
                    found = true;
                }
            }
        }
        if !found {
            entries.push(
                json!({"matcher": "*", "hooks": [{"type": "command", "command": managed_command}]}),
            );
        }
    }
    Ok(settings)
}

fn is_managed_command(value: &str) -> bool {
    value
        .strip_prefix(MANAGED_COMMAND_PREFIX)
        .is_some_and(|encoded| !encoded.is_empty() && !encoded.contains(char::is_whitespace))
}

pub(crate) fn capture_from_stdin(encoded_state_dir: Option<&str>) -> Result<Rendered, CliError> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(MAX_HOOK_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_HOOK_INPUT_BYTES {
        return Err(CliError::InvalidInput(
            "Claude lifecycle payload exceeds 64 KiB".to_owned(),
        ));
    }
    let payload: Value = serde_json::from_slice(&bytes).map_err(|error| {
        CliError::InvalidInput(format!("invalid Claude lifecycle payload: {error}"))
    })?;
    let state_dir = encoded_state_dir.map_or_else(state_dir, decode_state_dir)?;
    capture(&state_dir, &payload)
}

fn decode_state_dir(encoded: &str) -> Result<PathBuf, CliError> {
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| {
        CliError::InvalidInput("invalid encoded Claude tracking state path".to_owned())
    })?;
    let path = String::from_utf8(bytes).map_err(|_| {
        CliError::InvalidInput("Claude tracking state path must be valid UTF-8".to_owned())
    })?;
    if path.is_empty() {
        return Err(CliError::InvalidInput(
            "Claude tracking state path cannot be empty".to_owned(),
        ));
    }
    Ok(PathBuf::from(path))
}

fn capture(state_dir: &Path, payload: &Value) -> Result<Rendered, CliError> {
    let event = payload
        .get("hook_event_name")
        .or_else(|| payload.get("hookEventName"))
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::InvalidInput("missing Claude lifecycle event".to_owned()))?;
    if !matches!(event, "SessionStart" | "SessionEnd") {
        return Err(CliError::InvalidInput(
            "unsupported Claude lifecycle event".to_owned(),
        ));
    }
    let session = payload
        .get("session_id")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CliError::InvalidInput("missing Claude session id".to_owned()))?;
    if session.len() > 1024 {
        return Err(CliError::InvalidInput(
            "Claude session id is too long".to_owned(),
        ));
    }
    let observed_at = payload
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    chrono::DateTime::parse_from_rfc3339(&observed_at).map_err(|_| {
        CliError::InvalidInput("Claude lifecycle timestamp must be RFC 3339".to_owned())
    })?;
    let session_hash = hash(session);
    let project_hash = payload.get("cwd").and_then(Value::as_str).map(hash);
    let event_id = hash(&format!("{session_hash}:{event}"));
    fs::create_dir_all(state_dir)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(state_dir.join("events.lock"))?;
    FileExt::lock_exclusive(&lock)?;
    let path = state_dir.join("events.json");
    let mut events = if path.exists() {
        serde_json::from_slice::<Value>(&fs::read(&path)?)?
    } else {
        json!({})
    };
    let map = events.as_object_mut().ok_or_else(|| CliError::Config {
        message: "Claude event store must be a JSON object".to_owned(),
        source: None,
    })?;
    let duplicate = map.contains_key(&event_id);
    map.entry(event_id.clone()).or_insert_with(|| {
        json!({
            "schemaVersion": 1, "eventId": event_id, "lifecycle": event,
            "sessionHash": session_hash, "projectHash": project_hash,
            "observedAt": observed_at, "networkAccess": false
        })
    });
    if !duplicate {
        atomic_json(&path, &events)?;
    }
    Ok(Rendered::new(
        json!({"captured": true, "duplicate": duplicate, "networkAccess": false}),
        String::new(),
    ))
}

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn settings_path() -> Result<PathBuf, CliError> {
    if let Some(path) = std::env::var_os("DRAG_CLAUDE_SETTINGS") {
        return Ok(path.into());
    }
    dirs::home_dir()
        .map(|path| path.join(".claude/settings.json"))
        .ok_or_else(|| CliError::Config {
            message: "could not determine Claude settings path".to_owned(),
            source: None,
        })
}
fn state_dir() -> Result<PathBuf, CliError> {
    if let Some(path) = std::env::var_os("DRAG_TRACKING_DIR") {
        return Ok(path.into());
    }
    dirs::home_dir()
        .map(|path| path.join(".drag/tracking"))
        .ok_or_else(|| CliError::Config {
            message: "could not determine tracking state path".to_owned(),
            source: None,
        })
}

fn atomic_json(path: &Path, value: &Value) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(value)?;
    #[cfg(windows)]
    {
        let file = atomicwrites::AtomicFile::new(path, atomicwrites::AllowOverwrite);
        file.write(|temporary| temporary.write_all(&body))
            .map_err(std::io::Error::from)?;
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mode = fs::metadata(path)
            .map(|metadata| {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o777
            })
            .unwrap_or(0o600);
        let temporary = path.with_extension(format!("{}.drag.tmp", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(&body)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_preserves_settings_and_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let settings = directory.path().join("settings.json");
        let state = directory.path().join("state");
        fs::write(
            &settings,
            serde_json::to_vec(&json!({
                "theme": "dark",
                "hooks": {"SessionStart": [{"matcher": "tool", "hooks": [{"type": "command", "command": "other-tool"}]}]}
            }))?,
        )?;

        install(&settings, &state)?;
        install(&settings, &state)?;

        let value: Value = serde_json::from_slice(&fs::read(settings)?)?;
        assert_eq!(value["theme"], "dark");
        for event in ["SessionStart", "SessionEnd"] {
            let commands = value["hooks"][event]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
                .filter(|hook| hook["command"].as_str().is_some_and(is_managed_command))
                .count();
            assert_eq!(commands, 1);
            let command = value["hooks"][event]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
                .filter_map(|hook| hook["command"].as_str())
                .find(|command| is_managed_command(command))
                .ok_or("managed hook command missing")?;
            let encoded = command
                .strip_prefix(MANAGED_COMMAND_PREFIX)
                .ok_or("managed hook prefix missing")?;
            assert_eq!(decode_state_dir(encoded)?, state);
        }
        assert_eq!(
            value["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "other-tool"
        );
        Ok(())
    }

    #[test]
    fn plan_validates_existing_settings_without_writing() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let settings = directory.path().join("settings.json");
        fs::write(&settings, r#"{"hooks":[]}"#)?;
        let before = fs::read(&settings)?;
        assert!(prepared_settings(&settings, &directory.path().join("state")).is_err());
        assert_eq!(fs::read(settings)?, before);
        Ok(())
    }

    #[test]
    fn install_does_not_activate_hooks_when_state_initialization_fails(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let settings = directory.path().join("settings.json");
        let state = directory.path().join("not-a-directory");
        fs::write(&settings, r#"{"theme":"dark"}"#)?;
        fs::write(&state, "occupied")?;
        let before = fs::read(&settings)?;
        assert!(install(&settings, &state).is_err());
        assert_eq!(fs::read(settings)?, before);
        Ok(())
    }

    #[test]
    fn concurrent_captures_do_not_lose_events() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let state = directory.path().to_path_buf();
        let first_state = state.clone();
        let first = std::thread::spawn(move || {
            capture(
                &first_state,
                &json!({
                    "hook_event_name": "SessionStart", "session_id": "first",
                    "timestamp": "2026-03-01T10:00:00Z"
                }),
            )
        });
        let second_state = state.clone();
        let second = std::thread::spawn(move || {
            capture(
                &second_state,
                &json!({
                    "hook_event_name": "SessionStart", "session_id": "second",
                    "timestamp": "2026-03-01T10:00:01Z"
                }),
            )
        });
        first.join().map_err(|_| "first capture panicked")??;
        second.join().map_err(|_| "second capture panicked")??;
        let events: Value = serde_json::from_slice(&fs::read(state.join("events.json"))?)?;
        assert_eq!(events.as_object().map(serde_json::Map::len), Some(2));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn replacing_private_settings_preserves_permissions() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir()?;
        let settings = directory.path().join("settings.json");
        fs::write(&settings, "{}")?;
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o600))?;
        install(&settings, &directory.path().join("state"))?;
        assert_eq!(fs::metadata(settings)?.permissions().mode() & 0o777, 0o600);
        Ok(())
    }

    #[test]
    fn capture_minimizes_and_deduplicates_events() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let payload = json!({
            "hook_event_name": "SessionStart",
            "session_id": "secret-session",
            "cwd": "/secret/customer/repository",
            "timestamp": "2026-03-01T10:00:00Z",
            "transcript_path": "/must/not/persist"
        });
        let first = capture(directory.path(), &payload)?;
        let second = capture(directory.path(), &payload)?;
        assert_eq!(first.data["duplicate"], false);
        assert_eq!(second.data["duplicate"], true);
        let body = fs::read_to_string(directory.path().join("events.json"))?;
        assert!(!body.contains("secret-session"));
        assert!(!body.contains("/secret"));
        assert!(!body.contains("transcript"));
        let events: Value = serde_json::from_str(&body)?;
        assert_eq!(events.as_object().map(serde_json::Map::len), Some(1));
        Ok(())
    }

    #[test]
    fn capture_rejects_unexpected_lifecycle_payloads() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let result = capture(
            directory.path(),
            &json!({"hook_event_name": "ToolUse", "session_id": "id"}),
        );
        let Err(error) = result else {
            return Err("unsupported event should fail".into());
        };
        assert_eq!(error.code(), "invalid_input");
        assert!(!directory.path().join("events.json").exists());
        Ok(())
    }
}
