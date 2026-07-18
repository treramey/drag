use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use std::panic::AssertUnwindSafe;
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::time::Duration;

use chrono::NaiveDate;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use drag::models::ListPagination;
use drag::schedule::ScheduleDetails;
#[cfg(unix)]
use expectrl::session::OsSession;
#[cfg(unix)]
use expectrl::{ControlCode, Eof, Expect, Session};
#[cfg(unix)]
use futures_util::FutureExt;
use tempfile::TempDir;

use super::{
    normalize_jira_site, App, BrowserLauncher, Config, ConnectionOutcome, ConnectionVerifier,
    EnvironmentSetupPlan, JiraCredentials, NoopBrowserLauncher, OnboardingFuture,
    OnboardingSession, OnboardingWorkflow, RatatuiOnboardingSession, SecretInput, SetupCredentials,
    SetupPrompter, TempoCredentials, VerificationFuture, ATLASSIAN_TOKEN_URL,
};
use crate::cli::{DoctorArgs, SetupArgs};
use crate::list::ListReport;
use crate::list_tui::{ListReportAction, ListReportSession};
use crate::CliError;
#[cfg(unix)]
use crate::ResolvedOutputMode;

struct FakeVerifier {
    jira_error: Option<String>,
    tempo_error: Option<String>,
    tempo_accounts: Arc<Mutex<Vec<String>>>,
    config_update: Option<(PathBuf, Config)>,
}

#[derive(Default)]
struct PromptState {
    text_responses: VecDeque<String>,
    secret_responses: VecDeque<Option<String>>,
    text_prompts: Vec<(String, Option<String>)>,
    secret_prompts: Vec<(String, bool)>,
    messages: Vec<String>,
    browser_urls: Vec<String>,
    browser_failure: Option<String>,
    events: Vec<String>,
}

struct FakePrompter {
    terminal: bool,
    state: Arc<Mutex<PromptState>>,
}

struct FakeBrowserLauncher {
    state: Arc<Mutex<PromptState>>,
}

struct FakeConnectionEnvironment {
    values: BTreeMap<String, String>,
}

struct FakeListReportSession {
    eligible: bool,
    selected_dates: Arc<Mutex<Vec<NaiveDate>>>,
}

impl ListReportSession for FakeListReportSession {
    fn is_eligible(&self) -> bool {
        self.eligible
    }

    fn run<'a>(&'a self, report: &'a ListReport) -> crate::list_tui::ListReportFuture<'a> {
        Box::pin(async move {
            self.selected_dates
                .lock()
                .map_err(|_| CliError::Io(std::io::Error::other("list session lock poisoned")))?
                .push(report.selected_date());
            Ok(ListReportAction::Close)
        })
    }
}

fn empty_list_report(verbose: bool) -> ListReport {
    ListReport::new(
        NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN),
        Vec::new(),
        ScheduleDetails {
            month_required_duration: "160h".to_owned(),
            month_logged_duration: "72h".to_owned(),
            month_current_period_duration: "+4h".to_owned(),
            day_required_duration: "8h".to_owned(),
            day_logged_duration: "0h".to_owned(),
        },
        ListPagination {
            selected_date: "2026-07-14".to_owned(),
            month_start: "2026-07-01".to_owned(),
            month_end: "2026-07-31".to_owned(),
            limit: Some(100),
            page_limit: 1,
            all_pages: false,
            pages_retrieved: 1,
            records_retrieved: 0,
            records_returned: 0,
            next: None,
            complete: true,
            totals_complete: true,
        },
        BTreeMap::new(),
        verbose,
    )
}

#[tokio::test]
async fn eligible_human_list_is_presented_by_the_injected_report_session() -> Result<(), CliError> {
    let temp = TempDir::new()?;
    let selected_dates = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_connection_verifier(
        temp.path().join("config.json"),
        FakeVerifier {
            jira_error: None,
            tempo_error: None,
            tempo_accounts: Arc::new(Mutex::new(Vec::new())),
            config_update: None,
        },
    )
    .with_list_report_session(FakeListReportSession {
        eligible: true,
        selected_dates: Arc::clone(&selected_dates),
    });

    let rendered = app.finish_list(empty_list_report(false), true).await?;

    assert!(rendered.is_none());
    let dates = selected_dates
        .lock()
        .map_err(|_| CliError::Io(std::io::Error::other("selected dates lock poisoned")))?;
    assert_eq!(
        *dates,
        [NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN)]
    );
    Ok(())
}

