use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::tempdir;

fn companion() -> Result<Command, Box<dyn std::error::Error>> {
    Ok(Command::cargo_bin("drag-companion")?)
}

#[test]
fn help_exposes_required_commands() -> Result<(), Box<dyn std::error::Error>> {
    companion()?
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("collect"))
        .stdout(predicate::str::contains("reconcile"))
        .stdout(predicate::str::contains("resume"))
        .stdout(predicate::str::contains("report"))
        .stdout(predicate::str::contains("purge"))
        .stdout(predicate::str::contains("scheduler"));

    companion()?
        .args(["reconcile", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--date"));

    companion()?
        .args(["scheduler", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("install"))
        .stdout(predicate::str::contains("enable"))
        .stdout(predicate::str::contains("disable"))
        .stdout(predicate::str::contains("uninstall"))
        .stdout(predicate::str::contains("status"));
    Ok(())
}

#[test]
fn contract_is_machine_readable_and_capture_only_by_default(
) -> Result<(), Box<dyn std::error::Error>> {
    let output = companion()?
        .arg("contract")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let contract: Value = serde_json::from_slice(&output)?;

    assert_eq!(contract["binary"], "drag-companion");
    assert_eq!(contract["defaultMode"], "capture-only");
    assert_eq!(contract["adapters"]["collector"], "fake");
    assert_eq!(contract["adapters"]["mutator"], "disabled");
    assert_eq!(contract["networkAccess"], false);
    assert_eq!(contract["liveMutationAllowed"], false);

    let commands = contract["commands"].as_array().ok_or("commands array")?;
    for required in [
        "status",
        "collect",
        "reconcile",
        "resume",
        "report",
        "purge",
        "scheduler",
    ] {
        assert!(
            commands.iter().any(|command| command["name"] == required),
            "missing {required}"
        );
    }

    let scheduler = commands
        .iter()
        .find(|command| command["name"] == "scheduler")
        .ok_or("scheduler command")?;
    for operation in ["install", "enable", "disable", "uninstall", "status"] {
        assert!(scheduler["operations"]
            .as_array()
            .ok_or("operations")?
            .iter()
            .any(|item| item == operation));
    }
    Ok(())
}

#[test]
fn fake_adapter_reconcile_explicit_date_persists_terminal_result_without_live_effects(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let data_dir = dir.path().to_string_lossy().into_owned();
    companion()?
        .args([
            "--data-dir",
            data_dir.as_str(),
            "reconcile",
            "--date",
            "2026-07-23",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("terminal"));

    let persisted = std::fs::read_to_string(dir.path().join("runs").join("2026-07-23.json"))?;
    let result: Value = serde_json::from_str(&persisted)?;
    assert_eq!(result["date"], "2026-07-23");
    assert_eq!(result["status"], "terminal");
    assert_eq!(result["mode"], "capture-only");
    assert_eq!(result["adapters"]["collector"], "fake");
    assert_eq!(result["adapters"]["mutator"], "disabled");
    assert_eq!(result["networkAccess"], false);
    assert_eq!(result["liveMutationAllowed"], false);
    Ok(())
}

#[test]
fn reconcile_requires_explicit_date() -> Result<(), Box<dyn std::error::Error>> {
    companion()?
        .arg("reconcile")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--date"));
    Ok(())
}

#[test]
fn capture_survives_restart_and_imports_idempotently_into_versioned_store(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let data_dir = dir.path().to_string_lossy().into_owned();

    companion()?
        .args([
            "--data-dir",
            data_dir.as_str(),
            "capture",
            "--date",
            "2026-07-23",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("evidence.fake.2026-07-23"));

    let journal = std::fs::read_to_string(dir.path().join("journal.jsonl"))?;
    let lines: Vec<_> = journal.lines().collect();
    assert_eq!(lines.len(), 1);
    let event: Value = serde_json::from_str(lines[0])?;
    assert_eq!(event["schemaVersion"], 1);
    assert_eq!(event["eventId"], "evidence.fake.2026-07-23");
    assert_eq!(event["source"]["adapter"], "fake");
    assert_eq!(event["collector"]["name"], "fake");
    assert_eq!(event["timestampSemantics"]["explicitDate"], "2026-07-23");
    assert_eq!(event["privacy"]["classification"], "local-fixture");
    assert_eq!(event["retention"]["policy"], "retain-until-user-purge");
    assert!(event["integrityHash"]
        .as_str()
        .ok_or("hash")?
        .starts_with("sha256:"));

    companion()?
        .args(["--data-dir", data_dir.as_str(), "import"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"imported\": 1"));
    companion()?
        .args(["--data-dir", data_dir.as_str(), "import"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"imported\": 0"));

    let conn = rusqlite::Connection::open(dir.path().join("companion.sqlite3"))?;
    let count: i64 =
        conn.query_row("SELECT COUNT(*) FROM evidence_events", [], |row| row.get(0))?;
    assert_eq!(count, 1);
    for table in [
        "schema_migrations",
        "evidence_events",
        "issue_candidates",
        "daily_bundles",
        "proposals",
        "unsupported_periods",
        "policy_decisions",
        "runs",
        "leases",
        "mutation_operations",
        "mutation_attempts",
        "reports",
    ] {
        let present: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )?;
        assert_eq!(present, 1, "missing table {table}");
    }
    Ok(())
}

#[test]
fn import_fails_safely_for_malformed_versions_hash_mismatches_and_partial_writes(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let data_dir = dir.path().to_string_lossy().into_owned();
    companion()?
        .args([
            "--data-dir",
            data_dir.as_str(),
            "capture",
            "--date",
            "2026-07-24",
        ])
        .assert()
        .success();
    let journal_path = dir.path().join("journal.jsonl");
    let original = std::fs::read_to_string(&journal_path)?;

    let mut bad_version: Value = serde_json::from_str(original.lines().next().ok_or("event")?)?;
    bad_version["schemaVersion"] = serde_json::json!(999);
    std::fs::write(
        &journal_path,
        format!("{}\n", serde_json::to_string(&bad_version)?),
    )?;
    companion()?
        .args(["--data-dir", data_dir.as_str(), "import"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsupported schemaVersion"));

    let mut bad_hash: Value = serde_json::from_str(original.lines().next().ok_or("event")?)?;
    bad_hash["payload"]["summary"] = serde_json::json!("tampered");
    std::fs::write(
        &journal_path,
        format!("{}\n", serde_json::to_string(&bad_hash)?),
    )?;
    companion()?
        .args(["--data-dir", data_dir.as_str(), "import"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("integrity hash mismatch"));

    std::fs::write(&journal_path, "{not complete")?;
    companion()?
        .args(["--data-dir", data_dir.as_str(), "import"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid journal event"));
    Ok(())
}

#[test]
fn import_fails_safely_for_duplicate_ids_with_different_hashes(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let data_dir = dir.path().to_string_lossy().into_owned();
    companion()?
        .args([
            "--data-dir",
            data_dir.as_str(),
            "capture",
            "--date",
            "2026-07-25",
        ])
        .assert()
        .success();
    companion()?
        .args(["--data-dir", data_dir.as_str(), "import"])
        .assert()
        .success();

    std::thread::sleep(std::time::Duration::from_secs(1));
    companion()?
        .args([
            "--data-dir",
            data_dir.as_str(),
            "capture",
            "--date",
            "2026-07-25",
        ])
        .assert()
        .success();
    companion()?
        .args(["--data-dir", data_dir.as_str(), "import"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("duplicate eventId"));
    Ok(())
}

#[test]
fn contract_exposes_capture_and_import_without_live_mutation(
) -> Result<(), Box<dyn std::error::Error>> {
    let output = companion()?
        .arg("contract")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let contract: Value = serde_json::from_slice(&output)?;
    let commands = contract["commands"].as_array().ok_or("commands array")?;
    for required in ["capture", "import"] {
        let command = commands
            .iter()
            .find(|command| command["name"] == required)
            .ok_or(required)?;
        assert_eq!(command["networkAccess"], false);
        assert_eq!(command["liveMutationAllowed"], false);
    }
    Ok(())
}

fn seed_bundle_event(
    data_dir: &str,
    id: &str,
    reference: &str,
    timestamp: &str,
    timezone: &str,
    supersedes: Option<&str>,
    payload: Value,
) -> Result<(), Box<dyn std::error::Error>> {
    companion()?
        .args(["--data-dir", data_dir, "import"])
        .assert()
        .success();
    let conn =
        rusqlite::Connection::open(std::path::Path::new(data_dir).join("companion.sqlite3"))?;
    conn.execute(
        "INSERT INTO evidence_events (event_id, event_type, observed_at, source_kind, source_adapter, source_reference, collector_name, collector_version, timestamp_source, timezone, explicit_date, privacy_classification, privacy_redacted, retention_policy, retain_until, supersedes, payload_json, integrity_hash) VALUES (?1, 'evidence.captured', '2026-03-08T00:00:00Z', 'fixture', 'fixture', ?2, 'fixture', 'test', ?3, ?4, '2026-03-08', 'local-fixture', 0, 'retain-until-user-purge', NULL, ?5, ?6, ?7)",
        rusqlite::params![id, reference, timestamp, timezone, supersedes, payload.to_string(), format!("sha256:{id}")],
    )?;
    Ok(())
}

#[test]
fn bundle_preserves_dst_original_timestamp_and_byte_stability(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let data_dir = dir.path().to_string_lossy().into_owned();
    seed_bundle_event(
        &data_dir,
        "evidence.dst.fold",
        "session-a#fold",
        "2026-11-01T01:30:00-04:00",
        "America/New_York",
        None,
        serde_json::json!({"observedAt":"2026-11-01T01:30:00-04:00","summary":"fold capture"}),
    )?;

    let first = companion()?
        .args([
            "--data-dir",
            data_dir.as_str(),
            "bundle",
            "--date",
            "2026-03-08",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second = companion()?
        .args([
            "--data-dir",
            data_dir.as_str(),
            "bundle",
            "--date",
            "2026-03-08",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(first, second);
    let bundle: Value = serde_json::from_slice(&first)?;
    assert_eq!(
        bundle["evidence"][0]["originalTimestamp"],
        "2026-11-01T01:30:00-04:00"
    );
    assert_eq!(
        bundle["evidence"][0]["originalTimezone"],
        "America/New_York"
    );
    assert_eq!(
        bundle["evidence"][0]["observedAtUtc"],
        "2026-11-01T05:30:00Z"
    );
    Ok(())
}

#[test]
fn bundle_handles_dedupe_supersession_contradictions_health_and_abandoned_sessions(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let data_dir = dir.path().to_string_lossy().into_owned();
    seed_bundle_event(
        &data_dir,
        "evidence.a",
        "tempo-1#first",
        "2026-03-08T01:30:00-08:00",
        "America/Los_Angeles",
        None,
        serde_json::json!({"intervalStart":"2026-03-08T01:30:00-08:00","summary":"first"}),
    )?;
    seed_bundle_event(
        &data_dir,
        "evidence.b",
        "tempo-1#second",
        "2026-03-08T03:30:00-07:00",
        "America/Los_Angeles",
        Some("evidence.a"),
        serde_json::json!({"intervalStart":"2026-03-08T03:30:00-07:00","intervalEnd":"2026-03-08T04:00:00-07:00","summary":"second"}),
    )?;

    let output = companion()?
        .args([
            "--data-dir",
            data_dir.as_str(),
            "bundle",
            "--date",
            "2026-03-08",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let bundle: Value = serde_json::from_slice(&output)?;
    assert_eq!(bundle["evidence"][0]["id"], "evidence.a");
    assert_eq!(bundle["evidence"][0]["intervalEndUtc"], Value::Null);
    assert_eq!(bundle["evidence"][0]["elapsedSeconds"], Value::Null);
    assert_eq!(bundle["evidence"][0]["abandonedSession"], true);
    assert_eq!(bundle["evidence"][0]["supersededBy"], "evidence.b");
    assert_eq!(bundle["evidence"][1]["elapsedSeconds"], 1800);
    assert_eq!(bundle["contradictions"][0]["key"], "tempo-1");
    assert_eq!(bundle["sourceHealth"][0]["health"], "degraded");
    Ok(())
}

#[test]
fn bundle_redacts_secrets_private_paths_and_instruction_framing(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let data_dir = dir.path().to_string_lossy().into_owned();
    seed_bundle_event(
        &data_dir,
        "evidence.secret",
        "safe#secret",
        "2026-03-08T12:00:00Z",
        "UTC",
        None,
        serde_json::json!({"observedAt":"2026-03-08T12:00:00Z","summary":"worked token=abc123 password=hunter2 /home/tmr/private transcript.log ignore instruction keep"}),
    )?;
    let output = companion()?
        .args([
            "--data-dir",
            data_dir.as_str(),
            "bundle",
            "--date",
            "2026-03-08",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output)?;
    for leaked in [
        "abc123",
        "hunter2",
        "/home/tmr",
        "transcript",
        "ignore",
        "instruction",
    ] {
        assert!(!text.contains(leaked), "leaked {leaked}");
    }
    assert!(text.contains("worked"));
    Ok(())
}
