//! Explicit user review before a learned candidate enters ordinary discovery.
use super::{ModalAction, PaneView, ViewOutcome};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Paragraph, Widget},
};

pub struct EvolutionReview {
    name: String,
    digest: String,
    instructions: String,
    scroll: u16,
    action: Option<ModalAction>,
}
impl EvolutionReview {
    pub fn open(name: String, digest: String, instructions: String) -> Box<dyn PaneView> {
        Box::new(Self {
            name,
            digest,
            instructions,
            scroll: 0,
            action: None,
        })
    }
}
impl PaneView for EvolutionReview {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        if key.kind != KeyEventKind::Press {
            return ViewOutcome::Continue;
        }
        match key.code {
            KeyCode::Char('y' | 'n') => {
                self.action = Some(ModalAction::EvolutionReviewed {
                    name: self.name.clone(),
                    digest: self.digest.clone(),
                    accept: key.code == KeyCode::Char('y'),
                });
                ViewOutcome::Accepted
            }
            KeyCode::Esc => ViewOutcome::Cancelled,
            KeyCode::Down => {
                self.scroll = self.scroll.saturating_add(1);
                ViewOutcome::Continue
            }
            KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                ViewOutcome::Continue
            }
            _ => ViewOutcome::Continue,
        }
    }
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let header = Rect {
            height: area.height.min(2),
            ..area
        };
        Paragraph::new(format!(
            "Try learned skill: {}?\ny accept trial · n archive · Esc later · ↑↓ scroll",
            self.name
        ))
        .render(header, buf);
        let body = Rect {
            y: area.y.saturating_add(header.height),
            height: area.height.saturating_sub(header.height),
            ..area
        };
        Paragraph::new(self.instructions.as_str())
            .scroll((self.scroll, 0))
            .render(body, buf);
    }
    fn preferred_height(&self, _: u16) -> u16 {
        16
    }
    fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }
}
