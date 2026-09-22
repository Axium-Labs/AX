//! In-TUI tool approval dialog.
//!
//! Replaces the old flow that left the alternate screen for a stdin prompt:
//! approvals are now a modal view on the bottom-pane view stack, matching
//! Codex's approval overlay pattern.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use serde_json::Value;
use tokio::sync::oneshot;
use tool::SafetyLevel;

use super::super::ApprovalChoice;
use super::super::theme;
use super::view::{ModalAction, PaneView, ViewOutcome};

const MAX_INPUT_ROWS: usize = 8;

pub struct ApprovalDialog {
    tool: String,
    input: Value,
    safety: SafetyLevel,
    /// 0 = allow once, 1 = allow for the session, 2 = deny.
    selected: usize,
    reply: Option<oneshot::Sender<ApprovalChoice>>,
    action: Option<ModalAction>,
    outcome: ViewOutcome,
}

impl ApprovalDialog {
    pub fn new(
        tool: String,
        input: Value,
        safety: SafetyLevel,
        reply: oneshot::Sender<ApprovalChoice>,
    ) -> Self {
        Self {
            tool,
            input,
            safety,
            selected: 2,
            reply: Some(reply),
            action: None,
            outcome: ViewOutcome::Continue,
        }
    }

    fn resolve(&mut self, choice: ApprovalChoice) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(choice);
        }
        self.action = Some(ModalAction::Approval(choice != ApprovalChoice::Deny));
        self.outcome = if choice == ApprovalChoice::Deny {
            ViewOutcome::Cancelled
        } else {
            ViewOutcome::Accepted
        };
    }

    fn wrapped_input(&self, width: u16) -> Vec<String> {
        let pretty = serde_json::to_string_pretty(&self.input).unwrap_or_default();
        let max_width = (width as usize).saturating_sub(4).max(20);
        let mut rows = Vec::new();
        for line in pretty.lines() {
            // naive character-level wrap for long JSON lines
            let mut current = String::new();
            let mut width_seen = 0usize;
            for c in line.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if width_seen + cw > max_width && !current.is_empty() {
                    rows.push(std::mem::take(&mut current));
                    width_seen = 0;
                }
                current.push(c);
                width_seen += cw;
            }
            rows.push(current);
        }
        rows
    }
}

impl PaneView for ApprovalDialog {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        if key.kind != KeyEventKind::Press {
            return ViewOutcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT)
        {
            return ViewOutcome::Continue;
        }
        match key.code {
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(2),
            KeyCode::Char('y') => self.resolve(ApprovalChoice::AllowOnce),
            KeyCode::Char('n') | KeyCode::Esc => self.resolve(ApprovalChoice::Deny),
            KeyCode::Enter => self.resolve(match self.selected {
                0 => ApprovalChoice::AllowOnce,
                1 => ApprovalChoice::AllowSession,
                _ => ApprovalChoice::Deny,
            }),
            _ => {}
        }
        self.outcome
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            "AX needs your approval",
            theme::warn().add_modifier(ratatui::style::Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("tool     ", theme::muted()),
            Span::styled(self.tool.clone(), theme::accent()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("safety   ", theme::muted()),
            Span::raw(format!("{:?}", self.safety)),
        ]));
        lines.push(Line::from(Span::styled("input", theme::muted())));
        let input_rows = self.wrapped_input(area.width);
        for row in input_rows.into_iter().take(MAX_INPUT_ROWS) {
            lines.push(Line::from(Span::styled(format!("  {row}"), theme::dim())));
        }
        lines.push(Line::from(""));
        let options = [
            (0usize, "Yes — allow once"),
            (1, "Yes — allow for this session"),
            (2, "No — deny"),
        ];
        for (index, label) in options {
            let selected = index == self.selected;
            let marker = if selected { "› " } else { "  " };
            let style = if selected {
                theme::selected_row()
            } else {
                theme::body()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    marker,
                    if selected {
                        theme::selected_row()
                    } else {
                        theme::muted()
                    },
                ),
                Span::styled(label, style),
            ]));
        }
        lines.push(Line::from(Span::styled(
            "↑↓ move · enter confirm · y allow · n/esc deny",
            theme::dim(),
        )));
        Paragraph::new(lines)
            .style(Style::default().bg(theme::CANVAS_BG))
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn preferred_height(&self, width: u16) -> u16 {
        let input = self.wrapped_input(width).len().min(MAX_INPUT_ROWS);
        // title + blank + tool + safety + input label + input rows + blank +
        // three options + hint
        u16::try_from(10 + input).unwrap_or(u16::MAX)
    }

    fn title(&self) -> &'static str {
        "approval"
    }

    fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }
}
