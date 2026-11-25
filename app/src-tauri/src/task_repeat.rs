//! Repeating tasks: a series of copies of one task, made one at a time by the scheduler
//! engine (`shodh_rag::engine`, a `task_repeat` job).
//!
//! - **The next copy waits until the latest one is done.** Completing the latest copy
//!   (from the Tasks view, the reminder toast, the assistant or the phone: every path ends
//!   in [`on_calendar_change`]) makes the next copy at once. It is due at the next
//!   *upcoming* occurrence after the completion (never an overdue one).
//! - **When an occurrence passes while the latest copy is still open**, no copy is made.
//!   The series asks once, with three answers ([`Choice`]): make the next one, skip this
//!   one, or mark the open one done. Further occurrences fold into that same question.
//! - **A copy's id is `rep-<job>-<yyyymmddhhmm>`** (its local occurrence), inserted only
//!   when absent, so a retry, a crash or two completions at once never make two.
//! - **The series follows its latest copy**: editing it refreshes the snapshot the next
//!   copy is made from; deleting it stops the series; older copies are only history.
//! - A task list that cannot be read is retried on the engine's ladder; after three failed
//!   runs in a row the series is put on hold with an Attention item (its fix: Resume).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shodh_rag::engine::clock::resolve_local;
use shodh_rag::engine::runner::{Engine, Handled, JobHandler, Tick};
use shodh_rag::engine::schedule::{local_key, Schedule};
use shodh_rag::engine::store::{
    parse_instant, CatchUp, CreatedBy, EngineResult, EngineStore, HoldReason, Job, JobKind,
    JobPatch, NewJob, NewRun, Overlap, Run, RunState, Trigger, WaitReason,
};
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};

use crate::agent_tools::CalendarChange;
use crate::calendar_store::{
    parse_moment, parse_reminder, CalendarData, CalendarError, CalendarStore, Moment, NewTask,
    TaskPatch, TodoItem, REMINDER_FORMAT,
};
use crate::reminders::{DueReminder, RepeatAlert};

/// Emitted when a series, its runs or its question changed; the task sheet re-reads them.
pub const REPEAT_CHANGED_EVENT: &str = "task-repeat-changed";
/// The error code of a run that could not read or write the task list.
pub const CALENDAR_UNREADABLE: &str = "calendar_unreadable";
const COPY_PREFIX: &str = "rep-";
/// Most runs shown in a series' history.
pub const MAX_HISTORY: usize = 50;

/// The id of a series' copy for the occurrence whose local key is `key`.
pub fn copy_id(job_id: &str, key: &str) -> String {
    format!("{COPY_PREFIX}{job_id}-{key}")
}

/// The Inbox id of a series' question.
pub fn decision_item_id(job_id: &str) -> String {
    format!("repeat:{job_id}")
}

/// The Inbox id of a series on hold.
pub fn hold_item_id(job_id: &str) -> String {
    format!("repeat-hold:{job_id}")
}

/// The series a copy's id names.
fn job_of_copy(task_id: &str) -> Option<&str> {
    let rest = task_id.strip_prefix(COPY_PREFIX)?;
    let (job, key) = rest.rsplit_once('-')?;
    (key.len() == 12 && key.bytes().all(|b| b.is_ascii_digit()) && !job.is_empty()).then_some(job)
}

/// A task's due time as a local time, and whether it is a date only.
fn due_of(task: &TodoItem) -> Option<(NaiveDateTime, bool)> {
    match parse_moment(task.due_date.as_deref()?)? {
        Moment::Date(day) => Some((day.and_time(NaiveTime::MIN), true)),
        Moment::DateTime(at) => Some((at, false)),
    }
}

fn local_instant(at: NaiveDateTime) -> Option<DateTime<Utc>> {
    resolve_local(&Local, at)
}

fn day_label(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%a %-d %b").to_string()
}

fn is_done(task: &TodoItem) -> bool {
    task.status == "completed"
}

/// What the next copy is made from: the latest copy as it was last saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub priority: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Subtask titles; every copy starts with them unchecked.
    #[serde(default)]
    pub subtasks: Vec<String>,
    /// Due on a date, with no time.
    pub all_day: bool,
    /// The reminder, as minutes before the due time.
    #[serde(default)]
    pub reminder_offset_minutes: Option<i64>,
    /// The first task of the series.
    pub origin_task_id: String,
}

impl Snapshot {
    /// The snapshot of `task`, which must have a due date.
    pub fn of(task: &TodoItem, origin_task_id: &str) -> Option<Self> {
        let (due, all_day) = due_of(task)?;
        let reminder_offset_minutes = task
            .reminder
            .as_deref()
            .and_then(parse_reminder)
            .map(|at| (due - at).num_minutes());
        Some(Self {
            title: task.title.clone(),
            description: task.description.clone(),
            tags: task.tags.clone(),
            priority: task.priority.clone(),
            project: task.project.clone(),
            workspace_id: task.workspace_id.clone(),
            subtasks: task.subtasks.iter().map(|s| s.title.clone()).collect(),
            all_day,
            reminder_offset_minutes,
            origin_task_id: origin_task_id.to_string(),
        })
    }

    fn from_job(job: &Job) -> Option<Self> {
        serde_json::from_value(job.input.clone()).ok()
    }

    /// The fields of the copy due at `due` (local).
    fn new_task(&self, due: NaiveDateTime) -> NewTask {
        let due_date = if self.all_day {
            due.date().format("%Y-%m-%d").to_string()
        } else {
            due.format(REMINDER_FORMAT).to_string()
        };
        let reminder = self.reminder_offset_minutes.map(|minutes| {
            (due - TimeDelta::minutes(minutes))
                .format(REMINDER_FORMAT)
                .to_string()
        });
        NewTask {
            title: self.title.clone(),
            description: Some(self.description.clone()),
            due_date: Some(due_date),
            priority: Some(self.priority.clone()),
            tags: Some(self.tags.clone()),
            project: self.project.clone(),
            workspace_id: self.workspace_id.clone(),
            source: None,
            source_ref: None,
            reminder,
        }
    }
}

/// How `earlier` (an open or finished copy's due time) is named next to `later` (a newer
/// occurrence): "last week's" when it is the one just before, else by its date.
pub fn previous_label(schedule: &Schedule, earlier: DateTime<Utc>, later: DateTime<Utc>) -> String {
    if schedule.next_after(&Local, earlier) == Some(later) {
        match schedule {
            Schedule::Daily { .. } => "yesterday's".to_string(),
            Schedule::Weekly { days, .. } if days.len() == 1 => "last week's".to_string(),
            Schedule::Monthly { .. } => "last month's".to_string(),
            _ => "the last one".to_string(),
        }
    } else {
        format!("the one due {}", day_label(earlier))
    }
}

/// The due time of the copy for `occurrence`: the schedule's own wall-clock time on that
/// date, even when that time does not exist there (a daylight-saving gap rings at the
/// gap's end, as every reminder does). Writing the resolved time (03:00 for a 02:30 on the
/// spring-forward night) would read back as the user moving the series to 03:00.
pub fn nominal_due<Tz: chrono::TimeZone>(
    schedule: &Schedule,
    tz: &Tz,
    occurrence: DateTime<Utc>,
) -> NaiveDateTime {
    let local = occurrence.with_timezone(tz).naive_local();
    match schedule.time_of_day() {
        Some(time) => local.date().and_time(time),
        None => local,
    }
}

/// The schedule after the latest copy's due time became `due` (`all_day`: a date only),
/// or `None` when that is the schedule's own time (as every copy the series makes is).
pub fn moved_schedule(schedule: &Schedule, due: NaiveDateTime, all_day: bool) -> Option<Schedule> {
    let time = if all_day { NaiveTime::MIN } else { due.time() };
    let moved = schedule.clone().at_time(time);
    (&moved != schedule).then_some(moved)
}

/// The kinds of schedule a task can repeat on (days, not hours; once is no repeat).
pub fn task_schedule(schedule: &Schedule) -> Result<(), String> {
    match schedule {
        Schedule::Daily { .. }
        | Schedule::Weekdays { .. }
        | Schedule::Weekly { .. }
        | Schedule::Monthly { .. } => Ok(()),
        Schedule::Once { .. } | Schedule::EveryHours { .. } => Err(
            "A task repeats every day, every weekday, every week on chosen days or every month."
                .to_string(),
        ),
    }
}

/// The user's answer to a series' question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    /// Make the next copy now, leaving the open one as it is.
    Make,
    /// Make no copy for this occurrence; ask again at the next.
    Skip,
    /// Complete the open copy, which makes the next one.
    MarkDone,
}

/// What a repeating task tells the rest of the app.
#[async_trait]
pub trait RepeatSink: Send + Sync {
    /// A copy was made or changed: index it and refresh open views.
    async fn calendar_changed(&self, task: &TodoItem);
    async fn post(&self, item: NewInboxItem);
    async fn resolve(&self, id: &str);
    /// The in-app reminder toast and a desktop notification (Reminders setting).
    fn ring(&self, alert: &DueReminder);
    /// A series, its runs or its question changed.
    fn changed(&self);
}

/// The open question of a series, for the sheet.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionView {
    /// The occurrence that fell due (UTC, RFC 3339).
    pub occurrence: String,
    pub missed: u32,
    pub open_task_id: String,
    /// "Due again today, but last week's isn't done."
    pub detail: String,
}

