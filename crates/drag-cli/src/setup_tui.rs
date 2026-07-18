//! Ratatui presentation and Crossterm runtime for interactive setup.

use std::io::{self, IsTerminal};
use std::time::Duration;

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
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use tachyonfx::{
    color_from_hsl, color_to_hsl, fx, CellFilter, Effect, Interpolatable, Interpolation, SimpleRng,
};

use crate::config::normalize_jira_site;
use crate::output::escape_terminal_data;
use crate::setup::{
    setup_cancelled, BrowserLauncher, ConnectionOutcome, OnboardingFuture, OnboardingSession,
    OnboardingWorkflow, SecretInput, SystemBrowserLauncher,
};
use crate::tui_theme::{Palette, MUTED_COLOR, PRIMARY_COLOR, SUCCESS_COLOR};
use crate::CliError;

const MIN_TERMINAL_WIDTH: u16 = 84;
const MIN_TERMINAL_HEIGHT: u16 = 28;
const MAX_CONTENT_WIDTH: u16 = 100;
const MAX_FORM_WIDTH: u16 = 80;
const SPACE_SM: u16 = 1;
const SPACE_MD: u16 = 2;
const REVIEW_LABEL_WIDTH: usize = 11;
const REDUCED_MOTION_ENV: &str = "DRAG_REDUCED_MOTION";
const ANIMATION_TICK_RATE: Duration = Duration::from_millis(40);
const PLAYFUL_FRAME_RATE: Duration = Duration::from_millis(80);
const CURSOR_BLINK_HALF_PERIOD: Duration = Duration::from_millis(500);
const ANIMATION_RNG_SEED: u32 = 0x4452_4147;
const ENTRANCE_DURATION_MS: u32 = 240;
const REDUCED_MOTION_DURATION_MS: u32 = 140;
const DRAG_ART: [&str; 2] = ["█▀▄  █▀█  ▄▀█  █▀▀", "█▄▀  █▀▄  █▀█  █▄█"];
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const REVIEW_CONNECTOR_FRAMES: [&str; 4] = ["•  ▶", "─• ▶", "──•▶", "──▶ "];
const REVIEW_CONNECTOR_VERTICAL_FRAMES: [&str; 4] = ["·", "•", "▼", "▼"];
const STAGE_SEPARATOR: &str = " ─── ";
const STAGE_SEPARATOR_WIDTH: u16 = 5;
// Mirrors Exabind's selected-category perimeter pace without animating field content.
const FOCUS_BORDER_CELLS_PER_SECOND: f32 = 30.0;

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

struct AnimationTicker {
    state: Option<(tokio::time::Interval, tokio::time::Instant)>,
    active: bool,
}

impl AnimationTicker {
    fn terminal() -> Self {
        let start = tokio::time::Instant::now() + ANIMATION_TICK_RATE;
        let mut interval = tokio::time::interval_at(start, ANIMATION_TICK_RATE);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self {
            state: Some((interval, tokio::time::Instant::now())),
            active: false,
        }
    }

    #[cfg(test)]
    const fn disabled() -> Self {
        Self {
            state: None,
            active: false,
        }
    }

    async fn tick(&mut self) -> Duration {
        match self.state.as_mut() {
            Some((interval, previous_tick)) => {
                interval.tick().await;
                let now = tokio::time::Instant::now();
                let elapsed = now.duration_since(*previous_tick);
                *previous_tick = now;
                elapsed
            }
            None => std::future::pending().await,
        }
    }

    fn restart(&mut self) {
        let Some((interval, previous_tick)) = self.state.as_mut() else {
            return;
        };
        let now = tokio::time::Instant::now();
        *interval = tokio::time::interval_at(now + ANIMATION_TICK_RATE, ANIMATION_TICK_RATE);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        *previous_tick = now;
    }

    fn set_active(&mut self, active: bool) {
        if active && !self.active {
            self.restart();
        }
        self.active = active;
    }
}

fn reduced_motion_requested() -> bool {
    let value = std::env::var(REDUCED_MOTION_ENV).ok();
    reduced_motion_value(value.as_deref())
}

