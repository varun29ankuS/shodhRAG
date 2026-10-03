//! Calendar tools: create, list, update, complete and delete tasks and
//! events, and show a date, task or event in the Calendar view.
//!
//! Reads go straight to [`CalendarStore`]; every write goes through
//! `CalendarStore::update` and is reported to
//! [`super::HostEffects::calendar_changed`] (re-index + refresh). Update
//! previews compute the change with the same function the write uses, so
//! the approval card shows exactly the before → after that will be saved.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use serde_json::{json, Value};
use shodh_rag::harness::events::NavigationTarget;
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::navigate::emit_navigation;
use shodh_rag::harness::tools::{
    ApprovalPreview, HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::RiskTier;

use super::{invalid, limit_arg, str_arg, AgentHost, CalendarChange};
use crate::calendar_store::{
    apply_event_patch, apply_task_patch, list_events, list_tasks, parse_moment, CalendarError,
    CalendarEvent, CalendarStore, EventFilter, EventPatch, FieldChange, Moment, NewEvent, NewTask,
    SubtaskSpec, TaskFilter, TaskPatch, TodoItem, PRIORITIES, STATUSES,
};

const DEFAULT_LIST: usize = 25;
const MAX_LIST: usize = 100;

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    let h = || host.clone();
    registry.register(Arc::new(CreateTaskTool { host: h() }))?;
    registry.register(Arc::new(CreateEventTool { host: h() }))?;
    registry.register(Arc::new(ListTasksTool { host: h() }))?;
    registry.register(Arc::new(ListEventsTool { host: h() }))?;
    registry.register(Arc::new(UpdateTaskTool { host: h() }))?;
    registry.register(Arc::new(CompleteTaskTool { host: h() }))?;
    registry.register(Arc::new(UpdateEventTool { host: h() }))?;
    registry.register(Arc::new(DeleteTaskTool { host: h() }))?;
    registry.register(Arc::new(DeleteEventTool { host: h() }))?;
    registry.register(Arc::new(ShowCalendarTool { host: h() }))?;
    Ok(())
}

fn store(host: &AgentHost) -> CalendarStore {
    CalendarStore::in_dir(&host.data_dir)
}

/// Map a storage error to what the model sees.
fn tool_error(tool: &str, error: CalendarError) -> ToolError {
    match error {
        CalendarError::TaskNotFound(id) => ToolError::NotFound(format!(
            "No task has id {id}. Call list_tasks for valid ids."
        )),
        CalendarError::EventNotFound(id) => ToolError::NotFound(format!(
            "No event has id {id}. Call list_events for valid ids."
        )),
        CalendarError::SubtaskNotFound(id) => ToolError::NotFound(format!(
            "The task has no subtask with id {id}. Omit the id to add a new subtask."
        )),
        CalendarError::Invalid(reason) => invalid(tool, reason),
        other => ToolError::Failed(other.to_string()),
    }
}

fn date_arg(tool: &str, args: &Value, key: &str) -> Result<Option<NaiveDate>, ToolError> {
    match str_arg(args, key) {
        Some(raw) => parse_moment(raw)
            .map(|m| Some(m.date()))
            .ok_or_else(|| invalid(tool, format!("`{key}` {raw:?}: use YYYY-MM-DD"))),
        None => Ok(None),
    }
}

fn changes_json(changes: &[FieldChange]) -> Value {
    Value::Array(changes.iter().map(FieldChange::to_json).collect())
}

fn task_line(task: &TodoItem) -> Value {
    let done = task.subtasks.iter().filter(|s| s.completed).count();
    json!({
        "id": task.id,
        "title": task.title,
        "due": task.due_date,
        "status": task.status,
        "priority": task.priority,
        "project": task.project,
        "subtasks": if task.subtasks.is_empty() {
            Value::Null
        } else {
            json!(format!("{done}/{} done", task.subtasks.len()))
        },
    })
}

