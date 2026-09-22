//! Masked API-key entry used by provider login flows.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use super::{ModalAction, PaneView, ViewOutcome};
use crate::tui::theme;

pub struct SecretInput {
    provider: String,
    value: String,
    action: Option<ModalAction>,
}

impl SecretInput {
    pub fn open(provider: impl Into<String>) -> Box<dyn PaneView> {
        Box::new(Self {
            provider: provider.into(),
            value: String::new(),
            action: None,
        })
    }
}

impl PaneView for SecretInput {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        if key.kind != KeyEventKind::Press {
            return ViewOutcome::Continue;
        }
        match key.code {
            KeyCode::Esc => ViewOutcome::Cancelled,
            KeyCode::Backspace => {
                self.value.pop();
                ViewOutcome::Continue
            }
            KeyCode::Char(character) => {
                self.value.push(character);
                ViewOutcome::Continue
            }
            KeyCode::Enter if !self.value.trim().is_empty() => {
                self.action = Some(ModalAction::ApiKeyConfigured {
                    provider: self.provider.clone(),
                    key: self.value.trim().to_owned(),
                });
                ViewOutcome::Accepted
            }
            _ => ViewOutcome::Continue,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let lines = vec![
            Line::from(Span::styled(
                format!("Configure {} API key", self.provider),
                theme::title(),
            )),
            Line::from(Span::styled(
                "Stored in ~/.ax/auth.json (or $AX_HOME/auth.json).",
                theme::dim(),
            )),
            Line::default(),
            Line::from(vec![
                Span::styled("› ", theme::accent()),
                Span::raw("•".repeat(self.value.chars().count())),
            ]),
            Line::default(),
            Line::from(Span::styled("Enter save · Esc cancel", theme::dim())),
        ];
        Paragraph::new(lines)
            .style(Style::default().bg(theme::CANVAS_BG))
            .render(area, buf);
    }

    fn preferred_height(&self, _width: u16) -> u16 {
        6
    }
    fn title(&self) -> &'static str {
        "Configure API key"
    }
    fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }
}