fn reduced_motion_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        value == "1"
            || value.eq_ignore_ascii_case("true")
            || value.eq_ignore_ascii_case("yes")
            || value.eq_ignore_ascii_case("on")
    })
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
        let mut animation_ticker = AnimationTicker::terminal();
        let reduced_motion = reduced_motion_requested();
        let result = run_onboarding(
            terminal.terminal_mut(),
            &mut events,
            &mut animation_ticker,
            reduced_motion,
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
        let mut animation_ticker = AnimationTicker::disabled();
        let backend = TestBackend::new(TEST_WIDTH, TEST_HEIGHT);
        let mut terminal = Terminal::new(backend).map_err(BackendFailure::into_cli_error)?;
        let frames = std::sync::Arc::clone(&scripted.frames);

        run_onboarding(
            &mut terminal,
            &mut events,
            &mut animation_ticker,
            false,
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

struct BufferAnimation {
    effect: Option<Effect>,
    elapsed: Duration,
}

impl BufferAnimation {
    fn entrance(reduced_motion: bool) -> Self {
        let effect = if reduced_motion {
            fx::fade_from_fg(
                MUTED_COLOR,
                (REDUCED_MOTION_DURATION_MS, Interpolation::CubicOut),
            )
            .with_filter(CellFilter::AnyOf(vec![
                CellFilter::FgColor(PRIMARY_COLOR),
                CellFilter::FgColor(MUTED_COLOR),
            ]))
        } else {
            fx::coalesce((ENTRANCE_DURATION_MS, Interpolation::CubicOut))
                .with_rng(SimpleRng::new(ANIMATION_RNG_SEED))
                .with_filter(CellFilter::Text)
        };
        Self {
            effect: Some(effect),
            elapsed: Duration::ZERO,
        }
    }

    fn connection(reduced_motion: bool) -> Self {
        let effect = if reduced_motion {
            fx::fade_from_fg(
                MUTED_COLOR,
                (REDUCED_MOTION_DURATION_MS, Interpolation::CubicOut),
            )
            .with_filter(CellFilter::FgColor(SUCCESS_COLOR))
        } else {
            fx::coalesce((ENTRANCE_DURATION_MS, Interpolation::CubicOut))
                .with_rng(SimpleRng::new(ANIMATION_RNG_SEED))
                .with_filter(CellFilter::Text)
        };
        Self {
            effect: Some(effect),
            elapsed: Duration::ZERO,
        }
    }

    fn reduced_connector() -> Self {
        Self {
            effect: Some(
                fx::fade_from_fg(
                    MUTED_COLOR,
                    (REDUCED_MOTION_DURATION_MS, Interpolation::CubicOut),
                )
                .with_filter(CellFilter::FgColor(PRIMARY_COLOR)),
            ),
            elapsed: Duration::ZERO,
        }
    }

    const fn is_active(&self) -> bool {
        self.effect.is_some()
    }

    fn advance(&mut self, elapsed: Duration) {
        if self.effect.is_some() {
            self.elapsed += elapsed;
        }
    }

    fn complete(&mut self) {
        self.effect = None;
        self.elapsed = Duration::ZERO;
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let Some(effect) = self.effect.as_mut() else {
            return;
        };
        effect.process(self.elapsed, frame.buffer_mut(), area);
        self.elapsed = Duration::ZERO;
        if effect.done() {
            self.effect = None;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectedService {
    Jira,
    Tempo,
}

struct ConnectionAnimation {
    service: ConnectedService,
    buffer: BufferAnimation,
}

struct ReviewConnectorAnimation {
    progress: Duration,
    reduced_fade: Option<BufferAnimation>,
}

impl ReviewConnectorAnimation {
    fn new(reduced_motion: bool) -> Self {
        Self {
            progress: Duration::ZERO,
            reduced_fade: reduced_motion.then(BufferAnimation::reduced_connector),
        }
    }

    fn is_active(&self) -> bool {
        self.reduced_fade.as_ref().map_or(
            self.progress < Duration::from_millis(ENTRANCE_DURATION_MS.into()),
            |fade| fade.is_active(),
        )
    }

    fn advance(&mut self, elapsed: Duration) {
        if let Some(fade) = self.reduced_fade.as_mut() {
            fade.advance(elapsed);
        } else {
            self.progress += elapsed;
        }
    }

    fn frame(&self, side_by_side: bool) -> &'static str {
        if self.reduced_fade.is_some() {
            return if side_by_side { "──▶" } else { "▼" };
        }
        let frame = (self.progress.as_millis() / PLAYFUL_FRAME_RATE.as_millis())
            .min((REVIEW_CONNECTOR_FRAMES.len() - 1) as u128) as usize;
        if side_by_side {
            REVIEW_CONNECTOR_FRAMES[frame]
        } else {
            REVIEW_CONNECTOR_VERTICAL_FRAMES[frame]
        }
    }

    fn render_reduced(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if let Some(fade) = self.reduced_fade.as_mut() {
            fade.render(frame, area);
        }
    }

    fn complete(&mut self) {
        self.progress = Duration::from_millis(ENTRANCE_DURATION_MS.into());
        if let Some(fade) = self.reduced_fade.as_mut() {
            fade.complete();
        }
    }
}

enum OnboardingEvent {
    Terminal(Event),
    Tick(Duration),
}

struct OnboardingModel {
    entrance_animation: BufferAnimation,
    connection_animation: Option<ConnectionAnimation>,
    review_connector_animation: Option<ReviewConnectorAnimation>,
    pending_animation_elapsed: Duration,
    focus_border_elapsed: Duration,
    cursor_blink_elapsed: Duration,
    reduced_motion: bool,
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
    jira_page_can_open: bool,
    tempo_page_can_open: bool,
    jira_page_loaded: bool,
    tempo_page_loaded: bool,
    jira_status: ConnectionStatus,
    tempo_status: ConnectionStatus,
    error: Option<String>,
    warning: Option<String>,
}

impl OnboardingModel {
    fn new(workflow: &OnboardingWorkflow<'_>, reduced_motion: bool) -> Self {
        Self {
            entrance_animation: BufferAnimation::entrance(reduced_motion),
            connection_animation: None,
            review_connector_animation: None,
            pending_animation_elapsed: Duration::ZERO,
            focus_border_elapsed: Duration::ZERO,
            cursor_blink_elapsed: Duration::ZERO,
            reduced_motion,
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
            jira_page_can_open: false,
            tempo_page_can_open: false,
            jira_page_loaded: false,
            tempo_page_loaded: false,
            jira_status: ConnectionStatus::NotConnected,
            tempo_status: ConnectionStatus::NotConnected,
            error: None,
            warning: None,
        }
    }

    fn handle_onboarding_event(&mut self, event: OnboardingEvent) -> Action {
        match event {
            OnboardingEvent::Tick(elapsed) => {
                self.entrance_animation.advance(elapsed);
                if let Some(animation) = self.connection_animation.as_mut() {
                    animation.buffer.advance(elapsed);
                }
                if let Some(animation) = self.review_connector_animation.as_mut() {
                    animation.advance(elapsed);
                }
                if self.connection_pending() {
                    self.pending_animation_elapsed += elapsed;
                }
                if self.focus_border_animation_active() {
                    self.focus_border_elapsed += elapsed;
                }
                if self.cursor_blink_animation_active() {
                    self.cursor_blink_elapsed += elapsed;
                }
                Action::None
            }
            OnboardingEvent::Terminal(event) => {
                if matches!(event, Event::Key(_)) {
                    self.entrance_animation.complete();
                    if let Some(animation) = self.connection_animation.as_mut() {
                        animation.buffer.complete();
                    }
                    if let Some(animation) = self.review_connector_animation.as_mut() {
                        animation.complete();
                    }
                }
                self.handle_event(event)
            }
        }
    }

    fn animations_active(&self) -> bool {
        self.entrance_animation.is_active()
            || self
                .connection_animation
                .as_ref()
                .is_some_and(|animation| animation.buffer.is_active())
            || self
                .review_connector_animation
                .as_ref()
                .is_some_and(ReviewConnectorAnimation::is_active)
            || (!self.reduced_motion && self.connection_pending())
            || self.focus_border_animation_active()
            || self.cursor_blink_animation_active()
    }

    fn focus_border_animation_active(&self) -> bool {
        !self.reduced_motion && self.text_input_focused()
    }

    fn cursor_blink_animation_active(&self) -> bool {
        !self.reduced_motion && self.text_input_focused()
    }

    fn text_input_focused(&self) -> bool {
        matches!(
            (self.stage, self.focus),
            (UiStage::JiraDetails, 0 | 1) | (UiStage::JiraToken | UiStage::Tempo, 0)
        )
    }

    fn cursor_visible(&self) -> bool {
        self.reduced_motion
            || (self.cursor_blink_elapsed.as_millis() / CURSOR_BLINK_HALF_PERIOD.as_millis())
                .is_multiple_of(2)
    }

    fn reset_cursor_blink(&mut self) {
        self.cursor_blink_elapsed = Duration::ZERO;
    }

    fn connection_pending(&self) -> bool {
        self.jira_status == ConnectionStatus::Pending
            || self.tempo_status == ConnectionStatus::Pending
    }

    fn start_pending_animation(&mut self) {
        self.pending_animation_elapsed = Duration::ZERO;
    }

    fn pending_symbol(&self) -> &'static str {
        if self.reduced_motion || !self.connection_pending() {
            return "…";
        }
        let frame = (self.pending_animation_elapsed.as_millis() / PLAYFUL_FRAME_RATE.as_millis())
            % SPINNER_FRAMES.len() as u128;
        SPINNER_FRAMES[frame as usize]
    }

    fn start_connection_animation(&mut self, service: ConnectedService) {
        self.connection_animation = Some(ConnectionAnimation {
            service,
            buffer: BufferAnimation::connection(self.reduced_motion),
        });
    }

    fn start_review_connector_animation(&mut self) {
        self.review_connector_animation = Some(ReviewConnectorAnimation::new(self.reduced_motion));
    }

    fn review_connector(&self, side_by_side: bool) -> &'static str {
        self.review_connector_animation.as_ref().map_or_else(
            || if side_by_side { "──▶" } else { "▼" },
            |animation| animation.frame(side_by_side),
        )
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
        self.reset_cursor_blink();
    }

    fn handle_key(&mut self, key: KeyEvent) -> Action {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Action::None;
        }
        self.reset_cursor_blink();
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
            KeyCode::Char('j' | 'J')
                if self.stage == UiStage::Save
                    && matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
            {
                Action::EditJira
            }
            KeyCode::Char('t' | 'T')
                if self.stage == UiStage::Save
                    && matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
            {
                Action::EditTempo
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
        self.reset_cursor_blink();
    }

    fn focus_previous(&mut self) {
        self.focus = (self.focus + self.focus_count() - 1) % self.focus_count();
        self.reset_cursor_blink();
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
        self.reset_cursor_blink();
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
    EditJira,
    EditTempo,
    Back,
    Cancel,
}

async fn run_onboarding<'a, B, S, O>(
    terminal: &mut Terminal<B>,
    events: &mut S,
    animation_ticker: &mut AnimationTicker,
    reduced_motion: bool,
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
    let mut model = OnboardingModel::new(&workflow, reduced_motion);
    let mut undersized = terminal_is_undersized(terminal)?;

    loop {
        draw(terminal, &mut model, &mut observe)?;
        let event = next_onboarding_event(
            events,
            animation_ticker,
            model.animations_active() && !undersized,
        )
        .await?;
        if let OnboardingEvent::Terminal(terminal_event) = &event {
            update_undersized_state(&mut undersized, terminal_event);
            if undersized && !event_allowed_while_undersized(terminal_event) {
                continue;
            }
        }
        match model.handle_onboarding_event(event) {
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
                model.start_pending_animation();
                animation_ticker.restart();
                draw(terminal, &mut model, &mut observe)?;

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
                                draw(terminal, &mut model, &mut observe)?;
                            }
                            elapsed = animation_ticker.tick(), if !undersized => {
                                model.handle_onboarding_event(OnboardingEvent::Tick(elapsed));
                                draw(terminal, &mut model, &mut observe)?;
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
                        model.start_connection_animation(ConnectedService::Jira);
                        animation_ticker.restart();
                        transition_to(&mut model, &mut workflow, browser_launcher, UiStage::Tempo)?;
                    }
                    Ok(ConnectionOutcome::Rejected(error))
                    | Err(error @ CliError::InvalidInput(_)) => {
                        model.jira_status = ConnectionStatus::NotConnected;
                        model.jira_token.clear();
                        model.focus = 0;
                        model.error = Some(format!(
                            "Could not connect to Jira: {}",
                            escape_terminal_data(&error.to_string())
                        ));
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
                model.start_pending_animation();
                animation_ticker.restart();
                draw(terminal, &mut model, &mut observe)?;

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
                                draw(terminal, &mut model, &mut observe)?;
                            }
                            elapsed = animation_ticker.tick(), if !undersized => {
                                model.handle_onboarding_event(OnboardingEvent::Tick(elapsed));
                                draw(terminal, &mut model, &mut observe)?;
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
                        model.start_connection_animation(ConnectedService::Tempo);
                        model.start_review_connector_animation();
                        animation_ticker.restart();
                        transition_to(&mut model, &mut workflow, browser_launcher, UiStage::Save)?;
                    }
                    Ok(ConnectionOutcome::Rejected(error))
                    | Err(error @ CliError::InvalidInput(_)) => {
                        model.tempo_status = ConnectionStatus::NotConnected;
                        model.tempo_token.clear();
                        model.focus = 0;
                        model.error = Some(format!(
                            "Could not connect to Tempo: {}",
                            escape_terminal_data(&error.to_string())
                        ));
                    }
                    Err(error) => return Err(error),
                }
            }
            Action::Save => return Ok(workflow),
            Action::EditJira => {
                transition_to(
                    &mut model,
                    &mut workflow,
                    browser_launcher,
                    UiStage::JiraDetails,
                )?;
            }
            Action::EditTempo => {
                transition_to(&mut model, &mut workflow, browser_launcher, UiStage::Tempo)?;
            }
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
    match stage {
        UiStage::JiraToken if !model.jira_page_loaded => {
            let page = workflow.jira_token_page()?;
            model.jira_instruction = page.instruction.to_owned();
            model.jira_url = page.url.to_string();
            model.jira_page_can_open = page.open_browser;
            model.jira_page_loaded = true;
            present_page(model, browser_launcher, &page);
        }
        UiStage::Tempo if !model.tempo_page_loaded => {
            let page = workflow.tempo_token_page()?;
            model.tempo_instruction = page.instruction.to_owned();
            model.tempo_url = page.url.to_string();
            model.tempo_page_can_open = page.open_browser;
            model.tempo_page_loaded = true;
            present_page(model, browser_launcher, &page);
        }
        UiStage::JiraDetails | UiStage::JiraToken | UiStage::Tempo | UiStage::Save => {}
    };
    Ok(())
}

fn present_page(
    model: &mut OnboardingModel,
    browser_launcher: &dyn BrowserLauncher,
    page: &crate::setup::TokenPage,
) {
    if page.open_browser {
        if let Err(error) = browser_launcher.open(page.url.as_str()) {
            model.warning = Some(format!(
                "Could not open token settings: {}. Use the URL shown below.",
                escape_terminal_data(&error.to_string())
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

async fn next_onboarding_event<S>(
    events: &mut S,
    animation_ticker: &mut AnimationTicker,
    animation_active: bool,
) -> Result<OnboardingEvent, CliError>
where
    S: Stream<Item = io::Result<Event>> + Unpin,
{
    animation_ticker.set_active(animation_active);
    if !animation_active {
        return next_event(events).await.map(OnboardingEvent::Terminal);
    }

    tokio::select! {
        biased;
        event = events.next() => event_result(event).map(OnboardingEvent::Terminal),
        elapsed = animation_ticker.tick() => Ok(OnboardingEvent::Tick(elapsed)),
    }
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
    model: &mut OnboardingModel,
    observe: &mut O,
) -> Result<(), CliError>
where
    B: Backend,
    B::Error: BackendFailure,
    O: FnMut(&Terminal<B>) -> Result<(), CliError>,
{
    // Keep the hardware cursor hidden so animated diffs cannot expose the backend's
    // intermediate cursor moves. `render_field` draws the model-driven caret instead.
    terminal
        .hide_cursor()
        .map_err(BackendFailure::into_cli_error)?;
    terminal
        .draw(|frame| render_animated(frame, model))
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

fn render_animated(frame: &mut Frame<'_>, model: &mut OnboardingModel) {
    let Some(areas) = render(frame, model) else {
        return;
    };
    model.entrance_animation.render(frame, areas.brand);
    if let Some(animation) = model.connection_animation.as_mut() {
        let area = match animation.service {
            ConnectedService::Jira => areas.jira_status,
            ConnectedService::Tempo => areas.tempo_status,
        };
        animation.buffer.render(frame, area);
    }
    if let (Some(animation), Some(connector)) = (
        model.review_connector_animation.as_mut(),
        areas.review_connector,
    ) {
        animation.render_reduced(frame, connector);
    }
    if let Some(focused_input) = areas.focused_input {
        render_focus_border(
            frame.buffer_mut(),
            focused_input,
            model.focus_border_elapsed,
        );
    }
}

struct AnimatedAreas {
    brand: Rect,
    jira_status: Rect,
    tempo_status: Rect,
    review_connector: Option<Rect>,
    focused_input: Option<Rect>,
}

fn render(frame: &mut Frame<'_>, model: &OnboardingModel) -> Option<AnimatedAreas> {
    if frame.area().width < MIN_TERMINAL_WIDTH || frame.area().height < MIN_TERMINAL_HEIGHT {
        render_resize_message(frame, frame.area());
        return None;
    }

    let [_top_padding, header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(5),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(frame.area());

    let header = constrain_content_width(header);
    let body_width = if model.stage == UiStage::Save {
        MAX_CONTENT_WIDTH
    } else {
        MAX_FORM_WIDTH
    };
    let body = constrain_width_left(constrain_content_width(body), body_width);
    let footer = constrain_content_width(footer);

    let header_areas = render_header(frame, header, model);
    let (review_connector, focused_input) = match model.stage {
        UiStage::JiraDetails => {
            let focused_input = render_jira_details(frame, body, model);
            (None, focused_input)
        }
        UiStage::JiraToken => {
            let focused_input = render_jira_token(frame, body, model);
            (None, focused_input)
        }
        UiStage::Tempo => {
            let focused_input = render_tempo(frame, body, model);
            (None, focused_input)
        }
        UiStage::Save => (Some(render_save(frame, body, model)), None),
    };
    render_footer(frame, footer, model);
    Some(AnimatedAreas {
        brand: Rect::new(header.x, header.y, header.width, 2),
        jira_status: header_areas.jira_status,
        tempo_status: header_areas.tempo_status,
        review_connector,
        focused_input: model
            .focus_border_animation_active()
            .then_some(focused_input)
            .flatten(),
    })
}

fn constrain_content_width(area: Rect) -> Rect {
    constrain_width(area, MAX_CONTENT_WIDTH)
}

fn constrain_width_left(area: Rect, maximum: u16) -> Rect {
    Rect::new(area.x, area.y, area.width.min(maximum), area.height)
}

fn constrain_width(area: Rect, maximum: u16) -> Rect {
    let width = area.width.min(maximum);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y,
        width,
        area.height,
    )
}

struct FormSpacing {
    related: u16,
    section: u16,
}

const fn form_spacing(area: Rect, spacious_height: u16) -> FormSpacing {
    if area.height >= spacious_height {
        FormSpacing {
            related: SPACE_SM,
            section: SPACE_MD,
        }
    } else if area.height >= 16 {
        FormSpacing {
            related: SPACE_SM,
            section: SPACE_SM,
        }
    } else {
        FormSpacing {
            related: 0,
            section: 0,
        }
    }
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

struct HeaderAnimatedAreas {
    jira_status: Rect,
    tempo_status: Rect,
}

fn render_header(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &OnboardingModel,
) -> HeaderAnimatedAreas {
    let pending_symbol = model.pending_symbol();
    let stages = Line::from(vec![
        stage_span(
            "Jira account",
            matches!(model.stage, UiStage::JiraDetails | UiStage::JiraToken),
            model.jira_status,
            pending_symbol,
        ),
        ratatui::text::Span::styled(STAGE_SEPARATOR, Palette::muted()),
        stage_span(
            "Tempo account",
            model.stage == UiStage::Tempo,
            model.tempo_status,
            pending_symbol,
        ),
        ratatui::text::Span::styled(STAGE_SEPARATOR, Palette::muted()),
        stage_span(
            "Review & save",
            model.stage == UiStage::Save,
            ConnectionStatus::NotConnected,
            pending_symbol,
        ),
    ]);
    let mut title = DRAG_ART
        .iter()
        .map(|line| Line::styled(*line, Palette::primary().bold()))
        .collect::<Vec<_>>();
    title.push(Line::default());
    title.push(stages);
    let title = Text::from(title);
    frame.render_widget(Paragraph::new(title), area);
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));
    let version_width = u16::try_from(version.len())
        .unwrap_or(area.width)
        .min(area.width);
    frame.render_widget(
        Paragraph::new(version).style(Palette::muted()),
        Rect::new(
            area.right().saturating_sub(version_width),
            area.y,
            version_width,
            1,
        ),
    );
    let jira_width = u16::try_from("Jira account".len() + 2).unwrap_or(area.width);
    let tempo_width = u16::try_from("Tempo account".len() + 2).unwrap_or(area.width);
    HeaderAnimatedAreas {
        jira_status: Rect::new(area.x, area.y + 3, jira_width, 1),
        tempo_status: Rect::new(
            area.x + jira_width + STAGE_SEPARATOR_WIDTH,
            area.y + 3,
            tempo_width,
            1,
        ),
    }
}

fn stage_span(
    label: &'static str,
    active: bool,
    status: ConnectionStatus,
    pending_symbol: &str,
) -> ratatui::text::Span<'static> {
    let text = match status {
        ConnectionStatus::Connected => format!("✓ {label}"),
        ConnectionStatus::Pending => format!("{pending_symbol} {label}"),
        ConnectionStatus::NotConnected if active => format!("● {label}"),
        ConnectionStatus::NotConnected => format!("○ {label}"),
    };
    let style = match status {
        ConnectionStatus::Connected => Palette::success().bold(),
        ConnectionStatus::Pending => Palette::pending().bold(),
        ConnectionStatus::NotConnected if active => Palette::primary().bold(),
        ConnectionStatus::NotConnected => Palette::muted(),
    };
    ratatui::text::Span::styled(text, style)
}

fn render_jira_details(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) -> Option<Rect> {
    let spacing = form_spacing(area, 20);
    let [intro, _, hostname, host_help, _, email, _, action, feedback, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(spacing.section),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(spacing.related),
        Constraint::Length(3),
        Constraint::Length(spacing.section),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from("Connect your Jira account").bold(),
            Line::from("Enter the Atlassian account Drag should use.").dim(),
        ])),
        intro,
    );
    frame.render_widget(
        Paragraph::new("Your Atlassian workspace address, for example company.atlassian.net").dim(),
        host_help,
    );
    render_field(
        frame,
        hostname,
        "Jira site",
        &model.hostname,
        FieldPresentation {
            focused: model.focus == 0,
            cursor_visible: model.cursor_visible(),
            invalid: model
                .error
                .as_deref()
                .is_some_and(|error| error.contains("Jira site")),
            ..FieldPresentation::default()
        },
    );
    render_field(
        frame,
        email,
        "Atlassian email",
        &model.email,
        FieldPresentation {
            focused: model.focus == 1,
            cursor_visible: model.cursor_visible(),
            invalid: model
                .error
                .as_deref()
                .is_some_and(|error| error.contains("Atlassian email")),
            ..FieldPresentation::default()
        },
    );
    render_action(
        frame,
        action,
        "Continue to API token",
        model.focus == 2,
        ConnectionStatus::NotConnected,
        model.pending_symbol(),
    );
    render_feedback(frame, feedback, model);
    match model.focus {
        0 => Some(hostname),
        1 => Some(email),
        _ => None,
    }
}

fn render_jira_token(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) -> Option<Rect> {
    let fallback_height = if !model.jira_page_can_open || model.warning.is_some() {
        3
    } else {
        0
    };
    let [intro, _, token, _, raw_url, _, status, feedback, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(fallback_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new("Connect Jira").bold(), intro);
    render_field(
        frame,
        token,
        "Atlassian API token",
        &model.jira_token,
        FieldPresentation {
            focused: model.focus == 0,
            cursor_visible: model.cursor_visible(),
            masked: true,
            can_retain_secret: model.can_retain_jira_token,
            invalid: model
                .error
                .as_deref()
                .is_some_and(|error| error.contains("Atlassian API token")),
        },
    );
    render_token_url_fallback(
        frame,
        raw_url,
        &model.jira_instruction,
        &model.jira_url,
        model.jira_page_can_open,
        model.warning.is_some(),
    );
    render_action(
        frame,
        status,
        "Connect Jira",
        model.focus == 1,
        model.jira_status,
        model.pending_symbol(),
    );
    render_feedback(frame, feedback, model);
    (model.focus == 0).then_some(token)
}

fn render_tempo(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) -> Option<Rect> {
    let fallback_height = if !model.tempo_page_can_open || model.warning.is_some() {
        3
    } else {
        0
    };
    let [intro, _, token, _, raw_url, _, status, feedback, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(fallback_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new("Connect Tempo").bold(), intro);
    render_field(
        frame,
        token,
        "Tempo API token",
        &model.tempo_token,
        FieldPresentation {
            focused: model.focus == 0,
            cursor_visible: model.cursor_visible(),
            masked: true,
            can_retain_secret: model.can_retain_tempo_token,
            invalid: model
                .error
                .as_deref()
                .is_some_and(|error| error.contains("Tempo API token")),
        },
    );
    render_token_url_fallback(
        frame,
        raw_url,
        &model.tempo_instruction,
        &model.tempo_url,
        model.tempo_page_can_open,
        model.warning.is_some(),
    );
    render_action(
        frame,
        status,
        "Connect Tempo",
        model.focus == 1,
        model.tempo_status,
        model.pending_symbol(),
    );
    render_feedback(frame, feedback, model);
    (model.focus == 0).then_some(token)
}

fn render_token_url_fallback(
    frame: &mut Frame<'_>,
    area: Rect,
    instruction: &str,
    url: &str,
    can_open: bool,
    open_failed: bool,
) {
    if !can_open || open_failed {
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(instruction.dim()),
                Line::from(url.underlined()),
            ]))
            .wrap(Wrap { trim: false }),
            area,
        );
    }
}

