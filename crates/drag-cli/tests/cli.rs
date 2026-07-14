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
    assert_eq!(body["data"]["commands"]["setup"]["fromEnv"], true);
    assert_eq!(body["data"]["schemaVersion"], 1);
    Ok(())
}

#[test]
fn headless_setup_requires_only_the_four_connection_variables(
) -> Result<(), Box<dyn std::error::Error>> {
    const VARIABLES: [&str; 4] = [
        "ATLASSIAN_HOST",
        "ATLASSIAN_EMAIL",
        "ATLASSIAN_TOKEN",
        "TEMPO_TOKEN",
    ];
    for missing in VARIABLES {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let mut process = command(&path)?;
        process
            .args(["setup", "--from-env"])
            .env("ATLASSIAN_HOST", "example.atlassian.net")
            .env("ATLASSIAN_EMAIL", "person@example.com")
            .env("ATLASSIAN_TOKEN", "jira-token-must-not-leak")
            .env("TEMPO_TOKEN", "tempo-token-must-not-leak")
            .env("TEMPO_ACCOUNT_ID", "legacy-account-must-not-win")
            .env_remove(missing);
        let output = process.output()?;
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let body: Value = serde_json::from_slice(&output.stderr)?;
        assert_eq!(body["error"]["code"], "invalid_input");
        assert!(body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(missing)));
        let stderr = String::from_utf8(output.stderr)?;
        assert!(!stderr.contains("jira-token-must-not-leak"));
        assert!(!stderr.contains("tempo-token-must-not-leak"));
    }
    Ok(())
}

#[test]
fn headless_setup_rejects_unsafe_jira_sites_without_network_access(
) -> Result<(), Box<dyn std::error::Error>> {
    for site in [
        "http://example.atlassian.net",
        "https://user:password@example.atlassian.net",
        "example.atlassian.net/path",
    ] {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let output = command(&path)?
            .args(["setup", "--from-env"])
            .env("ATLASSIAN_HOST", site)
            .env("ATLASSIAN_EMAIL", "person@example.com")
            .env("ATLASSIAN_TOKEN", "jira-token-must-not-leak")
            .env("TEMPO_TOKEN", "tempo-token-must-not-leak")
            .output()?;
        assert_eq!(output.status.code(), Some(2));
        let body: Value = serde_json::from_slice(&output.stderr)?;
        assert_eq!(body["error"]["code"], "invalid_input");
    }
    Ok(())
}

#[test]
fn headless_setup_parses_existing_config_before_reading_credentials(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    fs::write(&path, "{not valid json")?;
    let before = fs::read(&path)?;
    let output = command(&path)?
        .args(["setup", "--from-env"])
        .env_remove("ATLASSIAN_HOST")
        .env_remove("ATLASSIAN_EMAIL")
        .env_remove("ATLASSIAN_TOKEN")
        .env_remove("TEMPO_TOKEN")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "config_error");
    assert_eq!(fs::read(path)?, before);
    Ok(())
}

#[test]
fn interactive_setup_without_a_terminal_points_automation_to_from_env(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let output = command(&path)?.arg("setup").output()?;

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let body: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(body["error"]["code"], "invalid_input");
    assert!(body["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("setup --from-env")));
    assert!(!path.exists());
    Ok(())
}
