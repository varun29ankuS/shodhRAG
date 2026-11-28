//! Task reminders: an in-app scheduler that shows a native notification
//! when a task's reminder comes due, while Shodh runs (also hidden in the
//! tray). Phone delivery is out of scope.
//!
//! - **Data**: `reminder` (and `snoozed_until`) are local wall-clock times
//!   ([`REMINDER_FORMAT`]); `reminder_fired_at` records that one was shown.
//!   See [`crate::calendar_store`].
//! - **Scheduling**: a system job of the engine's one loop
//!   ([`crate::system_jobs`], `reminder_plan`), due when the next reminder
//!   rings and at once after every calendar change ([`wake`]); the loop looks
//!   at the clock at least every [`MAX_SLEEP`], so a suspended computer or a
//!   clock change is noticed within a minute. Local times are converted to
//!   instants on every plan, never cached, and a change of time zone plans
//!   again at once. Without the engine (`shodh.db` cannot be opened) the loop
//!   runs the plan itself: reminders need only the calendar file.
//! - **Firing**: the task is marked fired in the calendar file first and the
//!   notification shown only if that write changed something, so a reminder
//!   is shown once even if two plans race.
//! - **Missed while closed**: reminders that came due before this launch are
//!   marked fired without one notification each; the UI lists them once
//!   ([`list_missed_reminders`] / [`dismiss_missed_reminders`]) and a single
//!   native notification says how many there were.
//! - **Missed while asleep**: when the computer wakes with more than one
//!   reminder [`LATE_AFTER_MINUTES`] late, they are told the same way, in one
//!   notification ([`after_sleep`]); one late reminder rings as usual.
//!
//! Notifications go through [`crate::notifications`] (category: reminders).
//! What Windows supports (tauri-plugin-notification 2.3, desktop): a title
//! and a body. The plugin's action types and click callbacks are mobile
//! only, so a toast cannot carry Snooze / Mark done buttons and clicking it
//! does not reach the app. Those actions live in the in-app reminder toast
//! ([`REMINDER_FIRED_EVENT`]), which waits until the window is shown again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, TimeZone, Timelike, Utc};
use serde::Serialize;
pub use shodh_rag::engine::clock::resolve_local;
pub use shodh_rag::engine::plan::{sleep_for, LATE_AFTER_MINUTES, MAX_SLEEP};
use shodh_rag::engine::runner::Handled;
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::calendar_commands::CALENDAR_CHANGED_EVENT;
use crate::calendar_store::{
    parse_reminder, CalendarData, CalendarError, CalendarStore, REMINDER_FORMAT,
};
use crate::system_jobs::{SystemWork, WorkDone};

/// Snooze length when the UI does not say.
pub const DEFAULT_SNOOZE_MINUTES: u32 = 10;
/// Longest snooze (one day).
pub const MAX_SNOOZE_MINUTES: u32 = 24 * 60;

/// Emitted with a [`DueReminder`] when a reminder rings.
pub const REMINDER_FIRED_EVENT: &str = "reminder-fired";
/// Emitted with the list of [`DueReminder`]s missed while the app was closed.
pub const REMINDERS_MISSED_EVENT: &str = "reminders-missed";

/// A reminder that is due (or will be).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DueReminder {
    pub task_id: String,
    pub title: String,
    /// The stored local time that rings (`snoozed_until`, else `reminder`).
    pub reminder: String,
    pub due_date: Option<String>,
    /// When it rings.
    pub at: DateTime<Utc>,
    /// Set when this is a repeating task's question rather than its own reminder: the
    /// next one fell due while this copy is still open (the toast offers the answers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<RepeatAlert>,
}

/// The question of a repeating task, carried by its reminder alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepeatAlert {
    pub job_id: String,
    /// The occurrence that fell due (UTC, RFC 3339).
    pub occurrence: String,
    /// What happened, in words: "Due again today, but last week's isn't done."
    pub detail: String,
}

/// Who reports what was missed while Shodh was closed: the reminders and the repeating
/// tasks each report once at launch, and one notification says it for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchPart {
    Reminders,
    Repeats,
}

/// The launch's missed lists, until both have come in.
#[derive(Debug, Default)]
pub struct LaunchMissed {
    reminders: Option<Vec<DueReminder>>,
    repeats: Option<Vec<DueReminder>>,
    told: bool,
}