fn render_save(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) -> Rect {
    let side_by_side = area.width >= 96;
    let manifest_height = if side_by_side { 6 } else { 13 };
    let [intro, _, manifest, _, action, feedback, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(manifest_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from("Ready to save").bold(),
            Line::from("Confirm the Jira account and Tempo workspace DRAG will connect.").dim(),
        ])),
        intro,
    );
    let connector = render_connection_manifest(frame, manifest, model, side_by_side);
    render_action(
        frame,
        constrain_width_left(action, 26),
        "Save configuration",
        true,
        ConnectionStatus::Connected,
        model.pending_symbol(),
    );
    render_feedback(frame, feedback, model);
    connector
}

fn render_connection_manifest(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &OnboardingModel,
    side_by_side: bool,
) -> Rect {
    let jira_details = || {
        vec![
            detail_line("Site", &model.hostname),
            detail_line("Account", &model.email),
            Line::default(),
            edit_line("J", "Edit Jira account"),
        ]
    };

    if side_by_side {
        let [jira, connector, tempo] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(4),
            Constraint::Fill(1),
        ])
        .areas(area);
        render_connection_endpoint(frame, jira, "JIRA", jira_details());
        let connector = Rect::new(connector.x, connector.y + 2, connector.width, 1);
        frame.render_widget(
            Paragraph::new(model.review_connector(true))
                .centered()
                .style(Palette::primary().bold()),
            connector,
        );
        render_tempo_endpoint(frame, tempo, model);
        connector
    } else {
        let [jira, connector, tempo] = Layout::vertical([
            Constraint::Length(6),
            Constraint::Length(1),
            Constraint::Length(6),
        ])
        .areas(area);
        render_connection_endpoint(frame, jira, "JIRA", jira_details());
        frame.render_widget(
            Paragraph::new(model.review_connector(false))
                .centered()
                .style(Palette::primary().bold()),
            connector,
        );
        render_tempo_endpoint(frame, tempo, model);
        connector
    }
}

