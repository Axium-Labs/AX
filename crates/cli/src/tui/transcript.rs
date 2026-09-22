//! Transcript: the scrolling middle region between the startup card and the
//! bottom pane. No println REPL — every piece of output is a styled entry.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::markdown;
use super::startup::StartupInfo;
use super::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptKind {
    User,
    Agent,
    /// Tool lifecycle / runtime notices (cyan).
    Tool,
    /// Neutral success notices (green).
    Status,
    /// Dim auxiliary information.
    Info,
    Error,
}

#[derive(Clone, Debug)]
pub enum TranscriptEntry {
    Card(StartupInfo),
    /// Styled remainder of a completed message whose prefix is in scrollback.
    Rendered(Vec<Line<'static>>),
    Message(TranscriptKind, String),
    Tool {
        name: String,
        state: ToolState,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Succeeded,
    Failed,
}

pub struct Transcript {
    pub entries: Vec<TranscriptEntry>,
    history: Vec<Line<'static>>,
    /// Distance from the bottom in visual rows.
    pub scroll_from_bottom: usize,
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            history: Vec::new(),
            scroll_from_bottom: 0,
        }
    }

    pub fn push(&mut self, kind: TranscriptKind, text: impl Into<String>) {
        self.entries
            .push(TranscriptEntry::Message(kind, text.into()));
    }

    pub fn card(&mut self, info: StartupInfo) {
        self.entries.push(TranscriptEntry::Card(info));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.history.clear();
        self.scroll_from_bottom = 0;
    }

    pub fn tool_started(&mut self, name: impl Into<String>) {
        self.entries.push(TranscriptEntry::Tool {
            name: name.into(),
            state: ToolState::Running,
        });
    }

    pub fn tool_finished(&mut self, name: &str, success: bool) {
        if let Some(TranscriptEntry::Tool { state, .. }) = self.entries.iter_mut().rev().find(
            |entry| matches!(entry, TranscriptEntry::Tool { name: found, state: ToolState::Running } if found == name),
        ) {
            *state = if success {
                ToolState::Succeeded
            } else {
                ToolState::Failed
            };
        } else {
            self.entries.push(TranscriptEntry::Tool {
                name: name.to_owned(),
                state: if success {
                    ToolState::Succeeded
                } else {
                    ToolState::Failed
                },
            });
        }
    }

    /// Append a streaming content delta to the current agent message, starting
    /// a new entry when needed.
    pub fn push_agent_delta(&mut self, delta: &str) {
        if let Some(TranscriptEntry::Message(TranscriptKind::Agent, text)) = self.entries.last_mut()
        {
            text.push_str(delta);
        } else {
            self.entries.push(TranscriptEntry::Message(
                TranscriptKind::Agent,
                delta.to_owned(),
            ));
        }
    }

