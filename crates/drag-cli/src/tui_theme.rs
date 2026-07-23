//! Shared visual language for Drag's terminal interfaces.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

// Keep the UI on the terminal's ANSI palette so it follows the user's theme.
pub(crate) const PRIMARY_COLOR: Color = Color::Magenta;
pub(crate) const MUTED_COLOR: Color = Color::DarkGray;
pub(crate) const SUCCESS_COLOR: Color = Color::Cyan;
pub(crate) const MAX_CONTENT_WIDTH: u16 = 115;
pub(crate) const DRAG_ART: [&str; 2] = ["█▀▄  █▀█  ▄▀█  █▀▀", "█▄▀  █▀▄  █▀█  █▄█"];

pub(crate) struct Palette;

impl Palette {
    pub(crate) const fn text() -> Style {
        Style::new()
    }

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
        Self::primary().reversed()
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

pub(crate) fn constrain_content_width(area: Rect) -> Rect {
    let width = area.width.min(MAX_CONTENT_WIDTH);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y,
        width,
        area.height,
    )
}

pub(crate) fn render_brand_header(
    frame: &mut Frame<'_>,
    area: Rect,
    available_update: Option<&str>,
) {
    let title = DRAG_ART
        .iter()
        .map(|line| Line::styled(*line, Palette::primary().bold()))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(title)), area);

    let version = format!("v{}", env!("CARGO_PKG_VERSION"));
    let version_width = u16::try_from(version.len())
        .unwrap_or(area.width)
        .min(area.width);
    let logo_width = DRAG_ART
        .iter()
        .filter_map(|line| u16::try_from(line.chars().count()).ok())
        .max()
        .unwrap_or(0);
    if area.width < logo_width.saturating_add(version_width).saturating_add(1) {
        return;
    }
    frame.render_widget(
        Paragraph::new(version).style(Palette::muted()),
        Rect::new(
            area.right().saturating_sub(version_width),
            area.y,
            version_width,
            1,
        ),
    );

    let Some(available_update) = available_update.filter(|_| area.height >= 2) else {
        return;
    };
    let update = format!("Update available: v{available_update}");
    let update_width = u16::try_from(update.len())
        .unwrap_or(area.width)
        .min(area.width);
    if area.width < logo_width.saturating_add(update_width).saturating_add(1) {
        return;
    }
    frame.render_widget(
        Paragraph::new(update).style(Palette::success().bold()),
        Rect::new(
            area.right().saturating_sub(update_width),
            area.y.saturating_add(1),
            update_width,
            1,
        ),
    );
}

pub(crate) fn footer_divider(width: u16) -> Line<'static> {
    Line::styled("─".repeat(usize::from(width)), Palette::muted())
}