fn event_line(event: &CalendarEvent) -> Value {
    json!({
        "id": event.id,
        "title": event.title,
        "start": event.start_time,
        "end": event.end_time,
        "allDay": event.all_day,
    })
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Render a change list for the model: `title: "A" → "B"; due: … `.
fn describe_changes(changes: &[FieldChange]) -> String {
    changes
        .iter()
        .map(|c| format!("{}: {} → {}", c.field, c.before, c.after))
        .collect::<Vec<_>>()
        .join("; ")
}

// ── create_task ────────────────────────────────────────────────────────────

pub struct CreateTaskTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for CreateTaskTool {
    fn name(&self) -> &'static str {
        app_tools::CREATE_TASK
    }
    fn label(&self) -> &'static str {
        "Create task"
    }
    fn label_template(&self) -> &'static str {
        "Creating task {title}"
    }
    fn description(&self) -> &'static str {
        "Create a calendar task (a to-do with an optional due date and priority). source_ref can \
         hold the file the task came from, e.g. a contract path."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "minLength": 1, "maxLength": 200},
                "due": {"type": "string", "minLength": 10, "maxLength": 40},
                "priority": {"type": "string", "enum": PRIORITIES},
                "notes": {"type": "string", "maxLength": 4000},
                "source_ref": {"type": "string", "minLength": 1, "maxLength": 1024}
            },
            "required": ["title"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        Ok(ApprovalPreview {
            label: None,
            details: json!({
                "title": str_arg(args, "title"),
                "due": str_arg(args, "due"),
                "priority": str_arg(args, "priority"),
                "notes": str_arg(args, "notes"),
                "sourceRef": str_arg(args, "source_ref"),
            }),
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::CREATE_TASK;
        let title = str_arg(&args, "title").ok_or_else(|| invalid(tool, "`title` is required"))?;
        let new = NewTask {
            title: title.to_string(),
            due_date: str_arg(&args, "due").map(str::to_string),
            priority: str_arg(&args, "priority").map(str::to_string),
            description: str_arg(&args, "notes").map(str::to_string),
            source: Some("agent".to_string()),
            source_ref: str_arg(&args, "source_ref").map(str::to_string),
            ..NewTask::default()
        };
        let task = store(&self.host)
            .update(|d| d.insert_task(new))
            .map_err(|e| tool_error(tool, e))?;
        self.host
            .effects
            .calendar_changed(CalendarChange::TaskSaved(task.clone()));
        let due_text = task
            .due_date
            .as_deref()
            .map(|d| format!(", due {d}"))
            .unwrap_or_default();
        Ok(ToolOutput {
            text_for_model: format!(
                "Created task \"{}\" (id {}{due_text}). Use show_calendar with this task_id to show it.",
                task.title, task.id
            ),
            summary_for_ui: format!("Created task “{}”{due_text}", task.title),
            detail: Some(json!({ "id": task.id, "title": task.title, "due": task.due_date })),
        })
    }
}

// ── create_event ───────────────────────────────────────────────────────────

pub struct CreateEventTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for CreateEventTool {
    fn name(&self) -> &'static str {
        app_tools::CREATE_EVENT
    }
    fn label(&self) -> &'static str {
        "Create event"
    }
    fn label_template(&self) -> &'static str {
        "Creating event {title}"
    }
    fn description(&self) -> &'static str {
        "Create a calendar event. A date without a time makes an all-day event."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "minLength": 1, "maxLength": 200},
                "start": {"type": "string", "minLength": 10, "maxLength": 40},
                "end": {"type": "string", "minLength": 10, "maxLength": 40},
                "notes": {"type": "string", "maxLength": 4000}
            },
            "required": ["title", "start"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        Ok(ApprovalPreview {
            label: None,
            details: json!({
                "title": str_arg(args, "title"),
                "start": str_arg(args, "start"),
                "end": str_arg(args, "end"),
                "notes": str_arg(args, "notes"),
            }),
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::CREATE_EVENT;
        let title = str_arg(&args, "title").ok_or_else(|| invalid(tool, "`title` is required"))?;
        let start = str_arg(&args, "start").ok_or_else(|| invalid(tool, "`start` is required"))?;
        let all_day = matches!(parse_moment(start), Some(Moment::Date(_)));
        let new = NewEvent {
            title: title.to_string(),
            start_time: start.to_string(),
            end_time: str_arg(&args, "end").map(str::to_string),
            all_day: Some(all_day),
            description: str_arg(&args, "notes").map(str::to_string),
            source: Some("agent".to_string()),
            ..NewEvent::default()
        };
        let event = store(&self.host)
            .update(|d| d.insert_event(new))
            .map_err(|e| tool_error(tool, e))?;
        self.host
            .effects
            .calendar_changed(CalendarChange::EventSaved(event.clone()));
        Ok(ToolOutput {
            text_for_model: format!(
                "Created event \"{}\" (id {}) starting {}. Use show_calendar with this event_id to show it.",
                event.title, event.id, event.start_time
            ),
            summary_for_ui: format!("Created event “{}” on {}", event.title, event.start_time),
            detail: Some(json!({
                "id": event.id,
                "title": event.title,
                "start": event.start_time,
                "end": event.end_time,
                "allDay": event.all_day
            })),
        })
    }
}

// ── list_tasks ─────────────────────────────────────────────────────────────

pub struct ListTasksTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for ListTasksTool {
    fn name(&self) -> &'static str {
        app_tools::LIST_TASKS
    }
    fn label(&self) -> &'static str {
        "List tasks"
    }
    fn label_template(&self) -> &'static str {
        "Listing[ {status!}] tasks[ due from {due_from!}][ due by {due_to!}][ matching {text}]"
    }
    fn description(&self) -> &'static str {
        "List calendar tasks with their ids, titles, due dates, status and priority. Filter by \
         status, an inclusive due-date range (YYYY-MM-DD; tasks without a due date are left out \
         when a range is given) and text in the title, notes, tags or project. Use it to find a \
         task's id before changing it, and to check your own changes."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {"type": "string", "enum": STATUSES},
                "due_from": {"type": "string", "minLength": 10, "maxLength": 40},
                "due_to": {"type": "string", "minLength": 10, "maxLength": 40},
                "text": {"type": "string", "minLength": 1, "maxLength": 200},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIST}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::LIST_TASKS;
        let filter = TaskFilter {
            status: str_arg(&args, "status").map(str::to_string),
            due_from: date_arg(tool, &args, "due_from")?,
            due_to: date_arg(tool, &args, "due_to")?,
            text: str_arg(&args, "text").map(str::to_string),
        };
        let limit = limit_arg(&args, DEFAULT_LIST, MAX_LIST);
        let data = store(&self.host).load().map_err(|e| tool_error(tool, e))?;
        let matched = list_tasks(&data, &filter);
        let total = matched.len();
        let shown: Vec<Value> = matched.iter().take(limit).map(|t| task_line(t)).collect();
        let more = if total > shown.len() {
            format!(
                " Showing the first {}; narrow the filter to see the rest.",
                shown.len()
            )
        } else {
            String::new()
        };
        let body = serde_json::to_string(&shown)
            .map_err(|e| ToolError::Failed(format!("Could not encode tasks: {e}")))?;
        Ok(ToolOutput {
            text_for_model: format!("{} matched.{more}\n{body}", plural(total, "task", "tasks")),
            summary_for_ui: plural(total, "task", "tasks"),
            detail: Some(json!({ "total": total, "tasks": shown })),
        })
    }
}