#[tokio::test]
async fn explicit_json_or_ineligible_human_list_remains_non_interactive() -> Result<(), CliError> {
    for (interactive, eligible) in [(false, true), (true, false)] {
        let temp = TempDir::new()?;
        let selected_dates = Arc::new(Mutex::new(Vec::new()));
        let app = App::with_connection_verifier(
            temp.path().join("config.json"),
            FakeVerifier {
                jira_error: None,
                tempo_error: None,
                tempo_accounts: Arc::new(Mutex::new(Vec::new())),
                config_update: None,
            },
        )
        .with_list_report_session(FakeListReportSession {
            eligible,
            selected_dates: Arc::clone(&selected_dates),
        });

        let rendered = app
            .finish_list(empty_list_report(false), interactive)
            .await?;

        let rendered = rendered.ok_or_else(|| CliError::Api("missing plain result".to_owned()))?;
        assert_eq!(rendered.data["date"], "2026-07-14");
        assert!(selected_dates
            .lock()
            .map_err(|_| CliError::Io(std::io::Error::other("selected dates lock poisoned")))?
            .is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn eligible_verbose_human_list_is_presented_by_the_report_session() -> Result<(), CliError> {
    let temp = TempDir::new()?;
    let selected_dates = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_connection_verifier(
        temp.path().join("config.json"),
        FakeVerifier {
            jira_error: None,
            tempo_error: None,
            tempo_accounts: Arc::new(Mutex::new(Vec::new())),
            config_update: None,
        },
    )
    .with_list_report_session(FakeListReportSession {
        eligible: true,
        selected_dates: Arc::clone(&selected_dates),
    });

    let rendered = app.finish_list(empty_list_report(true), true).await?;

    assert!(rendered.is_none());
    assert_eq!(
        *selected_dates
            .lock()
            .map_err(|_| CliError::Io(std::io::Error::other("selected dates lock poisoned")))?,
        [NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN)]
    );
    Ok(())
}

impl super::ConnectionEnvironment for FakeConnectionEnvironment {
    fn value(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned()
    }

    fn is_set(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }
}

impl BrowserLauncher for FakeBrowserLauncher {
    fn open(&self, url: &str) -> std::io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| std::io::Error::other("test browser lock poisoned"))?;
        state.browser_urls.push(url.to_owned());
        state.events.push(format!("browser:{url}"));
        match &state.browser_failure {
            Some(message) => Err(std::io::Error::other(message.clone())),
            None => Ok(()),
        }
    }
}

impl SetupPrompter for FakePrompter {
    fn is_terminal(&self) -> bool {
        self.terminal
    }

    fn message(&self, message: &str) -> Result<(), CliError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| CliError::Io(std::io::Error::other("test prompt lock poisoned")))?;
        state.messages.push(message.to_owned());
        state.events.push(format!("message:{message}"));
        Ok(())
    }

    fn prompt_text(&self, label: &str, default: Option<&str>) -> Result<String, CliError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| CliError::Io(std::io::Error::other("test prompt lock poisoned")))?;
        state
            .text_prompts
            .push((label.to_owned(), default.map(str::to_owned)));
        let response = state
            .text_responses
            .pop_front()
            .ok_or_else(super::setup_cancelled)?;
        if response.is_empty() {
            Ok(default.unwrap_or_default().to_owned())
        } else {
            Ok(response)
        }
    }

    fn prompt_secret(&self, label: &str, can_retain: bool) -> Result<Option<String>, CliError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| CliError::Io(std::io::Error::other("test prompt lock poisoned")))?;
        state.secret_prompts.push((label.to_owned(), can_retain));
        state.events.push(format!("secret:{label}"));
        state
            .secret_responses
            .pop_front()
            .ok_or_else(super::setup_cancelled)
    }
}

struct SequenceVerifier {
    jira_results: Mutex<VecDeque<Result<String, VerificationFailure>>>,
    tempo_results: Mutex<VecDeque<Result<(), VerificationFailure>>>,
}

#[cfg(unix)]
struct PanickingVerifier;

struct RecordingSequenceVerifier {
    jira_results: Mutex<VecDeque<Result<String, VerificationFailure>>>,
    tempo_results: Mutex<VecDeque<Result<(), VerificationFailure>>>,
    attempts: Arc<Mutex<Vec<RecordedVerification>>>,
}

enum RecordedVerification {
    Jira {
        hostname: String,
        email: String,
        token: String,
    },
    Tempo {
        account_id: String,
        token: String,
    },
}

struct ScriptedOnboardingSession {
    events: Arc<Mutex<Vec<String>>>,
}

struct IncompleteOnboardingSession;

struct PendingJiraVerifier;

struct PendingTempoVerifier;

