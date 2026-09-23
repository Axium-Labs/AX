//! Trait shared by every view that can replace the composer in the bottom
//! pane, modeled after Codex's `BottomPaneView` + `ViewStack` architecture.

use crossterm::event::KeyEvent;
use model::{ModelInfo, ReasoningEffort};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// Why a view closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewOutcome {
    Continue,
    Accepted,
    Cancelled,
}

/// Result payload a view produces when accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalAction {
    MemoryEdited {
        scope: String,
        key: String,
        expected: String,
        value: String,
    },
    ModelSelected(ModelInfo),
    ReasoningSelected {
        model: ModelInfo,
        effort: ReasoningEffort,
    },
    ApiKeyConfigured {
        provider: String,
        key: String,
    },
    SessionOpen(String),
    SessionNew,
    SessionDelete(String),
    /// Tool approval decision.
    Approval(bool),
    SurfaceSelected {
        surface: String,
        id: String,
    },
}

/// A modal bottom-pane view (picker, dialog, ...).
///
/// Views render into the composer region and grow upward; the composer itself
/// is retained while they are open, exactly like Codex's view stack.
pub trait PaneView {
    /// Route a key press to the view.
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome;

    /// Render the view into the provided region.
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Preferred height in rows for the given terminal width.
    fn preferred_height(&self, width: u16) -> u16;

    /// Human readable title (used in tests and accessibility hints).
    #[allow(dead_code)]
    fn title(&self) -> &'static str;

    /// Refresh a manager snapshot in place, retaining its navigation state.
    fn refresh_surface(&mut self, _surface: &str, _items: &[super::surface::SurfaceItem]) {}

    /// Consume the action produced by an accepted view.
    fn take_action(&mut self) -> Option<ModalAction> {
        None
    }
}
