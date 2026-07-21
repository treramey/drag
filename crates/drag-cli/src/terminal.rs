//! Shared stderr terminal initialization and restoration.

use std::io;

use crossterm::cursor::Show;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::CliError;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TerminalOptions {
    bracketed_paste: bool,
}

impl TerminalOptions {
    pub(crate) const fn bracketed_paste() -> Self {
        Self {
            bracketed_paste: true,
        }
    }
}

pub(crate) struct StderrTerminal {
    terminal: Terminal<CrosstermBackend<io::Stderr>>,
    lifecycle: LifecycleState,
}

impl StderrTerminal {
    pub(crate) fn new(options: TerminalOptions) -> Result<Self, CliError> {
        let mut lifecycle = LifecycleState::default();
        enable_raw_mode()?;
        lifecycle.raw_mode = true;

        let mut stderr = io::stderr();
        lifecycle.cursor = true;
        lifecycle.alternate_screen = true;
        if let Err(error) = execute!(stderr, EnterAlternateScreen) {
            let _ = restore_stderr(&mut lifecycle, &mut stderr);
            return Err(CliError::Io(error));
        }
        if options.bracketed_paste {
            lifecycle.bracketed_paste = true;
            if let Err(error) = execute!(stderr, EnableBracketedPaste) {
                let _ = restore_stderr(&mut lifecycle, &mut stderr);
                return Err(CliError::Io(error));
            }
        }

        match Terminal::new(CrosstermBackend::new(stderr)) {
            Ok(terminal) => Ok(Self {
                terminal,
                lifecycle,
            }),
            Err(error) => {
                let mut stderr = io::stderr();
                let _ = restore_stderr(&mut lifecycle, &mut stderr);
                Err(CliError::Io(error))
            }
        }
    }

    pub(crate) fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<io::Stderr>> {
        &mut self.terminal
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        let Self {
            terminal,
            lifecycle,
        } = self;
        lifecycle.restore_with(|step| match step {
            CleanupStep::ShowCursor => terminal.show_cursor(),
            CleanupStep::DisableBracketedPaste => {
                execute!(terminal.backend_mut(), DisableBracketedPaste)
            }
            CleanupStep::LeaveAlternateScreen => {
                execute!(terminal.backend_mut(), LeaveAlternateScreen)
            }
            CleanupStep::DisableRawMode => disable_raw_mode(),
        })
    }
}

impl Drop for StderrTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CleanupStep {
    ShowCursor,
    DisableBracketedPaste,
    LeaveAlternateScreen,
    DisableRawMode,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct LifecycleState {
    cursor: bool,
    bracketed_paste: bool,
    alternate_screen: bool,
    raw_mode: bool,
}

impl LifecycleState {
    fn restore_with(
        &mut self,
        mut restore: impl FnMut(CleanupStep) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut first_error = None;
        for (pending, step) in [
            (
                &mut self.bracketed_paste,
                CleanupStep::DisableBracketedPaste,
            ),
            (
                &mut self.alternate_screen,
                CleanupStep::LeaveAlternateScreen,
            ),
            (&mut self.cursor, CleanupStep::ShowCursor),
            (&mut self.raw_mode, CleanupStep::DisableRawMode),
        ] {
            if !*pending {
                continue;
            }
            match restore(step) {
                Ok(()) => *pending = false,
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn restore_stderr(lifecycle: &mut LifecycleState, stderr: &mut io::Stderr) -> io::Result<()> {
    lifecycle.restore_with(|step| match step {
        CleanupStep::ShowCursor => execute!(stderr, Show),
        CleanupStep::DisableBracketedPaste => execute!(stderr, DisableBracketedPaste),
        CleanupStep::LeaveAlternateScreen => execute!(stderr, LeaveAlternateScreen),
        CleanupStep::DisableRawMode => disable_raw_mode(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_lifecycle(bracketed_paste: bool) -> LifecycleState {
        LifecycleState {
            cursor: true,
            bracketed_paste,
            alternate_screen: true,
            raw_mode: true,
        }
    }

    #[test]
    fn restoration_attempts_every_step_and_returns_the_first_error() {
        let mut lifecycle = active_lifecycle(true);
        let mut attempted = Vec::new();

        let error = lifecycle
            .restore_with(|step| {
                attempted.push(step);
                match step {
                    CleanupStep::DisableBracketedPaste => {
                        Err(io::Error::other("bracketed paste failed"))
                    }
                    CleanupStep::LeaveAlternateScreen => {
                        Err(io::Error::other("alternate screen failed"))
                    }
                    CleanupStep::ShowCursor | CleanupStep::DisableRawMode => Ok(()),
                }
            })
            .err();

        assert_eq!(
            error.map(|error| error.to_string()).as_deref(),
            Some("bracketed paste failed")
        );
        assert_eq!(
            attempted,
            [
                CleanupStep::DisableBracketedPaste,
                CleanupStep::LeaveAlternateScreen,
                CleanupStep::ShowCursor,
                CleanupStep::DisableRawMode,
            ]
        );
        assert_eq!(
            lifecycle,
            LifecycleState {
                cursor: false,
                bracketed_paste: true,
                alternate_screen: true,
                raw_mode: false,
            }
        );
    }

    #[test]
    fn repeated_restoration_retries_only_failed_steps_until_complete() -> io::Result<()> {
        let mut lifecycle = active_lifecycle(false);
        let mut first_attempt = Vec::new();
        let _ = lifecycle.restore_with(|step| {
            first_attempt.push(step);
            if step == CleanupStep::LeaveAlternateScreen {
                Err(io::Error::other("leave failed"))
            } else {
                Ok(())
            }
        });
        let mut second_attempt = Vec::new();

        lifecycle.restore_with(|step| {
            second_attempt.push(step);
            Ok(())
        })?;

        assert_eq!(second_attempt, [CleanupStep::LeaveAlternateScreen]);
        assert_eq!(lifecycle, LifecycleState::default());
        Ok(())
    }
}