struct DoctorVerifier {
    jira_result: Mutex<Option<Result<String, VerificationFailure>>>,
    tempo_result: Mutex<Option<Result<(), VerificationFailure>>>,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

enum VerificationFailure {
    Authentication(String),
    Fatal(String),
}

impl OnboardingSession for ScriptedOnboardingSession {
    fn is_terminal(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, mut workflow: OnboardingWorkflow<'a>) -> OnboardingFuture<'a> {
        let events = Arc::clone(&self.events);
        Box::pin(async move {
            let jira_page = workflow.jira_token_page()?;
            events
                .lock()
                .map_err(|_| CliError::Api("test session lock was poisoned".to_owned()))?
                .push(format!("jira-browser:{}", jira_page.open_browser));
            match workflow
                .connect_jira(
                    "https://Example.atlassian.net/jira/software".to_owned(),
                    " scripted@example.com ".to_owned(),
                    SecretInput::Replace("scripted-jira-token".to_owned()),
                )
                .await?
            {
                ConnectionOutcome::Connected => {}
                ConnectionOutcome::Rejected(error) => return Err(error),
            }

            let tempo_page = workflow.tempo_token_page()?;
            events
                .lock()
                .map_err(|_| CliError::Api("test session lock was poisoned".to_owned()))?
                .push(format!("tempo-browser:{}", tempo_page.open_browser));
            match workflow
                .connect_tempo(SecretInput::Replace("scripted-tempo-token".to_owned()))
                .await?
            {
                ConnectionOutcome::Connected => {}
                ConnectionOutcome::Rejected(error) => return Err(error),
            }

            events
                .lock()
                .map_err(|_| CliError::Api("test session lock was poisoned".to_owned()))?
                .push("save".to_owned());
            Ok(workflow)
        })
    }
}

impl OnboardingSession for IncompleteOnboardingSession {
    fn is_terminal(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, workflow: OnboardingWorkflow<'a>) -> OnboardingFuture<'a> {
        Box::pin(async move { Ok(workflow) })
    }
}

impl VerificationFailure {
    fn into_cli_error(self) -> CliError {
        match self {
            Self::Authentication(message) => {
                CliError::authentication(crate::RemoteService::Unknown, message)
            }
            Self::Fatal(message) => CliError::Api(message),
        }
    }
}

impl ConnectionVerifier for SequenceVerifier {
    fn verify_jira<'a>(
        &'a self,
        _connection: &'a JiraCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, String> {
        Box::pin(async move {
            self.jira_results
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .pop_front()
                .ok_or_else(|| CliError::Api("unexpected Jira verification".to_owned()))?
                .map_err(VerificationFailure::into_cli_error)
        })
    }

    fn verify_tempo<'a>(
        &'a self,
        _connection: &'a TempoCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, ()> {
        Box::pin(async move {
            self.tempo_results
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .pop_front()
                .ok_or_else(|| CliError::Api("unexpected Tempo verification".to_owned()))?
                .map_err(VerificationFailure::into_cli_error)
        })
    }
}

#[cfg(unix)]
impl ConnectionVerifier for PanickingVerifier {
    fn verify_jira<'a>(
        &'a self,
        _connection: &'a JiraCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, String> {
        Box::pin(
            async move { std::panic::resume_unwind(Box::new("intentional PTY verifier panic")) },
        )
    }

    fn verify_tempo<'a>(
        &'a self,
        _connection: &'a TempoCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, ()> {
        Box::pin(async move {
            Err(CliError::Api(
                "Tempo verification must not run after a Jira panic".to_owned(),
            ))
        })
    }
}

impl ConnectionVerifier for RecordingSequenceVerifier {
    fn verify_jira<'a>(
        &'a self,
        connection: &'a JiraCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, String> {
        Box::pin(async move {
            self.attempts
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .push(RecordedVerification::Jira {
                    hostname: connection.hostname.clone(),
                    email: connection.atlassian_user_email.clone(),
                    token: connection.atlassian_token.clone(),
                });
            self.jira_results
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .pop_front()
                .ok_or_else(|| CliError::Api("unexpected Jira verification".to_owned()))?
                .map_err(VerificationFailure::into_cli_error)
        })
    }

    fn verify_tempo<'a>(
        &'a self,
        connection: &'a TempoCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, ()> {
        Box::pin(async move {
            self.attempts
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .push(RecordedVerification::Tempo {
                    account_id: connection.account_id.clone(),
                    token: connection.tempo_token.clone(),
                });
            self.tempo_results
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .pop_front()
                .ok_or_else(|| CliError::Api("unexpected Tempo verification".to_owned()))?
                .map_err(VerificationFailure::into_cli_error)
        })
    }
}

