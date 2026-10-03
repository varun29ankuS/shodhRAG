//! Calendar storage: tasks and events in `<app_data_dir>/calendar_data.json`.
//!
//! Every change is a read-modify-write of the whole file under one
//! process-wide lock, written atomically (temp file + rename), so the UI
//! commands and the agent's tools never lose each other's updates. The
//! operations here are pure data changes; callers report each change to
//! [`crate::agent_tools::HostEffects::calendar_changed`], which re-indexes
//! the record and tells open views to refresh.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

pub const CALENDAR_FILE: &str = "calendar_data.json";

/// Task priorities the calendar shows.
pub const PRIORITIES: [&str; 3] = ["low", "medium", "high"];

/// Task states the calendar shows.
pub const STATUSES: [&str; 3] = ["pending", "in_progress", "completed"];

/// Serialises every read-modify-write of the calendar file.
static CALENDAR_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("Task not found: {0}")]
    TaskNotFound(String),
    #[error("Event not found: {0}")]
    EventNotFound(String),
    #[error("Subtask not found: {0}")]
    SubtaskNotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("Calendar file error ({path}): {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Calendar data in {path} is not valid: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
}

pub type CalendarResult<T> = Result<T, CalendarError>;

// ── Records ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubTask {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// When to remind: an absolute local date-time `YYYY-MM-DDTHH:MM`
    /// (see [`check_reminder`]). The UI offers presets relative to the due
    /// time; what is stored is always the absolute moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder: Option<String>,
    /// When the reminder was shown (RFC 3339, UTC). Set once per reminder
    /// or snooze, so a reminder never fires twice; cleared when the
    /// reminder changes or is snoozed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder_fired_at: Option<String>,
    /// A snoozed reminder fires again at this local date-time
    /// (`YYYY-MM-DDTHH:MM`) instead of at `reminder`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Where the event takes place (free text: a room, an address, a link).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
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
    "user".to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CalendarData {
    #[serde(default)]
    pub tasks: Vec<TodoItem>,
    #[serde(default)]
    pub events: Vec<CalendarEvent>,
}

// ── Moments ──────────────────────────────────────────────────────

/// A calendar moment: a date (all-day) or a date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Moment {
    Date(NaiveDate),
    DateTime(NaiveDateTime),
}

impl Moment {
    pub fn as_datetime(self) -> NaiveDateTime {
        match self {
            Moment::Date(d) => d.and_hms_opt(0, 0, 0).unwrap_or_default(),
            Moment::DateTime(dt) => dt,
        }
    }

    pub fn date(self) -> NaiveDate {
        match self {
            Moment::Date(d) => d,
            Moment::DateTime(dt) => dt.date(),
        }
    }
}

/// Accept `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM[:SS]` or RFC 3339.
pub fn parse_moment(value: &str) -> Option<Moment> {
    let value = value.trim();
    if let Ok(d) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some(Moment::Date(d));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Some(Moment::DateTime(dt.naive_local()));
    }
    [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ]
    .iter()
    .find_map(|f| NaiveDateTime::parse_from_str(value, f).ok())
    .map(Moment::DateTime)
}

pub const MOMENT_HINT: &str = "use YYYY-MM-DD, YYYY-MM-DDTHH:MM or an RFC 3339 timestamp";

// ── Reminders ────────────────────────────────────────────────────

/// Stored shape of `reminder` and `snoozed_until`: a local wall-clock
/// date-time to the minute. Local, not UTC, so "remind me at 9:00" stays at
/// 9:00 across daylight-saving changes and when the computer changes time
/// zone; the scheduler converts it to an instant each time it plans.
pub const REMINDER_FORMAT: &str = "%Y-%m-%dT%H:%M";

pub const REMINDER_HINT: &str =
    "a reminder needs a date and a time: use YYYY-MM-DDTHH:MM (local time) or an RFC 3339 timestamp";

/// A stored reminder value as a local date-time, if it is one. Values
/// written before reminders were validated may be anything; those read as
/// `None` and never ring.
pub fn parse_reminder(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value.trim(), REMINDER_FORMAT)
        .ok()
        .or_else(|| match parse_moment(value)? {
            Moment::DateTime(dt) => Some(dt),
            Moment::Date(_) => None,
        })
}

/// Normalise a reminder to [`REMINDER_FORMAT`] in `tz`'s wall-clock time.
/// A timestamp with an offset (`…T09:00Z`) is converted to `tz`, so it rings
/// at that instant; a plain date-time is taken as already local. A date
/// without a time is rejected: a reminder must say when to ring.
pub fn normalize_reminder_in<Tz: chrono::TimeZone>(value: &str, tz: &Tz) -> CalendarResult<String> {
    let value = value.trim();
    let invalid = || CalendarError::Invalid(format!("`reminder` {value:?}: {REMINDER_HINT}"));
    let local = match DateTime::parse_from_rfc3339(value) {
        Ok(instant) => instant.with_timezone(tz).naive_local(),
        Err(_) => match parse_moment(value) {
            Some(Moment::DateTime(dt)) => dt,
            _ => return Err(invalid()),
        },
    };
    Ok(local.format(REMINDER_FORMAT).to_string())
}

/// [`normalize_reminder_in`] for this computer's time zone.
pub fn check_reminder(value: &str) -> CalendarResult<String> {
    normalize_reminder_in(value, &chrono::Local)
}

// ── Clearing optional fields ─────────────────────────────────────

/// The optional fields of a task an update may clear. Absent from an
/// update means "keep"; naming a field in `clear` is the only way to empty
/// it, so a client that omits a value can never erase it by accident.
pub const TASK_CLEARABLE: [&str; 4] = ["due_date", "project", "description", "reminder"];

