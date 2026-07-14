use std::fs;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn command(config: &std::path::Path) -> Result<Command, Box<dyn std::error::Error>> {
    let mut command = Command::cargo_bin("drag")?;
    command.args([
        "--config",
        config
            .to_str()
            .ok_or("temporary config path is not UTF-8")?,
        "--timezone",
        "Europe/Warsaw",
        "--output",
        "json",
    ]);
    Ok(command)
}

fn configured_file(directory: &TempDir) -> Result<std::path::PathBuf, std::io::Error> {
    let path = directory.path().join("config.json");
    fs::write(
        &path,
        r#"{
          "tempoToken":"tempo-secret",
          "accountId":"account-1",
          "atlassianUserEmail":"person@example.com",
          "atlassianToken":"atlassian-secret",
          "hostname":"example.atlassian.net",
          "aliases":{"dataType":"Map","value":[]},
          "trackers":{"dataType":"Map","value":[]}
        }"#,
    )?;
    Ok(path)
}

#[test]
fn reports_version() -> Result<(), Box<dyn std::error::Error>> {
    Command::cargo_bin("drag")?
        .arg("--version")
        .assert()
        .success();
    Ok(())
}

#[test]
fn alias_commands_preserve_colon_compatibility() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");

    command(&path)?
        .args(["alias:set", "lunch", "ABC-1"])
        .assert()
        .success();
    let output = command(&path)?.args(["alias:list"]).output()?;
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["data"]["aliases"]["lunch"], "ABC-1");

    let persisted = fs::read_to_string(path)?;
    assert!(persisted.contains("\"dataType\": \"Map\""));
    Ok(())
}

#[test]
fn tracker_commands_persist_state() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");

    command(&path)?
        .args(["tracker:start", "ABC-1", "--description", "review"])
        .assert()
        .success();
    let output = command(&path)?.args(["tracker:list"]).output()?;
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["data"]["trackers"][0]["tracker"]["issueKey"], "ABC-1");
    assert_eq!(body["data"]["trackers"][0]["tracker"]["isActive"], true);
    Ok(())
}

#[test]
fn log_dry_run_parses_without_network_access() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let output = command(&path)?
        .args([
            "log",
            "ABC-1",
            "1h15m",
            "2020-02-28",
            "--start",
            "9:30",
            "--dry-run",
        ])
        .output()?;
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["data"]["dryRun"], true);
    assert_eq!(body["data"]["request"]["timeSpentSeconds"], 4_500);
    assert_eq!(body["data"]["request"]["startTime"], "09:30:00");
    assert_eq!(body["data"]["issueKey"], "ABC-1");
    Ok(())
}

#[test]
fn log_accepts_raw_json_from_stdin() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let output = command(&path)?
        .args(["log", "--json", "-", "--dry-run"])
        .write_stdin(
            r#"{"issueKeyOrAlias":"ABC-1","durationOrInterval":"30m","when":"2020-02-28"}"#,
        )
        .output()?;
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["data"]["request"]["timeSpentSeconds"], 1_800);
    Ok(())
}

#[test]
fn original_autocomplete_command_works_without_a_shell_argument(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    command(&path)?.arg("autocomplete").assert().success();
    Ok(())
}

#[test]
fn invalid_duration_is_a_structured_usage_error() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    let output = command(&path)?
        .args(["log", "ABC-1", "nonsense", "--dry-run"])
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "invalid_duration");
    Ok(())
}

#[test]
fn tracker_stop_dry_run_does_not_mutate_the_tracker() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = configured_file(&directory)?;
    command(&path)?
        .args(["tracker:start", "ABC-1"])
        .assert()
        .success();
    let before = fs::read_to_string(&path)?;

    command(&path)?
        .args(["tracker", "stop", "ABC-1", "--dry-run"])
        .assert()
        .success();
    let after = fs::read_to_string(&path)?;
    assert_eq!(after, before);
    Ok(())
}

#[test]
fn schema_documents_safety_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let output = command(&path)?.arg("schema").output()?;
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["data"]["commands"]["log"]["dryRun"], true);
    assert_eq!(body["data"]["schemaVersion"], 1);
    Ok(())
}