impl ConnectionVerifier for PendingJiraVerifier {
    fn verify_jira<'a>(
        &'a self,
        _connection: &'a JiraCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, String> {
        Box::pin(std::future::pending())
    }

    fn verify_tempo<'a>(
        &'a self,
        _connection: &'a TempoCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, ()> {
        Box::pin(async {
            Err(CliError::Api(
                "Tempo verification should not start".to_owned(),
            ))
        })
    }
}

impl ConnectionVerifier for PendingTempoVerifier {
    fn verify_jira<'a>(
        &'a self,
        _connection: &'a JiraCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, String> {
        Box::pin(async { Ok("derived-account".to_owned()) })
    }

    fn verify_tempo<'a>(
        &'a self,
        _connection: &'a TempoCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, ()> {
        Box::pin(std::future::pending())
    }
}

impl ConnectionVerifier for DoctorVerifier {
    fn verify_jira<'a>(
        &'a self,
        _connection: &'a JiraCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, String> {
        Box::pin(async move {
            self.calls
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .push("jira");
            self.jira_result
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .take()
                .ok_or_else(|| CliError::Api("unexpected Jira verification".to_owned()))?
                .map_err(VerificationFailure::into_cli_error)
        })
    }

    fn verify_tempo<'a>(
        &'a self,
        _connection: &'a TempoCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, ()> {
        Box::pin(async move {
            self.calls
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .push("tempo");
            self.tempo_result
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .take()
                .ok_or_else(|| CliError::Api("unexpected Tempo verification".to_owned()))?
                .map_err(VerificationFailure::into_cli_error)
        })
    }
}

fn doctor_app(
    path: PathBuf,
    jira_result: Result<String, VerificationFailure>,
    tempo_result: Result<(), VerificationFailure>,
) -> (App, Arc<Mutex<Vec<&'static str>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_connection_verifier(
        path,
        DoctorVerifier {
            jira_result: Mutex::new(Some(jira_result)),
            tempo_result: Mutex::new(Some(tempo_result)),
            calls: Arc::clone(&calls),
        },
    );
    (app, calls)
}

fn interactive_app(
    path: PathBuf,
    state: Arc<Mutex<PromptState>>,
    jira_results: impl IntoIterator<Item = Result<String, VerificationFailure>>,
    tempo_results: impl IntoIterator<Item = Result<(), VerificationFailure>>,
) -> App {
    let browser_state = Arc::clone(&state);
    App::with_setup_dependencies(
        path,
        SequenceVerifier {
            jira_results: Mutex::new(jira_results.into_iter().collect()),
            tempo_results: Mutex::new(tempo_results.into_iter().collect()),
        },
        FakePrompter {
            terminal: true,
            state,
        },
        FakeBrowserLauncher {
            state: browser_state,
        },
    )
}

