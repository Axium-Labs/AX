//! Minimal multi-line text editor used by the AX composer.
//!
//! The cursor is tracked as a byte offset into the buffer. All movement is
//! character based (UTF-8 safe). Soft wrapping is performed by the composer at
//! render time; this type only tracks hard newlines inserted by the user.

#[derive(Debug, Default, Clone)]
pub struct TextArea {
    text: String,
    /// Byte offset of the cursor.
    cursor: usize,
}

impl TextArea {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    #[allow(dead_code)]
    pub fn set(&mut self, value: impl Into<String>) {
        self.text = value.into();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
        self.text.drain(prev..self.cursor);
        self.cursor = prev;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let next = self.text[self.cursor..]
            .char_indices()
            .nth(1)
            .map_or(self.text.len(), |(i, _)| self.cursor + i);
        self.text.drain(self.cursor..next);
    }

    pub fn move_left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor = self.text[self.cursor..]
                .char_indices()
                .nth(1)
                .map_or(self.text.len(), |(i, _)| self.cursor + i);
        }
    }

    /// Offset of the start of the logical line containing the cursor.
    fn line_bounds(&self) -> (usize, usize) {
        let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i);
        (start, end)
    }

    pub fn move_line_start(&mut self) {
        let (start, _) = self.line_bounds();
        self.cursor = start;
    }

    pub fn move_line_end(&mut self) {
        let (_, end) = self.line_bounds();
        self.cursor = end;
    }

    pub fn move_up(&mut self) {
        let (start, _) = self.line_bounds();
        if start == 0 {
            self.cursor = 0;
            return;
        }
        let column = self.text[start..self.cursor].chars().count();
        let prev_end = start - 1;
        let prev_start = self.text[..prev_end].rfind('\n').map_or(0, |i| i + 1);
        let prev_len = self.text[prev_start..prev_end].chars().count();
        let offset = column.min(prev_len);
        self.cursor = prev_start
            + self.text[prev_start..prev_end]
                .char_indices()
                .nth(offset)
                .map_or(prev_end - prev_start, |(i, _)| i);
    }

    pub fn move_down(&mut self) {
        let (_, end) = self.line_bounds();
        if end == self.text.len() {
            self.cursor = self.text.len();
            return;
        }
        let (start, _) = self.line_bounds();
        let column = self.text[start..self.cursor].chars().count();
        let next_start = end + 1;
        let next_end = self.text[next_start..]
            .find('\n')
            .map_or(self.text.len(), |i| next_start + i);
        let next_len = self.text[next_start..next_end].chars().count();
        let offset = column.min(next_len);
        self.cursor = next_start
            + self.text[next_start..next_end]
                .char_indices()
                .nth(offset)
                .map_or(next_end - next_start, |(i, _)| i);
    }

    pub fn delete_word_backward(&mut self) {
        let mut target = self.cursor;
        let bytes = self.text.as_bytes();
        while target > 0 && bytes[target - 1].is_ascii_whitespace() {
            target -= 1;
        }
        while target > 0 && !bytes[target - 1].is_ascii_whitespace() {
            target -= 1;
        }
        if target < self.cursor {
            self.text.drain(target..self.cursor);
            self.cursor = target;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_and_cursor_movement() {
        let mut area = TextArea::new();
        area.set("hello world");
        area.move_left();
        area.backspace();
        assert_eq!(area.text(), "hello word");
        area.insert_char('l');
        assert_eq!(area.text(), "hello world");
        area.set("one\ntwo");
        area.move_line_start();
        area.move_up();
        assert_eq!(area.cursor(), 0);
        area.move_down();
        assert_eq!(&area.text[area.cursor()..], "two");
    }

    #[test]
    fn multiline_navigation() {
        let mut area = TextArea::new();
        area.set("abc\ndefgh");
        // cursor at end of the second line; moving up clamps to column 3.
        area.move_up();
        assert_eq!(area.cursor(), 3);
        area.move_line_end();
        assert_eq!(area.cursor(), 3);
        // moving back down lands on column 3 of "defgh" -> "g".
        area.move_down();
        assert_eq!(&area.text[area.cursor()..], "gh");
    }
}
