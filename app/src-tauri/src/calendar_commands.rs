use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use chrono::Utc;
use uuid::Uuid;

use crate::rag_commands::RagState;

// ── Data Structures ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubTask {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub subtasks: Vec<SubTask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub start_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    #[serde(default)]
    pub all_day: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    pub created_at: String,
}

fn default_priority() -> String { "medium".to_string() }
fn default_status() -> String { "pending".to_string() }
fn default_source() -> String { "user".to_string() }

// ── Storage ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CalendarDataFile {
    #[serde(default)]
    tasks: Vec<TodoItem>,
    #[serde(default)]
    events: Vec<CalendarEvent>,
}

fn calendar_path(app: &AppHandle) -> Result<PathBuf, String> {
    let app_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;
    fs::create_dir_all(&app_dir).map_err(|e| format!("Failed to create app dir: {}", e))?;
    Ok(app_dir.join("calendar_data.json"))
}

fn read_calendar(app: &AppHandle) -> Result<CalendarDataFile, String> {
    let path = calendar_path(app)?;
    if !path.exists() {
        return Ok(CalendarDataFile { tasks: Vec::new(), events: Vec::new() });
    }
    let data = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read calendar data: {}", e))?;
    serde_json::from_str(&data)
        .map_err(|e| format!("Failed to parse calendar data: {}", e))
}

fn write_calendar(app: &AppHandle, data: &CalendarDataFile) -> Result<(), String> {
    let path = calendar_path(app)?;
    let tmp_path = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| format!("Failed to serialize calendar data: {}", e))?;
    fs::write(&tmp_path, &json)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;
    fs::rename(&tmp_path, &path)
        .map_err(|e| format!("Failed to rename temp file: {}", e))?;
    Ok(())
}

// ── RAG Indexing Helpers ─────────────────────────────────────────
//
// Convert local structs to shodh_rag equivalents and call the indexer.
// Best-effort: if RAG engine isn't ready, log and continue.

fn to_rag_subtask(s: &SubTask) -> shodh_rag::agent::calendar_tools::SubTask {
    shodh_rag::agent::calendar_tools::SubTask {
        id: s.id.clone(),
        title: s.title.clone(),
        completed: s.completed,
    }
}