/// The optional fields of an event an update may clear.
pub const EVENT_CLEARABLE: [&str; 3] = ["description", "end_time", "location"];

/// Validate a `clear` list against `allowed`; returns the names, deduplicated.
fn clear_names<'a>(
    clear: &'a [String],
    allowed: &[&str],
    what: &str,
) -> CalendarResult<Vec<&'a str>> {
    let mut out: Vec<&str> = Vec::new();
    for name in clear {
        let name = name.trim();
        if !allowed.contains(&name) {
            return Err(CalendarError::Invalid(format!(
                "`{name}` cannot be cleared on {what}; clearable fields: {}",
                allowed.join(", ")
            )));
        }
        if !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

fn set_and_cleared(field: &str) -> CalendarError {
    CalendarError::Invalid(format!(
        "`{field}` is both set and cleared in the same update"
    ))
}

/// `slot` cleared, unless the update also sets it.
fn clear_slot(slot: &mut Option<Option<String>>, field: &str) -> CalendarResult<()> {
    if matches!(slot, Some(Some(_))) {
        return Err(set_and_cleared(field));
    }
    *slot = Some(None);
    Ok(())
}

/// Notes cleared (set to empty), unless the update also sets them.
fn clear_text(slot: &mut Option<String>, field: &str) -> CalendarResult<()> {
    if slot.as_deref().is_some_and(|d| !d.is_empty()) {
        return Err(set_and_cleared(field));
    }
    *slot = Some(String::new());
    Ok(())
}

/// An optional text set to `value`: blank text means "none".
fn optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

// ── File access ──────────────────────────────────────────────────

/// The calendar file of one app data directory.
#[derive(Debug, Clone)]
pub struct CalendarStore {
    path: PathBuf,
}

impl CalendarStore {
    pub fn in_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(CALENDAR_FILE),
        }
    }

    fn read_unlocked(&self) -> CalendarResult<CalendarData> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map_err(|source| CalendarError::Parse {
                path: self.path.clone(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CalendarData::default()),
            Err(source) => Err(CalendarError::Io {
                path: self.path.clone(),
                source,
            }),
        }
    }

    fn write_unlocked(&self, data: &CalendarData) -> CalendarResult<()> {
        let io = |source| CalendarError::Io {
            path: self.path.clone(),
            source,
        };
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(io)?;
        }
        let text = serde_json::to_string_pretty(data).map_err(|source| CalendarError::Parse {
            path: self.path.clone(),
            source,
        })?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, text).map_err(io)?;
        fs::rename(&tmp, &self.path).map_err(io)
    }

    /// Current tasks and events.
    pub fn load(&self) -> CalendarResult<CalendarData> {
        let _guard = CALENDAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        self.read_unlocked()
    }

    /// Apply `change` to the stored data and save it. Nothing is written
    /// when `change` fails.
    pub fn update<R>(
        &self,
        change: impl FnOnce(&mut CalendarData) -> CalendarResult<R>,
    ) -> CalendarResult<R> {
        let _guard = CALENDAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = self.read_unlocked()?;
        let result = change(&mut data)?;
        self.write_unlocked(&data)?;
        Ok(result)
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

// ── Tasks ────────────────────────────────────────────────────────

/// Fields for a new task. Shared by the `create_task` command and the
/// agent's `create_task` tool.
#[derive(Debug, Clone, Default)]
pub struct NewTask {
    pub title: String,
    pub description: Option<String>,
    pub due_date: Option<String>,
    pub priority: Option<String>,
    pub tags: Option<Vec<String>>,
    pub project: Option<String>,
    pub source: Option<String>,
    pub source_ref: Option<String>,
    pub reminder: Option<String>,
}

/// A subtask in a full replacement list: an existing id keeps its identity.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtaskSpec {
    pub id: Option<String>,
    pub title: String,
    pub completed: bool,
}

/// Changes to a task. `None` leaves a field as it is; for the optional
/// fields, `Some(None)` clears it (an empty `description` clears the notes).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub due_date: Option<Option<String>>,
    pub priority: Option<String>,
    pub status: Option<String>,
    pub tags: Option<Vec<String>>,
    pub project: Option<Option<String>>,
    pub reminder: Option<Option<String>>,
    /// Replaces the subtask list.
    pub subtasks: Option<Vec<SubtaskSpec>>,
}

impl TaskPatch {
    pub fn is_empty(&self) -> bool {
        *self == TaskPatch::default()
    }

    /// Clear the fields named in `clear` (see [`TASK_CLEARABLE`]). Fails on
    /// an unknown name or on a field this patch also sets.
    pub fn clearing(mut self, clear: &[String]) -> CalendarResult<Self> {
        for name in clear_names(clear, &TASK_CLEARABLE, "a task")? {
            match name {
                "due_date" => clear_slot(&mut self.due_date, name)?,
                "project" => clear_slot(&mut self.project, name)?,
                "reminder" => clear_slot(&mut self.reminder, name)?,
                _ => clear_text(&mut self.description, name)?,
            }
        }
        Ok(self)
    }
}

/// One changed field, for approval previews and results.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldChange {
    pub field: &'static str,
    pub before: Value,
    pub after: Value,
}

impl FieldChange {
    pub fn to_json(&self) -> Value {
        json!({ "field": self.field, "before": self.before, "after": self.after })
    }
}

fn check_title(title: &str) -> CalendarResult<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(CalendarError::Invalid("The title cannot be empty".into()));
    }
    Ok(title.to_string())
}

fn check_one_of(field: &str, value: &str, allowed: &[&str]) -> CalendarResult<String> {
    if allowed.contains(&value) {
        Ok(value.to_string())
    } else {
        Err(CalendarError::Invalid(format!(
            "`{field}` must be one of {}",
            allowed.join(", ")
        )))
    }
}

