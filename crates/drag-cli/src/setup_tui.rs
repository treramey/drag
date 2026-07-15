//! Ratatui presentation and Crossterm runtime for interactive setup.

use std::io::{self, IsTerminal};

use crossterm::cursor::Show;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::{Stream, StreamExt};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::config::normalize_jira_site;
use crate::setup::{
    setup_cancelled, BrowserLauncher, ConnectionOutcome, OnboardingFuture, OnboardingSession,
    OnboardingWorkflow, SecretInput, SystemBrowserLauncher, TokenPage,
};
use crate::CliError;

const MIN_TERMINAL_WIDTH: u16 = 84;
const MIN_TERMINAL_HEIGHT: u16 = 28;

#[cfg(test)]
const TEST_WIDTH: u16 = 100;
#[cfg(test)]
const TEST_HEIGHT: u16 = 30;

trait BackendFailure {
    fn into_cli_error(self) -> CliError;
}

impl BackendFailure for io::Error {
    fn into_cli_error(self) -> CliError {
        CliError::Io(self)
    }
}

#[cfg(test)]
impl BackendFailure for std::convert::Infallible {
    fn into_cli_error(self) -> CliError {
        match self {}
    }
}

pub(crate) struct RatatuiOnboardingSession {
    browser_launcher: Box<dyn BrowserLauncher>,
    #[cfg(test)]
    scripted: Option<ScriptedSession>,
}

#[cfg(test)]
struct ScriptedSession {
    events: std::sync::Mutex<Option<Vec<Event>>>,
    frames: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl RatatuiOnboardingSession {
    pub(crate) fn terminal() -> Self {
        Self {
            browser_launcher: Box::new(SystemBrowserLauncher),
            #[cfg(test)]
            scripted: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted(
        browser_launcher: impl BrowserLauncher + 'static,
        events: Vec<Event>,
        frames: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Self {
        Self {
            browser_launcher: Box::new(browser_launcher),
            scripted: Some(ScriptedSession {
                events: std::sync::Mutex::new(Some(events)),
                frames,
            }),
        }
    }

    async fn run_terminal<'a>(
        &'a self,
        workflow: OnboardingWorkflow<'a>,
    ) -> Result<OnboardingWorkflow<'a>, CliError> {
        let mut terminal = StderrTerminal::new()?;
        let mut events = EventStream::new();
        let result = run_onboarding(
            terminal.terminal_mut(),
            &mut events,
            workflow,
            self.browser_launcher.as_ref(),
            |_| Ok(()),
        )
        .await;
        let restore_result = terminal.restore();

        match (result, restore_result) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(CliError::Io(error)),
            (Ok(workflow), Ok(())) => Ok(workflow),
        }
    }

    #[cfg(test)]
    async fn run_scripted<'a>(
        &'a self,
        scripted: &ScriptedSession,
        workflow: OnboardingWorkflow<'a>,
    ) -> Result<OnboardingWorkflow<'a>, CliError> {
        use futures_util::stream;
        use ratatui::backend::TestBackend;

        let events = scripted
            .events
            .lock()
            .map_err(|_| CliError::Io(io::Error::other("test event lock poisoned")))?
            .take()
            .ok_or_else(|| CliError::Io(io::Error::other("scripted session already consumed")))?;
        let mut events = stream::iter(events.into_iter().map(Ok));
        let backend = TestBackend::new(TEST_WIDTH, TEST_HEIGHT);
        let mut terminal = Terminal::new(backend).map_err(BackendFailure::into_cli_error)?;
        let frames = std::sync::Arc::clone(&scripted.frames);

        run_onboarding(
            &mut terminal,
            &mut events,
            workflow,
            self.browser_launcher.as_ref(),
            move |terminal| {
                frames
                    .lock()
                    .map_err(|_| CliError::Io(io::Error::other("test frame lock poisoned")))?
                    .push(test_backend_text(terminal));
                Ok(())
            },
        )
        .await
    }
}

impl OnboardingSession for RatatuiOnboardingSession {
    fn is_terminal(&self) -> bool {
        #[cfg(test)]
        if self.scripted.is_some() {
            return true;
        }

        io::stdin().is_terminal() && io::stderr().is_terminal()
    }