/// Why a series is on hold.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldView {
    pub reason: &'static str,
    pub detail: String,
}

/// A retry the series waits for.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryView {
    /// When it tries again (UTC, RFC 3339).
    pub at: String,
    pub detail: String,
}

/// A task's series, as the sheet and the assistant show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepeatView {
    pub job_id: String,
    pub schedule: Schedule,
    /// "Every week on Mon"
    pub label: String,
    /// "Repeats every Monday at 17:00"
    pub description: String,
    /// When the next copy falls due (UTC, RFC 3339).
    pub next_due_at: Option<String>,
    /// `active`, `waiting` (a question), `retrying` or `held`.
    pub state: &'static str,
    pub decision: Option<DecisionView>,
    pub hold: Option<HoldView>,
    pub retry: Option<RetryView>,
    /// The task asked about is the series' latest copy.
    pub is_latest: bool,
    pub latest_task_id: String,
}

/// A run of a series, for its history.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    #[serde(flatten)]
    pub run: Run,
    /// The copy this run made, while it exists.
    pub copy_task_id: Option<String>,
}

fn calendar_words(code: Option<&str>, detail: Option<&str>) -> String {
    match code {
        Some(CALENDAR_UNREADABLE) => "the task list couldn't be read".to_string(),
        _ => detail.unwrap_or("something went wrong").to_string(),
    }
}

/// The part that does a run's work and words its questions.
struct Core {
    calendar: CalendarStore,
    engine: Arc<Engine>,
    sink: Arc<dyn RepeatSink>,
    /// While the launch tick runs: the alerts of questions about times missed while Shodh
    /// was closed, which go to the one launch summary instead of ringing each.
    launching: Mutex<Option<Vec<DueReminder>>>,
}

impl Core {
    fn store(&self) -> &Arc<EngineStore> {
        self.engine.store()
    }

    async fn load(&self) -> Result<CalendarData, CalendarError> {
        let store = self.calendar.clone();
        let path = store.path().to_path_buf();
        tokio::task::spawn_blocking(move || store.load())
            .await
            .map_err(|e| CalendarError::Unreadable {
                path,
                message: format!("reading the task list was interrupted: {e}"),
            })?
    }

    async fn update<R: Send + 'static>(
        &self,
        change: impl FnOnce(&mut CalendarData) -> Result<R, CalendarError> + Send + 'static,
    ) -> Result<R, CalendarError> {
        let store = self.calendar.clone();
        let path = store.path().to_path_buf();
        tokio::task::spawn_blocking(move || store.update(change))
            .await
            .map_err(|e| CalendarError::Unreadable {
                path,
                message: format!("saving the task list was interrupted: {e}"),
            })?
    }

    /// The series' latest copy. When it is gone (a calendar restored from its backup), the
    /// newest copy that still exists takes its place; `None` when none does.
    fn subject_of(&self, job: &Job, data: &CalendarData) -> EngineResult<Option<TodoItem>> {
        if let Some(task) = data.tasks.iter().find(|t| t.id == job.subject) {
            return Ok(Some(task.clone()));
        }
        let origin = Snapshot::from_job(job).map(|s| s.origin_task_id);
        let prefix = copy_id(&job.id, "");
        let newest = data
            .tasks
            .iter()
            .filter(|t| t.id.starts_with(&prefix) || Some(&t.id) == origin.as_ref())
            .max_by(|a, b| {
                let key = |t: &TodoItem| (t.id.starts_with(&prefix), t.id.clone());
                key(a).cmp(&key(b))
            })
            .cloned();
        if let Some(task) = &newest {
            self.store().update_job(
                &job.id,
                &JobPatch {
                    subject: Some(task.id.clone()),
                    ..JobPatch::default()
                },
                self.engine.now(),
            )?;
        }
        Ok(newest)
    }

    /// Stops a series: its job and runs go, and so do its Inbox items.
    async fn stop(&self, job: &Job) {
        if let Err(e) = self.store().delete_job(&job.id) {
            tracing::warn!(job = %job.id, error = %e, "a repeating task's series could not be stopped");
            return;
        }
        self.sink.resolve(&decision_item_id(&job.id)).await;
        self.sink.resolve(&hold_item_id(&job.id)).await;
        self.sink.changed();
    }

    /// Makes the copy that follows `latest` and makes it the latest. It is due at the next
    /// occurrence after `latest`'s due time that is still ahead (never an overdue one).
    /// Returns it and its occurrence.
    async fn make_copy(
        &self,
        job: &Job,
        latest: &TodoItem,
    ) -> Result<(TodoItem, DateTime<Utc>), String> {
        let schedule = job
            .schedule
            .clone()
            .ok_or_else(|| "The repeat has no schedule.".to_string())?;
        let snapshot = Snapshot::from_job(job)
            .or_else(|| Snapshot::of(latest, &latest.id))
            .ok_or_else(|| "The task has no due date to repeat from.".to_string())?;
        let after = match due_of(latest).and_then(|(due, _)| local_instant(due)) {
            Some(due) => due,
            None => self
                .engine
                .planned_through(job)
                .map_err(|e| e.to_string())?,
        };
        let target = schedule
            .next_after(&Local, after.max(self.engine.now()))
            .ok_or_else(|| "The repeat has no next date.".to_string())?;
        let id = copy_id(&job.id, &local_key(&Local, target));
        let new = snapshot.new_task(nominal_due(&schedule, &Local, target));
        let subtasks = snapshot.subtasks.clone();
        // A copy made after `latest` that the series did not record yet (a crash between
        // the copy and the record) is the next copy, whatever the time is now.
        let prefix = copy_id(&job.id, "");
        let floor = copy_id(&job.id, &local_key(&Local, after));
        let latest_id = latest.id.clone();
        let (task, inserted) = self
            .update(move |d| {
                let made = d
                    .tasks
                    .iter()
                    .filter(|t| t.id.starts_with(&prefix) && t.id > floor && t.id != latest_id)
                    .max_by(|a, b| a.id.cmp(&b.id))
                    .cloned();
                match made {
                    Some(existing) => Ok((existing, false)),
                    None => d.insert_task_if_absent(&id, new, &subtasks),
                }
            })
            .await
            .map_err(|e| e.to_string())?;
        let target = if inserted {
            target
        } else {
            due_of(&task)
                .and_then(|(due, _)| local_instant(due))
                .unwrap_or(target)
        };
        self.store()
            .update_job(
                &job.id,
                &JobPatch {
                    subject: Some(task.id.clone()),
                    title: Some(task.title.clone()),
                    ..JobPatch::default()
                },
                self.engine.now(),
            )
            .map_err(|e| e.to_string())?;
        if inserted {
            self.sink.calendar_changed(&task).await;
        }
        self.sink.resolve(&decision_item_id(&job.id)).await;
        self.sink.changed();
        Ok((task, target))
    }

    /// The sentence of a series' question about `open` at `occurrence`.
    fn question(
        &self,
        job: &Job,
        open: &TodoItem,
        occurrence: DateTime<Utc>,
        missed: u32,
    ) -> String {
        let open_due = due_of(open).and_then(|(due, _)| local_instant(due));
        if missed > 0 {
            let since = open_due.map_or_else(|| "then".to_string(), day_label);
            return format!(
                "Due again {} times since {since}, but that one isn't done.",
                missed + 1
            );
        }
        let now = self.engine.now().with_timezone(&Local).date_naive();
        let when = if occurrence.with_timezone(&Local).date_naive() == now {
            "today".to_string()
        } else {
            day_label(occurrence)
        };
        let previous = match (&job.schedule, open_due) {
            (Some(schedule), Some(due)) => previous_label(schedule, due, occurrence),
            _ => "the last one".to_string(),
        };
        format!("Due again {when}, but {previous} isn't done.")
    }

    /// Posts (or updates) the series' one question and rings its alert once.
    async fn ask(&self, job: &Job, run: &Run, open: &TodoItem, ring: bool) {
        let occurrence = run.scheduled_at().unwrap_or_else(|| self.engine.now());
        let detail = self.question(job, open, occurrence, run.missed);
        self.sink
            .post(NewInboxItem {
                id: decision_item_id(&job.id),
                kind: InboxKind::Reminder,
                status: InboxStatus::NeedsYou,
                title: open.title.clone(),
                detail: Some(detail.clone()),
                link: Some(InboxLink::new(
                    "tasks",
                    Some(json!({ "kind": "calendar", "taskId": open.id })),
                )),
                data: json!({
                    "taskId": open.id,
                    "jobId": job.id,
                    "choices": ["make", "skip", "mark_done"],
                    "occurrence": run.scheduled_for,
                    "missed": run.missed,
                }),
            })
            .await;
        self.sink.changed();
        if !ring {
            return;
        }
        let alert = DueReminder {
            task_id: open.id.clone(),
            title: open.title.clone(),
            reminder: occurrence
                .with_timezone(&Local)
                .naive_local()
                .format(REMINDER_FORMAT)
                .to_string(),
            due_date: open.due_date.clone(),
            at: occurrence,
            repeat: Some(RepeatAlert {
                job_id: job.id.clone(),
                occurrence: run.scheduled_for.clone().unwrap_or_default(),
                detail,
            }),
        };
        let late = run.trigger == Trigger::CatchUp || run.missed > 0;
        let held_for_launch = {
            let mut launching = self.launching.lock().unwrap_or_else(|e| e.into_inner());
            match launching.as_mut() {
                Some(missed) if late => {
                    missed.push(alert.clone());
                    true
                }
                _ => false,
            }
        };
        if !held_for_launch {
            self.sink.ring(&alert);
        }
    }
}