fn render_tempo_endpoint(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) {
    render_connection_endpoint(
        frame,
        area,
        "TEMPO",
        vec![
            detail_line("Workspace", &model.hostname),
            styled_detail_line("Credential", "Verified", Palette::success()),
            Line::default(),
            edit_line("T", "Edit Tempo token"),
        ],
    );
}

fn render_connection_endpoint(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &'static str,
    details: Vec<Line<'_>>,
) {
    let title = Line::from(vec![
        ratatui::text::Span::styled(format!(" {label}  "), Palette::primary().bold()),
        ratatui::text::Span::styled("✓ connected ", Palette::success()),
    ]);
    frame.render_widget(
        Paragraph::new(Text::from(details)).block(
            Block::bordered()
                .title(title)
                .border_style(Palette::muted()),
        ),
        area,
    );
}

fn detail_line<'a>(label: &'static str, value: &'a str) -> Line<'a> {
    styled_detail_line(label, value, Style::new())
}

fn styled_detail_line<'a>(label: &'static str, value: &'a str, value_style: Style) -> Line<'a> {
    Line::from(vec![
        ratatui::text::Span::styled(format!("{label:<REVIEW_LABEL_WIDTH$}"), Palette::muted()),
        ratatui::text::Span::styled(value, value_style),
    ])
}

