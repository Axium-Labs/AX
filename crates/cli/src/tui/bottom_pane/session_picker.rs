//! Session picker (browse, open, create, delete).

use memory::Session;

use super::picker::{Picker, PickerItem};

pub struct SessionPicker;

impl SessionPicker {
    pub fn open(sessions: Vec<Session>) -> Box<dyn super::PaneView> {
        let items = sessions
            .into_iter()
            .map(|session| PickerItem {
                label: session.title.clone(),
                detail: format!("{} · {} messages", session.id, session.message_count),
                id: session.id,
            })
            .collect();
        Box::new(Picker::for_sessions(items))
    }
}
