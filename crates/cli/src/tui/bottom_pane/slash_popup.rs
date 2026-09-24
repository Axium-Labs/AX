//! Slash command popup rendered immediately above the composer.
//!
//! Mirrors Codex's `CommandPopup`: the popup opens as soon as the composer
//! text starts with `/`, filters live against the first token, and is driven
//! by Up/Down + Enter + Esc.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use std::time::{Duration, Instant};

use super::super::commands::{SlashCommandDef, filter_commands};
use super::super::theme;

pub const MAX_POPUP_ROWS: usize = 12;
const SELECTION_FEEDBACK: Duration = Duration::from_millis(150);

/// Outcome of routing a key to the open popup.
#[derive(Clone, Debug)]
pub enum SlashKeyOutcome {
    Handled,
    Selected(&'static SlashCommandDef),
    Closed,
}

pub struct SlashPopup {
    open: bool,
    /// Set while the user explicitly dismissed the popup with Esc.
    suppressed: bool,
    filter: String,
    selected: usize,
    selected_at: Instant,
}

impl Default for SlashPopup {
    fn default() -> Self {
        Self::new()
    }
}

impl SlashPopup {
    pub fn new() -> Self {
        Self {
            open: false,
            suppressed: false,
            filter: String::new(),
            selected: 0,
            selected_at: Instant::now(),
        }
    }

    /// Recompute popup state from the current composer text.
    pub fn sync(&mut self, text: &str) {
        let was_open = self.open;
        let previous_filter = self.filter.clone();
        if self.suppressed {
            self.open = false;
            return;
        }
        let first_line = text.lines().next().unwrap_or("");
        if let Some(after) = first_line.strip_prefix('/') {
            // Only the first token (no whitespace yet) keeps the popup open.
            self.open = !after.chars().any(char::is_whitespace);
            self.filter.replace_range(.., after);
        } else {
            self.open = false;
            self.filter.clear();
        }
        let count = self.items().len();
        if count == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(count - 1);
        }
        if self.open && (!was_open || self.filter != previous_filter) {
            self.selected_at = Instant::now();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open && !self.items().is_empty()
    }

    pub fn close(&mut self) {
        self.open = false;
        self.suppressed = true;
    }

    /// Reset the explicit-dismiss marker (called on any edit that changes the
    /// leading slash structure).
    pub fn unsuppress(&mut self) {
        self.suppressed = false;
    }

    pub fn items(&self) -> Vec<&'static SlashCommandDef> {
        filter_commands(&self.filter)
    }

    pub fn move_up(&mut self) {
        let count = self.items().len();
        if count > 0 {
            self.selected = if self.selected == 0 {
                count - 1
            } else {
                self.selected - 1
            };
            self.selected_at = Instant::now();
        }
    }

    pub fn move_down(&mut self) {
        let count = self.items().len();
        if count > 0 {
            self.selected = (self.selected + 1) % count;
            self.selected_at = Instant::now();
        }
    }

    pub fn is_animating(&self) -> bool {
        self.is_open()
            && self.selected_at.elapsed() < SELECTION_FEEDBACK + Duration::from_millis(80)
    }

    fn selection_active(&self) -> bool {
        self.selected_at.elapsed() < SELECTION_FEEDBACK
    }

