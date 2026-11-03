//! Calendar tools: create, list, update, complete and delete tasks and
//! events, and show a date, task or event in the Calendar view.
//!
//! Reads go straight to [`CalendarStore`]; every write goes through
//! `CalendarStore::update` and is reported to
//! [`super::HostEffects::calendar_changed`] (re-index + refresh). Update
//! previews compute the change with the same function the write uses, so
//! the approval card shows exactly the before → after that will be saved.
//!
//! A task may belong to a workspace (its project). The task tools take the
//! workspace by id or name (resolved like `resolve_sources` resolves sources:
//! nothing matching is an error that lists the valid workspaces), and a task
//! created in a workspace chat goes to that workspace unless the model says
//! otherwise.

use std::collections::HashMap;
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
use shodh_rag::workspaces::Workspace;

use super::{invalid, limit_arg, str_arg, AgentHost, CalendarChange};
use crate::calendar_store::{
    apply_event_patch, apply_task_patch, list_events, list_tasks, parse_moment, CalendarError,
    CalendarEvent, CalendarStore, EventFilter, EventPatch, FieldChange, Moment, NewEvent, NewTask,
    SubtaskSpec, TaskFilter, TaskPatch, TodoItem, PRIORITIES, STATUSES,
};

const DEFAULT_LIST: usize = 25;
const MAX_LIST: usize = 100;
/// Most workspaces listed in an error.
const MAX_LISTED_WORKSPACES: usize = 20;
/// How an approval card shows a task that belongs to no workspace.
const NO_WORKSPACE: &str = "No workspace";

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

/// Workspace names by id.
type WorkspaceNames = HashMap<String, String>;

fn task_line(task: &TodoItem, names: &WorkspaceNames) -> Value {
    let done = task.subtasks.iter().filter(|s| s.completed).count();
    let workspace = task
        .workspace_id
        .as_ref()
        .map(|id| json!({ "id": id, "name": names.get(id) }));
    json!({
        "id": task.id,
        "title": task.title,
        "due": task.due_date,
        "status": task.status,
        "priority": task.priority,
        "project": task.project,
        "workspace": workspace,
        "reminder": task.snoozed_until.as_ref().or(task.reminder.as_ref()),
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
        "location": event.location,
    })
}

// ── Workspaces of tasks ───────────────────────────────────────────────────

/// Every workspace, archived ones included (a task may stay in an archived
/// workspace).
async fn all_workspaces(host: &AgentHost) -> Result<Vec<Workspace>, ToolError> {
    host.workspaces
        .run(|s| s.list(true))
        .await
        .map_err(|e| match e.code {
            "unavailable" => ToolError::Unavailable(e.message),
            _ => ToolError::Failed(e.message),
        })
}

fn workspace_names(workspaces: &[Workspace]) -> WorkspaceNames {
    workspaces
        .iter()
        .map(|w| (w.id.clone(), w.name.clone()))
        .collect()
}

/// The valid workspaces, for an error.
fn valid_workspaces_text(workspaces: &[Workspace]) -> String {
    if workspaces.is_empty() {
        return "The user has no workspaces; leave workspace out.".to_string();
    }
    let listed: Vec<String> = workspaces
        .iter()
        .take(MAX_LISTED_WORKSPACES)
        .map(|w| {
            let archived = if w.archived { ", archived" } else { "" };
            format!("\"{}\" (id {}{archived})", w.name, w.id)
        })
        .collect();
    let more = workspaces.len().saturating_sub(MAX_LISTED_WORKSPACES);
    let more = if more > 0 {
        format!(" and {more} more (call list_workspaces)")
    } else {
        String::new()
    };
    format!("Valid workspaces: {}{more}.", listed.join(", "))
}

