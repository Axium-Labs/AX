//! Bottom pane: the owning container for the fixed composer, the slash
//! command popup and the modal view stack.
//!
//! Architecture adapted from Codex's `BottomPane`:
//! - the `Composer` is always retained, even while a modal view is shown;
//! - `SlashPopup` floats immediately above the composer;
//! - modal views (`Picker`, `SessionPicker`, `ModelPicker`,
//!   `ApprovalDialog`) are stacked and replace the composer region while open.

pub mod approval_dialog;
pub mod composer;
pub mod memory_editor;
pub mod model_picker;
pub mod picker;
pub mod secret_input;
pub mod session_picker;
pub mod slash_popup;
pub mod status_line;
pub mod surface;
pub mod textarea;
pub mod view;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};

pub use approval_dialog::ApprovalDialog;
pub use composer::{Composer, ComposerKey};
pub use slash_popup::{SlashKeyOutcome, SlashPopup};
pub use status_line::StatusLine;
pub use view::{ModalAction, PaneView, ViewOutcome};

pub struct BottomPane {
    composer: Composer,
    slash: SlashPopup,
    views: Vec<Box<dyn PaneView>>,
    status: StatusLine,
}

impl BottomPane {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            composer: Composer::new(),
            slash: SlashPopup::new(),
            views: Vec::new(),
            status: StatusLine::new(model),
        }
    }

    pub fn composer(&self) -> &Composer {
        &self.composer
    }

    pub fn composer_mut(&mut self) -> &mut Composer {
        &mut self.composer
    }

    pub fn status_mut(&mut self) -> &mut StatusLine {
        &mut self.status
    }

    pub fn slash(&self) -> &SlashPopup {
        &self.slash
    }

    pub fn slash_mut(&mut self) -> &mut SlashPopup {
        &mut self.slash
    }

    // ---- view stack -------------------------------------------------------

    pub fn push_view(&mut self, view: Box<dyn PaneView>) {
        self.views.push(view);
    }

    pub fn has_view(&self) -> bool {
        !self.views.is_empty()
    }

    pub fn pop_view(&mut self) {
        self.views.pop();
    }

    pub fn clear_views(&mut self) {
        self.views.clear();
    }

    pub fn refresh_surface(&mut self, surface: &str, items: &[surface::SurfaceItem]) {
        for view in &mut self.views {
            view.refresh_surface(surface, items);
        }
    }

    // ---- layout -----------------------------------------------------------

    /// Height of the slash popup row (0 when closed).
    pub fn popup_height(&self) -> u16 {
        // +2 for the dim header and navigation footer.
        if self.slash.is_open() {
            u16::try_from(self.slash.height()).unwrap_or(u16::MAX) + 2
        } else {
            0
        }
    }

    /// Height of the composer region.
    pub fn composer_height(&self, width: u16) -> u16 {
        self.composer.height(width)
    }

    /// Height of the active modal view. Codex-style selectors may use nearly
    /// the full terminal while retaining a transcript row and status line.
    pub fn view_height(&self, width: u16, screen_height: u16) -> Option<u16> {
        let view = self.views.last()?;
        let cap = screen_height.saturating_sub(3).max(6);
        Some(view.preferred_height(width).min(cap))
    }

    // ---- rendering --------------------------------------------------------

    pub fn render_slash_popup(&self, frame: &mut Frame<'_>, area: Rect) {
        self.slash.render(area, frame.buffer_mut());
    }

    pub fn render_composer(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        enabled: bool,
    ) -> Option<Position> {
        self.composer.render(area, frame.buffer_mut(), enabled)
    }

    pub fn render_active_view(&self, frame: &mut Frame<'_>, area: Rect) {
        if let Some(view) = self.views.last() {
            view.render(area, frame.buffer_mut());
        }
    }

    pub fn render_status(&self, frame: &mut Frame<'_>, area: Rect) {
        self.status.render(area, frame.buffer_mut());
    }

    /// Route a key to the active modal view. Returns the view outcome plus any
    /// action produced on accept.
    pub fn handle_view_key(&mut self, key: KeyEvent) -> Option<(ViewOutcome, Option<ModalAction>)> {
        let view = self.views.last_mut()?;
        let outcome = view.handle_key(key);
        if matches!(outcome, ViewOutcome::Continue) {
            return Some((ViewOutcome::Continue, None));
        }
        let action = view.take_action();
        let keep_parent = action.as_ref().is_some_and(|action| match action {
            ModalAction::ModelSelected(model) => !model.reasoning_efforts.is_empty(),
            ModalAction::SurfaceSelected { surface, .. } => {
                !surface.starts_with("permission-choice:")
            }
            _ => false,
        });
        if !keep_parent {
            self.views.pop();
        }
        Some((outcome, action))
    }
}
