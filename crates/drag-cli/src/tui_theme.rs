//! Shared visual language for Drag's terminal interfaces.

use ratatui::style::{Color, Style};

pub(crate) const PRIMARY_COLOR: Color = Color::Rgb(116, 39, 127);
pub(crate) const MUTED_COLOR: Color = Color::Rgb(101, 92, 82);
pub(crate) const SUCCESS_COLOR: Color = Color::Rgb(0, 121, 133);

pub(crate) struct Palette;

impl Palette {
    pub(crate) const fn primary() -> Style {
        Style::new().fg(PRIMARY_COLOR)
    }

    pub(crate) const fn muted() -> Style {
        Style::new().fg(MUTED_COLOR)
    }

    pub(crate) const fn focus() -> Style {
        Self::primary()
    }

    pub(crate) const fn action_focus() -> Style {
        Style::new().fg(Color::Rgb(243, 239, 230)).bg(PRIMARY_COLOR)
    }

    pub(crate) const fn pending() -> Style {
        Style::new().fg(Color::Yellow)
    }

    pub(crate) const fn success() -> Style {
        Style::new().fg(SUCCESS_COLOR)
    }

    pub(crate) const fn warning() -> Style {
        Style::new().fg(Color::Yellow)
    }

    pub(crate) const fn error() -> Style {
        Style::new().fg(Color::Red)
    }
}
