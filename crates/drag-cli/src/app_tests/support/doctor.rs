use super::*;

#[tokio::test]
async fn doctor_without_remote_checks_never_calls_the_verifier(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let (app, calls) = doctor_app(
        path,
        Err(VerificationFailure::Fatal(
            "Jira must not be called".to_owned(),
        )),
        Err(VerificationFailure::Fatal(
            "Tempo must not be called".to_owned(),
        )),
    );

    let result = app.doctor(DoctorArgs { remote: false }).await?;

    assert!(result.failure.is_none());
    assert!(result.data.get("remoteChecks").is_none());
    assert!(result.human.contains("Jira: configured"));
    assert!(result.human.contains("Tempo: configured"));
    assert!(calls
        .lock()
        .map_err(|_| "test verifier lock was poisoned")?
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn doctor_remote_checks_report_both_connected_without_writing_config(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let before = fs::read(&path)?;
    let (app, calls) = doctor_app(path.clone(), Ok("verified-account".to_owned()), Ok(()));

    let result = app.doctor(DoctorArgs { remote: true }).await?;

    assert!(result.failure.is_none());
    assert_eq!(result.data["remoteChecks"]["jira"]["status"], "connected");
    assert_eq!(result.data["remoteChecks"]["tempo"]["status"], "connected");
    assert_eq!(
        calls
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?
            .as_slice(),
        ["jira", "tempo"]
    );
    assert_eq!(fs::read(path)?, before);
    Ok(())
}

#[tokio::test]
async fn doctor_remote_checks_report_tempo_after_jira_failure_without_leaking_secrets(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let (app, calls) = doctor_app(
        path,
        Err(VerificationFailure::Authentication(
            "old-jira-token old-tempo-token Basic-secret".to_owned(),
        )),
        Ok(()),
    );

    let result = app.doctor(DoctorArgs { remote: true }).await?;

    assert_eq!(
        result.failure.as_ref().map(|failure| failure.code),
        Some("remote_check_failed")
    );
    assert_eq!(result.exit_code(), 1);
    assert_eq!(result.data["remoteChecks"]["jira"]["status"], "failed");
    assert_eq!(
        result.data["remoteChecks"]["jira"]["errorCode"],
        "api_error"
    );
    assert_eq!(result.data["remoteChecks"]["tempo"]["status"], "connected");
    assert_eq!(
        calls
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?
            .as_slice(),
        ["jira", "tempo"]
    );
    let output = format!("{} {}", result.human, result.data);
    assert!(!output.contains("old-jira-token"));
    assert!(!output.contains("old-tempo-token"));
    assert!(!output.contains("Basic-secret"));
    Ok(())
}

#[tokio::test]
async fn doctor_remote_checks_report_jira_after_tempo_failure(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let (app, calls) = doctor_app(
        path,
        Ok("verified-account".to_owned()),
        Err(VerificationFailure::Fatal("Tempo unavailable".to_owned())),
    );

    let result = app.doctor(DoctorArgs { remote: true }).await?;

    assert!(result.failure.is_some());
    assert_eq!(result.exit_code(), 1);
    assert_eq!(result.data["remoteChecks"]["jira"]["status"], "connected");
    assert_eq!(result.data["remoteChecks"]["tempo"]["status"], "failed");
    assert_eq!(
        calls
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?
            .as_slice(),
        ["jira", "tempo"]
    );
    Ok(())
}

#[tokio::test]
async fn doctor_remote_checks_report_each_missing_service_without_network_access(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let (app, calls) = doctor_app(
        path,
        Err(VerificationFailure::Fatal(
            "Jira must not be called".to_owned(),
        )),
        Err(VerificationFailure::Fatal(
            "Tempo must not be called".to_owned(),
        )),
    );

    let result = app.doctor(DoctorArgs { remote: true }).await?;

    assert!(result.failure.is_some());
    assert_eq!(result.exit_code(), 2);
    assert_eq!(
        result.data["remoteChecks"]["jira"]["status"],
        "notConfigured"
    );
    assert_eq!(
        result.data["remoteChecks"]["tempo"]["status"],
        "notConfigured"
    );
    assert!(calls
        .lock()
        .map_err(|_| "test verifier lock was poisoned")?
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn doctor_remote_checks_run_a_configured_service_when_the_other_is_missing(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let mut config = existing_config();
    config.hostname = None;
    config.atlassian_user_email = None;
    config.atlassian_token = None;
    config.save(&path)?;
    let (app, calls) = doctor_app(
        path,
        Err(VerificationFailure::Fatal(
            "Jira must not be called".to_owned(),
        )),
        Ok(()),
    );

    let result = app.doctor(DoctorArgs { remote: true }).await?;

    assert_eq!(
        result.data["remoteChecks"]["jira"]["status"],
        "notConfigured"
    );
    assert_eq!(result.data["remoteChecks"]["tempo"]["status"], "connected");
    assert_eq!(
        calls
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?
            .as_slice(),
        ["tempo"]
    );
    Ok(())
}

#[tokio::test]
async fn doctor_remote_checks_reject_malformed_config_before_network_access(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    fs::write(&path, "{not valid json")?;
    let (app, calls) = doctor_app(
        path,
        Err(VerificationFailure::Fatal(
            "Jira must not be called".to_owned(),
        )),
        Err(VerificationFailure::Fatal(
            "Tempo must not be called".to_owned(),
        )),
    );

    let Err(error) = app.doctor(DoctorArgs { remote: true }).await else {
        return Err("malformed config should fail doctor".into());
    };

    assert!(matches!(error, CliError::Config { .. }));
    assert!(calls
        .lock()
        .map_err(|_| "test verifier lock was poisoned")?
        .is_empty());
    Ok(())
}