#[async_trait]
impl JobHandler for Core {
    fn kind(&self) -> JobKind {
        JobKind::TaskRepeat
    }

    async fn run(&self, job: &Job, run: &Run) -> Handled {
        let transient = |e: String| Handled::Transient {
            code: CALENDAR_UNREADABLE.to_string(),
            detail: e,
        };
        let data = match self.load().await {
            Ok(data) => data,
            Err(e) => return transient(e.to_string()),
        };
        let latest = match self.subject_of(job, &data) {
            Ok(Some(task)) => task,
            Ok(None) => {
                self.stop(job).await;
                return Handled::Nothing;
            }
            Err(e) => return transient(e.to_string()),
        };
        if is_done(&latest) {
            let completed_due = due_of(&latest).and_then(|(due, _)| local_instant(due));
            return match self.make_copy(job, &latest).await {
                Ok((_, target)) => {
                    let previous = match (&job.schedule, completed_due) {
                        (Some(schedule), Some(due)) => previous_label(schedule, due, target),
                        _ => "the last one".to_string(),
                    };
                    let when = latest
                        .completed_at
                        .as_deref()
                        .and_then(parse_instant)
                        .map(|at| format!(" ({})", day_label(at)))
                        .unwrap_or_default();
                    Handled::Succeeded {
                        summary: format!("Made when {previous} was done{when}"),
                        scheduled_for: Some(target),
                    }
                }
                Err(e) => transient(e),
            };
        }
        match run.trigger {
            // A completion that found the latest copy open again (a second completion of
            // the copy before it): nothing to do, nothing to record.
            Trigger::Event | Trigger::Manual => Handled::Nothing,
            Trigger::Schedule | Trigger::CatchUp | Trigger::Retry => {
                self.ask(job, run, &latest, true).await;
                Handled::Waiting {
                    reason: WaitReason::Decision,
                }
            }
        }
    }

    async fn folded(&self, job: &Job, run: &Run) {
        let data = match self.load().await {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(job = %job.id, error = %e, "a repeating task's question could not be updated");
                return;
            }
        };
        if let Ok(Some(open)) = self.subject_of(job, &data) {
            self.ask(job, run, &open, true).await;
        }
    }

    async fn held(&self, job: &Job, run: &Run) {
        let reason = calendar_words(run.error_code.as_deref(), run.error_detail.as_deref());
        self.sink
            .post(NewInboxItem {
                id: hold_item_id(&job.id),
                kind: InboxKind::Automation,
                status: InboxStatus::NeedsYou,
                title: job.title.clone(),
                detail: Some(format!(
                    "Stopped repeating: {reason}, three times in a row. Resume it once that is fixed."
                )),
                link: Some(InboxLink::new(
                    "tasks",
                    Some(json!({ "kind": "calendar", "taskId": job.subject })),
                )),
                data: json!({
                    "taskId": job.subject,
                    "jobId": job.id,
                    "fix": "resume",
                }),
            })
            .await;
        self.sink.resolve(&decision_item_id(&job.id)).await;
        self.sink.changed();
    }
}

/// Repeating tasks over the engine and the task list.
pub struct Repeats {
    core: Arc<Core>,
    handler: Arc<dyn JobHandler>,
}

impl std::fmt::Debug for Repeats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repeats").finish_non_exhaustive()
    }
}

impl Repeats {
    pub fn new(engine: Arc<Engine>, calendar: CalendarStore, sink: Arc<dyn RepeatSink>) -> Self {
        let core = Arc::new(Core {
            calendar,
            engine,
            sink,
            launching: Mutex::new(None),
        });
        let handler: Arc<dyn JobHandler> = core.clone();
        Self { core, handler }
    }

    fn store(&self) -> &Arc<EngineStore> {
        self.core.store()
    }

    /// The handler of the series, for the engine's loop.
    pub fn handler(&self) -> Arc<dyn JobHandler> {
        self.handler.clone()
    }

    /// One tick of the series' schedules.
    pub async fn tick(&self) -> EngineResult<Tick> {
        self.core.engine.tick(&self.handler).await
    }

    /// When the next tick should be.
    pub fn next_wake(&self) -> EngineResult<Option<DateTime<Utc>>> {
        self.core.engine.next_wake(JobKind::TaskRepeat)
    }