    fn run<'a>(&'a self, workflow: OnboardingWorkflow<'a>) -> OnboardingFuture<'a> {
        Box::pin(async move {
            #[cfg(test)]
            if let Some(scripted) = &self.scripted {
                return self.run_scripted(scripted, workflow).await;
            }

            self.run_terminal(workflow).await
        })
    }
}

struct StderrTerminal {
    terminal: Terminal<CrosstermBackend<io::Stderr>>,
    restored: bool,
}

impl StderrTerminal {
    fn new() -> Result<Self, CliError> {
        enable_raw_mode()?;
        let mut stderr = io::stderr();
        if let Err(error) = execute!(stderr, EnterAlternateScreen, EnableBracketedPaste) {
            let _ = execute!(stderr, DisableBracketedPaste, LeaveAlternateScreen, Show);
            let _ = disable_raw_mode();
            return Err(CliError::Io(error));
        }

        match Terminal::new(CrosstermBackend::new(stderr)) {
            Ok(terminal) => Ok(Self {
                terminal,
                restored: false,
            }),
            Err(error) => {
                let mut stderr = io::stderr();
                let _ = execute!(stderr, DisableBracketedPaste, LeaveAlternateScreen, Show);
                let _ = disable_raw_mode();
                Err(CliError::Io(error))
            }
        }
    }

    fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<io::Stderr>> {
        &mut self.terminal
    }

    fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;

