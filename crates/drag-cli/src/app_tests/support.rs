use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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

use super::super::{
    normalize_jira_site, setup_cancelled, App, BrowserLauncher, Config, ConnectionEnvironment,
    ConnectionOutcome, ConnectionVerifier, EnvironmentSetupPlan, JiraCredentials,
    NoopBrowserLauncher, OnboardingFuture, OnboardingSession, OnboardingWorkflow,
    RatatuiOnboardingSession, SecretInput, SetupCredentials, SetupCredentialsExt, SetupPrompter,
    TempoCredentials, VerificationFuture, ATLASSIAN_TOKEN_URL,
};
use crate::cli::{DoctorArgs, SetupArgs};
use crate::list::{
    debounce_list_fetch, take_reusable_report, AbortOnDropTask, CachedListReport, ListReport,
    ListReportAction, ListReportSession,
};
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

    fn run<'a>(&'a self, report: &'a ListReport) -> crate::list::ListReportFuture<'a> {
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
            seconds: drag::schedule::ScheduleSeconds {
                month_required: 160 * 3_600,
                month_logged: 72 * 3_600,
                month_balance: 4 * 3_600,
                day_required: 8 * 3_600,
                day_logged: 0,
            },
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
        verbose,
    )
}

mod list;

impl ConnectionEnvironment for FakeConnectionEnvironment {
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
            .ok_or_else(setup_cancelled)?;
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
            .ok_or_else(setup_cancelled)
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
            workflow.continue_from_jira_details()?;
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

mod terminal;

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
    }
}

mod doctor;

mod setup;
