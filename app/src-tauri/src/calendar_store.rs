//! Calendar storage: tasks and events in `<app_data_dir>/calendar_data.json`.
//!
//! Every change is a read-modify-write of the whole file under one
//! process-wide lock, written atomically (temp file + rename), so the UI
//! commands and the agent's tools never lose each other's updates. The
//! operations here are pure data changes; callers report each change to
//! [`crate::agent_tools::HostEffects::calendar_changed`], which re-indexes
//! the record and tells open views to refresh.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use std::sync::Arc;

use crate::durable_file::{self, Loaded, Origin};
use crate::store_notices::{self, NoticeKind, StoreNotice};
use crate::task_db::{Actor, TaskDb};

pub const CALENDAR_FILE: &str = "calendar_data.json";

/// Task priorities, Linear's five: no priority, urgent, high, medium, low.
pub const PRIORITIES: [&str; 5] = ["none", "urgent", "high", "medium", "low"];

/// Task states: Backlog, Todo (`pending`, the word older clients read), In progress, Done
/// (`completed`) and Canceled.
pub const STATUSES: [&str; 5] = ["backlog", "pending", "in_progress", "completed", "canceled"];

/// Whether a task in `status` is closed: done or canceled. A closed task's reminder never
/// rings.
pub fn is_closed(status: &str) -> bool {
    matches!(status, "completed" | "canceled")
}

/// Who a task is for: the user, a person (a name; no one is told), or Shodh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnerKind {
    #[default]
    Me,
    Person,
    Shodh,
}

impl OwnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Me => "me",
            Self::Person => "person",
            Self::Shodh => "shodh",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "me" => Some(Self::Me),
            "person" => Some(Self::Person),
            "shodh" => Some(Self::Shodh),
            _ => None,
        }
    }
}

/// A task's owner (its assignee).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub kind: OwnerKind,
    /// A person's name.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl Owner {
    pub fn is_me(&self) -> bool {
        self.kind == OwnerKind::Me
    }
}

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
    #[error("The calendar could not be read: {message}")]
    Unreadable { path: PathBuf, message: String },
    #[error("Tasks could not be saved: {0}")]
    Store(String),
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
    /// The workspace the task belongs to (its project). Tasks written before
    /// workspaces held tasks have none; deleting a workspace clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
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
    /// The task's id in its workspace (`THE-12`), given when it is first stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The project the task is in, when it has a workspace and a project (its name is
    /// `project`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The task this one is a sub-issue of, in the same workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Who the task is for; absent means the user.
    #[serde(default, skip_serializing_if = "Owner::is_me")]
    pub owner: Owner,
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
    /// When it last changed (RFC 3339, UTC): a phone's calendar sync keeps the
    /// latest of two edits made on both sides. Events saved before it was kept
    /// have none (their `created_at` stands in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// The priority of a task made without one.
pub const NEW_TASK_PRIORITY: &str = "none";

/// The priority of a stored task written without one (before priorities could be absent).
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
pub const TASK_CLEARABLE: [&str; 5] = [
    "due_date",
    "project",
    "description",
    "reminder",
    "workspace_id",
];

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

/// Where the calendar's original file is kept once its tasks moved to `shodh.db`.
pub const PRE_TASKS_FILE: &str = "calendar_data.pre-tasks.json";

/// The calendar of one app data directory: its events in the calendar file, its tasks in
/// `shodh.db` once the app gave it the database ([`crate::task_db::use_database`]), else
/// in the file too.
#[derive(Debug, Clone)]
pub struct CalendarStore {
    path: PathBuf,
    tasks: Option<Arc<TaskDb>>,
}