// ── list_events ────────────────────────────────────────────────────────────

pub struct ListEventsTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for ListEventsTool {
    fn name(&self) -> &'static str {
        app_tools::LIST_EVENTS
    }
    fn label(&self) -> &'static str {
        "List events"
    }
    fn label_template(&self) -> &'static str {
        "Listing events[ from {from!}][ to {to!}][ matching {text}]"
    }
    fn description(&self) -> &'static str {
        "List calendar events with their ids, titles and times. Filter by an inclusive date range \
         (YYYY-MM-DD; events overlapping it are included) and text in the title or notes."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "from": {"type": "string", "minLength": 10, "maxLength": 40},
                "to": {"type": "string", "minLength": 10, "maxLength": 40},
                "text": {"type": "string", "minLength": 1, "maxLength": 200},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIST}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::LIST_EVENTS;
        let filter = EventFilter {
            from: date_arg(tool, &args, "from")?,
            to: date_arg(tool, &args, "to")?,
            text: str_arg(&args, "text").map(str::to_string),
        };
        let limit = limit_arg(&args, DEFAULT_LIST, MAX_LIST);
        let data = store(&self.host).load().map_err(|e| tool_error(tool, e))?;
        let matched = list_events(&data, &filter);
        let total = matched.len();
        let shown: Vec<Value> = matched.iter().take(limit).map(|e| event_line(e)).collect();
        let more = if total > shown.len() {
            format!(
                " Showing the first {}; narrow the filter to see the rest.",
                shown.len()
            )
        } else {
            String::new()
        };
        let body = serde_json::to_string(&shown)
            .map_err(|e| ToolError::Failed(format!("Could not encode events: {e}")))?;
        Ok(ToolOutput {
            text_for_model: format!(
                "{} matched.{more}\n{body}",
                plural(total, "event", "events")
            ),
            summary_for_ui: plural(total, "event", "events"),
            detail: Some(json!({ "total": total, "events": shown })),
        })
    }
}

// ── update_task / complete_task ────────────────────────────────────────────