    /// The launch: finishes what a crash left half done (a completed latest copy with no
    /// next copy), re-posts every open question, and runs the first tick. Returns the
    /// alerts of questions about times missed while Shodh was closed, for the one launch
    /// summary.
    pub async fn launch(&self) -> Vec<DueReminder> {
        *self
            .core
            .launching
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Vec::new());
        if let Err(e) = self.reconcile().await {
            tracing::warn!(error = %e, "repeating tasks could not be checked at launch");
        }
        if let Err(e) = self.tick().await {
            tracing::warn!(error = %e, "repeating tasks could not be planned at launch");
        }
        self.core
            .launching
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .unwrap_or_default()
    }

    async fn reconcile(&self) -> EngineResult<()> {
        let data = match self.core.load().await {
            Ok(data) => data,
            // The first tick's runs wait on the ladder for it.
            Err(_) => return Ok(()),
        };
        for job in self.store().jobs(JobKind::TaskRepeat)? {
            let Some(latest) = self.core.subject_of(&job, &data)? else {
                self.core.stop(&job).await;
                continue;
            };
            let active = self.store().active_run(&job.id)?;
            match active {
                Some(run)
                    if run.state == RunState::Waiting
                        && run.wait_reason == Some(WaitReason::Decision) =>
                {
                    if is_done(&latest) {
                        self.core.engine.event(&self.handler, &job.id).await?;
                    } else {
                        self.core.ask(&job, &run, &latest, false).await;
                    }
                }
                None if is_done(&latest) => {
                    self.core.engine.event(&self.handler, &job.id).await?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A task changed anywhere (the hook of [`on_calendar_change`]).
    pub async fn calendar_changed(&self, change: &CalendarChange) {
        let result = match change {
            CalendarChange::TaskSaved(task) => self.task_saved(task).await,
            CalendarChange::TaskRemoved(id) => {
                match self.store().job_by_subject(JobKind::TaskRepeat, id) {
                    Ok(Some(job)) => {
                        self.core.stop(&job).await;
                        Ok(())
                    }
                    Ok(None) => Ok(()),
                    Err(e) => Err(e),
                }
            }
            CalendarChange::EventSaved(_) | CalendarChange::EventRemoved(_) => Ok(()),
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, "a task change could not be applied to its repeat");
        }
    }

    async fn task_saved(&self, task: &TodoItem) -> EngineResult<()> {
        let Some(job) = self.store().job_by_subject(JobKind::TaskRepeat, &task.id)? else {
            return Ok(());
        };
        let old = Snapshot::from_job(&job);
        let origin = old
            .as_ref()
            .map_or_else(|| task.id.clone(), |s| s.origin_task_id.clone());
        let mut patch = JobPatch::default();
        let mut changed = false;
        if let Some(snapshot) = Snapshot::of(task, &origin) {
            if old.as_ref() != Some(&snapshot) {
                patch.input = Some(serde_json::to_value(&snapshot).unwrap_or(Value::Null));
                changed = true;
            }
            if let (Some(schedule), Some((due, all_day))) = (&job.schedule, due_of(task)) {
                if let Some(moved) = moved_schedule(schedule, due, all_day) {
                    patch.schedule = Some(moved);
                    changed = true;
                }
            }
        }
        if task.title != job.title {
            patch.title = Some(task.title.clone());
            changed = true;
        }
        if changed {
            self.store()
                .update_job(&job.id, &patch, self.core.engine.now())?;
            self.core.sink.changed();
        }
        if is_done(task) {
            self.core.engine.event(&self.handler, &job.id).await?;
        }
        Ok(())
    }

    /// The series of `task_id`: the one it is the latest copy of, an older copy of, or the
    /// first task of.
    fn series_of(&self, task_id: &str) -> EngineResult<Option<Job>> {
        if let Some(job) = self.store().job_by_subject(JobKind::TaskRepeat, task_id)? {
            return Ok(Some(job));
        }
        if let Some(job_id) = job_of_copy(task_id) {
            if let Some(job) = self.store().job(job_id)? {
                return Ok(Some(job));
            }
        }
        Ok(self
            .store()
            .jobs(JobKind::TaskRepeat)?
            .into_iter()
            .find(|job| Snapshot::from_job(job).is_some_and(|s| s.origin_task_id == task_id)))
    }

    /// The series of a task, as the sheet shows it.
    pub async fn get(&self, task_id: &str) -> Result<Option<RepeatView>, String> {
        let Some(job) = self.series_of(task_id).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let Some(schedule) = job.schedule.clone() else {
            return Ok(None);
        };
        let data = self.core.load().await.ok();
        let all_day = Snapshot::from_job(&job).is_some_and(|s| s.all_day);
        let active = self
            .store()
            .active_run(&job.id)
            .map_err(|e| e.to_string())?;
        let next_due_at = self
            .core
            .engine
            .next_due(&job)
            .map_err(|e| e.to_string())?
            .map(|at| at.to_rfc3339());
        let mut view = RepeatView {
            job_id: job.id.clone(),
            label: schedule.label(!all_day),
            description: schedule.describe(!all_day),
            schedule,
            next_due_at,
            state: "active",
            decision: None,
            hold: None,
            retry: None,
            is_latest: job.subject == task_id,
            latest_task_id: job.subject.clone(),
        };
        if let Some(reason) = job.hold_reason {
            let last = self
                .store()
                .runs(&job.id, 1)
                .map_err(|e| e.to_string())?
                .into_iter()
                .next();
            let detail = last.map_or_else(
                || "it was paused".to_string(),
                |run| calendar_words(run.error_code.as_deref(), run.error_detail.as_deref()),
            );
            view.state = "held";
            view.hold = Some(HoldView {
                reason: reason.as_str(),
                detail: format!("Couldn't repeat: {detail}, three times in a row."),
            });
            return Ok(Some(view));
        }
        if let Some(run) = active {
            match run.wait_reason {
                Some(WaitReason::Decision) => {
                    let open = data
                        .as_ref()
                        .and_then(|d| d.tasks.iter().find(|t| t.id == job.subject));
                    let occurrence = run.scheduled_at().unwrap_or_else(|| self.core.engine.now());
                    view.state = "waiting";
                    view.decision = Some(DecisionView {
                        occurrence: run.scheduled_for.clone().unwrap_or_default(),
                        missed: run.missed,
                        open_task_id: job.subject.clone(),
                        detail: open.map_or_else(
                            || "Due again, but the last one isn't done.".to_string(),
                            |open| self.core.question(&job, open, occurrence, run.missed),
                        ),
                    });
                }
                Some(WaitReason::Backoff) => {
                    view.state = "retrying";
                    view.retry = Some(RetryView {
                        at: run.next_attempt_at.clone().unwrap_or_default(),
                        detail: calendar_words(
                            run.error_code.as_deref(),
                            run.error_detail.as_deref(),
                        ),
                    });
                }
                _ => {}
            }
        }
        Ok(Some(view))
    }

    /// Repeats `task_id` on `schedule` (its time of day follows the task's due time), or
    /// changes the schedule of its series. Saving a series that is on hold resumes it.
    pub async fn set(
        &self,
        task_id: &str,
        schedule: Schedule,
        by: CreatedBy,
    ) -> Result<RepeatView, String> {
        let data = self.core.load().await.map_err(|e| e.to_string())?;
        let task = data.task(task_id).map_err(|e| e.to_string())?.clone();
        let (due, all_day) = due_of(&task).ok_or_else(|| {
            "Set a due date first: each copy is due on the next date of the repeat.".to_string()
        })?;
        task_schedule(&schedule)?;
        let time = if all_day { NaiveTime::MIN } else { due.time() };
        let schedule = schedule.normalized().at_time(time);
        schedule.validate().map_err(|e| e.to_string())?;
        let now = self.core.engine.now();
        match self.series_of(task_id).map_err(|e| e.to_string())? {
            Some(job) => {
                self.store()
                    .update_job(
                        &job.id,
                        &JobPatch {
                            schedule: Some(schedule),
                            hold_reason: Some(None),
                            reset_failures: true,
                            ..JobPatch::default()
                        },
                        now,
                    )
                    .map_err(|e| e.to_string())?;
                self.core.sink.resolve(&hold_item_id(&job.id)).await;
                // A latest copy done while the series could not run (on hold, or while the
                // task list could not be read) makes its next copy now.
                let latest_done = data
                    .tasks
                    .iter()
                    .find(|t| t.id == job.subject)
                    .is_some_and(is_done);
                if latest_done {
                    self.core
                        .engine
                        .event(&self.handler, &job.id)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            None => {
                let snapshot = Snapshot::of(&task, &task.id)
                    .ok_or_else(|| "The task has no due date to repeat from.".to_string())?;
                let job = self
                    .store()
                    .create_job(
                        &NewJob {
                            id: uuid::Uuid::new_v4().to_string(),
                            kind: JobKind::TaskRepeat,
                            subject: task.id.clone(),
                            title: task.title.clone(),
                            schedule: Some(schedule),
                            input: serde_json::to_value(&snapshot).map_err(|e| e.to_string())?,
                            overlap: Overlap::Skip,
                            catch_up: CatchUp::Once,
                            max_attempts: 3,
                            created_by: by,
                            provenance: json!({ "taskId": task.id }),
                        },
                        now,
                    )
                    .map_err(|e| e.to_string())?;
                let who = match by {
                    CreatedBy::Agent => "Shodh, with your approval",
                    CreatedBy::User | CreatedBy::System => "you",
                };
                self.store()
                    .insert_run(
                        &NewRun {
                            id: uuid::Uuid::new_v4().to_string(),
                            job_id: job.id.clone(),
                            trigger: Trigger::Manual,
                            scheduled_for: local_instant(due),
                            missed: 0,
                            state: RunState::Succeeded,
                            summary: Some(format!("Repeat turned on by {who}")),
                        },
                        now,
                    )
                    .map_err(|e| e.to_string())?
                    .map_err(|_| "This task already has a repeat in progress.".to_string())?;
            }
        }
        self.refresh_next_due(task_id)?;
        self.core.sink.changed();
        self.get(task_id)
            .await?
            .ok_or_else(|| "The repeat could not be read back.".to_string())
    }

    fn refresh_next_due(&self, task_id: &str) -> Result<(), String> {
        let Some(job) = self.series_of(task_id).map_err(|e| e.to_string())? else {
            return Ok(());
        };
        let next = self
            .core
            .engine
            .next_due(&job)
            .map_err(|e| e.to_string())?
            .map(shodh_rag::engine::lease::timestamp);
        self.store()
            .update_job(
                &job.id,
                &JobPatch {
                    next_due_at: Some(next),
                    ..JobPatch::default()
                },
                self.core.engine.now(),
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Stops the series of `task_id`; every copy stays. True when there was one.
    pub async fn clear(&self, task_id: &str) -> Result<bool, String> {
        let Some(job) = self.series_of(task_id).map_err(|e| e.to_string())? else {
            return Ok(false);
        };
        self.core.stop(&job).await;
        Ok(true)
    }

    /// The history of the series of `task_id`, newest first.
    pub async fn runs(&self, task_id: &str, limit: usize) -> Result<Vec<RunView>, String> {
        let Some(job) = self.series_of(task_id).map_err(|e| e.to_string())? else {
            return Ok(Vec::new());
        };
        let runs = self
            .store()
            .runs(&job.id, limit.clamp(1, MAX_HISTORY))
            .map_err(|e| e.to_string())?;
        let data = self.core.load().await.ok();
        Ok(runs
            .into_iter()
            .map(|run| {
                let copy_task_id = (run.state == RunState::Succeeded
                    && run.trigger != Trigger::Manual)
                    .then(|| run.scheduled_at())
                    .flatten()
                    .map(|at| copy_id(&job.id, &local_key(&Local, at)))
                    .filter(|id| {
                        data.as_ref()
                            .is_some_and(|d| d.tasks.iter().any(|t| &t.id == id))
                    });
                RunView { run, copy_task_id }
            })
            .collect())
    }

    /// The user's answer to the question of series `job_id`.
    pub async fn decide(&self, job_id: &str, choice: Choice) -> Result<(), String> {
        let job = self
            .store()
            .job(job_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "This repeat was stopped.".to_string())?;
        let run = self
            .store()
            .active_run(job_id)
            .map_err(|e| e.to_string())?
            .filter(|r| r.state == RunState::Waiting && r.wait_reason == Some(WaitReason::Decision))
            .ok_or_else(|| "Nothing is waiting for an answer.".to_string())?;
        match choice {
            Choice::Make => {
                let data = self.core.load().await.map_err(|e| e.to_string())?;
                let open = data.task(&job.subject).map_err(|e| e.to_string())?.clone();
                let (_, target) = self.core.make_copy(&job, &open).await?;
                self.core
                    .engine
                    .settle(
                        &self.handler,
                        job_id,
                        &run.id,
                        Handled::Succeeded {
                            summary: "You chose Make the next one (the one before wasn't done)"
                                .to_string(),
                            scheduled_for: Some(target),
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Choice::Skip => {
                self.core
                    .engine
                    .settle(
                        &self.handler,
                        job_id,
                        &run.id,
                        Handled::Skipped {
                            summary: "Skipped by you".to_string(),
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                self.core.sink.resolve(&decision_item_id(job_id)).await;
            }
            Choice::MarkDone => {
                let subject = job.subject.clone();
                let patch = TaskPatch {
                    status: Some("completed".to_string()),
                    ..TaskPatch::default()
                };
                let (_, done, _) = self
                    .core
                    .update(move |d| d.patch_task(&subject, &patch))
                    .await
                    .map_err(|e| e.to_string())?;
                self.core.sink.calendar_changed(&done).await;
                self.calendar_changed(&CalendarChange::TaskSaved(done))
                    .await;
            }
        }
        self.refresh_next_due(&job.subject)?;
        self.core.sink.changed();
        Ok(())
    }

    /// One line per series state, for the assistant's task list: "Repeats every Monday at
    /// 17:00; next due Mon 1 Dec" and the question or hold when there is one.
    pub async fn summary(&self, task_id: &str) -> Option<String> {
        let view = self.get(task_id).await.ok()??;
        let mut line = view.description.clone();
        if let Some(next) = view.next_due_at.as_deref().and_then(parse_instant) {
            line.push_str(&format!("; next due {}", day_label(next)));
        }
        if let Some(decision) = &view.decision {
            line.push_str(&format!(
                "; waiting for the user's answer: {} (repeat_next: make or skip, or complete the open copy {})",
                decision.detail, decision.open_task_id
            ));
        }
        if let Some(hold) = &view.hold {
            line.push_str(&format!("; on hold: {}", hold.detail));
        }
        if !view.is_latest {
            line.push_str(&format!("; the latest copy is {}", view.latest_task_id));
        }
        Some(line)
    }

    /// Whether `task_id` is the latest copy of a series (deleting it stops the series).
    pub fn is_latest_copy(&self, task_id: &str) -> bool {
        self.store()
            .job_by_subject(JobKind::TaskRepeat, task_id)
            .ok()
            .flatten()
            .is_some()
    }
}

// ── Commands ─────────────────────────────────────────────────────

/// The series of a task (any of its copies), or `None` when it does not repeat.
#[tauri::command]
pub async fn task_repeat_get(
    app: tauri::AppHandle,
    task_id: String,
) -> Result<Option<RepeatView>, String> {
    crate::engine_commands::repeats_for_command(&app)
        .await?
        .get(&task_id)
        .await
}

/// Repeats a task on `schedule`, or changes its series (from any copy). Saving a series
/// that is on hold resumes it.
#[tauri::command]
pub async fn task_repeat_set(
    app: tauri::AppHandle,
    task_id: String,
    schedule: Schedule,
) -> Result<RepeatView, String> {
    let view = crate::engine_commands::repeats_for_command(&app)
        .await?
        .set(&task_id, schedule, CreatedBy::User)
        .await?;
    crate::engine_commands::wake(&app);
    Ok(view)
}

/// Stops a task's series; every copy stays. True when it repeated.
#[tauri::command]
pub async fn task_repeat_clear(app: tauri::AppHandle, task_id: String) -> Result<bool, String> {
    let cleared = crate::engine_commands::repeats_for_command(&app)
        .await?
        .clear(&task_id)
        .await?;
    crate::engine_commands::wake(&app);
    Ok(cleared)
}

/// The history of a task's series, newest first (at most [`MAX_HISTORY`]).
#[tauri::command]
pub async fn task_repeat_runs(
    app: tauri::AppHandle,
    task_id: String,
    limit: Option<usize>,
) -> Result<Vec<RunView>, String> {
    crate::engine_commands::repeats_for_command(&app)
        .await?
        .runs(&task_id, limit.unwrap_or(MAX_HISTORY))
        .await
}

/// The user's answer to a series' question.
#[tauri::command]
pub async fn task_repeat_decide(
    app: tauri::AppHandle,
    job_id: String,
    choice: Choice,
) -> Result<(), String> {
    crate::engine_commands::repeats_for_command(&app)
        .await?
        .decide(&job_id, choice)
        .await?;
    crate::engine_commands::wake(&app);
    Ok(())
}

/// Every task change (the UI, the assistant, the phone) reaches the repeats here, from
/// [`crate::agent_tools::TauriEffects::calendar_changed`].
pub fn on_calendar_change(app: &tauri::AppHandle, change: &CalendarChange) {
    let app = app.clone();
    let change = change.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(repeats) = crate::engine_commands::repeats(&app).await {
            repeats.calendar_changed(&change).await;
            crate::engine_commands::wake(&app);
        }
    });
}

/// Repeating tasks for other modules' tests: over a temporary data folder, on a clock the
/// test moves, telling nobody.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use shodh_rag::engine::clock::Clock;

    pub struct TestClock(pub Mutex<DateTime<Utc>>);

    impl TestClock {
        pub fn set(&self, now: DateTime<Utc>) {
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) = now;
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    struct QuietSink;

    #[async_trait]
    impl RepeatSink for QuietSink {
        async fn calendar_changed(&self, _task: &TodoItem) {}
        async fn post(&self, _item: NewInboxItem) {}
        async fn resolve(&self, _id: &str) {}
        fn ring(&self, _alert: &DueReminder) {}
        fn changed(&self) {}
    }

    /// The instant of a local wall-clock time on this computer.
    pub fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        let naive = chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap();
        resolve_local(&Local, naive).unwrap()
    }

    /// Repeats over the task list and `shodh.db` of `dir`, on `clock`.
    pub fn repeats(dir: &std::path::Path, clock: Arc<TestClock>) -> Repeats {
        let store = Arc::new(EngineStore::open(&dir.join("shodh.db"), None).unwrap());
        let engine = Arc::new(Engine::with_clock(store, clock, "agent-test"));
        Repeats::new(engine, CalendarStore::in_dir(dir), Arc::new(QuietSink))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::engine::clock::Clock;

    struct TestClock(Mutex<DateTime<Utc>>);

    impl Clock for TestClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    /// What the series told the Inbox, in order.
    enum InboxOp {
        Post(String),
        Resolve(String),
    }

    #[derive(Default)]
    struct Recorded {
        posted: Vec<NewInboxItem>,
        ops: Vec<InboxOp>,
        rung: Vec<DueReminder>,
        saved: Vec<String>,
    }

    #[derive(Default)]
    struct Sink(Mutex<Recorded>);

    #[async_trait]
    impl RepeatSink for Sink {
        async fn calendar_changed(&self, task: &TodoItem) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .saved
                .push(task.id.clone());
        }
        async fn post(&self, item: NewInboxItem) {
            let mut r = self.0.lock().unwrap_or_else(|e| e.into_inner());
            r.ops.push(InboxOp::Post(item.id.clone()));
            r.posted.push(item);
        }
        async fn resolve(&self, id: &str) {
            self.0
                .lock()
                .unwrap()
                .ops
                .push(InboxOp::Resolve(id.to_string()));
        }
        fn ring(&self, alert: &DueReminder) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .rung
                .push(alert.clone());
        }
        fn changed(&self) {}
    }

    impl Sink {
        /// The Inbox as the posts and removals left it: the ids still there.
        fn open_items(&self) -> Vec<String> {
            let r = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let mut open: Vec<String> = Vec::new();
            for op in &r.ops {
                match op {
                    InboxOp::Post(id) => {
                        if !open.contains(id) {
                            open.push(id.clone());
                        }
                    }
                    InboxOp::Resolve(id) => open.retain(|o| o != id),
                }
            }
            open
        }

        fn rung(&self) -> usize {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).rung.len()
        }

        fn clear(&self) {
            let mut r = self.0.lock().unwrap_or_else(|e| e.into_inner());
            r.posted.clear();
            r.ops.clear();
            r.rung.clear();
        }
    }

    struct Env {
        _dir: tempfile::TempDir,
        calendar: CalendarStore,
        store: Arc<EngineStore>,
        clock: Arc<TestClock>,
        sink: Arc<Sink>,
        repeats: Repeats,
    }

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        let naive = chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap();
        resolve_local(&Local, naive).unwrap()
    }

    fn env(now: DateTime<Utc>) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let calendar = CalendarStore::in_dir(dir.path());
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        let clock = Arc::new(TestClock(Mutex::new(now)));
        let engine = Arc::new(Engine::with_clock(store.clone(), clock.clone(), "app-test"));
        let sink = Arc::new(Sink::default());
        let repeats = Repeats::new(engine, calendar.clone(), sink.clone());
        Env {
            _dir: dir,
            calendar,
            store,
            clock,
            sink,
            repeats,
        }
    }

    impl Env {
        fn at(&self, now: DateTime<Utc>) {
            *self.clock.0.lock().unwrap_or_else(|e| e.into_inner()) = now;
        }

        fn add(&self, title: &str, due: &str) -> TodoItem {
            self.calendar
                .update(|d| {
                    d.insert_task(NewTask {
                        title: title.into(),
                        due_date: Some(due.into()),
                        reminder: None,
                        ..NewTask::default()
                    })
                })
                .unwrap()
        }

        fn tasks(&self) -> Vec<TodoItem> {
            self.calendar.load().unwrap().tasks
        }

        fn task(&self, id: &str) -> TodoItem {
            self.calendar.load().unwrap().task(id).unwrap().clone()
        }

        async fn complete(&self, id: &str) -> TodoItem {
            let patch = TaskPatch {
                status: Some("completed".into()),
                ..TaskPatch::default()
            };
            let (_, done, _) = self.calendar.update(|d| d.patch_task(id, &patch)).unwrap();
            self.repeats
                .calendar_changed(&CalendarChange::TaskSaved(done.clone()))
                .await;
            done
        }

        fn job(&self) -> Job {
            let jobs = self.store.jobs(JobKind::TaskRepeat).unwrap();
            assert_eq!(jobs.len(), 1);
            jobs.into_iter().next().unwrap()
        }

        fn runs(&self) -> Vec<Run> {
            self.store.runs(&self.job().id, 100).unwrap()
        }
    }

    fn weekly_mon() -> Schedule {
        Schedule::Weekly {
            days: vec![shodh_rag::engine::schedule::WeekDay::Mon],
            at: NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
        }
    }

    fn daily() -> Schedule {
        Schedule::Daily {
            at: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
        }
    }

    /// A weekly series of "Submit timesheet", due Monday 23 November 2026 at 17:00, set at
    /// 10:00 that day.
    async fn weekly_series() -> (Env, TodoItem) {
        let e = env(local(2026, 11, 23, 10, 0));
        let task = e.add("Submit timesheet", "2026-11-23T17:00");
        e.repeats
            .set(&task.id, weekly_mon(), CreatedBy::User)
            .await
            .unwrap();
        (e, task)
    }

    #[test]
    fn copy_ids_name_their_series() {
        let id = copy_id("0b6c2f4e-1d2a-4b3c-9d8e-7f6a5b4c3d2e", "202611301700");
        assert_eq!(
            job_of_copy(&id),
            Some("0b6c2f4e-1d2a-4b3c-9d8e-7f6a5b4c3d2e")
        );
        assert_eq!(job_of_copy("rep-j-2026"), None);
        assert_eq!(job_of_copy("0b6c2f4e-1d2a"), None);
    }

    #[tokio::test]
    async fn completing_the_latest_copy_makes_exactly_one_next_copy_at_the_following_occurrence() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 23, 12, 0));
        e.complete(&task.id).await;
        let job = e.job();
        let copy = e.task(&job.subject);
        assert_eq!(copy.id, copy_id(&job.id, "202611301700"));
        assert_eq!(copy.due_date.as_deref(), Some("2026-11-30T17:00"));
        assert_eq!(copy.status, "pending");
        assert_eq!(e.tasks().len(), 2);
        let runs = e.runs();
        assert_eq!(runs.len(), 2, "the turn-on record and the copy");
        assert_eq!(runs[0].state, RunState::Succeeded);
        assert!(runs[0]
            .summary
            .as_deref()
            .is_some_and(|s| s.starts_with("Made when last week's was done")));
    }

    #[tokio::test]
    async fn a_copy_completed_late_is_due_at_the_next_upcoming_occurrence() {
        let (e, task) = weekly_series().await;
        // Monday 30 November passed while Shodh was closed; done on Wednesday 2 December.
        e.at(local(2026, 12, 2, 10, 0));
        e.complete(&task.id).await;
        let copy = e.task(&e.job().subject);
        assert_eq!(
            copy.due_date.as_deref(),
            Some("2026-12-07T17:00"),
            "never overdue when it appears"
        );
        assert_eq!(e.tasks().len(), 2);
    }

    #[tokio::test]
    async fn completing_twice_ui_then_phone_change_makes_one_copy() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 23, 12, 0));
        let done = e.complete(&task.id).await;
        // The phone's update of the same task arrives after the copy was made.
        e.repeats
            .calendar_changed(&CalendarChange::TaskSaved(done))
            .await;
        assert_eq!(e.tasks().len(), 2);
        assert_eq!(e.runs().len(), 2, "exactly one new history row");
        assert!(e.sink.open_items().is_empty(), "no question");
        // A second completion of the newest copy's predecessor, once the newest is open,
        // asks nothing either.
        let job = e.job();
        e.repeats
            .calendar_changed(&CalendarChange::TaskSaved(e.task(&task.id)))
            .await;
        assert_eq!(e.job().subject, job.subject);
        assert_eq!(e.runs().len(), 2);
    }

    #[tokio::test]
    async fn an_occurrence_with_the_last_copy_open_makes_no_copy_and_one_decision() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 30, 17, 1));
        e.repeats.tick().await.unwrap();
        assert_eq!(e.tasks().len(), 1, "no copy");
        let job = e.job();
        assert_eq!(e.sink.open_items(), vec![decision_item_id(&job.id)]);
        let posted = e.sink.0.lock().unwrap_or_else(|e| e.into_inner()).posted[0].clone();
        assert_eq!(posted.kind, InboxKind::Reminder);
        assert_eq!(posted.data["taskId"], json!(task.id));
        assert_eq!(
            posted.detail.as_deref(),
            Some("Due again today, but last week's isn't done.")
        );
        assert_eq!(e.sink.rung(), 1);
        let view = e.repeats.get(&task.id).await.unwrap().unwrap();
        assert_eq!(view.state, "waiting");
        assert_eq!(view.decision.unwrap().open_task_id, task.id);
    }

    #[tokio::test]
    async fn make_the_next_one_creates_the_copy_for_the_next_upcoming_occurrence() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 30, 17, 1));
        e.repeats.tick().await.unwrap();
        e.at(local(2026, 11, 30, 17, 5));
        let job = e.job();
        e.repeats.decide(&job.id, Choice::Make).await.unwrap();
        let job = e.job();
        let copy = e.task(&job.subject);
        assert_eq!(copy.due_date.as_deref(), Some("2026-12-07T17:00"));
        assert_eq!(
            e.task(&task.id).status,
            "pending",
            "the open one is left as it is"
        );
        assert!(e.sink.open_items().is_empty(), "the question is answered");
        let runs = e.runs();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].state, RunState::Succeeded);
        assert!(runs[0]
            .summary
            .as_deref()
            .unwrap()
            .contains("Make the next one"));
        assert!(e.repeats.decide(&job.id, Choice::Make).await.is_err());
    }

    #[tokio::test]
    async fn skip_this_one_records_skipped_and_waits_for_the_next() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 30, 17, 1));
        e.repeats.tick().await.unwrap();
        let job = e.job();
        e.repeats.decide(&job.id, Choice::Skip).await.unwrap();
        assert_eq!(e.tasks().len(), 1);
        assert!(e.sink.open_items().is_empty());
        assert_eq!(e.runs()[0].state, RunState::Skipped);
        e.sink.clear();
        e.at(local(2026, 12, 7, 17, 1));
        e.repeats.tick().await.unwrap();
        assert_eq!(e.sink.open_items(), vec![decision_item_id(&job.id)]);
        assert_eq!(e.task(&task.id).status, "pending");
    }

    #[tokio::test]
    async fn mark_last_done_completes_it_and_makes_the_next_upcoming_copy() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 30, 17, 1));
        e.repeats.tick().await.unwrap();
        let waiting = e.store.active_run(&e.job().id).unwrap().unwrap();
        e.at(local(2026, 11, 30, 17, 5));
        e.repeats
            .decide(&e.job().id, Choice::MarkDone)
            .await
            .unwrap();
        assert_eq!(e.task(&task.id).status, "completed");
        let copy = e.task(&e.job().subject);
        assert_eq!(copy.due_date.as_deref(), Some("2026-12-07T17:00"));
        assert!(e.sink.open_items().is_empty());
        let answered = e.store.run(&waiting.id).unwrap().unwrap();
        assert_eq!(answered.state, RunState::Succeeded);
        assert_eq!(e.runs().len(), 2, "the question's run is the copy's run");
    }

    #[tokio::test]
    async fn an_ignored_decision_across_two_more_occurrences_stays_one_item_and_one_possible_copy()
    {
        let (e, task) = weekly_series().await;
        for day in [(11, 30), (12, 7), (12, 14)] {
            e.at(local(2026, day.0, day.1, 17, 1));
            e.repeats.tick().await.unwrap();
        }
        let job = e.job();
        assert_eq!(e.sink.open_items(), vec![decision_item_id(&job.id)]);
        assert_eq!(e.sink.rung(), 3, "it rings once for each new occurrence");
        let runs = e.runs();
        assert_eq!(runs.len(), 2, "the turn-on record and the one question");
        assert_eq!(runs[0].missed, 2);
        let last = e
            .sink
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .posted
            .last()
            .unwrap()
            .clone();
        assert_eq!(
            last.detail.as_deref(),
            Some("Due again 3 times since Mon 23 Nov, but that one isn't done.")
        );
        e.at(local(2026, 12, 14, 18, 0));
        e.repeats.decide(&job.id, Choice::MarkDone).await.unwrap();
        assert_eq!(e.tasks().len(), 2, "one copy came of it");
        assert_eq!(
            e.task(&e.job().subject).due_date.as_deref(),
            Some("2026-12-21T17:00")
        );
        assert_eq!(e.task(&task.id).status, "completed");
    }

    #[tokio::test]
    async fn a_three_day_sleep_folds_into_the_same_decision() {
        let e = env(local(2026, 11, 23, 8, 0));
        let task = e.add("Water the plants", "2026-11-23T09:00");
        e.repeats
            .set(&task.id, daily(), CreatedBy::User)
            .await
            .unwrap();
        // Shodh was closed from Monday to Thursday morning.
        e.at(local(2026, 11, 26, 10, 0));
        let missed = e.repeats.launch().await;
        assert_eq!(missed.len(), 1, "one alert, for the launch summary");
        assert!(missed[0].repeat.is_some());
        assert_eq!(e.sink.rung(), 0, "nothing rings on its own");
        let runs = e.runs();
        assert_eq!(runs[0].state, RunState::Waiting);
        assert_eq!(runs[0].missed, 2);
        assert_eq!(runs[0].trigger, Trigger::CatchUp);
        assert_eq!(e.tasks().len(), 1);
        // Asleep with Shodh open: the fold rings once.
        e.at(local(2026, 11, 27, 9, 1));
        e.repeats.tick().await.unwrap();
        assert_eq!(e.sink.rung(), 1);
        assert_eq!(e.runs().len(), 2);
    }

    #[tokio::test]
    async fn completing_from_the_phone_answers_the_decision() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 30, 17, 1));
        e.repeats.tick().await.unwrap();
        // The phone's update_task saves the task and reports it like any change.
        e.at(local(2026, 11, 30, 19, 0));
        e.complete(&task.id).await;
        assert!(e.sink.open_items().is_empty());
        assert_eq!(
            e.task(&e.job().subject).due_date.as_deref(),
            Some("2026-12-07T17:00")
        );
        assert_eq!(e.runs().len(), 2);
    }

    #[tokio::test]
    async fn a_crash_mid_run_makes_no_duplicate_copy() {
        let (e, task) = weekly_series().await;
        let start = local(2026, 11, 23, 12, 0);
        e.at(start);
        let patch = TaskPatch {
            status: Some("completed".into()),
            ..TaskPatch::default()
        };
        e.calendar
            .update(|d| d.patch_task(&task.id, &patch))
            .unwrap();
        let job = e.job();
        // A run of another process made the copy, then died before writing its end.
        let run = e
            .store
            .insert_run(
                &NewRun {
                    id: "crashed".into(),
                    job_id: job.id.clone(),
                    trigger: Trigger::Event,
                    scheduled_for: None,
                    missed: 0,
                    state: RunState::Queued,
                    summary: None,
                },
                start,
            )
            .unwrap()
            .unwrap();
        e.store
            .claim(
                &run.id,
                "dead",
                false,
                false,
                start,
                &shodh_rag::engine::lease::lease_cutoff(start),
            )
            .unwrap()
            .unwrap();
        let snapshot = Snapshot::from_job(&job).unwrap();
        let id = copy_id(&job.id, "202611301700");
        let new = snapshot.new_task(
            chrono::NaiveDate::from_ymd_opt(2026, 11, 30)
                .unwrap()
                .and_hms_opt(17, 0, 0)
                .unwrap(),
        );
        e.calendar
            .update(|d| d.insert_task_if_absent(&id, new, &[]))
            .unwrap();
        e.at(start + TimeDelta::minutes(10));
        e.repeats.tick().await.unwrap();
        assert_eq!(e.tasks().len(), 2, "still one copy");
        assert_eq!(e.job().subject, id);
        assert_eq!(
            e.store.run("crashed").unwrap().unwrap().state,
            RunState::Succeeded
        );
    }

    #[tokio::test]
    async fn a_locked_calendar_retries_then_holds_after_three_runs() {
        let (e, task) = weekly_series().await;
        let job = e.job();
        let good = std::fs::read_to_string(e.calendar.path()).unwrap();
        let damage = || {
            std::fs::write(e.calendar.path(), "{\"tasks\": [").unwrap();
            std::fs::write(
                crate::durable_file::backup_path(e.calendar.path()),
                "{\"tasks\": [",
            )
            .unwrap();
        };
        damage();
        let mut now = local(2026, 11, 30, 17, 1);
        for _ in 0..3 {
            e.at(now);
            e.repeats.tick().await.unwrap();
            // Two retries on the ladder (1 and 2 minutes), then the run fails.
            for wait in [1, 2] {
                now += TimeDelta::minutes(wait);
                e.at(now);
                e.repeats.tick().await.unwrap();
            }
            now += TimeDelta::days(7);
        }
        let held = e.store.job(&job.id).unwrap().unwrap();
        assert_eq!(held.hold_reason, Some(HoldReason::Failing));
        assert_eq!(e.sink.open_items(), vec![hold_item_id(&job.id)]);
        let failed: Vec<Run> = e
            .runs()
            .into_iter()
            .filter(|r| r.state == RunState::Failed)
            .collect();
        assert_eq!(failed.len(), 3);
        assert!(failed
            .iter()
            .all(|r| r.error_code.as_deref() == Some(CALENDAR_UNREADABLE)));
        // Fixed and resumed: Resume is saving the repeat again.
        std::fs::write(e.calendar.path(), &good).unwrap();
        let view = e
            .repeats
            .set(&task.id, weekly_mon(), CreatedBy::User)
            .await
            .unwrap();
        assert_eq!(view.state, "active");
        assert!(e.sink.open_items().is_empty());
        assert_eq!(e.store.job(&job.id).unwrap().unwrap().failure_streak, 0);
    }

    #[tokio::test]
    async fn editing_the_latest_copy_refreshes_the_snapshot_and_older_edits_do_not() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 23, 12, 0));
        e.complete(&task.id).await;
        let job = e.job();
        let latest = job.subject.clone();
        let rename = |id: &str, title: &str| {
            let patch = TaskPatch {
                title: Some(title.into()),
                tags: Some(vec!["work".into()]),
                ..TaskPatch::default()
            };
            e.calendar.update(|d| d.patch_task(id, &patch)).unwrap().1
        };
        let older = rename(&task.id, "Old name");
        e.repeats
            .calendar_changed(&CalendarChange::TaskSaved(older))
            .await;
        assert_eq!(
            Snapshot::from_job(&e.job()).unwrap().title,
            "Submit timesheet"
        );
        let newer = rename(&latest, "Submit hours");
        e.repeats
            .calendar_changed(&CalendarChange::TaskSaved(newer))
            .await;
        let snapshot = Snapshot::from_job(&e.job()).unwrap();
        assert_eq!(snapshot.title, "Submit hours");
        assert_eq!(snapshot.tags, vec!["work".to_string()]);
        assert_eq!(snapshot.origin_task_id, task.id);
        assert_eq!(e.job().title, "Submit hours");
    }

    #[tokio::test]
    async fn deleting_the_latest_copy_stops_the_series_and_removes_its_decision() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 30, 17, 1));
        e.repeats.tick().await.unwrap();
        let job = e.job();
        e.calendar.update(|d| d.delete_task(&task.id)).unwrap();
        e.repeats
            .calendar_changed(&CalendarChange::TaskRemoved(task.id.clone()))
            .await;
        assert!(e.store.job(&job.id).unwrap().is_none());
        assert!(e.store.runs(&job.id, 10).unwrap().is_empty());
        assert!(e.sink.open_items().is_empty());
    }

    #[tokio::test]
    async fn deleting_an_older_copy_keeps_the_series() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 23, 12, 0));
        e.complete(&task.id).await;
        let job = e.job();
        e.calendar.update(|d| d.delete_task(&task.id)).unwrap();
        e.repeats
            .calendar_changed(&CalendarChange::TaskRemoved(task.id.clone()))
            .await;
        assert_eq!(e.job().id, job.id);
        assert!(e.repeats.get(&job.subject).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn detaching_a_workspace_clears_it_in_the_snapshot() {
        let e = env(local(2026, 11, 23, 10, 0));
        let task = e
            .calendar
            .update(|d| {
                d.insert_task(NewTask {
                    title: "Read papers".into(),
                    due_date: Some("2026-11-23T17:00".into()),
                    workspace_id: Some("ws-1".into()),
                    ..NewTask::default()
                })
            })
            .unwrap();
        e.repeats
            .set(&task.id, weekly_mon(), CreatedBy::User)
            .await
            .unwrap();
        assert_eq!(
            Snapshot::from_job(&e.job())
                .unwrap()
                .workspace_id
                .as_deref(),
            Some("ws-1")
        );
        let detached = e
            .calendar
            .update(|d| Ok(d.detach_workspace("ws-1")))
            .unwrap();
        for task in detached {
            e.repeats
                .calendar_changed(&CalendarChange::TaskSaved(task))
                .await;
        }
        assert_eq!(Snapshot::from_job(&e.job()).unwrap().workspace_id, None);
    }

    #[tokio::test]
    async fn launch_reconcile_repoints_a_missing_subject_and_finishes_a_half_made_copy() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 23, 12, 0));
        e.complete(&task.id).await;
        let job = e.job();
        let copy = job.subject.clone();
        // The latest copy is gone (the calendar came back from its backup): the newest copy
        // that still exists, the first task, takes its place; it is done, so the next copy
        // is made again.
        e.calendar.update(|d| d.delete_task(&copy)).unwrap();
        e.repeats.launch().await;
        assert_eq!(
            e.job().subject,
            copy,
            "the same occurrence's copy, made once more"
        );
        assert_eq!(e.tasks().len(), 2);
        // A series whose tasks are all gone stops.
        e.calendar
            .update(|d| {
                d.tasks.clear();
                Ok(())
            })
            .unwrap();
        e.repeats.launch().await;
        assert!(e.store.jobs(JobKind::TaskRepeat).unwrap().is_empty());
    }

    #[tokio::test]
    async fn copy_reminders_keep_their_offset_from_due() {
        let e = env(local(2026, 11, 23, 10, 0));
        let task = e
            .calendar
            .update(|d| {
                d.insert_task(NewTask {
                    title: "Standup notes".into(),
                    due_date: Some("2026-11-23T17:00".into()),
                    reminder: Some("2026-11-23T16:30".into()),
                    ..NewTask::default()
                })
            })
            .unwrap();
        e.repeats
            .set(&task.id, weekly_mon(), CreatedBy::User)
            .await
            .unwrap();
        e.at(local(2026, 11, 23, 17, 10));
        e.complete(&task.id).await;
        let copy = e.task(&e.job().subject);
        assert_eq!(copy.reminder.as_deref(), Some("2026-11-30T16:30"));
    }

    #[tokio::test]
    async fn all_day_tasks_repeat_as_dates() {
        let e = env(local(2026, 11, 23, 10, 0));
        let task = e.add("Pay rent", "2026-11-23");
        let view = e
            .repeats
            .set(
                &task.id,
                Schedule::Monthly {
                    day: shodh_rag::engine::schedule::MonthDay::Day(23),
                    at: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                },
                CreatedBy::User,
            )
            .await
            .unwrap();
        assert_eq!(view.label, "Every month on the 23rd");
        e.complete(&task.id).await;
        let copy = e.task(&e.job().subject);
        assert_eq!(copy.due_date.as_deref(), Some("2026-12-23"));
    }

    #[tokio::test]
    async fn set_refuses_a_task_without_a_due_date() {
        let e = env(local(2026, 11, 23, 10, 0));
        let task = e
            .calendar
            .update(|d| {
                d.insert_task(NewTask {
                    title: "Someday".into(),
                    ..NewTask::default()
                })
            })
            .unwrap();
        let err = e
            .repeats
            .set(&task.id, weekly_mon(), CreatedBy::User)
            .await
            .unwrap_err();
        assert!(err.starts_with("Set a due date first"));
        assert!(e.store.jobs(JobKind::TaskRepeat).unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_from_an_older_copy_changes_the_series() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 23, 12, 0));
        e.complete(&task.id).await;
        let view = e
            .repeats
            .set(&task.id, daily(), CreatedBy::User)
            .await
            .unwrap();
        assert!(!view.is_latest);
        assert_eq!(e.store.jobs(JobKind::TaskRepeat).unwrap().len(), 1);
        assert!(matches!(e.job().schedule, Some(Schedule::Daily { .. })));
        let from_copy = e.repeats.get(&e.job().subject).await.unwrap().unwrap();
        assert!(from_copy.is_latest);
        assert_eq!(from_copy.label, "Every day at 17:00");
    }

    #[tokio::test]
    async fn clear_keeps_copies() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 23, 12, 0));
        e.complete(&task.id).await;
        assert!(e.repeats.clear(&task.id).await.unwrap());
        assert_eq!(e.tasks().len(), 2);
        assert!(e.store.jobs(JobKind::TaskRepeat).unwrap().is_empty());
        assert!(!e.repeats.clear(&task.id).await.unwrap());
    }

    #[tokio::test]
    async fn history_links_the_copies_runs_made() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 23, 12, 0));
        e.complete(&task.id).await;
        let runs = e.repeats.runs(&task.id, 10).await.unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(
            runs[0].copy_task_id.as_deref(),
            Some(e.job().subject.as_str())
        );
        assert_eq!(
            runs[1].copy_task_id, None,
            "the turn-on record made no copy"
        );
        assert_eq!(
            runs[1].run.summary.as_deref(),
            Some("Repeat turned on by you")
        );
        let json = serde_json::to_value(&runs[0]).unwrap();
        assert!(json.get("copyTaskId").is_some());
        assert!(json.get("scheduledFor").is_some());
        assert!(json.get("runToken").is_none());
    }

    #[test]
    fn a_dst_gap_never_moves_the_series_time() {
        use chrono::TimeZone;
        use chrono_tz::America::New_York;
        let at = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        let day = |d, h, m| {
            chrono::NaiveDate::from_ymd_opt(2027, 3, d)
                .unwrap()
                .and_time(at(h, m))
        };
        let schedule = Schedule::Daily { at: at(2, 30) };
        // New York springs forward on Sunday 14 March 2027: 02:30 does not exist and the
        // occurrence is 03:00 EDT (07:00 UTC).
        let target = schedule
            .next_after(
                &New_York,
                Utc.with_ymd_and_hms(2027, 3, 13, 12, 0, 0).unwrap(),
            )
            .unwrap();
        assert_eq!(target, Utc.with_ymd_and_hms(2027, 3, 14, 7, 0, 0).unwrap());
        // The copy keeps the series' own time, so saving it changes nothing.
        let due = nominal_due(&schedule, &New_York, target);
        assert_eq!(due, day(14, 2, 30));
        assert_eq!(moved_schedule(&schedule, due, false), None);
        let resolved = target.with_timezone(&New_York).naive_local();
        assert_eq!(resolved, day(14, 3, 0));
        assert!(
            moved_schedule(&schedule, resolved, false).is_some(),
            "the resolved time would have read as the user moving the series"
        );
        // The day after, the series is still at 02:30.
        let after = resolve_local(&New_York, due).unwrap();
        let next = schedule.next_after(&New_York, after).unwrap();
        assert_eq!(nominal_due(&schedule, &New_York, next), day(15, 2, 30));
        // A user moving a copy to 04:00 moves the series.
        assert_eq!(
            moved_schedule(&schedule, day(15, 4, 0), false),
            Some(Schedule::Daily { at: at(4, 0) })
        );
        assert_eq!(
            moved_schedule(&schedule, day(15, 4, 0), true),
            Some(Schedule::Daily { at: at(0, 0) }),
            "a date-only copy repeats at the start of the day"
        );
    }

    #[tokio::test]
    async fn moving_the_latest_copy_to_another_time_moves_the_series() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 23, 12, 0));
        e.complete(&task.id).await;
        // The copy the series made reads back as its own time: nothing moves.
        let job = e.job();
        assert_eq!(job.schedule, Some(weekly_mon()));
        let patch = TaskPatch {
            due_date: Some(Some("2026-11-30T18:00".into())),
            ..TaskPatch::default()
        };
        let (_, moved, _) = e
            .calendar
            .update(|d| d.patch_task(&job.subject, &patch))
            .unwrap();
        e.repeats
            .calendar_changed(&CalendarChange::TaskSaved(moved))
            .await;
        assert_eq!(
            e.job().schedule,
            Some(Schedule::Weekly {
                days: vec![shodh_rag::engine::schedule::WeekDay::Mon],
                at: NaiveTime::from_hms_opt(18, 0, 0).unwrap(),
            })
        );
    }

    #[tokio::test]
    async fn resume_makes_the_copy_missed_while_held() {
        let (e, task) = weekly_series().await;
        let job = e.job();
        e.store
            .update_job(
                &job.id,
                &JobPatch {
                    hold_reason: Some(Some(HoldReason::Failing)),
                    ..JobPatch::default()
                },
                local(2026, 11, 23, 11, 0),
            )
            .unwrap();
        e.at(local(2026, 11, 23, 12, 0));
        e.complete(&task.id).await;
        assert_eq!(e.tasks().len(), 1, "a series on hold makes nothing");
        // Resumed on Wednesday: the copy is made, due at the next upcoming Monday.
        e.at(local(2026, 11, 25, 9, 0));
        let view = e
            .repeats
            .set(&task.id, weekly_mon(), CreatedBy::User)
            .await
            .unwrap();
        assert_eq!(view.state, "active");
        assert_eq!(e.tasks().len(), 2);
        assert_eq!(
            e.task(&e.job().subject).due_date.as_deref(),
            Some("2026-11-30T17:00")
        );
    }

    #[tokio::test]
    async fn a_copy_made_before_a_crash_is_adopted_even_after_its_date() {
        let (e, task) = weekly_series().await;
        e.at(local(2026, 11, 23, 12, 0));
        let patch = TaskPatch {
            status: Some("completed".into()),
            ..TaskPatch::default()
        };
        e.calendar
            .update(|d| d.patch_task(&task.id, &patch))
            .unwrap();
        let job = e.job();
        // The copy was saved, then Shodh stopped before the series recorded it.
        let snapshot = Snapshot::from_job(&job).unwrap();
        let id = copy_id(&job.id, "202611301700");
        let new = snapshot.new_task(
            chrono::NaiveDate::from_ymd_opt(2026, 11, 30)
                .unwrap()
                .and_hms_opt(17, 0, 0)
                .unwrap(),
        );
        e.calendar
            .update(|d| d.insert_task_if_absent(&id, new, &[]))
            .unwrap();
        // Shodh opens again after that Monday: the copy is taken up, not made again.
        e.at(local(2026, 12, 2, 10, 0));
        e.repeats.launch().await;
        assert_eq!(e.tasks().len(), 2, "no second copy");
        assert_eq!(e.job().subject, id);
    }

    #[tokio::test]
    async fn tasks_repeat_on_days_not_hours_or_once() {
        let e = env(local(2026, 11, 23, 10, 0));
        let task = e.add("Stretch", "2026-11-23T17:00");
        let hourly = Schedule::EveryHours {
            hours: 2,
            from: local(2026, 11, 23, 10, 0),
        };
        assert!(e
            .repeats
            .set(&task.id, hourly, CreatedBy::User)
            .await
            .is_err());
        let once = Schedule::Once {
            at: chrono::NaiveDate::from_ymd_opt(2026, 11, 30)
                .unwrap()
                .and_hms_opt(17, 0, 0)
                .unwrap(),
        };
        assert!(e
            .repeats
            .set(&task.id, once, CreatedBy::User)
            .await
            .is_err());
        assert!(e.store.jobs(JobKind::TaskRepeat).unwrap().is_empty());
    }
}
