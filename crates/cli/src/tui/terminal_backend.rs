//! Keep terminal history writes consistent with live buffer diffs.
//!
//! Ratatui 0.29's `insert_before` passes every cell to the backend, including
//! the blank continuation cells of wide glyphs. Live `Buffer::diff` skips them.
//! Codex writes history text spans directly; this adapter preserves that same
//! invariant while retaining Ratatui's inline viewport/scrollback management.
use std::io;

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use unicode_width::UnicodeWidthStr;

pub struct WideCellBackend<B>(pub B);

impl<B: Backend> Backend for WideCellBackend<B> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut occupied_until: Option<(u16, usize)> = None;
        self.0.draw(content.filter(|(x, y, cell)| {
            if occupied_until.is_some_and(|(row, end)| row == *y && usize::from(*x) < end) {
                return false;
            }
            occupied_until = Some((*y, usize::from(*x) + cell.symbol().width().max(1)));
            !cell.skip
        }))
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.0.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.0.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.0.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.0.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.0.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.0.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.0.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.0.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.0.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::CrosstermBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Paragraph, Widget};

    fn terminal_screen(buffer: &Buffer, history: bool) -> vt100::Screen {
        crossterm::style::force_color_output(true);
        let mut bytes = Vec::new();
        let mut backend = WideCellBackend(CrosstermBackend::new(&mut bytes));
        if history {
            // The exact dense cell iteration used by Terminal::insert_before.
            let width = buffer.area.width;
            backend
                .draw(buffer.content.iter().enumerate().map(|(i, cell)| {
                    let i = u16::try_from(i).unwrap();
                    (i % width, i / width, cell)
                }))
                .unwrap();
        } else {
            backend
                .draw(Buffer::empty(buffer.area).diff(buffer).into_iter())
                .unwrap();
        }
        backend.flush().unwrap();
        let mut parser = vt100::Parser::new(buffer.area.height, buffer.area.width, 0);
        parser.process(&bytes);
        parser.screen().clone()
    }

    #[test]
    fn history_and_live_ansi_render_identical_chinese_and_backgrounds() {
        let area = Rect::new(0, 0, 40, 4);
        let style = Style::default()
            .fg(Color::Rgb(212, 212, 212))
            .bg(Color::Rgb(52, 52, 64));
        let mut buffer = Buffer::empty(area);
        Paragraph::new(vec![
            Line::styled("你可以帮我做什么？", style),
            Line::from(vec![
                Span::styled("代码开发", style.add_modifier(Modifier::BOLD)),
                Span::raw(" Rust / 中文"),
            ]),
            Line::raw("中A文B e\u{301} 😀 done"),
            Line::raw("最后一行"),
        ])
        .render(area, &mut buffer);
        let live = terminal_screen(&buffer, false);
        let history = terminal_screen(&buffer, true);
        let normalized = |screen: &vt100::Screen| {
            screen
                .contents()
                .lines()
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(normalized(&history), normalized(&live));
        assert!(normalized(&history).starts_with(
            "你可以帮我做什么？\n代码开发 Rust / 中文\n中A文B e\u{301} 😀 done\n最后一行"
        ));
        for row in 0..area.height {
            for column in 0..area.width {
                let actual = history.cell(row, column).unwrap();
                let expected = live.cell(row, column).unwrap();
                assert_eq!(
                    actual.contents().trim_end(),
                    expected.contents().trim_end(),
                    "cell {column},{row}"
                );
                assert_eq!(
                    actual.bgcolor(),
                    expected.bgcolor(),
                    "background {column},{row}"
                );
                assert_eq!(
                    actual.fgcolor(),
                    expected.fgcolor(),
                    "foreground {column},{row}"
                );
                assert_eq!(actual.bold(), expected.bold(), "bold {column},{row}");
            }
        }
        for column in (0..u16::try_from("你可以帮我做什么？".width()).unwrap()).step_by(2)
        {
            assert_eq!(
                history.cell(0, column).unwrap().bgcolor(),
                vt100::Color::Rgb(52, 52, 64)
            );
        }
    }

    #[test]
    fn wide_to_narrow_redraw_clears_old_wide_cells() {
        let area = Rect::new(0, 0, 10, 1);
        let mut old = Buffer::empty(area);
        Paragraph::new("中文测试").render(area, &mut old);
        let mut new = Buffer::empty(area);
        Paragraph::new("a中b").render(area, &mut new);
        let mut bytes = Vec::new();
        let mut backend = WideCellBackend(CrosstermBackend::new(&mut bytes));
        backend
            .draw(Buffer::empty(area).diff(&old).into_iter())
            .unwrap();
        backend.draw(old.diff(&new).into_iter()).unwrap();
        backend.flush().unwrap();
        let mut parser = vt100::Parser::new(1, 10, 0);
        parser.process(&bytes);
        assert_eq!(parser.screen().contents().trim_end(), "a中b");
    }
}