impl LaunchMissed {
    /// Takes `part`'s list. Returns what to announce now: nothing until both parts have
    /// reported, then both lists together once; a part that reports again after that
    /// (the reminders, once an unreadable calendar can be read) is announced alone.
    pub fn report(&mut self, part: LaunchPart, list: Vec<DueReminder>) -> Option<Vec<DueReminder>> {
        if self.told {
            return (!list.is_empty()).then_some(list);
        }
        let slot = match part {
            LaunchPart::Reminders => &mut self.reminders,
            LaunchPart::Repeats => &mut self.repeats,
        };
        slot.get_or_insert_with(Vec::new).extend(list);
        if self.reminders.is_none() || self.repeats.is_none() {
            return None;
        }
        self.told = true;
        let mut all = self.reminders.take().unwrap_or_default();
        all.extend(self.repeats.take().unwrap_or_default());
        (!all.is_empty()).then_some(all)
    }
}

/// Reminders not yet shown, of tasks that are not done, in `tz`. Values that
/// are not a date-time (written before reminders were validated) are skipped.
pub fn pending<Tz: TimeZone>(data: &CalendarData, tz: &Tz) -> Vec<DueReminder> {
    let mut out: Vec<DueReminder> = data
        .tasks
        .iter()
        .filter(|t| !crate::calendar_store::is_closed(&t.status) && t.reminder_fired_at.is_none())
        .filter_map(|t| {
            let reminder = t.snoozed_until.as_deref().or(t.reminder.as_deref())?;
            let at = resolve_local(tz, parse_reminder(reminder)?)?;
            Some(DueReminder {
                task_id: t.id.clone(),
                title: t.title.clone(),
                reminder: reminder.to_string(),
                due_date: t.due_date.clone(),
                at,
                repeat: None,
            })
        })
        .collect();
    out.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.task_id.cmp(&b.task_id)));
    out
}

/// What to do now and when to look again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Came due before the app started: list once, do not ring each.
    pub missed: Vec<DueReminder>,
    /// Ring now.
    pub due: Vec<DueReminder>,
    /// The next reminder after `now`.
    pub next_wake: Option<DateTime<Utc>>,
}

/// Split `pending` at `now`. On the first plan after launch, pass the launch
/// time as `launched_at`: reminders due before it were missed while the app
/// was closed.
pub fn plan(
    pending: Vec<DueReminder>,
    now: DateTime<Utc>,
    launched_at: Option<DateTime<Utc>>,
) -> Plan {
    let mut out = Plan::default();
    for reminder in pending {
        if reminder.at > now {
            out.next_wake = Some(out.next_wake.map_or(reminder.at, |n| n.min(reminder.at)));
        } else if launched_at.is_some_and(|launch| reminder.at < launch) {
            out.missed.push(reminder);
        } else {
            out.due.push(reminder);
        }
    }
    out
}

/// The reminders of a plan's `due` to ring now, and those to tell in one notification:
/// more than one reminder more than [`LATE_AFTER_MINUTES`] late means the computer slept
/// through them (or Shodh could not run), and a notification each would bury the user.
/// One late reminder rings as usual.
pub fn after_sleep(
    due: Vec<DueReminder>,
    now: DateTime<Utc>,
) -> (Vec<DueReminder>, Vec<DueReminder>) {
    let late_after = chrono::TimeDelta::minutes(LATE_AFTER_MINUTES);
    let (late, on_time): (Vec<DueReminder>, Vec<DueReminder>) =
        due.into_iter().partition(|r| now - r.at > late_after);
    if late.len() < 2 {
        let mut ring = on_time;
        ring.extend(late);
        ring.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.task_id.cmp(&b.task_id)));
        return (ring, Vec::new());
    }
    (on_time, late)
}

/// The local time a snooze of `minutes` from `now` rings at, rounded up to
/// a whole minute (stored times have no seconds, and rounding down would
/// ring early).
pub fn snooze_until(now: NaiveDateTime, minutes: u32) -> NaiveDateTime {
    let target = now + chrono::Duration::minutes(i64::from(minutes));
    let whole = target
        .with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(target);
    if whole < target {
        whole + chrono::Duration::minutes(1)
    } else {
        whole
    }
}

