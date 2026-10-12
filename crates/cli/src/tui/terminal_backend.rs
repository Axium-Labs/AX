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

#[cfg(windows)]
pub fn console_input_mode() -> io::Result<u32> {
    use crossterm_winapi::{ConsoleMode, Handle};
    ConsoleMode::from(Handle::current_in_handle()?).mode()
}

#[cfg(windows)]
pub fn set_console_input_mode(mode: u32) -> io::Result<()> {
    use crossterm_winapi::{ConsoleMode, Handle};
    ConsoleMode::from(Handle::current_in_handle()?).set_mode(mode)
}

/// Leave click/drag selection and copying to the terminal host.
pub fn enable_native_selection() -> io::Result<()> {
    use crossterm::Command;
    use std::io::Write;
    #[cfg(windows)]
    let _ = crossterm::ansi_support::supports_ansi();
    // Serialize the ANSI command directly: Crossterm's Windows disable command
    // otherwise requires a prior enable and restores its saved input mode.
    let mut sequence = String::new();
    crossterm::event::DisableMouseCapture
        .write_ansi(&mut sequence)
        .map_err(io::Error::other)?;
    io::stdout().write_all(sequence.as_bytes())?;
    io::stdout().flush()?;
    #[cfg(windows)]
    {
        // Quick Edit selects text in classic ConHost. Keep raw keyboard input,
        // enable window resize events, and remove application mouse events.
        let mode = console_input_mode()?;
        set_console_input_mode((mode | 0x0040 | 0x0080 | 0x0008) & !0x0010)?;
    }
    Ok(())
}

/// The native Windows screen buffer may have a nonzero visible window origin.
/// Crossterm 0.28 subtracts its top only for some cursor rows and ignores its
/// left offset. Ratatui expects window-relative positions, while native ANSI
/// cursor addressing uses the screen buffer's absolute column.
pub struct ConsoleBackend<B>(pub WideCellBackend<B>);

impl<B: Backend> Backend for ConsoleBackend<B> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        #[cfg(windows)]
        {
            let left = crossterm_winapi::ScreenBuffer::current()?
                .info()?
                .terminal_window()
                .left;
            let left = u16::try_from(left.max(0)).unwrap_or_default();
            self.0
                .draw(content.map(|(x, y, cell)| (x.saturating_add(left), y, cell)))
        }
        #[cfg(not(windows))]
        self.0.draw(content)
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
        #[cfg(windows)]
        {
            let info = crossterm_winapi::ScreenBuffer::current()?.info()?;
            let cursor = info.cursor_pos();
            let window = info.terminal_window();
            Ok(Position::new(
                u16::try_from(cursor.x.saturating_sub(window.left).max(0)).unwrap_or_default(),
                u16::try_from(cursor.y.saturating_sub(window.top).max(0)).unwrap_or_default(),
            ))
        }
        #[cfg(not(windows))]
        self.0.get_cursor_position()
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        #[cfg(windows)]
        let position = {
            let left = crossterm_winapi::ScreenBuffer::current()?
                .info()?
                .terminal_window()
                .left;
            Position::new(
                position
                    .x
                    .saturating_add(u16::try_from(left.max(0)).unwrap_or_default()),
                position.y,
            )
        };
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
    use crate::tui::theme;
    use ratatui::backend::CrosstermBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
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
        let style = Style::default().fg(theme::BODY).bg(theme::USER_MESSAGE_BG);
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
                vt100::Color::Rgb(36, 42, 54)
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
