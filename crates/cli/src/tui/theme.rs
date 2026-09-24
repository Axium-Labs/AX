//! AX terminal palette.
//!
//! Quiet slate surfaces on a black terminal canvas. A soft teal carries
//! focus; green, amber, and coral are reserved for semantic status.

use ratatui::style::{Color, Modifier, Style};

/// Use the terminal's true black canvas. Transcript output should look like
/// normal console output, not like a second oversized input panel.
pub const USER_MESSAGE_BG: Color = Color::Rgb(36, 42, 54);
pub const CANVAS_BG: Color = Color::Black;
pub const TOOL_PENDING_BG: Color = Color::Rgb(29, 41, 50);
pub const TOOL_SUCCESS_BG: Color = Color::Rgb(30, 50, 45);
pub const TOOL_ERROR_BG: Color = Color::Rgb(57, 42, 46);
pub const BODY: Color = Color::Rgb(229, 233, 237);
pub const TITLE: Color = Color::Rgb(244, 246, 247);
pub const MUTED: Color = Color::Rgb(175, 186, 196);
pub const DIM: Color = Color::Rgb(142, 155, 166);
/// Selection highlight background for pickers and popups.
pub const SELECTION_BG: Color = Color::Rgb(41, 59, 70);
pub const SELECTED_FG: Color = Color::Rgb(240, 246, 247);
pub const ACTIVE_FG: Color = Color::Rgb(168, 230, 224);
pub const ACCENT: Color = Color::Rgb(130, 212, 213);
pub const BORDER_ACCENT: Color = Color::Rgb(79, 191, 195);
pub const BORDER_MUTED: Color = Color::Rgb(74, 86, 99);
pub const SUCCESS: Color = Color::Rgb(159, 215, 180);
pub const WARN: Color = Color::Rgb(231, 197, 134);
pub const ERROR: Color = Color::Rgb(240, 167, 163);
/// Backing for fenced code blocks (slightly lighter than the black canvas).
pub const CODE_BG: Color = Color::Rgb(23, 28, 37);
pub const CODE_FG: Color = Color::Rgb(221, 186, 156);

/// Default body text, independent of the terminal's configured foreground.
pub fn body() -> Style {
    Style::default().fg(BODY)
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
    Style::default().fg(TITLE).add_modifier(Modifier::BOLD)
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
    Style::default().fg(CODE_FG)
}

/// Selected row inside a popup/picker.
pub fn selected_row() -> Style {
    Style::default()
        .bg(SELECTION_BG)
        .fg(SELECTED_FG)
        .add_modifier(Modifier::BOLD)
}

/// Brief keyboard-selection feedback, without relying on terminal defaults.
pub fn selected_row_active() -> Style {
    selected_row().fg(ACTIVE_FG)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color) -> f64 {
        let Color::Rgb(red, green, blue) = color else {
            panic!("palette contrast test requires RGB colors");
        };
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.040_45 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)
    }

    fn contrast(foreground: Color, background: Color) -> f64 {
        let foreground = luminance(foreground);
        let background = luminance(background);
        (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
    }

    #[test]
    fn text_remains_readable_on_each_surface() {
        let pairs = [
            (BODY, Color::Rgb(0, 0, 0)),
            (MUTED, Color::Rgb(0, 0, 0)),
            (DIM, Color::Rgb(0, 0, 0)),
            (ACCENT, Color::Rgb(0, 0, 0)),
            (SUCCESS, TOOL_SUCCESS_BG),
            (ERROR, TOOL_ERROR_BG),
            (BODY, USER_MESSAGE_BG),
            (BODY, TOOL_PENDING_BG),
            (BODY, TOOL_SUCCESS_BG),
            (BODY, TOOL_ERROR_BG),
            (BODY, CODE_BG),
            (CODE_FG, Color::Rgb(0, 0, 0)),
            (SELECTED_FG, SELECTION_BG),
            (ACTIVE_FG, SELECTION_BG),
        ];
        for (foreground, background) in pairs {
            assert!(
                contrast(foreground, background) >= 4.5,
                "low contrast: {foreground:?} on {background:?}"
            );
        }
        assert!(luminance(TITLE) > luminance(BODY));
        assert!(luminance(BODY) > luminance(MUTED));
        assert!(luminance(MUTED) > luminance(DIM));
    }
}
