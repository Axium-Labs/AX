//! Plain-text memory editor with an optimistic update baseline.
use super::{ModalAction, PaneView, ViewOutcome, textarea::TextArea};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Paragraph, Widget, Wrap},
};

pub struct MemoryEditor {
    scope: String,
    key: String,
    expected: String,
    text: TextArea,
    action: Option<ModalAction>,
}
impl MemoryEditor {
    pub fn open(scope: String, key: String, expected: String, value: String) -> Box<dyn PaneView> {
        let mut text = TextArea::new();
        text.set(value);
        Box::new(Self {
            scope,
            key,
            expected,
            text,
            action: None,
        })
    }
}
impl PaneView for MemoryEditor {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        if key.kind != KeyEventKind::Press {
            return ViewOutcome::Continue;
        }
        match key.code {
            KeyCode::Esc => return ViewOutcome::Cancelled,
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                self.action = Some(ModalAction::MemoryEdited {
                    scope: self.scope.clone(),
                    key: self.key.clone(),
                    expected: self.expected.clone(),
                    value: self.text.text().to_owned(),
                });
                return ViewOutcome::Accepted;
            }
            KeyCode::Enter => self.text.insert_newline(),
            KeyCode::Backspace => self.text.backspace(),
            KeyCode::Delete => self.text.delete(),
            KeyCode::Left => self.text.move_left(),
            KeyCode::Right => self.text.move_right(),
            KeyCode::Up => self.text.move_up(),
            KeyCode::Down => self.text.move_down(),
            KeyCode::Home => self.text.move_line_start(),
            KeyCode::End => self.text.move_line_end(),
            KeyCode::Char(c) => self.text.insert_char(c),
            _ => {}
        }
        ViewOutcome::Continue
    }
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let value = self.text.text();
        let (before, after) = value.split_at(self.text.cursor());
        let prefix = format!(
            "Edit {} / {}\nAlt+Enter saves; Esc cancels\n{before}",
            self.scope, self.key
        );
        let scroll = Paragraph::new(prefix.clone())
            .wrap(Wrap { trim: false })
            .line_count(area.width)
            .saturating_sub(usize::from(area.height));
        Paragraph::new(format!("{prefix}▏{after}"))
            .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0))
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
    fn preferred_height(&self, _: u16) -> u16 {
        12
    }
    fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }
}