fn to_rag_task(task: &TodoItem) -> shodh_rag::agent::calendar_tools::TodoItem {
    shodh_rag::agent::calendar_tools::TodoItem {
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

fn to_rag_event(event: &CalendarEvent) -> shodh_rag::agent::calendar_tools::CalendarEvent {
    shodh_rag::agent::calendar_tools::CalendarEvent {
        id: event.id.clone(),
        title: event.title.clone(),
        description: event.description.clone(),
        start_time: event.start_time.clone(),
        end_time: event.end_time.clone(),
        all_day: event.all_day,
        color: event.color.clone(),
        source: event.source.clone(),
        source_ref: event.source_ref.clone(),
        created_at: event.created_at.clone(),
    }
}

/// Index a task in the RAG engine (best-effort, fire-and-forget).
fn spawn_index_task(app: &AppHandle, task: &TodoItem) {
    let rag_state: tauri::State<'_, RagState> = app.state();
    let rag = rag_state.rag.clone();
    let rag_task = to_rag_task(task);
    tokio::spawn(async move {
        let mut engine = rag.write().await;
        if let Err(e) = shodh_rag::agent::calendar_indexer::index_task(&mut engine, &rag_task, "calendar").await {
            tracing::warn!(task_id = %rag_task.id, error = %e, "Failed to index task in RAG");
        }
    });
}

/// Remove a task from the RAG index (best-effort, fire-and-forget).
fn spawn_deindex_task(app: &AppHandle, task_id: &str) {
    let rag_state: tauri::State<'_, RagState> = app.state();
    let rag = rag_state.rag.clone();
    let id = task_id.to_string();
    tokio::spawn(async move {
        let mut engine = rag.write().await;
        if let Err(e) = shodh_rag::agent::calendar_indexer::deindex_task(&mut engine, &id).await {
            tracing::warn!(task_id = %id, error = %e, "Failed to deindex task from RAG");
        }
    });
}

/// Index an event in the RAG engine (best-effort, fire-and-forget).
fn spawn_index_event(app: &AppHandle, event: &CalendarEvent) {
    let rag_state: tauri::State<'_, RagState> = app.state();
    let rag = rag_state.rag.clone();
    let rag_event = to_rag_event(event);
    tokio::spawn(async move {
        let mut engine = rag.write().await;
        if let Err(e) = shodh_rag::agent::calendar_indexer::index_event(&mut engine, &rag_event, "calendar").await {
            tracing::warn!(event_id = %rag_event.id, error = %e, "Failed to index event in RAG");
        }
    });
}

/// Remove an event from the RAG index (best-effort, fire-and-forget).
fn spawn_deindex_event(app: &AppHandle, event_id: &str) {
    let rag_state: tauri::State<'_, RagState> = app.state();
    let rag = rag_state.rag.clone();
    let id = event_id.to_string();
    tokio::spawn(async move {
        let mut engine = rag.write().await;
        if let Err(e) = shodh_rag::agent::calendar_indexer::deindex_event(&mut engine, &id).await {
            tracing::warn!(event_id = %id, error = %e, "Failed to deindex event from RAG");
        }
    });
}

// ── Task Commands ────────────────────────────────────────────────

#[tauri::command]
pub async fn load_tasks(app: AppHandle) -> Result<Vec<TodoItem>, String> {
    let data = read_calendar(&app)?;
    Ok(data.tasks)
}

#[tauri::command]
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
    let now = Utc::now().to_rfc3339();
    let task = TodoItem {
        id: Uuid::new_v4().to_string(),
        title,
        description: description.unwrap_or_default(),
        due_date,
        priority: priority.unwrap_or_else(|| "medium".to_string()),
        status: "pending".to_string(),
        tags: tags.unwrap_or_default(),
        subtasks: Vec::new(),
        project,
        source: source.unwrap_or_else(|| "user".to_string()),
        source_ref,
        created_at: now.clone(),
        updated_at: now,
        completed_at: None,
        reminder,
    };

    let mut data = read_calendar(&app)?;
    data.tasks.push(task.clone());
    write_calendar(&app, &data)?;

    spawn_index_task(&app, &task);

    tracing::info!(task_id = %task.id, title = %task.title, "Created task");
    Ok(task)
}

#[tauri::command]
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
) -> Result<TodoItem, String> {
    let mut data = read_calendar(&app)?;
    let task = data.tasks.iter_mut().find(|t| t.id == id)
        .ok_or_else(|| format!("Task not found: {}", id))?;

    if let Some(v) = title { task.title = v; }
    if let Some(v) = description { task.description = v; }
    if let Some(v) = due_date { task.due_date = Some(v); }
    if let Some(v) = priority { task.priority = v; }
    if let Some(v) = status {
        if v == "completed" && task.status != "completed" {
            task.completed_at = Some(Utc::now().to_rfc3339());
        } else if v != "completed" {
            task.completed_at = None;
        }
        task.status = v;
    }
    if let Some(v) = tags { task.tags = v; }
    if let Some(v) = project { task.project = Some(v); }
    if let Some(v) = reminder { task.reminder = Some(v); }
    task.updated_at = Utc::now().to_rfc3339();

    let updated = task.clone();
    write_calendar(&app, &data)?;

    spawn_index_task(&app, &updated);

    tracing::info!(task_id = %updated.id, "Updated task");
    Ok(updated)
}

#[tauri::command]
pub async fn delete_task(app: AppHandle, id: String) -> Result<bool, String> {
    let mut data = read_calendar(&app)?;
    let len_before = data.tasks.len();
    data.tasks.retain(|t| t.id != id);
    if data.tasks.len() == len_before {
        return Err(format!("Task not found: {}", id));
    }
    write_calendar(&app, &data)?;

    spawn_deindex_task(&app, &id);

    tracing::info!(task_id = %id, "Deleted task");
    Ok(true)
}

// ── Subtask Commands ─────────────────────────────────────────────

