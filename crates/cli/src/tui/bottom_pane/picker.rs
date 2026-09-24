//! Generic borderless list picker used by the model and session pickers.
//! Modeled on Codex's `ListSelectionView`: live filter, wrap-around movement,
//! Enter to accept, Esc to cancel.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use super::super::theme;
use super::view::{ModalAction, PaneView, ViewOutcome};

pub const MAX_VISIBLE_ROWS: usize = 10;

#[derive(Clone, Debug)]
pub struct PickerItem {
    /// Stable identity (provider id or session id).
    pub id: String,
    /// Primary label.
    pub label: String,
    /// Secondary dim description.
    pub detail: String,
    pub project: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Session,
}

pub struct Picker {
    kind: PickerKind,
    title: &'static str,
    items: Vec<PickerItem>,
    filter: String,
    selected: usize,
    project_filter: Option<String>,
    /// Session actions use Ctrl shortcuts so every character remains searchable.
    extra_hint: bool,
    outcome: ViewOutcome,
    action: Option<ModalAction>,
}

impl Picker {
    pub fn for_sessions(items: Vec<PickerItem>) -> Self {
        Self {
            kind: PickerKind::Session,
            title: "Resume a previous session",
            items,
            filter: String::new(),
            selected: 0,
            project_filter: None,
            extra_hint: true,
            outcome: ViewOutcome::Continue,
            action: None,
        }
    }

    fn filtered(&self) -> Vec<&PickerItem> {
        let needle = self.filter.to_ascii_lowercase();
        self.items
            .iter()
            .filter(|item| {
                self.project_filter
                    .as_ref()
                    .is_none_or(|project| project == &item.project)
                    && (item.label.to_ascii_lowercase().contains(&needle)
                        || item.id.to_ascii_lowercase().contains(&needle)
                        || item.detail.to_ascii_lowercase().contains(&needle))
            })
            .collect()
    }

    fn clamp_selection(&mut self) {
        let count = self.filtered().len();
        if count == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(count - 1);
        }
    }

    fn accept(&mut self) {
        let Some(item) = self.filtered().get(self.selected).copied() else {
            return;
        };
        self.action = Some(match self.kind {
            PickerKind::Session => ModalAction::SessionOpen(item.id.clone()),
        });
        self.outcome = ViewOutcome::Accepted;
    }

    fn render_rows(&self, area: Rect, buf: &mut Buffer) {
        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(self.title, theme::title())));
        lines.push(Line::from(Span::styled(
            format!(
                "Project: {}",
                self.project_filter.as_deref().unwrap_or("all")
            ),
            theme::dim(),
        )));
        if !self.filter.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("› ", theme::accent()),
                Span::styled(self.filter.clone(), theme::body()),
            ]));
        }
        let items = self.filtered();
        if items.is_empty() {
            lines.push(Line::from(Span::styled("  no matches", theme::dim())));
        }
        let count = items.len();
        let scroll = self
            .selected
            .saturating_sub(MAX_VISIBLE_ROWS - 1)
            .min(count.saturating_sub(MAX_VISIBLE_ROWS));
        for (index, item) in items.iter().enumerate().skip(scroll).take(MAX_VISIBLE_ROWS) {
            let selected = index == self.selected;
            let marker = if selected { "› " } else { "  " };
            let row_style = if selected {
                theme::selected_row()
            } else {
                theme::body()
            };
            let label_width = 22usize;
            lines.push(Line::from(vec![
                Span::styled(
                    marker,
                    if selected {
                        theme::selected_row()
                    } else {
                        theme::muted()
                    },
                ),
                Span::styled(format!("{:<label_width$}", item.label), row_style),
                Span::styled(
                    item.detail.clone(),
                    if selected {
                        theme::selected_row()
                    } else {
                        theme::dim()
                    },
                ),
            ]));
        }
        let hint = if self.extra_hint {
            "↑↓ move · enter open · Ctrl+P project · Ctrl+R rename · Ctrl+D delete · Ctrl+N new"
        } else {
            "↑↓ move · enter select · esc back"
        };
        lines.push(Line::from(Span::styled(hint, theme::dim())));

        ratatui::widgets::Paragraph::new(lines)
            .style(Style::default().bg(theme::CANVAS_BG))
            .render(area, buf);
    }
}

