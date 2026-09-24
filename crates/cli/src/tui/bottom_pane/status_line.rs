//! pi-style footer: a two-line status bar.
//!
//! Line 1 shows the working directory (and session) in dim gray. Line 2 shows
//! context usage on the left (pi's `↑tokens · percent%/window (auto)` with
//! color-coded percent) and the current model, right-aligned, on the right
//! (pi's `deepseek-vl-flash · high`).

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};
use std::time::Instant;

use super::super::theme;

#[derive(Clone, Debug)]
pub struct StatusLine {
    pub model: String,
    pub session: String,
    pub directory: String,
    pub working: bool,
    pub streaming: bool,
    pub unseen_output: bool,
    pub loading_session: Option<Instant>,
    pub context_percent: usize,
    pub context_window: usize,
    pub context_tokens: usize,
    pub reasoning: Option<String>,
    /// Animation frame index for the spinner shown while working.
    pub spin: usize,
}

impl StatusLine {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            session: String::new(),
            directory: String::new(),
            working: false,
            streaming: false,
            unseen_output: false,
            loading_session: None,
            context_percent: 0,
            context_window: 0,
            context_tokens: 0,
            reasoning: None,
            spin: 0,
        }
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }
        let width = area.width as usize;
        let top = Rect::new(area.x, area.y, area.width, 1);
        let bottom = Rect::new(area.x, area.y + 1, area.width, 1);

        // Line 1: pwd • session (dim), truncated to the viewport.
        let pwd = compact_home(&self.directory);
        let pwd_line = if self.session.is_empty() || self.session == "New Session" {
            pwd
        } else {
            format!("{pwd} • {}", self.session)
        };
        let top_line = if self.loading_session.is_some() {
            "Loading session…".to_owned()
        } else if self.unseen_output {
            "↓ New output · End to follow".to_owned()
        } else {
            pwd_line
        };
        let top_style = if self.loading_session.is_some() || self.unseen_output {
            theme::accent()
        } else {
            theme::dim()
        };
        Line::from(Span::styled(truncate(top_line, width), top_style)).render(top, buf);

        // Line 2: context usage (left) and model name (right-aligned).
        let tokens = format_tokens(self.context_tokens);
        let window = format_tokens(self.context_window);
        let percent_style = if self.context_percent > 90 {
            theme::error()
        } else if self.context_percent > 70 {
            theme::warn()
        } else {
            theme::dim()
        };
        let working_marker = if self.streaming {
            "▍ ".to_owned()
        } else if self.working {
            const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            format!("{} ", SPINNER[self.spin % SPINNER.len()])
        } else {
            String::new()
        };
        let left = format!("{working_marker}↑{tokens} · ");
        let right = match &self.reasoning {
            Some(effort) => format!("{} · {}", self.model, effort),
            None => self.model.clone(),
        };
        let left_width = left.chars().count();
        let percent_text = format!("{}/{} (auto)", self.context_percent, window);
        let percent_width = percent_text.chars().count();
        let right_width = right.chars().count();
        let pad = width.saturating_sub(left_width + percent_width + right_width);

        let mut spans = vec![Span::styled(left, theme::dim())];
        spans.push(Span::styled(percent_text, percent_style));
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.push(Span::styled(right, theme::body()));
        Line::from(spans).render(bottom, buf);
    }
}

fn compact_home(directory: &str) -> String {
    let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) else {
        return directory.to_owned();
    };
    directory.strip_prefix(&home).map_or_else(
        || directory.to_owned(),
        |suffix| {
            if suffix.is_empty() {
                "~".to_owned()
            } else {
                format!("~{suffix}")
            }
        },
    )
}

fn truncate(text: String, width: usize) -> String {
    if text.chars().count() <= width {
        return text;
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Compact token counts like pi's `formatTokens` ("1.5k", "12k", "3.2M").
fn format_tokens(count: usize) -> String {
    if count < 1000 {
        count.to_string()
    } else if count < 10_000 {
        format!("{}.{}k", count / 1000, (count % 1000) / 100)
    } else if count < 1_000_000 {
        format!("{}k", count / 1000)
    } else {
        format!("{}.{}M", count / 1_000_000, (count % 1_000_000) / 100_000)
    }
}
