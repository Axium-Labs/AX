//! Model catalog and reasoning-effort pickers.
//!
//! `ModelPicker` is a Rust port of pi's `ModelSelectorComponent`: only models
//! from configured providers are shown, the current model sorts first with a
//! ✓ marker, and typing filters the list by id / name / provider.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use model::ModelInfo;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use super::{ModalAction, PaneView, ViewOutcome};
use crate::tui::theme;
use std::sync::{Arc, RwLock};

/// Handles returned by [`ModelPicker::open_refreshable`]: the view, the
/// replaceable model snapshot, and a notice channel for refresh status.
pub type ModelPickerHandles = (
    Box<dyn PaneView>,
    Arc<RwLock<Vec<ModelInfo>>>,
    Arc<RwLock<Option<String>>>,
);

/// Maximum number of list rows rendered at once, mirroring pi.
const MAX_VISIBLE: usize = 12;

pub struct ModelPicker {
    models: Arc<RwLock<Vec<ModelInfo>>>,
    notice: Arc<RwLock<Option<String>>>,
    current_provider: String,
    current_model: String,
    filter: String,
    selected: usize,
    action: Option<ModalAction>,
}

impl ModelPicker {
    pub fn open_refreshable(
        models: Vec<ModelInfo>,
        current_provider: impl Into<String>,
        current_model: impl Into<String>,
        initial_filter: impl Into<String>,
    ) -> ModelPickerHandles {
        let models = Arc::new(RwLock::new(models));
        let notice = Arc::new(RwLock::new(None));
        let mut view = Self {
            models: Arc::clone(&models),
            notice: Arc::clone(&notice),
            current_provider: current_provider.into(),
            current_model: current_model.into(),
            filter: initial_filter.into(),
            selected: 0,
            action: None,
        };
        // Highlight the current model on open (pi: selectedIndex = currentIndex).
        view.selected = view
            .visible()
            .iter()
            .position(|m| view.is_current(m))
            .unwrap_or(0);
        (Box::new(view), models, notice)
    }

    fn is_current(&self, model: &ModelInfo) -> bool {
        model.provider == self.current_provider && model.id == self.current_model
    }

    /// Filtered models, sorted current-first then by provider (pi `sortModels`).
    fn visible(&self) -> Vec<ModelInfo> {
        let needle = self.filter.to_ascii_lowercase();
        let mut models = self
            .models
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        models.retain(|m| {
            needle.is_empty()
                || m.id.to_ascii_lowercase().contains(&needle)
                || m.display_name.to_ascii_lowercase().contains(&needle)
                || m.provider.to_ascii_lowercase().contains(&needle)
        });
        models.sort_by(|a, b| {
            let a_current = self.is_current(a);
            let b_current = self.is_current(b);
            match (a_current, b_current) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.provider.cmp(&b.provider).then(a.id.cmp(&b.id)),
            }
        });
        models
    }

    fn move_by(&mut self, delta: isize) {
        let count = self.visible().len();
        if count == 0 {
            self.selected = 0;
        } else if delta < 0 {
            self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
        } else {
            self.selected = (self.selected + 1) % count;
        }
    }
}

