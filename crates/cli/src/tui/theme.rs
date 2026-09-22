//! AX terminal palette.
//!
//! Codex-style restraint: black canvas, default foreground, dim gray for
//! auxiliary information, with cyan/green used sparingly.

use ratatui::style::{Color, Modifier, Style};

/// Use the terminal's true black canvas. Transcript output should look like
/// normal console output, not like a second oversized input panel.
pub const USER_MESSAGE_BG: Color = Color::Rgb(52, 52, 64);
pub const CANVAS_BG: Color = Color::Black;
pub const TOOL_PENDING_BG: Color = Color::Rgb(40, 40, 50);
pub const TOOL_SUCCESS_BG: Color = Color::Rgb(40, 50, 40);
pub const TOOL_ERROR_BG: Color = Color::Rgb(60, 40, 40);
/// Dim gray auxiliary text.
pub const MUTED: Color = Color::Rgb(160, 160, 160);
pub const DIM: Color = Color::Rgb(128, 128, 128);
/// Selection highlight background for pickers and popups.
pub const SELECTION_BG: Color = Color::Rgb(56, 56, 64);
pub const ACCENT: Color = Color::Cyan;
pub const BORDER_ACCENT: Color = Color::Rgb(0, 215, 255);
pub const BORDER_MUTED: Color = Color::Rgb(80, 80, 80);
pub const SUCCESS: Color = Color::Rgb(181, 189, 104);
pub const WARN: Color = Color::Yellow;
pub const ERROR: Color = Color::Red;
/// Backing for fenced code blocks (slightly lighter than the black canvas).
pub const CODE_BG: Color = Color::Rgb(26, 26, 32);

/// Default body text (inherits terminal foreground).
pub fn body() -> Style {
    Style::default().fg(Color::Rgb(212, 212, 212))
}

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn dim() -> Style {
    Style::default().fg(DIM)
}

pub fn accent() -> Style {
    Style::default().fg(ACCENT)
}

pub fn accent_bold() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn title() -> Style {
    Style::default()
        .fg(Color::Reset)
        .add_modifier(Modifier::BOLD)
}

pub fn success() -> Style {
    Style::default().fg(SUCCESS)
}

pub fn error() -> Style {
    Style::default().fg(ERROR)
}

pub fn warn() -> Style {
    Style::default().fg(WARN)
}

/// Inline code text (warm terminal tan so code stands out from body text).
pub fn code() -> Style {
    Style::default().fg(Color::Rgb(214, 157, 133))
}

/// Selected row inside a popup/picker.
pub fn selected_row() -> Style {
    Style::default()
        .bg(SELECTION_BG)
        .fg(Color::Reset)
        .add_modifier(Modifier::BOLD)
}
