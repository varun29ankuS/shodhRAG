//! Calendar & Task Tools — Agent tools for creating/querying tasks and events.
//!
//! Backed by a `CalendarStore` that persists to a JSON file.
//! These tools let agents create reminders, deadlines, and events
//! on behalf of the user during conversation.

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::context::AgentContext;
use super::tools::{AgentTool, ToolInput, ToolResult};
use crate::rag_engine::RAGEngine;

// ── Shared Data Structures ───────────────────────────────────────

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

fn default_priority() -> String {
    "medium".to_string()
}
fn default_status() -> String {
    "pending".to_string()
}
fn default_source() -> String {
    "agent".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CalendarDataFile {
    #[serde(default)]
    tasks: Vec<TodoItem>,
    #[serde(default)]
    events: Vec<CalendarEvent>,
}

// ── CalendarStore ────────────────────────────────────────────────

/// In-memory + file-backed store for tasks and events.
/// Shared between agent tools and Tauri commands.
///
/// When a RAG engine reference is set, mutations also index/deindex
/// items in the vector store for semantic search.
pub struct CalendarStore {
    data: CalendarDataFile,
    path: Option<PathBuf>,
    /// Optional RAG engine for semantic indexing of calendar items.
    rag_engine: Option<Arc<RwLock<RAGEngine>>>,
}

impl CalendarStore {
    pub fn new() -> Self {
        Self {
            data: CalendarDataFile::default(),
            path: None,
            rag_engine: None,
        }
    }

    /// Set the file path and load existing data.
    pub fn set_path(&mut self, path: PathBuf) {
        if path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                if let Ok(loaded) = serde_json::from_str::<CalendarDataFile>(&contents) {
                    self.data = loaded;
                }
            }
        }
        self.path = Some(path);
    }

    /// Set the RAG engine reference for semantic indexing.
    pub fn set_rag_engine(&mut self, engine: Arc<RwLock<RAGEngine>>) {
        self.rag_engine = Some(engine);
    }

    fn persist(&self) -> Result<()> {
        if let Some(ref path) = self.path {
            let tmp = path.with_extension("json.tmp");
            let json = serde_json::to_string_pretty(&self.data)?;
            std::fs::write(&tmp, &json)?;
            std::fs::rename(&tmp, path)?;
        }
        Ok(())
    }

    /// Fire-and-forget RAG indexing for a task.
    /// Best-effort: JSON file is the source of truth, RAG index is a search overlay.
    fn spawn_index_task(&self, task: TodoItem) {
        if let Some(ref rag) = self.rag_engine {
            let rag = rag.clone();
            tokio::spawn(async move {
                let mut engine = rag.write().await;
                if let Err(e) =
                    super::calendar_indexer::index_task(&mut engine, &task, "calendar").await
                {
                    tracing::warn!(task_id = %task.id, error = %e, "Failed to index task in RAG");
                }
            });
        }
    }

    /// Fire-and-forget RAG indexing for an event.
    fn spawn_index_event(&self, event: CalendarEvent) {
        if let Some(ref rag) = self.rag_engine {
            let rag = rag.clone();
            tokio::spawn(async move {
                let mut engine = rag.write().await;
                if let Err(e) =
                    super::calendar_indexer::index_event(&mut engine, &event, "calendar").await
                {
                    tracing::warn!(event_id = %event.id, error = %e, "Failed to index event in RAG");
                }
            });
        }
    }

    pub fn add_task(&mut self, task: TodoItem) -> Result<TodoItem> {
        self.data.tasks.push(task.clone());
        self.persist()?;
        self.spawn_index_task(task.clone());
        Ok(task)
    }

    pub fn list_tasks(
        &self,
        status: Option<&str>,
        from_date: Option<&str>,
        to_date: Option<&str>,
    ) -> Vec<&TodoItem> {
        self.data
            .tasks
            .iter()
            .filter(|t| {
                if let Some(s) = status {
                    if t.status != s {
                        return false;
                    }
                }
                if let Some(from) = from_date {
                    if let Some(ref dd) = t.due_date {
                        if dd.as_str() < from {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                if let Some(to) = to_date {
                    if let Some(ref dd) = t.due_date {
                        if dd.as_str() > to {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                true
            })
            .collect()
    }

    pub fn add_event(&mut self, event: CalendarEvent) -> Result<CalendarEvent> {
        self.data.events.push(event.clone());
        self.persist()?;
        self.spawn_index_event(event.clone());
        Ok(event)
    }

    pub fn list_events(&self) -> &[CalendarEvent] {
        &self.data.events
    }
}

pub type SharedCalendarStore = Arc<RwLock<CalendarStore>>;

pub fn new_calendar_store() -> SharedCalendarStore {
    Arc::new(RwLock::new(CalendarStore::new()))
}

// ── CreateTaskTool ───────────────────────────────────────────────

pub struct CreateTaskTool {
    store: SharedCalendarStore,
}

impl CreateTaskTool {
    pub fn new(store: SharedCalendarStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl AgentTool for CreateTaskTool {
    fn id(&self) -> &str {
        "create_task"
    }
    fn name(&self) -> &str {
        "Create Task"
    }

    fn description(&self) -> &str {
        "Create a task or reminder for the user. Use when the user asks to remember something, \
         schedule a follow-up, or when you find an actionable deadline in a document. \
         The task appears in the user's Tasks tab."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short, actionable task title"
                },
                "description": {
                    "type": "string",
                    "description": "Detailed description or context (supports markdown)"
                },
                "due_date": {
                    "type": "string",
                    "description": "Due date in ISO 8601 format (e.g., 2026-02-25T17:00:00Z)"
                },
                "priority": {
                    "type": "string",
                    "enum": ["low", "medium", "high"],
                    "description": "Task priority level"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tags for categorization"
                },
                "project": {
                    "type": "string",
                    "description": "Project name to assign this task to"
                }
            },
            "required": ["title"]
        })
    }

    async fn execute(&self, input: ToolInput, _context: AgentContext) -> Result<ToolResult> {
        let title = input.parameters["title"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'title' parameter"))?
            .to_string();

        let description = input
            .parameters
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let due_date = input
            .parameters
            .get("due_date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let priority = input
            .parameters
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("medium")
            .to_string();

        let tags: Vec<String> = input
            .parameters
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let project = input
            .parameters
            .get("project")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let now = Utc::now().to_rfc3339();
        let task = TodoItem {
            id: Uuid::new_v4().to_string(),
            title: title.clone(),
            description,
            due_date: due_date.clone(),
            priority: priority.clone(),
            status: "pending".to_string(),
            tags: tags.clone(),
            subtasks: Vec::new(),
            project,
            source: "agent".to_string(),
            source_ref: None,
            created_at: now.clone(),
            updated_at: now,
            completed_at: None,
            reminder: None,
        };

        let mut store = self.store.write().await;
        store.add_task(task.clone())?;

        let due_str = due_date.as_deref().unwrap_or("no due date");
        let output = format!(
            "Created task: \"{}\" (priority: {}, due: {}). It's now visible in the Tasks tab.",
            title, priority, due_str
        );

        Ok(ToolResult {
            success: true,
            output,
            data: serde_json::to_value(&task)?,
            error: None,
        })
    }
}

// ── CreateEventTool ──────────────────────────────────────────────

pub struct CreateEventTool {
    store: SharedCalendarStore,
}

impl CreateEventTool {
    pub fn new(store: SharedCalendarStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl AgentTool for CreateEventTool {
    fn id(&self) -> &str {
        "create_event"
    }
    fn name(&self) -> &str {
        "Create Calendar Event"
    }

    fn description(&self) -> &str {
        "Create a calendar event. Use when the user mentions a meeting, appointment, \
         deadline, or any time-based event they want to track."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Event title"
                },
                "start_time": {
                    "type": "string",
                    "description": "Start time in ISO 8601 format (e.g., 2026-02-25T14:00:00Z)"
                },
                "end_time": {
                    "type": "string",
                    "description": "End time in ISO 8601 format"
                },
                "all_day": {
                    "type": "boolean",
                    "description": "Whether this is an all-day event"
                },
                "description": {
                    "type": "string",
                    "description": "Event description or notes"
                }
            },
            "required": ["title", "start_time"]
        })
    }

    async fn execute(&self, input: ToolInput, _context: AgentContext) -> Result<ToolResult> {
        let title = input.parameters["title"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'title' parameter"))?
            .to_string();

        let start_time = input.parameters["start_time"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'start_time' parameter"))?
            .to_string();

        let end_time = input
            .parameters
            .get("end_time")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let all_day = input
            .parameters
            .get("all_day")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let description = input
            .parameters
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let event = CalendarEvent {
            id: Uuid::new_v4().to_string(),
            title: title.clone(),
            description,
            start_time: start_time.clone(),
            end_time,
            all_day,
            color: None,
            source: "agent".to_string(),
            source_ref: None,
            created_at: Utc::now().to_rfc3339(),
        };

        let mut store = self.store.write().await;
        store.add_event(event.clone())?;

        let output = format!(
            "Created calendar event: \"{}\" at {}. It's now visible in the Tasks tab calendar.",
            title, start_time
        );

        Ok(ToolResult {
            success: true,
            output,
            data: serde_json::to_value(&event)?,
            error: None,
        })
    }
}

// ── ListTasksTool ────────────────────────────────────────────────

pub struct ListTasksTool {
    store: SharedCalendarStore,
}

impl ListTasksTool {
    pub fn new(store: SharedCalendarStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl AgentTool for ListTasksTool {
    fn id(&self) -> &str {
        "list_tasks"
    }
    fn name(&self) -> &str {
        "List Tasks"
    }

    fn description(&self) -> &str {
        "List the user's tasks, optionally filtered by status or date range. \
         Use to check what's pending, find overdue items, or summarize upcoming work."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed"],
                    "description": "Filter by task status"
                },
                "from_date": {
                    "type": "string",
                    "description": "Only include tasks due on or after this date (ISO 8601)"
                },
                "to_date": {
                    "type": "string",
                    "description": "Only include tasks due on or before this date (ISO 8601)"
                }
            }
        })
    }

    async fn execute(&self, input: ToolInput, _context: AgentContext) -> Result<ToolResult> {
        let status = input.parameters.get("status").and_then(|v| v.as_str());
        let from_date = input.parameters.get("from_date").and_then(|v| v.as_str());
        let to_date = input.parameters.get("to_date").and_then(|v| v.as_str());

        let store = self.store.read().await;
        let tasks = store.list_tasks(status, from_date, to_date);

        if tasks.is_empty() {
            return Ok(ToolResult {
                success: true,
                output: "No tasks found matching the criteria.".to_string(),
                data: serde_json::json!([]),
                error: None,
            });
        }

        let mut output = format!("Found {} task(s):\n", tasks.len());
        for (i, t) in tasks.iter().enumerate() {
            let due = t.due_date.as_deref().unwrap_or("no due date");
            let check = if t.status == "completed" {
                "[x]"
            } else {
                "[ ]"
            };
            output.push_str(&format!(
                "{}. {} **{}** — priority: {}, due: {}, status: {}\n",
                i + 1,
                check,
                t.title,
                t.priority,
                due,
                t.status
            ));
            if !t.description.is_empty() {
                output.push_str(&format!(
                    "   {}\n",
                    t.description.lines().next().unwrap_or("")
                ));
            }
        }

        let data: Vec<serde_json::Value> = tasks
            .iter()
            .map(|t| serde_json::to_value(t).unwrap_or_default())
            .collect();

        Ok(ToolResult {
            success: true,
            output,
            data: serde_json::json!(data),
            error: None,
        })
    }
}

/// Register all calendar tools into a ToolRegistry.
pub fn register_calendar_tools(
    registry: &mut super::tools::ToolRegistry,
    store: SharedCalendarStore,
) {
    registry.register(Arc::new(CreateTaskTool::new(store.clone())));
    registry.register(Arc::new(CreateEventTool::new(store.clone())));
    registry.register(Arc::new(ListTasksTool::new(store)));
}
