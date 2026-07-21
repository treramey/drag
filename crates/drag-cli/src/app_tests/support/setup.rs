#[cfg(unix)]
use super::terminal::{
    assert_terminal_restored, expect_terminal_restoration, pty_output_path, read_pty_json_output,
    send_paste, spawn_setup_pty,
};
use super::*;

#[test]
fn normalizes_bare_hosts_and_https_jira_urls() -> Result<(), Box<dyn std::error::Error>> {
    for (input, expected) in [
        ("EXAMPLE.atlassian.net", "example.atlassian.net"),
        (
            "https://Example.atlassian.net/jira/software/projects/ABC?view=all#top",
            "example.atlassian.net",
        ),
    ] {
        assert_eq!(normalize_jira_site(input)?, expected);
    }
    Ok(())
}

#[test]
fn rejects_unsafe_jira_sites() {
    for input in [
        "",
        "http://example.atlassian.net",
        "https://user:password@example.atlassian.net",
        "https://example.atlassian.net:8443",
        "example.atlassian.net/path",
        "https://127.0.0.1",
        "bad host.atlassian.net",
    ] {
        assert!(normalize_jira_site(input).is_err(), "{input:?}");
    }
}

#[cfg(unix)]
#[test]
fn pty_first_run_hides_tokens_and_emits_json_success() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let mut session = spawn_setup_pty(&path, "success")?;

    session.expect("Jira site")?;
    send_paste(&mut session, "https://Example.atlassian.net/jira/software")?;
    session.send("\t")?;
    session.expect("Atlassian email")?;
    send_paste(&mut session, "person@example.com")?;
    session.send("\t\r")?;
    session.expect("Atlassian API token")?;
    send_paste(&mut session, "pty-jira-secret")?;
    session.send("\t\r")?;
    let jira_output = session.expect("Tempo API token")?;
    assert!(!String::from_utf8_lossy(jira_output.before()).contains("pty-jira-secret"));
    send_paste(&mut session, "pty-tempo-secret")?;
    session.send("\t\r")?;
    let tempo_output = session.expect("Save configuration")?;
    assert!(!String::from_utf8_lossy(tempo_output.before()).contains("pty-tempo-secret"));
    session.send("\r")?;
    expect_terminal_restoration(&mut session)?;
    session.expect(Eof)?;

    let body = read_pty_json_output(&path)?;
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["source"], "interactive");

    let saved = Config::load(&path)?;
    assert_eq!(saved.hostname.as_deref(), Some("example.atlassian.net"));
    assert_eq!(saved.account_id.as_deref(), Some("pty-account"));
    assert_eq!(saved.atlassian_token.as_deref(), Some("pty-jira-secret"));
    assert_eq!(saved.tempo_token.as_deref(), Some("pty-tempo-secret"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn pty_authentication_retries_reuse_latest_jira_values_and_retry_only_tempo_token(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let mut session = spawn_setup_pty(&path, "retry")?;

    session.expect("Jira site")?;
    send_paste(&mut session, "example.atlassian.net")?;
    session.send("\t")?;
    send_paste(&mut session, "person@example.com")?;
    session.send("\t\r")?;
    session.expect("Atlassian API token")?;
    send_paste(&mut session, "bad-jira-token")?;
    session.send("\t\r")?;
    session.expect("Could not connect to Jira")?;
    send_paste(&mut session, "good-jira-token")?;
    session.send("\t\r")?;
    session.expect("Tempo API token")?;
    send_paste(&mut session, "bad-tempo-token")?;
    session.send("\t\r")?;
    session.expect("Could not connect to Tempo")?;
    send_paste(&mut session, "good-tempo-token")?;
    session.send("\t\r")?;
    session.expect("Save configuration")?;
    session.send("\r")?;
    expect_terminal_restoration(&mut session)?;
    session.expect(Eof)?;

    let saved = Config::load(&path)?;
    assert_eq!(saved.hostname.as_deref(), Some("example.atlassian.net"));
    assert_eq!(
        saved.atlassian_user_email.as_deref(),
        Some("person@example.com")
    );
    assert_eq!(saved.atlassian_token.as_deref(), Some("good-jira-token"));
    assert_eq!(saved.tempo_token.as_deref(), Some("good-tempo-token"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn pty_reconfiguration_offers_defaults_and_retains_tokens() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let mut session = spawn_setup_pty(&path, "reconfigure")?;

    session.expect("old.atlassian.net")?;
    session.send("\t\t\r")?;
    session.expect("Atlassian API token")?;
    session.send("\t\r")?;
    session.expect("Tempo API token")?;
    session.send("\t\r")?;
    session.expect("Save configuration")?;
    session.send("\r")?;
    expect_terminal_restoration(&mut session)?;
    session.expect(Eof)?;

    let saved = Config::load(&path)?;
    assert_eq!(saved.atlassian_token.as_deref(), Some("old-jira-token"));
    assert_eq!(saved.tempo_token.as_deref(), Some("old-tempo-token"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn pty_late_interrupt_leaves_existing_config_unchanged() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let before = fs::read(&path)?;
    let mut session = spawn_setup_pty(&path, "late-cancel")?;

    session.expect("old.atlassian.net")?;
    session.send("\t\t\r")?;
    session.expect("Atlassian API token")?;
    session.send("\t\r")?;
    session.expect("Tempo API token")?;
    session.send("\t\r")?;
    session.expect("Save configuration")?;
    session.send(ControlCode::EndOfText)?;
    let cancelled = session.expect("interactive setup was cancelled")?;
    assert_terminal_restored(cancelled.before());
    session.expect(Eof)?;

    assert_eq!(fs::read(path)?, before);
    Ok(())
}

#[cfg(unix)]
#[test]
fn pty_resize_message_preserves_entered_state_and_allows_cancellation(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let mut session = spawn_setup_pty(&path, "resize")?;

    session
        .expect("Jira site")
        .map_err(|error| format!("waiting for initial Jira details stage: {error}"))?;
    send_paste(&mut session, "example.atlassian.net")?;
    session
        .expect("example.atlassian.net")
        .map_err(|error| format!("waiting for pasted Jira host: {error}"))?;
    session.get_process_mut().set_window_size(50, 10)?;
    session
        .expect("Terminal too small")
        .map_err(|error| format!("waiting for undersized message: {error}"))?;
    send_paste(&mut session, "hidden-input-must-be-ignored")?;
    session.send("\t\t\t\r")?;
    std::thread::sleep(Duration::from_millis(100));
    session.get_process_mut().set_window_size(100, 30)?;
    session
        .expect("Connect your Jira account")
        .map_err(|error| format!("waiting for restored Jira stage: {error}"))?;
    session
        .expect("example.atlassian.net")
        .map_err(|error| format!("waiting for preserved Jira host: {error}"))?;
    session.send(ControlCode::EndOfText)?;
    let cancelled = session
        .expect("interactive setup was cancelled")
        .map_err(|error| format!("waiting for cancellation result: {error}"))?;
    assert_terminal_restored(cancelled.before());
    session.expect(Eof)?;

    assert!(!path.exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn pty_panic_restores_terminal_state() -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let mut session = spawn_setup_pty(&path, "ratatui-panic")?;

    session.expect("Jira site")?;
    send_paste(&mut session, "example.atlassian.net")?;
    session.send("\t")?;
    send_paste(&mut session, "person@example.com")?;
    session.send("\t\r")?;
    session.expect("Atlassian API token")?;
    send_paste(&mut session, "panic-jira-token")?;
    session.send("\t\r")?;
    let panicked = expect_terminal_restoration(&mut session)?;
    session.expect(Eof)?;

    assert!(!String::from_utf8_lossy(&panicked).contains("panic-jira-token"));
    let stdout = fs::read_to_string(pty_output_path(&path))?;
    assert!(stdout.contains("test app::tests::support::terminal::pty_setup_helper ... ok"));
    assert!(!stdout.contains("FAILED"));
    assert!(!path.exists());
    Ok(())
}

#[tokio::test]
async fn high_level_onboarding_session_drives_verification_and_transactional_save(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let initial = existing_config();
    initial.save(&path)?;
    let events = Arc::new(Mutex::new(Vec::new()));
    let tempo_accounts = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_onboarding_session(
        path.clone(),
        FakeVerifier {
            jira_error: None,
            tempo_error: None,
            tempo_accounts: Arc::clone(&tempo_accounts),
            config_update: None,
        },
        ScriptedOnboardingSession {
            events: Arc::clone(&events),
        },
    );

    app.setup(SetupArgs {
        from_env: false,
        no_open: true,
        dry_run: false,
        verify: false,
    })
    .await?;

    let saved = Config::load(&path)?;
    let observed = (
        saved.hostname.as_deref(),
        saved.atlassian_user_email.as_deref(),
        saved.atlassian_token.as_deref(),
        saved.tempo_token.as_deref(),
        saved.account_id.as_deref(),
    );
    assert_eq!(
        observed,
        (
            Some("example.atlassian.net"),
            Some("scripted@example.com"),
            Some("scripted-jira-token"),
            Some("scripted-tempo-token"),
            Some("derived-account"),
        )
    );
    assert_eq!(
        tempo_accounts
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?
            .as_slice(),
        ["derived-account"]
    );
    assert_eq!(
        events
            .lock()
            .map_err(|_| "test session lock was poisoned")?
            .as_slice(),
        ["jira-browser:false", "tempo-browser:false", "save"]
    );
    Ok(())
}

#[tokio::test]
async fn ratatui_first_run_masks_secrets_verifies_and_saves_from_scripted_events(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let prompt_state = Arc::new(Mutex::new(PromptState {
        browser_failure: Some("no default browser".to_owned()),
        ..PromptState::default()
    }));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_onboarding_session(
        path.clone(),
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
            tempo_results: Mutex::new(VecDeque::from([Ok(())])),
        },
        RatatuiOnboardingSession::scripted(
            FakeBrowserLauncher {
                state: Arc::clone(&prompt_state),
            },
            first_run_tui_events(true),
            Arc::clone(&frames),
        ),
    );

    let result = app
        .setup(SetupArgs {
            from_env: false,
            no_open: false,
            dry_run: false,
            verify: false,
        })
        .await?;

    let saved = Config::load(&path)?;
    assert_eq!(
        (
            saved.hostname.as_deref(),
            saved.atlassian_user_email.as_deref(),
            saved.atlassian_token.as_deref(),
            saved.tempo_token.as_deref(),
            saved.account_id.as_deref(),
        ),
        (
            Some("example.atlassian.net"),
            Some("person@example.com"),
            Some("scripted-jira-secret"),
            Some("scripted-tempo-secret"),
            Some("derived-account"),
        )
    );
    assert_eq!(result.data["source"], "interactive");

    let captured_frames = frames.lock().map_err(|_| "test frame lock poisoned")?;
    assert!(captured_frames
        .iter()
        .any(|frame| frame.contains("Warning: Could not open")));
    assert!(!captured_frames
        .last()
        .ok_or("Ratatui did not render a Save frame")?
        .contains("Warning:"));
    let frames = captured_frames.join("\n--- frame ---\n");
    for visible in [
        "Connect Jira",
        "Connect Tempo",
        "Save",
        "Verifying Connect Jira",
        "Verifying Connect Tempo",
        "example.atlassian.net",
        "person@example.com",
        ATLASSIAN_TOKEN_URL,
        "api-integration",
        "Ready to save",
        "Workspace",
        "Edit Jira account",
        "Edit Tempo token",
    ] {
        assert!(frames.contains(visible), "missing rendered text: {visible}");
    }
    for secret in [
        "scripted-jira-secret",
        "scripted-tempo-secret",
        "derived-account",
    ] {
        assert!(!frames.contains(secret), "rendered secret: {secret}");
    }

    let prompt_state = prompt_state
        .lock()
        .map_err(|_| "test browser lock poisoned")?;
    assert_eq!(
    prompt_state.browser_urls,
    [
        ATLASSIAN_TOKEN_URL,
        "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration",
    ]
);
    Ok(())
}

#[tokio::test]
async fn ratatui_opens_atlassian_only_after_explicit_token_stage_entry(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let initial = existing_config();
    initial.save(&path)?;
    let before = fs::read(&path)?;
    let browser_state = Arc::new(Mutex::new(PromptState::default()));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_onboarding_session(
        path.clone(),
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::new()),
            tempo_results: Mutex::new(VecDeque::new()),
        },
        RatatuiOnboardingSession::scripted(
            FakeBrowserLauncher {
                state: Arc::clone(&browser_state),
            },
            vec![
                Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
                Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
                Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            ],
            Arc::clone(&frames),
        ),
    );

    let error = app
        .setup(SetupArgs {
            from_env: false,
            no_open: false,
            dry_run: false,
            verify: false,
        })
        .await
        .err()
        .ok_or("token-stage checkpoint unexpectedly completed setup")?;

    assert!(error.to_string().contains("cancelled"));
    assert_eq!(fs::read(path)?, before);
    assert_eq!(
        browser_state
            .lock()
            .map_err(|_| "test browser lock poisoned")?
            .browser_urls,
        [ATLASSIAN_TOKEN_URL]
    );
    let frames = frames.lock().map_err(|_| "test frame lock poisoned")?;
    assert!(frames.first().is_some_and(|frame| {
        frame.contains("Jira site")
            && frame.contains("Atlassian email")
            && frame.contains("Continue to API token")
            && !frame.contains(ATLASSIAN_TOKEN_URL)
    }));
    assert!(frames
        .iter()
        .any(|frame| frame.contains("Connect Jira") && frame.contains("••••")));
    Ok(())
}

#[tokio::test]
async fn ratatui_back_from_jira_token_discards_only_the_unverified_buffer(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let before = fs::read(&path)?;
    let browser_state = Arc::new(Mutex::new(PromptState::default()));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let events = vec![
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("unverified-jira-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    ];
    let app = App::with_onboarding_session(
        path.clone(),
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::new()),
            tempo_results: Mutex::new(VecDeque::new()),
        },
        RatatuiOnboardingSession::scripted(
            FakeBrowserLauncher {
                state: Arc::clone(&browser_state),
            },
            events,
            Arc::clone(&frames),
        ),
    );

    let error = app
        .setup(SetupArgs {
            from_env: false,
            no_open: false,
            dry_run: false,
            verify: false,
        })
        .await
        .err()
        .ok_or("unverified Jira token buffer unexpectedly completed setup")?;

    assert!(error.to_string().contains("cancelled"));
    assert_eq!(fs::read(path)?, before);
    assert_eq!(
        browser_state
            .lock()
            .map_err(|_| "test browser lock poisoned")?
            .browser_urls,
        [ATLASSIAN_TOKEN_URL]
    );
    assert!(frames
        .lock()
        .map_err(|_| "test frame lock poisoned")?
        .last()
        .is_some_and(|frame| {
            frame.contains("••••") && !frame.contains("unverified-jira-token")
        }));
    Ok(())
}

#[tokio::test]
async fn ratatui_validation_and_authentication_retries_stay_in_the_failed_stage(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let browser_state = Arc::new(Mutex::new(PromptState::default()));
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let events = vec![
        // Reject an invalid site and an empty Jira form before any verification call.
        Event::Paste("/".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("example.atlassian.net".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("person@example.com".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        // Reject an empty replacement before retrying Jira authentication.
        Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("rejected-jira-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("replacement-jira-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        // Tempo also validates locally and retries in place.
        Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("rejected-tempo-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("replacement-tempo-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ];
    let app = App::with_onboarding_session(
        path.clone(),
        RecordingSequenceVerifier {
            jira_results: Mutex::new(VecDeque::from([
                Err(VerificationFailure::Authentication(
                    "Jira credentials rejected".to_owned(),
                )),
                Ok("derived-account".to_owned()),
            ])),
            tempo_results: Mutex::new(VecDeque::from([
                Err(VerificationFailure::Authentication(
                    "Tempo token rejected".to_owned(),
                )),
                Ok(()),
            ])),
            attempts: Arc::clone(&attempts),
        },
        RatatuiOnboardingSession::scripted(
            FakeBrowserLauncher {
                state: Arc::clone(&browser_state),
            },
            events,
            Arc::clone(&frames),
        ),
    );

    app.setup(SetupArgs {
        from_env: false,
        no_open: false,
        dry_run: false,
        verify: false,
    })
    .await?;

    let attempts = attempts
        .lock()
        .map_err(|_| "test verifier lock was poisoned")?;
    assert_eq!(attempts.len(), 4);
    assert!(matches!(
        &attempts[0],
        RecordedVerification::Jira { hostname, email, token }
            if hostname == "example.atlassian.net"
                && email == "person@example.com"
                && token == "rejected-jira-token"
    ));
    assert!(matches!(
        &attempts[1],
        RecordedVerification::Jira { hostname, email, token }
            if hostname == "example.atlassian.net"
                && email == "person@example.com"
                && token == "replacement-jira-token"
    ));
    assert!(matches!(
        &attempts[2],
        RecordedVerification::Tempo { account_id, token }
            if account_id == "derived-account" && token == "rejected-tempo-token"
    ));
    assert!(matches!(
        &attempts[3],
        RecordedVerification::Tempo { account_id, token }
            if account_id == "derived-account" && token == "replacement-tempo-token"
    ));
    drop(attempts);
    assert_eq!(
        browser_state
            .lock()
            .map_err(|_| "test browser lock poisoned")?
            .browser_urls
            .len(),
        2
    );
    let saved = Config::load(&path)?;
    assert_eq!(saved.hostname.as_deref(), Some("example.atlassian.net"));
    assert_eq!(
        saved.atlassian_user_email.as_deref(),
        Some("person@example.com")
    );
    assert_eq!(
        saved.atlassian_token.as_deref(),
        Some("replacement-jira-token")
    );
    assert_eq!(
        saved.tempo_token.as_deref(),
        Some("replacement-tempo-token")
    );

    let captured_frames = frames.lock().map_err(|_| "test frame lock poisoned")?;
    for message in [
        "Invalid Jira site",
        "Jira site is required",
        "Atlassian email is required",
        "Atlassian API token is required",
        "Could not connect to Jira",
        "Tempo API token is required",
        "Could not connect to Tempo",
    ] {
        assert!(
            captured_frames.iter().any(|frame| frame.contains(message)),
            "missing recovery message: {message}"
        );
    }
    assert!(captured_frames.iter().any(|frame| {
        frame.contains("Could not connect to Tempo") && frame.contains("✓ Jira account")
    }));
    let site_error = captured_frames
        .iter()
        .position(|frame| frame.contains("Jira site is required"))
        .ok_or("missing Jira site validation frame")?;
    assert!(!captured_frames
        .get(site_error + 1)
        .ok_or("missing frame after Jira site correction")?
        .contains("Jira site is required"));
    for secret in [
        "rejected-jira-token",
        "rejected-tempo-token",
        "derived-account",
    ] {
        assert!(!captured_frames.iter().any(|frame| frame.contains(secret)));
    }
    Ok(())
}

#[tokio::test]
async fn ratatui_no_open_keeps_both_links_visible_without_browser_calls(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let browser_state = Arc::new(Mutex::new(PromptState::default()));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_onboarding_session(
        path,
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
            tempo_results: Mutex::new(VecDeque::from([Ok(())])),
        },
        RatatuiOnboardingSession::scripted(
            FakeBrowserLauncher {
                state: Arc::clone(&browser_state),
            },
            first_run_tui_events(true),
            Arc::clone(&frames),
        ),
    );

    app.setup(SetupArgs {
        from_env: false,
        no_open: true,
        dry_run: false,
        verify: false,
    })
    .await?;

    assert!(browser_state
        .lock()
        .map_err(|_| "test browser lock poisoned")?
        .browser_urls
        .is_empty());
    let rendered = frames
        .lock()
        .map_err(|_| "test frame lock poisoned")?
        .join("\n");
    assert!(rendered.contains(ATLASSIAN_TOKEN_URL));
    assert!(rendered.contains("api-integration"));
    Ok(())
}

#[tokio::test]
async fn ratatui_whitespace_does_not_silently_retain_stored_tokens(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let frames = Arc::new(Mutex::new(Vec::new()));
    let events = vec![
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste(" ".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("replacement-jira-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste(" ".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("replacement-tempo-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ];
    let app = App::with_onboarding_session(
        path.clone(),
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::from([Ok("replacement-account".to_owned())])),
            tempo_results: Mutex::new(VecDeque::from([Ok(())])),
        },
        RatatuiOnboardingSession::scripted(NoopBrowserLauncher, events, Arc::clone(&frames)),
    );

    app.setup(SetupArgs {
        from_env: false,
        no_open: true,
        dry_run: false,
        verify: false,
    })
    .await?;

    let saved = Config::load(&path)?;
    assert_eq!(
        saved.atlassian_token.as_deref(),
        Some("replacement-jira-token")
    );
    assert_eq!(
        saved.tempo_token.as_deref(),
        Some("replacement-tempo-token")
    );
    let rendered = frames
        .lock()
        .map_err(|_| "test frame lock poisoned")?
        .join("\n");
    assert!(rendered.contains("Could not connect to Jira: token is required"));
    assert!(rendered.contains("Could not connect to Tempo: token is required"));
    Ok(())
}

#[tokio::test]
async fn ratatui_fatal_verification_failure_propagates_without_rendering_secrets(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let frames = Arc::new(Mutex::new(Vec::new()));
    let events = first_run_tui_events(true);
    let app = App::with_onboarding_session(
        path.clone(),
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::from([Err(VerificationFailure::Fatal(
                "network timeout".to_owned(),
            ))])),
            tempo_results: Mutex::new(VecDeque::new()),
        },
        RatatuiOnboardingSession::scripted(NoopBrowserLauncher, events, Arc::clone(&frames)),
    );

    let error = app
        .setup(SetupArgs {
            from_env: false,
            no_open: true,
            dry_run: false,
            verify: false,
        })
        .await
        .err()
        .ok_or("fatal Jira verification unexpectedly became recoverable")?;

    assert!(matches!(error, CliError::Api(message) if message == "network timeout"));
    assert!(!path.exists());
    let rendered = frames
        .lock()
        .map_err(|_| "test frame lock poisoned")?
        .join("\n");
    assert!(rendered.contains("Verifying Connect Jira"));
    assert!(!rendered.contains("scripted-jira-secret"));
    assert!(!rendered.contains("derived-account"));
    assert!(!rendered.contains("Could not connect to Jira"));
    Ok(())
}

#[tokio::test]
async fn ratatui_first_run_does_not_write_before_explicit_save(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let app = App::with_onboarding_session(
        path.clone(),
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
            tempo_results: Mutex::new(VecDeque::from([Ok(())])),
        },
        RatatuiOnboardingSession::scripted(
            NoopBrowserLauncher,
            first_run_tui_events(false),
            Arc::new(Mutex::new(Vec::new())),
        ),
    );

    let error = app
        .setup(SetupArgs {
            from_env: false,
            no_open: true,
            dry_run: false,
            verify: false,
        })
        .await
        .err()
        .ok_or("setup unexpectedly saved without the Save action")?;

    assert!(error.to_string().contains("cancelled"));
    assert!(!path.exists());
    Ok(())
}

#[tokio::test]
async fn ratatui_reconfiguration_retains_replaces_backtracks_and_reverifies(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let browser_state = Arc::new(Mutex::new(PromptState::default()));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_onboarding_session(
        path.clone(),
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::from([
                Ok("initial-derived-account".to_owned()),
                Ok("final-derived-account".to_owned()),
            ])),
            tempo_results: Mutex::new(VecDeque::from([Ok(()), Ok(()), Ok(())])),
        },
        RatatuiOnboardingSession::scripted(
            FakeBrowserLauncher {
                state: Arc::clone(&browser_state),
            },
            reconfiguration_tui_events(),
            Arc::clone(&frames),
        ),
    );

    let result = app
        .setup(SetupArgs {
            from_env: false,
            no_open: false,
            dry_run: false,
            verify: false,
        })
        .await?;

    let saved = Config::load(&path)?;
    assert_eq!(
        (
            saved.hostname.as_deref(),
            saved.atlassian_user_email.as_deref(),
            saved.atlassian_token.as_deref(),
            saved.tempo_token.as_deref(),
            saved.account_id.as_deref(),
        ),
        (
            Some("old.atlassian.net.updated"),
            Some("old@example.com.updated"),
            Some("replacement-jira-token"),
            Some("replacement-tempo-token"),
            Some("final-derived-account"),
        )
    );

    let captured_frames = frames.lock().map_err(|_| "test frame lock poisoned")?;
    assert!(captured_frames.first().is_some_and(|frame| {
        frame.contains("old.atlassian.net")
            && frame.contains("old@example.com")
            && frame.contains("Esc")
            && frame.contains("cancel")
            && !frame.contains(ATLASSIAN_TOKEN_URL)
    }));
    assert!(captured_frames.iter().any(|frame| {
        frame.contains("old.atlassian.net")
            && frame.contains("old@example.com")
            && frame.contains("Continue to API token")
            && !frame.contains("••••")
    }));
    assert!(captured_frames
        .iter()
        .any(|frame| { frame.contains("Connect Jira") && frame.contains("••••") }));
    assert!(captured_frames.iter().any(|frame| {
        frame.contains("Connect Tempo")
            && frame.contains("old.atlassian.net.updated")
            && frame.contains("••••")
            && frame.contains("Esc")
            && frame.contains("back")
    }));
    assert!(captured_frames.iter().any(|frame| {
        frame.contains("✓ Jira account") && frame.contains("● Tempo account")
    }));
    assert!(captured_frames.iter().any(|frame| {
        frame.contains("old@example.com.updated")
            && frame.contains("● Jira account")
            && frame.contains("○ Tempo account")
    }));
    assert!(captured_frames.last().is_some_and(|frame| {
        frame.contains("old@example.com.updated")
            && frame.contains("JIRA")
            && frame.contains("TEMPO")
            && frame.matches("✓ connected").count() == 2
    }));
    assert_eq!(
    browser_state
        .lock()
        .map_err(|_| "test browser lock poisoned")?
        .browser_urls,
    [
        ATLASSIAN_TOKEN_URL,
        "https://old.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration",
    ]
);

    let rendered = format!("{} {}", result.human, result.data);
    for secret in [
        "old-jira-token",
        "old-tempo-token",
        "replacement-jira-token",
        "replacement-tempo-token",
        "old-account",
        "initial-derived-account",
        "final-derived-account",
    ] {
        assert!(!captured_frames.iter().any(|frame| frame.contains(secret)));
        assert!(!rendered.contains(secret));
    }
    Ok(())
}

#[tokio::test]
async fn ratatui_backtracking_without_edits_does_not_repeat_verification(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let browser_state = Arc::new(Mutex::new(PromptState::default()));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let events = vec![
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        // Complete setup once with retained credentials.
        Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        // Navigate back to Jira without editing anything.
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        // Continue through the still-connected stages and save.
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ];
    let app = App::with_onboarding_session(
        path.clone(),
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
            tempo_results: Mutex::new(VecDeque::from([Ok(())])),
        },
        RatatuiOnboardingSession::scripted(
            FakeBrowserLauncher {
                state: Arc::clone(&browser_state),
            },
            events,
            Arc::clone(&frames),
        ),
    );

    app.setup(SetupArgs {
        from_env: false,
        no_open: false,
        dry_run: false,
        verify: false,
    })
    .await?;

    let saved = Config::load(&path)?;
    assert_eq!(saved.account_id.as_deref(), Some("derived-account"));
    assert_eq!(
        browser_state
            .lock()
            .map_err(|_| "test browser lock poisoned")?
            .browser_urls
            .len(),
        2
    );
    assert!(frames
        .lock()
        .map_err(|_| "test frame lock poisoned")?
        .iter()
        .any(|frame| {
            frame.contains("✓ Jira account")
                && frame.contains("✓ Tempo account")
                && frame.contains("continue")
        }));
    Ok(())
}

#[tokio::test]
async fn ratatui_backtracking_discards_an_unverified_tempo_token_buffer(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let frames = Arc::new(Mutex::new(Vec::new()));
    let events = vec![
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        // Reach Save with both stored credentials verified.
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        // Start a replacement, then leave Tempo without verifying it.
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Event::Paste("partial-tempo-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        // Continue through Jira and retain the stored Tempo credential.
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ];
    let app = App::with_onboarding_session(
        path.clone(),
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
            tempo_results: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
        },
        RatatuiOnboardingSession::scripted(NoopBrowserLauncher, events, Arc::clone(&frames)),
    );

    app.setup(SetupArgs {
        from_env: false,
        no_open: true,
        dry_run: false,
        verify: false,
    })
    .await?;

    let saved = Config::load(&path)?;
    assert_eq!(saved.tempo_token.as_deref(), Some("old-tempo-token"));
    assert!(frames
        .lock()
        .map_err(|_| "test frame lock poisoned")?
        .iter()
        .any(|frame| { frame.contains("Connect Tempo") && frame.contains("••••") }));
    Ok(())
}

#[tokio::test]
async fn ratatui_pending_tempo_back_discards_the_unverified_token_buffer(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let before = fs::read(&path)?;
    let frames = Arc::new(Mutex::new(Vec::new()));
    let events = vec![
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("partial-tempo-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        // Continue through the still-connected Jira stage, then cancel on Tempo.
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    ];
    let app = App::with_onboarding_session(
        path.clone(),
        PendingTempoVerifier,
        RatatuiOnboardingSession::scripted(NoopBrowserLauncher, events, Arc::clone(&frames)),
    );

    let error = app
        .setup(SetupArgs {
            from_env: false,
            no_open: true,
            dry_run: false,
            verify: false,
        })
        .await
        .err()
        .ok_or("pending Tempo setup unexpectedly succeeded")?;

    assert!(error.to_string().contains("cancelled"));
    assert_eq!(fs::read(path)?, before);
    assert!(frames
        .lock()
        .map_err(|_| "test frame lock poisoned")?
        .iter()
        .any(|frame| { frame.contains("Connect Tempo") && frame.contains("••••") }));
    Ok(())
}

#[tokio::test]
async fn ratatui_reconfiguration_cancellation_leaves_config_byte_for_byte_unchanged(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let before = fs::read(&path)?;
    let frames = Arc::new(Mutex::new(Vec::new()));
    let events = vec![
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    ];
    let app = App::with_onboarding_session(
        path.clone(),
        SequenceVerifier {
            jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
            tempo_results: Mutex::new(VecDeque::from([Ok(())])),
        },
        RatatuiOnboardingSession::scripted(NoopBrowserLauncher, events, Arc::clone(&frames)),
    );

    let error = app
        .setup(SetupArgs {
            from_env: false,
            no_open: true,
            dry_run: false,
            verify: false,
        })
        .await
        .err()
        .ok_or("reconfiguration unexpectedly saved after cancellation")?;

    assert!(error.to_string().contains("cancelled"));
    assert_eq!(fs::read(path)?, before);
    assert!(frames
        .lock()
        .map_err(|_| "test frame lock poisoned")?
        .iter()
        .any(|frame| frame.contains("Save configuration")));
    Ok(())
}

#[tokio::test]
async fn ratatui_verification_keeps_terminal_events_responsive(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let frames = Arc::new(Mutex::new(Vec::new()));
    let mut events = first_run_tui_events(true);
    events.truncate(13);
    events.push(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    let app = App::with_onboarding_session(
        path.clone(),
        PendingJiraVerifier,
        RatatuiOnboardingSession::scripted(NoopBrowserLauncher, events, Arc::clone(&frames)),
    );

    let error = app
        .setup(SetupArgs {
            from_env: false,
            no_open: true,
            dry_run: false,
            verify: false,
        })
        .await
        .err()
        .ok_or("pending Jira verification ignored cancellation")?;

    assert!(error.to_string().contains("cancelled"));
    assert!(!path.exists());
    assert!(frames
        .lock()
        .map_err(|_| "test frame lock poisoned")?
        .iter()
        .any(|frame| frame.contains("Verifying Connect Jira")));
    Ok(())
}

#[tokio::test]
async fn incomplete_onboarding_session_cannot_save_credentials(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let initial = existing_config();
    initial.save(&path)?;
    let before = fs::read(&path)?;
    let app = App::with_onboarding_session(
        path.clone(),
        FakeVerifier {
            jira_error: None,
            tempo_error: None,
            tempo_accounts: Arc::new(Mutex::new(Vec::new())),
            config_update: None,
        },
        IncompleteOnboardingSession,
    );

    let error = app
        .setup(SetupArgs {
            from_env: false,
            no_open: true,
            dry_run: false,
            verify: false,
        })
        .await
        .err()
        .ok_or("incomplete onboarding unexpectedly succeeded")?;

    assert_eq!(
        (error.to_string(), fs::read(path)?),
        ("invalid onboarding workflow state".to_owned(), before)
    );
    Ok(())
}

#[tokio::test]
async fn interactive_setup_connects_both_services_and_saves_once_complete(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let state = Arc::new(Mutex::new(PromptState {
        text_responses: VecDeque::from([
            "https://Example.atlassian.net/jira/software/projects/DRAG".to_owned(),
            "person@example.com".to_owned(),
        ]),
        secret_responses: VecDeque::from([
            Some("jira-secret".to_owned()),
            Some("tempo-secret".to_owned()),
        ]),
        ..PromptState::default()
    }));
    let app = interactive_app(
        path.clone(),
        Arc::clone(&state),
        [Ok("derived-account".to_owned())],
        [Ok(())],
    );

    let result = app
        .setup(SetupArgs {
            from_env: false,
            no_open: false,
            dry_run: false,
            verify: false,
        })
        .await?;

    let saved = Config::load(&path)?;
    assert_eq!(saved.hostname.as_deref(), Some("example.atlassian.net"));
    assert_eq!(saved.account_id.as_deref(), Some("derived-account"));
    assert_eq!(saved.atlassian_token.as_deref(), Some("jira-secret"));
    assert_eq!(saved.tempo_token.as_deref(), Some("tempo-secret"));
    assert_eq!(result.data["source"], "interactive");
    assert_eq!(result.data["connection"]["jira"]["status"], "connected");
    assert_eq!(result.data["connection"]["tempo"]["status"], "connected");
    let output = format!("{} {}", result.human, result.data);
    assert!(!output.contains("derived-account"));
    assert!(!output.contains("jira-secret"));
    assert!(!output.contains("tempo-secret"));
    assert!(!output.contains(ATLASSIAN_TOKEN_URL));
    let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
    assert_eq!(
        state
            .text_prompts
            .iter()
            .map(|(label, _)| label.as_str())
            .collect::<Vec<_>>(),
        ["Jira site (hostname or HTTPS URL)", "Atlassian email"]
    );
    assert!(state
        .messages
        .iter()
        .any(|message| message.contains(ATLASSIAN_TOKEN_URL)));
    assert!(state.messages.iter().any(|message| message.contains(
    "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration"
)));
    assert_eq!(
    state.browser_urls,
    [
        ATLASSIAN_TOKEN_URL,
        "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration"
    ]
);
    assert_eq!(
    state
        .events
        .iter()
        .filter(|event| {
            event.starts_with("message:Create or manage")
                || event.starts_with("browser:")
                || event.starts_with("secret:")
        })
        .map(String::as_str)
        .collect::<Vec<_>>(),
    [
        "message:Create or manage your Atlassian API token:\nhttps://id.atlassian.com/manage-profile/security/api-tokens",
        "browser:https://id.atlassian.com/manage-profile/security/api-tokens",
        "secret:Atlassian API token",
        "message:Create or manage your Tempo API token:\nhttps://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration",
        "browser:https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration",
        "secret:Tempo API token"
    ]
);
    Ok(())
}

#[tokio::test]
async fn interactive_setup_no_open_prints_links_without_launching_browser(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let state = Arc::new(Mutex::new(PromptState {
        text_responses: VecDeque::from([
            "example.atlassian.net".to_owned(),
            "person@example.com".to_owned(),
        ]),
        secret_responses: VecDeque::from([
            Some("jira-secret".to_owned()),
            Some("tempo-secret".to_owned()),
        ]),
        ..PromptState::default()
    }));
    let app = interactive_app(
        path,
        Arc::clone(&state),
        [Ok("derived-account".to_owned())],
        [Ok(())],
    );

    app.setup(SetupArgs {
        from_env: false,
        no_open: true,
        dry_run: false,
        verify: false,
    })
    .await?;

    let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
    assert!(state.browser_urls.is_empty());
    assert!(state
        .messages
        .iter()
        .any(|message| message.contains(ATLASSIAN_TOKEN_URL)));
    assert!(state.messages.iter().any(|message| message.contains(
    "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration"
)));
    Ok(())
}

#[tokio::test]
async fn browser_launch_failure_warns_and_allows_setup_to_finish(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let state = Arc::new(Mutex::new(PromptState {
        text_responses: VecDeque::from([
            "example.atlassian.net".to_owned(),
            "person@example.com".to_owned(),
        ]),
        secret_responses: VecDeque::from([
            Some("jira-secret".to_owned()),
            Some("tempo-secret".to_owned()),
        ]),
        browser_failure: Some("no default browser".to_owned()),
        ..PromptState::default()
    }));
    let app = interactive_app(
        path.clone(),
        Arc::clone(&state),
        [Ok("derived-account".to_owned())],
        [Ok(())],
    );

    let result = app
        .setup(SetupArgs {
            from_env: false,
            no_open: false,
            dry_run: false,
            verify: false,
        })
        .await?;

    assert!(path.exists());
    let output = format!("{} {}", result.human, result.data);
    assert!(!output.contains("no default browser"));
    assert!(!output.contains(ATLASSIAN_TOKEN_URL));
    let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
    assert_eq!(state.browser_urls.len(), 2);
    assert_eq!(
        state
            .messages
            .iter()
            .filter(|message| message.starts_with("Warning: could not open"))
            .count(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn environment_setup_never_launches_or_prompts_with_any_no_open_value(
) -> Result<(), Box<dyn std::error::Error>> {
    for no_open in [false, true] {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let state = Arc::new(Mutex::new(PromptState::default()));
        let mut app = interactive_app(
            path,
            Arc::clone(&state),
            [Ok("derived-account".to_owned())],
            [Ok(())],
        );
        app.connection_environment = Box::new(FakeConnectionEnvironment {
            values: BTreeMap::from([
                (
                    "ATLASSIAN_HOST".to_owned(),
                    "example.atlassian.net".to_owned(),
                ),
                (
                    "ATLASSIAN_EMAIL".to_owned(),
                    "person@example.com".to_owned(),
                ),
                ("ATLASSIAN_TOKEN".to_owned(), "jira-secret".to_owned()),
                ("TEMPO_TOKEN".to_owned(), "tempo-secret".to_owned()),
            ]),
        });

        app.setup(SetupArgs {
            from_env: true,
            no_open,
            dry_run: false,
            verify: false,
        })
        .await?;

        let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
        assert!(state.browser_urls.is_empty());
        assert!(state.text_prompts.is_empty());
        assert!(state.secret_prompts.is_empty());
        assert!(state.messages.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn interactive_reconfiguration_offers_defaults_and_retains_hidden_tokens(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let state = Arc::new(Mutex::new(PromptState {
        text_responses: VecDeque::from([
            "old.atlassian.net".to_owned(),
            "old@example.com".to_owned(),
        ]),
        secret_responses: VecDeque::from([None, None]),
        ..PromptState::default()
    }));
    let app = interactive_app(
        path.clone(),
        Arc::clone(&state),
        [Ok("new-derived-account".to_owned())],
        [Ok(())],
    );

    app.setup(SetupArgs {
        from_env: false,
        no_open: false,
        dry_run: false,
        verify: false,
    })
    .await?;

    let saved = Config::load(&path)?;
    assert_eq!(saved.atlassian_token.as_deref(), Some("old-jira-token"));
    assert_eq!(saved.tempo_token.as_deref(), Some("old-tempo-token"));
    let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
    assert_eq!(
        state.text_prompts,
        [
            (
                "Jira site (hostname or HTTPS URL)".to_owned(),
                Some("old.atlassian.net".to_owned())
            ),
            (
                "Atlassian email".to_owned(),
                Some("old@example.com".to_owned())
            )
        ]
    );
    assert_eq!(
        state.secret_prompts,
        [
            ("Atlassian API token".to_owned(), true),
            ("Tempo API token".to_owned(), true)
        ]
    );
    Ok(())
}

#[tokio::test]
async fn interactive_setup_retries_only_the_failed_connection(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let state = Arc::new(Mutex::new(PromptState {
        text_responses: VecDeque::from([
            "not a host".to_owned(),
            "example.atlassian.net".to_owned(),
            "person@example.com".to_owned(),
            String::new(),
            String::new(),
        ]),
        secret_responses: VecDeque::from([
            Some("bad-jira".to_owned()),
            Some("good-jira".to_owned()),
            Some("bad-tempo".to_owned()),
            Some("good-tempo".to_owned()),
        ]),
        ..PromptState::default()
    }));
    let app = interactive_app(
        path.clone(),
        Arc::clone(&state),
        [
            Err(VerificationFailure::Authentication(
                "authentication failed".to_owned(),
            )),
            Ok("derived-account".to_owned()),
        ],
        [
            Err(VerificationFailure::Authentication(
                "token rejected".to_owned(),
            )),
            Ok(()),
        ],
    );

    app.setup(SetupArgs {
        from_env: false,
        no_open: false,
        dry_run: false,
        verify: false,
    })
    .await?;

    let saved = Config::load(&path)?;
    assert_eq!(saved.atlassian_token.as_deref(), Some("good-jira"));
    assert_eq!(saved.tempo_token.as_deref(), Some("good-tempo"));
    let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
    let messages = &state.messages;
    assert!(messages
        .iter()
        .any(|message| message.contains("Invalid Jira site")));
    assert!(messages
        .iter()
        .any(|message| message.contains("Could not connect to Jira")));
    assert!(messages
        .iter()
        .any(|message| message.contains("Could not connect to Tempo")));
    assert_eq!(
        state.text_prompts[3..],
        [
            (
                "Jira site (hostname or HTTPS URL)".to_owned(),
                Some("example.atlassian.net".to_owned())
            ),
            (
                "Atlassian email".to_owned(),
                Some("person@example.com".to_owned())
            )
        ]
    );
    assert_eq!(
    state.browser_urls,
    [
        ATLASSIAN_TOKEN_URL,
        "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration"
    ]
);
    Ok(())
}

#[tokio::test]
async fn interactive_setup_propagates_non_authentication_verification_errors(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let state = Arc::new(Mutex::new(PromptState {
        text_responses: VecDeque::from([
            "example.atlassian.net".to_owned(),
            "person@example.com".to_owned(),
        ]),
        secret_responses: VecDeque::from([Some("jira-token".to_owned())]),
        ..PromptState::default()
    }));
    let app = interactive_app(
        path.clone(),
        Arc::clone(&state),
        [Err(VerificationFailure::Fatal(
            "network timeout".to_owned(),
        ))],
        std::iter::empty(),
    );

    let error = match app
        .setup(SetupArgs {
            from_env: false,
            no_open: false,
            dry_run: false,
            verify: false,
        })
        .await
    {
        Ok(_) => return Err("setup should propagate the network error".into()),
        Err(error) => error,
    };

    assert!(matches!(error, CliError::Api(message) if message == "network timeout"));
    assert!(!path.exists());
    let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
    assert_eq!(state.text_prompts.len(), 2);
    assert!(!state
        .messages
        .iter()
        .any(|message| message.contains("try again")));
    Ok(())
}

#[tokio::test]
async fn interactive_setup_does_not_retry_fatal_tempo_errors(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let state = Arc::new(Mutex::new(PromptState {
        text_responses: VecDeque::from([
            "example.atlassian.net".to_owned(),
            "person@example.com".to_owned(),
        ]),
        secret_responses: VecDeque::from([
            Some("jira-token".to_owned()),
            Some("tempo-token".to_owned()),
        ]),
        ..PromptState::default()
    }));
    let app = interactive_app(
        path.clone(),
        Arc::clone(&state),
        [Ok("derived-account".to_owned())],
        [Err(VerificationFailure::Fatal(
            "malformed response".to_owned(),
        ))],
    );

    let error = match app
        .setup(SetupArgs {
            from_env: false,
            no_open: false,
            dry_run: false,
            verify: false,
        })
        .await
    {
        Ok(_) => return Err("setup should propagate the response error".into()),
        Err(error) => error,
    };

    assert!(matches!(error, CliError::Api(message) if message == "malformed response"));
    assert!(!path.exists());
    let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
    assert_eq!(state.secret_prompts.len(), 2);
    assert!(!state
        .messages
        .iter()
        .any(|message| message.contains("Check the Tempo token")));
    Ok(())
}

#[tokio::test]
async fn interactive_cancellation_leaves_existing_config_unchanged(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let before = fs::read(&path)?;
    let state = Arc::new(Mutex::new(PromptState::default()));
    let app = interactive_app(path.clone(), state, std::iter::empty(), std::iter::empty());

    let error = match app
        .setup(SetupArgs {
            from_env: false,
            no_open: false,
            dry_run: false,
            verify: false,
        })
        .await
    {
        Ok(_) => return Err("setup should be cancelled when input ends".into()),
        Err(error) => error,
    };

    assert!(error.to_string().contains("cancelled"));
    assert_eq!(fs::read(path)?, before);
    Ok(())
}

#[tokio::test]
async fn cancellation_after_a_failed_connection_check_leaves_config_unchanged(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let before = fs::read(&path)?;
    let state = Arc::new(Mutex::new(PromptState {
        text_responses: VecDeque::from([
            "old.atlassian.net".to_owned(),
            "old@example.com".to_owned(),
        ]),
        secret_responses: VecDeque::from([None]),
        ..PromptState::default()
    }));
    let app = interactive_app(
        path.clone(),
        state,
        [Err(VerificationFailure::Authentication(
            "authentication failed".to_owned(),
        ))],
        std::iter::empty(),
    );

    assert!(app
        .setup(SetupArgs {
            from_env: false,
            no_open: false,
            dry_run: false,
            verify: false,
        })
        .await
        .is_err());

    assert_eq!(fs::read(path)?, before);
    Ok(())
}

#[test]
fn setup_environment_does_not_read_the_compatibility_account_id(
) -> Result<(), Box<dyn std::error::Error>> {
    let values = BTreeMap::from([
        ("ATLASSIAN_HOST", "example.atlassian.net"),
        ("ATLASSIAN_EMAIL", "person@example.com"),
        ("ATLASSIAN_TOKEN", " jira-secret\n"),
        ("TEMPO_TOKEN", " tempo-secret\n"),
        ("TEMPO_ACCOUNT_ID", "must-not-be-used"),
    ]);
    let mut requested = Vec::new();
    let credentials = SetupCredentials::from_source(|name| {
        requested.push(name.to_owned());
        values
            .get(name)
            .map(|value| (*value).to_owned())
            .ok_or_else(|| CliError::InvalidInput(format!("missing {name}")))
    })?;

    assert_eq!(credentials.hostname, "example.atlassian.net");
    assert_eq!(credentials.atlassian_token, "jira-secret");
    assert_eq!(credentials.tempo_token, "tempo-secret");
    assert_eq!(
        requested,
        [
            "ATLASSIAN_HOST",
            "ATLASSIAN_EMAIL",
            "ATLASSIAN_TOKEN",
            "TEMPO_TOKEN"
        ]
    );
    Ok(())
}

#[tokio::test]
async fn verified_environment_setup_derives_account_and_preserves_local_state(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let config = existing_config();
    config.save(&path)?;
    let tempo_accounts = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_connection_verifier(
        path.clone(),
        FakeVerifier {
            jira_error: None,
            tempo_error: None,
            tempo_accounts: Arc::clone(&tempo_accounts),
            config_update: None,
        },
    );

    let result = app
        .verify_and_save_environment_setup(EnvironmentSetupPlan::new(setup_credentials()))
        .await?;

    let saved = Config::load(&path)?;
    assert_eq!(saved.account_id.as_deref(), Some("derived-account"));
    assert_eq!(saved.tempo_token.as_deref(), Some("new-tempo-token"));
    let accounts = tempo_accounts
        .lock()
        .map_err(|_| "test verifier lock was poisoned")?;
    assert_eq!(accounts.as_slice(), ["derived-account"]);
    assert_eq!(result.data["source"], "environment");
    assert_eq!(result.data["verification"]["jira"], "connected");
    assert_eq!(result.data["verification"]["tempo"], "connected");
    let output = format!("{} {}", result.human, result.data);
    assert!(!output.contains("new-tempo-token"));
    assert!(!output.contains("new-jira-token"));
    assert!(!output.contains("derived-account"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    }
    Ok(())
}

#[tokio::test]
async fn verified_environment_setup_dry_run_completes_read_only_checks_without_saving(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let before = fs::read(&path)?;
    let tempo_accounts = Arc::new(Mutex::new(Vec::new()));
    let mut app = App::with_connection_verifier(
        path.clone(),
        FakeVerifier {
            jira_error: None,
            tempo_error: None,
            tempo_accounts: Arc::clone(&tempo_accounts),
            config_update: None,
        },
    );
    app.connection_environment = Box::new(FakeConnectionEnvironment {
        values: BTreeMap::from([
            (
                "ATLASSIAN_HOST".to_owned(),
                "example.atlassian.net".to_owned(),
            ),
            ("ATLASSIAN_EMAIL".to_owned(), "new@example.com".to_owned()),
            ("ATLASSIAN_TOKEN".to_owned(), "new-jira-token".to_owned()),
            ("TEMPO_TOKEN".to_owned(), "new-tempo-token".to_owned()),
        ]),
    });

    let result = app
        .setup(SetupArgs {
            from_env: true,
            no_open: false,
            dry_run: true,
            verify: true,
        })
        .await?;

    assert_eq!(fs::read(path)?, before);
    assert_eq!(result.data["configured"], false);
    assert_eq!(result.data["remoteVerification"]["status"], "completed");
    assert_eq!(result.data["remoteVerification"]["jira"], "connected");
    assert_eq!(result.data["remoteVerification"]["tempo"], "connected");
    assert_eq!(
        tempo_accounts
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?
            .as_slice(),
        ["derived-account"]
    );
    let output = format!("{} {}", result.human, result.data);
    assert!(!output.contains("new-tempo-token"));
    assert!(!output.contains("new-jira-token"));
    assert!(!output.contains("derived-account"));
    Ok(())
}

#[tokio::test]
async fn failed_verification_leaves_config_byte_for_byte_unchanged(
) -> Result<(), Box<dyn std::error::Error>> {
    for (jira_error, tempo_error) in [
        (Some("jira rejected credentials".to_owned()), None),
        (None, Some("tempo rejected credentials".to_owned())),
    ] {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let config = existing_config();
        config.save(&path)?;
        let before = fs::read(&path)?;
        let tempo_accounts = Arc::new(Mutex::new(Vec::new()));
        let jira_should_fail = jira_error.is_some();
        let app = App::with_connection_verifier(
            path.clone(),
            FakeVerifier {
                jira_error,
                tempo_error,
                tempo_accounts: Arc::clone(&tempo_accounts),
                config_update: None,
            },
        );

        assert!(app
            .verify_and_save_environment_setup(EnvironmentSetupPlan::new(setup_credentials()))
            .await
            .is_err());
        assert_eq!(fs::read(path)?, before);
        let accounts = tempo_accounts
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?;
        if jira_should_fail {
            assert!(accounts.is_empty());
        } else {
            assert_eq!(accounts.as_slice(), ["derived-account"]);
        }
    }
    Ok(())
}
