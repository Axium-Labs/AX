//! `CommonMark` events become styled terminal lines, as in Codex's Markdown renderer.
//! Parse the whole active message so inline styles and fences survive stream chunks.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::theme;

pub fn agent_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let mut renderer = Renderer::new(width);
    for event in Parser::new_ext(
        text,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES,
    ) {
        renderer.event(event);
    }
    renderer.flush();
    while renderer.lines.last().is_some_and(|line| line.width() == 0) {
        renderer.lines.pop();
    }
    renderer.lines
}

struct Renderer {
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    quote_depth: usize,
    code: bool,
    width: u16,
    table_cell: usize,
}

impl Renderer {
    fn new(width: u16) -> Self {
        Self {
            lines: vec![Line::raw("")],
            spans: Vec::new(),
            styles: vec![theme::body()],
            lists: Vec::new(),
            quote_depth: 0,
            code: false,
            width,
            table_cell: 0,
        }
    }

    fn style(&self) -> Style {
        *self.styles.last().unwrap()
    }

    fn push_style(&mut self, style: Style) {
        self.styles.push(self.style().patch(style));
    }

    fn append(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if self.spans.is_empty() {
            let indent = format!(
                " {}{}",
                "│ ".repeat(self.quote_depth),
                "  ".repeat(self.lists.len())
            );
            self.spans.push(Span::styled(indent, theme::muted()));
        }
        self.spans.push(Span::styled(text.to_owned(), style));
    }

    fn flush(&mut self) {
        if !self.spans.is_empty() {
            self.lines.push(Line::from(std::mem::take(&mut self.spans)));
        }
    }

    fn blank(&mut self) {
        self.flush();
        if self.lines.last().is_some_and(|line| line.width() > 0) {
            self.lines.push(Line::raw(""));
        }
    }

    fn start(&mut self, tag: &Tag<'_>) {
        match tag {
            Tag::Heading { .. } => {
                self.blank();
                self.push_style(theme::title());
            }
            Tag::Strong => self.push_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Emphasis => self.push_style(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strikethrough => {
                self.push_style(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { .. } => {
                self.push_style(theme::accent().add_modifier(Modifier::UNDERLINED));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.quote_depth += 1;
            }
            Tag::CodeBlock(_) => {
                self.blank();
                self.code = true;
            }
            Tag::List(start) => {
                self.flush();
                self.lists.push(*start);
            }
            Tag::Item => {
                self.flush();
                let indent = format!(
                    " {}{}",
                    "│ ".repeat(self.quote_depth),
                    "  ".repeat(self.lists.len().saturating_sub(1))
                );
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => "• ".to_owned(),
                };
                self.spans
                    .push(Span::styled(format!("{indent}{marker}"), theme::muted()));
            }
            Tag::Table(_) => self.blank(),
            Tag::TableHead => {
                self.flush();
                self.table_cell = 0;
                self.push_style(theme::title());
            }
            Tag::TableRow => {
                self.flush();
                self.table_cell = 0;
            }
            Tag::TableCell => {
                if self.table_cell > 0 {
                    self.append(" │ ", theme::muted());
                }
                self.table_cell += 1;
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Heading(_) => {
                self.styles.pop();
                self.blank();
            }
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link => {
                self.styles.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.flush();
                self.code = false;
                self.blank();
            }
            TagEnd::Item | TagEnd::TableRow => self.flush(),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::TableHead => {
                self.flush();
                self.styles.pop();
            }
            TagEnd::Table => self.blank(),
            _ => {}
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(&tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) if self.code => {
                for part in text.split_inclusive('\n') {
                    self.append(
                        part.trim_end_matches('\n').trim_end_matches('\r'),
                        theme::body().bg(theme::CODE_BG),
                    );
                    if part.ends_with('\n') {
                        if self.spans.is_empty() {
                            self.lines.push(Line::raw(""));
                        } else {
                            self.flush();
                        }
                    }
                }
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                self.append(&text, self.style());
            }
            Event::Code(text) => self.append(&text, self.style().patch(theme::code())),
            Event::SoftBreak => self.append(" ", self.style()),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.blank();
                self.append(
                    &"─".repeat(usize::from(self.width.saturating_sub(2).max(1))),
                    theme::dim(),
                );
                self.blank();
            }
            Event::TaskListMarker(checked) => {
                self.append(if checked { "☑ " } else { "☐ " }, theme::muted());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn multiline_nested_styles_and_headings() {
        let lines = agent_lines(
            "## 边界\n\n所以 **直接丢个任务\n给我 *现在***。\n\n[文档](https://example.com)",
            60,
        );
        let text = plain(&lines);
        assert!(!text.contains("##"));
        // Use a valid CommonMark closing delimiter across a soft line break.
        let lines = agent_lines("所以 **直接丢个任务\n给我 *现在***。", 60);
        assert!(!plain(&lines).contains('*'));
        assert!(lines.iter().flat_map(|l| &l.spans).any(|s| {
            s.content == "现在"
                && s.style
                    .add_modifier
                    .contains(Modifier::BOLD | Modifier::ITALIC)
        }));
        assert!(text.contains("文档"));
    }

    #[test]
    fn fences_lists_escapes_and_code_are_parsed() {
        let lines = agent_lines(
            "- **one**\n  - two\n\n```rust\nlet x = **raw**;\n\nnext();\n```\n\n\\*literal\\* and `code`\n\n---",
            60,
        );
        let text = plain(&lines);
        assert!(text.contains("• one"));
        assert!(text.contains("   • two"));
        assert!(text.contains("let x = **raw**;\n\n next();"));
        assert!(!text.contains("```"));
        assert!(text.contains("*literal* and code"));
        assert!(text.contains('─'));
    }
}
