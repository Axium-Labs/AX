use super::{ModalAction, PaneView, ViewOutcome, textarea::TextArea};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Paragraph, Widget},
};

pub struct SessionRename {
    id: String,
    text: TextArea,
    action: Option<ModalAction>,
}

impl SessionRename {
    pub fn open(id: String, title: String) -> Box<dyn PaneView> {
        let mut text = TextArea::new();
        text.set(title);
        Box::new(Self {
            id,
            text,
            action: None,
        })
    }
}

impl PaneView for SessionRename {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        if key.kind != KeyEventKind::Press {
            return ViewOutcome::Continue;
        }
        match key.code {
            KeyCode::Esc => return ViewOutcome::Cancelled,
            KeyCode::Enter => {
                let title = self.text.text().trim();
                if title.is_empty() {
                    return ViewOutcome::Continue;
                }
                self.action = Some(ModalAction::SessionRenamed {
                    id: self.id.clone(),
                    title: title.to_owned(),
                });
                return ViewOutcome::Accepted;
            }
            KeyCode::Backspace => self.text.backspace(),
            KeyCode::Delete => self.text.delete(),
            KeyCode::Left => self.text.move_left(),
            KeyCode::Right => self.text.move_right(),
            KeyCode::Home => self.text.move_line_start(),
            KeyCode::End => self.text.move_line_end(),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.text.insert_char(c);
            }
            _ => {}
        }
        ViewOutcome::Continue
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(format!(
            "Rename session · Enter save · Esc cancel\n› {}",
            self.text.text()
        ))
        .render(area, buf);
    }
    fn preferred_height(&self, _: u16) -> u16 {
        3
    }
    fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }
}