    pub fn selected(&self) -> Option<&'static SlashCommandDef> {
        self.items().get(self.selected).copied()
    }

    /// Route Up/Down/Enter/Esc/Tab while the popup is open.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<SlashKeyOutcome> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT)
        {
            return None;
        }
        match key.code {
            KeyCode::Up => {
                self.move_up();
                Some(SlashKeyOutcome::Handled)
            }
            KeyCode::Down | KeyCode::Tab => {
                self.move_down();
                Some(SlashKeyOutcome::Handled)
            }
            KeyCode::Enter => self.selected().map(SlashKeyOutcome::Selected),
            KeyCode::Esc => {
                self.close();
                Some(SlashKeyOutcome::Closed)
            }
            _ => None,
        }
    }

    pub fn height(&self) -> usize {
        if !self.is_open() {
            return 0;
        }
        self.items().len().min(MAX_POPUP_ROWS)
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if !self.is_open() {
            return;
        }
        let items = self.items();
        let start = self
            .selected
            .saturating_sub(MAX_POPUP_ROWS.saturating_sub(1));
        let rows = items
            .into_iter()
            .enumerate()
            .skip(start)
            .take(MAX_POPUP_ROWS)
            .map(|(index, command)| {
                let selected = index == self.selected;
                let marker = if selected { "› " } else { "  " };
                let name_style = if selected && self.selection_active() {
                    theme::selected_row_active()
                } else if selected {
                    theme::selected_row()
                } else {
                    theme::accent()
                };
                let desc_style = if selected {
                    theme::selected_row()
                } else {
                    theme::muted()
                };
                let marker_style = if selected {
                    theme::selected_row()
                } else {
                    theme::muted()
                };
                let hint_style = if selected {
                    theme::selected_row()
                } else {
                    theme::muted()
                };
                let mut spans = vec![
                    Span::styled(marker, marker_style),
                    Span::styled(format!("{:<14}", command.name), name_style),
                ];
                if let Some(hint) = command.argument_hint {
                    spans.push(Span::styled(format!("{hint:<20}"), hint_style));
                }
                spans.push(Span::styled(command.description, desc_style));
                Line::from(spans)
            })
            .collect::<Vec<_>>();
        let mut lines = rows;
        // Dim separator directly above the popup list.
        lines.insert(0, Line::from(Span::styled(" commands", theme::muted())));
        lines.push(Line::from(Span::styled(
            " ↑↓ navigate   Enter select   Esc close",
            theme::muted(),
        )));
        let paragraph =
            ratatui::widgets::Paragraph::new(lines).style(Style::default().bg(theme::CANVAS_BG));
        paragraph.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_filters_selects_and_closes() {
        let mut popup = SlashPopup::new();
        popup.sync("/");
        assert!(popup.is_open());
        popup.sync("/mo");
        let names = popup
            .items()
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"/model"));
        assert_eq!(names, vec!["/model", "/memory"]);
        assert!(!names.contains(&"/exit"));
        popup.move_down();
        popup.close();
        assert!(!popup.is_open());
        // Stays suppressed after close until the composer changes.
        popup.sync("/mo");
        assert!(!popup.is_open());
        popup.unsuppress();
        popup.sync("/mo");
        assert!(popup.is_open());
    }

    #[test]
    fn hints_and_descriptions_use_readable_explicit_colors() {
        let mut popup = SlashPopup::new();
        popup.sync("/mo");
        let area = Rect::new(0, 0, 100, 6);
        let mut buffer = Buffer::empty(area);
        popup.render(area, &mut buffer);
        assert_eq!(buffer[(2, 1)].fg, theme::ACTIVE_FG);
        assert_eq!(buffer[(16, 1)].fg, theme::SELECTED_FG);
        assert_eq!(buffer[(16, 2)].fg, theme::MUTED);
    }

    #[test]
    fn navigation_hint_stays_visible_when_scrolling_commands() {
        let mut popup = SlashPopup::new();
        popup.sync("/");
        popup.selected = popup.items().len() - 1;
        let area = Rect::new(0, 0, 100, u16::try_from(popup.height() + 2).unwrap());
        let mut buffer = Buffer::empty(area);
        popup.render(area, &mut buffer);
        let last = (0..area.width)
            .map(|x| buffer[(x, area.height - 1)].symbol())
            .collect::<String>();
        assert!(last.contains("Esc close"));
    }

    #[test]
    fn whitespace_after_token_closes_popup() {
        let mut popup = SlashPopup::new();
        popup.sync("/model ");
        assert!(!popup.is_open());
    }
}