// ── Runtime ──────────────────────────────────────────────────────

/// Scheduler state, managed by Tauri.
pub struct ReminderState {
    /// The calendar changed since the last plan: it is read again.
    changed: AtomicBool,
    planner: tokio::sync::Mutex<Planner>,
    missed: Mutex<Vec<DueReminder>>,
    launch: Mutex<LaunchMissed>,
}

/// What the plans of this launch carry from one to the next.
struct Planner {
    launched_at: DateTime<Utc>,
    /// No plan has run yet: reminders due before the launch were missed while closed.
    first: bool,
    /// The calendar as last read, while nothing was written to it.
    cached: Option<CalendarData>,
    /// Reads of the calendar that failed in a row.
    failures: u32,
}

impl Default for ReminderState {
    fn default() -> Self {
        Self {
            changed: AtomicBool::new(false),
            planner: tokio::sync::Mutex::new(Planner {
                launched_at: Utc::now(),
                first: true,
                cached: None,
                failures: 0,
            }),
            missed: Mutex::new(Vec::new()),
            launch: Mutex::new(LaunchMissed::default()),
        }
    }
}

/// Plan again now (a reminder may have been set, moved or removed). A no-op before
/// the reminders' state exists.
pub fn wake(app: &AppHandle) {
    if let Some(state) = app.try_state::<ReminderState>() {
        state.changed.store(true, Ordering::SeqCst);
    }
    crate::engine_commands::due_now(app, SystemWork::ReminderPlan);
}

async fn load(store: &CalendarStore) -> Result<CalendarData, CalendarError> {
    let path = store.path().to_path_buf();
    let store = store.clone();
    tokio::task::spawn_blocking(move || store.load())
        .await
        .map_err(|e| CalendarError::Unreadable {
            path,
            message: format!("reading the calendar was interrupted: {e}"),
        })?
}

/// Mark `reminder` fired; true when this call did it.
async fn mark_fired(store: &CalendarStore, reminder: &DueReminder) -> bool {
    let store = store.clone();
    let task_id = reminder.task_id.clone();
    let due = reminder.reminder.clone();
    let fired_at = Utc::now().to_rfc3339();
    let result = tokio::task::spawn_blocking(move || {
        store.update(|d| d.mark_reminder_fired(&task_id, &due, &fired_at))
    })
    .await;
    match result {
        Ok(Ok(marked)) => marked,
        // Deleted since it was planned: nothing to remind about.
        Ok(Err(CalendarError::TaskNotFound(_))) => false,
        Ok(Err(e)) => {
            tracing::warn!(task_id = %reminder.task_id, error = %e, "could not record a reminder as shown");
            false
        }
        Err(e) => {
            tracing::warn!(error = %e, "recording a reminder was interrupted");
            false
        }
    }
}

/// A desktop notification of the reminders category: shown while the window is not
/// in front (the in-app reminder toast covers it otherwise) and the user has not
/// turned reminders off in Settings → Notifications.
fn notify(app: &AppHandle, title: &str, body: &str) {
    crate::notifications::notify(app, crate::notifications::Category::Reminders, title, body);
}

fn due_text(reminder: &DueReminder) -> String {
    match reminder.due_date.as_deref() {
        Some(due) => format!("{} (due {})", reminder.title, due.replace('T', " ")),
        None => reminder.title.clone(),
    }
}

/// The Inbox item of a reminder that rang (or was missed while Shodh was closed):
/// it waits for the user until dismissed; Open shows the task.
pub fn inbox_item(reminder: &DueReminder, missed: bool) -> NewInboxItem {
    NewInboxItem {
        id: format!("reminder:{}", reminder.task_id),
        kind: InboxKind::Reminder,
        status: InboxStatus::NeedsYou,
        title: reminder.title.clone(),
        detail: Some(match (missed, reminder.due_date.as_deref()) {
            (true, Some(due)) => format!("Missed reminder, due {}", due.replace('T', " ")),
            (true, None) => "Missed reminder".to_string(),
            (false, Some(due)) => format!("Reminder, due {}", due.replace('T', " ")),
            (false, None) => "Reminder".to_string(),
        }),
        link: Some(InboxLink::new(
            "tasks",
            Some(serde_json::json!({ "kind": "calendar", "taskId": reminder.task_id })),
        )),
        data: serde_json::json!({ "taskId": reminder.task_id }),
    }
}