        let mut first_error = None;
        if let Err(error) = self.terminal.show_cursor() {
            first_error = Some(error);
        }
        if let Err(error) = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            Show
        ) {
            first_error.get_or_insert(error);
        }
        if let Err(error) = disable_raw_mode() {
            first_error.get_or_insert(error);
        }

        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for StderrTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiStage {
    JiraDetails,
    JiraToken,
    Tempo,
    Save,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectionStatus {
    NotConnected,
    Pending,
    Connected,
}

struct OnboardingModel {
    stage: UiStage,
    focus: usize,
    hostname: String,
    email: String,
    jira_token: String,
    tempo_token: String,
    can_retain_jira_token: bool,
    can_retain_tempo_token: bool,
    jira_instruction: String,
    tempo_instruction: String,
    jira_url: String,
    tempo_url: String,
    jira_page_loaded: bool,
    tempo_page_loaded: bool,
    jira_status: ConnectionStatus,
    tempo_status: ConnectionStatus,
    error: Option<String>,
    warning: Option<String>,
}

impl OnboardingModel {
    fn new(workflow: &OnboardingWorkflow<'_>) -> Self {
        Self {
            stage: UiStage::JiraDetails,
            focus: 0,
            hostname: workflow.hostname_default().unwrap_or_default().to_owned(),
            email: workflow.email_default().unwrap_or_default().to_owned(),
            jira_token: String::new(),
            tempo_token: String::new(),
            can_retain_jira_token: workflow.can_retain_jira_token(),
            can_retain_tempo_token: workflow.can_retain_tempo_token(),
            jira_instruction: String::new(),
            tempo_instruction: String::new(),
            jira_url: String::new(),
            tempo_url: String::new(),
            jira_page_loaded: false,
            tempo_page_loaded: false,
            jira_status: ConnectionStatus::NotConnected,
            tempo_status: ConnectionStatus::NotConnected,
            error: None,
            warning: None,
        }
    }

    fn handle_event(&mut self, event: Event) -> Action {
        match event {
            Event::Key(key) => self.handle_key(key),
            Event::Paste(value) => {
                if !value.is_empty() && self.push_to_focused_input(&value) {
                    self.input_changed();
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn set_stage(&mut self, stage: UiStage) {
        self.stage = stage;
        self.focus = 0;
    }

    fn handle_key(&mut self, key: KeyEvent) -> Action {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Cancel;
        }

        match key.code {
            KeyCode::Esc if self.stage == UiStage::JiraDetails => Action::Cancel,
            KeyCode::Esc => Action::Back,
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.focus_previous();
                Action::None
            }
            KeyCode::Tab => {
                self.focus_next();
                Action::None
            }
            KeyCode::BackTab => {
                self.focus_previous();
                Action::None
            }
            KeyCode::Enter => self.activate_or_advance(),
            KeyCode::Backspace => {
                if let Some(input) = self.focused_input_mut() {
                    if input.pop().is_some() {
                        self.input_changed();
                    }
                }
                Action::None
            }
            KeyCode::Char(character) if text_input_modifiers(key.modifiers) => {
                if self.push_to_focused_input(character.encode_utf8(&mut [0; 4])) {
                    self.input_changed();
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn focus_count(&self) -> usize {
        match self.stage {
            UiStage::JiraDetails => 3,
            UiStage::JiraToken => 2,
            UiStage::Tempo => 2,
            UiStage::Save => 1,
        }
    }

    fn focus_next(&mut self) {
        self.focus = (self.focus + 1) % self.focus_count();
    }

    fn focus_previous(&mut self) {
        self.focus = (self.focus + self.focus_count() - 1) % self.focus_count();
    }

    fn focused_input_mut(&mut self) -> Option<&mut String> {
        match (self.stage, self.focus) {
            (UiStage::JiraDetails, 0) => Some(&mut self.hostname),
            (UiStage::JiraDetails, 1) => Some(&mut self.email),
            (UiStage::JiraToken, 0) => Some(&mut self.jira_token),
            (UiStage::Tempo, 0) => Some(&mut self.tempo_token),
            _ => None,
        }
    }

    fn push_to_focused_input(&mut self, value: &str) -> bool {
        let Some(input) = self.focused_input_mut() else {
            return false;
        };
        input.push_str(value);
        true
    }

    fn input_changed(&mut self) {
        match self.stage {
            UiStage::JiraDetails | UiStage::JiraToken => {
                self.jira_status = ConnectionStatus::NotConnected;
                self.tempo_status = ConnectionStatus::NotConnected;
            }
            UiStage::Tempo => self.tempo_status = ConnectionStatus::NotConnected,
            UiStage::Save => {}
        }
        self.error = None;
    }

    fn activate_or_advance(&mut self) -> Action {
        match self.stage {
            UiStage::JiraDetails if self.focus == 2 => Action::Continue,
            UiStage::JiraToken if self.focus == 1 => Action::ConnectJira,
            UiStage::Tempo if self.focus == 1 => Action::ConnectTempo,
            UiStage::Save => Action::Save,
            _ => {
                self.focus_next();
                Action::None
            }
        }
    }

    fn validate_jira_details(&mut self) -> bool {
        if self.hostname.trim().is_empty() {
            self.focus = 0;
            self.error = Some(
                "Jira site is required. Enter a bare hostname or an HTTPS Jira URL.".to_owned(),
            );
            return false;
        }
        if let Err(error) = normalize_jira_site(&self.hostname) {
            self.focus = 0;
            self.error = Some(format!(
                "Invalid Jira site: {error}. Enter a bare hostname or an HTTPS Jira URL."
            ));
            return false;
        }
        if self.email.trim().is_empty() {
            self.focus = 1;
            self.error = Some("Atlassian email is required.".to_owned());
            return false;
        }
        true
    }

    fn validate_jira_token(&mut self) -> bool {
        if self.jira_token.trim().is_empty() && !self.can_retain_jira_token {
            self.focus = 0;
            self.error = Some("Atlassian API token is required.".to_owned());
            return false;
        }
        true
    }

    fn validate_tempo(&mut self) -> bool {
        if self.tempo_token.trim().is_empty() && !self.can_retain_tempo_token {
            self.focus = 0;
            self.error = Some("Tempo API token is required.".to_owned());
            return false;
        }
        true
    }

    fn pending_cancel(event: &Event) -> bool {
        match event {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
            }
            _ => false,
        }
    }

    fn pending_back(event: &Event) -> bool {
        matches!(
            event,
            Event::Key(key)
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                    && key.code == KeyCode::Esc
        )
    }
}

fn text_input_modifiers(modifiers: KeyModifiers) -> bool {
    matches!(modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
        || modifiers == KeyModifiers::CONTROL | KeyModifiers::ALT
        || modifiers == KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT
}

fn event_allowed_while_undersized(event: &Event) -> bool {
    matches!(event, Event::Resize(_, _)) || OnboardingModel::pending_cancel(event)
}

fn update_undersized_state(undersized: &mut bool, event: &Event) {
    if let Event::Resize(width, height) = event {
        *undersized = size_is_undersized(*width, *height);
    }
}

enum Action {
    None,
    Continue,
    ConnectJira,
    ConnectTempo,
    Save,
    Back,
    Cancel,
}

async fn run_onboarding<'a, B, S, O>(
    terminal: &mut Terminal<B>,
    events: &mut S,
    mut workflow: OnboardingWorkflow<'a>,
    browser_launcher: &dyn BrowserLauncher,
    mut observe: O,
) -> Result<OnboardingWorkflow<'a>, CliError>
where
    B: Backend,
    B::Error: BackendFailure,
    S: Stream<Item = io::Result<Event>> + Unpin,
    O: FnMut(&Terminal<B>) -> Result<(), CliError>,
{
    let mut model = OnboardingModel::new(&workflow);
    let mut undersized = terminal_is_undersized(terminal)?;

    loop {
        draw(terminal, &model, &mut observe)?;
        let event = next_event(events).await?;
        update_undersized_state(&mut undersized, &event);
        if undersized && !event_allowed_while_undersized(&event) {
            continue;
        }
        match model.handle_event(event) {
            Action::None => {}
            Action::Cancel => return Err(setup_cancelled()),
            Action::Continue => match model.stage {
                UiStage::JiraDetails => {
                    if model.validate_jira_details() {
                        transition_to(
                            &mut model,
                            &mut workflow,
                            browser_launcher,
                            UiStage::JiraToken,
                        )?;
                    }
                }
                UiStage::JiraToken | UiStage::Tempo | UiStage::Save => {}
            },
            Action::Back => {
                model.error = None;
                model.warning = None;
                match model.stage {
                    UiStage::JiraDetails => return Err(setup_cancelled()),
                    UiStage::JiraToken => {
                        model.jira_token.clear();
                        transition_to(
                            &mut model,
                            &mut workflow,
                            browser_launcher,
                            UiStage::JiraDetails,
                        )?;
                    }
                    UiStage::Tempo => {
                        model.tempo_token.clear();
                        transition_to(
                            &mut model,
                            &mut workflow,
                            browser_launcher,
                            UiStage::JiraToken,
                        )?;
                    }
                    UiStage::Save => {
                        transition_to(&mut model, &mut workflow, browser_launcher, UiStage::Tempo)?;
                    }
                }
            }
            Action::ConnectJira => {
                if model.jira_status == ConnectionStatus::Connected {
                    transition_to(&mut model, &mut workflow, browser_launcher, UiStage::Tempo)?;
                    continue;
                }

                if !model.validate_jira_token() {
                    continue;
                }

                model.error = None;
                model.jira_status = ConnectionStatus::Pending;
                model.tempo_status = ConnectionStatus::NotConnected;
                draw(terminal, &model, &mut observe)?;

                let hostname = model.hostname.clone();
                let email = model.email.clone();
                let token = if model.jira_token.is_empty() && model.can_retain_jira_token {
                    SecretInput::Retain
                } else {
                    SecretInput::Replace(model.jira_token.clone())
                };
                workflow.invalidate_jira();
                let outcome = {
                    let verification = workflow.connect_jira(hostname, email, token);
                    tokio::pin!(verification);
                    loop {
                        tokio::select! {
                            biased;
                            result = &mut verification => break result,
                            event = events.next() => {
                                let event = event_result(event)?;
                                update_undersized_state(&mut undersized, &event);
                                if undersized && !event_allowed_while_undersized(&event) {
                                    continue;
                                }
                                if OnboardingModel::pending_cancel(&event)
                                    || OnboardingModel::pending_back(&event)
                                {
                                    return Err(setup_cancelled());
                                }
                                draw(terminal, &model, &mut observe)?;
                            }
                        }
                    }
                };

                match outcome {
                    Ok(ConnectionOutcome::Connected) => {
                        model.jira_status = ConnectionStatus::Connected;
                        model.jira_token.clear();
                        model.can_retain_jira_token = true;
                        model.hostname = workflow.hostname_default().unwrap_or_default().to_owned();
                        model.email = workflow.email_default().unwrap_or_default().to_owned();
                        model.tempo_page_loaded = false;
                        model.warning = None;
                        transition_to(&mut model, &mut workflow, browser_launcher, UiStage::Tempo)?;
                    }
                    Ok(ConnectionOutcome::Rejected(error))
                    | Err(error @ CliError::InvalidInput(_)) => {
                        model.jira_status = ConnectionStatus::NotConnected;
                        model.jira_token.clear();
                        model.focus = 0;
                        model.error = Some(format!("Could not connect to Jira: {error}"));
                    }
                    Err(error) => return Err(error),
                }
            }
            Action::ConnectTempo => {
                if model.tempo_status == ConnectionStatus::Connected {
                    transition_to(&mut model, &mut workflow, browser_launcher, UiStage::Save)?;
                    continue;
                }

                if !model.validate_tempo() {
                    continue;
                }

                model.error = None;
                model.tempo_status = ConnectionStatus::Pending;
                draw(terminal, &model, &mut observe)?;

                let token = if model.tempo_token.is_empty() && model.can_retain_tempo_token {
                    SecretInput::Retain
                } else {
                    SecretInput::Replace(model.tempo_token.clone())
                };
                workflow.invalidate_tempo()?;
                let outcome = {
                    let verification = workflow.connect_tempo(token);
                    tokio::pin!(verification);
                    loop {
                        tokio::select! {
                            biased;
                            result = &mut verification => break Some(result),
                            event = events.next() => {
                                let event = event_result(event)?;
                                update_undersized_state(&mut undersized, &event);
                                if undersized && !event_allowed_while_undersized(&event) {
                                    continue;
                                }
                                if OnboardingModel::pending_cancel(&event) {
                                    return Err(setup_cancelled());
                                }
                                if OnboardingModel::pending_back(&event) {
                                    break None;
                                }
                                draw(terminal, &model, &mut observe)?;
                            }
                        }
                    }
                };

                let Some(outcome) = outcome else {
                    model.tempo_token.clear();
                    model.tempo_status = ConnectionStatus::NotConnected;
                    transition_to(
                        &mut model,
                        &mut workflow,
                        browser_launcher,
                        UiStage::JiraToken,
                    )?;
                    continue;
                };

                match outcome {
                    Ok(ConnectionOutcome::Connected) => {
                        model.tempo_status = ConnectionStatus::Connected;
                        model.tempo_token.clear();
                        model.can_retain_tempo_token = true;
                        model.warning = None;
                        transition_to(&mut model, &mut workflow, browser_launcher, UiStage::Save)?;
                    }
                    Ok(ConnectionOutcome::Rejected(error))
                    | Err(error @ CliError::InvalidInput(_)) => {
                        model.tempo_status = ConnectionStatus::NotConnected;
                        model.tempo_token.clear();
                        model.focus = 0;
                        model.error = Some(format!("Could not connect to Tempo: {error}"));
                    }
                    Err(error) => return Err(error),
                }
            }
            Action::Save => return Ok(workflow),
        }
    }
}

fn transition_to(
    model: &mut OnboardingModel,
    workflow: &mut OnboardingWorkflow<'_>,
    browser_launcher: &dyn BrowserLauncher,
    stage: UiStage,
) -> Result<(), CliError> {
    model.set_stage(stage);
    enter_stage(model, workflow, browser_launcher, stage)
}

fn enter_stage(
    model: &mut OnboardingModel,
    workflow: &mut OnboardingWorkflow<'_>,
    browser_launcher: &dyn BrowserLauncher,
    stage: UiStage,
) -> Result<(), CliError> {
    let page = match stage {
        UiStage::JiraToken if !model.jira_page_loaded => {
            let page = workflow.jira_token_page()?;
            model.jira_instruction = page.instruction.to_owned();
            model.jira_url = page.url.to_string();
            model.jira_page_loaded = true;
            Some(page)
        }
        UiStage::Tempo if !model.tempo_page_loaded => {
            let page = workflow.tempo_token_page()?;
            model.tempo_instruction = page.instruction.to_owned();
            model.tempo_url = page.url.to_string();
            model.tempo_page_loaded = true;
            Some(page)
        }
        UiStage::JiraDetails | UiStage::JiraToken | UiStage::Tempo | UiStage::Save => None,
    };

    if let Some(page) = page {
        present_page(model, browser_launcher, &page);
    }
    Ok(())
}

fn present_page(
    model: &mut OnboardingModel,
    browser_launcher: &dyn BrowserLauncher,
    page: &TokenPage,
) {
    if page.open_browser {
        if let Err(error) = browser_launcher.open(&page.url) {
            model.warning = Some(format!(
                "Could not open the token page in your browser: {error}. Use the URL shown here."
            ));
        }
    }
}

async fn next_event<S>(events: &mut S) -> Result<Event, CliError>
where
    S: Stream<Item = io::Result<Event>> + Unpin,
{
    event_result(events.next().await)
}

fn event_result(event: Option<io::Result<Event>>) -> Result<Event, CliError> {
    match event {
        Some(Ok(event)) => Ok(event),
        Some(Err(error)) => Err(CliError::Io(error)),
        None => Err(CliError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "terminal event stream ended",
        ))),
    }
}

fn draw<B, O>(
    terminal: &mut Terminal<B>,
    model: &OnboardingModel,
    observe: &mut O,
) -> Result<(), CliError>
where
    B: Backend,
    B::Error: BackendFailure,
    O: FnMut(&Terminal<B>) -> Result<(), CliError>,
{
    terminal
        .draw(|frame| render(frame, model))
        .map_err(BackendFailure::into_cli_error)?;
    observe(terminal)
}

fn terminal_is_undersized<B>(terminal: &Terminal<B>) -> Result<bool, CliError>
where
    B: Backend,
    B::Error: BackendFailure,
{
    let size = terminal.size().map_err(BackendFailure::into_cli_error)?;
    Ok(size_is_undersized(size.width, size.height))
}

const fn size_is_undersized(width: u16, height: u16) -> bool {
    width < MIN_TERMINAL_WIDTH || height < MIN_TERMINAL_HEIGHT
}

fn render(frame: &mut Frame<'_>, model: &OnboardingModel) {
    if frame.area().width < MIN_TERMINAL_WIDTH || frame.area().height < MIN_TERMINAL_HEIGHT {
        render_resize_message(frame, frame.area());
        return;
    }

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(3),
    ])
    .areas(frame.area());

    render_header(frame, header, model);
    match model.stage {
        UiStage::JiraDetails => render_jira_details(frame, body, model),
        UiStage::JiraToken => render_jira_token(frame, body, model),
        UiStage::Tempo => render_tempo(frame, body, model),
        UiStage::Save => render_save(frame, body, model),
    }
    render_footer(frame, footer, model);
}

fn render_resize_message(frame: &mut Frame<'_>, area: Rect) {
    let message = Text::from(vec![
        Line::from("Terminal too small").bold(),
        Line::default(),
        Line::from(format!(
            "Current size: {} columns by {} rows.",
            area.width, area.height
        )),
        Line::from(format!(
            "Resize to at least {MIN_TERMINAL_WIDTH} columns by {MIN_TERMINAL_HEIGHT} rows to continue."
        )),
        Line::from("Your entered setup values are preserved.").dim(),
        Line::from("Ctrl-C cancels without saving.").dim(),
    ]);
    frame.render_widget(
        Paragraph::new(message)
            .centered()
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(" Drag setup ")),
        area,
    );
}

fn render_header(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) {
    let stages = Line::from(vec![
        stage_span(
            "Connect Jira",
            matches!(model.stage, UiStage::JiraDetails | UiStage::JiraToken),
            model.jira_status,
        ),
        "  →  ".dim(),
        stage_span(
            "Connect Tempo",
            model.stage == UiStage::Tempo,
            model.tempo_status,
        ),
        "  →  ".dim(),
        stage_span(
            "Save",
            model.stage == UiStage::Save,
            ConnectionStatus::NotConnected,
        ),
    ]);
    let title = Text::from(vec![Line::from("Drag setup").bold(), stages]);
    frame.render_widget(Paragraph::new(title), area);
}

fn stage_span(
    label: &'static str,
    active: bool,
    status: ConnectionStatus,
) -> ratatui::text::Span<'static> {
    let text = match status {
        ConnectionStatus::Connected => format!("✓ {label}"),
        ConnectionStatus::Pending => format!("… {label}"),
        ConnectionStatus::NotConnected if active => format!("› {label}"),
        ConnectionStatus::NotConnected => format!("○ {label}"),
    };
    if status == ConnectionStatus::Connected {
        text.green().bold()
    } else if active {
        text.cyan().bold()
    } else {
        text.dim()
    }
}

