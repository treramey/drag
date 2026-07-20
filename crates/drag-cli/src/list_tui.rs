//! Ratatui presentation for completed list reports.

use std::borrow::Cow;
use std::future::Future;
use std::io::{self, IsTerminal};
use std::pin::Pin;

use chrono::Datelike;
use crossterm::cursor::Show;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::calendar::{CalendarEventStore, Monthly};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};
use ratatui_braille_bar::BrailleSpinner;
use tokio::sync::Mutex;

use crate::browser::{BrowserLauncher, SystemBrowserLauncher};
use crate::list::ListReport;
use crate::output::escape_terminal_data;
use crate::tui_theme::{constrain_content_width, footer_divider, render_brand_header, Palette};
use crate::CliError;

const DASHBOARD_CALENDAR_WIDTH: u16 = 30;
const DASHBOARD_CALENDAR_HEIGHT: u16 = 13;
const DASHBOARD_MONTH_SUMMARY_HEIGHT: u16 = 3;
const DASHBOARD_DATE_HEIGHT: u16 = 2;
const DASHBOARD_DAY_SUMMARY_HEIGHT: u16 = 3;
const DASHBOARD_DETAILS_HEIGHT: u16 = 5;
const SPACIOUS_HEADER_TOP_PADDING: u16 = 2;
const SPACIOUS_HEADER_HEIGHT: u16 = 5;
const COMPACT_HEADER_TOP_PADDING: u16 = 1;
const COMPACT_HEADER_HEIGHT: u16 = 2;
const MIN_COMPACT_HEADER_HEIGHT: u16 = 20;
const MIN_SPACIOUS_HEADER_HEIGHT: u16 = 28;
const MIN_DIVIDED_FOOTER_HEIGHT: u16 = 16;

pub(crate) type ListReportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ListReportAction, CliError>> + Send + 'a>>;
pub(crate) type PendingListReportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ListReport, CliError>> + Send + 'a>>;
pub(crate) type SuspenseFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ListReportSuspenseOutcome, CliError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListReportAction {
    Close,
    PreviousDate,
    NextDate,
}

pub(crate) enum ListReportSuspenseOutcome {
    Loaded(Box<ListReport>),
    Action(ListReportAction),
}

pub(crate) trait ListReportSession: Send + Sync {
    fn is_eligible(&self) -> bool;
    fn run<'a>(&'a self, report: &'a ListReport) -> ListReportFuture<'a>;

    fn suspense<'a>(
        &'a self,
        _date: chrono::NaiveDate,
        _background: &'a ListReport,
        report: PendingListReportFuture<'a>,
    ) -> SuspenseFuture<'a> {
        Box::pin(async move {
            report
                .await
                .map(Box::new)
                .map(ListReportSuspenseOutcome::Loaded)
        })
    }
}

pub(crate) struct RatatuiListReportSession {
    browser_launcher: Box<dyn BrowserLauncher>,
    terminal: Mutex<Option<StderrTerminal>>,
}

impl RatatuiListReportSession {
    pub(crate) fn terminal() -> Self {
        Self {
            browser_launcher: Box::new(SystemBrowserLauncher),
            terminal: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn terminal_with_browser_launcher(
        browser_launcher: impl BrowserLauncher + 'static,
    ) -> Self {
        Self {
            browser_launcher: Box::new(browser_launcher),
            terminal: Mutex::new(None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Message {
    MoveUp,
    MoveDown,
    Open,
}

struct ListReportModel<'a> {
    report: &'a ListReport,
    selected_date: chrono::NaiveDate,
    table_state: TableState,
    status: Option<String>,
}

impl<'a> ListReportModel<'a> {
    fn new(report: &'a ListReport) -> Self {
        let mut table_state = TableState::default();
        table_state.select((!report.worklogs().is_empty()).then_some(0));
        Self {
            report,
            selected_date: report.selected_date(),
            table_state,
            status: None,
        }
    }

    fn loading(report: &'a ListReport, selected_date: chrono::NaiveDate) -> Self {
        Self {
            selected_date,
            ..Self::new(report)
        }
    }

    fn selected_date(&self) -> chrono::NaiveDate {
        self.selected_date
    }

    fn focused_row(&self) -> Option<usize> {
        self.table_state.selected()
    }

    fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    fn update(&mut self, message: Message, browser_launcher: &dyn BrowserLauncher) {
        let Some(focused_row) = self.focused_row() else {
            return;
        };
        if message == Message::Open {
            let worklog = &self.report.worklogs()[focused_row];
            self.status = Some(if browser_launcher.open(&worklog.link).is_ok() {
                format!("Opened {}", escape_terminal_data(&worklog.issue_key))
            } else {
                format!(
                    "Could not open {} in the browser",
                    escape_terminal_data(&worklog.issue_key)
                )
            });
            return;
        }
        let next = match message {
            Message::MoveUp => focused_row.saturating_sub(1),
            Message::MoveDown => focused_row
                .saturating_add(1)
                .min(self.report.worklogs().len().saturating_sub(1)),
            Message::Open => focused_row,
        };
        self.table_state.select(Some(next));
        self.status = None;
    }
}

impl ListReportSession for RatatuiListReportSession {
    fn is_eligible(&self) -> bool {
        io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal()
    }

    fn run<'a>(&'a self, report: &'a ListReport) -> ListReportFuture<'a> {
        Box::pin(run_terminal(
            report,
            self.browser_launcher.as_ref(),
            &self.terminal,
        ))
    }

    fn suspense<'a>(
        &'a self,
        date: chrono::NaiveDate,
        background: &'a ListReport,
        report: PendingListReportFuture<'a>,
    ) -> SuspenseFuture<'a> {
        Box::pin(run_suspense(
            date,
            background,
            report,
            self.browser_launcher.as_ref(),
            &self.terminal,
        ))
    }
}

async fn run_suspense(
    date: chrono::NaiveDate,
    background: &ListReport,
    mut report: PendingListReportFuture<'_>,
    browser_launcher: &dyn BrowserLauncher,
    terminal_state: &Mutex<Option<StderrTerminal>>,
) -> Result<ListReportSuspenseOutcome, CliError> {
    let mut terminal_state = terminal_state.lock().await;
    let terminal = terminal_state
        .as_mut()
        .ok_or_else(|| CliError::Io(io::Error::other("list terminal was not initialized")))?;
    let mut events = EventStream::new();
    let mut ticks = tokio::time::interval(std::time::Duration::from_millis(80));
    let mut model = ListReportModel::loading(background, date);
    loop {
        terminal
            .terminal
            .draw(|frame| render_suspense(frame, date, &mut model))?;
        tokio::select! {
            loaded = &mut report => {
                return loaded
                    .map(Box::new)
                    .map(ListReportSuspenseOutcome::Loaded);
            }
            _ = ticks.tick() => {}
            event = events.next() => {
                let event = event.ok_or_else(|| CliError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof, "terminal event stream ended"
                )))??;
                let Event::Key(key) = event else {
                    continue;
                };
                if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    continue;
                }
                if should_quit(key.code, key.modifiers) {
                    terminal.restore()?;
                    *terminal_state = None;
                    return Ok(ListReportSuspenseOutcome::Action(ListReportAction::Close));
                }
                if let Some(action) = date_action_for_key_event(key.code, key.kind) {
                    return Ok(ListReportSuspenseOutcome::Action(action));
                }
                if let Some(message) = message_for_key_event(key.code, key.kind) {
                    model.update(message, browser_launcher);
                }
            }
        }
    }
}

