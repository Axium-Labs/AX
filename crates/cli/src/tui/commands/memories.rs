//! Memory management views backed by the same scoped store used by the runtime.
use super::{App, BottomPane, ReplState, Result, SurfaceView, TranscriptKind, item};
use crate::tui::bottom_pane::memory_editor::MemoryEditor;
use memory::{MemoryRecord, MemoryScope};

fn scope(name: &str) -> Result<MemoryScope> {
    Ok(match name {
        "global" => MemoryScope::Global,
        "project" => MemoryScope::Project,
        "session" => MemoryScope::Session,
        _ => anyhow::bail!("Unknown memory scope"),
    })
}

pub(super) fn open_list(state: &mut ReplState, name: &str, pane: &mut BottomPane) -> Result<()> {
    let records = state.memory_records(scope(name)?)?;
    pane.push_view(SurfaceView::manager(
        "Memory facts",
        format!("memory-items:{name}"),
        vec![
            format!("Scope: {name}"),
            "Type to search; Enter opens value, source, and actions".into(),
        ],
        records
            .iter()
            .map(|record| item(&record.key, &record.key, &record.value))
            .collect(),
        "Enter details | Esc back",
    ));
    Ok(())
}

pub(super) fn open_record(
    state: &mut ReplState,
    name: &str,
    key: &str,
    pane: &mut BottomPane,
) -> Result<()> {
    let record = state
        .memory_records(scope(name)?)?
        .into_iter()
        .find(|record| record.key == key)
        .ok_or_else(|| anyhow::anyhow!("Memory no longer exists"))?;
    let age = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        .saturating_sub(u64::try_from(record.updated_at).unwrap_or(0));
    let mut lines = vec![
        format!("Scope: {name}"),
        format!("Key: {key}"),
        format!("Source: {}", record.source),
        format!("Updated {} minutes ago", age / 60),
        format!("Include in every task: {}", record.always_include),
    ];
    lines.extend(
        record
            .value
            .lines()
            .take(3)
            .map(|line| line.chars().take(80).collect::<String>()),
    );
    lines.push("Edit opens the full value.".into());
    let reference = serde_json::to_string(&(name, key, &record.value))?;
    pane.push_view(SurfaceView::manager(
        "Memory fact",
        format!("memory-record:{reference}"),
        lines,
        vec![
            item("edit", "Edit", "update this fact"),
            item("delete", "Delete", "remove this fact"),
        ],
        "Enter select | Esc back",
    ));
    Ok(())
}

fn change(
    state: &mut ReplState,
    name: &str,
    key: &str,
    expected: &str,
    value: &str,
    delete: bool,
) -> Result<()> {
    let scope = scope(name)?;
    let mut record: MemoryRecord = state
        .memory_records(scope)?
        .into_iter()
        .find(|record| record.key == key)
        .ok_or_else(|| anyhow::anyhow!("Memory no longer exists"))?;
    record.value = value.into();
    record.source = "user/memory-editor".into();
    state.change_memory(&record, Some(expected), delete)?;
    Ok(())
}

pub(super) fn action(
    state: &mut ReplState,
    reference: &str,
    action: &str,
    pane: &mut BottomPane,
    app: &mut App,
) -> Result<()> {
    let (name, key, expected): (String, String, String) = serde_json::from_str(reference)?;
    if action == "edit" {
        pane.push_view(MemoryEditor::open(name, key, expected.clone(), expected));
    } else if action == "delete" {
        match change(state, &name, &key, &expected, "", true) {
            Ok(()) => {
                pane.pop_view();
                refresh(state, &name, pane)?;
                app.push(TranscriptKind::Status, "Memory deleted");
            }
            Err(error) => app.push(TranscriptKind::Error, error.to_string()),
        }
    }
    Ok(())
}

fn refresh(state: &mut ReplState, name: &str, pane: &mut BottomPane) -> Result<()> {
    let records = state.memory_records(scope(name)?)?;
    let items = records
        .iter()
        .map(|record| item(&record.key, &record.key, &record.value))
        .collect::<Vec<_>>();
    pane.refresh_surface(&format!("memory-items:{name}"), &items);
    Ok(())
}

pub(super) fn edit(
    state: &mut ReplState,
    name: &str,
    key: &str,
    expected: &str,
    value: &str,
    pane: &mut BottomPane,
    app: &mut App,
) -> Result<()> {
    match change(state, name, key, expected, value, false) {
        Ok(()) => {
            pane.pop_view();
            refresh(state, name, pane)?;
            open_record(state, name, key, pane)?;
            app.push(TranscriptKind::Status, "Memory updated");
        }
        Err(error) => {
            app.push(TranscriptKind::Error, error.to_string());
            pane.push_view(MemoryEditor::open(
                name.into(),
                key.into(),
                expected.into(),
                value.into(),
            ));
        }
    }
    Ok(())
}