fn render_jira_details(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) {
    let [intro, hostname, email, action, feedback, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new("First, identify the Jira account you want Drag to use."),
        intro,
    );
    render_field(
        frame,
        hostname,
        "Jira site",
        &model.hostname,
        model.focus == 0,
        false,
        false,
    );
    render_field(
        frame,
        email,
        "Atlassian email",
        &model.email,
        model.focus == 1,
        false,
        false,
    );
    render_action(
        frame,
        action,
        "Continue to API token",
        model.focus == 2,
        ConnectionStatus::NotConnected,
    );
    render_feedback(frame, feedback, model);
}

fn render_jira_token(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) {
    let [intro, token, url, status, feedback, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new("Now create or enter an Atlassian API token, then connect Jira."),
        intro,
    );
    render_field(
        frame,
        token,
        "Atlassian API token",
        &model.jira_token,
        model.focus == 0,
        true,
        model.can_retain_jira_token,
    );
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(model.jira_instruction.as_str().dim()),
            Line::from(model.jira_url.as_str().underlined()),
        ]))
        .wrap(Wrap { trim: false }),
        url,
    );
    render_action(
        frame,
        status,
        "Connect Jira",
        model.focus == 1,
        model.jira_status,
    );
    render_feedback(frame, feedback, model);
}