impl PaneView for Picker {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        if key.kind != KeyEventKind::Press {
            return ViewOutcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('p') => {
                    let mut projects = self
                        .items
                        .iter()
                        .map(|item| item.project.clone())
                        .collect::<Vec<_>>();
                    projects.sort();
                    projects.dedup();
                    self.project_filter = match &self.project_filter {
                        None => projects.first().cloned(),
                        Some(current) => projects
                            .iter()
                            .position(|name| name == current)
                            .and_then(|index| projects.get(index + 1))
                            .cloned(),
                    };
                    self.selected = 0;
                }
                KeyCode::Char('r') => {
                    if let Some(item) = self.filtered().get(self.selected).copied() {
                        self.action = Some(ModalAction::SessionRenameStart {
                            id: item.id.clone(),
                            title: item.label.clone(),
                        });
                        self.outcome = ViewOutcome::Accepted;
                    }
                }
                KeyCode::Char('n') => {
                    self.action = Some(ModalAction::SessionNew);
                    self.outcome = ViewOutcome::Accepted;
                }
                KeyCode::Char('d') => {
                    if let Some(item) = self.filtered().get(self.selected).copied() {
                        self.action = Some(ModalAction::SessionDelete(item.id.clone()));
                        self.outcome = ViewOutcome::Accepted;
                    }
                }
                _ => {}
            }
            return self.outcome;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            return ViewOutcome::Continue;
        }
        match key.code {
            KeyCode::Esc => {
                self.outcome = ViewOutcome::Cancelled;
            }
            KeyCode::Up => {
                let count = self.filtered().len();
                if count > 0 {
                    self.selected = if self.selected == 0 {
                        count - 1
                    } else {
                        self.selected - 1
                    };
                }
            }
            KeyCode::Down => {
                let count = self.filtered().len();
                if count > 0 {
                    self.selected = (self.selected + 1) % count;
                }
            }
            KeyCode::Enter => self.accept(),
            KeyCode::Backspace => {
                self.filter.pop();
                self.clamp_selection();
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.clamp_selection();
            }
            _ => {}
        }
        self.outcome
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_rows(area, buf);
    }

    fn preferred_height(&self, _width: u16) -> u16 {
        let rows = self.filtered().len().min(MAX_VISIBLE_ROWS);
        let filter_row = u16::from(!self.filter.is_empty());
        // title + project filter + optional query + rows + hint
        3 + filter_row + u16::try_from(rows).unwrap_or(u16::MAX)
    }

    fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_search_and_project_filter_keep_actions_on_visible_item() {
        let mut picker = Picker::for_sessions(vec![
            PickerItem {
                id: "a|1".into(),
                label: "Deploy".into(),
                detail: "alpha".into(),
                project: "alpha".into(),
            },
            PickerItem {
                id: "b|2".into(),
                label: "Debug".into(),
                detail: "beta".into(),
                project: "beta".into(),
            },
        ]);
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        picker.handle_key(key(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(picker.filtered().len(), 2);
        picker.handle_key(key(KeyCode::Char('e'), KeyModifiers::NONE));
        assert_eq!(picker.filtered().len(), 2);
        picker.handle_key(key(KeyCode::Char('b'), KeyModifiers::NONE));
        assert_eq!(picker.filtered()[0].id, "b|2");
        picker.handle_key(key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert!(picker.filtered().is_empty());
        picker.handle_key(key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(picker.filtered()[0].id, "b|2");
        assert_eq!(
            picker.handle_key(key(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            ViewOutcome::Accepted
        );
        assert!(
            matches!(picker.take_action(), Some(ModalAction::SessionRenameStart { id, .. }) if id == "b|2")
        );
    }
}