fn first_run_tui_events(save: bool) -> Vec<Event> {
    let mut events = vec![
        Event::Paste("https://Example.atlassian.net/jira/software".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Paste("person".to_owned()),
        Event::Key(KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )),
        Event::Paste("example.com".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        Event::Paste("scripted-jira-secret".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        Event::Paste("scripted-tempo-secret".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ];
    if save {
        events.push(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
    } else {
        events.push(Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
    }
    events
}

fn reconfiguration_tui_events() -> Vec<Event> {
    vec![
        // Retain the stored Jira credential and verify the prefilled identity.
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        // Retain the stored Jira credential.
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        // Retain the stored Tempo credential.
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        // Return from Save and replace only the Tempo credential.
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Event::Paste("replacement-tempo-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        // Return through Tempo and Jira token to edit the verified identity.
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Event::Paste(".updated".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Paste(".updated".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Paste("replacement-jira-token".to_owned()),
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        // Jira edits require Tempo to be verified again before Save.
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ]
}

#[cfg(unix)]
fn spawn_setup_pty(
    path: &std::path::Path,
    scenario: &str,
) -> Result<OsSession, Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new("sh");
    command
        .args([
            "-c",
            "exec \"$1\" --exact app::tests::pty_setup_helper --ignored --nocapture --test-threads=1 >\"$2\"",
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
fn pty_output_path(config_path: &std::path::Path) -> PathBuf {
    config_path.with_extension("stdout.json")
}

#[cfg(unix)]
fn read_pty_json_output(
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
        "running 1 test\ntest app::tests::pty_setup_helper ..."
    );
    assert!(output[json_end..].starts_with("\nok\n\ntest result: ok."));

    Ok(serde_json::from_str(&output[json_start..json_end])?)
}

#[cfg(unix)]
fn send_paste(session: &mut OsSession, value: &str) -> Result<(), Box<dyn std::error::Error>> {
    session.send(format!("\u{1b}[200~{value}\u{1b}[201~"))?;
    Ok(())
}

#[cfg(unix)]
fn assert_terminal_restored(output: &[u8]) {
    let output = String::from_utf8_lossy(output);
    for restoration in ["\u{1b}[?2004l", "\u{1b}[?1049l", "\u{1b}[?25h"] {
        assert!(
            output.contains(restoration),
            "missing terminal restoration sequence {restoration:?}"
        );
    }
}

#[cfg(unix)]
fn expect_terminal_restoration(
    session: &mut OsSession,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let paste = session.expect("\u{1b}[?2004l")?;
    let output = paste.before().to_vec();
    session.expect("\u{1b}[?1049l")?;
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

impl ConnectionVerifier for FakeVerifier {
    fn verify_jira<'a>(
        &'a self,
        _connection: &'a JiraCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, String> {
        let error = self.jira_error.clone();
        Box::pin(async move {
            match error {
                Some(message) => Err(CliError::Api(message)),
                None => Ok("derived-account".to_owned()),
            }
        })
    }

    fn verify_tempo<'a>(
        &'a self,
        connection: &'a TempoCredentials,
        _debug: bool,
    ) -> VerificationFuture<'a, ()> {
        let account_id = connection.account_id.clone();
        let error = self.tempo_error.clone();
        let accounts = Arc::clone(&self.tempo_accounts);
        let config_update = self.config_update.clone();
        Box::pin(async move {
            accounts
                .lock()
                .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                .push(account_id);
            if let Some((path, config)) = config_update {
                config.save(&path)?;
            }
            match error {
                Some(message) => Err(CliError::Api(message)),
                None => Ok(()),
            }
        })
    }
}

fn setup_credentials() -> SetupCredentials {
    SetupCredentials {
        tempo_token: "new-tempo-token".to_owned(),
        atlassian_user_email: "new@example.com".to_owned(),
        atlassian_token: "new-jira-token".to_owned(),
        hostname: "example.atlassian.net".to_owned(),
    }
}

fn existing_config() -> Config {
    Config {
        tempo_token: Some("old-tempo-token".to_owned()),
        account_id: Some("old-account".to_owned()),
        atlassian_user_email: Some("old@example.com".to_owned()),
        atlassian_token: Some("old-jira-token".to_owned()),
        hostname: Some("old.atlassian.net".to_owned()),
        aliases: BTreeMap::from([("lunch".to_owned(), "ABC-1".to_owned())]),
    }
}

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
    assert!(saved.aliases.contains_key("lunch"));
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
    assert!(stdout.contains("test app::tests::pty_setup_helper ... ok"));
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
    let mut concurrent = initial;
    concurrent
        .aliases
        .insert("meeting".to_owned(), "ABC-3".to_owned());
    let events = Arc::new(Mutex::new(Vec::new()));
    let tempo_accounts = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_onboarding_session(
        path.clone(),
        FakeVerifier {
            jira_error: None,
            tempo_error: None,
            tempo_accounts: Arc::clone(&tempo_accounts),
            config_update: Some((path.clone(), concurrent)),
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
        saved.aliases.get("meeting").map(String::as_str),
    );
    assert_eq!(
        observed,
        (
            Some("example.atlassian.net"),
            Some("scripted@example.com"),
            Some("scripted-jira-token"),
            Some("scripted-tempo-token"),
            Some("derived-account"),
            Some("ABC-3"),
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
    assert!(saved.aliases.contains_key("lunch"));

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
    assert!(saved.aliases.contains_key("lunch"));
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
    assert_eq!(
        saved.aliases.get("lunch").map(String::as_str),
        Some("ABC-1")
    );
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
async fn verified_environment_setup_preserves_config_updates_made_during_verification(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let path = directory.path().join("config.json");
    existing_config().save(&path)?;
    let mut updated_config = existing_config();
    updated_config
        .aliases
        .insert("meeting".to_owned(), "ABC-3".to_owned());
    let app = App::with_connection_verifier(
        path.clone(),
        FakeVerifier {
            jira_error: None,
            tempo_error: None,
            tempo_accounts: Arc::new(Mutex::new(Vec::new())),
            config_update: Some((path.clone(), updated_config)),
        },
    );

    app.verify_and_save_environment_setup(EnvironmentSetupPlan::new(setup_credentials()))
        .await?;

    let saved = Config::load(&path)?;
    assert_eq!(
        saved.aliases.get("meeting").map(String::as_str),
        Some("ABC-3")
    );
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