fn check_moment(field: &str, value: &str) -> CalendarResult<String> {
    parse_moment(value)
        .map(|_| value.trim().to_string())
        .ok_or_else(|| CalendarError::Invalid(format!("`{field}` {value:?}: {MOMENT_HINT}")))
}

fn subtasks_json(subtasks: &[SubTask]) -> Value {
    Value::Array(
        subtasks
            .iter()
            .map(|s| json!({ "title": s.title, "completed": s.completed }))
            .collect(),
    )
}

/// Apply `patch` to `task` and return what changed. Validates every field
/// before touching the task.
pub fn apply_task_patch(
    task: &mut TodoItem,
    patch: &TaskPatch,
) -> CalendarResult<Vec<FieldChange>> {
    let title = patch.title.as_deref().map(check_title).transpose()?;
    let priority = patch
        .priority
        .as_deref()
        .map(|p| check_one_of("priority", p, &PRIORITIES))
        .transpose()?;
    let status = patch
        .status
        .as_deref()
        .map(|s| check_one_of("status", s, &STATUSES))
        .transpose()?;
    let due = match &patch.due_date {
        Some(Some(d)) => Some(Some(check_moment("due", d)?)),
        Some(None) => Some(None),
        None => None,
    };
    let reminder = match &patch.reminder {
        Some(Some(r)) => Some(Some(check_reminder(r)?)),
        Some(None) => Some(None),
        None => None,
    };
    let subtasks = match &patch.subtasks {
        Some(specs) => {
            let mut out = Vec::with_capacity(specs.len());
            for spec in specs {
                let title = check_title(&spec.title)?;
                let id = match &spec.id {
                    Some(id) if task.subtasks.iter().any(|s| &s.id == id) => id.clone(),
                    Some(id) => return Err(CalendarError::SubtaskNotFound(id.clone())),
                    None => Uuid::new_v4().to_string(),
                };
                out.push(SubTask {
                    id,
                    title,
                    completed: spec.completed,
                });
            }
            Some(out)
        }
        None => None,
    };

    let before = task.clone();
    if let Some(v) = title {
        task.title = v;
    }
    if let Some(v) = &patch.description {
        task.description = v.clone();
    }
    if let Some(v) = due {
        task.due_date = v;
    }
    if let Some(v) = priority {
        task.priority = v;
    }
    if let Some(v) = status {
        if v == "completed" && task.status != "completed" {
            task.completed_at = Some(now());
        } else if v != "completed" {
            task.completed_at = None;
        }
        task.status = v;
    }
    if let Some(v) = &patch.tags {
        task.tags = v.clone();
    }
    if let Some(v) = &patch.project {
        task.project = optional_text(v.clone());
    }
    if let Some(v) = reminder {
        if v != task.reminder {
            // A new or removed reminder starts over: not yet shown, not snoozed.
            task.reminder_fired_at = None;
            task.snoozed_until = None;
        }
        task.reminder = v;
    }
    if let Some(v) = subtasks {
        task.subtasks = v;
    }

    let mut changes = Vec::new();
    let mut diff = |field: &'static str, b: Value, a: Value| {
        if b != a {
            changes.push(FieldChange {
                field,
                before: b,
                after: a,
            });
        }
    };
    diff("title", json!(before.title), json!(task.title));
    diff("notes", json!(before.description), json!(task.description));
    diff("due", json!(before.due_date), json!(task.due_date));
    diff("priority", json!(before.priority), json!(task.priority));
    diff("status", json!(before.status), json!(task.status));
    diff("tags", json!(before.tags), json!(task.tags));
    diff("project", json!(before.project), json!(task.project));
    diff("reminder", json!(before.reminder), json!(task.reminder));
    diff(
        "subtasks",
        subtasks_json(&before.subtasks),
        subtasks_json(&task.subtasks),
    );
    if !changes.is_empty() {
        task.updated_at = now();
    }
    Ok(changes)
}

