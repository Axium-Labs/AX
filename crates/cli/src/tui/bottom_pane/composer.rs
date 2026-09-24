//! Pi-style fixed bottom editor: an accent horizontal frame, compact `›`
//! prompt, multi-line editing, and no full-width gray Codex composer fill.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

use super::super::theme;
use super::textarea::TextArea;

const PROMPT_COLS: usize = 2;
const MAX_COMPOSER_ROWS: usize = 8;
const PLACEHOLDER: &str = "Message AX…   (/ for commands, alt+enter for a new line)";

pub struct Composer {
    area: TextArea,
}

impl Default for Composer {
    fn default() -> Self {
        Self::new()
    }
}

/// One soft-wrapped visual row.
struct VisualRow {
    text: String,
    first: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerKey {
    Ignored,
    Edited,
    Submit,
}

impl Composer {
    pub fn new() -> Self {
        Self {
            area: TextArea::new(),
        }
    }

    pub fn text(&self) -> &str {
        self.area.text()
    }

    pub fn cursor(&self) -> usize {
        self.area.cursor()
    }

    pub fn insert_file_reference(&mut self, start: usize, path: &str) {
        let reference = if path.contains(char::is_whitespace) {
            format!("@\"{path}\" ")
        } else {
            format!("@{path} ")
        };
        self.area
            .replace_range(start, self.area.cursor(), &reference);
    }

    pub fn clear(&mut self) {
        self.area.clear();
    }

    /// Route a key to the editor.
    pub fn handle_key(&mut self, key: KeyEvent) -> ComposerKey {
        if key.kind != KeyEventKind::Press {
            return ComposerKey::Ignored;
        }
        // Alt+Enter or Ctrl+J inserts a hard newline.
        let newline = key.code == KeyCode::Enter
            && (key.modifiers.contains(KeyModifiers::ALT)
                || key.modifiers.contains(KeyModifiers::CONTROL));
        if newline {
            self.area.insert_newline();
            return ComposerKey::Edited;
        }
        if key.code == KeyCode::Enter && key.modifiers.is_empty() {
            return ComposerKey::Submit;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('w') => self.area.delete_word_backward(),
                KeyCode::Char('a') => self.area.move_line_start(),
                KeyCode::Char('e') => self.area.move_line_end(),
                KeyCode::Char('j') => self.area.insert_newline(),
                _ => return ComposerKey::Ignored,
            }
            return ComposerKey::Edited;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            return ComposerKey::Ignored;
        }
        match key.code {
            KeyCode::Char(c) => self.area.insert_char(c),
            KeyCode::Backspace => self.area.backspace(),
            KeyCode::Delete => self.area.delete(),
            KeyCode::Left => self.area.move_left(),
            KeyCode::Right => self.area.move_right(),
            KeyCode::Up => self.area.move_up(),
            KeyCode::Down => self.area.move_down(),
            KeyCode::Home => self.area.move_line_start(),
            KeyCode::End => self.area.move_line_end(),
            _ => return ComposerKey::Ignored,
        }
        ComposerKey::Edited
    }

    fn wrap_width(width: u16) -> usize {
        (width as usize).saturating_sub(PROMPT_COLS).max(4)
    }

    /// Soft-wrap the buffer into visual rows.
    fn visual_rows(&self, width: u16) -> Vec<VisualRow> {
        let wrap = Self::wrap_width(width);
        let mut rows = Vec::new();
        let logical: Vec<&str> = self.area.text().split('\n').collect();
        for (line_index, line) in logical.into_iter().enumerate() {
            let mut current = String::new();
            let mut current_width = 0usize;
            let mut emitted = false;
            for c in line.chars() {
                let cw = c.width().unwrap_or(0);
                if current_width + cw > wrap && !current.is_empty() {
                    rows.push(VisualRow {
                        text: std::mem::take(&mut current),
                        first: line_index == 0 && !emitted,
                    });
                    emitted = true;
                    current_width = 0;
                }
                current.push(c);
                current_width += cw;
            }
            rows.push(VisualRow {
                text: current,
                first: line_index == 0 && !emitted,
            });
        }
        if rows.is_empty() {
            rows.push(VisualRow {
                text: String::new(),
                first: true,
            });
        }
        rows
    }

    /// Required composer height including Codex-style top/bottom padding.
    pub fn height(&self, width: u16) -> u16 {
        let rows = self.visual_rows(width).len();
        u16::try_from(rows.clamp(1, MAX_COMPOSER_ROWS).saturating_add(2)).unwrap_or(u16::MAX)
    }