    fn entry_lines(entry: &TranscriptEntry, width: u16) -> Vec<Line<'static>> {
        match entry {
            TranscriptEntry::Card(info) => info.lines(width),
            TranscriptEntry::Rendered(lines) => lines.clone(),
            TranscriptEntry::Tool { name, state } => {
                let (marker, label, bg) = match state {
                    ToolState::Running => ("•", "running", theme::TOOL_PENDING_BG),
                    ToolState::Succeeded => ("✓", "completed", theme::TOOL_SUCCESS_BG),
                    ToolState::Failed => ("×", "failed", theme::TOOL_ERROR_BG),
                };
                let inner = usize::from(width.saturating_sub(2));
                let content = format!(" {marker} {name} · {label}");
                let padding = inner.saturating_sub(content.chars().count());
                let style = Style::default().bg(bg).fg(ratatui::style::Color::Reset);
                vec![Line::from(Span::styled(
                    format!("{content}{} ", " ".repeat(padding)),
                    style,
                ))]
            }
            TranscriptEntry::Message(TranscriptKind::User, text) => {
                let style = theme::body().bg(theme::USER_MESSAGE_BG);
                let columns = usize::from(width.max(1));
                let inset = usize::from(width > 2);
                let inner = columns.saturating_sub(inset * 2).max(1);
                // Nonbreaking padding keeps Paragraph from wrapping an all-space
                // background row into a second empty row.
                let blank = || Line::styled("\u{00a0}".repeat(columns), style);
                let mut lines = vec![blank()];
                for logical in text.split('\n') {
                    let mut row = String::new();
                    let mut used = 0;
                    for grapheme in logical.trim_end_matches('\r').graphemes(true) {
                        let glyph = if grapheme == "\t" { "    " } else { grapheme };
                        let cells = glyph.width();
                        if used + cells > inner && !row.is_empty() {
                            lines.push(Line::styled(
                                format!(
                                    "{}{}{}",
                                    "\u{00a0}".repeat(inset),
                                    row,
                                    "\u{00a0}".repeat(columns - inset - used)
                                ),
                                style,
                            ));
                            row.clear();
                            used = 0;
                        }
                        row.push_str(glyph);
                        used += cells;
                    }
                    lines.push(Line::styled(
                        format!(
                            "{}{}{}",
                            "\u{00a0}".repeat(inset),
                            row,
                            "\u{00a0}".repeat(columns.saturating_sub(inset + used))
                        ),
                        style,
                    ));
                }
                lines.push(blank());
                lines.push(Line::raw(""));
                lines
            }
            TranscriptEntry::Message(TranscriptKind::Agent, text) => {
                markdown::agent_lines(text, width)
            }
            TranscriptEntry::Message(kind, text) => {
                let (prefix, style) = match kind {
                    TranscriptKind::User | TranscriptKind::Agent => unreachable!(),
                    TranscriptKind::Tool => ("• ", theme::accent()),
                    TranscriptKind::Status => ("  ", theme::success()),
                    TranscriptKind::Info => ("  ", theme::muted()),
                    TranscriptKind::Error => ("Error: ", theme::error()),
                };
                let mut lines = Vec::new();
                for (index, content) in text.lines().enumerate() {
                    let lead = if index == 0 { prefix } else { "  " };
                    lines.push(Line::from(vec![
                        Span::styled(lead.to_owned(), style),
                        Span::styled(content.to_owned(), style),
                    ]));
                }
                if lines.is_empty() {
                    lines.push(Line::from(Span::styled(prefix.to_owned(), style)));
                }
                lines.push(Line::raw(""));
                lines
            }
        }
    }

    fn entry_height(entry: &TranscriptEntry, width: u16) -> usize {
        Paragraph::new(Self::entry_lines(entry, width))
            .wrap(Wrap { trim: false })
            .line_count(width.max(1))
    }

    pub fn height(&self, width: u16) -> usize {
        self.entries
            .iter()
            .map(|entry| Self::entry_height(entry, width))
            .sum()
    }

    pub fn full_height(&self, width: u16) -> usize {
        self.height(width)
            + if self.history.is_empty() {
                0
            } else {
                Paragraph::new(self.history.clone())
                    .wrap(Wrap { trim: false })
                    .line_count(width.max(1))
            }
    }

    pub fn scroll_up(&mut self, rows: usize, width: u16, visible_rows: u16) {
        let maximum = self
            .full_height(width)
            .saturating_sub(usize::from(visible_rows));
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(rows).min(maximum);
    }

    pub fn scroll_down(&mut self, rows: usize) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(rows);
    }

    /// Move completed overflow into the terminal's real scrollback. Ratatui's
    /// inline viewport otherwise keeps old transcript rows only in an in-memory
    /// window, which makes them disappear when the terminal itself is scrolled.
    pub fn drain_overflow(
        &mut self,
        width: u16,
        visible_rows: u16,
        allow_last: bool,
    ) -> Vec<Line<'static>> {
        let limit = usize::from(visible_rows.max(1));
        let mut total = self.height(width);
        let mut committed = Vec::new();
        while total > limit && !self.entries.is_empty() {
            if !allow_last && self.entries.len() == 1 {
                break;
            }
            let lines = Self::entry_lines(&self.entries[0], width);
            let mut split = 0;
            for line in &lines {
                let rows = Paragraph::new(line.clone())
                    .wrap(Wrap { trim: false })
                    .line_count(width.max(1));
                // Retain a full visible tail. Never clear the viewport just
                // because a completed message is taller than one screen.
                if total.saturating_sub(rows) < limit {
                    break;
                }
                total -= rows;
                split += 1;
            }
            if split == 0 {
                break;
            }
            self.entries.remove(0);
            committed.extend(lines[..split].iter().cloned());
            if split < lines.len() {
                self.entries
                    .insert(0, TranscriptEntry::Rendered(lines[split..].to_vec()));
                break;
            }
        }
        self.history.extend(committed.iter().cloned());
        committed
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        let width = area.width;
        let mut lines = if self.scroll_from_bottom > 0 {
            self.history.clone()
        } else {
            Vec::new()
        };
        for entry in &self.entries {
            lines.extend(Self::entry_lines(entry, width));
        }
        let visible = area.height as usize;
        let rendered_rows = Paragraph::new(lines.clone())
            .wrap(Wrap { trim: false })
            .line_count(width.max(1));
        let maximum_scroll = rendered_rows.saturating_sub(visible);
        let from_top = maximum_scroll.saturating_sub(self.scroll_from_bottom.min(maximum_scroll));
        let paragraph = Paragraph::new(lines)
            .style(Style::default().bg(theme::CANVAS_BG))
            .wrap(Wrap { trim: false })
            .scroll((u16::try_from(from_top).unwrap_or(u16::MAX), 0));
        frame.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn drains_completed_overflow_for_terminal_scrollback() {
        let mut transcript = Transcript::new();
        for index in 0..12 {
            transcript.push(TranscriptKind::Agent, format!("answer {index}"));
        }
        let committed = transcript.drain_overflow(40, 8, true);
        assert!(!committed.is_empty());
        assert!(transcript.entries.len() < 12);
        let remaining_height = transcript
            .entries
            .iter()
            .map(|entry| Transcript::entry_height(entry, 40))
            .sum::<usize>();
        assert!(remaining_height <= 8);
    }

    #[test]
    fn drained_continuation_flows_top_aligned_without_gap() {
        let mut transcript = Transcript::new();
        for index in 0..12 {
            transcript.push(TranscriptKind::Agent, format!("answer {index}"));
        }
        let committed = transcript.drain_overflow(40, 8, true);
        assert!(!committed.is_empty());
        // Content arriving after the drain continues the conversation already
        // committed to the real terminal buffer. It must start at the top of
        // the viewport (directly below that content), not be bottom-anchored
        // with a blank gap splitting the answer.
        transcript.push(TranscriptKind::Agent, "continuation tail");
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| transcript.render(frame, frame.area()))
            .expect("draw transcript");
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        // After the drain, 4 entries remain (8 rows) and the continuation adds
        // 2 more (10 rows). Top-aligned, the continuation lands at row 9;
        // bottom-anchored it would sit on the last row (11).
        let lines = rendered.lines().collect::<Vec<_>>();
        let row = lines
            .iter()
            .position(|line| line.contains("continuation tail"))
            .unwrap_or_else(|| panic!("continuation missing:\n{rendered}"));
        assert_eq!(row, 9, "continuation must flow from the top:\n{rendered}");
        assert!(
            !lines.last().unwrap().contains("continuation tail"),
            "continuation must not be bottom-anchored:\n{rendered}"
        );
    }

    #[test]
    fn keeps_active_last_entry_in_live_viewport() {
        let mut transcript = Transcript::new();
        transcript.push(TranscriptKind::Info, "old notice");
        transcript.push(TranscriptKind::Agent, "streaming\n".repeat(20));
        let _ = transcript.drain_overflow(20, 4, false);
        assert!(matches!(
            transcript.entries.last(),
            Some(TranscriptEntry::Message(TranscriptKind::Agent, _))
        ));
    }

    #[test]
    fn wrapped_long_answer_keeps_its_tail_visible() {
        let mut transcript = Transcript::new();
        transcript.push(
            TranscriptKind::Agent,
            format!("{}\nTAIL", "a very long wrapped response ".repeat(12)),
        );
        let backend = TestBackend::new(20, 4);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| transcript.render(frame, frame.area()))
            .expect("draw transcript");
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("TAIL"), "rendered buffer:\n{rendered}");
    }

    #[test]
    fn short_transcript_flows_from_top() {
        let mut transcript = Transcript::new();
        transcript.push(TranscriptKind::Agent, "single short answer");
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| transcript.render(frame, frame.area()))
            .expect("draw transcript");
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        // The parent sizes the transcript to its actual height.
        assert!(
            rendered[1].contains("single short answer"),
            "rendered:\n{}",
            rendered.join("\n")
        );
        assert!(!rendered[rendered.len() - 2].contains("single short answer"));
    }

    #[test]
    fn user_message_background_covers_padding_and_wrapped_chinese_rows() {
        let width = 16;
        let entry = TranscriptEntry::Message(
            TranscriptKind::User,
            "还有吗？中文换行测试\nsecond line".to_owned(),
        );
        let lines = Transcript::entry_lines(&entry, width);
        let height = u16::try_from(lines.len()).unwrap();
        assert_eq!(
            Paragraph::new(lines.clone())
                .wrap(Wrap { trim: false })
                .line_count(width),
            usize::from(height)
        );
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(lines).wrap(Wrap { trim: false }),
                    frame.area(),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        for y in 0..height - 1 {
            // Wide glyph continuation cells are skipped by the terminal backend.
            let mut x = 0;
            while x < width {
                let cell = &buffer[(x, y)];
                assert_eq!(cell.bg, theme::USER_MESSAGE_BG, "cell {x},{y}");
                x += u16::try_from(cell.symbol().width().max(1)).unwrap();
            }
        }
        assert_eq!(buffer[(1, 1)].symbol(), "还");
        assert!(buffer[(0, 0)].symbol().trim().is_empty());
        assert!(buffer[(0, height - 2)].symbol().trim().is_empty());
        assert_ne!(buffer[(0, height - 1)].bg, theme::USER_MESSAGE_BG);
    }

    fn visible_text(transcript: &Transcript, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| transcript.render(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn can_scroll_to_committed_history_and_return_to_latest() {
        let mut transcript = Transcript::new();
        for index in 0..30 {
            transcript.push(TranscriptKind::Agent, format!("answer {index}"));
        }
        let full_height = transcript.full_height(40);
        assert!(!transcript.drain_overflow(40, 6, true).is_empty());
        assert_eq!(transcript.full_height(40), full_height);
        assert!(visible_text(&transcript, 40, 6).contains("answer 29"));
        transcript.scroll_up(usize::MAX, 40, 6);
        let first = visible_text(&transcript, 40, 6);
        assert!(first.contains("answer 0"));
        assert!(!first.contains("answer 29"));
        transcript.scroll_down(usize::MAX);
        assert!(visible_text(&transcript, 40, 6).contains("answer 29"));
        transcript.clear();
        assert_eq!(transcript.full_height(40), 0);
        assert_eq!(transcript.scroll_from_bottom, 0);
    }

    #[test]
    fn can_read_start_of_active_response_without_committing_markdown() {
        let mut transcript = Transcript::new();
        transcript.push_agent_delta(&format!(
            "FIRST\n\n{}\n\nLATEST",
            "paragraph\n\n".repeat(30)
        ));
        assert!(transcript.drain_overflow(40, 6, false).is_empty());
        transcript.scroll_up(usize::MAX, 40, 6);
        let before = visible_text(&transcript, 40, 6);
        assert!(before.contains("FIRST"));
        let height = transcript.full_height(40);
        transcript.push_agent_delta("\n\nNEW CONTENT");
        // The event loop preserves the reading position as new rows arrive.
        transcript.scroll_from_bottom += transcript.full_height(40) - height;
        assert_eq!(visible_text(&transcript, 40, 6), before);
        transcript.scroll_down(usize::MAX);
        assert!(visible_text(&transcript, 40, 6).contains("NEW CONTENT"));
    }

    #[test]
    fn agent_markdown_has_no_literal_stars() {
        let mut transcript = Transcript::new();
        transcript.push(TranscriptKind::Agent, "**bold** and - item");
        let backend = TestBackend::new(60, 6);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| transcript.render(frame, frame.area()))
            .expect("draw transcript");
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains('*'), "rendered buffer:\n{rendered}");
        assert!(rendered.contains("bold"), "rendered buffer:\n{rendered}");
    }
}
