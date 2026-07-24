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