fn edit_line(shortcut: &'static str, label: &'static str) -> Line<'static> {
    Line::from(vec![
        ratatui::text::Span::styled(format!("{shortcut}  "), Palette::primary().bold()),
        ratatui::text::Span::styled(label, Palette::muted()),
    ])
}

#[derive(Default)]
struct FieldPresentation {
    focused: bool,
    cursor_visible: bool,
    masked: bool,
    can_retain_secret: bool,
    invalid: bool,
}

fn render_field(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    value: &str,
    presentation: FieldPresentation,
) {
    let FieldPresentation {
        focused,
        cursor_visible,
        masked,
        can_retain_secret,
        invalid,
    } = presentation;
    let retained = masked && value.is_empty() && can_retain_secret;
    let display = if retained {
        "••••••••••••".to_owned()
    } else if masked {
        "•".repeat(value.chars().count())
    } else {
        value.to_owned()
    };
    let border_style = if invalid {
        Palette::error()
    } else if focused {
        Palette::focus()
    } else {
        Palette::muted()
    };
    let title = if invalid {
        format!(" ✕ {label} (invalid) ")
    } else if focused && retained {
        format!(" › {label} (stored) ")
    } else if focused {
        format!(" › {label} ")
    } else if retained {
        format!(" {label} (stored) ")
    } else {
        format!(" {label} ")
    };
    let block = Block::bordered().title(title).border_style(border_style);
    frame.render_widget(Paragraph::new(display.as_str()).block(block), area);

    if focused && cursor_visible && area.width > 2 && !retained {
        let cursor_offset = display
            .chars()
            .count()
            .min(usize::from(area.width.saturating_sub(3))) as u16;
        if let Some(cell) = frame
            .buffer_mut()
            .cell_mut(Position::new(area.x + 1 + cursor_offset, area.y + 1))
        {
            cell.set_style(Style::new().reversed());
        }
    }
}

fn render_focus_border(buffer: &mut Buffer, area: Rect, elapsed: Duration) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let phase = (elapsed.as_secs_f32() * FOCUS_BORDER_CELLS_PER_SECOND) as usize;
    let mut border_index = 0;

    for x in area.x..area.right() {
        tint_focus_border_cell(buffer, Position::new(x, area.y), phase + border_index);
        border_index += 1;
    }
    for y in area.y + 1..area.bottom() - 1 {
        tint_focus_border_cell(
            buffer,
            Position::new(area.right() - 1, y),
            phase + border_index,
        );
        border_index += 1;
    }
    for x in (area.x..area.right()).rev() {
        tint_focus_border_cell(
            buffer,
            Position::new(x, area.bottom() - 1),
            phase + border_index,
        );
        border_index += 1;
    }
    for y in (area.y + 1..area.bottom() - 1).rev() {
        tint_focus_border_cell(buffer, Position::new(area.x, y), phase + border_index);
        border_index += 1;
    }
}

fn tint_focus_border_cell(buffer: &mut Buffer, position: Position, color_index: usize) {
    let Some(cell) = buffer.cell_mut(position) else {
        return;
    };
    if matches!(
        cell.symbol(),
        "─" | "│" | "┌" | "┐" | "└" | "┘" | "━" | "┃" | "┏" | "┓" | "┗" | "┛"
    ) {
        cell.set_fg(focus_border_color(color_index));
    }
}