async fn ring(app: &AppHandle, store: &CalendarStore, reminder: DueReminder) {
    if !mark_fired(store, &reminder).await {
        return;
    }
    notify(app, "Reminder", &due_text(&reminder));
    crate::inbox_commands::post(app, inbox_item(&reminder, false)).await;
    if let Err(e) = app.emit(REMINDER_FIRED_EVENT, &reminder) {
        tracing::warn!(error = %e, "could not emit {}", REMINDER_FIRED_EVENT);
    }
    if let Err(e) = app.emit(CALENDAR_CHANGED_EVENT, ()) {
        tracing::warn!(error = %e, "could not emit {}", CALENDAR_CHANGED_EVENT);
    }
}

/// Marks `missed` fired and posts each to the Inbox as missed; returns those this call
/// marked.
async fn mark_missed(
    app: &AppHandle,
    store: &CalendarStore,
    missed: Vec<DueReminder>,
) -> Vec<DueReminder> {
    let mut recorded = Vec::with_capacity(missed.len());
    for reminder in missed {
        if mark_fired(store, &reminder).await {
            recorded.push(reminder);
        }
    }
    for reminder in &recorded {
        crate::inbox_commands::post(app, inbox_item(reminder, true)).await;
    }
    recorded
}

async fn record_missed(app: &AppHandle, store: &CalendarStore, missed: Vec<DueReminder>) {
    let recorded = mark_missed(app, store, missed).await;
    report_launch(app, LaunchPart::Reminders, recorded);
}

/// Reminders the computer slept through: one notification for all of them, and the
/// in-app list, as for those missed while Shodh was closed.
async fn record_late(app: &AppHandle, store: &CalendarStore, late: Vec<DueReminder>) {
    let recorded = mark_missed(app, store, late).await;
    if let Some(state) = app.try_state::<ReminderState>() {
        announce_missed(app, &state, "Missed reminders", recorded);
    }
}

/// Hands `part`'s list of what was missed while Shodh was closed to the launch summary:
/// once both the reminders and the repeating tasks have reported, one desktop
/// notification says it for both, and the in-app list shows them.
pub fn report_launch(app: &AppHandle, part: LaunchPart, list: Vec<DueReminder>) {
    let Some(state) = app.try_state::<ReminderState>() else {
        return;
    };
    let announce = state
        .launch
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .report(part, list);
    let Some(recorded) = announce else {
        return;
    };
    announce_missed(app, &state, "Missed while Shodh was closed", recorded);
}

/// One desktop notification titled `title` for every reminder of `recorded`, which join
/// the in-app list of missed reminders.
fn announce_missed(
    app: &AppHandle,
    state: &ReminderState,
    title: &str,
    recorded: Vec<DueReminder>,
) {
    if recorded.is_empty() {
        return;
    }
    let body = match recorded.as_slice() {
        [one] => match &one.repeat {
            Some(repeat) => format!("{}: {}", one.title, repeat.detail),
            None => due_text(one),
        },
        many => format!("{} reminders. Open Shodh to see them.", many.len()),
    };
    notify(app, title, &body);
    let list = {
        let mut kept = state.missed.lock().unwrap_or_else(|e| e.into_inner());
        kept.extend(recorded);
        kept.clone()
    };
    if let Err(e) = app.emit(REMINDERS_MISSED_EVENT, &list) {
        tracing::warn!(error = %e, "could not emit {}", REMINDERS_MISSED_EVENT);
    }
    if let Err(e) = app.emit(CALENDAR_CHANGED_EVENT, ()) {
        tracing::warn!(error = %e, "could not emit {}", CALENDAR_CHANGED_EVENT);
    }
}

/// Longest wait between two reads of a calendar that cannot be read.
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(30 * 60);

/// How long to wait before reading an unreadable calendar again after
/// `failures` failed reads in a row: a minute, doubling, at most
/// [`MAX_RETRY_DELAY`].
pub fn retry_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    MAX_SLEEP
        .saturating_mul(1u32 << doublings)
        .min(MAX_RETRY_DELAY)
}

