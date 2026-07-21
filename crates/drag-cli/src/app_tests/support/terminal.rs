#[cfg(unix)]
use super::*;

#[cfg(unix)]
pub(super) fn spawn_setup_pty(
    path: &std::path::Path,
    scenario: &str,
) -> Result<OsSession, Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new("sh");
    command
    .args([
        "-c",
        "exec \"$1\" --exact app::tests::support::terminal::pty_setup_helper --ignored --nocapture --test-threads=1 >\"$2\"",
        "drag-pty-wrapper",
    ])
    .arg(executable)
    .arg(pty_output_path(path))
    .env("DRAG_PTY_CONFIG", path)
    .env("DRAG_PTY_SCENARIO", scenario);
    for variable in [
        "TEMPO_TOKEN",
        "TEMPO_ACCOUNT_ID",
        "ATLASSIAN_EMAIL",
        "ATLASSIAN_TOKEN",
        "ATLASSIAN_HOST",
        "DRAG_REDUCED_MOTION",
    ] {
        command.env_remove(variable);
    }
    let mut session = Session::spawn(command)?;
    session.get_process_mut().set_window_size(100, 30)?;
    session.set_expect_timeout(Some(Duration::from_secs(10)));
    Ok(session)
}

#[cfg(unix)]
fn spawn_list_pty() -> Result<OsSession, Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command.args([
        "--exact",
        "app::tests::support::terminal::pty_list_report_helper",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    for variable in [
        "TEMPO_TOKEN",
        "TEMPO_ACCOUNT_ID",
        "ATLASSIAN_EMAIL",
        "ATLASSIAN_TOKEN",
        "ATLASSIAN_HOST",
        "BROWSER",
    ] {
        command.env_remove(variable);
    }
    let mut session = Session::spawn(command)?;
    session.get_process_mut().set_window_size(100, 24)?;
    session.set_expect_timeout(Some(Duration::from_secs(10)));
    Ok(session)
}

#[cfg(unix)]
pub(super) fn pty_output_path(config_path: &std::path::Path) -> PathBuf {
    config_path.with_extension("stdout.json")
}

#[cfg(unix)]
pub(super) fn read_pty_json_output(
    config_path: &std::path::Path,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let output = fs::read_to_string(pty_output_path(config_path))?;
    assert!(!output.contains('\u{1b}'));
    assert!(!output.contains("Drag setup"));
    assert!(!output.contains("Connect Jira"));

    let json_start = output
        .find("{\n  \"ok\": true,")
        .ok_or("PTY stdout did not contain a JSON success envelope")?;
    let json_end = output
        .rfind("\n}")
        .map(|offset| offset + 2)
        .ok_or("PTY stdout contained an incomplete JSON success envelope")?;
    assert_eq!(
        output[..json_start].trim(),
        "running 1 test\ntest app::tests::support::terminal::pty_setup_helper ..."
    );
    assert!(output[json_end..].starts_with("\nok\n\ntest result: ok."));

    Ok(serde_json::from_str(&output[json_start..json_end])?)
}

#[cfg(unix)]
pub(super) fn send_paste(
    session: &mut OsSession,
    value: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    session.send(format!("\u{1b}[200~{value}\u{1b}[201~"))?;
    Ok(())
}

#[cfg(unix)]
pub(super) fn assert_terminal_restored(output: &[u8]) {
    let output = String::from_utf8_lossy(output);
    for restoration in ["\u{1b}[?2004l", "\u{1b}[?1049l", "\u{1b}[?25h"] {
        assert!(
            output.contains(restoration),
            "missing terminal restoration sequence {restoration:?}"
        );
    }
}

#[cfg(unix)]
pub(super) fn expect_terminal_restoration(
    session: &mut OsSession,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let paste = session.expect("\u{1b}[?2004l")?;
    let output = paste.before().to_vec();
    session.expect("\u{1b}[?1049l")?;
    session.expect("\u{1b}[?25h")?;
    Ok(output)
}

#[cfg(unix)]
fn expect_list_terminal_restoration(
    session: &mut OsSession,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let alternate_screen = session.expect("\u{1b}[?1049l")?;
    let output = alternate_screen.before().to_vec();
    session.expect("\u{1b}[?25h")?;
    Ok(output)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "PTY child process invoked by the interactive setup tests"]
async fn pty_setup_helper() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(std::env::var("DRAG_PTY_CONFIG")?);
    let scenario = std::env::var("DRAG_PTY_SCENARIO")?;
    let (jira_results, tempo_results) = match scenario.as_str() {
        "success" | "reconfigure" | "late-cancel" | "resize" => (
            VecDeque::from([Ok("pty-account".to_owned())]),
            VecDeque::from([Ok(())]),
        ),
        "retry" => (
            VecDeque::from([
                Err(VerificationFailure::Authentication(
                    "Jira credentials rejected".to_owned(),
                )),
                Ok("pty-account".to_owned()),
            ]),
            VecDeque::from([
                Err(VerificationFailure::Authentication(
                    "Tempo token rejected".to_owned(),
                )),
                Ok(()),
            ]),
        ),
        "ratatui-fatal" => (
            VecDeque::from([Err(VerificationFailure::Fatal(
                "fatal PTY verification failure".to_owned(),
            ))]),
            VecDeque::new(),
        ),
        "ratatui-panic" => (VecDeque::new(), VecDeque::new()),
        _ => return Err(format!("unknown PTY scenario: {scenario}").into()),
    };
    let verifier = SequenceVerifier {
        jira_results: Mutex::new(jira_results),
        tempo_results: Mutex::new(tempo_results),
    };
    let app = if scenario == "ratatui-panic" {
        App::with_onboarding_session(
            path,
            PanickingVerifier,
            RatatuiOnboardingSession::terminal(),
        )
    } else {
        App::with_onboarding_session(path, verifier, RatatuiOnboardingSession::terminal())
    };

    let setup = app.setup(SetupArgs {
        from_env: false,
        no_open: true,
        dry_run: false,
        verify: false,
    });
    if scenario == "ratatui-panic" {
        let outcome = AssertUnwindSafe(setup).catch_unwind().await;
        assert!(!crossterm::terminal::is_raw_mode_enabled()?);
        let Err(payload) = outcome else {
            return Err("expected the PTY verifier to panic".into());
        };
        if payload.downcast_ref::<&str>().copied() != Some("intentional PTY verifier panic") {
            return Err("PTY verifier produced an unexpected panic payload".into());
        }
        return Ok(());
    }

    let result = setup.await;
    assert!(!crossterm::terminal::is_raw_mode_enabled()?);
    match result {
        Ok(result) => crate::emit_result(result, ResolvedOutputMode::Json)?,
        Err(error) => crate::emit_error(&error, ResolvedOutputMode::Json),
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "PTY child process invoked by the interactive list report test"]
async fn pty_list_report_helper() -> Result<(), Box<dyn std::error::Error>> {
    let report = empty_list_report(false);
    let session = crate::list_tui::RatatuiListReportSession::terminal_with_browser_launcher(
        NoopBrowserLauncher,
    );
    assert!(session.is_eligible());
    assert_eq!(session.run(&report).await?, ListReportAction::Close);
    assert!(!crossterm::terminal::is_raw_mode_enabled()?);
    Ok(())
}

#[cfg(unix)]
#[test]
fn pty_list_report_starts_quits_and_restores_the_terminal() -> Result<(), Box<dyn std::error::Error>>
{
    let mut session = spawn_list_pty()?;

    session
        .expect("July 2026")
        .map_err(|error| format!("waiting for list calendar: {error}"))?;
    session
        .expect("Tuesday, 2026-07-14")
        .map_err(|error| format!("waiting for selected list date: {error}"))?;
    session.send("q")?;
    let restored = expect_list_terminal_restoration(&mut session)
        .map_err(|error| format!("waiting for list terminal restoration: {error}"))?;
    session
        .expect("ok")
        .map_err(|error| format!("waiting for successful list helper exit: {error}"))?;
    session
        .expect(Eof)
        .map_err(|error| format!("waiting for list helper EOF: {error}"))?;

    assert!(!restored.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn pty_fatal_error_restores_ratatui_before_emitting_structured_error(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    let mut session = spawn_setup_pty(&path, "ratatui-fatal")?;

    session.expect("Jira site")?;
    session.send("example.atlassian.net")?;
    session.send("\t")?;
    session.send("person@example.com")?;
    session.send("\t\r")?;
    session.expect("Atlassian API token")?;
    session.send("pty-fatal-jira-token")?;
    session.send("\t")?;
    session.send("\r")?;

    let error_output = session.expect("\"code\": \"api_error\"")?;
    let before_error = String::from_utf8_lossy(error_output.before());
    for restoration in ["\u{1b}[?2004l", "\u{1b}[?1049l", "\u{1b}[?25h"] {
        assert!(
            before_error.contains(restoration),
            "missing terminal restoration sequence before structured error"
        );
    }
    assert!(!before_error.contains("pty-fatal-jira-token"));
    session.expect("fatal PTY verification failure")?;
    session.expect(Eof)?;
    assert!(!path.exists());
    Ok(())
}