fn render_tempo(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) {
    let [intro, token, url, status, feedback, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(4),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new("Jira is connected. Add a Tempo API token to continue."),
        intro,
    );
    render_field(
        frame,
        token,
        "Tempo API token",
        &model.tempo_token,
        model.focus == 0,
        true,
        model.can_retain_tempo_token,
    );
    let url_text = if let Some((origin, path)) = model.tempo_url.split_once("/plugins") {
        Text::from(vec![
            Line::from(model.tempo_instruction.as_str().dim()),
            Line::from(origin.to_owned().underlined()),
            Line::from(format!("/plugins{path}").underlined()),
        ])
    } else {
        Text::from(vec![
            Line::from(model.tempo_instruction.as_str().dim()),
            Line::from(model.tempo_url.as_str().underlined()),
        ])
    };
    frame.render_widget(Paragraph::new(url_text).wrap(Wrap { trim: false }), url);
    render_action(
        frame,
        status,
        "Connect Tempo",
        model.focus == 1,
        model.tempo_status,
    );
    render_feedback(frame, feedback, model);
}

fn render_save(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) {
    let [intro, review, action, feedback, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(7),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new("Review the non-secret connection details, then save explicitly."),
        intro,
    );
    let review_text = Text::from(vec![
        Line::from(vec!["Jira site: ".dim(), model.hostname.as_str().into()]),
        Line::from(vec!["Atlassian email: ".dim(), model.email.as_str().into()]),
        Line::from("✓ Jira connected".green()),
        Line::from("✓ Tempo connected".green()),
        Line::from("Nothing has been saved yet.".yellow()),
    ]);
    frame.render_widget(
        Paragraph::new(review_text).block(Block::bordered().title(" Review ")),
        review,
    );
    render_action(
        frame,
        action,
        "Save configuration",
        true,
        ConnectionStatus::Connected,
    );
    render_feedback(frame, feedback, model);
}