    fn line_visual_height(line: &str, wrap: usize) -> usize {
        let width = line.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>();
        if width == 0 {
            1
        } else {
            (width - 1) / wrap + 1
        }
    }

    /// Logical lines with their starting byte offsets.
    fn logical_lines(&self) -> Vec<(&str, usize)> {
        let mut lines = Vec::new();
        let mut start = 0usize;
        for (index, _) in self.area.text().match_indices('\n') {
            lines.push((&self.area.text()[start..index], start));
            start = index + 1;
        }
        lines.push((&self.area.text()[start..], start));
        lines
    }

    /// Cursor position as (visual row, column within wrapped row).
    fn cursor_xy(&self, width: u16) -> (usize, usize) {
        let wrap = Self::wrap_width(width);
        let cursor = self.area.cursor();
        let mut row = 0usize;
        for (line, start) in self.logical_lines() {
            let len = line.len();
            if cursor <= start + len {
                let offset = cursor.saturating_sub(start).min(len);
                let col_width: usize = line[..offset].chars().map(|c| c.width().unwrap_or(0)).sum();
                return (row + col_width / wrap, col_width % wrap);
            }
            row += Self::line_visual_height(line, wrap);
        }
        (row, 0)
    }

    /// Vertical scroll offset in visual rows required to keep cursor visible.
    fn scroll_offset(&self, width: u16, height: u16) -> usize {
        let (cursor_row, _) = self.cursor_xy(width);
        let visible = height as usize;
        cursor_row.checked_sub(visible - 1).unwrap_or_default()
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, enabled: bool) -> Option<Position> {
        let width = area.width;
        let content_height = self.height(width).saturating_sub(2);
        let rows = self.visual_rows(width);
        let scroll = self.scroll_offset(width, content_height);

        let bg = theme::CANVAS_BG;
        let base = theme::body().bg(bg);
        let border = if enabled {
            Style::default().fg(theme::BORDER_ACCENT).bg(bg)
        } else {
            Style::default().fg(theme::BORDER_MUTED).bg(bg)
        };
        let rule = "─".repeat(usize::from(width));
        let mut lines = vec![Line::from(Span::styled(rule.clone(), border))];
        let empty = self.area.is_empty();
        for (index, row) in rows
            .iter()
            .enumerate()
            .skip(scroll)
            .take(content_height as usize)
        {
            let prompt = if row.first {
                Span::styled("› ", theme::accent_bold().bg(bg))
            } else {
                Span::styled("  ", theme::muted().bg(bg))
            };
            let content = if empty && index == 0 {
                Span::styled(PLACEHOLDER, theme::dim().bg(bg))
            } else {
                Span::styled(row.text.clone(), base)
            };
            lines.push(Line::from(vec![prompt, content]));
        }
        while lines.len() < usize::from(content_height.saturating_add(1)) {
            lines.push(Line::from(Span::styled("  ", theme::muted().bg(bg))));
        }
        lines.push(Line::from(Span::styled(rule, border)));

        ratatui::widgets::Paragraph::new(lines)
            .style(base)
            .render(area, buf);

        if !enabled {
            return None;
        }
        let (cursor_row, col) = self.cursor_xy(width);
        let y = area.y + 1 + u16::try_from(cursor_row.saturating_sub(scroll)).unwrap_or(u16::MAX);
        let x = area.x + u16::try_from(PROMPT_COLS + col).unwrap_or(u16::MAX);
        (y < area.bottom() && x < area.right()).then_some(Position { x, y })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn grows_and_wraps() {
        let mut composer = Composer::new();
        assert_eq!(composer.height(40), 3);
        for _ in 0..100 {
            assert_eq!(
                composer.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
                ComposerKey::Edited
            );
        }
        // 38 content columns per visual row at width 40.
        assert_eq!(composer.height(40), 5);
    }

    #[test]
    fn multiline_height_and_cursor() {
        let mut composer = Composer::new();
        for c in "one\ntwo\nthree".chars() {
            let key = if c == '\n' {
                KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)
            } else {
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
            };
            assert_eq!(composer.handle_key(key), ComposerKey::Edited);
        }
        assert_eq!(composer.height(40), 5);
        composer.area.move_line_start();
        let (row, _) = composer.cursor_xy(40);
        assert_eq!(row, 2);
    }

    #[test]
    fn renders_prompt_and_placeholder() {
        let backend = TestBackend::new(60, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let composer = Composer::new();
        terminal
            .draw(|frame| {
                composer.render(frame.area(), frame.buffer_mut(), true);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let all = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains('›'));
        assert!(all.contains("for commands"));
    }
}
