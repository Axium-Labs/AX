//! Session picker (browse, open, create, delete).

use crate::session_projects::ProjectLocation;
use memory::Session;

use super::picker::{Picker, PickerItem};

pub struct SessionPicker;

impl SessionPicker {
    pub fn open(sessions: Vec<(Session, ProjectLocation)>) -> Box<dyn super::PaneView> {
        let items = sessions
            .into_iter()
            .map(|(session, project)| PickerItem {
                label: session.title.clone(),
                detail: format!(
                    "{} · {} messages",
                    project.root.display(),
                    session.message_count
                ),
                id: format!("{}|{}", project.id, session.id),
                project: project.root.display().to_string(),
            })
            .collect();
        Box::new(Picker::for_sessions(items))
    }
}