fn render_suspense(
    frame: &mut Frame<'_>,
    date: chrono::NaiveDate,
    model: &mut ListReportModel<'_>,
) {
    model.selected_date = date;
    let background = model.report;
    render(frame, model);
    let (_, report_area) = list_areas(frame.area());

    let pagination_height = u16::from(!background.pagination().totals_complete) * 2;
    let loading_area = if let Some(month_width) =
        dashboard_month_width(report_area, pagination_height, model.selected_date())
    {
        let [dashboard, _, _, _] = dashboard_areas(report_area, model, pagination_height);
        let inner = Block::bordered().inner(dashboard);
        let [_, report] =
            Layout::horizontal([Constraint::Length(month_width), Constraint::Fill(1)]).areas(inner);
        let content = horizontal_content_padding(Block::new().borders(Borders::LEFT).inner(report));
        let [_, _, _, _, _, day] = dashboard_report_areas(content, model);
        top_line(Block::new().borders(Borders::TOP).inner(day))
    } else {
        let month_height = month_height(
            report_area,
            pagination_height,
            background,
            model.selected_date(),
        );
        let details_height =
            focused_details_height(report_area.height, month_height, pagination_height, model);
        let footer_height = footer_height(report_area.height);
        let [_, _, _, _, day, _, _] = Layout::vertical([
            Constraint::Length(month_height),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(details_height),
            Constraint::Length(3),
            Constraint::Length(pagination_height),
            Constraint::Length(footer_height),
        ])
        .areas(report_area);
        top_line(Block::bordered().inner(day))
    };

    const LOADING_LABEL_WIDTH: u16 = 17;
    const SUMMARY_LOADING_GAP: u16 = 2;
    let summary_width = u16::try_from(day_summary_text(background).chars().count())
        .unwrap_or(u16::MAX)
        .saturating_add(SUMMARY_LOADING_GAP);
    let available_width = loading_area.width.saturating_sub(summary_width);
    let spinner_width =
        (loading_area.width / 4).min(available_width.saturating_sub(LOADING_LABEL_WIDTH));
    let desired_status_width = spinner_width + LOADING_LABEL_WIDTH;
    let status_width = available_width.min(desired_status_width);
    if status_width < LOADING_LABEL_WIDTH {
        render_compact_loading_status(frame, report_area);
        return;
    }
    let status = Rect::new(
        loading_area.right().saturating_sub(status_width),
        loading_area.y,
        status_width,
        loading_area.height,
    );
    frame.render_widget(Clear, status);
    let [spinner, label] = Layout::horizontal([
        Constraint::Length(spinner_width.min(status.width)),
        Constraint::Fill(1),
    ])
    .areas(status);
    frame.render_widget(
        BrailleSpinner::new().color(crate::tui_theme::PRIMARY_COLOR),
        spinner,
    );
    frame.render_widget(Paragraph::new(" Loading entries…"), label);
}

fn render_compact_loading_status(frame: &mut Frame<'_>, report_area: Rect) {
    if report_area.width == 0 || report_area.height == 0 {
        return;
    }
    let content = Rect::new(
        report_area.x,
        report_area.bottom().saturating_sub(1),
        report_area.width,
        1,
    );
    frame.render_widget(Clear, content);
    let label = if content.width >= 8 {
        "Loading…"
    } else {
        "…"
    };
    frame.render_widget(Paragraph::new(label), content);
}

fn top_line(area: Rect) -> Rect {
    Rect::new(area.x, area.y, area.width, area.height.min(1))
}

struct StderrTerminal {
    terminal: Terminal<CrosstermBackend<io::Stderr>>,
    restored: bool,
}

impl StderrTerminal {
    fn new() -> Result<Self, CliError> {
        enable_raw_mode()?;
        let mut stderr = io::stderr();
        if let Err(error) = execute!(stderr, EnterAlternateScreen) {
            let _ = execute!(stderr, LeaveAlternateScreen, Show);
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        Terminal::new(CrosstermBackend::new(stderr))
            .map(|terminal| Self {
                terminal,
                restored: false,
            })
            .map_err(|error| {
                let mut stderr = io::stderr();
                let _ = execute!(stderr, LeaveAlternateScreen, Show);
                let _ = disable_raw_mode();
                CliError::Io(error)
            })
    }

    fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        let mut first_error = self.terminal.show_cursor().err();
        if let Err(error) = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show) {
            first_error.get_or_insert(error);
        }
        if let Err(error) = disable_raw_mode() {
            first_error.get_or_insert(error);
        }
        match first_error {
            Some(error) => Err(error),
            None => {
                self.restored = true;
                Ok(())
            }
        }
    }
}

impl Drop for StderrTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

async fn run_terminal(
    report: &ListReport,
    browser_launcher: &dyn BrowserLauncher,
    terminal_state: &Mutex<Option<StderrTerminal>>,
) -> Result<ListReportAction, CliError> {
    let mut terminal_state = terminal_state.lock().await;
    if terminal_state.is_none() {
        *terminal_state = Some(StderrTerminal::new()?);
    }
    let terminal = terminal_state
        .as_mut()
        .ok_or_else(|| CliError::Io(io::Error::other("list terminal was not initialized")))?;
    let mut events = EventStream::new();
    let mut model = ListReportModel::new(report);
    loop {
        terminal.terminal.draw(|frame| render(frame, &mut model))?;
        let event = events.next().await.ok_or_else(|| {
            CliError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "terminal event stream ended",
            ))
        })??;
        if let Event::Key(key) = event {
            if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                continue;
            }
            if should_quit(key.code, key.modifiers) {
                terminal.restore()?;
                *terminal_state = None;
                return Ok(ListReportAction::Close);
            }
            if let Some(action) = date_action_for_key_event(key.code, key.kind) {
                return Ok(action);
            }
            if let Some(message) = message_for_key_event(key.code, key.kind) {
                model.update(message, browser_launcher);
            }
        }
    }
}

fn date_action_for_key_event(code: KeyCode, kind: KeyEventKind) -> Option<ListReportAction> {
    if kind != KeyEventKind::Press {
        return None;
    }
    match code {
        KeyCode::Char('h') => Some(ListReportAction::PreviousDate),
        KeyCode::Char('l') => Some(ListReportAction::NextDate),
        _ => None,
    }
}

fn message_for_key(code: KeyCode) -> Option<Message> {
    match code {
        KeyCode::Up | KeyCode::Char('k') => Some(Message::MoveUp),
        KeyCode::Down | KeyCode::Char('j') => Some(Message::MoveDown),
        KeyCode::Char('o') => Some(Message::Open),
        _ => None,
    }
}

fn message_for_key_event(code: KeyCode, kind: KeyEventKind) -> Option<Message> {
    let message = message_for_key(code)?;
    if message == Message::Open && kind != KeyEventKind::Press {
        return None;
    }
    Some(message)
}

fn should_quit(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Char('q') | KeyCode::Esc)
        || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
}

fn render(frame: &mut Frame<'_>, model: &mut ListReportModel<'_>) {
    let (header, report_area) = list_areas(frame.area());
    if header.height > 0 {
        render_brand_header(frame, header);
    }
    let report = model.report;
    let pagination_height = u16::from(!report.pagination().totals_complete) * 2;
    if let Some(month_width) =
        dashboard_month_width(report_area, pagination_height, model.selected_date())
    {
        render_dashboard(frame, report_area, model, pagination_height, month_width);
        return;
    }
    render_stacked(frame, report_area, model, pagination_height);
}

fn list_areas(area: Rect) -> (Rect, Rect) {
    let (top_padding, header_height) = if area.height >= MIN_SPACIOUS_HEADER_HEIGHT {
        (SPACIOUS_HEADER_TOP_PADDING, SPACIOUS_HEADER_HEIGHT)
    } else if area.height >= MIN_COMPACT_HEADER_HEIGHT {
        (COMPACT_HEADER_TOP_PADDING, COMPACT_HEADER_HEIGHT)
    } else {
        (0, 0)
    };
    let [_, header, report] = Layout::vertical([
        Constraint::Length(top_padding),
        Constraint::Length(header_height),
        Constraint::Fill(1),
    ])
    .areas(area);
    (
        constrain_content_width(header),
        constrain_content_width(report),
    )
}

fn dashboard_month_width(
    terminal_area: Rect,
    pagination_height: u16,
    selected_date: chrono::NaiveDate,
) -> Option<u16> {
    const MIN_REPORT_WIDTH: u16 = 55;
    calendar_date(selected_date)?;
    let month_width = DASHBOARD_CALENDAR_WIDTH;
    let minimum_width = month_width
        .saturating_add(MIN_REPORT_WIDTH)
        .saturating_add(2);
    let minimum_height = DASHBOARD_CALENDAR_HEIGHT
        .saturating_add(5)
        .saturating_add(footer_height(terminal_area.height))
        .saturating_add(pagination_height);
    (terminal_area.width >= minimum_width && terminal_area.height >= minimum_height)
        .then_some(month_width)
}

