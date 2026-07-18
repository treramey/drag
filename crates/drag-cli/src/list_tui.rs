//! Ratatui presentation for completed list reports.

use std::future::Future;
use std::io::{self, IsTerminal};
use std::pin::Pin;

use crossterm::cursor::Show;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};

use crate::list::ListReport;
use crate::output::escape_terminal_data;
use crate::CliError;

pub(crate) type ListReportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), CliError>> + Send + 'a>>;

pub(crate) trait ListReportSession: Send + Sync {
    fn is_eligible(&self) -> bool;
    fn run<'a>(&'a self, report: &'a ListReport) -> ListReportFuture<'a>;
}

pub(crate) struct RatatuiListReportSession;

impl ListReportSession for RatatuiListReportSession {
    fn is_eligible(&self) -> bool {
        io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal()
    }

    fn run<'a>(&'a self, report: &'a ListReport) -> ListReportFuture<'a> {
        Box::pin(run_terminal(report))
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
        self.restored = true;
        let mut first_error = self.terminal.show_cursor().err();
        if let Err(error) = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show) {
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

async fn run_terminal(report: &ListReport) -> Result<(), CliError> {
    let mut terminal = StderrTerminal::new()?;
    let mut events = EventStream::new();
    loop {
        terminal.terminal.draw(|frame| render(frame, report))?;
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
                return Ok(());
            }
        }
    }
}

fn should_quit(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Char('q') | KeyCode::Esc)
        || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
}

fn render(frame: &mut Frame<'_>, report: &ListReport) {
    let [month, date, worklogs, day, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    render_month(frame, month, report);
    frame.render_widget(
        Paragraph::new(report.selected_date().format("%A, %Y-%m-%d").to_string()).bold(),
        date,
    );
    render_worklogs(frame, worklogs, report);
    let schedule = report.schedule();
    frame.render_widget(
        Paragraph::new(format!(
            "{} / {} · logged / required",
            schedule.day_logged_duration, schedule.day_required_duration
        ))
        .block(Block::bordered().title("Day summary")),
        day,
    );
    frame.render_widget(
        Line::from(vec![
            " q ".bold().cyan(),
            "quit   ".dim(),
            "Esc ".bold().cyan(),
            "close   ".dim(),
            "Ctrl-C ".bold().cyan(),
            "exit".dim(),
        ]),
        footer,
    );
}

fn render_month(frame: &mut Frame<'_>, area: Rect, report: &ListReport) {
    let schedule = report.schedule();
    frame.render_widget(
        Paragraph::new(format!(
            "{} / {} logged · {} current period",
            schedule.month_logged_duration,
            schedule.month_required_duration,
            schedule.month_current_period_duration
        ))
        .block(Block::bordered().title(report.selected_date().format("%B %Y").to_string())),
        area,
    );
}

fn render_worklogs(frame: &mut Frame<'_>, area: Rect, report: &ListReport) {
    if report.worklogs().is_empty() {
        frame.render_widget(
            Paragraph::new(format!(
                "No worklogs for {}",
                report.selected_date().format("%A, %Y-%m-%d")
            ))
            .centered()
            .block(Block::bordered().title("Worklogs")),
            area,
        );
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
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(18),
            Constraint::Fill(1),
            Constraint::Length(12),
        ],
    )
    .header(Row::new(["ID", "Time", "Issue", "Duration"]).bold())
    .block(Block::bordered().title("Worklogs"));
    frame.render_widget(table, area);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::NaiveDate;
    use crossterm::event::{KeyCode, KeyModifiers};
    use drag::models::{ClockInterval, ListPagination, Worklog};
    use drag::schedule::ScheduleDetails;
    use ratatui::{backend::TestBackend, Terminal};

    use super::{render, should_quit};
    use crate::list::ListReport;

    fn report(worklogs: Vec<Worklog>) -> ListReport {
        ListReport::new(
            NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN),
            worklogs,
            ScheduleDetails {
                month_required_duration: "160h".to_owned(),
                month_logged_duration: "72h".to_owned(),
                month_current_period_duration: "+4h".to_owned(),
                day_required_duration: "8h".to_owned(),
                day_logged_duration: "1h 30m".to_owned(),
            },
            ListPagination {
                selected_date: "2026-07-14".to_owned(),
                month_start: "2026-07-01".to_owned(),
                month_end: "2026-07-31".to_owned(),
                limit: Some(100),
                page_limit: 1,
                all_pages: false,
                pages_retrieved: 1,
                records_retrieved: 1,
                records_returned: 1,
                next: None,
                complete: true,
                totals_complete: true,
            },
            BTreeMap::from([("standup".to_owned(), "OPS-42".to_owned())]),
            false,
        )
    }

    fn screen(report: &ListReport) -> String {
        let backend = TestBackend::new(100, 24);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => match error {},
        };
        match terminal.draw(|frame| render(frame, report)) {
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

    #[test]
    fn populated_report_shows_month_day_worklogs_and_quit_controls() {
        let report = report(vec![Worklog {
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
        }]);

        let screen = screen(&report);

        for expected in [
            "July 2026",
            "72h / 160h",
            "+4h",
            "Tuesday, 2026-07-14",
            "751393",
            "09:00–10:30",
            "(standup) OPS-42",
            "1h 30m / 8h",
            "q quit",
            "Esc close",
            "Ctrl-C exit",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}\n{screen}");
        }
    }

    #[test]
    fn empty_report_shows_empty_state_and_schedule_summaries() {
        let screen = screen(&report(Vec::new()));

        assert!(screen.contains("No worklogs for Tuesday, 2026-07-14"));
        assert!(screen.contains("72h / 160h"));
        assert!(screen.contains("1h 30m / 8h"));
    }

    #[test]
    fn documented_quit_keys_close_the_report() {
        assert!(should_quit(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(should_quit(KeyCode::Esc, KeyModifiers::NONE));
        assert!(should_quit(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!should_quit(KeyCode::Char('c'), KeyModifiers::NONE));
    }
}