impl PaneView for ModelPicker {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        if key.kind != KeyEventKind::Press {
            return ViewOutcome::Continue;
        }
        match key.code {
            KeyCode::Esc => ViewOutcome::Cancelled,
            KeyCode::Up => {
                self.move_by(-1);
                ViewOutcome::Continue
            }
            KeyCode::Down => {
                self.move_by(1);
                ViewOutcome::Continue
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
                ViewOutcome::Continue
            }
            KeyCode::Enter => {
                if let Some(model) = self.visible().get(self.selected).cloned() {
                    self.action = Some(ModalAction::ModelSelected(model));
                    ViewOutcome::Accepted
                } else {
                    ViewOutcome::Continue
                }
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.selected = 0;
                ViewOutcome::Continue
            }
            _ => ViewOutcome::Continue,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines = vec![
            Line::from(Span::styled("Select model", theme::title())),
            Line::from(Span::styled(
                "Only showing models from configured providers.",
                theme::dim(),
            )),
            Line::from(vec![
                Span::styled("Type to search: ", theme::dim()),
                Span::raw(&self.filter),
            ]),
        ];
        let visible = self.visible();
        let total = visible.len();
        let start = self.selected.saturating_sub(MAX_VISIBLE / 2);
        let start = start.min(total.saturating_sub(MAX_VISIBLE));
        let end = (start + MAX_VISIBLE).min(total);
        for (index, model) in visible.iter().enumerate().take(end).skip(start) {
            let selected = index == self.selected;
            let current = self.is_current(model);
            lines.push(Line::from(vec![
                Span::styled(
                    if selected { "→ " } else { "  " },
                    if selected {
                        theme::accent()
                    } else {
                        theme::dim()
                    },
                ),
                Span::styled(if current { "✓ " } else { "  " }, theme::accent()),
                Span::styled(
                    &model.id,
                    if selected {
                        theme::accent()
                    } else {
                        theme::body()
                    },
                ),
                Span::styled(format!(" [{}]", model.provider), theme::dim()),
            ]));
        }
        if total == 0 {
            lines.push(Line::from(Span::styled(
                "  No matching models",
                theme::dim(),
            )));
        } else {
            let selected = &visible[self.selected];
            lines.push(Line::from(Span::styled(
                format!("  Model Name: {}", selected.display_name),
                theme::dim(),
            )));
            if start > 0 || end < total {
                lines.push(Line::from(Span::styled(
                    format!("  ({}/{total})", self.selected + 1),
                    theme::dim(),
                )));
            }
        }
        if let Some(notice) = self
            .notice
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            lines.push(Line::from(Span::styled(
                format!("  {notice}"),
                theme::dim(),
            )));
        }
        lines.push(Line::from(vec![
            Span::styled("Current  ", theme::dim()),
            Span::raw(&self.current_model),
        ]));
        lines.push(Line::from(Span::styled(
            "↑↓ navigate · Enter select · Esc back",
            theme::dim(),
        )));
        Paragraph::new(lines)
            .style(Style::default().bg(theme::CANVAS_BG))
            .render(area, buf);
    }

    fn preferred_height(&self, _width: u16) -> u16 {
        u16::try_from(self.visible().len().min(MAX_VISIBLE).saturating_add(7))
            .unwrap_or(12)
            .min(24)
    }

    fn title(&self) -> &'static str {
        "Select model"
    }

    fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }
}

pub struct ReasoningPicker {
    model: ModelInfo,
    selected: usize,
    action: Option<ModalAction>,
}
impl ReasoningPicker {
    pub fn open(model: ModelInfo) -> Box<dyn PaneView> {
        let selected = model
            .default_reasoning_effort
            .and_then(|current| model.reasoning_efforts.iter().position(|e| *e == current))
            .unwrap_or(0);
        Box::new(Self {
            model,
            selected,
            action: None,
        })
    }
}
impl PaneView for ReasoningPicker {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        if key.kind != KeyEventKind::Press {
            return ViewOutcome::Continue;
        }
        let count = self.model.reasoning_efforts.len();
        match key.code {
            KeyCode::Esc => ViewOutcome::Cancelled,
            KeyCode::Up if count > 0 => {
                self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
                ViewOutcome::Continue
            }
            KeyCode::Down if count > 0 => {
                self.selected = (self.selected + 1) % count;
                ViewOutcome::Continue
            }
            KeyCode::Enter if count > 0 => {
                self.action = Some(ModalAction::ReasoningSelected {
                    model: self.model.clone(),
                    effort: self.model.reasoning_efforts[self.selected],
                });
                ViewOutcome::Accepted
            }
            _ => ViewOutcome::Continue,
        }
    }
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines = vec![Line::from(Span::styled("Reasoning effort", theme::title()))];
        for (index, effort) in self.model.reasoning_efforts.iter().enumerate() {
            let selected = index == self.selected;
            lines.push(Line::from(Span::styled(
                format!(" {} {effort}", if selected { "❯" } else { " " }),
                if selected {
                    theme::selected_row()
                } else {
                    theme::body()
                },
            )));
        }
        lines.push(Line::from(Span::styled(
            "Enter confirm · Esc back",
            theme::dim(),
        )));
        Paragraph::new(lines)
            .style(Style::default().bg(theme::CANVAS_BG))
            .render(area, buf);
    }
    fn preferred_height(&self, _width: u16) -> u16 {
        u16::try_from(self.model.reasoning_efforts.len())
            .unwrap_or(u16::MAX)
            .saturating_add(3)
    }
    fn title(&self) -> &'static str {
        "Reasoning effort"
    }
    fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }
}