impl CalendarData {
    pub fn task(&self, id: &str) -> CalendarResult<&TodoItem> {
        self.tasks
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| CalendarError::TaskNotFound(id.to_string()))
    }

    fn task_mut(&mut self, id: &str) -> CalendarResult<&mut TodoItem> {
        self.tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| CalendarError::TaskNotFound(id.to_string()))
    }

    pub fn event(&self, id: &str) -> CalendarResult<&CalendarEvent> {
        self.events
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| CalendarError::EventNotFound(id.to_string()))
    }

    fn event_mut(&mut self, id: &str) -> CalendarResult<&mut CalendarEvent> {
        self.events
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| CalendarError::EventNotFound(id.to_string()))
    }

    pub fn insert_task(&mut self, new: NewTask) -> CalendarResult<TodoItem> {
        let title = check_title(&new.title)?;
        let priority = match new.priority.as_deref() {
            Some(p) => check_one_of("priority", p, &PRIORITIES)?,
            None => default_priority(),
        };
        let due_date = new
            .due_date
            .as_deref()
            .map(|d| check_moment("due", d))
            .transpose()?;
        let reminder = new.reminder.as_deref().map(check_reminder).transpose()?;
        let stamp = now();
        let task = TodoItem {
            id: Uuid::new_v4().to_string(),
            title,
            description: new.description.unwrap_or_default(),
            due_date,
            priority,
            status: default_status(),
            tags: new.tags.unwrap_or_default(),
            subtasks: Vec::new(),
            project: optional_text(new.project),
            source: new.source.unwrap_or_else(default_source),
            source_ref: new.source_ref,
            created_at: stamp.clone(),
            updated_at: stamp,
            completed_at: None,
            reminder,
            reminder_fired_at: None,
            snoozed_until: None,
        };
        self.tasks.push(task.clone());
        Ok(task)
    }

    /// Apply `patch` to task `id`; returns the task before and after.
    pub fn patch_task(
        &mut self,
        id: &str,
        patch: &TaskPatch,
    ) -> CalendarResult<(TodoItem, TodoItem, Vec<FieldChange>)> {
        let task = self.task_mut(id)?;
        let before = task.clone();
        let changes = apply_task_patch(task, patch)?;
        Ok((before, task.clone(), changes))
    }

    pub fn delete_task(&mut self, id: &str) -> CalendarResult<TodoItem> {
        let index = self
            .tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| CalendarError::TaskNotFound(id.to_string()))?;
        Ok(self.tasks.remove(index))
    }

    pub fn add_subtask(&mut self, task_id: &str, title: &str) -> CalendarResult<TodoItem> {
        let title = check_title(title)?;
        let task = self.task_mut(task_id)?;
        task.subtasks.push(SubTask {
            id: Uuid::new_v4().to_string(),
            title,
            completed: false,
        });
        task.updated_at = now();
        Ok(task.clone())
    }

    pub fn toggle_subtask(&mut self, task_id: &str, subtask_id: &str) -> CalendarResult<TodoItem> {
        let task = self.task_mut(task_id)?;
        let subtask = task
            .subtasks
            .iter_mut()
            .find(|s| s.id == subtask_id)
            .ok_or_else(|| CalendarError::SubtaskNotFound(subtask_id.to_string()))?;
        subtask.completed = !subtask.completed;
        task.updated_at = now();
        Ok(task.clone())
    }

    pub fn rename_subtask(
        &mut self,
        task_id: &str,
        subtask_id: &str,
        title: &str,
    ) -> CalendarResult<TodoItem> {
        let title = check_title(title)?;
        let task = self.task_mut(task_id)?;
        let subtask = task
            .subtasks
            .iter_mut()
            .find(|s| s.id == subtask_id)
            .ok_or_else(|| CalendarError::SubtaskNotFound(subtask_id.to_string()))?;
        if subtask.title != title {
            subtask.title = title;
            task.updated_at = now();
        }
        Ok(task.clone())
    }

    /// Record that the reminder ringing at `due` (the task's current
    /// `snoozed_until` or `reminder` value) was shown at `fired_at`.
    /// Returns false, changing nothing, when it was already shown or the
    /// reminder has changed since it was planned: the caller then must not
    /// notify, which makes firing idempotent.
    pub fn mark_reminder_fired(
        &mut self,
        task_id: &str,
        due: &str,
        fired_at: &str,
    ) -> CalendarResult<bool> {
        let task = self.task_mut(task_id)?;
        let current = task.snoozed_until.as_deref().or(task.reminder.as_deref());
        if task.reminder_fired_at.is_some() || current != Some(due) {
            return Ok(false);
        }
        task.reminder_fired_at = Some(fired_at.to_string());
        Ok(true)
    }

    /// Ring the task's reminder again at `until` (local, [`REMINDER_FORMAT`]).
    pub fn snooze_reminder(
        &mut self,
        task_id: &str,
        until: NaiveDateTime,
    ) -> CalendarResult<TodoItem> {
        let task = self.task_mut(task_id)?;
        if task.reminder.is_none() {
            return Err(CalendarError::Invalid(format!(
                "Task \"{}\" has no reminder to snooze",
                task.title
            )));
        }
        task.snoozed_until = Some(until.format(REMINDER_FORMAT).to_string());
        task.reminder_fired_at = None;
        Ok(task.clone())
    }

    pub fn delete_subtask(&mut self, task_id: &str, subtask_id: &str) -> CalendarResult<TodoItem> {
        let task = self.task_mut(task_id)?;
        task.subtasks.retain(|s| s.id != subtask_id);
        task.updated_at = now();
        Ok(task.clone())
    }

    pub fn insert_event(&mut self, new: NewEvent) -> CalendarResult<CalendarEvent> {
        let title = check_title(&new.title)?;
        let start = check_moment("start", &new.start_time)?;
        let end = new
            .end_time
            .as_deref()
            .map(|e| check_moment("end", e))
            .transpose()?;
        check_order(&start, end.as_deref())?;
        let event = CalendarEvent {
            id: Uuid::new_v4().to_string(),
            title,
            description: new.description.unwrap_or_default(),
            start_time: start,
            end_time: end,
            all_day: new.all_day.unwrap_or(false),
            color: new.color,
            location: optional_text(new.location),
            source: new.source.unwrap_or_else(default_source),
            source_ref: new.source_ref,
            created_at: now(),
        };
        self.events.push(event.clone());
        Ok(event)
    }

    pub fn patch_event(
        &mut self,
        id: &str,
        patch: &EventPatch,
    ) -> CalendarResult<(CalendarEvent, CalendarEvent, Vec<FieldChange>)> {
        let event = self.event_mut(id)?;
        let before = event.clone();
        let changes = apply_event_patch(event, patch)?;
        Ok((before, event.clone(), changes))
    }

    pub fn delete_event(&mut self, id: &str) -> CalendarResult<CalendarEvent> {
        let index = self
            .events
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| CalendarError::EventNotFound(id.to_string()))?;
        Ok(self.events.remove(index))
    }
}

// ── Events ───────────────────────────────────────────────────────

/// Fields for a new calendar event. Shared by the `create_event` command
/// and the agent's `create_event` tool.
#[derive(Debug, Clone, Default)]
pub struct NewEvent {
    pub title: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub all_day: Option<bool>,
    pub description: Option<String>,
    pub color: Option<String>,
    pub location: Option<String>,
    pub source: Option<String>,
    pub source_ref: Option<String>,
}

