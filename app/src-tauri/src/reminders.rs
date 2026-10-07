//! Task reminders: an in-app scheduler that shows a native notification
//! when a task's reminder comes due, while Shodh runs (also hidden in the
//! tray). Phone delivery is out of scope.
//!
//! - **Data**: `reminder` (and `snoozed_until`) are local wall-clock times
//!   ([`REMINDER_FORMAT`]); `reminder_fired_at` records that one was shown.
//!   See [`crate::calendar_store`].
//! - **Scheduling**: one task sleeps until the next reminder is due (at most
//!   [`MAX_SLEEP`], so a suspended computer or a clock change is noticed
//!   within a minute) and wakes early on every calendar change. Local times
//!   are converted to instants on every plan, never cached, so time-zone and
//!   daylight-saving changes apply at once.
//! - **Firing**: the task is marked fired in the calendar file first and the
//!   notification shown only if that write changed something, so a reminder
//!   is shown once even if two plans race.
//! - **Missed while closed**: reminders that came due before this launch are
//!   marked fired without one notification each; the UI lists them once
//!   ([`list_missed_reminders`] / [`dismiss_missed_reminders`]) and a single
//!   native notification says how many there were.
//!
//! What Windows supports (tauri-plugin-notification 2.3, desktop): a title
//! and a body. The plugin's action types and click callbacks are mobile
//! only, so a toast cannot carry Snooze / Mark done buttons and clicking it
//! does not reach the app. Those actions live in the in-app reminder toast
//! ([`REMINDER_FIRED_EVENT`]), which waits until the window is shown again.

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, LocalResult, NaiveDateTime, TimeZone, Timelike, Utc};
use serde::Serialize;
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Notify;

use crate::calendar_commands::CALENDAR_CHANGED_EVENT;
use crate::calendar_store::{
    parse_reminder, CalendarData, CalendarError, CalendarStore, REMINDER_FORMAT,
};

/// Longest the scheduler sleeps before looking at the clock again. Sleeping
/// is measured on a monotonic clock, which a suspended computer or a manual
/// clock change does not move like the wall clock; re-checking every minute
/// bounds how late a reminder can be after either.
pub const MAX_SLEEP: Duration = Duration::from_secs(60);

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
}

/// The instant a local wall-clock time means in `tz`. An ambiguous time
/// (clocks went back, the hour happens twice) is its first occurrence; a
/// time that does not exist (clocks went forward) is the first minute after
/// the gap, so a reminder in the skipped hour rings as soon as it ends
/// rather than never.
pub fn resolve_local<Tz: TimeZone>(tz: &Tz, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    let mut probe = local;
    // Real gaps are at most a few hours; a day bounds the search.
    for _ in 0..=24 * 60 {
        match tz.from_local_datetime(&probe) {
            LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => {
                return Some(t.with_timezone(&Utc))
            }
            LocalResult::None => probe += chrono::Duration::minutes(1),
        }
    }
    None
}

/// Reminders not yet shown, of tasks that are not done, in `tz`. Values that
/// are not a date-time (written before reminders were validated) are skipped.
pub fn pending<Tz: TimeZone>(data: &CalendarData, tz: &Tz) -> Vec<DueReminder> {
    let mut out: Vec<DueReminder> = data
        .tasks
        .iter()
        .filter(|t| t.status != "completed" && t.reminder_fired_at.is_none())
        .filter_map(|t| {
            let reminder = t.snoozed_until.as_deref().or(t.reminder.as_deref())?;
            let at = resolve_local(tz, parse_reminder(reminder)?)?;
            Some(DueReminder {
                task_id: t.id.clone(),
                title: t.title.clone(),
                reminder: reminder.to_string(),
                due_date: t.due_date.clone(),
                at,
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

/// How long to sleep before the next plan.
pub fn sleep_for(next_wake: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Duration {
    match next_wake {
        Some(at) => (at - now).to_std().unwrap_or(Duration::ZERO).min(MAX_SLEEP),
        None => MAX_SLEEP,
    }
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
#[derive(Default)]
pub struct ReminderState {
    wake: Notify,
    missed: Mutex<Vec<DueReminder>>,
}

impl ReminderState {
    /// Re-plan now (the calendar changed).
    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

/// Re-plan after a calendar change. A no-op before the scheduler exists.
pub fn wake(app: &AppHandle) {
    if let Some(state) = app.try_state::<ReminderState>() {
        state.wake();
    }
}

/// Start the scheduler. Call once, after [`ReminderState`] is managed.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(run(app));
}

async fn load(store: &CalendarStore) -> Result<CalendarData, String> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || store.load().map_err(|e| e.to_string()))
        .await
        .map_err(|e| format!("reading the calendar was interrupted: {e}"))?
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

fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!(error = %e, "could not show a reminder notification");
    }
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

async fn record_missed(app: &AppHandle, store: &CalendarStore, missed: Vec<DueReminder>) {
    let mut recorded = Vec::with_capacity(missed.len());
    for reminder in missed {
        if mark_fired(store, &reminder).await {
            recorded.push(reminder);
        }
    }
    if recorded.is_empty() {
        return;
    }
    let body = match recorded.as_slice() {
        [one] => due_text(one),
        many => format!("{} reminders. Open Shodh to see them.", many.len()),
    };
    notify(app, "Missed while Shodh was closed", &body);
    for reminder in &recorded {
        crate::inbox_commands::post(app, inbox_item(reminder, true)).await;
    }
    let state = app.state::<ReminderState>();
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

async fn run(app: AppHandle) {
    let store = match app.path().app_data_dir() {
        Ok(dir) => CalendarStore::in_dir(&dir),
        Err(e) => {
            tracing::error!(error = %e, "reminders are off: no app data directory");
            return;
        }
    };
    let launched_at = Utc::now();
    let mut first = true;
    let mut cached: Option<CalendarData> = None;
    loop {
        let state = app.state::<ReminderState>();
        // Registered before reading, so a change made while planning still
        // wakes the next sleep.
        let woken = state.wake.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();

        let data = match cached.take() {
            Some(data) => Ok(data),
            None => load(&store).await,
        };
        let next_wake = match data {
            Ok(data) => {
                let plan = plan(
                    pending(&data, &chrono::Local),
                    Utc::now(),
                    first.then_some(launched_at),
                );
                first = false;
                let wrote = !plan.missed.is_empty() || !plan.due.is_empty();
                if !plan.missed.is_empty() {
                    record_missed(&app, &store, plan.missed).await;
                }
                for reminder in plan.due {
                    ring(&app, &store, reminder).await;
                }
                if !wrote {
                    cached = Some(data);
                }
                plan.next_wake
            }
            Err(e) => {
                tracing::warn!(error = %e, "reminders: calendar unreadable; retrying");
                None
            }
        };

        tokio::select! {
            _ = tokio::time::sleep(sleep_for(next_wake, Utc::now())) => {}
            _ = &mut woken => cached = None,
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
    let dir = app
        .path()
        .app_data_dir()
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