/// The workspace `value` names: its id, or its name (ignoring case). A value
/// that matches nothing, or a name several workspaces share, is an error that
/// lists the candidates; nothing is guessed.
fn resolve_workspace<'a>(
    workspaces: &'a [Workspace],
    value: &str,
) -> Result<&'a Workspace, ToolError> {
    let value = value.trim();
    if let Some(w) = workspaces.iter().find(|w| w.id == value) {
        return Ok(w);
    }
    let lower = value.to_lowercase();
    if let Some(w) = workspaces.iter().find(|w| w.id.to_lowercase() == lower) {
        return Ok(w);
    }
    let named: Vec<&Workspace> = workspaces
        .iter()
        .filter(|w| w.name.trim().to_lowercase() == lower)
        .collect();
    match named.as_slice() {
        [one] => Ok(*one),
        [] => Err(ToolError::NotFound(format!(
            "No workspace matches \"{value}\"; workspace takes a workspace id or name. {}",
            valid_workspaces_text(workspaces)
        ))),
        several => Err(ToolError::NotFound(format!(
            "\"{value}\" names more than one workspace: {}; pass its id instead.",
            several
                .iter()
                .map(|w| format!("\"{}\" (id {})", w.name, w.id))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// The `workspace` argument: `None` when absent, `Some(None)` for `null`
/// (no workspace), else the workspace it names.
async fn workspace_arg(
    host: &AgentHost,
    tool: &str,
    args: &Value,
) -> Result<Option<Option<Workspace>>, ToolError> {
    match nullable_arg(args, "workspace") {
        None => Ok(None),
        Some(None) => Ok(Some(None)),
        Some(Some(value)) if value.is_empty() => Err(invalid(
            tool,
            "`workspace` is empty: give a workspace id or name, or null for no workspace",
        )),
        Some(Some(value)) => {
            let workspaces = all_workspaces(host).await?;
            let found = resolve_workspace(&workspaces, &value)?;
            Ok(Some(Some(found.clone())))
        }
    }
}

/// The workspace a new task is saved in, as `(id, name)`: the one the call
/// names (`null` for none), else the chat's workspace (the app sets the scope
/// from the conversation's workspace). The preview and the write both call
/// this, so the approval card names the workspace the task lands in.
async fn new_task_workspace(
    host: &AgentHost,
    tool: &str,
    args: &Value,
    ctx: &ToolContext,
) -> Result<Option<(String, String)>, ToolError> {
    if let Some(chosen) = workspace_arg(host, tool, args).await? {
        return Ok(chosen.map(|w| (w.id, w.name)));
    }
    let Some(id) = ctx.scope().workspace.clone() else {
        return Ok(None);
    };
    let name = match ctx.scope().workspace_name.clone() {
        Some(name) => name,
        // The name is only for display: when the scope lacks it and the
        // workspace list cannot be read, the id still identifies it.
        None => all_workspaces(host)
            .await
            .ok()
            .and_then(|all| all.into_iter().find(|w| w.id == id).map(|w| w.name))
            .unwrap_or_else(|| id.clone()),
    };
    Ok(Some((id, name)))
}

/// Show workspace changes by name (the approval card is read by the user).
fn name_workspace_changes(changes: &mut [FieldChange], names: &WorkspaceNames) {
    for change in changes.iter_mut().filter(|c| c.field == "workspace") {
        for value in [&mut change.before, &mut change.after] {
            if let Some(name) = value.as_str().and_then(|id| names.get(id)) {
                *value = json!(name);
            }
        }
    }
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
        "Create a calendar task (a to-do with an optional due date, priority and reminder). \
         reminder is when to notify the user: a local date-time YYYY-MM-DDTHH:MM (or an RFC 3339 \
         timestamp), e.g. an hour before the due time. source_ref can hold the file the task came \
         from, e.g. a contract path. workspace files the task under a workspace (its id or name \
         from list_workspaces); in a workspace chat it defaults to that workspace, and null files \
         it under no workspace."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "minLength": 1, "maxLength": 200},
                "due": {"type": "string", "minLength": 10, "maxLength": 40},
                "priority": {"type": "string", "enum": PRIORITIES},
                "notes": {"type": "string", "maxLength": 4000},
                "reminder": {"type": "string", "minLength": 16, "maxLength": 40},
                "source_ref": {"type": "string", "minLength": 1, "maxLength": 1024},
                "workspace": {"type": ["string", "null"], "minLength": 1, "maxLength": 200}
            },
            "required": ["title"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    /// The card shows the workspace the task will be saved in, decided by
    /// the same function the write uses: the one the call names, else the
    /// chat's workspace.
    async fn preview_in(
        &self,
        args: &Value,
        ctx: &ToolContext,
    ) -> Result<ApprovalPreview, ToolError> {
        let workspace = new_task_workspace(&self.host, app_tools::CREATE_TASK, args, ctx).await?;
        Ok(ApprovalPreview {
            label: None,
            details: json!({
                "title": str_arg(args, "title"),
                "due": str_arg(args, "due"),
                "priority": str_arg(args, "priority"),
                "notes": str_arg(args, "notes"),
                "reminder": str_arg(args, "reminder"),
                "sourceRef": str_arg(args, "source_ref"),
                "workspace": workspace
                    .map(|(_, name)| name)
                    .unwrap_or_else(|| NO_WORKSPACE.to_string()),
            }),
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::CREATE_TASK;
        let title = str_arg(&args, "title").ok_or_else(|| invalid(tool, "`title` is required"))?;
        let workspace = new_task_workspace(&self.host, tool, &args, ctx).await?;
        let new = NewTask {
            title: title.to_string(),
            due_date: str_arg(&args, "due").map(str::to_string),
            priority: str_arg(&args, "priority").map(str::to_string),
            description: str_arg(&args, "notes").map(str::to_string),
            reminder: str_arg(&args, "reminder").map(str::to_string),
            source: Some("agent".to_string()),
            source_ref: str_arg(&args, "source_ref").map(str::to_string),
            workspace_id: workspace.as_ref().map(|(id, _)| id.clone()),
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
        let in_workspace = workspace
            .as_ref()
            .map(|(_, name)| format!(" in workspace “{name}”"))
            .unwrap_or_default();
        Ok(ToolOutput {
            text_for_model: format!(
                "Created task \"{}\" (id {}{due_text}){in_workspace}. Use show_calendar with this task_id to show it.",
                task.title, task.id
            ),
            summary_for_ui: format!("Created task “{}”{due_text}{in_workspace}", task.title),
            detail: Some(json!({
                "id": task.id,
                "title": task.title,
                "due": task.due_date,
                "workspaceId": task.workspace_id,
            })),
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
        "Create a calendar event, optionally with a location. A date without a time makes an \
         all-day event."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "minLength": 1, "maxLength": 200},
                "start": {"type": "string", "minLength": 10, "maxLength": 40},
                "end": {"type": "string", "minLength": 10, "maxLength": 40},
                "notes": {"type": "string", "maxLength": 4000},
                "location": {"type": "string", "minLength": 1, "maxLength": 500}
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
                "location": str_arg(args, "location"),
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
            location: str_arg(&args, "location").map(str::to_string),
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
                "allDay": event.all_day,
                "location": event.location
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
        "Listing[ {status!}] tasks[ in {workspace}][ due from {due_from!}][ due by {due_to!}][ matching {text}]"
    }
    fn description(&self) -> &'static str {
        "List the user's tasks (to-dos) with their ids, titles, due dates, status and priority. \
         Call it for every question about the user's tasks (what they have, what is open, due, \
         overdue or done): the tasks are not in the indexed documents. Without filters it lists \
         all tasks (up to limit) and says how many there are. Filter by status, an inclusive due-date range (YYYY-MM-DD; tasks without a \
         due date are left out when a range is given), text in the title, notes, tags or \
         project, and workspace (an id or name: only that workspace's tasks). Also use it to \
         find a task's id before changing it, and to check your own changes."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {"type": "string", "enum": STATUSES},
                "due_from": {"type": "string", "minLength": 10, "maxLength": 40},
                "due_to": {"type": "string", "minLength": 10, "maxLength": 40},
                "text": {"type": "string", "minLength": 1, "maxLength": 200},
                "workspace": {"type": "string", "minLength": 1, "maxLength": 200},
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
        let (workspace_id, mut names) = match str_arg(&args, "workspace") {
            Some(value) => {
                let workspaces = all_workspaces(&self.host).await?;
                let id = resolve_workspace(&workspaces, value)?.id.clone();
                (Some(id), Some(workspace_names(&workspaces)))
            }
            None => (None, None),
        };
        let filter = TaskFilter {
            status: str_arg(&args, "status").map(str::to_string),
            due_from: date_arg(tool, &args, "due_from")?,
            due_to: date_arg(tool, &args, "due_to")?,
            text: str_arg(&args, "text").map(str::to_string),
            workspace_id,
        };
        let limit = limit_arg(&args, DEFAULT_LIST, MAX_LIST);
        let data = store(&self.host).load().map_err(|e| tool_error(tool, e))?;
        let matched = list_tasks(&data, &filter);
        let total = matched.len();
        if names.is_none() && matched.iter().any(|t| t.workspace_id.is_some()) {
            // Names are a convenience: without the workspace store the ids still show.
            names = all_workspaces(&self.host)
                .await
                .ok()
                .map(|w| workspace_names(&w));
        }
        let names = names.unwrap_or_default();
        let shown: Vec<Value> = matched
            .iter()
            .take(limit)
            .map(|t| task_line(t, &names))
            .collect();
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
        "List the user's calendar events (meetings, appointments) with their ids, titles and \
         times. Call it for every question about the user's events or schedule: the events are \
         not in the indexed documents. Filter by an inclusive date range (YYYY-MM-DD; events \
         overlapping it are included) and text in the title or notes."
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

/// A nullable string argument: absent keeps the field, `null` clears it.
fn nullable_arg(args: &Value, key: &str) -> Option<Option<String>> {
    match args.get(key) {
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.trim().to_string())),
        _ => None,
    }
}

async fn task_patch_from_args(
    host: &AgentHost,
    tool: &str,
    args: &Value,
) -> Result<TaskPatch, ToolError> {
    let due_date = nullable_arg(args, "due");
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
        project: nullable_arg(args, "project"),
        workspace_id: workspace_arg(host, tool, args)
            .await?
            .map(|chosen| chosen.map(|w| w.id)),
        reminder: nullable_arg(args, "reminder"),
        subtasks,
        ..TaskPatch::default()
    };
    if patch.is_empty() {
        return Err(invalid(
            tool,
            "give at least one field to change: title, notes, due, priority, status, project, \
             workspace, reminder or subtasks",
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
        "Change a task (id from list_tasks): title, notes (\"\" clears them), due date, priority, \
         status, project, workspace (an id or name from list_workspaces), reminder, or the full \
         subtask list (keep a subtask's id to keep it; omit the id to add one; leave one out to \
         remove it). null clears due, project, workspace or reminder. \
         reminder is when to notify the user, a local date-time YYYY-MM-DDTHH:MM (or RFC 3339); \
         setting it again re-arms a reminder that already rang. Only the fields you pass change."
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
                "project": {"type": ["string", "null"], "maxLength": 200},
                "workspace": {"type": ["string", "null"], "minLength": 1, "maxLength": 200},
                "reminder": {"type": ["string", "null"], "maxLength": 40},
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
        let patch = task_patch_from_args(&self.host, tool, args).await?;
        let (before, mut changes) = preview_task_patch(&self.host, tool, task_id, &patch)?;
        if changes.iter().any(|c| c.field == "workspace") {
            // Names read better than ids; the ids stay when the store is unavailable.
            if let Ok(workspaces) = all_workspaces(&self.host).await {
                name_workspace_changes(&mut changes, &workspace_names(&workspaces));
            }
        }
        Ok(ApprovalPreview {
            label: Some(format!("Update task “{}”", before.title)),
            details: json!({ "task": before.title, "changes": changes_json(&changes) }),
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::UPDATE_TASK;
        let task_id =
            str_arg(&args, "task_id").ok_or_else(|| invalid(tool, "`task_id` is required"))?;
        let patch = task_patch_from_args(&self.host, tool, &args).await?;
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
    let end_time = nullable_arg(args, "end");
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
        location: nullable_arg(args, "location"),
    };
    if patch.is_empty() {
        return Err(invalid(
            tool,
            "give at least one field to change: title, notes, start, end, all_day or location",
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
        "Change an event (id from list_events): title, notes (\"\" clears them), start, end, \
         all_day or location; null clears end or location. A start given as a date alone makes \
         it all-day. Only the fields you pass change."
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
                "all_day": {"type": "boolean"},
                "location": {"type": ["string", "null"], "maxLength": 500}
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
        "Open the tasks view (the calendar) for the user at a date (YYYY-MM-DD), a task or an event (ids from \
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
            "tasks",
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
    use shodh_rag::harness::tools::RunScope;
    use shodh_rag::harness::AgentEvent;
    use shodh_rag::workspaces::{NewWorkspace, WorkspaceAuthor};

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
    async fn reminders_projects_and_locations_can_be_set_and_cleared() {
        let t = testing::host().await;
        let (ctx, _rx) = testing::ctx();
        let created = CreateTaskTool {
            host: t.host.clone(),
        }
        .execute(
            json!({"title": "File ITR", "due": "2026-10-30", "reminder": "2026-10-29T09:00"}),
            &ctx,
        )
        .await
        .unwrap();
        let id = created.detail.unwrap()["id"].as_str().unwrap().to_string();
        let stored = store(&t.host).load().unwrap();
        assert_eq!(
            stored.task(&id).unwrap().reminder.as_deref(),
            Some("2026-10-29T09:00")
        );
        let bad = CreateTaskTool {
            host: t.host.clone(),
        }
        .execute(json!({"title": "x", "reminder": "2026-10-29"}), &ctx)
        .await;
        assert!(
            matches!(bad, Err(ToolError::InvalidArguments { .. })),
            "a reminder needs a time"
        );

        let update = UpdateTaskTool {
            host: t.host.clone(),
        };
        update
            .execute(json!({"task_id": id, "project": "Tax"}), &ctx)
            .await
            .unwrap();
        let preview = update
            .preview(&json!({"task_id": id, "project": null, "reminder": null}))
            .await
            .unwrap();
        let fields: Vec<&str> = preview.details["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["field"].as_str().unwrap())
            .collect();
        assert_eq!(fields, vec!["project", "reminder"]);
        update
            .execute(
                json!({"task_id": id, "project": null, "reminder": null}),
                &ctx,
            )
            .await
            .unwrap();
        let task = store(&t.host).load().unwrap().task(&id).unwrap().clone();
        assert_eq!(task.project, None);
        assert_eq!(task.reminder, None);

        let event = CreateEventTool {
            host: t.host.clone(),
        }
        .execute(
            json!({"title": "Review", "start": "2026-10-15T10:00", "location": "Room 4B"}),
            &ctx,
        )
        .await
        .unwrap();
        let event_id = event.detail.unwrap()["id"].as_str().unwrap().to_string();
        let listed = ListEventsTool {
            host: t.host.clone(),
        }
        .execute(json!({"text": "room 4b"}), &ctx)
        .await
        .unwrap();
        assert!(listed.text_for_model.contains("Room 4B"));
        UpdateEventTool {
            host: t.host.clone(),
        }
        .execute(json!({"event_id": event_id, "location": null}), &ctx)
        .await
        .unwrap();
        assert_eq!(
            store(&t.host)
                .load()
                .unwrap()
                .event(&event_id)
                .unwrap()
                .location,
            None
        );
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
                assert_eq!(view, "tasks");
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

    // ── Workspaces ─────────────────────────────────────────────────────────

    async fn workspace(t: &testing::TestHost, name: &str) -> Workspace {
        let new = NewWorkspace {
            name: name.to_string(),
            ..NewWorkspace::default()
        };
        t.host
            .workspaces
            .run(move |s| s.create(&new, WorkspaceAuthor::User))
            .await
            .unwrap()
    }

    fn listed(id: &str, name: &str) -> Workspace {
        Workspace {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            icon: "folder".to_string(),
            color: "neutral".to_string(),
            template: "blank".to_string(),
            pinned: false,
            archived: false,
            created_at: String::new(),
            updated_at: String::new(),
            last_active_at: None,
            instructions_version: 0,
            source_counts: Default::default(),
        }
    }

    fn stored_workspace(t: &testing::TestHost, task_id: &str) -> Option<String> {
        store(&t.host)
            .load()
            .unwrap()
            .task(task_id)
            .unwrap()
            .workspace_id
            .clone()
    }

    #[test]
    fn workspaces_resolve_by_id_or_name_and_unknown_ones_list_the_valid_ones() {
        let all = vec![
            listed("ws-1", "Thesis"),
            listed("ws-2", "Tax 2026"),
            listed("ws-3", "Reading"),
            listed("ws-4", "reading"),
        ];
        assert_eq!(resolve_workspace(&all, "ws-2").unwrap().id, "ws-2");
        assert_eq!(resolve_workspace(&all, " WS-1 ").unwrap().id, "ws-1");
        assert_eq!(resolve_workspace(&all, "thesis").unwrap().id, "ws-1");
        assert_eq!(resolve_workspace(&all, "TAX 2026").unwrap().id, "ws-2");
        match resolve_workspace(&all, "Reading") {
            Err(ToolError::NotFound(message)) => {
                assert!(message.contains("more than one"), "{message}");
                assert!(
                    message.contains("ws-3") && message.contains("ws-4"),
                    "{message}"
                );
            }
            other => panic!("{other:?}"),
        }
        match resolve_workspace(&all, "Groceries") {
            Err(ToolError::NotFound(message)) => {
                assert!(
                    message.contains("No workspace matches \"Groceries\""),
                    "{message}"
                );
                assert!(message.contains("\"Thesis\" (id ws-1)"), "{message}");
                assert!(message.contains("\"Tax 2026\" (id ws-2)"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        match resolve_workspace(&[], "Thesis") {
            Err(ToolError::NotFound(message)) => {
                assert!(message.contains("no workspaces"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn tasks_take_a_workspace_by_name_or_id_and_list_by_it() {
        let t = testing::host().await;
        let thesis = workspace(&t, "Thesis").await;
        let tax = workspace(&t, "Tax").await;
        let (ctx, _rx) = testing::ctx();
        let create = CreateTaskTool {
            host: t.host.clone(),
        };
        let out = create
            .execute(json!({"title": "Read paper", "workspace": "thesis"}), &ctx)
            .await
            .unwrap();
        let read = out.detail.unwrap()["id"].as_str().unwrap().to_string();
        assert_eq!(stored_workspace(&t, &read), Some(thesis.id.clone()));
        assert!(out.text_for_model.contains("in workspace “Thesis”"));
        let out = create
            .execute(json!({"title": "File return", "workspace": tax.id}), &ctx)
            .await
            .unwrap();
        let file = out.detail.unwrap()["id"].as_str().unwrap().to_string();
        assert_eq!(stored_workspace(&t, &file), Some(tax.id.clone()));

        let unknown = create
            .execute(json!({"title": "x", "workspace": "Groceries"}), &ctx)
            .await;
        match unknown {
            Err(ToolError::NotFound(message)) => {
                assert!(
                    message.contains("Thesis") && message.contains("Tax"),
                    "{message}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            store(&t.host).load().unwrap().tasks.len(),
            2,
            "nothing created"
        );

        let list = ListTasksTool {
            host: t.host.clone(),
        };
        let out = list
            .execute(json!({"workspace": "Thesis"}), &ctx)
            .await
            .unwrap();
        assert_eq!(out.summary_for_ui, "1 task");
        let rows = &out.detail.unwrap()["tasks"];
        assert_eq!(rows[0]["id"], read.as_str());
        assert_eq!(rows[0]["workspace"]["name"], "Thesis");
        assert!(matches!(
            list.execute(json!({"workspace": "nope"}), &ctx).await,
            Err(ToolError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn tasks_created_in_a_workspace_chat_go_to_that_workspace() {
        let t = testing::host().await;
        let thesis = workspace(&t, "Thesis").await;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = ToolContext::new("run-1", "step-1", tx).with_scope(Arc::new(RunScope {
            workspace: Some(thesis.id.clone()),
            workspace_name: Some(thesis.name.clone()),
            ..RunScope::default()
        }));
        let create = CreateTaskTool {
            host: t.host.clone(),
        };
        let out = create
            .execute(json!({"title": "Outline chapter 2"}), &ctx)
            .await
            .unwrap();
        let id = out.detail.unwrap()["id"].as_str().unwrap().to_string();
        assert_eq!(stored_workspace(&t, &id), Some(thesis.id.clone()));
        let out = create
            .execute(json!({"title": "Buy milk", "workspace": null}), &ctx)
            .await
            .unwrap();
        let id = out.detail.unwrap()["id"].as_str().unwrap().to_string();
        assert_eq!(stored_workspace(&t, &id), None, "null means no workspace");

        let (plain, _rx) = testing::ctx();
        let out = create
            .execute(json!({"title": "Outside any workspace"}), &plain)
            .await
            .unwrap();
        let id = out.detail.unwrap()["id"].as_str().unwrap().to_string();
        assert_eq!(stored_workspace(&t, &id), None);
    }

    #[tokio::test]
    async fn create_task_preview_names_the_workspace_the_task_lands_in() {
        let t = testing::host().await;
        let thesis = workspace(&t, "Thesis").await;
        workspace(&t, "Tax").await;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let in_thesis = ToolContext::new("run-1", "step-1", tx).with_scope(Arc::new(RunScope {
            workspace: Some(thesis.id.clone()),
            workspace_name: Some(thesis.name.clone()),
            ..RunScope::default()
        }));
        let create = CreateTaskTool {
            host: t.host.clone(),
        };
        let shown = |preview: ApprovalPreview| preview.details["workspace"].clone();

        // The chat's workspace is the default.
        let preview = create
            .preview_in(&json!({"title": "Outline chapter 2"}), &in_thesis)
            .await
            .unwrap();
        assert_eq!(shown(preview), "Thesis");
        // null files the task under no workspace, even in a workspace chat.
        let preview = create
            .preview_in(&json!({"title": "Buy milk", "workspace": null}), &in_thesis)
            .await
            .unwrap();
        assert_eq!(shown(preview), "No workspace");
        // A workspace the call names wins over the chat's, shown by name.
        let preview = create
            .preview_in(
                &json!({"title": "File return", "workspace": "tax"}),
                &in_thesis,
            )
            .await
            .unwrap();
        assert_eq!(shown(preview), "Tax");
        // Outside a workspace chat, nothing named: no workspace.
        let (plain, _rx) = testing::ctx();
        let preview = create
            .preview_in(&json!({"title": "Outside any workspace"}), &plain)
            .await
            .unwrap();
        assert_eq!(shown(preview), "No workspace");

        // A scope without the name still shows the name, looked up by id.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let unnamed = ToolContext::new("run-2", "step-1", tx).with_scope(Arc::new(RunScope {
            workspace: Some(thesis.id.clone()),
            ..RunScope::default()
        }));
        let preview = create
            .preview_in(&json!({"title": "Read paper"}), &unnamed)
            .await
            .unwrap();
        assert_eq!(shown(preview), "Thesis");

        // An unknown workspace fails before the user is asked.
        assert!(matches!(
            create
                .preview_in(&json!({"title": "x", "workspace": "Groceries"}), &in_thesis)
                .await,
            Err(ToolError::NotFound(_))
        ));
        assert!(
            store(&t.host).load().unwrap().tasks.is_empty(),
            "a preview saves nothing"
        );
    }

    #[tokio::test]
    async fn update_task_moves_and_clears_the_workspace_and_previews_names() {
        let t = testing::host().await;
        let thesis = workspace(&t, "Thesis").await;
        let tax = workspace(&t, "Tax").await;
        let id = create(&t, "Read paper", "2026-10-20").await;
        let (ctx, _rx) = testing::ctx();
        let update = UpdateTaskTool {
            host: t.host.clone(),
        };
        update
            .execute(json!({"task_id": id, "workspace": "Thesis"}), &ctx)
            .await
            .unwrap();
        assert_eq!(stored_workspace(&t, &id), Some(thesis.id.clone()));

        let preview = update
            .preview(&json!({"task_id": id, "workspace": "tax"}))
            .await
            .unwrap();
        let change = &preview.details["changes"][0];
        assert_eq!(change["field"], "workspace");
        assert_eq!(change["before"], "Thesis");
        assert_eq!(change["after"], "Tax");
        update
            .execute(json!({"task_id": id, "workspace": tax.id}), &ctx)
            .await
            .unwrap();
        assert_eq!(stored_workspace(&t, &id), Some(tax.id.clone()));

        assert!(matches!(
            update
                .preview(&json!({"task_id": id, "workspace": "Groceries"}))
                .await,
            Err(ToolError::NotFound(_))
        ));
        update
            .execute(json!({"task_id": id, "workspace": null}), &ctx)
            .await
            .unwrap();
        let task = store(&t.host).load().unwrap().task(&id).unwrap().clone();
        assert_eq!(task.workspace_id, None);
        assert_eq!(task.title, "Read paper", "the task is kept");
    }
}