fn focus_border_color(index: usize) -> Color {
    let (hue, saturation, lightness) = color_to_hsl(&PRIMARY_COLOR);
    let stops = [
        (4, color_from_hsl(hue, saturation, 40.0)),
        (2, color_from_hsl(hue, saturation, 80.0)),
        (
            4,
            color_from_hsl(
                (hue - 25.0).rem_euclid(360.0),
                saturation,
                (lightness + 10.0).min(100.0),
            ),
        ),
        (
            7,
            color_from_hsl(
                hue,
                (saturation - 20.0).max(0.0),
                (lightness + 10.0).min(100.0),
            ),
        ),
        (
            7,
            color_from_hsl(
                (hue + 25.0).rem_euclid(360.0),
                saturation,
                (lightness + 10.0).min(100.0),
            ),
        ),
        (
            7,
            color_from_hsl(
                hue,
                (saturation + 20.0).min(100.0),
                (lightness + 10.0).min(100.0),
            ),
        ),
    ];
    let cycle_length = stops.iter().map(|(length, _)| length).sum::<usize>();
    let mut offset = index % cycle_length;
    let mut previous = PRIMARY_COLOR;
    for (length, target) in stops {
        if offset < length {
            return previous.lerp(&target, offset as f32 / length as f32);
        }
        offset -= length;
        previous = target;
    }
    PRIMARY_COLOR
}