#[tauri::command]
pub async fn add_subtask(app: AppHandle, task_id: String, title: String) -> Result<TodoItem, String> {
    let mut data = read_calendar(&app)?;
    let task = data.tasks.iter_mut().find(|t| t.id == task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;

    task.subtasks.push(SubTask {
        id: Uuid::new_v4().to_string(),
        title,
        completed: false,
    });
    task.updated_at = Utc::now().to_rfc3339();

    let updated = task.clone();
    write_calendar(&app, &data)?;

    // Re-index parent task with updated subtasks
    spawn_index_task(&app, &updated);

    Ok(updated)
}

#[tauri::command]
pub async fn toggle_subtask(app: AppHandle, task_id: String, subtask_id: String) -> Result<TodoItem, String> {
    let mut data = read_calendar(&app)?;
    let task = data.tasks.iter_mut().find(|t| t.id == task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;

    let subtask = task.subtasks.iter_mut().find(|s| s.id == subtask_id)
        .ok_or_else(|| format!("Subtask not found: {}", subtask_id))?;

    subtask.completed = !subtask.completed;
    task.updated_at = Utc::now().to_rfc3339();

    let updated = task.clone();
    write_calendar(&app, &data)?;

    spawn_index_task(&app, &updated);

    Ok(updated)
}

#[tauri::command]
pub async fn delete_subtask(app: AppHandle, task_id: String, subtask_id: String) -> Result<TodoItem, String> {
    let mut data = read_calendar(&app)?;
    let task = data.tasks.iter_mut().find(|t| t.id == task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;

    task.subtasks.retain(|s| s.id != subtask_id);
    task.updated_at = Utc::now().to_rfc3339();

    let updated = task.clone();
    write_calendar(&app, &data)?;

    spawn_index_task(&app, &updated);

    Ok(updated)
}

// ── Event Commands ───────────────────────────────────────────────

#[tauri::command]
pub async fn load_events(app: AppHandle) -> Result<Vec<CalendarEvent>, String> {
    let data = read_calendar(&app)?;
    Ok(data.events)
}

#[tauri::command]
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
) -> Result<CalendarEvent, String> {
    let event = CalendarEvent {
        id: Uuid::new_v4().to_string(),
        title,
        description: description.unwrap_or_default(),
        start_time,
        end_time,
        all_day: all_day.unwrap_or(false),
        color,
        source: source.unwrap_or_else(|| "user".to_string()),
        source_ref,
        created_at: Utc::now().to_rfc3339(),
    };

    let mut data = read_calendar(&app)?;
    data.events.push(event.clone());
    write_calendar(&app, &data)?;

    spawn_index_event(&app, &event);

    tracing::info!(event_id = %event.id, title = %event.title, "Created event");
    Ok(event)
}

#[tauri::command]
pub async fn update_event(
    app: AppHandle,
    id: String,
    title: Option<String>,
    description: Option<String>,
    start_time: Option<String>,
    end_time: Option<String>,
    all_day: Option<bool>,
    color: Option<String>,
) -> Result<CalendarEvent, String> {
    let mut data = read_calendar(&app)?;
    let event = data.events.iter_mut().find(|e| e.id == id)
        .ok_or_else(|| format!("Event not found: {}", id))?;

    if let Some(v) = title { event.title = v; }
    if let Some(v) = description { event.description = v; }
    if let Some(v) = start_time { event.start_time = v; }
    if let Some(v) = end_time { event.end_time = Some(v); }
    if let Some(v) = all_day { event.all_day = v; }
    if let Some(v) = color { event.color = Some(v); }

    let updated = event.clone();
    write_calendar(&app, &data)?;

    spawn_index_event(&app, &updated);

    tracing::info!(event_id = %updated.id, "Updated event");
    Ok(updated)
}

#[tauri::command]
pub async fn delete_event(app: AppHandle, id: String) -> Result<bool, String> {
    let mut data = read_calendar(&app)?;
    let len_before = data.events.len();
    data.events.retain(|e| e.id != id);
    if data.events.len() == len_before {
        return Err(format!("Event not found: {}", id));
    }
    write_calendar(&app, &data)?;

    spawn_deindex_event(&app, &id);

    tracing::info!(event_id = %id, "Deleted event");
    Ok(true)
}

// ── Bulk Reindex ─────────────────────────────────────────────────

/// Re-index all existing calendar data into the RAG engine.
/// Called on startup to ensure the search index is populated.
pub async fn reindex_all_calendar_data(app: &AppHandle) {
    let data = match read_calendar(app) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "Could not read calendar data for reindexing");
            return;
        }
    };

    if data.tasks.is_empty() && data.events.is_empty() {
        return;
    }

    let rag_state: tauri::State<'_, RagState> = app.state();
    let rag = rag_state.rag.clone();

    let rag_tasks: Vec<_> = data.tasks.iter().map(to_rag_task).collect();
    let rag_events: Vec<_> = data.events.iter().map(to_rag_event).collect();

    tokio::spawn(async move {
        let mut engine = rag.write().await;
        match shodh_rag::agent::calendar_indexer::reindex_all(&mut engine, &rag_tasks, &rag_events, "calendar").await {
            Ok((t, e)) => tracing::info!(tasks = t, events = e, "Calendar data reindexed into RAG on startup"),
            Err(e) => tracing::warn!(error = %e, "Failed to reindex calendar data into RAG"),
        }
    });
}