fn render_stacked(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &mut ListReportModel<'_>,
    pagination_height: u16,
) {
    let report = model.report;
    let month_height = month_height(area, pagination_height, report, model.selected_date());
    let details_height =
        focused_details_height(area.height, month_height, pagination_height, model);
    let footer_height = footer_height(area.height);
    let [month, date, worklogs, details, day, pagination, footer] = Layout::vertical([
        Constraint::Length(month_height),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(details_height),
        Constraint::Length(3),
        Constraint::Length(pagination_height),
        Constraint::Length(footer_height),
    ])
    .areas(area);
    render_month(frame, month, report, model.selected_date());
    frame.render_widget(
        Paragraph::new(model.selected_date().format("%A, %Y-%m-%d").to_string())
            .style(Palette::primary().bold()),
        date,
    );
    render_worklogs(frame, worklogs, model, true);
    render_focused_details(frame, details, model);
    let schedule = report.schedule();
    frame.render_widget(
        Paragraph::new(schedule.day_logged_duration.as_str()).block(
            Block::bordered()
                .title(primary("Day summary"))
                .border_style(Palette::muted()),
        ),
        day,
    );
    render_pagination_notice(frame, pagination, report);
    render_footer(frame, footer, model);
}

fn render_dashboard(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &mut ListReportModel<'_>,
    pagination_height: u16,
    month_width: u16,
) {
    let [dashboard, pagination, _, footer] = dashboard_areas(area, model, pagination_height);
    let panel = Block::bordered().border_style(Palette::muted());
    let inner = panel.inner(dashboard);
    frame.render_widget(panel, dashboard);
    let [month, report] =
        Layout::horizontal([Constraint::Length(month_width), Constraint::Fill(1)]).areas(inner);
    render_dashboard_month(frame, month, model.report, model.selected_date());
    render_dashboard_report(frame, report, model);
    render_pagination_notice(frame, pagination, model.report);
    render_footer(frame, footer, model);
}

fn dashboard_areas(area: Rect, model: &ListReportModel<'_>, pagination_height: u16) -> [Rect; 4] {
    let footer_height = footer_height(area.height);
    let available_height = area
        .height
        .saturating_sub(pagination_height.saturating_add(footer_height));
    let dashboard_height = dashboard_desired_height(model).min(available_height);
    Layout::vertical([
        Constraint::Length(dashboard_height),
        Constraint::Length(pagination_height),
        Constraint::Fill(1),
        Constraint::Length(footer_height),
    ])
    .areas(area)
}

fn dashboard_desired_height(model: &ListReportModel<'_>) -> u16 {
    let month_height = DASHBOARD_CALENDAR_HEIGHT + DASHBOARD_MONTH_SUMMARY_HEIGHT;
    let details_height = if model.report.verbose() && model.focused_row().is_some() {
        DASHBOARD_DETAILS_HEIGHT
    } else {
        0
    };
    let report_height = DASHBOARD_DATE_HEIGHT
        .saturating_add(worklogs_content_height(model.report))
        .saturating_add(details_height)
        .saturating_add(DASHBOARD_DAY_SUMMARY_HEIGHT);
    month_height.max(report_height).saturating_add(2)
}

fn worklogs_content_height(report: &ListReport) -> u16 {
    if report.worklogs().is_empty() {
        1
    } else {
        u16::try_from(report.worklogs().len())
            .unwrap_or(u16::MAX)
            .saturating_add(1)
    }
}

fn horizontal_content_padding(area: Rect) -> Rect {
    let [_, content, _] = Layout::horizontal([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    content
}

fn render_dashboard_month(
    frame: &mut Frame<'_>,
    area: Rect,
    report: &ListReport,
    selected_date: chrono::NaiveDate,
) {
    let area = horizontal_content_padding(area);
    let summary_height = DASHBOARD_MONTH_SUMMARY_HEIGHT.min(area.height);
    let [calendar_area, summary, _] = Layout::vertical([
        Constraint::Length(DASHBOARD_CALENDAR_HEIGHT.min(area.height)),
        Constraint::Length(summary_height),
        Constraint::Fill(1),
    ])
    .areas(area);
    render_large_calendar(frame, calendar_area, report, selected_date);
    let summary_block = Block::new()
        .borders(Borders::TOP)
        .border_style(Palette::muted());
    let summary_inner = summary_block.inner(summary);
    frame.render_widget(summary_block, summary);
    if !same_calendar_month(report.selected_date(), selected_date) {
        return;
    }
    let schedule = report.schedule();
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(current_period_text(&schedule.month_current_period_duration)),
            Line::from(month_totals_text(
                &schedule.month_logged_duration,
                &schedule.month_required_duration,
            )),
        ]),
        summary_inner,
    );
}

fn render_large_calendar(
    frame: &mut Frame<'_>,
    area: Rect,
    report: &ListReport,
    selected: chrono::NaiveDate,
) {
    let start = selected
        .with_day(1)
        .and_then(|first| {
            first.checked_sub_days(chrono::Days::new(u64::from(
                first.weekday().num_days_from_sunday(),
            )))
        })
        .unwrap_or(selected);
    let mut lines = vec![
        Line::styled(
            selected.format("%B %Y").to_string(),
            Palette::primary().bold(),
        )
        .centered(),
        Line::from(
            ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"]
                .into_iter()
                .map(|day| Span::styled(format!("{day:^4}"), Palette::muted().bold()))
                .collect::<Vec<_>>(),
        ),
    ];
    for week in 0..6_u64 {
        let mut days = Vec::with_capacity(7);
        for day in 0..7_u64 {
            let date = start
                .checked_add_days(chrono::Days::new(week * 7 + day))
                .unwrap_or(start);
            let style = if date == selected {
                Palette::action_focus().bold()
            } else if date == report.today() {
                Palette::focus().bold()
            } else if date.month() != selected.month() {
                Palette::muted()
            } else {
                ratatui::style::Style::new()
            };
            days.push(Span::styled(format!("{:^4}", date.day()), style));
        }
        lines.push(Line::from(days));
        if week < 5 {
            lines.push(Line::default());
        }
    }
    frame.render_widget(Text::from(lines), area);
}

fn render_dashboard_report(frame: &mut Frame<'_>, area: Rect, model: &mut ListReportModel<'_>) {
    let divider = Block::new()
        .borders(Borders::LEFT)
        .border_style(Palette::muted());
    let inner = horizontal_content_padding(divider.inner(area));
    frame.render_widget(divider, area);
    let [date, _date_gap, worklogs, details, _, day] = dashboard_report_areas(inner, model);
    frame.render_widget(
        Paragraph::new(model.selected_date().format("%A, %Y-%m-%d").to_string())
            .style(Palette::primary().bold()),
        date,
    );
    render_worklogs(frame, worklogs, model, false);
    render_focused_details(frame, details, model);
    let day_block = Block::new()
        .borders(Borders::TOP)
        .border_style(Palette::muted());
    let day_inner = day_block.inner(day);
    frame.render_widget(day_block, day);
    frame.render_widget(Line::from(day_summary_text(model.report)), day_inner);
}

fn day_summary_text(report: &ListReport) -> String {
    format!("Day summary: {}", report.schedule().day_logged_duration)
}

fn same_calendar_month(left: chrono::NaiveDate, right: chrono::NaiveDate) -> bool {
    left.year() == right.year() && left.month() == right.month()
}

fn current_period_text(duration: &str) -> Cow<'_, str> {
    duration.strip_prefix('-').map_or_else(
        || Cow::Owned(format!("{duration} current period")),
        |remaining| Cow::Owned(format!("{remaining} left")),
    )
}

fn month_totals_text(logged: &str, required: &str) -> String {
    format!("{logged} logged of {required}")
}

fn dashboard_report_areas(inner: Rect, model: &ListReportModel<'_>) -> [Rect; 6] {
    let details_height =
        if model.report.verbose() && model.focused_row().is_some() && inner.height >= 15 {
            DASHBOARD_DETAILS_HEIGHT
        } else {
            0
        };
    let reserved_height = DASHBOARD_DATE_HEIGHT
        .saturating_add(details_height)
        .saturating_add(DASHBOARD_DAY_SUMMARY_HEIGHT);
    let worklogs_height =
        worklogs_content_height(model.report).min(inner.height.saturating_sub(reserved_height));
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(worklogs_height),
        Constraint::Length(details_height),
        Constraint::Fill(1),
        Constraint::Length(DASHBOARD_DAY_SUMMARY_HEIGHT),
    ])
    .areas(inner)
}