impl CalendarStore {
    pub fn in_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(CALENDAR_FILE),
            tasks: crate::task_db::database_of(data_dir),
        }
    }

    /// The calendar of `data_dir`, its tasks in `tasks`.
    pub fn with_tasks(data_dir: &Path, tasks: Arc<TaskDb>) -> Self {
        Self {
            path: data_dir.join(CALENDAR_FILE),
            tasks: Some(tasks),
        }
    }

    /// The task database, when tasks are kept there.
    pub fn task_db(&self) -> Option<&Arc<TaskDb>> {
        self.tasks.as_ref()
    }

    /// The calendar as stored, and where its tasks are: the file's events, and its tasks
    /// from the database. Tasks still in the file (an older version's, or written while
    /// the database could not be opened) are imported first; the original file is kept
    /// as [`PRE_TASKS_FILE`], and the tasks leave the file only once they are stored. An
    /// import that fails leaves the file as it is, says so, and keeps the tasks there.
    fn read_all(&self) -> CalendarResult<(Loaded<CalendarData>, Option<Arc<TaskDb>>)> {
        let mut loaded = self.read_unlocked()?;
        let Some(db) = self.tasks.clone() else {
            return Ok((loaded, None));
        };
        if !loaded.value.tasks.is_empty() {
            if let Err(e) = self.import_file_tasks(&db, &loaded) {
                tracing::warn!(error = %e, "tasks could not move to the app database; they stay in the calendar file");
                store_notices::report(StoreNotice::new(
                    "calendar",
                    NoticeKind::Unreadable,
                    &self.path,
                    "Tasks stay in the calendar file for now",
                    format!("{e}. They are moved again at the next start; nothing is lost."),
                ));
                return Ok((loaded, None));
            }
        }
        loaded.value.tasks = db.load().map_err(|message| CalendarError::Unreadable {
            path: self.path.clone(),
            message,
        })?;
        Ok((loaded, Some(db)))
    }

    fn import_file_tasks(&self, db: &TaskDb, loaded: &Loaded<CalendarData>) -> CalendarResult<()> {
        let kept = self.path.with_file_name(PRE_TASKS_FILE);
        if !kept.exists() && self.path.exists() {
            std::fs::copy(&self.path, &kept).map_err(|source| CalendarError::Io {
                path: kept.clone(),
                source,
            })?;
        }
        let written = db
            .import(&loaded.value.tasks)
            .map_err(CalendarError::Store)?;
        tracing::info!(
            tasks = loaded.value.tasks.len(),
            written,
            "tasks moved to the app database"
        );
        let events_only = CalendarData {
            tasks: Vec::new(),
            events: loaded.value.events.clone(),
        };
        self.write_unlocked(&events_only, &loaded.origin)
    }

    /// The calendar and the file it came from: the backup when the file is
    /// damaged (reported as a notice). Fails when neither can be read;
    /// nothing is written then.
    fn read_unlocked(&self) -> CalendarResult<Loaded<CalendarData>> {
        let loaded = durable_file::read_with_backup(
            &self.path,
            |text| serde_json::from_str::<CalendarData>(text).map_err(|e| e.to_string()),
            CalendarData::default,
        )
        .map_err(|failure| CalendarError::Unreadable {
            path: self.path.clone(),
            message: failure.to_string(),
        })?;
        if let Origin::Backup { problem } = &loaded.origin {
            store_notices::report(StoreNotice::new(
                "calendar",
                NoticeKind::Restored,
                &self.path,
                "Tasks and events restored from the backup",
                format!(
                    "The calendar file could not be read ({problem}), so the copy kept before the \
                     last change was loaded. The latest change may be missing, and a reminder \
                     already shown may ring once more. The damaged file is kept next to it \
                     ({CALENDAR_FILE}.damaged-…) when the calendar is next saved."
                ),
            ));
        }
        Ok(loaded)
    }

    fn write_unlocked(&self, data: &CalendarData, origin: &Origin) -> CalendarResult<()> {
        let text = serde_json::to_string_pretty(data).map_err(|source| CalendarError::Parse {
            path: self.path.clone(),
            source,
        })?;
        durable_file::write_with_backup(&self.path, text.as_bytes(), origin).map_err(|source| {
            CalendarError::Io {
                path: self.path.clone(),
                source,
            }
        })
    }

    /// The calendar file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current tasks and events.
    pub fn load(&self) -> CalendarResult<CalendarData> {
        let _guard = CALENDAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        self.read_all().map(|(loaded, _)| loaded.value)
    }

    /// Apply `change` to the stored data and save it, as the user's change. Nothing is
    /// written when `change` fails.
    pub fn update<R>(
        &self,
        change: impl FnOnce(&mut CalendarData) -> CalendarResult<R>,
    ) -> CalendarResult<R> {
        self.update_as(&Actor::user(), change)
    }

    /// [`Self::update`] as `actor`'s change: each task it changes records who did it.
    pub fn update_as<R>(
        &self,
        actor: &Actor,
        change: impl FnOnce(&mut CalendarData) -> CalendarResult<R>,
    ) -> CalendarResult<R> {
        let _guard = CALENDAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (
            Loaded {
                value: mut data,
                origin,
            },
            db,
        ) = self.read_all()?;
        let Some(db) = db else {
            let result = change(&mut data)?;
            self.write_unlocked(&data, &origin)?;
            return Ok(result);
        };
        let tasks_before = data.tasks.clone();
        let events_before = data.events.clone();
        let result = change(&mut data)?;
        db.save(&tasks_before, &mut data.tasks, actor)
            .map_err(CalendarError::Store)?;
        if data.events != events_before {
            let events_only = CalendarData {
                tasks: Vec::new(),
                events: data.events,
            };
            self.write_unlocked(&events_only, &origin)?;
        }
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
    /// Id of the workspace the task belongs to. Callers check that it exists.
    pub workspace_id: Option<String>,
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
    /// The workspace the task belongs to; callers check that it exists.
    pub workspace_id: Option<Option<String>>,
    pub reminder: Option<Option<String>>,
    /// Replaces the subtask list.
    pub subtasks: Option<Vec<SubtaskSpec>>,
    /// Makes the task a sub-issue of another of its workspace (`Some(None)`: of none).
    pub parent_id: Option<Option<String>>,
    /// Who the task is for.
    pub owner: Option<Owner>,
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
                "workspace_id" => clear_slot(&mut self.workspace_id, name)?,
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
        if is_closed(&v) && !is_closed(&task.status) {
            task.completed_at = Some(now());
        } else if !is_closed(&v) {
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
    if let Some(v) = &patch.workspace_id {
        task.workspace_id = optional_text(v.clone());
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
    if let Some(v) = &patch.parent_id {
        task.parent_id = optional_text(v.clone());
    }
    if let Some(v) = &patch.owner {
        task.owner = Owner {
            kind: v.kind,
            reference: if v.kind == OwnerKind::Person {
                optional_text(v.reference.clone())
            } else {
                None
            },
        };
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
    diff(
        "workspace",
        json!(before.workspace_id),
        json!(task.workspace_id),
    );
    diff("reminder", json!(before.reminder), json!(task.reminder));
    diff(
        "subtasks",
        subtasks_json(&before.subtasks),
        subtasks_json(&task.subtasks),
    );
    diff("parent", json!(before.parent_id), json!(task.parent_id));
    diff("owner", json!(before.owner), json!(task.owner));
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
        // A new task starts at No priority; a stored task without one is Medium, as it
        // always was ([`default_priority`]).
        let priority = match new.priority.as_deref() {
            Some(p) => check_one_of("priority", p, &PRIORITIES)?,
            None => NEW_TASK_PRIORITY.to_string(),
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
            workspace_id: optional_text(new.workspace_id),
            source: new.source.unwrap_or_else(default_source),
            source_ref: new.source_ref,
            created_at: stamp.clone(),
            updated_at: stamp,
            completed_at: None,
            reminder,
            reminder_fired_at: None,
            snoozed_until: None,
            key: None,
            project_id: None,
            parent_id: None,
            owner: Owner::default(),
        };
        self.tasks.push(task.clone());
        Ok(task)
    }

    /// Inserts a task under `id`, with unchecked subtasks titled `subtasks`, unless a task
    /// with that id exists: then that task is returned as it is. True when it was
    /// inserted. Validated as [`CalendarData::insert_task`] validates. A repeating task's
    /// copy for one occurrence always has the same id, so a retry never makes two.
    pub fn insert_task_if_absent(
        &mut self,
        id: &str,
        new: NewTask,
        subtasks: &[String],
    ) -> CalendarResult<(TodoItem, bool)> {
        if let Some(existing) = self.tasks.iter().find(|t| t.id == id) {
            return Ok((existing.clone(), false));
        }
        let titles = subtasks
            .iter()
            .map(|title| check_title(title))
            .collect::<CalendarResult<Vec<String>>>()?;
        let mut task = self.insert_task(new)?;
        task.id = id.to_string();
        task.subtasks = titles
            .into_iter()
            .map(|title| SubTask {
                id: Uuid::new_v4().to_string(),
                title,
                completed: false,
            })
            .collect();
        if let Some(stored) = self.tasks.last_mut() {
            *stored = task.clone();
        }
        Ok((task, true))
    }

    /// Apply `patch` to task `id`; returns the task before and after.
    ///
    /// A task moved to another workspace leaves its project and its parent behind (unless
    /// the patch gives new ones), and its sub-issues stay where they were, without it.
    pub fn patch_task(
        &mut self,
        id: &str,
        patch: &TaskPatch,
    ) -> CalendarResult<(TodoItem, TodoItem, Vec<FieldChange>)> {
        let current = self.task(id)?.clone();
        let workspace_after = match &patch.workspace_id {
            Some(v) => optional_text(v.clone()),
            None => current.workspace_id.clone(),
        };
        if let Some(Some(parent)) = &patch.parent_id {
            self.check_parent(id, parent.trim(), workspace_after.as_deref())?;
        }
        let task = self.task_mut(id)?;
        let before = task.clone();
        let mut changes = apply_task_patch(task, patch)?;
        if before.workspace_id != task.workspace_id {
            if patch.project.is_none() && task.project.is_some() {
                changes.push(FieldChange {
                    field: "project",
                    before: json!(task.project),
                    after: Value::Null,
                });
                task.project = None;
                task.project_id = None;
            }
            if patch.parent_id.is_none() && task.parent_id.is_some() {
                changes.push(FieldChange {
                    field: "parent",
                    before: json!(task.parent_id),
                    after: Value::Null,
                });
                task.parent_id = None;
            }
        }
        let after = task.clone();
        if before.workspace_id != after.workspace_id {
            let stamp = now();
            for child in self.tasks.iter_mut().filter(|t| {
                t.parent_id.as_deref() == Some(id) && t.workspace_id != after.workspace_id
            }) {
                child.parent_id = None;
                child.updated_at = stamp.clone();
            }
        }
        Ok((before, after, changes))
    }

    /// Whether task `id` may be a sub-issue of `parent`, in `workspace`: the parent exists,
    /// is in the same workspace, and is not the task or one of its sub-issues.
    fn check_parent(&self, id: &str, parent: &str, workspace: Option<&str>) -> CalendarResult<()> {
        let title = |task_id: &str| {
            self.tasks
                .iter()
                .find(|t| t.id == task_id)
                .map_or_else(|| task_id.to_string(), |t| t.title.clone())
        };
        let found = self.task(parent)?;
        if found.workspace_id.as_deref() != workspace {
            return Err(CalendarError::Invalid(format!(
                "A sub-issue is in its parent's workspace: {} is in another one.",
                found.title
            )));
        }
        let mut at = Some(parent.to_string());
        let mut steps = 0;
        while let Some(current) = at {
            if current == id {
                return Err(CalendarError::Invalid(format!(
                    "That would make {} a sub-issue of itself.",
                    title(id)
                )));
            }
            steps += 1;
            if steps > self.tasks.len() {
                break;
            }
            at = self
                .tasks
                .iter()
                .find(|t| t.id == current)
                .and_then(|t| t.parent_id.clone());
        }
        Ok(())
    }

    /// Take the tasks of a deleted workspace out of it. The tasks are kept,
    /// with no workspace; returns them as changed.
    pub fn detach_workspace(&mut self, workspace_id: &str) -> Vec<TodoItem> {
        let stamp = now();
        self.tasks
            .iter_mut()
            .filter(|t| t.workspace_id.as_deref() == Some(workspace_id))
            .map(|t| {
                // Its projects go with the workspace; the names stay with the tasks.
                t.workspace_id = None;
                t.project_id = None;
                t.updated_at = stamp.clone();
                t.clone()
            })
            .collect()
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
        let created = now();
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
            created_at: created.clone(),
            updated_at: Some(created),
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
        if !changes.is_empty() {
            event.updated_at = Some(now());
        }
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
    /// Only the tasks of this workspace.
    pub workspace_id: Option<String>,
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
            filter
                .workspace_id
                .as_deref()
                .is_none_or(|w| t.workspace_id.as_deref() == Some(w))
        })
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

    fn task_in(store: &CalendarStore, title: &str, workspace: Option<&str>) -> TodoItem {
        store
            .update(|d| {
                d.insert_task(NewTask {
                    title: title.into(),
                    workspace_id: workspace.map(str::to_string),
                    ..NewTask::default()
                })
            })
            .unwrap()
    }

    #[test]
    fn tasks_written_before_workspaces_load_without_one() {
        let old = r#"{"tasks": [{"id": "t1", "title": "Pay rent", "project": "Home",
                       "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"}]}"#;
        let data: CalendarData = serde_json::from_str(old).unwrap();
        let task = data.task("t1").unwrap();
        assert_eq!(task.workspace_id, None);
        assert_eq!(
            task.project.as_deref(),
            Some("Home"),
            "the project text is kept"
        );
        let text = serde_json::to_string(&data).unwrap();
        assert!(!text.contains("workspaceId"), "absent stays absent");

        let (dir, store) = store();
        let t = task_in(&store, "Draft", Some("ws-1"));
        let raw = std::fs::read_to_string(dir.path().join(CALENDAR_FILE)).unwrap();
        assert!(raw.contains(r#""workspaceId": "ws-1""#), "{raw}");
        assert_eq!(store.load().unwrap().task(&t.id).unwrap(), &t);
    }

    #[test]
    fn task_lists_filter_by_workspace() {
        let (_dir, store) = store();
        let a = task_in(&store, "Read paper", Some("ws-1"));
        let b = task_in(&store, "Write review", Some("ws-2"));
        let c = task_in(&store, "Loose end", None);
        let data = store.load().unwrap();
        let ids = |workspace: Option<&str>| -> Vec<String> {
            let filter = TaskFilter {
                workspace_id: workspace.map(str::to_string),
                ..TaskFilter::default()
            };
            let mut ids: Vec<String> = list_tasks(&data, &filter)
                .iter()
                .map(|t| t.id.clone())
                .collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(Some("ws-1")), vec![a.id.clone()]);
        assert_eq!(ids(Some("ws-2")), vec![b.id.clone()]);
        assert!(ids(Some("ws-3")).is_empty());
        let mut all = vec![a.id, b.id, c.id];
        all.sort();
        assert_eq!(ids(None), all);
    }

    #[test]
    fn workspaces_can_be_set_moved_and_cleared() {
        let (_dir, store) = store();
        let t = store
            .update(|d| {
                d.insert_task(NewTask {
                    title: "Read paper".into(),
                    description: Some("Section 3".into()),
                    project: Some("Thesis".into()),
                    workspace_id: Some("ws-1".into()),
                    ..NewTask::default()
                })
            })
            .unwrap();
        let moved = TaskPatch {
            workspace_id: Some(Some("ws-2".into())),
            ..TaskPatch::default()
        };
        let (_, after, changes) = store.update(|d| d.patch_task(&t.id, &moved)).unwrap();
        assert_eq!(after.workspace_id.as_deref(), Some("ws-2"));
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field, "workspace");
        assert_eq!(changes[0].before, json!("ws-1"));
        assert_eq!(changes[0].after, json!("ws-2"));

        let both = TaskPatch {
            workspace_id: Some(Some("ws-3".into())),
            ..TaskPatch::default()
        }
        .clearing(&["workspace_id".into()]);
        assert!(both.is_err(), "set and cleared at once");

        let clear = TaskPatch::default()
            .clearing(&["workspace_id".into()])
            .unwrap();
        let (_, cleared, changes) = store.update(|d| d.patch_task(&t.id, &clear)).unwrap();
        assert_eq!(cleared.workspace_id, None);
        assert_eq!(cleared.description, "Section 3", "the notes are untouched");
        assert_eq!(cleared.project.as_deref(), Some("Thesis"));
        let fields: Vec<&str> = changes.iter().map(|c| c.field).collect();
        assert_eq!(fields, vec!["workspace"]);
        assert_eq!(
            store.load().unwrap().task(&t.id).unwrap().workspace_id,
            None
        );
    }

    #[test]
    fn insert_if_absent_never_makes_two() {
        let (_dir, store) = store();
        let new = || NewTask {
            title: "Submit timesheet".into(),
            due_date: Some("2026-11-30T17:00".into()),
            ..NewTask::default()
        };
        let subtasks = vec!["Hours".to_string(), "Send".to_string()];
        let (first, inserted) = store
            .update(|d| d.insert_task_if_absent("rep-j-202611301700", new(), &subtasks))
            .unwrap();
        assert!(inserted);
        assert_eq!(first.id, "rep-j-202611301700");
        assert_eq!(first.subtasks.len(), 2);
        assert!(first.subtasks.iter().all(|s| !s.completed));
        let (again, inserted) = store
            .update(|d| d.insert_task_if_absent("rep-j-202611301700", new(), &subtasks))
            .unwrap();
        assert!(!inserted);
        assert_eq!(again, first);
        let data = store.load().unwrap();
        assert_eq!(data.tasks.len(), 1);
        assert_eq!(data.tasks[0], first);
        assert!(store
            .update(|d| d.insert_task_if_absent("rep-j-x", new(), &[" ".to_string()]))
            .is_err());
        assert_eq!(store.load().unwrap().tasks.len(), 1, "nothing written");
    }

    #[test]
    fn deleting_a_workspace_detaches_its_tasks_and_keeps_them() {
        let (_dir, store) = store();
        let a = task_in(&store, "Read paper", Some("ws-1"));
        let b = task_in(&store, "Cite it", Some("ws-1"));
        let other = task_in(&store, "Other", Some("ws-2"));
        let loose = task_in(&store, "Loose", None);
        let detached = store.update(|d| Ok(d.detach_workspace("ws-1"))).unwrap();
        let mut ids: Vec<&str> = detached.iter().map(|t| t.id.as_str()).collect();
        ids.sort_unstable();
        let mut expected = vec![a.id.as_str(), b.id.as_str()];
        expected.sort_unstable();
        assert_eq!(ids, expected);
        assert!(detached.iter().all(|t| t.workspace_id.is_none()));

        let data = store.load().unwrap();
        assert_eq!(data.tasks.len(), 4, "no task is deleted");
        assert_eq!(data.task(&a.id).unwrap().workspace_id, None);
        assert_eq!(data.task(&a.id).unwrap().title, "Read paper");
        assert_eq!(data.task(&b.id).unwrap().workspace_id, None);
        assert_eq!(data.task(&other.id).unwrap(), &other);
        assert_eq!(data.task(&loose.id).unwrap(), &loose);
        let again = store.update(|d| Ok(d.detach_workspace("ws-1"))).unwrap();
        assert!(again.is_empty());
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

    fn titles(data: &CalendarData) -> Vec<&str> {
        data.tasks.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn a_truncated_calendar_loads_the_backup_with_a_notice() {
        let (dir, store) = store();
        task(&store, "First", None);
        task(&store, "Second", None);
        let full = std::fs::read_to_string(store.path()).unwrap();
        std::fs::write(store.path(), &full.as_bytes()[..full.len() / 2]).unwrap();

        assert_eq!(titles(&store.load().unwrap()), ["First"]);
        let path = store.path().display().to_string();
        assert!(crate::store_notices::store_notices()
            .iter()
            .any(|n| n.store == "calendar" && n.path == path && n.kind == NoticeKind::Restored));

        task(&store, "Third", None);
        assert_eq!(titles(&store.load().unwrap()), ["First", "Third"]);
        let kept_aside = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".damaged-"));
        assert!(kept_aside, "the damaged file is kept");
    }

    #[test]
    fn a_damaged_calendar_without_a_backup_is_never_overwritten() {
        let (_dir, store) = store();
        std::fs::write(store.path(), "{\"tasks\": [").unwrap();
        let error = store.load().unwrap_err();
        assert!(matches!(error, CalendarError::Unreadable { .. }), "{error}");
        assert!(error
            .to_string()
            .contains(&store.path().display().to_string()));
        assert!(store
            .update(|d| d.insert_task(NewTask {
                title: "Lost".into(),
                ..NewTask::default()
            }))
            .is_err());
        assert_eq!(
            std::fs::read_to_string(store.path()).unwrap(),
            "{\"tasks\": ["
        );
    }
}