/// The calendar cannot be read (the file and its backup are damaged, or it
/// cannot be opened): reminders cannot ring. Said once, in plain words.
fn calendar_unreadable(store: &CalendarStore, error: &CalendarError) {
    tracing::warn!(error = %error, "reminders: calendar unreadable; retrying with a growing wait");
    crate::store_notices::report(crate::store_notices::StoreNotice::new(
        "calendar",
        crate::store_notices::NoticeKind::Unreadable,
        store.path(),
        "Tasks and reminders could not be loaded",
        format!(
            "{error}. Reminders cannot ring until it can be read. Nothing is written to the file \
             in the meantime, so it stays as it is for you to restore or move away."
        ),
    ));
}

/// One plan of the reminders (the `reminder_plan` system job): the reminders due ring,
/// those missed while Shodh was closed (on the first plan) or while the computer slept are
/// told once, and the work is next due when the next reminder rings. An unreadable
/// calendar is read again after a growing wait ([`retry_delay`]), and at once after a
/// calendar change.
pub async fn system_plan(app: &AppHandle) -> WorkDone {
    let Some(state) = app.try_state::<ReminderState>() else {
        return WorkDone::nothing(None);
    };
    let mut planner = state.planner.lock().await;
    let store = match crate::profile::app_data_dir(app) {
        Ok(dir) => CalendarStore::in_dir(&dir),
        Err(e) => {
            if planner.first {
                planner.first = false;
                tracing::error!(error = %e, "reminders are off: no app data directory");
                report_launch(app, LaunchPart::Reminders, Vec::new());
            }
            return WorkDone::nothing(None);
        }
    };
    if state.changed.swap(false, Ordering::SeqCst) {
        planner.cached = None;
    }
    let data = match planner.cached.take() {
        Some(data) => Ok(data),
        None => load(&store).await,
    };
    match data {
        Ok(data) => {
            planner.failures = 0;
            let now = Utc::now();
            let plan = plan(
                pending(&data, &chrono::Local),
                now,
                planner.first.then_some(planner.launched_at),
            );
            let wrote = !plan.missed.is_empty() || !plan.due.is_empty();
            let summary = format!("{} rang, {} missed", plan.due.len(), plan.missed.len());
            if planner.first || !plan.missed.is_empty() {
                // The first plan always reports, even with nothing missed, so the
                // repeating tasks' part of the launch summary is never held up.
                record_missed(app, &store, plan.missed).await;
            }
            planner.first = false;
            let (ring_now, late) = after_sleep(plan.due, now);
            for reminder in ring_now {
                ring(app, &store, reminder).await;
            }
            record_late(app, &store, late).await;
            if !wrote {
                planner.cached = Some(data);
                return WorkDone::nothing(plan.next_wake);
            }
            WorkDone::did(summary, plan.next_wake)
        }
        Err(e) => {
            planner.failures = planner.failures.saturating_add(1);
            if planner.first && planner.failures == 1 {
                // Nothing can be known missed yet: the repeating tasks' part of the
                // launch summary goes out without waiting for the calendar.
                report_launch(app, LaunchPart::Reminders, Vec::new());
            }
            if planner.failures == 1 {
                calendar_unreadable(&store, &e);
            } else {
                tracing::debug!(error = %e, failures = planner.failures, "reminders: calendar still unreadable");
            }
            let wait = chrono::TimeDelta::from_std(retry_delay(planner.failures))
                .unwrap_or_else(|_| chrono::TimeDelta::minutes(30));
            WorkDone {
                handled: Handled::Skipped {
                    summary: format!("The calendar could not be read: {e}"),
                },
                next_due: Some(Utc::now() + wait),
            }
        }
    }
}

// ── Commands ─────────────────────────────────────────────────────