fn month_height(
    terminal_area: Rect,
    pagination_height: u16,
    report: &ListReport,
    selected_date: chrono::NaiveDate,
) -> u16 {
    const COMPACT_MONTH_HEIGHT: u16 = 3;
    // Date heading, one bordered table row with its header, day summary, and
    // footer remain visible before the month summary expands into a calendar.
    // Verbose reports also keep enough room for the focused Jira URL.
    const PRIMARY_REPORT_HEIGHT_WITH_COMPACT_FOOTER: u16 = 9;
    const MIN_VERBOSE_DETAILS_HEIGHT: u16 = 3;
    let Some(date) = calendar_date(selected_date) else {
        return COMPACT_MONTH_HEIGHT;
    };
    let calendar = Monthly::new(date, CalendarEventStore::default())
        .show_month_header(Palette::primary())
        .show_weekdays_header(Palette::muted());
    let calendar_height = calendar.height().saturating_add(3);
    let calendar_width = calendar.width().saturating_add(2);
    let details_height = if report.verbose() && !report.worklogs().is_empty() {
        MIN_VERBOSE_DETAILS_HEIGHT
    } else {
        0
    };
    let primary_report_height = PRIMARY_REPORT_HEIGHT_WITH_COMPACT_FOOTER
        .saturating_add(footer_height(terminal_area.height).saturating_sub(1));
    if terminal_area.width >= calendar_width
        && terminal_area.height
            >= calendar_height + primary_report_height + details_height + pagination_height
    {
        calendar_height
    } else {
        COMPACT_MONTH_HEIGHT
    }
}

fn focused_details_height(
    terminal_height: u16,
    month_height: u16,
    pagination_height: u16,
    model: &ListReportModel<'_>,
) -> u16 {
    if !model.report.verbose() || model.focused_row().is_none() {
        return 0;
    }
    // Month, date, day summary, footer, pagination, and a bordered table with
    // one visible row take priority over secondary verbose details.
    let primary_report_height = 8_u16.saturating_add(footer_height(terminal_height));
    let available =
        terminal_height.saturating_sub(month_height + primary_report_height + pagination_height);
    match available {
        5.. => 5,
        3..=4 => 3,
        _ => 0,
    }
}

fn render_focused_details(frame: &mut Frame<'_>, area: Rect, model: &ListReportModel<'_>) {
    if area.height == 0 || !model.report.verbose() {
        return;
    }
    let Some(worklog) = model
        .focused_row()
        .and_then(|index| model.report.worklogs().get(index))
    else {
        return;
    };
    let title = format!(
        "Focused · {}",
        escape_terminal_data(&model.report.issue_label(worklog))
    );
    let block = Block::bordered()
        .title(primary(title))
        .border_style(Palette::muted());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let jira = Line::from(format!("Jira: {}", escape_terminal_data(&worklog.link)));
    if inner.height == 1 {
        frame.render_widget(jira, inner);
        return;
    }
    let [description, jira_line] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(format!(
            "Description: {}",
            escape_terminal_data(&worklog.description)
        ))
        .wrap(Wrap { trim: false }),
        description,
    );
    frame.render_widget(jira, jira_line);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, model: &ListReportModel<'_>) {
    let [divider, content] = if area.height >= 2 {
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area)
    } else {
        [Rect::new(area.x, area.y, area.width, 0), area]
    };
    if divider.height > 0 {
        frame.render_widget(footer_divider(divider.width), divider);
    }
    if let Some(status) = model.status() {
        frame.render_widget(Line::from(status.to_owned()), content);
        return;
    }
    let spans = if area.width >= 70 {
        vec![
            primary(" h/l "),
            muted("date  "),
            primary(" ↑/k "),
            muted("up  "),
            primary("↓/j "),
            muted("down  "),
            primary("o "),
            muted("open  "),
            primary("q "),
            muted("quit"),
            muted_bold("   Esc "),
            muted("close   "),
            muted_bold("Ctrl-C "),
            muted("exit"),
        ]
    } else if area.width >= 48 {
        vec![
            primary(" q "),
            muted("quit  "),
            primary("h/l "),
            muted("date  "),
            primary("↑/k "),
            muted("up  "),
            primary("↓/j "),
            muted("down  "),
            primary("o "),
            muted("open"),
        ]
    } else if area.width >= 32 {
        vec![
            primary(" q "),
            muted("quit "),
            primary("h/l "),
            muted("date "),
            primary("↑↓ "),
            muted("move "),
            primary("o "),
            muted("open"),
        ]
    } else if area.width >= 24 {
        vec![
            primary(" q "),
            muted("quit  "),
            primary("↑↓ "),
            muted("move  "),
            primary("o "),
            muted("open"),
        ]
    } else {
        vec![primary(" q "), muted("quit")]
    };
    frame.render_widget(Line::from(spans), content);
}

const fn footer_height(terminal_height: u16) -> u16 {
    if terminal_height >= MIN_DIVIDED_FOOTER_HEIGHT {
        2
    } else {
        1
    }
}

fn primary<'a>(content: impl Into<Cow<'a, str>>) -> Span<'a> {
    Span::styled(content, Palette::primary().bold())
}

fn muted<'a>(content: impl Into<Cow<'a, str>>) -> Span<'a> {
    Span::styled(content, Palette::muted())
}

fn muted_bold<'a>(content: impl Into<Cow<'a, str>>) -> Span<'a> {
    Span::styled(content, Palette::muted().bold())
}

fn render_pagination_notice(frame: &mut Frame<'_>, area: Rect, report: &ListReport) {
    if report.pagination().totals_complete {
        return;
    }
    let mut lines = Vec::with_capacity(2);
    if report.pagination().next.is_some() {
        lines.push(Line::from(
            "More worklogs are available; use JSON pagination metadata to continue.",
        ));
    }
    lines.push(Line::from("Totals reflect this bounded segment."));
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Palette::warning()),
        area,
    );
}

