//! Transcript: the scrolling middle region between the startup card and the
//! bottom pane. No println REPL — every piece of output is a styled entry.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

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
        detail: String,
        state: ToolState,
        finished_at: Option<Instant>,
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
    pub streaming: bool,
    /// Completed Markdown is parsed once per terminal width. Only the active
    /// tail changes as content deltas arrive.
    rendered: RefCell<HashMap<(usize, u16), Vec<Line<'static>>>>,
    stream_prefix: RefCell<Option<StreamPrefix>>,
}

const TOOL_SETTLE: Duration = Duration::from_millis(150);

struct StreamPrefix {
    index: usize,
    width: u16,
    end: usize,
    lines: Vec<Line<'static>>,
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
            streaming: false,
            rendered: RefCell::new(HashMap::new()),
            stream_prefix: RefCell::new(None),
        }
    }

    pub fn push(&mut self, kind: TranscriptKind, text: impl Into<String>) {
        self.entries
            .push(TranscriptEntry::Message(kind, text.into()));
    }

    pub fn mark_next_queued_running(&mut self) {
        if let Some(TranscriptEntry::Message(TranscriptKind::Status, text)) = self
            .entries
            .iter_mut()
            .find(|entry| matches!(entry, TranscriptEntry::Message(TranscriptKind::Status, text) if text.starts_with("Queued —")))
        {
            "Running queued task…".clone_into(text);
        }
    }

    pub fn card(&mut self, info: StartupInfo) {
        self.entries.push(TranscriptEntry::Card(info));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.history.clear();
        self.scroll_from_bottom = 0;
        self.streaming = false;
        self.rendered.borrow_mut().clear();
        self.stream_prefix.borrow_mut().take();
    }

    pub fn tool_started(&mut self, name: impl Into<String>, detail: impl Into<String>) {
        self.entries.push(TranscriptEntry::Tool {
            name: name.into(),
            detail: detail.into(),
            state: ToolState::Running,
            finished_at: None,
        });
    }

    pub fn tool_finished(&mut self, name: &str, success: bool) {
        if let Some(TranscriptEntry::Tool { state, finished_at, .. }) = self.entries.iter_mut().rev().find(
            |entry| matches!(entry, TranscriptEntry::Tool { name: found, state: ToolState::Running, .. } if found == name),
        ) {
            *state = if success {
                ToolState::Succeeded
            } else {
                ToolState::Failed
            };
            *finished_at = Some(Instant::now());
        } else {
            self.entries.push(TranscriptEntry::Tool {
                name: name.to_owned(),
                detail: String::new(),
                state: if success {
                    ToolState::Succeeded
                } else {
                    ToolState::Failed
                },
                finished_at: Some(Instant::now()),
            });
        }
    }

    /// Append a streaming content delta to the current agent message, starting
    /// a new entry when needed.
    pub fn push_agent_delta(&mut self, delta: &str) {
        self.streaming = true;
        let last = self.entries.len().saturating_sub(1);
        self.rendered
            .borrow_mut()
            .retain(|(index, _), _| *index != last);
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
            TranscriptEntry::Tool {
                name,
                detail,
                state,
                finished_at,
            } => {
                let (marker, label, bg, marker_color) = match state {
                    ToolState::Running => ("•", "running", theme::TOOL_PENDING_BG, theme::ACCENT),
                    ToolState::Succeeded => ("✓", "done", theme::TOOL_SUCCESS_BG, theme::SUCCESS),
                    ToolState::Failed => ("×", "failed", theme::TOOL_ERROR_BG, theme::ERROR),
                };
                let settling = finished_at.is_some_and(|at| at.elapsed() < TOOL_SETTLE);
                let content = if *state == ToolState::Running || settling {
                    detail.clone()
                } else {
                    format!("{name} · {label}")
                };
                let marker = format!(" {marker} ");
                let used = marker.width() + content.width();
                let padding = usize::from(width).saturating_sub(used);
                let body = theme::body().bg(bg);
                vec![Line::from(vec![
                    Span::styled(marker, Style::default().fg(marker_color).bg(bg)),
                    Span::styled(content, body),
                    Span::styled(" ".repeat(padding), body),
                ])]
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

    fn lines_for(&self, index: usize, width: u16) -> Vec<Line<'static>> {
        let entry = &self.entries[index];
        if let TranscriptEntry::Message(TranscriptKind::Agent, text) = entry
            && self.streaming
            && index + 1 == self.entries.len()
        {
            // A later reference definition can restyle an earlier link, so
            // keep the full CommonMark parser context for those messages.
            if text.contains("][") || text.contains("]:") {
                self.stream_prefix.borrow_mut().take();
                return Self::entry_lines(entry, width);
            }
            let previous_end = self
                .stream_prefix
                .borrow()
                .as_ref()
                .filter(|cached| cached.index == index && cached.end <= text.len())
                .map_or(0, |cached| cached.end);
            let end = previous_end + markdown::stable_prefix_end(&text[previous_end..]);
            if end == 0 {
                return Self::entry_lines(entry, width);
            }
            let cached = self.stream_prefix.borrow().as_ref().and_then(|cached| {
                ((cached.index, cached.width, cached.end) == (index, width, end))
                    .then(|| cached.lines.clone())
            });
            let prefix = if let Some(lines) = cached {
                lines
            } else {
                let lines = markdown::agent_lines(&text[..end], width);
                *self.stream_prefix.borrow_mut() = Some(StreamPrefix {
                    index,
                    width,
                    end,
                    lines: lines.clone(),
                });
                lines
            };
            let mut lines = prefix;
            lines.extend(markdown::agent_lines(&text[end..], width));
            return lines;
        }
        if !matches!(entry, TranscriptEntry::Message(TranscriptKind::Agent, _)) {
            return Self::entry_lines(entry, width);
        }
        if let Some(lines) = self.rendered.borrow().get(&(index, width)) {
            return lines.clone();
        }
        let lines = Self::entry_lines(entry, width);
        self.rendered
            .borrow_mut()
            .insert((index, width), lines.clone());
        lines
    }

    fn entry_height(&self, index: usize, width: u16) -> usize {
        Paragraph::new(self.lines_for(index, width))
            .wrap(Wrap { trim: false })
            .line_count(width.max(1))
    }

    pub fn height(&self, width: u16) -> usize {
        let rows: usize = (0..self.entries.len())
            .map(|index| self.entry_height(index, width))
            .sum();
        let cursor_row = self.streaming
            && matches!(
                self.entries.last(),
                Some(TranscriptEntry::Message(TranscriptKind::Agent, _))
            )
            && self
                .lines_for(self.entries.len() - 1, width)
                .last()
                .is_some_and(|line| line.width() >= usize::from(width));
        rows + usize::from(cursor_row)
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

    pub fn is_animating(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(entry, TranscriptEntry::Tool { finished_at: Some(at), .. } if at.elapsed() < TOOL_SETTLE)
        })
    }

    pub fn settle_transitions(&mut self) -> bool {
        let mut changed = false;
        for entry in &mut self.entries {
            if let TranscriptEntry::Tool { finished_at, .. } = entry
                && finished_at.is_some_and(|at| at.elapsed() >= TOOL_SETTLE)
            {
                *finished_at = None;
                changed = true;
            }
        }
        changed
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
            if matches!(self.entries.first(), Some(TranscriptEntry::Tool { finished_at: Some(at), .. }) if at.elapsed() < TOOL_SETTLE)
            {
                break;
            }
            if matches!(self.entries.first(), Some(TranscriptEntry::Message(TranscriptKind::Status, text)) if text.starts_with("Queued —"))
            {
                break;
            }
            if !allow_last && self.entries.len() == 1 {
                break;
            }
            let lines = self.lines_for(0, width);
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
            self.rendered.borrow_mut().clear();
            self.stream_prefix.borrow_mut().take();
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
        for index in 0..self.entries.len() {
            lines.extend(self.lines_for(index, width));
        }
        if self.streaming
            && matches!(
                self.entries.last(),
                Some(TranscriptEntry::Message(TranscriptKind::Agent, _))
            )
            && let Some(line) = lines.last_mut()
        {
            if line.width() < usize::from(width) {
                line.spans.push(Span::styled("▍", theme::accent()));
            } else {
                lines.push(Line::from(Span::styled("▍", theme::accent())));
            }
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
    fn tool_detail_collapses_after_completion() {
        let mut transcript = Transcript::new();
        transcript.tool_started("search", "searching 'secret' in src");
        let running = Transcript::entry_lines(&transcript.entries[0], 80);
        assert!(running[0].spans[1].content.contains("searching 'secret'"));
        assert_eq!(running[0].spans[0].style.fg, Some(theme::ACCENT));
        transcript.tool_finished("search", true);
        let settling = Transcript::entry_lines(&transcript.entries[0], 80);
        assert!(settling[0].spans[1].content.contains("searching 'secret'"));
        assert!(transcript.is_animating());
        if let TranscriptEntry::Tool { finished_at, .. } = &mut transcript.entries[0] {
            *finished_at = Instant::now().checked_sub(TOOL_SETTLE);
        }
        let finished = Transcript::entry_lines(&transcript.entries[0], 80);
        assert!(finished[0].spans[1].content.contains("search · done"));
        assert!(!finished[0].spans[1].content.contains("secret"));
        assert_eq!(finished[0].spans[0].style.fg, Some(theme::SUCCESS));
    }

    #[test]
    fn queued_notice_updates_before_it_enters_terminal_scrollback() {
        let mut transcript = Transcript::new();
        transcript.push(
            TranscriptKind::Status,
            "Queued — will run after the current task finishes",
        );
        transcript.push(TranscriptKind::Agent, "many rows\n\n".repeat(20));
        assert!(transcript.drain_overflow(40, 6, false).is_empty());
        transcript.mark_next_queued_running();
        assert!(matches!(
            transcript.entries.first(),
            Some(TranscriptEntry::Message(TranscriptKind::Status, text)) if text == "Running queued task…"
        ));
        assert!(!transcript.drain_overflow(40, 6, true).is_empty());
    }

    #[test]
    fn streaming_cursor_disappears_when_the_turn_finishes() {
        let backend = TestBackend::new(40, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut transcript = Transcript::new();
        transcript.push_agent_delta("Hello");
        terminal
            .draw(|frame| transcript.render(frame, frame.area()))
            .unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol() == "▍")
        );
        transcript.streaming = false;
        terminal
            .draw(|frame| transcript.render(frame, frame.area()))
            .unwrap();
        assert!(
            !terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol() == "▍")
        );
    }

    #[test]
    fn streaming_blocks_match_the_complete_markdown_render() {
        for text in [
            "First paragraph.\n\nSecond paragraph",
            "- first\n- second\n\nAfter list",
            "```rust\nlet x = 1;\n```\n\nAfter code",
            "[link][id]\n\n[id]: https://example.com\n\nAfter link",
            "# Heading\n\n> quoted\n> again\n\nNormal text",
            "| A | B |\n|---|---|\n| 1 | 2 |\n\nAfter table",
            "- [x] first\n- [ ] second\n\nAfter tasks",
        ] {
            let mut transcript = Transcript::new();
            let mut so_far = String::new();
            for part in text.split_inclusive('\n') {
                so_far.push_str(part);
                transcript.push_agent_delta(part);
                let streamed = transcript.lines_for(0, 80);
                let whole = markdown::agent_lines(&so_far, 80);
                assert_eq!(streamed, whole, "render changed for {so_far}");
            }
        }
        let mut transcript = Transcript::new();
        transcript.push_agent_delta("First paragraph.\n\nSecond");
        let _ = transcript.lines_for(0, 80);
        transcript.push_agent_delta(" paragraph.\n\nThird");
        assert_eq!(
            transcript.lines_for(0, 80),
            markdown::agent_lines("First paragraph.\n\nSecond paragraph.\n\nThird", 80)
        );
    }

    #[test]
    fn drains_completed_overflow_for_terminal_scrollback() {
        let mut transcript = Transcript::new();
        for index in 0..12 {
            transcript.push(TranscriptKind::Agent, format!("answer {index}"));
        }
        let committed = transcript.drain_overflow(40, 8, true);
        assert!(!committed.is_empty());
        assert!(transcript.entries.len() < 12);
        let remaining_height = transcript.height(40);
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
                );
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
