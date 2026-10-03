//! Calendar commands for the UI. Storage and the data rules live in
//! [`crate::calendar_store`], shared with the agent's calendar tools; every
//! change is reported through [`HostEffects::calendar_changed`], which
//! re-indexes the record and emits [`CALENDAR_CHANGED_EVENT`].

use tauri::{AppHandle, Manager};

use crate::agent_tools::{CalendarChange, HostEffects, TauriEffects};
use crate::calendar_store::{CalendarStore, EventPatch, TaskPatch};
use crate::rag_commands::RagState;

pub use crate::calendar_store::{CalendarEvent, NewEvent, NewTask, TodoItem};

/// Emitted after calendar data is written; listeners re-read tasks and events.
pub const CALENDAR_CHANGED_EVENT: &str = "calendar-changed";

fn store(app: &AppHandle) -> Result<CalendarStore, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;
    Ok(CalendarStore::in_dir(&dir))
}

fn report(app: &AppHandle, change: CalendarChange) {
    TauriEffects::new(app.clone()).calendar_changed(change);
}

// ── RAG Indexing Helpers ─────────────────────────────────────────
//
// Convert local structs to shodh_rag equivalents and call the indexer.
// Best-effort: if RAG engine isn't ready, log and continue.

fn to_rag_subtask(s: &crate::calendar_store::SubTask) -> shodh_rag::agent::calendar::SubTask {
    shodh_rag::agent::calendar::SubTask {
        id: s.id.clone(),
        title: s.title.clone(),
        completed: s.completed,
    }
}

fn to_rag_task(task: &TodoItem) -> shodh_rag::agent::calendar::TodoItem {
    shodh_rag::agent::calendar::TodoItem {
        id: task.id.clone(),
        title: task.title.clone(),
        description: task.description.clone(),
        due_date: task.due_date.clone(),
        priority: task.priority.clone(),
        status: task.status.clone(),
        tags: task.tags.clone(),
        subtasks: task.subtasks.iter().map(to_rag_subtask).collect(),
        project: task.project.clone(),
        source: task.source.clone(),
        source_ref: task.source_ref.clone(),
        created_at: task.created_at.clone(),
        updated_at: task.updated_at.clone(),
        completed_at: task.completed_at.clone(),
        reminder: task.reminder.clone(),
    }
}

fn to_rag_event(event: &CalendarEvent) -> shodh_rag::agent::calendar::CalendarEvent {
    shodh_rag::agent::calendar::CalendarEvent {
        id: event.id.clone(),
        title: event.title.clone(),
        description: event.description.clone(),
        start_time: event.start_time.clone(),
        end_time: event.end_time.clone(),
        all_day: event.all_day,
        color: event.color.clone(),
        location: event.location.clone(),
        source: event.source.clone(),
        source_ref: event.source_ref.clone(),
        created_at: event.created_at.clone(),
    }
}

/// Re-index (or de-index) the changed record in the background.
pub(crate) fn spawn_reindex(app: &AppHandle, change: &CalendarChange) {
    let rag = app.state::<RagState>().rag.clone();
    match change {
        CalendarChange::TaskSaved(task) => {
            let rag_task = to_rag_task(task);
            tokio::spawn(async move {
                let mut engine = rag.write().await;
                if let Err(e) = shodh_rag::agent::calendar_indexer::index_task(
                    &mut engine,
                    &rag_task,
                    "calendar",
                )
                .await
                {
                    tracing::warn!(task_id = %rag_task.id, error = %e, "Failed to index task in RAG");
                }
            });
        }
        CalendarChange::TaskRemoved(id) => {
            let id = id.clone();
            tokio::spawn(async move {
                let mut engine = rag.write().await;
                if let Err(e) =
                    shodh_rag::agent::calendar_indexer::deindex_task(&mut engine, &id).await
                {
                    tracing::warn!(task_id = %id, error = %e, "Failed to deindex task from RAG");
                }
            });
        }
        CalendarChange::EventSaved(event) => {
            let rag_event = to_rag_event(event);
            tokio::spawn(async move {
                let mut engine = rag.write().await;
                if let Err(e) = shodh_rag::agent::calendar_indexer::index_event(
                    &mut engine,
                    &rag_event,
                    "calendar",
                )
                .await
                {
                    tracing::warn!(event_id = %rag_event.id, error = %e, "Failed to index event in RAG");
                }
            });
        }
        CalendarChange::EventRemoved(id) => {
            let id = id.clone();
            tokio::spawn(async move {
                let mut engine = rag.write().await;
                if let Err(e) =
                    shodh_rag::agent::calendar_indexer::deindex_event(&mut engine, &id).await
                {
                    tracing::warn!(event_id = %id, error = %e, "Failed to deindex event from RAG");
                }
            });
        }
    }
}

