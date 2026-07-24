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
