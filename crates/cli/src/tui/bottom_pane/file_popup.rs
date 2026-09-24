use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use super::super::theme;
use crate::file_reference;

pub enum FileKeyOutcome {
    Handled,
    Selected(usize, String),
}

pub struct FilePopup {
    root: PathBuf,
    paths: Option<Vec<String>>,
    start: Option<usize>,
    query: String,
    selected: usize,
    selected_at: Instant,
    suppressed: bool,
}

impl FilePopup {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            paths: None,
            start: None,
            query: String::new(),
            selected: 0,
            selected_at: Instant::now(),
            suppressed: false,
        }
    }

    pub fn sync(&mut self, text: &str, cursor: usize) {
        let Some((start, query)) = file_reference::current_query(text, cursor) else {
            self.start = None;
            self.suppressed = false;
            return;
        };
        if self.start != Some(start) {
            self.selected = 0;
            self.suppressed = false;
            self.selected_at = Instant::now();
        }
        if self.query != query {
            self.suppressed = false;
            self.selected_at = Instant::now();
        }
        self.start = Some(start);
        query.clone_into(&mut self.query);
        if self.paths.is_none() {
            self.paths = Some(file_reference::files(&self.root));
        }
        self.selected = self.selected.min(self.matches().len().saturating_sub(1));
    }

    fn matches(&self) -> Vec<&str> {
        file_reference::matches(&self.query, self.paths.as_deref().unwrap_or(&[]))
    }

    pub fn is_open(&self) -> bool {
        self.start.is_some() && !self.suppressed && !self.matches().is_empty()
    }
    pub fn is_animating(&self) -> bool {
        self.is_open() && self.selected_at.elapsed() < Duration::from_millis(230)
    }
    fn selection_active(&self) -> bool {
        self.selected_at.elapsed() < Duration::from_millis(150)
    }
    pub fn height(&self) -> u16 {
        if self.is_open() {
            u16::try_from(self.matches().len() + 2).unwrap_or(10)
        } else {
            0
        }
    }
    pub fn close(&mut self) {
        self.suppressed = true;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<FileKeyOutcome> {
        if !self.is_open()
            || key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        match key.code {
            KeyCode::Up => {
                self.selected = self
                    .selected
                    .checked_sub(1)
                    .unwrap_or(self.matches().len() - 1);
                self.selected_at = Instant::now();
                Some(FileKeyOutcome::Handled)
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1) % self.matches().len();
                self.selected_at = Instant::now();
                Some(FileKeyOutcome::Handled)
            }
            KeyCode::Esc => {
                self.close();
                Some(FileKeyOutcome::Handled)
            }
            KeyCode::Enter | KeyCode::Tab => {
                let path = self.matches().get(self.selected)?.to_string();
                let start = self.start?;
                self.start = None;
                Some(FileKeyOutcome::Selected(start, path))
            }
            _ => None,
        }
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if !self.is_open() {
            return;
        }
        let mut lines = vec![Line::styled(" files", theme::muted())];
        for (index, path) in self.matches().iter().enumerate() {
            let style = if index == self.selected && self.selection_active() {
                theme::selected_row_active()
            } else if index == self.selected {
                theme::selected_row()
            } else {
                theme::body()
            };
            lines.push(Line::from(Span::styled(
                format!("{} {path}", if index == self.selected { "›" } else { " " }),
                style,
            )));
        }
        lines.push(Line::styled(
            " ↑↓ navigate · Enter add · Esc close",
            theme::muted(),
        ));
        Paragraph::new(lines).render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn at_query_selects_and_inserts_matching_file() {
        let root = std::env::temp_dir().join(format!("ax-file-popup-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("README.md"), "hello").unwrap();
        let mut popup = FilePopup::new(root.clone());
        popup.sync("check @READ", "check @READ".len());
        assert!(popup.is_open());
        assert!(
            matches!(popup.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Some(FileKeyOutcome::Selected(6, path)) if path == "README.md")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