/// Reminders that came due while the app was closed, not yet dismissed.
#[tauri::command]
pub fn list_missed_reminders(state: State<'_, ReminderState>) -> Vec<DueReminder> {
    state
        .missed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// The user has seen the missed reminders.
#[tauri::command]
pub fn dismiss_missed_reminders(state: State<'_, ReminderState>) {
    state
        .missed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

/// Ring a task's reminder again in `minutes` (default 10, at most a day).
/// Returns the task.
#[tauri::command]
pub async fn snooze_reminder(
    app: AppHandle,
    task_id: String,
    minutes: Option<u32>,
) -> Result<crate::calendar_store::TodoItem, String> {
    let minutes = minutes.unwrap_or(DEFAULT_SNOOZE_MINUTES);
    if minutes == 0 || minutes > MAX_SNOOZE_MINUTES {
        return Err(format!(
            "A snooze is 1 to {MAX_SNOOZE_MINUTES} minutes, not {minutes}"
        ));
    }
    let dir = crate::profile::app_data_dir(&app)
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;
    let until = snooze_until(chrono::Local::now().naive_local(), minutes);
    let task = CalendarStore::in_dir(&dir)
        .update(|d| d.snooze_reminder(&task_id, until))
        .map_err(|e| e.to_string())?;
    tracing::info!(task_id = %task_id, until = %until.format(REMINDER_FORMAT), "Snoozed reminder");
    if let Err(e) = app.emit(CALENDAR_CHANGED_EVENT, ()) {
        tracing::warn!(error = %e, "could not emit {}", CALENDAR_CHANGED_EVENT);
    }
    wake(&app);
    Ok(task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_store::{NewTask, TaskPatch};
    use chrono::NaiveDate;
    use chrono_tz::{America::New_York, Asia::Kolkata, Europe::London};

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap()
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.from_utc_datetime(&local(y, m, d, h, min))
    }

    fn data_with(reminders: &[(&str, Option<&str>)]) -> CalendarData {
        let mut data = CalendarData::default();
        for (title, reminder) in reminders {
            data.insert_task(NewTask {
                title: (*title).into(),
                reminder: reminder.map(str::to_string),
                ..NewTask::default()
            })
            .unwrap();
        }
        data
    }

    #[test]
    fn an_unreadable_calendar_is_read_again_less_and_less_often() {
        assert_eq!(retry_delay(1), MAX_SLEEP);
        assert_eq!(retry_delay(2), MAX_SLEEP * 2);
        assert_eq!(retry_delay(3), MAX_SLEEP * 4);
        assert_eq!(retry_delay(6), MAX_SLEEP * 30, "capped at half an hour");
        for failures in [7, 20, 1_000, u32::MAX] {
            assert_eq!(retry_delay(failures), MAX_RETRY_DELAY, "{failures}");
        }
        // Never faster than the first retry.
        assert_eq!(retry_delay(0), MAX_SLEEP);
    }

    #[test]
    fn a_damaged_calendar_without_a_backup_is_reported_and_left_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let store = CalendarStore::in_dir(dir.path());
        std::fs::write(store.path(), "{\"tasks\": [").unwrap();
        let error = store.load().unwrap_err();
        calendar_unreadable(&store, &error);
        let path = store.path().display().to_string();
        let notice = crate::store_notices::store_notices()
            .into_iter()
            .find(|n| n.store == "calendar" && n.path == path)
            .expect("the user is told once");
        assert_eq!(notice.kind, crate::store_notices::NoticeKind::Unreadable);
        assert!(notice.message.contains("Reminders cannot ring"));
        assert_eq!(
            std::fs::read_to_string(store.path()).unwrap(),
            "{\"tasks\": ["
        );
    }

    #[test]
    fn local_times_resolve_across_daylight_saving() {
        // An ordinary time.
        assert_eq!(
            resolve_local(&Kolkata, local(2026, 10, 29, 9, 0)),
            Some(utc(2026, 10, 29, 3, 30))
        );
        // New York springs forward at 02:00 on 8 March 2026: 02:30 does not
        // exist, so it rings when the gap ends (03:00 EDT = 07:00 UTC).
        assert_eq!(
            resolve_local(&New_York, local(2026, 3, 8, 2, 30)),
            Some(utc(2026, 3, 8, 7, 0))
        );
        // It falls back at 02:00 on 1 November 2026: 01:30 happens twice;
        // the first (EDT, 05:30 UTC) is used.
        assert_eq!(
            resolve_local(&New_York, local(2026, 11, 1, 1, 30)),
            Some(utc(2026, 11, 1, 5, 30))
        );
        // London: 01:15 on 29 March 2026 is skipped.
        assert_eq!(
            resolve_local(&London, local(2026, 3, 29, 1, 15)),
            Some(utc(2026, 3, 29, 1, 0))
        );
        // The same wall-clock time is a different instant once DST ends.
        let summer = resolve_local(&London, local(2026, 10, 24, 9, 0)).unwrap();
        let winter = resolve_local(&London, local(2026, 10, 25, 9, 0)).unwrap();
        assert_eq!(winter - summer, chrono::Duration::hours(25));
    }

    #[test]
    fn pending_skips_done_fired_and_unreadable_reminders() {
        let mut data = data_with(&[
            ("Later", Some("2026-10-29T10:00")),
            ("Sooner", Some("2026-10-29T09:00")),
            ("None", None),
        ]);
        // Values written before reminders were validated.
        data.tasks[2].reminder = Some("next week".into());
        let done = data
            .insert_task(NewTask {
                title: "Done".into(),
                reminder: Some("2026-10-29T08:00".into()),
                ..NewTask::default()
            })
            .unwrap();
        data.patch_task(
            &done.id,
            &TaskPatch {
                status: Some("completed".into()),
                ..TaskPatch::default()
            },
        )
        .unwrap();
        let fired = data
            .insert_task(NewTask {
                title: "Fired".into(),
                reminder: Some("2026-10-29T08:30".into()),
                ..NewTask::default()
            })
            .unwrap();
        assert!(data
            .mark_reminder_fired(&fired.id, "2026-10-29T08:30", "x")
            .unwrap());
        let titles: Vec<String> = pending(&data, &Kolkata)
            .into_iter()
            .map(|r| r.title)
            .collect();
        assert_eq!(titles, vec!["Sooner", "Later"]);
    }

    #[test]
    fn plans_ring_due_reminders_and_wake_for_the_next() {
        let data = data_with(&[
            ("Past", Some("2026-10-29T08:59")),
            ("Now", Some("2026-10-29T09:00")),
            ("Next", Some("2026-10-29T09:20")),
            ("After", Some("2026-10-29T11:00")),
        ]);
        let now = utc(2026, 10, 29, 3, 30); // 09:00 in Kolkata
        let p = plan(pending(&data, &Kolkata), now, None);
        let due: Vec<&str> = p.due.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(
            due,
            vec!["Past", "Now"],
            "running app: late ones still ring"
        );
        assert!(p.missed.is_empty());
        assert_eq!(p.next_wake, Some(utc(2026, 10, 29, 3, 50)));
        // Sleep until the next one, but never longer than the cap.
        assert_eq!(sleep_for(p.next_wake, now), MAX_SLEEP);
        assert_eq!(
            sleep_for(p.next_wake, utc(2026, 10, 29, 3, 49)),
            Duration::from_secs(60)
        );
        assert_eq!(
            sleep_for(
                p.next_wake,
                Utc.with_ymd_and_hms(2026, 10, 29, 3, 49, 30).unwrap()
            ),
            Duration::from_secs(30)
        );
        assert_eq!(
            sleep_for(p.next_wake, utc(2026, 10, 29, 4, 0)),
            Duration::ZERO
        );
        assert_eq!(sleep_for(None, now), MAX_SLEEP);
    }

    #[test]
    fn reminders_due_before_launch_are_missed_not_rung() {
        let data = data_with(&[
            ("Yesterday", Some("2026-10-28T09:00")),
            ("This morning", Some("2026-10-29T08:00")),
            ("Upcoming", Some("2026-10-29T12:00")),
        ]);
        let launch = utc(2026, 10, 29, 3, 30);
        let first = plan(pending(&data, &Kolkata), launch, Some(launch));
        let missed: Vec<&str> = first.missed.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(missed, vec!["Yesterday", "This morning"]);
        assert!(first.due.is_empty());
        assert_eq!(first.next_wake, Some(utc(2026, 10, 29, 6, 30)));
        // Later plans never call anything missed.
        let later = plan(pending(&data, &Kolkata), utc(2026, 10, 29, 7, 0), None);
        assert_eq!(later.due.len(), 3);
        assert!(later.missed.is_empty());
    }

    fn due(task_id: &str) -> DueReminder {
        DueReminder {
            task_id: task_id.into(),
            title: task_id.into(),
            reminder: "2026-11-23T09:00".into(),
            due_date: None,
            at: utc(2026, 11, 23, 3, 30),
            repeat: None,
        }
    }

    #[test]
    fn the_launch_summary_waits_for_both_parts_and_is_told_once() {
        let mut launch = LaunchMissed::default();
        assert_eq!(launch.report(LaunchPart::Reminders, vec![due("a")]), None);
        assert_eq!(
            launch.report(LaunchPart::Repeats, vec![due("r")]),
            Some(vec![due("a"), due("r")]),
            "one notification for both"
        );
        // A later report (a calendar that could not be read at first) is told alone.
        assert_eq!(
            launch.report(LaunchPart::Reminders, vec![due("b")]),
            Some(vec![due("b")])
        );
        assert_eq!(launch.report(LaunchPart::Reminders, Vec::new()), None);
    }

    #[test]
    fn a_failing_engine_still_lets_the_missed_reminder_summary_through() {
        // When the engine fails at launch, its drop guard reports an empty list.
        let mut launch = LaunchMissed::default();
        assert_eq!(launch.report(LaunchPart::Reminders, vec![due("a")]), None);
        assert_eq!(
            launch.report(LaunchPart::Repeats, Vec::new()),
            Some(vec![due("a")])
        );
        // Nothing missed on either side: nothing to say.
        let mut quiet = LaunchMissed::default();
        assert_eq!(quiet.report(LaunchPart::Repeats, Vec::new()), None);
        assert_eq!(quiet.report(LaunchPart::Reminders, Vec::new()), None);
    }

    #[test]
    fn reminders_slept_through_are_told_once_and_one_late_reminder_still_rings() {
        let data = data_with(&[
            ("Standup", Some("2026-10-29T09:00")),
            ("Call bank", Some("2026-10-29T10:00")),
            ("Pay rent", Some("2026-10-29T11:00")),
            ("Lunch", Some("2026-10-29T12:58")),
            ("Later", Some("2026-10-29T15:00")),
        ]);
        // The computer slept from before 09:00 (Kolkata) to 13:00; Shodh was running.
        let woke = utc(2026, 10, 29, 7, 30);
        let p = plan(pending(&data, &Kolkata), woke, None);
        let (ring, late) = after_sleep(p.due, woke);
        let rung: Vec<&str> = ring.iter().map(|r| r.title.as_str()).collect();
        let told: Vec<&str> = late.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(rung, vec!["Lunch"], "two minutes late: it rings");
        assert_eq!(
            told,
            vec!["Standup", "Call bank", "Pay rent"],
            "one notification"
        );
        assert_eq!(p.next_wake, Some(utc(2026, 10, 29, 9, 30)));
        // Only one reminder late: it rings, as it would have.
        let one = data_with(&[("Call bank", Some("2026-10-29T10:00"))]);
        let p = plan(pending(&one, &Kolkata), woke, None);
        let (ring, late) = after_sleep(p.due, woke);
        assert_eq!(ring.len(), 1);
        assert!(late.is_empty());
        // On time: nothing is held back.
        let (ring, late) = after_sleep(
            pending(&data, &Kolkata)[..1].to_vec(),
            utc(2026, 10, 29, 3, 31),
        );
        assert_eq!(ring.len(), 1);
        assert!(late.is_empty());
    }

    #[test]
    fn snoozing_rounds_up_to_a_whole_minute() {
        assert_eq!(
            snooze_until(local(2026, 10, 29, 9, 0), 10),
            local(2026, 10, 29, 9, 10)
        );
        let with_seconds = local(2026, 10, 29, 9, 3) + chrono::Duration::seconds(40);
        assert_eq!(snooze_until(with_seconds, 10), local(2026, 10, 29, 9, 14));
        assert_eq!(
            snooze_until(local(2026, 10, 29, 23, 55), 10),
            local(2026, 10, 30, 0, 5)
        );
    }

    #[test]
    fn a_snoozed_reminder_rings_at_the_snooze_time() {
        let mut data = data_with(&[("Call bank", Some("2026-10-29T09:00"))]);
        let id = data.tasks[0].id.clone();
        assert!(data
            .mark_reminder_fired(&id, "2026-10-29T09:00", "x")
            .unwrap());
        assert!(pending(&data, &Kolkata).is_empty());
        data.snooze_reminder(&id, snooze_until(local(2026, 10, 29, 9, 1), 10))
            .unwrap();
        let again = pending(&data, &Kolkata);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].reminder, "2026-10-29T09:11");
        assert_eq!(again[0].at, utc(2026, 10, 29, 3, 41));
    }
}