/// Changes to an event. For `end_time` and `location`, `Some(None)` clears
/// it; an empty `description` clears the notes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<Option<String>>,
    pub all_day: Option<bool>,
    pub color: Option<String>,
    pub location: Option<Option<String>>,
}

impl EventPatch {
    pub fn is_empty(&self) -> bool {
        *self == EventPatch::default()
    }

    /// Clear the fields named in `clear` (see [`EVENT_CLEARABLE`]). Fails on
    /// an unknown name or on a field this patch also sets.
    pub fn clearing(mut self, clear: &[String]) -> CalendarResult<Self> {
        for name in clear_names(clear, &EVENT_CLEARABLE, "an event")? {
            match name {
                "end_time" => clear_slot(&mut self.end_time, name)?,
                "location" => clear_slot(&mut self.location, name)?,
                _ => clear_text(&mut self.description, name)?,
            }
        }
        Ok(self)
    }
}

fn check_order(start: &str, end: Option<&str>) -> CalendarResult<()> {
    if let (Some(s), Some(e)) = (parse_moment(start), end.and_then(parse_moment)) {
        if e.as_datetime() < s.as_datetime() {
            return Err(CalendarError::Invalid("The end is before the start".into()));
        }
    }
    Ok(())
}

/// Apply `patch` to `event` and return what changed.
pub fn apply_event_patch(
    event: &mut CalendarEvent,
    patch: &EventPatch,
) -> CalendarResult<Vec<FieldChange>> {
    let title = patch.title.as_deref().map(check_title).transpose()?;
    let start = patch
        .start_time
        .as_deref()
        .map(|s| check_moment("start", s))
        .transpose()?;
    let end = match &patch.end_time {
        Some(Some(e)) => Some(Some(check_moment("end", e)?)),
        Some(None) => Some(None),
        None => None,
    };
    let new_start = start.clone().unwrap_or_else(|| event.start_time.clone());
    let new_end = end.clone().unwrap_or_else(|| event.end_time.clone());
    check_order(&new_start, new_end.as_deref())?;

    let before = event.clone();
    if let Some(v) = title {
        event.title = v;
    }
    if let Some(v) = &patch.description {
        event.description = v.clone();
    }
    event.start_time = new_start;
    event.end_time = new_end;
    if let Some(v) = patch.all_day {
        event.all_day = v;
    }
    if let Some(v) = &patch.color {
        event.color = Some(v.clone());
    }
    if let Some(v) = &patch.location {
        event.location = optional_text(v.clone());
    }

    let mut changes = Vec::new();
    let mut diff = |field: &'static str, b: Value, a: Value| {
        if b != a {
            changes.push(FieldChange {
                field,
                before: b,
                after: a,
            });
        }
    };
    diff("title", json!(before.title), json!(event.title));
    diff("notes", json!(before.description), json!(event.description));
    diff("start", json!(before.start_time), json!(event.start_time));
    diff("end", json!(before.end_time), json!(event.end_time));
    diff("allDay", json!(before.all_day), json!(event.all_day));
    diff("color", json!(before.color), json!(event.color));
    diff("location", json!(before.location), json!(event.location));
    Ok(changes)
}

// ── Queries ──────────────────────────────────────────────────────

/// Filters for listing tasks. Every field is optional.
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub status: Option<String>,
    /// Inclusive due-date bounds; tasks without a due date are excluded
    /// when either bound is set.
    pub due_from: Option<NaiveDate>,
    pub due_to: Option<NaiveDate>,
    /// Case-insensitive substring of title, notes, tags or project.
    pub text: Option<String>,
}

fn contains_ci(haystack: &str, needle_lower: &str) -> bool {
    haystack.to_lowercase().contains(needle_lower)
}

/// Tasks matching `filter`: due ones first by due date, then the rest by
/// creation time.
pub fn list_tasks<'a>(data: &'a CalendarData, filter: &TaskFilter) -> Vec<&'a TodoItem> {
    let needle = filter.text.as_deref().map(str::to_lowercase);
    let mut out: Vec<&TodoItem> = data
        .tasks
        .iter()
        .filter(|t| filter.status.as_deref().is_none_or(|s| t.status == s))
        .filter(|t| {
            if filter.due_from.is_none() && filter.due_to.is_none() {
                return true;
            }
            let Some(due) = t.due_date.as_deref().and_then(parse_moment) else {
                return false;
            };
            let day = due.date();
            filter.due_from.is_none_or(|f| day >= f) && filter.due_to.is_none_or(|e| day <= e)
        })
        .filter(|t| {
            needle.as_deref().is_none_or(|n| {
                contains_ci(&t.title, n)
                    || contains_ci(&t.description, n)
                    || t.tags.iter().any(|tag| contains_ci(tag, n))
                    || t.project.as_deref().is_some_and(|p| contains_ci(p, n))
            })
        })
        .collect();
    out.sort_by(|a, b| {
        let da = a.due_date.as_deref().and_then(parse_moment);
        let db = b.due_date.as_deref().and_then(parse_moment);
        match (da, db) {
            (Some(x), Some(y)) => x.as_datetime().cmp(&y.as_datetime()),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.created_at.cmp(&b.created_at),
        }
    });
    out
}

/// Filters for listing events.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Events overlapping this inclusive date range.
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub text: Option<String>,
}