fn render_action(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    focused: bool,
    status: ConnectionStatus,
    pending_symbol: &str,
) {
    let focused_action = focused
        && status != ConnectionStatus::Pending
        && (status == ConnectionStatus::NotConnected || label == "Save configuration");
    let text = match status {
        ConnectionStatus::Pending => format!("{pending_symbol} Verifying {label}…"),
        ConnectionStatus::Connected if label != "Save configuration" => {
            format!("✓ {label} connected")
        }
        _ => format!("{label}  →"),
    };
    let style = if status == ConnectionStatus::Pending {
        Palette::pending().bold()
    } else if focused_action {
        Palette::action_focus().bold()
    } else if status == ConnectionStatus::Connected {
        Palette::success().bold()
    } else {
        Palette::muted()
    };
    let line = if focused_action {
        Line::from(vec![
            ratatui::text::Span::styled("▌", Palette::primary().bold()),
            ratatui::text::Span::styled(format!(" {text} "), style),
            ratatui::text::Span::styled("▐", Palette::primary().bold()),
        ])
    } else {
        Line::styled(format!("  {text}"), style)
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_feedback(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) {
    let line = if let Some(error) = &model.error {
        Line::styled(format!("✕ Error: {error}"), Palette::error())
    } else if let Some(warning) = &model.warning {
        Line::styled(format!("! Warning: {warning}"), Palette::warning())
    } else {
        Line::default()
    };
    frame.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), area);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, model: &OnboardingModel) {
    if model.stage == UiStage::Save {
        let footer = Text::from(vec![
            Line::styled("─".repeat(usize::from(area.width)), Palette::muted()),
            Line::from(vec![
                ratatui::text::Span::styled(" J ", Palette::primary().bold()),
                ratatui::text::Span::styled("edit Jira  ", Palette::muted()),
                ratatui::text::Span::styled(" T ", Palette::primary().bold()),
                ratatui::text::Span::styled("edit Tempo  ", Palette::muted()),
                ratatui::text::Span::styled(" Enter ", Palette::primary().bold()),
                ratatui::text::Span::styled("save  ", Palette::muted()),
                ratatui::text::Span::styled(" Esc ", Palette::muted().bold()),
                ratatui::text::Span::styled("edit Tempo", Palette::muted()),
            ]),
        ]);
        frame.render_widget(Paragraph::new(footer), area);
        return;
    }

    let action = match model.stage {
        UiStage::JiraDetails if model.focus < 2 => "next",
        UiStage::JiraDetails => "continue to API token",
        UiStage::JiraToken if model.jira_status == ConnectionStatus::Connected => "continue",
        UiStage::Tempo if model.tempo_status == ConnectionStatus::Connected => "continue",
        UiStage::JiraToken if model.focus == 0 => "next",
        UiStage::Tempo if model.focus == 0 => "next",
        UiStage::JiraToken => "connect Jira",
        UiStage::Tempo => "connect Tempo",
        UiStage::Save => "save",
    };
    let escape_action = if model.stage == UiStage::JiraDetails {
        "cancel"
    } else {
        "back"
    };
    let mut controls = vec![
        ratatui::text::Span::styled(" Tab ", Palette::muted().bold()),
        ratatui::text::Span::styled("next  ", Palette::muted()),
        ratatui::text::Span::styled(" Shift-Tab ", Palette::muted().bold()),
        ratatui::text::Span::styled("previous  ", Palette::muted()),
        ratatui::text::Span::styled(" Enter ", Palette::primary().bold()),
        ratatui::text::Span::styled(format!("{action}  "), Palette::muted()),
    ];
    controls.extend([
        ratatui::text::Span::styled(" Esc ", Palette::muted().bold()),
        ratatui::text::Span::styled(format!("{escape_action}  "), Palette::muted()),
    ]);
    let footer = Text::from(vec![
        Line::styled("─".repeat(usize::from(area.width)), Palette::muted()),
        Line::from(controls),
    ]);
    frame.render_widget(Paragraph::new(footer), area);
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
    use ratatui::style::{Color, Modifier};

    use super::{
        event_allowed_while_undersized, reduced_motion_value, test_backend_text, Action,
        BufferAnimation, ConnectionStatus, OnboardingEvent, OnboardingModel, Terminal, UiStage,
        DRAG_ART, MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH,
    };

    const fn inactive_animation() -> BufferAnimation {
        BufferAnimation {
            effect: None,
            elapsed: std::time::Duration::ZERO,
        }
    }

    fn model() -> OnboardingModel {
        OnboardingModel {
            entrance_animation: inactive_animation(),
            connection_animation: None,
            review_connector_animation: None,
            pending_animation_elapsed: std::time::Duration::ZERO,
            focus_border_elapsed: std::time::Duration::ZERO,
            cursor_blink_elapsed: std::time::Duration::ZERO,
            reduced_motion: false,
            stage: UiStage::JiraToken,
            focus: 0,
            hostname: String::new(),
            email: String::new(),
            jira_token: String::new(),
            tempo_token: String::new(),
            can_retain_jira_token: false,
            can_retain_tempo_token: false,
            jira_instruction: "Create or manage your Atlassian API token:".to_owned(),
            tempo_instruction: "Create or manage your Tempo API token:".to_owned(),
            jira_url: "https://id.atlassian.com/manage-profile/security/api-tokens".to_owned(),
            tempo_url: String::new(),
            jira_page_can_open: true,
            tempo_page_can_open: false,
            jira_page_loaded: true,
            tempo_page_loaded: false,
            jira_status: ConnectionStatus::NotConnected,
            tempo_status: ConnectionStatus::NotConnected,
            error: None,
            warning: None,
        }
    }

    #[test]
    fn ticks_advance_the_model_owned_entrance_animation() {
        let mut model = model();
        model.entrance_animation = BufferAnimation::entrance(false);

        assert!(matches!(
            model.handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(
                80
            ))),
            Action::None
        ));
        assert_eq!(
            model.entrance_animation.elapsed,
            std::time::Duration::from_millis(80)
        );
        assert!(model.entrance_animation.is_active());
    }

    #[tokio::test(start_paused = true)]
    async fn reactivated_ticker_excludes_time_spent_inactive() {
        let mut ticker = super::AnimationTicker::terminal();
        ticker.set_active(true);
        tokio::time::advance(super::ANIMATION_TICK_RATE).await;
        assert_eq!(ticker.tick().await, super::ANIMATION_TICK_RATE);

        ticker.set_active(false);
        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        ticker.set_active(true);
        tokio::time::advance(super::ANIMATION_TICK_RATE).await;

        assert_eq!(ticker.tick().await, super::ANIMATION_TICK_RATE);
    }

    #[test]
    fn independently_constructed_reveals_render_identical_frames(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut first = model();
        first.entrance_animation = BufferAnimation::entrance(false);
        first.handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(120)));

        let mut second = model();
        second.entrance_animation = BufferAnimation::entrance(false);
        second
            .handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(120)));

        assert_eq!(
            render_animation_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &mut first)?,
            render_animation_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &mut second)?
        );
        Ok(())
    }

    #[test]
    fn entrance_frames_are_deterministic_without_wall_clock_time(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut model = model();
        model.entrance_animation = BufferAnimation::entrance(false);
        let initial = render_animation_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &mut model)?;

        model.handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(120)));
        let intermediate =
            render_animation_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &mut model)?;
        let active_at_midpoint = model.entrance_animation.is_active();

        model.handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(120)));
        let completed = render_animation_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &mut model)?;

        assert!(initial.contains("Connect Jira"));
        assert!(initial.contains("● Jira account"));
        assert_ne!(initial, intermediate);
        assert!(active_at_midpoint);
        let outside_brand = |frame: &str| {
            frame
                .lines()
                .enumerate()
                .filter(|(line, _)| !matches!(line, 2 | 3))
                .map(|(_, text)| text)
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(outside_brand(&initial), outside_brand(&intermediate));
        assert_eq!(outside_brand(&intermediate), outside_brand(&completed));
        assert!(completed.contains(DRAG_ART[1]));
        assert!(completed.contains(concat!("v", env!("CARGO_PKG_VERSION"))));
        assert!(!model.entrance_animation.is_active());
        Ok(())
    }

    #[test]
    fn key_input_completes_the_animation_and_keeps_its_normal_behavior() {
        let mut model = model();
        model.entrance_animation = BufferAnimation::entrance(false);
        model.set_stage(UiStage::JiraDetails);

        let action = model.handle_onboarding_event(OnboardingEvent::Terminal(Event::Key(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        )));

        assert!(matches!(action, Action::None));
        assert!(!model.entrance_animation.is_active());
        assert_eq!(model.hostname, "x");
    }

    fn render_animation_text(
        width: u16,
        height: u16,
        model: &mut OnboardingModel,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| super::render_animated(frame, model))?;
        Ok(test_backend_text(&terminal))
    }

    fn render_text(
        width: u16,
        height: u16,
        model: &OnboardingModel,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| {
            let _ = super::render(frame, model);
        })?;
        Ok(test_backend_text(&terminal))
    }

    fn rendered_color(
        model: &OnboardingModel,
        symbol: &str,
    ) -> Result<Color, Box<dyn std::error::Error>> {
        let mut terminal =
            Terminal::new(TestBackend::new(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT))?;
        terminal.draw(|frame| {
            let _ = super::render(frame, model);
        })?;
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .find(|cell| cell.symbol() == symbol)
            .map(|cell| cell.fg)
            .ok_or_else(|| format!("rendered symbol {symbol:?} was not found").into())
    }

    fn rendered_animation_color(
        model: &mut OnboardingModel,
        symbol: &str,
    ) -> Result<Color, Box<dyn std::error::Error>> {
        let mut terminal =
            Terminal::new(TestBackend::new(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT))?;
        terminal.draw(|frame| super::render_animated(frame, model))?;
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .find(|cell| cell.symbol() == symbol)
            .map(|cell| cell.fg)
            .ok_or_else(|| format!("rendered symbol {symbol:?} was not found").into())
    }

    fn rendered_cursor_cells(
        model: &mut OnboardingModel,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let mut terminal =
            Terminal::new(TestBackend::new(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT))?;
        terminal.draw(|frame| super::render_animated(frame, model))?;
        Ok(terminal
            .backend()
            .buffer()
            .content
            .iter()
            .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
            .count())
    }

    #[test]
    fn reduced_motion_uses_a_short_color_only_brand_transition(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut model = model();
        model.entrance_animation = BufferAnimation::entrance(true);

        assert_eq!(
            rendered_animation_color(&mut model, "█")?,
            Color::Rgb(101, 92, 82)
        );

        model.handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(140)));
        assert_eq!(
            rendered_animation_color(&mut model, "█")?,
            Color::Rgb(116, 39, 127)
        );
        assert!(!model.entrance_animation.is_active());
        Ok(())
    }

    #[test]
    fn reduced_motion_environment_accepts_only_explicit_truthy_values() {
        for value in [Some("1"), Some("true"), Some("YES"), Some("on")] {
            assert!(reduced_motion_value(value));
        }
        for value in [None, Some(""), Some("0"), Some("false"), Some("no")] {
            assert!(!reduced_motion_value(value));
        }
    }

    #[test]
    fn supported_terminal_shows_branded_lockup_and_package_version(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut branded = model();
        for stage in [
            UiStage::JiraDetails,
            UiStage::JiraToken,
            UiStage::Tempo,
            UiStage::Save,
        ] {
            branded.set_stage(stage);
            let rendered = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &branded)?;
            assert!(rendered.contains("█▀▄  █▀█  ▄▀█  █▀▀"));
            assert!(rendered.contains(concat!("v", env!("CARGO_PKG_VERSION"))));
        }
        assert_eq!(rendered_color(&branded, "█")?, Color::Rgb(116, 39, 127));
        Ok(())
    }

    #[test]
    fn focused_action_highlight_wraps_only_its_label() -> Result<(), Box<dyn std::error::Error>> {
        let mut model = model();
        model.set_stage(UiStage::JiraDetails);
        model.focus = 2;
        let mut terminal =
            Terminal::new(TestBackend::new(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT))?;
        terminal.draw(|frame| {
            let _ = super::render(frame, &model);
        })?;

        let highlighted = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .filter(|cell| cell.bg == Color::Rgb(116, 39, 127))
            .count();
        assert!(highlighted > 0);
        assert!(highlighted < usize::from(super::MAX_CONTENT_WIDTH / 2));
        Ok(())
    }

    #[test]
    fn form_column_shares_the_header_left_edge() -> Result<(), Box<dyn std::error::Error>> {
        let rendered = render_text(super::TEST_WIDTH, super::TEST_HEIGHT, &model())?;
        let heading = rendered
            .lines()
            .find(|line| line.contains("Connect Jira"))
            .ok_or("Connect Jira heading was not rendered")?;

        assert!(heading.starts_with("Connect Jira"));
        Ok(())
    }

    #[test]
    fn fields_use_non_color_cues_and_semantic_state_colors(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut focused = model();
        let focused_text = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &focused)?;
        assert!(focused_text.contains("Atlassian API token"));
        assert!(focused_text.contains("› Atlassian API token"));
        assert_eq!(rendered_color(&focused, "›")?, Color::Rgb(116, 39, 127));

        focused.error = Some("Atlassian API token is required.".to_owned());
        let invalid_text = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &focused)?;
        assert!(invalid_text.contains("✕ Atlassian API token (invalid)"));
        assert_eq!(rendered_color(&focused, "✕")?, Color::Red);

        focused.error = None;
        focused.can_retain_jira_token = true;
        focused.focus = 1;
        let retained_text = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &focused)?;
        assert!(retained_text.contains("Atlassian API token (stored)"));
        assert!(retained_text.contains("••••"));

        focused.can_retain_jira_token = false;
        focused.jira_token = "never-render-this-secret".to_owned();
        let populated_text = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &focused)?;
        assert!(populated_text.contains("Atlassian API token"));
        assert!(populated_text.contains("••••"));
        assert!(!populated_text.contains("never-render-this-secret"));
        Ok(())
    }

    #[test]
    fn actions_use_non_color_cues_and_semantic_status_colors(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut model = model();
        model.jira_status = ConnectionStatus::Pending;
        let pending = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &model)?;
        assert!(pending.contains("⠋ Verifying Connect Jira…"));
        assert_eq!(rendered_color(&model, "⠋")?, Color::Yellow);

        model.jira_status = ConnectionStatus::Connected;
        let connected = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &model)?;
        assert!(connected.contains("✓ Connect Jira connected"));
        assert_eq!(rendered_color(&model, "✓")?, Color::Rgb(0, 121, 133));

        model.jira_status = ConnectionStatus::NotConnected;
        model.error = Some("Could not connect to Jira".to_owned());
        let failed = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &model)?;
        assert!(failed.contains("✕ Error: Could not connect to Jira"));
        assert_eq!(rendered_color(&model, "✕")?, Color::Red);
        Ok(())
    }

    #[test]
    fn pending_spinner_advances_after_two_animation_ticks() {
        let mut pending = model();
        pending.jira_status = ConnectionStatus::Pending;
        pending.start_pending_animation();

        assert_eq!(pending.pending_symbol(), "⠋");
        pending
            .handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(40)));
        assert_eq!(pending.pending_symbol(), "⠋");
        pending
            .handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(40)));
        assert_eq!(pending.pending_symbol(), "⠙");
    }

    #[test]
    fn reduced_motion_keeps_pending_indicator_static() {
        let mut pending = model();
        pending.reduced_motion = true;
        pending.jira_status = ConnectionStatus::Pending;
        pending.start_pending_animation();
        pending
            .handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(240)));

        assert_eq!(pending.pending_symbol(), "…");
    }

    #[test]
    fn focused_input_border_cycles_around_the_field() -> Result<(), Box<dyn std::error::Error>> {
        let mut focused = model();

        let initial = rendered_animation_color(&mut focused, "└")?;
        focused
            .handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(80)));
        let advanced = rendered_animation_color(&mut focused, "└")?;

        assert_ne!(initial, advanced);
        Ok(())
    }

    #[test]
    fn animated_draw_uses_one_software_cursor() -> Result<(), Box<dyn std::error::Error>> {
        let mut focused = model();
        focused.set_stage(UiStage::JiraDetails);
        focused.hostname = "silvervine.atlassian.net".to_owned();
        let mut terminal =
            Terminal::new(TestBackend::new(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT))?;

        super::draw(&mut terminal, &mut focused, &mut |_| Ok(()))?;

        assert!(!terminal.backend().cursor_visible());
        assert_eq!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn software_cursor_blink_is_deterministic_and_resets_on_input(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut focused = model();

        assert_eq!(rendered_cursor_cells(&mut focused)?, 1);
        focused.handle_onboarding_event(OnboardingEvent::Tick(super::CURSOR_BLINK_HALF_PERIOD));
        assert_eq!(rendered_cursor_cells(&mut focused)?, 0);
        focused.handle_onboarding_event(OnboardingEvent::Terminal(Event::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
        ))));
        assert_eq!(rendered_cursor_cells(&mut focused)?, 1);
        Ok(())
    }

    #[test]
    fn focus_border_animation_stops_on_actions_and_under_reduced_motion() {
        let mut focused = model();
        assert!(focused.animations_active());

        focused.focus = 1;
        assert!(!focused.animations_active());

        focused.focus = 0;
        focused.reduced_motion = true;
        assert!(!focused.animations_active());
    }

    #[test]
    fn connected_service_animation_keeps_the_next_form_readable(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut connected = model();
        connected.jira_status = ConnectionStatus::Connected;
        connected.set_stage(UiStage::Tempo);
        connected.start_connection_animation(super::ConnectedService::Jira);

        let initial =
            render_animation_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &mut connected)?;
        assert!(initial.contains("Connect Tempo"));

        connected
            .handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(240)));
        let completed =
            render_animation_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &mut connected)?;
        assert!(completed.contains("✓ Jira account"));
        Ok(())
    }

    #[test]
    fn review_connector_carries_a_signal_between_static_cards(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut review = model();
        review.set_stage(UiStage::Save);
        review.start_review_connector_animation();

        let initial = render_text(super::TEST_WIDTH, super::TEST_HEIGHT, &review)?;
        assert!(initial.contains("•  ▶"));

        review
            .handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(160)));
        let traveling = render_text(super::TEST_WIDTH, super::TEST_HEIGHT, &review)?;
        assert!(traveling.contains("──•▶"));

        review.handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(80)));
        let completed = render_text(super::TEST_WIDTH, super::TEST_HEIGHT, &review)?;
        assert!(completed.contains("──▶"));
        Ok(())
    }

    #[test]
    fn review_identifies_both_endpoints_without_an_unsaved_warning(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut review = model();
        review.set_stage(UiStage::Save);
        review.hostname = "silvervine.atlassian.net".to_owned();
        review.email = "person@silvervine.example".to_owned();
        review.jira_status = ConnectionStatus::Connected;
        review.tempo_status = ConnectionStatus::Connected;

        let rendered = render_text(super::TEST_WIDTH, super::TEST_HEIGHT, &review)?;

        for expected in [
            "Ready to save",
            "JIRA",
            "TEMPO",
            "Workspace",
            "Edit Jira account",
            "Edit Tempo token",
            "Save configuration",
        ] {
            assert!(
                rendered.contains(expected),
                "missing review text: {expected}"
            );
        }
        assert!(!rendered.contains("Nothing has been saved"));

        let narrow = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &review)?;
        assert!(narrow.contains('▼'));
        assert!(narrow.contains("Save configuration"));
        Ok(())
    }

    #[test]
    fn review_shortcuts_open_the_requested_connection_stage() {
        let mut review = model();
        review.set_stage(UiStage::Save);
        assert!(matches!(
            review.handle_event(Event::Key(KeyEvent::new(
                KeyCode::Char('j'),
                KeyModifiers::NONE
            ))),
            Action::EditJira
        ));
        assert!(matches!(
            review.handle_event(Event::Key(KeyEvent::new(
                KeyCode::Char('T'),
                KeyModifiers::SHIFT
            ))),
            Action::EditTempo
        ));
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
        model.set_stage(UiStage::JiraToken);
        model.entrance_animation = BufferAnimation::entrance(false);
        model.hostname = "example.atlassian.net".to_owned();
        model.email = "person@example.com".to_owned();
        model.jira_instruction = "Create or manage your Atlassian API token".to_owned();
        model.jira_url = "https://id.atlassian.com/manage-profile/security/api-tokens".to_owned();
        assert!(matches!(
            model.handle_onboarding_event(OnboardingEvent::Tick(std::time::Duration::from_millis(
                80
            ),)),
            Action::None
        ));
        let animation_elapsed = model.entrance_animation.elapsed;
        assert!(matches!(
            model.handle_event(Event::Resize(
                MIN_TERMINAL_WIDTH - 1,
                MIN_TERMINAL_HEIGHT - 1
            )),
            Action::None
        ));
        assert!(model.stage == UiStage::JiraToken);
        assert_eq!(model.hostname, "example.atlassian.net");
        assert_eq!(model.email, "person@example.com");
        assert_eq!(model.entrance_animation.elapsed, animation_elapsed);
        assert!(model.entrance_animation.is_active());
        let small = render_text(MIN_TERMINAL_WIDTH - 1, MIN_TERMINAL_HEIGHT - 1, &model)?;
        let restored = render_text(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, &model)?;

        assert!(small.contains("Your entered setup values are preserved"));
        assert!(restored.contains("Atlassian API token"));
        assert!(restored.contains("Connect Jira"));
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