// ── Task Commands ────────────────────────────────────────────────

#[tauri::command]
pub async fn load_tasks(app: AppHandle) -> Result<Vec<TodoItem>, String> {
    Ok(store(&app)?.load().map_err(|e| e.to_string())?.tasks)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri passes each field as its own argument.
pub async fn create_task(
    app: AppHandle,
    title: String,
    description: Option<String>,
    due_date: Option<String>,
    priority: Option<String>,
    tags: Option<Vec<String>>,
    project: Option<String>,
    source: Option<String>,
    source_ref: Option<String>,
    reminder: Option<String>,
) -> Result<TodoItem, String> {
    let task = store(&app)?
        .update(|d| {
            d.insert_task(NewTask {
                title,
                description,
                due_date,
                priority,
                tags,
                project,
                source,
                source_ref,
                reminder,
            })
        })
        .map_err(|e| e.to_string())?;
    report(&app, CalendarChange::TaskSaved(task.clone()));
    tracing::info!(task_id = %task.id, title = %task.title, "Created task");
    Ok(task)
}

/// Change a task. A field left out keeps its value; `clear` names the
/// optional fields to empty (`due_date`, `project`, `description`,
/// `reminder`), since an absent value can never mean "remove".
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri passes each field as its own argument.
pub async fn update_task(
    app: AppHandle,
    id: String,
    title: Option<String>,
    description: Option<String>,
    due_date: Option<String>,
    priority: Option<String>,
    status: Option<String>,
    tags: Option<Vec<String>>,
    project: Option<String>,
    reminder: Option<String>,
    clear: Option<Vec<String>>,
) -> Result<TodoItem, String> {
    let patch = TaskPatch {
        title,
        description,
        due_date: due_date.map(Some),
        priority,
        status,
        tags,
        project: project.map(Some),
        reminder: reminder.map(Some),
        subtasks: None,
    }
    .clearing(&clear.unwrap_or_default())
    .map_err(|e| e.to_string())?;
    let (_, updated, _) = store(&app)?
        .update(|d| d.patch_task(&id, &patch))
        .map_err(|e| e.to_string())?;
    report(&app, CalendarChange::TaskSaved(updated.clone()));
    tracing::info!(task_id = %updated.id, "Updated task");
    Ok(updated)
}

#[tauri::command]
pub async fn delete_task(app: AppHandle, id: String) -> Result<bool, String> {
    store(&app)?
        .update(|d| d.delete_task(&id))
        .map_err(|e| e.to_string())?;
    report(&app, CalendarChange::TaskRemoved(id.clone()));
    tracing::info!(task_id = %id, "Deleted task");
    Ok(true)
}

// ── Subtask Commands ─────────────────────────────────────────────

#[tauri::command]
pub async fn add_subtask(
    app: AppHandle,
    task_id: String,
    title: String,
) -> Result<TodoItem, String> {
    let updated = store(&app)?
        .update(|d| d.add_subtask(&task_id, &title))
        .map_err(|e| e.to_string())?;
    report(&app, CalendarChange::TaskSaved(updated.clone()));
    Ok(updated)
}

#[tauri::command]
pub async fn toggle_subtask(
    app: AppHandle,
    task_id: String,
    subtask_id: String,
) -> Result<TodoItem, String> {
    let updated = store(&app)?
        .update(|d| d.toggle_subtask(&task_id, &subtask_id))
        .map_err(|e| e.to_string())?;
    report(&app, CalendarChange::TaskSaved(updated.clone()));
    Ok(updated)
}

#[tauri::command]
pub async fn rename_subtask(
    app: AppHandle,
    task_id: String,
    subtask_id: String,
    title: String,
) -> Result<TodoItem, String> {
    let updated = store(&app)?
        .update(|d| d.rename_subtask(&task_id, &subtask_id, &title))
        .map_err(|e| e.to_string())?;
    report(&app, CalendarChange::TaskSaved(updated.clone()));
    Ok(updated)
}

#[tauri::command]
pub async fn delete_subtask(
    app: AppHandle,
    task_id: String,
    subtask_id: String,
) -> Result<TodoItem, String> {
    let updated = store(&app)?
        .update(|d| d.delete_subtask(&task_id, &subtask_id))
        .map_err(|e| e.to_string())?;
    report(&app, CalendarChange::TaskSaved(updated.clone()));
    Ok(updated)
}

// ── Event Commands ───────────────────────────────────────────────

#[tauri::command]
pub async fn load_events(app: AppHandle) -> Result<Vec<CalendarEvent>, String> {
    Ok(store(&app)?.load().map_err(|e| e.to_string())?.events)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri passes each field as its own argument.
pub async fn create_event(
    app: AppHandle,
    title: String,
    start_time: String,
    end_time: Option<String>,
    all_day: Option<bool>,
    description: Option<String>,
    color: Option<String>,
    source: Option<String>,
    source_ref: Option<String>,
    location: Option<String>,
) -> Result<CalendarEvent, String> {
    let event = store(&app)?
        .update(|d| {
            d.insert_event(NewEvent {
                title,
                start_time,
                end_time,
                all_day,
                description,
                color,
                location,
                source,
                source_ref,
            })
        })
        .map_err(|e| e.to_string())?;
    report(&app, CalendarChange::EventSaved(event.clone()));
    tracing::info!(event_id = %event.id, title = %event.title, "Created event");
    Ok(event)
}

/// Change an event. A field left out keeps its value; `clear` names the
/// optional fields to empty (`description`, `end_time`, `location`).
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri passes each field as its own argument.
pub async fn update_event(
    app: AppHandle,
    id: String,
    title: Option<String>,
    description: Option<String>,
    start_time: Option<String>,
    end_time: Option<String>,
    all_day: Option<bool>,
    color: Option<String>,
    location: Option<String>,
    clear: Option<Vec<String>>,
) -> Result<CalendarEvent, String> {
    let patch = EventPatch {
        title,
        description,
        start_time,
        end_time: end_time.map(Some),
        all_day,
        color,
        location: location.map(Some),
    }
    .clearing(&clear.unwrap_or_default())
    .map_err(|e| e.to_string())?;
    let (_, updated, _) = store(&app)?
        .update(|d| d.patch_event(&id, &patch))
        .map_err(|e| e.to_string())?;
    report(&app, CalendarChange::EventSaved(updated.clone()));
    tracing::info!(event_id = %updated.id, "Updated event");
    Ok(updated)
}

#[tauri::command]
pub async fn delete_event(app: AppHandle, id: String) -> Result<bool, String> {
    store(&app)?
        .update(|d| d.delete_event(&id))
        .map_err(|e| e.to_string())?;
    report(&app, CalendarChange::EventRemoved(id.clone()));
    tracing::info!(event_id = %id, "Deleted event");
    Ok(true)
}

// ── Bulk Reindex ─────────────────────────────────────────────────

/// Re-index all existing calendar data into the RAG engine.
/// Called on startup to ensure the search index is populated.
pub async fn reindex_all_calendar_data(app: &AppHandle) {
    let data = match store(app).and_then(|s| s.load().map_err(|e| e.to_string())) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "Could not read calendar data for reindexing");
            return;
        }
    };

    if data.tasks.is_empty() && data.events.is_empty() {
        return;
    }

    let rag = app.state::<RagState>().rag.clone();
    let rag_tasks: Vec<_> = data.tasks.iter().map(to_rag_task).collect();
    let rag_events: Vec<_> = data.events.iter().map(to_rag_event).collect();

    tokio::spawn(async move {
        let mut engine = rag.write().await;
        match shodh_rag::agent::calendar_indexer::reindex_all(
            &mut engine,
            &rag_tasks,
            &rag_events,
            "calendar",
        )
        .await
        {
            Ok((t, e)) => tracing::info!(
                tasks = t,
                events = e,
                "Calendar data reindexed into RAG on startup"
            ),
            Err(e) => tracing::warn!(error = %e, "Failed to reindex calendar data into RAG"),
        }
    });
}