fn task_patch_from_args(tool: &str, args: &Value) -> Result<TaskPatch, ToolError> {
    let due_date = match args.get("due") {
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.trim().to_string())),
        _ => None,
    };
    let subtasks = args.get("subtasks").and_then(Value::as_array).map(|items| {
        items
            .iter()
            .map(|item| SubtaskSpec {
                id: str_arg(item, "id").map(str::to_string),
                title: str_arg(item, "title").unwrap_or_default().to_string(),
                completed: item
                    .get("completed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
            .collect()
    });
    let patch = TaskPatch {
        title: str_arg(args, "title").map(str::to_string),
        description: args
            .get("notes")
            .and_then(Value::as_str)
            .map(str::to_string),
        due_date,
        priority: str_arg(args, "priority").map(str::to_string),
        status: str_arg(args, "status").map(str::to_string),
        subtasks,
        ..TaskPatch::default()
    };
    if patch.is_empty() {
        return Err(invalid(
            tool,
            "give at least one field to change: title, notes, due, priority, status or subtasks",
        ));
    }
    Ok(patch)
}

/// The task as stored and the changes `patch` would make, without saving.
fn preview_task_patch(
    host: &AgentHost,
    tool: &str,
    task_id: &str,
    patch: &TaskPatch,
) -> Result<(TodoItem, Vec<FieldChange>), ToolError> {
    let data = store(host).load().map_err(|e| tool_error(tool, e))?;
    let mut task = data.task(task_id).map_err(|e| tool_error(tool, e))?.clone();
    let before = task.clone();
    let changes = apply_task_patch(&mut task, patch).map_err(|e| tool_error(tool, e))?;
    if changes.is_empty() {
        return Err(ToolError::Failed(format!(
            "Task \"{}\" already has those values; nothing to change.",
            before.title
        )));
    }
    Ok((before, changes))
}

/// Save `patch` to the task and report it.
fn save_task_patch(
    host: &AgentHost,
    tool: &str,
    task_id: &str,
    patch: &TaskPatch,
) -> Result<ToolOutput, ToolError> {
    let (before, after, changes) = store(host)
        .update(|d| d.patch_task(task_id, patch))
        .map_err(|e| tool_error(tool, e))?;
    if changes.is_empty() {
        return Ok(ToolOutput {
            text_for_model: format!(
                "Task \"{}\" (id {}) already had those values; nothing changed.",
                after.title, after.id
            ),
            summary_for_ui: format!("No change to “{}”", after.title),
            detail: Some(json!({ "id": after.id, "changes": [] })),
        });
    }
    host.effects
        .calendar_changed(CalendarChange::TaskSaved(after.clone()));
    Ok(ToolOutput {
        text_for_model: format!(
            "Updated task \"{}\" (id {}): {}.",
            before.title,
            after.id,
            describe_changes(&changes)
        ),
        summary_for_ui: format!(
            "Updated “{}” ({})",
            after.title,
            changes
                .iter()
                .map(|c| c.field)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        detail: Some(
            json!({ "id": after.id, "title": after.title, "changes": changes_json(&changes) }),
        ),
    })
}

pub struct UpdateTaskTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for UpdateTaskTool {
    fn name(&self) -> &'static str {
        app_tools::UPDATE_TASK
    }
    fn label(&self) -> &'static str {
        "Update task"
    }
    fn label_template(&self) -> &'static str {
        "Updating task[ {title}]"
    }
    fn description(&self) -> &'static str {
        "Change a task (id from list_tasks): title, notes, due date (null clears it), priority, \
         status, or the full subtask list (keep a subtask's id to keep it; omit the id to add one; \
         leave one out to remove it). Only the fields you pass change."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "string", "minLength": 1, "maxLength": 200},
                "title": {"type": "string", "minLength": 1, "maxLength": 200},
                "notes": {"type": "string", "maxLength": 4000},
                "due": {"type": ["string", "null"], "maxLength": 40},
                "priority": {"type": "string", "enum": PRIORITIES},
                "status": {"type": "string", "enum": STATUSES},
                "subtasks": {
                    "type": "array",
                    "maxItems": 50,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "minLength": 1, "maxLength": 200},
                            "title": {"type": "string", "minLength": 1, "maxLength": 200},
                            "completed": {"type": "boolean"}
                        },
                        "required": ["title"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["task_id"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let tool = app_tools::UPDATE_TASK;
        let task_id =
            str_arg(args, "task_id").ok_or_else(|| invalid(tool, "`task_id` is required"))?;
        let patch = task_patch_from_args(tool, args)?;
        let (before, changes) = preview_task_patch(&self.host, tool, task_id, &patch)?;
        Ok(ApprovalPreview {
            label: Some(format!("Update task “{}”", before.title)),
            details: json!({ "task": before.title, "changes": changes_json(&changes) }),
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::UPDATE_TASK;
        let task_id =
            str_arg(&args, "task_id").ok_or_else(|| invalid(tool, "`task_id` is required"))?;
        let patch = task_patch_from_args(tool, &args)?;
        save_task_patch(&self.host, tool, task_id, &patch)
    }
}

pub struct CompleteTaskTool {
    host: Arc<AgentHost>,
}

fn complete_patch() -> TaskPatch {
    TaskPatch {
        status: Some("completed".to_string()),
        ..TaskPatch::default()
    }
}

fn task_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string", "minLength": 1, "maxLength": 200}
        },
        "required": ["task_id"],
        "additionalProperties": false
    })
}