fn render_field(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    value: &str,
    focused: bool,
    masked: bool,
    can_retain_secret: bool,
) {
    let display = if masked && value.is_empty() && can_retain_secret {
        "Stored credential available — leave blank to retain".to_owned()
    } else if masked {
        "•".repeat(value.chars().count())
    } else {
        value.to_owned()
    };
    let border_style = if focused {
        Style::default().cyan()
    } else {
        Style::default()
    };
    let title = if focused {
        format!(" › {label} (focused) ")
    } else {
        format!(" {label} ")
    };
    let block = Block::bordered().title(title).border_style(border_style);
    frame.render_widget(Paragraph::new(display.as_str()).block(block), area);

    if focused && area.width > 2 && !(masked && value.is_empty() && can_retain_secret) {
        let cursor_offset = display
            .chars()
            .count()
            .min(usize::from(area.width.saturating_sub(3))) as u16;
        frame.set_cursor_position(Position::new(area.x + 1 + cursor_offset, area.y + 1));
    }
}

fn render_action(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    focused: bool,
    status: ConnectionStatus,
) {
    let status_text = match status {
        ConnectionStatus::Pending => format!("… Verifying {label}…"),
        ConnectionStatus::Connected if label != "Save configuration" => {
            format!("✓ {label} connected")
        }
        _ => format!("[ {label} ]"),
    };
    let text = if focused {
        format!("› {status_text} (focused)")
    } else {
        status_text
    };
    let style = if status == ConnectionStatus::Connected {
        Style::default().green().bold()
    } else if focused {
        Style::default().cyan().bold()
    } else {
        Style::default()
    };
    frame.render_widget(
        Paragraph::new(text)
            .centered()
            .style(style)
            .block(Block::bordered().border_style(style)),
        area,
    );
}