fn render_month(
    frame: &mut Frame<'_>,
    area: Rect,
    report: &ListReport,
    selected_date: chrono::NaiveDate,
) {
    let totals_are_current = same_calendar_month(report.selected_date(), selected_date);
    if area.height <= 3 {
        let summary = if totals_are_current {
            let schedule = report.schedule();
            format!(
                "{} · {}",
                month_totals_text(
                    &schedule.month_logged_duration,
                    &schedule.month_required_duration,
                ),
                current_period_text(&schedule.month_current_period_duration)
            )
        } else {
            String::new()
        };
        frame.render_widget(
            Paragraph::new(summary).block(
                Block::bordered()
                    .title(primary(selected_date.format("%B %Y").to_string()))
                    .border_style(Palette::muted()),
            ),
            area,
        );
        return;
    }
    let block = Block::bordered().border_style(Palette::muted());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [calendar_area, summary_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    let Some(selected_date) = calendar_date(selected_date) else {
        return;
    };
    let mut events = CalendarEventStore::default();
    if let Some(today) = calendar_date(report.today()) {
        events.add(today, Palette::focus().bold());
    }
    events.add(selected_date, Palette::action_focus().bold());
    let calendar = Monthly::new(selected_date, events)
        .show_month_header(Palette::primary().bold())
        .show_weekdays_header(Palette::muted().bold())
        .show_surrounding(Palette::muted());
    frame.render_widget(calendar, calendar_area);
    if !totals_are_current {
        return;
    }
    let schedule = report.schedule();
    frame.render_widget(
        Line::from(format!(
            "{} · {}",
            month_totals_text(
                &schedule.month_logged_duration,
                &schedule.month_required_duration,
            ),
            current_period_text(&schedule.month_current_period_duration)
        )),
        summary_area,
    );
}

fn calendar_date(date: chrono::NaiveDate) -> Option<time::Date> {
    time::Date::from_ordinal_date(date.year(), u16::try_from(date.ordinal()).ok()?).ok()
}

fn render_worklogs(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &mut ListReportModel<'_>,
    bordered: bool,
) {
    let report = model.report;
    if report.worklogs().is_empty() {
        let empty = Paragraph::new(if report.pagination().totals_complete {
            "No worklogs".to_owned()
        } else {
            "No worklogs in this retrieved segment".to_owned()
        })
        .centered();
        if bordered {
            frame.render_widget(
                empty.block(
                    Block::bordered()
                        .title(primary("Worklogs"))
                        .border_style(Palette::muted()),
                ),
                area,
            );
        } else {
            frame.render_widget(empty, area);
        }
        return;
    }
    let rows = report.worklogs().iter().map(|worklog| {
        let interval = worklog.interval.as_ref().map_or_else(
            || "unknown".to_owned(),
            |interval| format!("{}–{}", interval.start_time, interval.end_time),
        );
        Row::new([
            Cell::from(escape_terminal_data(&worklog.id)),
            Cell::from(interval),
            Cell::from(escape_terminal_data(&report.issue_label(worklog))),
            Cell::from(escape_terminal_data(&worklog.duration)),
        ])
    });
    let widths = if area.width >= 72 {
        [
            Constraint::Length(12),
            Constraint::Length(13),
            Constraint::Fill(1),
            Constraint::Length(10),
        ]
    } else if area.width >= 48 {
        [
            Constraint::Length(12),
            Constraint::Length(11),
            Constraint::Fill(1),
            Constraint::Length(8),
        ]
    } else {
        [
            Constraint::Length(8),
            Constraint::Length(11),
            Constraint::Fill(1),
            Constraint::Length(8),
        ]
    };
    let table = Table::new(rows, widths)
        .header(Row::new(["ID", "Time", "Issue", "Duration"]).style(Palette::muted().bold()))
        .row_highlight_style(Palette::action_focus().bold())
        .highlight_symbol("▶ ");
    let table = if bordered {
        table.block(
            Block::bordered()
                .title(primary("Worklogs"))
                .border_style(Palette::muted()),
        )
    } else {
        table
    };
    frame.render_stateful_widget(table, area, &mut model.table_state);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io;
    use std::sync::{Arc, Mutex};

    use chrono::{Datelike, NaiveDate};
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    use drag::models::{ClockInterval, ListPagination, Worklog};
    use drag::schedule::ScheduleDetails;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::{backend::TestBackend, Terminal};

    use super::{
        current_period_text, date_action_for_key_event, message_for_key, message_for_key_event,
        month_totals_text, render, render_suspense, should_quit, ListReportAction, ListReportModel,
        Message,
    };
    use crate::browser::{BrowserLauncher, NoopBrowserLauncher};
    use crate::list::ListReport;
    use crate::tui_theme::{Palette, DRAG_ART, MAX_CONTENT_WIDTH};

    struct FakeBrowserLauncher {
        opened: Arc<Mutex<Vec<String>>>,
        failure: Option<&'static str>,
    }

    impl BrowserLauncher for FakeBrowserLauncher {
        fn open(&self, url: &str) -> io::Result<()> {
            self.opened
                .lock()
                .map_err(|_| io::Error::other("browser test lock poisoned"))?
                .push(url.to_owned());
            self.failure
                .map_or(Ok(()), |message| Err(io::Error::other(message)))
        }
    }

    fn report(worklogs: Vec<Worklog>, totals_complete: bool) -> ListReport {
        report_with_verbose(worklogs, totals_complete, false)
    }

    fn report_with_verbose(
        worklogs: Vec<Worklog>,
        totals_complete: bool,
        verbose: bool,
    ) -> ListReport {
        let selected_date = NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN);
        report_with_dates(
            worklogs,
            totals_complete,
            verbose,
            selected_date,
            selected_date,
        )
    }

    fn report_with_dates(
        worklogs: Vec<Worklog>,
        totals_complete: bool,
        verbose: bool,
        selected_date: NaiveDate,
        today: NaiveDate,
    ) -> ListReport {
        ListReport::new(
            selected_date,
            worklogs,
            ScheduleDetails {
                month_required_duration: "160h".to_owned(),
                month_logged_duration: "72h".to_owned(),
                month_current_period_duration: "+4h".to_owned(),
                day_required_duration: "8h".to_owned(),
                day_logged_duration: "1h 30m".to_owned(),
            },
            ListPagination {
                selected_date: selected_date.to_string(),
                month_start: selected_date
                    .with_day(1)
                    .unwrap_or(selected_date)
                    .to_string(),
                month_end: selected_date.to_string(),
                limit: Some(100),
                page_limit: 1,
                all_pages: false,
                pages_retrieved: 1,
                records_retrieved: 1,
                records_returned: 1,
                next: (!totals_complete).then(|| "opaque-token".to_owned()),
                complete: totals_complete,
                totals_complete,
            },
            BTreeMap::from([("standup".to_owned(), "OPS-42".to_owned())]),
            verbose,
        )
        .with_today(today)
    }

    fn screen(report: &ListReport) -> String {
        let mut model = ListReportModel::new(report);
        screen_with_size(&mut model, 100, 24)
    }

    fn screen_with_size(model: &mut ListReportModel<'_>, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => match error {},
        };
        match terminal.draw(|frame| render(frame, model)) {
            Ok(_) => {}
            Err(error) => match error {},
        }
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn screen_lines_with_size(
        model: &mut ListReportModel<'_>,
        width: u16,
        height: u16,
    ) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => match error {},
        };
        match terminal.draw(|frame| render(frame, model)) {
            Ok(_) => {}
            Err(error) => match error {},
        }
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .filter_map(|x| buffer.cell((x, y)))
                    .map(|cell| cell.symbol())
                    .collect()
            })
            .collect()
    }

    fn suspense_screen_lines(
        report: &ListReport,
        date: NaiveDate,
        width: u16,
        height: u16,
    ) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => match error {},
        };
        let mut model = ListReportModel::new(report);
        match terminal.draw(|frame| render_suspense(frame, date, &mut model)) {
            Ok(_) => {}
            Err(error) => match error {},
        }
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .filter_map(|x| buffer.cell((x, y)))
                    .map(|cell| cell.symbol())
                    .collect()
            })
            .collect()
    }

    fn suspense_text_color(
        report: &ListReport,
        date: NaiveDate,
        width: u16,
        height: u16,
        text: &str,
    ) -> Option<Color> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).ok()?;
        let mut model = ListReportModel::new(report);
        terminal
            .draw(|frame| render_suspense(frame, date, &mut model))
            .ok()?;
        let buffer = terminal.backend().buffer();
        for y in 0..height {
            let line: String = (0..width)
                .filter_map(|x| buffer.cell((x, y)))
                .map(|cell| cell.symbol())
                .collect();
            if let Some(byte_index) = line.find(text) {
                let x = u16::try_from(line[..byte_index].chars().count()).ok()?;
                return buffer.cell((x, y)).map(|cell| cell.fg);
            }
        }
        None
    }

    fn calendar_day_style(
        report: &ListReport,
        row: &str,
        day_index: usize,
    ) -> Option<(Color, Color, Modifier)> {
        let mut model = ListReportModel::new(report);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).ok()?;
        terminal.draw(|frame| render(frame, &mut model)).ok()?;
        let buffer = terminal.backend().buffer();
        for y in 0..buffer.area.height {
            let line: String = (0..100)
                .filter_map(|x| buffer.cell((x, y)))
                .map(|cell| cell.symbol())
                .collect();
            if let Some(row_byte) = line.find(row) {
                let row_x = line[..row_byte].chars().count();
                let day_x = row_x + day_index.checked_mul(4)? + 1;
                return buffer
                    .cell((u16::try_from(day_x).ok()?, y))
                    .map(|cell| (cell.fg, cell.bg, cell.modifier));
            }
        }
        None
    }

    fn materialized_style(style: Style) -> (Color, Color, Modifier) {
        (
            style.fg.unwrap_or(Color::Reset),
            style.bg.unwrap_or(Color::Reset),
            style.add_modifier,
        )
    }

    #[test]
    fn month_totals_keep_compact_duration_spacing() {
        assert_eq!(month_totals_text("89h15m", "176h"), "89h15m logged of 176h");
    }

    #[test]
    fn negative_current_period_duration_is_presented_as_time_left() {
        assert_eq!(current_period_text("-14h45m"), "14h45m left");
        assert_eq!(current_period_text("+4h"), "+4h current period");
    }

    #[test]
    fn populated_report_shows_month_day_worklogs_and_quit_controls() {
        let report = report(
            vec![Worklog {
                id: "751393".to_owned(),
                interval: Some(ClockInterval {
                    start_time: "09:00".to_owned(),
                    end_time: "10:30".to_owned(),
                }),
                issue_id: "42".to_owned(),
                issue_key: "OPS-42".to_owned(),
                duration: "1h 30m".to_owned(),
                description: "Daily standup".to_owned(),
                link: "https://example.atlassian.net/browse/OPS-42".to_owned(),
            }],
            true,
        );

        let screen = screen(&report);

        for expected in [
            "July 2026",
            "72h logged of 160h",
            "+4h",
            "Tuesday, 2026-07-14",
            "751393",
            "09:00–10:30",
            "(standup) OPS-42",
            "Day summary: 1h 30m",
            "h/l date",
            "q quit",
            "Esc close",
            "Ctrl-C exit",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}\n{screen}");
        }
    }

    #[test]
    fn list_report_shows_the_shared_brand_lockup_and_version() {
        let report = report(Vec::new(), true);
        let mut model = ListReportModel::new(&report);

        let lines = screen_lines_with_size(&mut model, 100, 40);

        assert!(lines.get(2).is_some_and(|line| {
            line.contains(DRAG_ART[0]) && line.contains(concat!("v", env!("CARGO_PKG_VERSION")))
        }));
        assert!(lines.get(3).is_some_and(|line| line.contains(DRAG_ART[1])));
    }

    #[test]
    fn narrow_header_keeps_the_logo_intact_by_suppressing_the_version() {
        let report = report(Vec::new(), true);
        let mut model = ListReportModel::new(&report);

        let screen = screen_with_size(&mut model, 23, 20);

        assert!(screen.contains(DRAG_ART[0]), "{screen}");
        assert!(!screen.contains(concat!("v", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn wide_terminal_centers_the_list_at_the_setup_content_width() {
        const TERMINAL_WIDTH: u16 = 140;
        let report = report(Vec::new(), true);
        let mut model = ListReportModel::new(&report);

        let lines = screen_lines_with_size(&mut model, TERMINAL_WIDTH, 40);
        let dashboard_border = lines
            .iter()
            .find(|line| line.contains('┌') && line.contains('┐'));
        let expected_left = usize::from((TERMINAL_WIDTH - MAX_CONTENT_WIDTH) / 2);
        let expected_right = expected_left + usize::from(MAX_CONTENT_WIDTH) - 1;

        assert!(dashboard_border.is_some_and(|line| {
            line.chars().position(|character| character == '┌') == Some(expected_left)
                && line.chars().position(|character| character == '┐') == Some(expected_right)
        }));
    }

    #[test]
    fn wide_report_composes_calendar_and_worklogs_in_one_dashboard() {
        let report = report(Vec::new(), true);
        let mut model = ListReportModel::new(&report);

        let lines = screen_lines_with_size(&mut model, 100, 24);

        assert!(
            lines
                .iter()
                .any(|line| line.contains("July 2026") && line.contains("Tuesday, 2026-07-14")),
            "{}",
            lines.join("\n")
        );
        let dashboard_start = lines
            .iter()
            .position(|line| line.starts_with('┌') && line.ends_with('┐'));
        let dashboard_end = lines
            .iter()
            .position(|line| line.starts_with('└') && line.trim_end().ends_with('┘'));
        assert_eq!(dashboard_start, Some(3), "{}", lines.join("\n"));
        assert_eq!(dashboard_end, Some(20), "{}", lines.join("\n"));
        assert!(
            lines.get(21).is_some_and(|line| line.trim().is_empty()),
            "{}",
            lines.join("\n")
        );
        assert!(
            lines
                .get(22)
                .is_some_and(|line| line.chars().all(|character| character == '─')),
            "{}",
            lines.join("\n")
        );
    }

    #[test]
    fn dashboard_uses_fixed_bottom_aligned_summary_footers() {
        let report = report(
            vec![Worklog {
                id: "751393".to_owned(),
                interval: Some(ClockInterval {
                    start_time: "09:00".to_owned(),
                    end_time: "10:30".to_owned(),
                }),
                issue_id: "42".to_owned(),
                issue_key: "OPS-42".to_owned(),
                duration: "1h 30m".to_owned(),
                description: String::new(),
                link: "https://example.atlassian.net/browse/OPS-42".to_owned(),
            }],
            true,
        );
        let mut model = ListReportModel::new(&report);

        let lines = screen_lines_with_size(&mut model, 100, 40);
        let day_summary = lines.iter().position(|line| line.contains("Day summary"));
        let current_period = lines
            .iter()
            .position(|line| line.contains("+4h current period"));
        let month_total = lines
            .iter()
            .position(|line| line.contains("72h logged of 160h"));

        for (label, position) in [
            ("day summary", day_summary),
            ("current period", current_period),
            ("month total", month_total),
        ] {
            assert!(
                position.is_some(),
                "{label} should be visible\n{}",
                lines.join("\n")
            );
        }
        let day_summary = day_summary.unwrap_or(usize::MAX);
        let current_period = current_period.unwrap_or(usize::MAX);
        let month_total = month_total.unwrap_or(usize::MAX);

        assert_eq!(day_summary, current_period, "{}", lines.join("\n"));
        assert_eq!(month_total, current_period + 1, "{}", lines.join("\n"));
        assert_eq!(day_summary, 22, "{}", lines.join("\n"));
    }

    #[test]
    fn suspense_updates_the_calendar_while_current_entries_remain_visible() {
        let report = report(
            vec![Worklog {
                id: "751393".to_owned(),
                interval: None,
                issue_id: "42".to_owned(),
                issue_key: "OPS-42".to_owned(),
                duration: "1h 30m".to_owned(),
                description: "Daily standup".to_owned(),
                link: "https://example.atlassian.net/browse/OPS-42".to_owned(),
            }],
            true,
        );
        let next_date = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap_or(NaiveDate::MIN);

        let lines = suspense_screen_lines(&report, next_date, 100, 24);
        let screen = lines.join("\n");

        for expected in [
            "July 2026",
            "72h logged of 160h",
            "Loading entries…",
            "751393",
            "OPS-42",
            "Day summary",
            "h/l date",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}\n{screen}");
        }
        assert!(screen.contains("Wednesday, 2026-07-15"), "{screen}");
        assert!(!screen.contains("Tuesday, 2026-07-14"), "{screen}");
        let loading_line = lines
            .iter()
            .position(|line| line.contains("Loading entries…"));
        let day_summary_line = lines.iter().position(|line| line.contains("Day summary"));
        assert_eq!(loading_line, day_summary_line, "{screen}");
        assert!(
            lines
                .iter()
                .find_map(|line| line.find("Loading entries…"))
                .is_some_and(|column| column > 60),
            "{screen}"
        );
    }

    #[test]
    fn narrow_suspense_uses_the_footer_for_a_compact_loading_status() {
        let report = report(Vec::new(), true);
        let next_date = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap_or(NaiveDate::MIN);

        let screen = suspense_screen_lines(&report, next_date, 32, 20).join("\n");

        assert!(screen.contains("Wednesday, 2026-07-15"), "{screen}");
        assert!(screen.contains("Loading…"), "{screen}");
    }

    #[test]
    fn suspense_loading_text_uses_the_terminal_foreground() {
        let report = report(Vec::new(), true);
        let next_date = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap_or(NaiveDate::MIN);

        assert_eq!(
            suspense_text_color(&report, next_date, 100, 24, "Loading entries…"),
            Some(Color::Reset)
        );
        assert_eq!(
            suspense_text_color(&report, next_date, 32, 20, "Loading…"),
            Some(Color::Reset)
        );
    }

    #[test]
    fn suspense_can_move_the_calendar_across_a_month_boundary() {
        let report = report(Vec::new(), true);
        let next_month = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap_or(NaiveDate::MIN);

        let screen = suspense_screen_lines(&report, next_month, 100, 24).join("\n");

        assert!(screen.contains("August 2026"), "{screen}");
        assert!(screen.contains("Saturday, 2026-08-01"), "{screen}");
        assert!(screen.contains("Loading entries…"), "{screen}");
        assert!(!screen.contains("72h logged of 160h"), "{screen}");
        assert!(!screen.contains("+4h current period"), "{screen}");
        assert!(!screen.contains("Tuesday, 2026-07-14"), "{screen}");
    }

    #[test]
    fn calendar_shows_the_selected_month_with_surrounding_dates() {
        let selected_date = NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN);
        let today = NaiveDate::from_ymd_opt(2026, 7, 3).unwrap_or(NaiveDate::MIN);
        let report = report_with_dates(Vec::new(), true, false, selected_date, today);

        let screen = screen(&report);

        for expected in [
            "Su  Mo  Tu  We  Th  Fr  Sa",
            "28  29  30  1   2   3   4",
            "5   6   7   8   9   10  11",
            "12  13  14  15  16  17  18",
            "26  27  28  29  30  31  1",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}\n{screen}");
        }
    }

    #[test]
    fn calendar_distinguishes_today_from_the_selected_date() {
        let selected_date = NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN);
        let today = NaiveDate::from_ymd_opt(2026, 7, 3).unwrap_or(NaiveDate::MIN);
        let report = report_with_dates(Vec::new(), true, false, selected_date, today);

        let today_style = calendar_day_style(&report, "28  29  30  1   2   3   4", 5);
        let selected_style = calendar_day_style(&report, "12  13  14  15  16  17  18", 2);

        assert_eq!(
            today_style,
            Some(materialized_style(Palette::focus().bold()))
        );
        assert_eq!(
            selected_style,
            Some(materialized_style(Palette::action_focus().bold()))
        );
        assert_ne!(today_style, selected_style);
    }

    #[test]
    fn selected_date_style_takes_precedence_when_it_is_today() {
        let selected_date = NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN);
        let report = report_with_dates(Vec::new(), true, false, selected_date, selected_date);

        assert_eq!(
            calendar_day_style(&report, "12  13  14  15  16  17  18", 2),
            Some(materialized_style(Palette::action_focus().bold()))
        );
    }

    #[test]
    fn calendar_and_heading_follow_a_selected_date_across_a_month_boundary() {
        let selected_date = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap_or(NaiveDate::MIN);
        let today = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap_or(NaiveDate::MIN);
        let report = report_with_dates(Vec::new(), true, false, selected_date, today);

        let screen = screen(&report);

        for expected in [
            "August 2026",
            "Saturday, 2026-08-01",
            "26  27  28  29  30  31  1",
            "30  31  1   2   3   4   5",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}\n{screen}");
        }
    }

    #[test]
    fn empty_report_shows_empty_state_and_schedule_summaries() {
        let screen = screen(&report(Vec::new(), true));

        assert!(screen.contains("No worklogs"));
        assert!(!screen.contains("No worklogs for Tuesday"));
        assert!(screen.contains("72h logged of 160h"));
        assert!(screen.contains("Day summary: 1h 30m"));
        assert!(!screen.contains("logged / required"));
    }

    #[test]
    fn incomplete_empty_report_qualifies_segment_and_totals() {
        let screen = screen(&report(Vec::new(), false));

        assert!(screen.contains("No worklogs in this retrieved segment"));
        assert!(screen.contains("More worklogs are available"));
        assert!(screen.contains("Totals reflect this bounded segment"));
        assert!(!screen.contains("No worklogs for Tuesday"));
    }

    #[test]
    fn documented_quit_keys_close_the_report() {
        assert!(should_quit(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(should_quit(KeyCode::Esc, KeyModifiers::NONE));
        assert!(should_quit(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!should_quit(KeyCode::Char('c'), KeyModifiers::NONE));
    }

    #[test]
    fn focus_starts_on_the_first_worklog_and_navigation_stays_bounded() {
        let report = report(
            ["first", "second", "third"]
                .into_iter()
                .map(|id| Worklog {
                    id: id.to_owned(),
                    interval: None,
                    issue_id: id.to_owned(),
                    issue_key: format!("OPS-{id}"),
                    duration: "30m".to_owned(),
                    description: String::new(),
                    link: format!("https://example.atlassian.net/browse/OPS-{id}"),
                })
                .collect(),
            true,
        );
        let mut model = ListReportModel::new(&report);

        assert_eq!(model.focused_row(), Some(0));
        model.update(Message::MoveUp, &NoopBrowserLauncher);
        assert_eq!(model.focused_row(), Some(0));
        model.update(Message::MoveDown, &NoopBrowserLauncher);
        model.update(Message::MoveDown, &NoopBrowserLauncher);
        model.update(Message::MoveDown, &NoopBrowserLauncher);
        assert_eq!(model.focused_row(), Some(2));
        model.update(Message::MoveUp, &NoopBrowserLauncher);
        assert_eq!(model.focused_row(), Some(1));
    }

    #[test]
    fn empty_report_has_no_focus_and_navigation_is_a_no_op() {
        let report = report(Vec::new(), true);
        let mut model = ListReportModel::new(&report);

        model.update(Message::MoveDown, &NoopBrowserLauncher);
        model.update(Message::MoveUp, &NoopBrowserLauncher);

        assert_eq!(model.focused_row(), None);
    }

    #[test]
    fn arrow_and_vim_keys_map_to_row_navigation() {
        for code in [KeyCode::Up, KeyCode::Char('k')] {
            assert_eq!(message_for_key(code), Some(Message::MoveUp));
        }
        for code in [KeyCode::Down, KeyCode::Char('j')] {
            assert_eq!(message_for_key(code), Some(Message::MoveDown));
        }
        assert_eq!(message_for_key(KeyCode::Char('x')), None);
    }

    #[test]
    fn h_and_l_change_dates_only_on_key_press() {
        assert_eq!(
            date_action_for_key_event(KeyCode::Char('h'), KeyEventKind::Press),
            Some(ListReportAction::PreviousDate)
        );
        assert_eq!(
            date_action_for_key_event(KeyCode::Char('l'), KeyEventKind::Press),
            Some(ListReportAction::NextDate)
        );
        assert_eq!(
            date_action_for_key_event(KeyCode::Char('l'), KeyEventKind::Repeat),
            None
        );
    }

    #[test]
    fn open_uses_the_focused_worklogs_existing_jira_url() {
        let report = report(
            ["first", "second"]
                .into_iter()
                .map(|id| Worklog {
                    id: id.to_owned(),
                    interval: None,
                    issue_id: id.to_owned(),
                    issue_key: format!("OPS-{id}"),
                    duration: "30m".to_owned(),
                    description: String::new(),
                    link: format!("https://jira.example/browse/OPS-{id}?source=drag"),
                })
                .collect(),
            true,
        );
        let opened = Arc::new(Mutex::new(Vec::new()));
        let launcher = FakeBrowserLauncher {
            opened: Arc::clone(&opened),
            failure: None,
        };
        let mut model = ListReportModel::new(&report);

        model.update(Message::MoveDown, &launcher);
        model.update(Message::Open, &launcher);

        assert_eq!(
            *opened.lock().unwrap_or_else(|error| error.into_inner()),
            ["https://jira.example/browse/OPS-second?source=drag"]
        );
        assert_eq!(model.status(), Some("Opened OPS-second"));
    }

    #[test]
    fn browser_failure_is_recoverable_and_does_not_expose_its_details() {
        let report = report(
            vec![Worklog {
                id: "first".to_owned(),
                interval: None,
                issue_id: "1".to_owned(),
                issue_key: "OPS-1".to_owned(),
                duration: "30m".to_owned(),
                description: String::new(),
                link: "https://jira.example/browse/OPS-1".to_owned(),
            }],
            true,
        );
        let launcher = FakeBrowserLauncher {
            opened: Arc::new(Mutex::new(Vec::new())),
            failure: Some("secret browser configuration"),
        };
        let mut model = ListReportModel::new(&report);

        model.update(Message::Open, &launcher);
        assert_eq!(model.status(), Some("Could not open OPS-1 in the browser"));
        let screen = screen_with_size(&mut model, 80, 20);
        assert!(
            screen.contains("Could not open OPS-1 in the browser"),
            "{screen}"
        );
        assert!(!screen.contains("secret browser configuration"), "{screen}");
        model.update(Message::MoveDown, &launcher);
        assert_eq!(model.focused_row(), Some(0));
        assert_eq!(model.status(), None);
    }

    #[test]
    fn open_without_a_focused_worklog_is_a_no_op() {
        let report = report(Vec::new(), true);
        let opened = Arc::new(Mutex::new(Vec::new()));
        let launcher = FakeBrowserLauncher {
            opened: Arc::clone(&opened),
            failure: None,
        };
        let mut model = ListReportModel::new(&report);

        model.update(Message::Open, &launcher);

        assert!(opened
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
        assert_eq!(model.status(), None);
    }

    #[test]
    fn open_key_and_footer_are_discoverable() {
        assert_eq!(message_for_key(KeyCode::Char('o')), Some(Message::Open));
        let report = report(Vec::new(), true);
        let screen = screen(&report);
        assert!(screen.contains("o open"), "{screen}");
    }

    #[test]
    fn key_repeat_moves_focus_but_does_not_repeat_browser_launches() {
        assert_eq!(
            message_for_key_event(KeyCode::Down, KeyEventKind::Repeat),
            Some(Message::MoveDown)
        );
        assert_eq!(
            message_for_key_event(KeyCode::Char('o'), KeyEventKind::Press),
            Some(Message::Open)
        );
        assert_eq!(
            message_for_key_event(KeyCode::Char('o'), KeyEventKind::Repeat),
            None
        );
    }

    #[test]
    fn compact_footer_keeps_quit_move_and_open_hints_at_boundary_widths() {
        let report = report(Vec::new(), true);
        for width in [24, 32] {
            let mut model = ListReportModel::new(&report);
            let screen = screen_with_size(&mut model, width, 20);
            for expected in ["q quit", "move", "o open"] {
                assert!(
                    screen.contains(expected),
                    "missing {expected:?} at width {width}\n{screen}"
                );
            }
        }
    }

    #[test]
    fn focused_row_is_visible_and_scrolling_keeps_the_last_row_on_screen() {
        let report = report(
            (1..=12)
                .map(|index| Worklog {
                    id: format!("row-{index}"),
                    interval: Some(ClockInterval {
                        start_time: "09:00".to_owned(),
                        end_time: "09:30".to_owned(),
                    }),
                    issue_id: index.to_string(),
                    issue_key: format!("OPS-{index}"),
                    duration: "30m".to_owned(),
                    description: format!("description {index}"),
                    link: format!("https://example.atlassian.net/browse/OPS-{index}"),
                })
                .collect(),
            true,
        );
        let mut model = ListReportModel::new(&report);

        let initial = screen_with_size(&mut model, 100, 16);
        assert!(initial.contains("▶ row-1"), "{initial}");

        for _ in 1..report.worklogs().len() {
            model.update(Message::MoveDown, &NoopBrowserLauncher);
        }
        let scrolled = screen_with_size(&mut model, 100, 16);

        assert!(scrolled.contains("▶ row-12"), "{scrolled}");
        assert!(!scrolled.contains("row-1        09:00"), "{scrolled}");
    }

    #[test]
    fn verbose_report_shows_details_for_the_focused_worklog() {
        let report = report_with_verbose(
            vec![
                Worklog {
                    id: "first".to_owned(),
                    interval: None,
                    issue_id: "1".to_owned(),
                    issue_key: "OPS-1".to_owned(),
                    duration: "30m".to_owned(),
                    description: "First description".to_owned(),
                    link: "https://example.atlassian.net/browse/OPS-1".to_owned(),
                },
                Worklog {
                    id: "second".to_owned(),
                    interval: None,
                    issue_id: "2".to_owned(),
                    issue_key: "OPS-2".to_owned(),
                    duration: "45m".to_owned(),
                    description: "Second description".to_owned(),
                    link: "https://example.atlassian.net/browse/OPS-2".to_owned(),
                },
            ],
            true,
            true,
        );
        let mut model = ListReportModel::new(&report);

        let first = screen_with_size(&mut model, 100, 24);
        assert!(first.contains("First description"), "{first}");
        assert!(
            first.contains("https://example.atlassian.net/browse/OPS-1"),
            "{first}"
        );

        model.update(Message::MoveDown, &NoopBrowserLauncher);
        let second = screen_with_size(&mut model, 100, 24);
        assert!(second.contains("Second description"), "{second}");
        assert!(
            second.contains("https://example.atlassian.net/browse/OPS-2"),
            "{second}"
        );
        assert!(!second.contains("First description"), "{second}");
    }

    #[test]
    fn verbose_details_reserve_a_visible_line_for_the_jira_url() {
        let report = report_with_verbose(
            vec![Worklog {
                id: "first".to_owned(),
                interval: None,
                issue_id: "1".to_owned(),
                issue_key: "OPS-1".to_owned(),
                duration: "30m".to_owned(),
                description: "A long description that wraps across every available description line and must not displace the Jira URL. ".repeat(4),
                link: "https://example.atlassian.net/browse/OPS-1".to_owned(),
            }],
            true,
            true,
        );
        let mut model = ListReportModel::new(&report);

        let screen = screen_with_size(&mut model, 80, 24);

        assert!(
            screen.contains("Jira: https://example.atlassian.net/browse/OPS-1"),
            "{screen}"
        );
    }

    #[test]
    fn narrow_report_keeps_primary_fields_alias_and_navigation_hints() {
        let report = report(
            vec![Worklog {
                id: "751393".to_owned(),
                interval: Some(ClockInterval {
                    start_time: "09:00".to_owned(),
                    end_time: "10:30".to_owned(),
                }),
                issue_id: "42".to_owned(),
                issue_key: "OPS-42".to_owned(),
                duration: "1h 30m".to_owned(),
                description: String::new(),
                link: "https://example.atlassian.net/browse/OPS-42".to_owned(),
            }],
            true,
        );
        let mut model = ListReportModel::new(&report);

        let narrow = screen_with_size(&mut model, 52, 20);

        for expected in [
            "751393",
            "09:00–10:30",
            "(standup)",
            "1h 30m",
            "↑/k",
            "↓/j",
            "q quit",
        ] {
            assert!(narrow.contains(expected), "missing {expected:?}\n{narrow}");
        }
    }

    #[test]
    fn medium_width_report_preserves_a_twelve_character_worklog_id() {
        let report = report(
            vec![Worklog {
                id: "123456789012".to_owned(),
                interval: None,
                issue_id: "42".to_owned(),
                issue_key: "OPS-42".to_owned(),
                duration: "30m".to_owned(),
                description: String::new(),
                link: "https://example.atlassian.net/browse/OPS-42".to_owned(),
            }],
            true,
        );
        let mut model = ListReportModel::new(&report);

        let screen = screen_with_size(&mut model, 60, 20);

        assert!(screen.contains("123456789012"), "{screen}");
    }

    #[test]
    fn short_verbose_report_prioritizes_the_focused_worklog_table() {
        let report = report_with_verbose(
            vec![Worklog {
                id: "first".to_owned(),
                interval: None,
                issue_id: "1".to_owned(),
                issue_key: "OPS-1".to_owned(),
                duration: "30m".to_owned(),
                description: "Verbose detail".to_owned(),
                link: "https://example.atlassian.net/browse/OPS-1".to_owned(),
            }],
            true,
            true,
        );
        let mut model = ListReportModel::new(&report);

        let short = screen_with_size(&mut model, 80, 14);

        assert!(short.contains("▶ first"), "{short}");
        assert!(short.contains("q quit"), "{short}");
    }

    #[test]
    fn short_incomplete_report_keeps_a_focused_worklog_visible() {
        let report = report(
            vec![Worklog {
                id: "first".to_owned(),
                interval: None,
                issue_id: "1".to_owned(),
                issue_key: "OPS-1".to_owned(),
                duration: "30m".to_owned(),
                description: String::new(),
                link: "https://example.atlassian.net/browse/OPS-1".to_owned(),
            }],
            false,
        );
        let mut model = ListReportModel::new(&report);

        let short = screen_with_size(&mut model, 80, 14);

        assert!(short.contains("▶ first"), "{short}");
    }

    #[test]
    fn very_narrow_footer_prioritizes_the_quit_hint() {
        let report = report(Vec::new(), true);
        let mut model = ListReportModel::new(&report);

        let narrow = screen_with_size(&mut model, 20, 14);

        assert!(narrow.contains("q quit"), "{narrow}");
    }
}