#[async_trait]
impl HostTool for CompleteTaskTool {
    fn name(&self) -> &'static str {
        app_tools::COMPLETE_TASK
    }
    fn label(&self) -> &'static str {
        "Complete task"
    }
    fn label_template(&self) -> &'static str {
        "Marking a task done"
    }
    fn description(&self) -> &'static str {
        "Mark a task (id from list_tasks) as completed."
    }
    fn schema(&self) -> Value {
        task_id_schema()
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let tool = app_tools::COMPLETE_TASK;
        let task_id =
            str_arg(args, "task_id").ok_or_else(|| invalid(tool, "`task_id` is required"))?;
        let (before, changes) = preview_task_patch(&self.host, tool, task_id, &complete_patch())?;
        Ok(ApprovalPreview {
            label: Some(format!("Mark “{}” as done", before.title)),
            details: json!({ "task": before.title, "changes": changes_json(&changes) }),
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::COMPLETE_TASK;
        let task_id =
            str_arg(&args, "task_id").ok_or_else(|| invalid(tool, "`task_id` is required"))?;
        save_task_patch(&self.host, tool, task_id, &complete_patch())
    }
}

// ── update_event ───────────────────────────────────────────────────────────

fn event_patch_from_args(tool: &str, args: &Value) -> Result<EventPatch, ToolError> {
    let end_time = match args.get("end") {
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.trim().to_string())),
        _ => None,
    };
    let start_time = str_arg(args, "start").map(str::to_string);
    let all_day = match (&start_time, args.get("all_day").and_then(Value::as_bool)) {
        (_, Some(explicit)) => Some(explicit),
        (Some(start), None) => Some(matches!(parse_moment(start), Some(Moment::Date(_)))),
        (None, None) => None,
    };
    let patch = EventPatch {
        title: str_arg(args, "title").map(str::to_string),
        description: args
            .get("notes")
            .and_then(Value::as_str)
            .map(str::to_string),
        start_time,
        end_time,
        all_day,
        color: None,
    };
    if patch.is_empty() {
        return Err(invalid(
            tool,
            "give at least one field to change: title, notes, start, end or all_day",
        ));
    }
    Ok(patch)
}