fn render_feedback(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) {
    let line = if let Some(error) = &model.error {
        Line::from(format!("Error: {error}")).red()
    } else if let Some(warning) = &model.warning {
        Line::from(format!("Warning: {warning}")).yellow()
    } else {
        Line::default()
    };
    frame.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), area);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) {
    let action = match model.stage {
        UiStage::JiraDetails => "continue to API token",
        UiStage::JiraToken if model.jira_status == ConnectionStatus::Connected => "continue",
        UiStage::Tempo if model.tempo_status == ConnectionStatus::Connected => "continue",
        UiStage::JiraToken => "connect Jira",
        UiStage::Tempo => "connect Tempo",
        UiStage::Save => "save",
    };
    let escape_action = if model.stage == UiStage::JiraDetails {
        "cancel"
    } else {
        "back"
    };
    let footer = Line::from(vec![
        " Tab ".bold().cyan(),
        "next  ".dim(),
        " Shift-Tab ".bold().cyan(),
        "previous  ".dim(),
        " Enter ".bold().cyan(),
        format!("{action}  ").dim(),
        " Esc ".bold().cyan(),
        format!("{escape_action}  ").dim(),
        " Ctrl-C ".bold().cyan(),
        "cancel".dim(),
    ]);
    frame.render_widget(Paragraph::new(footer).block(Block::bordered()), area);
}

