//! Startup information card rendered at the top of a fresh transcript.

use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme;

#[derive(Clone, Debug)]
pub struct StartupInfo {
    pub version: String,
    pub model: String,
    pub directory: String,
}

impl StartupInfo {
    pub fn new(version: &str, model: &str, _session: &str, directory: &str) -> Self {
        Self {
            version: version.to_owned(),
            model: model.to_owned(),
            directory: directory.to_owned(),
        }
    }

    /// Render the compact AX startup card as transcript lines.
    pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let card_width = usize::from(width).clamp(24, 52);
        let inner = card_width.saturating_sub(2);
        let framed = |content: String| {
            let clipped = clip(&content, inner.saturating_sub(2));
            let padding = inner.saturating_sub(1 + UnicodeWidthStr::width(clipped.as_str()));
            Line::from(vec![
                Span::styled("│ ", theme::dim()),
                Span::styled(clipped, theme::body()),
                Span::raw(" ".repeat(padding)),
                Span::styled("│", theme::dim()),
            ])
        };
        let field = |label: &str, value: &str| framed(format!("{label:<11}{value}"));
        vec![
            Line::from(Span::styled(
                format!("╭{}╮", "─".repeat(inner)),
                theme::dim(),
            )),
            framed(format!(">_ AX Runtime (v{})", self.version)),
            framed(String::new()),
            field("model:", &self.model),
            field("directory:", &self.directory),
            Line::from(Span::styled(
                format!("╰{}╯", "─".repeat(inner)),
                theme::dim(),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Tip: ", theme::accent()),
                Span::styled("Type / to discover AX commands.", theme::dim()),
            ]),
            Line::from(""),
        ]
    }
}

fn clip(value: &str, width: usize) -> String {
    let mut output = String::new();
    for character in value.chars() {
        let next = UnicodeWidthStr::width(output.as_str()) + character.width().unwrap_or_default();
        if next > width {
            break;
        }
        output.push(character);
    }
    output
}