pub struct UpdateEventTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for UpdateEventTool {
    fn name(&self) -> &'static str {
        app_tools::UPDATE_EVENT
    }
    fn label(&self) -> &'static str {
        "Update event"
    }
    fn label_template(&self) -> &'static str {
        "Updating event[ {title}]"
    }
    fn description(&self) -> &'static str {
        "Change an event (id from list_events): title, notes, start, end (null clears it) or \
         all_day. A start given as a date alone makes it all-day. Only the fields you pass change."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "event_id": {"type": "string", "minLength": 1, "maxLength": 200},
                "title": {"type": "string", "minLength": 1, "maxLength": 200},
                "notes": {"type": "string", "maxLength": 4000},
                "start": {"type": "string", "minLength": 10, "maxLength": 40},
                "end": {"type": ["string", "null"], "maxLength": 40},
                "all_day": {"type": "boolean"}
            },
            "required": ["event_id"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let tool = app_tools::UPDATE_EVENT;
        let event_id =
            str_arg(args, "event_id").ok_or_else(|| invalid(tool, "`event_id` is required"))?;
        let patch = event_patch_from_args(tool, args)?;
        let data = store(&self.host).load().map_err(|e| tool_error(tool, e))?;
        let mut event = data
            .event(event_id)
            .map_err(|e| tool_error(tool, e))?
            .clone();
        let title = event.title.clone();
        let changes = apply_event_patch(&mut event, &patch).map_err(|e| tool_error(tool, e))?;
        if changes.is_empty() {
            return Err(ToolError::Failed(format!(
                "Event \"{title}\" already has those values; nothing to change."
            )));
        }
        Ok(ApprovalPreview {
            label: Some(format!("Update event “{title}”")),
            details: json!({ "event": title, "changes": changes_json(&changes) }),
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::UPDATE_EVENT;
        let event_id =
            str_arg(&args, "event_id").ok_or_else(|| invalid(tool, "`event_id` is required"))?;
        let patch = event_patch_from_args(tool, &args)?;
        let (before, after, changes) = store(&self.host)
            .update(|d| d.patch_event(event_id, &patch))
            .map_err(|e| tool_error(tool, e))?;
        if !changes.is_empty() {
            self.host
                .effects
                .calendar_changed(CalendarChange::EventSaved(after.clone()));
        }
        Ok(ToolOutput {
            text_for_model: if changes.is_empty() {
                format!(
                    "Event \"{}\" already had those values; nothing changed.",
                    after.title
                )
            } else {
                format!(
                    "Updated event \"{}\" (id {}): {}.",
                    before.title,
                    after.id,
                    describe_changes(&changes)
                )
            },
            summary_for_ui: if changes.is_empty() {
                format!("No change to “{}”", after.title)
            } else {
                format!(
                    "Updated “{}” ({})",
                    after.title,
                    changes
                        .iter()
                        .map(|c| c.field)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            detail: Some(
                json!({ "id": after.id, "title": after.title, "changes": changes_json(&changes) }),
            ),
        })
    }
}

// ── delete_task / delete_event ─────────────────────────────────────────────

pub struct DeleteTaskTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for DeleteTaskTool {
    fn name(&self) -> &'static str {
        app_tools::DELETE_TASK
    }
    fn label(&self) -> &'static str {
        "Delete task"
    }
    fn label_template(&self) -> &'static str {
        "Deleting a task"
    }
    fn description(&self) -> &'static str {
        "Delete a task (id from list_tasks) permanently, with its subtasks."
    }
    fn schema(&self) -> Value {
        task_id_schema()
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Destructive
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let tool = app_tools::DELETE_TASK;
        let task_id =
            str_arg(args, "task_id").ok_or_else(|| invalid(tool, "`task_id` is required"))?;
        let data = store(&self.host).load().map_err(|e| tool_error(tool, e))?;
        let task = data.task(task_id).map_err(|e| tool_error(tool, e))?;
        Ok(ApprovalPreview {
            label: Some(format!("Delete task “{}”", task.title)),
            details: json!({
                "title": task.title,
                "due": task.due_date,
                "status": task.status,
                "subtasks": task.subtasks.len(),
            }),
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::DELETE_TASK;
        let task_id =
            str_arg(&args, "task_id").ok_or_else(|| invalid(tool, "`task_id` is required"))?;
        let removed = store(&self.host)
            .update(|d| d.delete_task(task_id))
            .map_err(|e| tool_error(tool, e))?;
        self.host
            .effects
            .calendar_changed(CalendarChange::TaskRemoved(removed.id.clone()));
        Ok(ToolOutput {
            text_for_model: format!("Deleted task \"{}\" (id {}).", removed.title, removed.id),
            summary_for_ui: format!("Deleted “{}”", removed.title),
            detail: Some(json!({ "id": removed.id, "title": removed.title })),
        })
    }
}

pub struct DeleteEventTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for DeleteEventTool {
    fn name(&self) -> &'static str {
        app_tools::DELETE_EVENT
    }
    fn label(&self) -> &'static str {
        "Delete event"
    }
    fn label_template(&self) -> &'static str {
        "Deleting an event"
    }
    fn description(&self) -> &'static str {
        "Delete a calendar event (id from list_events) permanently."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "event_id": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "required": ["event_id"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Destructive
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let tool = app_tools::DELETE_EVENT;
        let event_id =
            str_arg(args, "event_id").ok_or_else(|| invalid(tool, "`event_id` is required"))?;
        let data = store(&self.host).load().map_err(|e| tool_error(tool, e))?;
        let event = data.event(event_id).map_err(|e| tool_error(tool, e))?;
        Ok(ApprovalPreview {
            label: Some(format!("Delete event “{}”", event.title)),
            details: json!({
                "title": event.title,
                "start": event.start_time,
                "end": event.end_time,
            }),
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::DELETE_EVENT;
        let event_id =
            str_arg(&args, "event_id").ok_or_else(|| invalid(tool, "`event_id` is required"))?;
        let removed = store(&self.host)
            .update(|d| d.delete_event(event_id))
            .map_err(|e| tool_error(tool, e))?;
        self.host
            .effects
            .calendar_changed(CalendarChange::EventRemoved(removed.id.clone()));
        Ok(ToolOutput {
            text_for_model: format!("Deleted event \"{}\" (id {}).", removed.title, removed.id),
            summary_for_ui: format!("Deleted “{}”", removed.title),
            detail: Some(json!({ "id": removed.id, "title": removed.title })),
        })
    }
}

// ── show_calendar ──────────────────────────────────────────────────────────

pub struct ShowCalendarTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for ShowCalendarTool {
    fn name(&self) -> &'static str {
        app_tools::SHOW_CALENDAR
    }
    fn label(&self) -> &'static str {
        "Show calendar"
    }
    fn label_template(&self) -> &'static str {
        "Opening the calendar[ on {date!}]"
    }
    fn description(&self) -> &'static str {
        "Open the Calendar view for the user at a date (YYYY-MM-DD), a task or an event (ids from \
         list_tasks / list_events). Give at least one."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "date": {"type": "string", "minLength": 10, "maxLength": 40},
                "task_id": {"type": "string", "minLength": 1, "maxLength": 200},
                "event_id": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::SHOW_CALENDAR;
        let mut date = date_arg(tool, &args, "date")?;
        let task_id = str_arg(&args, "task_id");
        let event_id = str_arg(&args, "event_id");
        if date.is_none() && task_id.is_none() && event_id.is_none() {
            return Err(invalid(tool, "give a date, a task_id or an event_id"));
        }
        let data = store(&self.host).load().map_err(|e| tool_error(tool, e))?;
        let mut shown = Vec::new();
        if let Some(id) = task_id {
            let task = data.task(id).map_err(|e| tool_error(tool, e))?;
            if date.is_none() {
                date = task
                    .due_date
                    .as_deref()
                    .and_then(parse_moment)
                    .map(Moment::date);
            }
            shown.push(format!("task “{}”", task.title));
        }
        if let Some(id) = event_id {
            let event = data.event(id).map_err(|e| tool_error(tool, e))?;
            if date.is_none() {
                date = parse_moment(&event.start_time).map(Moment::date);
            }
            shown.push(format!("event “{}”", event.title));
        }
        let date_text = date.map(|d| d.format("%Y-%m-%d").to_string());
        if let Some(d) = &date_text {
            shown.push(d.clone());
        }
        emit_navigation(
            ctx,
            "calendar",
            task_id.or(event_id).map(str::to_string),
            Some(NavigationTarget::Calendar {
                date: date_text.clone(),
                task_id: task_id.map(str::to_string),
                event_id: event_id.map(str::to_string),
            }),
        );
        let what = shown.join(", ");
        Ok(ToolOutput {
            text_for_model: format!("The calendar is now open at {what}."),
            summary_for_ui: format!("Opened the calendar at {what}"),
            detail: Some(json!({ "date": date_text, "taskId": task_id, "eventId": event_id })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use shodh_rag::harness::AgentEvent;

    async fn create(t: &testing::TestHost, title: &str, due: &str) -> String {
        let (ctx, _rx) = testing::ctx();
        let out = CreateTaskTool {
            host: t.host.clone(),
        }
        .execute(json!({"title": title, "due": due}), &ctx)
        .await
        .unwrap();
        out.detail.unwrap()["id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn list_tasks_returns_ids_and_honours_filters() {
        let t = testing::host().await;
        let a = create(&t, "File GST return", "2026-10-20").await;
        create(&t, "Renew lease", "2026-11-05").await;
        let (ctx, _rx) = testing::ctx();
        let tool = ListTasksTool {
            host: t.host.clone(),
        };
        let out = tool
            .execute(
                json!({"due_from": "2026-10-01", "due_to": "2026-10-31"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.summary_for_ui, "1 task");
        assert!(out.text_for_model.contains(&a));
        let all = tool.execute(json!({"limit": 1}), &ctx).await.unwrap();
        assert!(all.text_for_model.contains("Showing the first 1"));
        let bad = tool.execute(json!({"due_from": "soon"}), &ctx).await;
        assert!(matches!(bad, Err(ToolError::InvalidArguments { .. })));
        assert_eq!(t.effects.calendar.lock().unwrap().len(), 2, "two creations");
    }

    #[tokio::test]
    async fn update_preview_shows_before_and_after_and_execute_saves_it() {
        let t = testing::host().await;
        let id = create(&t, "Draft memo", "2026-10-20").await;
        let tool = UpdateTaskTool {
            host: t.host.clone(),
        };
        let args = json!({"task_id": id, "title": "Final memo", "due": null, "priority": "high"});
        let preview = tool.preview(&args).await.unwrap();
        assert_eq!(preview.label.as_deref(), Some("Update task “Draft memo”"));
        let changes = preview.details["changes"].as_array().unwrap();
        let fields: Vec<&str> = changes
            .iter()
            .map(|c| c["field"].as_str().unwrap())
            .collect();
        assert_eq!(fields, vec!["title", "due", "priority"]);
        assert_eq!(changes[0]["before"], "Draft memo");
        assert_eq!(changes[0]["after"], "Final memo");
        assert_eq!(changes[1]["after"], Value::Null);

        let (ctx, _rx) = testing::ctx();
        let out = tool.execute(args, &ctx).await.unwrap();
        assert_eq!(
            out.summary_for_ui,
            "Updated “Final memo” (title, due, priority)"
        );
        let stored = store(&t.host).load().unwrap();
        let task = stored.task(&id).unwrap();
        assert_eq!(task.title, "Final memo");
        assert_eq!(task.due_date, None);
        assert!(matches!(
            t.effects.calendar.lock().unwrap().last(),
            Some(CalendarChange::TaskSaved(saved)) if saved.title == "Final memo"
        ));
    }

    #[tokio::test]
    async fn update_rejects_empty_and_unknown_and_no_op_changes() {
        let t = testing::host().await;
        let id = create(&t, "Memo", "2026-10-20").await;
        let tool = UpdateTaskTool {
            host: t.host.clone(),
        };
        assert!(matches!(
            tool.preview(&json!({"task_id": id})).await,
            Err(ToolError::InvalidArguments { .. })
        ));
        assert!(matches!(
            tool.preview(&json!({"task_id": "missing", "title": "x"}))
                .await,
            Err(ToolError::NotFound(_))
        ));
        assert!(matches!(
            tool.preview(&json!({"task_id": id, "title": "Memo"})).await,
            Err(ToolError::Failed(_))
        ));
        assert!(matches!(
            tool.preview(&json!({"task_id": id, "due": "whenever"}))
                .await,
            Err(ToolError::InvalidArguments { .. })
        ));
    }

    #[tokio::test]
    async fn complete_and_delete_tasks() {
        let t = testing::host().await;
        let id = create(&t, "Pay invoice", "2026-10-20").await;
        let (ctx, _rx) = testing::ctx();
        let complete = CompleteTaskTool {
            host: t.host.clone(),
        };
        let preview = complete.preview(&json!({"task_id": id})).await.unwrap();
        assert_eq!(preview.details["changes"][0]["after"], "completed");
        complete
            .execute(json!({"task_id": id}), &ctx)
            .await
            .unwrap();
        assert!(complete.preview(&json!({"task_id": id})).await.is_err());

        let delete = DeleteTaskTool {
            host: t.host.clone(),
        };
        let preview = delete.preview(&json!({"task_id": id})).await.unwrap();
        assert_eq!(preview.label.as_deref(), Some("Delete task “Pay invoice”"));
        delete.execute(json!({"task_id": id}), &ctx).await.unwrap();
        assert!(store(&t.host).load().unwrap().tasks.is_empty());
        assert!(matches!(
            t.effects.calendar.lock().unwrap().last(),
            Some(CalendarChange::TaskRemoved(removed)) if removed == &id
        ));
    }

    #[tokio::test]
    async fn events_update_list_and_delete() {
        let t = testing::host().await;
        let (ctx, _rx) = testing::ctx();
        let created = CreateEventTool {
            host: t.host.clone(),
        }
        .execute(
            json!({"title": "Board meeting", "start": "2026-10-15T10:00"}),
            &ctx,
        )
        .await
        .unwrap();
        let id = created.detail.unwrap()["id"].as_str().unwrap().to_string();
        let update = UpdateEventTool {
            host: t.host.clone(),
        };
        let preview = update
            .preview(&json!({"event_id": id, "start": "2026-10-16"}))
            .await
            .unwrap();
        let fields: Vec<&str> = preview.details["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["field"].as_str().unwrap())
            .collect();
        assert_eq!(fields, vec!["start", "allDay"]);
        assert!(update
            .preview(
                &json!({"event_id": id, "start": "2026-10-16T10:00", "end": "2026-10-16T09:00"})
            )
            .await
            .is_err());
        update
            .execute(json!({"event_id": id, "start": "2026-10-16"}), &ctx)
            .await
            .unwrap();
        let listed = ListEventsTool {
            host: t.host.clone(),
        }
        .execute(json!({"from": "2026-10-16", "to": "2026-10-16"}), &ctx)
        .await
        .unwrap();
        assert_eq!(listed.summary_for_ui, "1 event");
        DeleteEventTool {
            host: t.host.clone(),
        }
        .execute(json!({"event_id": id}), &ctx)
        .await
        .unwrap();
        assert!(store(&t.host).load().unwrap().events.is_empty());
    }

    #[tokio::test]
    async fn show_calendar_checks_ids_and_navigates_to_the_due_date() {
        let t = testing::host().await;
        let id = create(&t, "Renew lease", "2026-11-05T17:00").await;
        let (ctx, mut rx) = testing::ctx();
        let tool = ShowCalendarTool {
            host: t.host.clone(),
        };
        assert!(tool.execute(json!({}), &ctx).await.is_err());
        assert!(matches!(
            tool.execute(json!({"task_id": "nope"}), &ctx).await,
            Err(ToolError::NotFound(_))
        ));
        tool.execute(json!({"task_id": id}), &ctx).await.unwrap();
        match rx.recv().await.unwrap() {
            AgentEvent::Navigated {
                view,
                focus,
                target,
                ..
            } => {
                assert_eq!(view, "calendar");
                assert_eq!(focus.as_deref(), Some(id.as_str()));
                assert_eq!(
                    target,
                    Some(NavigationTarget::Calendar {
                        date: Some("2026-11-05".into()),
                        task_id: Some(id.clone()),
                        event_id: None,
                    })
                );
            }
            other => panic!("{other:?}"),
        }
    }
}