#[cfg(test)]
fn test_backend_text(terminal: &Terminal<ratatui::backend::TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = buffer.area;
    let mut output = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            output.push_str(buffer[(x, y)].symbol());
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;

    use super::{
        event_allowed_while_undersized, test_backend_text, Action, ConnectionStatus,
        OnboardingModel, Terminal, UiStage, MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH,
    };

    fn model() -> OnboardingModel {
        OnboardingModel {
            stage: UiStage::JiraToken,
            focus: 0,
            hostname: String::new(),
            email: String::new(),
            jira_token: String::new(),
            tempo_token: String::new(),
            can_retain_jira_token: false,
            can_retain_tempo_token: false,
            jira_instruction: "Create an Atlassian token.".to_owned(),
            tempo_instruction: String::new(),
            jira_url: "https://id.atlassian.com/manage-profile/security/api-tokens".to_owned(),
            tempo_url: String::new(),
            jira_page_loaded: true,
            tempo_page_loaded: false,
            jira_status: ConnectionStatus::NotConnected,
            tempo_status: ConnectionStatus::NotConnected,
            error: None,
            warning: None,
        }
    }

    fn render_text(
        width: u16,
        height: u16,
        model: &OnboardingModel,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| super::render(frame, model))?;
        Ok(test_backend_text(&terminal))
    }

    #[test]
    fn undersized_terminal_shows_actionable_resize_message(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let rendered = render_text(MIN_TERMINAL_WIDTH - 1, MIN_TERMINAL_HEIGHT - 1, &model())?;

        assert!(rendered.contains("Terminal too small"));
        assert!(rendered.contains("Resize to at least 84 columns by 28 rows"));
        assert!(rendered.contains("Ctrl-C cancels without saving"));
        Ok(())
    }

    #[test]
    fn resize_event_preserves_entered_state_until_terminal_is_large_enough(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut model = model();
        model.set_stage(UiStage::JiraDetails);
        assert!(matches!(
            model.handle_event(Event::Paste("example.atlassian.net".to_owned())),
            Action::None
        ));
        assert!(matches!(
            model.handle_event(Event::Resize(
                MIN_TERMINAL_WIDTH - 1,
                MIN_TERMINAL_HEIGHT - 1
            )),
            Action::None
        ));
        let small = render_text(MIN_TERMINAL_WIDTH - 1, MIN_TERMINAL_HEIGHT - 1, &model)?;
        let restored = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &model)?;

        assert!(small.contains("Your entered setup values are preserved"));
        assert!(restored.contains("example.atlassian.net"));
        Ok(())
    }

    #[test]
    fn undersized_event_gate_allows_only_resize_and_ctrl_c() {
        assert!(event_allowed_while_undersized(&Event::Resize(84, 28)));
        assert!(event_allowed_while_undersized(&Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        ))));
        assert!(!event_allowed_while_undersized(&Event::Paste(
            "secret".to_owned()
        )));
        assert!(!event_allowed_while_undersized(&Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ))));
    }
}
