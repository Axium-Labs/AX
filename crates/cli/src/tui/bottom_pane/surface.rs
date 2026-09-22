//! Shared visual grammar for Manager and Info Panel slash surfaces.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceStyle {
    Manager,
    InfoPanel,
}

#[derive(Clone, Debug)]
pub struct SurfaceItem {
    pub id: String,
    pub label: String,
    pub value: String,
}

pub struct SurfaceView {
    title: &'static str,
    surface: String,
    style: SurfaceStyle,
    intro: Vec<String>,
    items: Vec<SurfaceItem>,
    footer: &'static str,
    selected: usize,
    filter: String,
    action: Option<ModalAction>,
}

impl SurfaceView {
    pub fn manager(
        title: &'static str,
        surface: impl Into<String>,
        intro: Vec<String>,
        items: Vec<SurfaceItem>,
        footer: &'static str,
    ) -> Box<dyn PaneView> {
        Box::new(Self {
            title,
            surface: surface.into(),
            style: SurfaceStyle::Manager,
            intro,
            items,
            footer,
            selected: 0,
            filter: String::new(),
            action: None,
        })
    }
    pub fn info(title: &'static str, lines: Vec<String>) -> Box<dyn PaneView> {
        Box::new(Self {
            title,
            surface: "info".to_owned(),
            style: SurfaceStyle::InfoPanel,
            intro: lines,
            items: Vec::new(),
            footer: "Esc back",
            selected: 0,
            filter: String::new(),
            action: None,
        })
    }

    fn visible(&self) -> Vec<&SurfaceItem> {
        if self.filter.is_empty() {
            return self.items.iter().collect();
        }
        let needle = self.filter.to_ascii_lowercase();
        self.items
            .iter()
            .filter(|item| {
                item.label.to_ascii_lowercase().contains(&needle)
                    || item.value.to_ascii_lowercase().contains(&needle)
            })
            .collect()
    }
}

impl PaneView for SurfaceView {
    fn refresh_surface(&mut self, surface: &str, items: &[SurfaceItem]) {
        if self.surface != surface {
            return;
        }
        let selected_id = self
            .visible()
            .get(self.selected)
            .map(|item| item.id.clone());
        self.items = items.to_vec();
        let visible = self.visible();
        self.selected = selected_id
            .and_then(|id| visible.iter().position(|item| item.id == id))
            .unwrap_or_else(|| self.selected.min(visible.len().saturating_sub(1)));
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        if key.kind != KeyEventKind::Press {
            return ViewOutcome::Continue;
        }
        match key.code {
            KeyCode::Esc => ViewOutcome::Cancelled,
            KeyCode::Up if !self.visible().is_empty() => {
                self.selected = self
                    .selected
                    .checked_sub(1)
                    .unwrap_or(self.visible().len() - 1);
                ViewOutcome::Continue
            }
            KeyCode::Down if !self.visible().is_empty() => {
                self.selected = (self.selected + 1) % self.visible().len();
                ViewOutcome::Continue
            }
            KeyCode::Enter if self.style == SurfaceStyle::Manager && !self.visible().is_empty() => {
                let id = self.visible()[self.selected].id.clone();
                self.action = Some(ModalAction::SurfaceSelected {
                    surface: self.surface.clone(),
                    id,
                });
                ViewOutcome::Accepted
            }
            KeyCode::Char(command @ ('c' | 'C' | 'x' | 'X' | 'r' | 'R'))
                if self.surface == "mcp" && !self.visible().is_empty() =>
            {
                let server = self.visible()[self.selected].id.clone();
                self.action = Some(ModalAction::SurfaceSelected {
                    surface: "mcp-action".to_owned(),
                    id: format!("{}:{server}", command.to_ascii_lowercase()),
                });
                ViewOutcome::Accepted
            }
            KeyCode::Backspace if self.style == SurfaceStyle::Manager => {
                self.filter.pop();
                self.selected = 0;
                ViewOutcome::Continue
            }
            KeyCode::Char(c) if self.style == SurfaceStyle::Manager => {
                self.filter.push(c);
                self.selected = 0;
                ViewOutcome::Continue
            }
            _ => ViewOutcome::Continue,
        }
    }
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines = vec![Line::from(Span::styled(self.title, theme::title()))];
        for line in &self.intro {
            lines.push(Line::from(Span::styled(line, theme::dim())));
        }
        if !self.filter.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("› ", theme::accent()),
                Span::raw(&self.filter),
            ]));
        }
        if !self.intro.is_empty() && !self.items.is_empty() {
            lines.push(Line::default());
        }
        let reserved = self.intro.len()
            + usize::from(!self.filter.is_empty())
            + usize::from(!self.intro.is_empty() && !self.items.is_empty())
            + 3;
        let item_rows = usize::from(area.height).saturating_sub(reserved).max(1);
        let scroll = self.selected.saturating_sub(item_rows.saturating_sub(1));
        for (index, item) in self
            .visible()
            .iter()
            .enumerate()
            .skip(scroll)
            .take(item_rows)
        {
            let selected = index == self.selected;
            lines.push(Line::from(vec![
                Span::styled(
                    if selected { " ❯ " } else { "   " },
                    if selected {
                        theme::selected_row()
                    } else {
                        theme::muted()
                    },
                ),
                Span::styled(
                    format!("{:<26}", item.label),
                    if selected {
                        theme::selected_row()
                    } else {
                        theme::body()
                    },
                ),
                Span::styled(
                    &item.value,
                    if selected {
                        theme::selected_row()
                    } else {
                        theme::dim()
                    },
                ),
            ]));
        }
        lines.push(Line::from(Span::styled(
            "────────────────────────────────────────────",
            theme::dim(),
        )));
        lines.push(Line::from(Span::styled(self.footer, theme::dim())));
        Paragraph::new(lines)
            .style(Style::default().bg(theme::CANVAS_BG))
            .render(area, buf);
    }
    fn preferred_height(&self, _width: u16) -> u16 {
        u16::try_from((self.intro.len() + self.items.len() + 4).min(28)).unwrap_or(28)
    }
    fn title(&self) -> &'static str {
        self.title
    }
    fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn refresh_keeps_filter_and_selection_and_handles_disappearing_matches() {
        let items = |value: &str| {
            vec![
                SurfaceItem {
                    id: "a".into(),
                    label: "Alpha".into(),
                    value: value.into(),
                },
                SurfaceItem {
                    id: "b".into(),
                    label: "Beta".into(),
                    value: value.into(),
                },
            ]
        };
        let mut view = SurfaceView::manager("MCP", "mcp", vec![], items("sleeping"), "");
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        view.handle_key(key(KeyCode::Char('b')));
        view.refresh_surface("mcp", &items("connected"));
        view.handle_key(key(KeyCode::Enter));
        assert_eq!(
            view.take_action(),
            Some(ModalAction::SurfaceSelected {
                surface: "mcp".into(),
                id: "b".into()
            })
        );
        let area = Rect::new(0, 0, 80, 12);
        let mut buffer = Buffer::empty(area);
        view.render(area, &mut buffer);
        let text = buffer
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(text.contains("connected"));
        assert!(!text.contains("Alpha"));
        view.refresh_surface("mcp", &[]);
        assert_eq!(view.handle_key(key(KeyCode::Enter)), ViewOutcome::Continue);
        view.refresh_surface("mcp", &items("sleeping"));
        view.handle_key(key(KeyCode::Enter));
        assert_eq!(
            view.take_action(),
            Some(ModalAction::SurfaceSelected {
                surface: "mcp".into(),
                id: "b".into()
            })
        );
    }
}