/// Events matching `filter`, by start time.
pub fn list_events<'a>(data: &'a CalendarData, filter: &EventFilter) -> Vec<&'a CalendarEvent> {
    let needle = filter.text.as_deref().map(str::to_lowercase);
    let mut out: Vec<&CalendarEvent> = data
        .events
        .iter()
        .filter(|e| {
            let Some(start) = parse_moment(&e.start_time) else {
                return filter.from.is_none() && filter.to.is_none();
            };
            let end = e
                .end_time
                .as_deref()
                .and_then(parse_moment)
                .unwrap_or(start);
            filter.from.is_none_or(|f| end.date() >= f)
                && filter.to.is_none_or(|t| start.date() <= t)
        })
        .filter(|e| {
            needle.as_deref().is_none_or(|n| {
                contains_ci(&e.title, n)
                    || contains_ci(&e.description, n)
                    || e.location.as_deref().is_some_and(|l| contains_ci(l, n))
            })
        })
        .collect();
    out.sort_by_key(|e| parse_moment(&e.start_time).map(Moment::as_datetime));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, CalendarStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = CalendarStore::in_dir(dir.path());
        (dir, store)
    }

    fn task(store: &CalendarStore, title: &str, due: Option<&str>) -> TodoItem {
        store
            .update(|d| {
                d.insert_task(NewTask {
                    title: title.into(),
                    due_date: due.map(str::to_string),
                    ..NewTask::default()
                })
            })
            .unwrap()
    }

    #[test]
    fn moments_parse_dates_and_times() {
        assert!(matches!(parse_moment("2026-10-30"), Some(Moment::Date(_))));
        assert!(matches!(
            parse_moment("2026-10-30T09:30"),
            Some(Moment::DateTime(_))
        ));
        assert!(matches!(
            parse_moment("2026-10-30T09:30:00+05:30"),
            Some(Moment::DateTime(_))
        ));
        assert_eq!(parse_moment("next friday"), None);
        assert_eq!(parse_moment("2026-13-01"), None);
    }

    #[test]
    fn missing_file_is_an_empty_calendar_and_writes_persist() {
        let (_dir, store) = store();
        assert_eq!(store.load().unwrap(), CalendarData::default());
        let t = task(&store, "Pay invoice", Some("2026-10-30"));
        let loaded = store.load().unwrap();
        assert_eq!(loaded.tasks, vec![t]);
    }

    #[test]
    fn failed_changes_write_nothing() {
        let (_dir, store) = store();
        task(&store, "A", None);
        let err = store
            .update(|d| d.patch_task("nope", &TaskPatch::default()))
            .unwrap_err();
        assert!(matches!(err, CalendarError::TaskNotFound(_)));
        let bad = store.update(|d| {
            d.insert_task(NewTask {
                title: "  ".into(),
                ..NewTask::default()
            })
        });
        assert!(matches!(bad, Err(CalendarError::Invalid(_))));
        assert_eq!(store.load().unwrap().tasks.len(), 1);
    }

    #[test]
    fn task_patches_report_before_and_after() {
        let (_dir, store) = store();
        let t = task(&store, "Draft", Some("2026-10-30"));
        let (before, after, changes) = store
            .update(|d| {
                d.patch_task(
                    &t.id,
                    &TaskPatch {
                        title: Some("Final".into()),
                        due_date: Some(None),
                        status: Some("completed".into()),
                        subtasks: Some(vec![SubtaskSpec {
                            id: None,
                            title: "Proofread".into(),
                            completed: false,
                        }]),
                        ..TaskPatch::default()
                    },
                )
            })
            .unwrap();
        assert_eq!(before.title, "Draft");
        assert_eq!(after.title, "Final");
        assert!(after.completed_at.is_some());
        assert_eq!(after.due_date, None);
        let fields: Vec<&str> = changes.iter().map(|c| c.field).collect();
        assert_eq!(fields, vec!["title", "due", "status", "subtasks"]);
        assert_eq!(changes[1].before, json!("2026-10-30"));
        assert_eq!(changes[1].after, Value::Null);

        let unchanged = store
            .update(|d| {
                d.patch_task(
                    &t.id,
                    &TaskPatch {
                        title: Some("Final".into()),
                        ..TaskPatch::default()
                    },
                )
            })
            .unwrap();
        assert!(unchanged.2.is_empty());
        let stored = store.load().unwrap();
        assert_eq!(stored.task(&t.id).unwrap().status, "completed");
    }

    #[test]
    fn task_patches_validate_every_field_first() {
        let (_dir, store) = store();
        let t = task(&store, "Draft", None);
        for patch in [
            TaskPatch {
                priority: Some("critical".into()),
                ..TaskPatch::default()
            },
            TaskPatch {
                status: Some("done".into()),
                ..TaskPatch::default()
            },
            TaskPatch {
                due_date: Some(Some("tomorrow".into())),
                ..TaskPatch::default()
            },
            TaskPatch {
                subtasks: Some(vec![SubtaskSpec {
                    id: Some("missing".into()),
                    title: "x".into(),
                    completed: false,
                }]),
                ..TaskPatch::default()
            },
        ] {
            let err = store.update(|d| d.patch_task(&t.id, &patch));
            assert!(err.is_err(), "{patch:?}");
        }
        assert_eq!(store.load().unwrap().task(&t.id).unwrap(), &t);
    }

    #[test]
    fn task_lists_filter_by_status_due_range_and_text() {
        let (_dir, store) = store();
        let a = task(&store, "File GST return", Some("2026-10-20"));
        let b = task(&store, "Renew lease", Some("2026-10-05"));
        let c = task(&store, "Call bank", None);
        store
            .update(|d| {
                d.patch_task(
                    &c.id,
                    &TaskPatch {
                        status: Some("completed".into()),
                        ..TaskPatch::default()
                    },
                )
            })
            .unwrap();
        let data = store.load().unwrap();
        let ids = |f: &TaskFilter| -> Vec<String> {
            list_tasks(&data, f).iter().map(|t| t.id.clone()).collect()
        };
        assert_eq!(
            ids(&TaskFilter::default()),
            vec![b.id.clone(), a.id.clone(), c.id.clone()]
        );
        assert_eq!(
            ids(&TaskFilter {
                due_from: NaiveDate::from_ymd_opt(2026, 10, 10),
                due_to: NaiveDate::from_ymd_opt(2026, 10, 31),
                ..TaskFilter::default()
            }),
            vec![a.id.clone()]
        );
        assert_eq!(
            ids(&TaskFilter {
                status: Some("completed".into()),
                ..TaskFilter::default()
            }),
            vec![c.id.clone()]
        );
        assert_eq!(
            ids(&TaskFilter {
                text: Some("gst".into()),
                ..TaskFilter::default()
            }),
            vec![a.id]
        );
    }

    #[test]
    fn events_validate_order_and_list_by_overlap() {
        let (_dir, store) = store();
        let e = store
            .update(|d| {
                d.insert_event(NewEvent {
                    title: "Offsite".into(),
                    start_time: "2026-10-10".into(),
                    end_time: Some("2026-10-12".into()),
                    ..NewEvent::default()
                })
            })
            .unwrap();
        let bad = store.update(|d| {
            d.insert_event(NewEvent {
                title: "Backwards".into(),
                start_time: "2026-10-10T10:00".into(),
                end_time: Some("2026-10-10T09:00".into()),
                ..NewEvent::default()
            })
        });
        assert!(bad.is_err());
        let data = store.load().unwrap();
        let hit = list_events(
            &data,
            &EventFilter {
                from: NaiveDate::from_ymd_opt(2026, 10, 11),
                to: NaiveDate::from_ymd_opt(2026, 10, 11),
                text: None,
            },
        );
        assert_eq!(hit.len(), 1);
        let miss = list_events(
            &data,
            &EventFilter {
                from: NaiveDate::from_ymd_opt(2026, 10, 13),
                ..EventFilter::default()
            },
        );
        assert!(miss.is_empty());

        let moved = store.update(|d| {
            d.patch_event(
                &e.id,
                &EventPatch {
                    start_time: Some("2026-10-13".into()),
                    ..EventPatch::default()
                },
            )
        });
        assert!(moved.is_err(), "start after the existing end");
        let (_, after, changes) = store
            .update(|d| {
                d.patch_event(
                    &e.id,
                    &EventPatch {
                        title: Some("Team offsite".into()),
                        end_time: Some(None),
                        ..EventPatch::default()
                    },
                )
            })
            .unwrap();
        assert_eq!(after.end_time, None);
        let fields: Vec<&str> = changes.iter().map(|c| c.field).collect();
        assert_eq!(fields, vec!["title", "end"]);
        let removed = store.update(|d| d.delete_event(&e.id)).unwrap();
        assert_eq!(removed.title, "Team offsite");
        assert!(store.load().unwrap().events.is_empty());
    }

    #[test]
    fn records_written_before_reminders_and_locations_still_load() {
        let old = r#"{
            "tasks": [{"id": "t1", "title": "Pay rent", "createdAt": "2026-01-01T00:00:00Z",
                       "updatedAt": "2026-01-01T00:00:00Z", "reminder": "whenever"}],
            "events": [{"id": "e1", "title": "Standup", "startTime": "2026-10-10T09:00",
                        "createdAt": "2026-01-01T00:00:00Z"}]
        }"#;
        let data: CalendarData = serde_json::from_str(old).unwrap();
        let task = data.task("t1").unwrap();
        assert_eq!(task.reminder_fired_at, None);
        assert_eq!(task.snoozed_until, None);
        assert_eq!(parse_reminder(task.reminder.as_deref().unwrap()), None);
        assert_eq!(data.event("e1").unwrap().location, None);
        // Absent fields stay absent when written back.
        let text = serde_json::to_string(&data).unwrap();
        assert!(!text.contains("location") && !text.contains("snoozedUntil"));
    }

    #[test]
    fn locations_round_trip_and_can_be_cleared() {
        let (_dir, store) = store();
        let e = store
            .update(|d| {
                d.insert_event(NewEvent {
                    title: "Review".into(),
                    start_time: "2026-10-10T10:00".into(),
                    location: Some("  Room 4B ".into()),
                    ..NewEvent::default()
                })
            })
            .unwrap();
        assert_eq!(e.location.as_deref(), Some("Room 4B"));
        let loaded = store.load().unwrap();
        assert_eq!(loaded.event(&e.id).unwrap(), &e);
        let found = list_events(
            &loaded,
            &EventFilter {
                text: Some("room 4b".into()),
                ..EventFilter::default()
            },
        );
        assert_eq!(found.len(), 1);
        let patch = EventPatch::default()
            .clearing(&["location".into(), "end_time".into()])
            .unwrap();
        let (_, after, changes) = store.update(|d| d.patch_event(&e.id, &patch)).unwrap();
        assert_eq!(after.location, None);
        let fields: Vec<&str> = changes.iter().map(|c| c.field).collect();
        assert_eq!(fields, vec!["location"], "end was already empty");
    }

    #[test]
    fn clearing_needs_an_allowlisted_name_and_never_conflicts_with_a_set() {
        let unknown = TaskPatch::default().clearing(&["title".into()]);
        assert!(matches!(unknown, Err(CalendarError::Invalid(_))));
        let event_only = TaskPatch::default().clearing(&["location".into()]);
        assert!(event_only.is_err(), "location is an event field");
        let both = TaskPatch {
            project: Some(Some("Tax".into())),
            ..TaskPatch::default()
        }
        .clearing(&["project".into()]);
        assert!(both.is_err());
        let notes = TaskPatch {
            description: Some("keep".into()),
            ..TaskPatch::default()
        }
        .clearing(&["description".into()]);
        assert!(notes.is_err());
        assert!(EventPatch::default()
            .clearing(&["due_date".into()])
            .is_err());
    }

    #[test]
    fn cleared_fields_empty_and_absent_fields_keep() {
        let (_dir, store) = store();
        let t = store
            .update(|d| {
                d.insert_task(NewTask {
                    title: "File return".into(),
                    description: Some("Form 16".into()),
                    due_date: Some("2026-10-30".into()),
                    project: Some("Tax".into()),
                    reminder: Some("2026-10-29T09:00".into()),
                    ..NewTask::default()
                })
            })
            .unwrap();
        // Nothing named: nothing cleared.
        let keep = TaskPatch {
            title: Some("File ITR".into()),
            ..TaskPatch::default()
        }
        .clearing(&[])
        .unwrap();
        let (_, kept, _) = store.update(|d| d.patch_task(&t.id, &keep)).unwrap();
        assert_eq!(kept.project.as_deref(), Some("Tax"));
        assert_eq!(kept.due_date.as_deref(), Some("2026-10-30"));
        assert_eq!(kept.reminder.as_deref(), Some("2026-10-29T09:00"));
        let clear = TaskPatch::default()
            .clearing(&[
                "due_date".into(),
                "project".into(),
                "description".into(),
                "reminder".into(),
                "project".into(),
            ])
            .unwrap();
        let (_, cleared, changes) = store.update(|d| d.patch_task(&t.id, &clear)).unwrap();
        assert_eq!(cleared.due_date, None);
        assert_eq!(cleared.project, None);
        assert_eq!(cleared.description, "");
        assert_eq!(cleared.reminder, None);
        let fields: Vec<&str> = changes.iter().map(|c| c.field).collect();
        assert_eq!(fields, vec!["notes", "due", "project", "reminder"]);
        // Blank text sets nothing rather than an empty project.
        let blank = TaskPatch {
            project: Some(Some("   ".into())),
            ..TaskPatch::default()
        };
        let (_, after, _) = store.update(|d| d.patch_task(&t.id, &blank)).unwrap();
        assert_eq!(after.project, None);
    }

    #[test]
    fn subtasks_can_be_renamed() {
        let (_dir, store) = store();
        let t = task(&store, "Trip", None);
        let with_sub = store
            .update(|d| d.add_subtask(&t.id, "Book hotel"))
            .unwrap();
        let sub = with_sub.subtasks[0].id.clone();
        let renamed = store
            .update(|d| d.rename_subtask(&t.id, &sub, "  Book hotel near venue "))
            .unwrap();
        assert_eq!(renamed.subtasks[0].title, "Book hotel near venue");
        assert_eq!(renamed.subtasks[0].id, sub, "the id is kept");
        assert!(matches!(
            store.update(|d| d.rename_subtask(&t.id, &sub, "  ")),
            Err(CalendarError::Invalid(_))
        ));
        assert!(matches!(
            store.update(|d| d.rename_subtask(&t.id, "missing", "x")),
            Err(CalendarError::SubtaskNotFound(_))
        ));
        assert_eq!(
            store.load().unwrap().task(&t.id).unwrap().subtasks[0].title,
            "Book hotel near venue"
        );
    }

    #[test]
    fn reminders_are_normalised_to_local_minutes() {
        let ist = chrono::FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
        assert_eq!(
            normalize_reminder_in("2026-10-29T09:00", &ist).unwrap(),
            "2026-10-29T09:00"
        );
        assert_eq!(
            normalize_reminder_in("2026-10-29 09:00:45", &ist).unwrap(),
            "2026-10-29T09:00"
        );
        // An instant with an offset rings at that instant, in local time.
        assert_eq!(
            normalize_reminder_in("2026-10-29T03:30:00Z", &ist).unwrap(),
            "2026-10-29T09:00"
        );
        assert!(normalize_reminder_in("2026-10-29", &ist).is_err());
        assert!(normalize_reminder_in("tomorrow 9am", &ist).is_err());
    }

    #[test]
    fn changing_a_reminder_resets_its_fired_and_snoozed_state() {
        let (_dir, store) = store();
        let t = store
            .update(|d| {
                d.insert_task(NewTask {
                    title: "Call bank".into(),
                    reminder: Some("2026-10-29T09:00".into()),
                    ..NewTask::default()
                })
            })
            .unwrap();
        let fire = |at: &str| {
            store
                .update(|d| d.mark_reminder_fired(&t.id, "2026-10-29T09:00", at))
                .unwrap()
        };
        assert!(fire("2026-10-29T03:30:00Z"));
        assert!(!fire("2026-10-29T03:31:00Z"), "a reminder fires once");
        let nine_ten = NaiveDate::from_ymd_opt(2026, 10, 29)
            .unwrap()
            .and_hms_opt(9, 10, 0)
            .unwrap();
        let snoozed = store
            .update(|d| d.snooze_reminder(&t.id, nine_ten))
            .unwrap();
        assert_eq!(snoozed.snoozed_until.as_deref(), Some("2026-10-29T09:10"));
        assert_eq!(snoozed.reminder_fired_at, None);
        assert!(!fire("x"), "the planned time is no longer current");
        let patch = TaskPatch {
            reminder: Some(Some("2026-10-30T08:00".into())),
            ..TaskPatch::default()
        };
        let (_, moved, _) = store.update(|d| d.patch_task(&t.id, &patch)).unwrap();
        assert_eq!(moved.snoozed_until, None);
        assert_eq!(moved.reminder_fired_at, None);
        let plain = task(&store, "Plain", None);
        assert!(store
            .update(|d| d.snooze_reminder(&plain.id, nine_ten))
            .is_err());
    }
}
