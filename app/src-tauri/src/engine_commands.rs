//! The scheduler engine in the app: opened on `shodh.db` on first use, one loop that
//! plans and runs every kind of job (repeating tasks, What's new's folder watches), and
//! the wake every task change sends.
//!
//! - **One loop.** It sleeps until the earliest due time, for at most a minute (a
//!   suspended computer or a clock change is seen within a minute), and wakes early on
//!   every task change. A scheduled run never moves the screen: it posts to the Inbox and
//!   rings the reminder alert.
//! - **Launch.** The first tick finishes what a crash left half done and folds the times
//!   missed while Shodh was closed into each series' one question. Their alerts join the
//!   reminders' "missed while closed" summary: one desktop notification for both. The
//!   loop reports its part on every way out of the launch, so a failing engine never holds
//!   the reminders' summary up.
//! - **Automations have a loop of their own** on the same engine: one agent run takes
//!   minutes, and a repeating task's question must not wait for it. It runs one
//!   automation at a time, and ends runs whose approval expired.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use shodh_rag::audit::AuditKey;
use shodh_rag::engine::clock::{next_utc_midnight, utc_day};
use shodh_rag::engine::ledger::{DayCalls, Ledger, Spender, SpenderCalls};
use shodh_rag::engine::plan::sleep_for;
use shodh_rag::engine::runner::{Engine, JobHandler};
use shodh_rag::engine::store::{EngineError, EngineResult, EngineStore, JobKind};
use shodh_rag::inbox::NewInboxItem;
use shodh_rag::whats_new::store::FeedStore;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Notify, OnceCell};

use crate::agent_tools::{CalendarChange, HostEffects, TauriEffects};
use crate::audit_commands::AuditState;
use crate::calendar_store::{CalendarStore, TodoItem};
use crate::paper_alerts::PaperAlerts;
use crate::reminders::{DueReminder, LaunchPart, REMINDER_FIRED_EVENT};
use crate::task_repeat::{RepeatSink, Repeats, REPEAT_CHANGED_EVENT};
use crate::whats_new::WhatsNew;

/// The least wait after a tick that failed (the database could not be read or written).
const TICK_ERROR_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The engine's state, managed by Tauri.
#[derive(Clone)]
pub struct EngineState {
    inner: Arc<Inner>,
}

struct Inner {
    database: Option<(PathBuf, Option<AuditKey>)>,
    /// The one engine of this process: every kind of job runs on it.
    engine: OnceCell<Arc<Engine>>,
    repeats: OnceCell<Arc<Repeats>>,
    whats_new: OnceCell<Arc<WhatsNew>>,
    paper_alerts: OnceCell<Arc<PaperAlerts>>,
    /// The ledger of model calls.
    ledger: OnceCell<Arc<Ledger>>,
    automations: OnceCell<Arc<crate::automations::Automations>>,
    wake: Notify,
    /// Wakes the automations' loop (an automation changed, a run was started).
    wake_automations: Notify,
}

const DATABASE_UNAVAILABLE: &str =
    "Scheduled work needs the app database (shodh.db), which could not be opened; see the \
     audit page.";

impl EngineState {
    /// State over the database the audit log opened (`None` when it could not be opened).
    pub fn new(audit: &AuditState) -> Self {
        Self {
            inner: Arc::new(Inner {
                database: audit.database(),
                engine: OnceCell::new(),
                repeats: OnceCell::new(),
                whats_new: OnceCell::new(),
                paper_alerts: OnceCell::new(),
                ledger: OnceCell::new(),
                automations: OnceCell::new(),
                wake: Notify::new(),
                wake_automations: Notify::new(),
            }),
        }
    }

    async fn engine(&self) -> Result<Arc<Engine>, String> {
        self.inner
            .engine
            .get_or_try_init(|| async {
                let (path, key) = self
                    .inner
                    .database
                    .clone()
                    .ok_or_else(|| DATABASE_UNAVAILABLE.to_string())?;
                let store =
                    tokio::task::spawn_blocking(move || EngineStore::open(&path, key.as_ref()))
                        .await
                        .map_err(|e| e.to_string())?
                        .map_err(|e| e.to_string())?;
                Ok(Arc::new(Engine::new(Arc::new(store))))
            })
            .await
            .cloned()
    }

    async fn ledger(&self) -> Result<Arc<Ledger>, String> {
        self.inner
            .ledger
            .get_or_try_init(|| async {
                let (path, key) = self
                    .inner
                    .database
                    .clone()
                    .ok_or_else(|| DATABASE_UNAVAILABLE.to_string())?;
                let ledger = tokio::task::spawn_blocking(move || Ledger::open(&path, key.as_ref()))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                Ok(Arc::new(ledger))
            })
            .await
            .cloned()
    }

    async fn automations(
        &self,
        app: &AppHandle,
    ) -> Result<Arc<crate::automations::Automations>, String> {
        self.inner
            .automations
            .get_or_try_init(|| async {
                let engine = self.engine().await?;
                let env = Arc::new(crate::automation_agent::TauriEnv { app: app.clone() });
                Ok(Arc::new(crate::automations::Automations::new(engine, env)))
            })
            .await
            .cloned()
    }

    async fn whats_new(&self, app: &AppHandle) -> Result<Arc<WhatsNew>, String> {
        self.inner
            .whats_new
            .get_or_try_init(|| async {
                let engine = self.engine().await?;
                let (path, key) = self
                    .inner
                    .database
                    .clone()
                    .ok_or_else(|| DATABASE_UNAVAILABLE.to_string())?;
                let feed =
                    tokio::task::spawn_blocking(move || FeedStore::open(&path, key.as_ref()))
                        .await
                        .map_err(|e| e.to_string())?
                        .map_err(|e| e.to_string())?;
                let sink = Arc::new(crate::whats_new::TauriSink { app: app.clone() });
                Ok(Arc::new(WhatsNew::new(engine, Arc::new(feed), sink)))
            })
            .await
            .cloned()
    }

    async fn feed(&self) -> Result<Arc<FeedStore>, String> {
        let (path, key) = self
            .inner
            .database
            .clone()
            .ok_or_else(|| DATABASE_UNAVAILABLE.to_string())?;
        let feed = tokio::task::spawn_blocking(move || FeedStore::open(&path, key.as_ref()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(feed))
    }

    async fn paper_alerts(&self, app: &AppHandle) -> Result<Arc<PaperAlerts>, String> {
        self.inner
            .paper_alerts
            .get_or_try_init(|| async {
                let engine = self.engine().await?;
                let feed = self.feed().await?;
                let env = Arc::new(crate::paper_alerts::TauriAlertEnv { app: app.clone() });
                let sink = Arc::new(crate::whats_new::TauriSink { app: app.clone() });
                Ok(Arc::new(PaperAlerts::new(engine, feed, env, sink)))
            })
            .await
            .cloned()
    }

    async fn repeats(&self, app: &AppHandle) -> Result<Arc<Repeats>, String> {
        self.inner
            .repeats
            .get_or_try_init(|| async {
                let engine = self.engine().await?;
                let dir = crate::profile::app_data_dir(app)
                    .map_err(|e| format!("Failed to get app data directory: {e}"))?;
                let sink: Arc<dyn RepeatSink> = Arc::new(TauriSink { app: app.clone() });
                Ok(Arc::new(Repeats::new(
                    engine,
                    CalendarStore::in_dir(&dir),
                    sink,
                )))
            })
            .await
            .cloned()
    }
}

/// The ledger of model calls, or `None` (logged) when it cannot be opened.
pub async fn ledger(app: &AppHandle) -> Option<Arc<Ledger>> {
    let state = app.try_state::<EngineState>()?.inner().clone();
    match state.ledger().await {
        Ok(ledger) => Some(ledger),
        Err(e) => {
            tracing::debug!(error = %e, "the ledger of model calls is unavailable");
            None
        }
    }
}

/// Automations, or `None` (logged) when the engine cannot be opened.
pub async fn automations(app: &AppHandle) -> Option<Arc<crate::automations::Automations>> {
    let state = app.try_state::<EngineState>()?.inner().clone();
    match state.automations(app).await {
        Ok(automations) => Some(automations),
        Err(e) => {
            tracing::debug!(error = %e, "automations are unavailable");
            None
        }
    }
}

/// Automations for a command: an error the UI can show when they are unavailable.
pub async fn automations_for_command(
    app: &AppHandle,
) -> Result<Arc<crate::automations::Automations>, String> {
    let state = app
        .try_state::<EngineState>()
        .ok_or_else(|| "Automations are not ready yet.".to_string())?
        .inner()
        .clone();
    state.automations(app).await
}

/// Look at the automations again now (one changed or was started).
pub fn wake_automations(app: &AppHandle) {
    if let Some(state) = app.try_state::<EngineState>() {
        state.inner.wake_automations.notify_one();
    }
}

/// The repeating tasks, or `None` (logged) when the engine cannot be opened.
pub async fn repeats(app: &AppHandle) -> Option<Arc<Repeats>> {
    let state = app.try_state::<EngineState>()?.inner().clone();
    match state.repeats(app).await {
        Ok(repeats) => Some(repeats),
        Err(e) => {
            tracing::debug!(error = %e, "repeating tasks are unavailable");
            None
        }
    }
}

/// The repeating tasks for a command: an error the UI can show when they are unavailable.
pub async fn repeats_for_command(app: &AppHandle) -> Result<Arc<Repeats>, String> {
    let state = app
        .try_state::<EngineState>()
        .ok_or_else(|| "Repeating tasks are not ready yet.".to_string())?
        .inner()
        .clone();
    state.repeats(app).await
}

/// What's new, or `None` (logged) when the engine or its feed cannot be opened.
pub async fn whats_new(app: &AppHandle) -> Option<Arc<WhatsNew>> {
    let state = app.try_state::<EngineState>()?.inner().clone();
    match state.whats_new(app).await {
        Ok(whats_new) => Some(whats_new),
        Err(e) => {
            tracing::debug!(error = %e, "What's new is unavailable");
            None
        }
    }
}

/// What's new for a command: an error the UI can show when it is unavailable.
pub async fn whats_new_for_command(app: &AppHandle) -> Result<Arc<WhatsNew>, String> {
    let state = app
        .try_state::<EngineState>()
        .ok_or_else(|| "What's new is not ready yet.".to_string())?
        .inner()
        .clone();
    state.whats_new(app).await
}

/// Paper alerts, or `None` (logged) when the engine or its feed cannot be opened.
pub async fn paper_alerts(app: &AppHandle) -> Option<Arc<PaperAlerts>> {
    let state = app.try_state::<EngineState>()?.inner().clone();
    match state.paper_alerts(app).await {
        Ok(alerts) => Some(alerts),
        Err(e) => {
            tracing::debug!(error = %e, "paper alerts are unavailable");
            None
        }
    }
}

/// Paper alerts for a command: an error the UI can show when they are unavailable.
pub async fn paper_alerts_for_command(app: &AppHandle) -> Result<Arc<PaperAlerts>, String> {
    let state = app
        .try_state::<EngineState>()
        .ok_or_else(|| "Paper alerts are not ready yet.".to_string())?
        .inner()
        .clone();
    state.paper_alerts(app).await
}

/// Records one model call of `spender` in `pool` today (UTC). Counting never stops a
/// call: a ledger that cannot be opened or written is logged and the call goes on.
pub async fn record_call(app: &AppHandle, spender: Spender, pool: String) {
    let Some(state) = app.try_state::<EngineState>().map(|s| s.inner().clone()) else {
        return;
    };
    let ledger = match state.ledger().await {
        Ok(ledger) => ledger,
        Err(e) => {
            tracing::debug!(error = %e, "the ledger of model calls is unavailable");
            return;
        }
    };
    let day = utc_day(Utc::now());
    let recorded = tokio::task::spawn_blocking(move || ledger.record(day, spender, &pool, 1)).await;
    match recorded {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            tracing::warn!(error = %e, spender = spender.as_str(), "a model call could not be counted")
        }
        Err(e) => tracing::warn!(error = %e, "counting a model call stopped"),
    }
}

/// Today's model calls (UTC day), by spender and by pool.
pub async fn calls_today(app: &AppHandle) -> Result<DayCalls, String> {
    let state = app
        .try_state::<EngineState>()
        .ok_or_else(|| DATABASE_UNAVAILABLE.to_string())?
        .inner()
        .clone();
    let ledger = state.ledger().await?;
    let day = utc_day(Utc::now());
    tokio::task::spawn_blocking(move || ledger.day(day))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Today's model calls for the counter (Settings → Model, `system_status`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageToday {
    /// The UTC day, `YYYY-MM-DD`.
    pub day: String,
    pub total: u32,
    /// The spenders that made calls today, in the counter's order.
    pub by_spender: Vec<SpenderCalls>,
    /// When the day's count starts again (the next 00:00 UTC), RFC 3339.
    pub resets_at: String,
}

/// The ledger's day, with learning's own count of its calls (kept in its own store
/// until it moves into the ledger) in its place among the spenders.
pub fn usage_today(day: DayCalls, learning_calls: u32, now: DateTime<Utc>) -> ModelUsageToday {
    let mut by_spender = day.by_spender;
    if learning_calls > 0 && !by_spender.iter().any(|s| s.spender == Spender::Learning) {
        by_spender.push(SpenderCalls {
            spender: Spender::Learning,
            calls: learning_calls,
        });
        by_spender.sort_by_key(|s| Spender::ALL.iter().position(|a| *a == s.spender));
    }
    ModelUsageToday {
        day: day.day,
        total: by_spender
            .iter()
            .map(|s| s.calls)
            .fold(0, u32::saturating_add),
        by_spender,
        resets_at: next_utc_midnight(now).to_rfc3339(),
    }
}

impl ModelUsageToday {
    /// The counter in words, for the assistant: "Today: 23 model calls (chat 9, tables 8).
    /// The count starts again at 00:00 UTC."
    pub fn sentence(&self) -> String {
        let counted = if self.total == 0 {
            "Today: no model calls yet.".to_string()
        } else {
            let parts: Vec<String> = self
                .by_spender
                .iter()
                .map(|s| format!("{} {}", s.spender.as_str(), s.calls))
                .collect();
            let noun = if self.total == 1 {
                "model call"
            } else {
                "model calls"
            };
            format!("Today: {} {noun} ({}).", self.total, parts.join(", "))
        };
        format!("{counted} The count starts again at 00:00 UTC.")
    }
}

/// Today's model calls, with learning's own count.
pub async fn read_usage_today(app: &AppHandle) -> Result<ModelUsageToday, String> {
    let day = calls_today(app).await?;
    let learning = match app.try_state::<crate::memory_learn::LearnState>() {
        Some(learn) => learn.calls_today().await.unwrap_or_else(|e| {
            tracing::debug!(error = %e, "learning's calls today could not be read");
            0
        }),
        None => 0,
    };
    Ok(usage_today(day, learning, Utc::now()))
}

/// Today's model calls: chat, tables and the other spenders.
#[tauri::command]
pub async fn model_usage_today(app: AppHandle) -> Result<ModelUsageToday, String> {
    read_usage_today(&app).await
}

/// One tick for every kind of job, in turn. A kind that fails does not keep the others
/// from their tick; its error is returned with it.
pub async fn tick_all(
    engine: &Engine,
    handlers: &[Arc<dyn JobHandler>],
) -> Vec<(JobKind, EngineError)> {
    let mut failed = Vec::new();
    for handler in handlers {
        if let Err(e) = engine.tick(handler).await {
            failed.push((handler.kind(), e));
        }
    }
    failed
}

/// The earliest time any job of `kinds` (or a retry) is due.
pub fn next_wake_all(engine: &Engine, kinds: &[JobKind]) -> EngineResult<Option<DateTime<Utc>>> {
    let mut next: Option<DateTime<Utc>> = None;
    for kind in kinds {
        if let Some(due) = engine.next_wake(*kind)? {
            next = Some(next.map_or(due, |n| n.min(due)));
        }
    }
    Ok(next)
}

/// Plan again now (a task or a repeat changed). A no-op before the loop exists.
pub fn wake(app: &AppHandle) {
    if let Some(state) = app.try_state::<EngineState>() {
        state.inner.wake.notify_one();
    }
}

/// The repeating tasks' part of the launch summary, given to the reminders however the
/// launch ends (with nothing missed when it failed).
struct LaunchReport {
    app: AppHandle,
    missed: Vec<DueReminder>,
}

impl Drop for LaunchReport {
    fn drop(&mut self) {
        crate::reminders::report_launch(
            &self.app,
            LaunchPart::Repeats,
            std::mem::take(&mut self.missed),
        );
    }
}

/// Start the loop. Call once, after [`EngineState`] and the reminders' state are managed.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut report = LaunchReport {
            app: app.clone(),
            missed: Vec::new(),
        };
        let Some(state) = app.try_state::<EngineState>().map(|s| s.inner().clone()) else {
            return;
        };
        let repeats = match state.repeats(&app).await {
            Ok(repeats) => repeats,
            Err(e) => {
                tracing::warn!(error = %e, "repeating tasks are off");
                return;
            }
        };
        report.missed = repeats.launch().await;
        drop(report);
        let engine = match state.engine().await {
            Ok(engine) => engine,
            Err(e) => {
                tracing::warn!(error = %e, "scheduled work is off");
                return;
            }
        };
        let mut handlers = vec![repeats.handler()];
        match state.whats_new(&app).await {
            Ok(whats_new) => {
                whats_new.launch().await;
                handlers.push(whats_new.handler());
            }
            Err(e) => tracing::warn!(error = %e, "What's new is off"),
        }
        match state.paper_alerts(&app).await {
            Ok(alerts) => handlers.push(alerts.handler()),
            Err(e) => tracing::warn!(error = %e, "paper alerts are off"),
        }
        let kinds: Vec<JobKind> = handlers.iter().map(|h| h.kind()).collect();
        match state.automations(&app).await {
            Ok(automations) => spawn_automations(state.clone(), engine.clone(), automations),
            Err(e) => tracing::warn!(error = %e, "automations are off"),
        }
        loop {
            // Registered before planning, so a change made meanwhile still wakes the sleep.
            let woken = state.inner.wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            let next = next_wake_all(&engine, &kinds).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "scheduled work: the next due time could not be read");
                None
            });
            tokio::select! {
                _ = tokio::time::sleep(sleep_for(next, Utc::now())) => {}
                _ = &mut woken => {}
            }
            let failed = tick_all(&engine, &handlers).await;
            for (kind, e) in &failed {
                tracing::warn!(kind = kind.as_str(), error = %e, "scheduled work could not be planned");
            }
            if !failed.is_empty() {
                // A database that keeps failing is not tried again in a tight loop, however
                // often tasks change meanwhile.
                tokio::time::sleep(TICK_ERROR_WAIT).await;
            }
        }
    });
}

/// The automations' loop: the same rules as the main loop (sleep until the earliest due
/// time or retry, at most a minute; wake early on a change), one automation at a time.
fn spawn_automations(
    state: EngineState,
    engine: Arc<Engine>,
    automations: Arc<crate::automations::Automations>,
) {
    tauri::async_runtime::spawn(async move {
        automations.launch().await;
        let handler = automations.handler();
        let kinds = [JobKind::Playbook];
        loop {
            let woken = state.inner.wake_automations.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            let next = next_wake_all(&engine, &kinds)
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "automations: the next due time could not be read");
                    None
                })
                .into_iter()
                .chain(automations.next_expiry())
                .min();
            tokio::select! {
                _ = tokio::time::sleep(sleep_for(next, Utc::now())) => {}
                _ = &mut woken => {}
            }
            automations.sweep().await;
            if let Err(e) = engine.tick(&handler).await {
                tracing::warn!(error = %e, "automations could not be planned");
                tokio::time::sleep(TICK_ERROR_WAIT).await;
            }
        }
    });
}

/// What the repeating tasks tell the app.
struct TauriSink {
    app: AppHandle,
}

#[async_trait]
impl RepeatSink for TauriSink {
    async fn calendar_changed(&self, task: &TodoItem) {
        TauriEffects::new(self.app.clone())
            .calendar_changed(CalendarChange::TaskSaved(task.clone()));
    }

    async fn post(&self, item: NewInboxItem) {
        crate::inbox_commands::post(&self.app, item).await;
    }

    async fn resolve(&self, id: &str) {
        crate::inbox_commands::resolve(&self.app, id.to_string()).await;
    }

    fn ring(&self, alert: &DueReminder) {
        let body = match &alert.repeat {
            Some(repeat) => format!("{}: {}", alert.title, repeat.detail),
            None => alert.title.clone(),
        };
        crate::notifications::notify(
            &self.app,
            crate::notifications::Category::Reminders,
            "Due again",
            body,
        );
        if let Err(e) = self.app.emit(REMINDER_FIRED_EVENT, alert) {
            tracing::warn!(error = %e, "could not emit {}", REMINDER_FIRED_EVENT);
        }
    }

    fn changed(&self) {
        if let Err(e) = self.app.emit(REPEAT_CHANGED_EVENT, ()) {
            tracing::warn!(error = %e, "could not emit {}", REPEAT_CHANGED_EVENT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, NaiveTime};
    use serde_json::json;
    use shodh_rag::engine::clock::{resolve_local, Clock};
    use shodh_rag::engine::runner::Handled;
    use shodh_rag::engine::schedule::Schedule;
    use shodh_rag::engine::store::{CatchUp, CreatedBy, Job, NewJob, Overlap, Run};
    use std::sync::Mutex;

    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// A handler of one kind that records the jobs it ran.
    struct Recording {
        kind: JobKind,
        ran: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl JobHandler for Recording {
        fn kind(&self) -> JobKind {
            self.kind
        }
        async fn run(&self, job: &Job, _run: &Run) -> Handled {
            self.ran
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(job.id.clone());
            Handled::Succeeded {
                summary: "done".into(),
                scheduled_for: None,
            }
        }
        async fn folded(&self, _job: &Job, _run: &Run) {}
        async fn held(&self, _job: &Job, _run: &Run) {}
    }

    fn local(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        let naive = chrono::NaiveDate::from_ymd_opt(2026, 11, d)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap();
        resolve_local(&Local, naive).unwrap()
    }

    fn daily(store: &EngineStore, id: &str, kind: JobKind, hour: u32) {
        store
            .create_job(
                &NewJob {
                    id: id.into(),
                    kind,
                    subject: id.into(),
                    title: id.into(),
                    schedule: Some(Schedule::Daily {
                        at: NaiveTime::from_hms_opt(hour, 0, 0).unwrap(),
                    }),
                    input: json!({}),
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 3,
                    created_by: CreatedBy::System,
                    provenance: json!({}),
                },
                local(23, 8, 0),
            )
            .unwrap();
    }

    fn recording(kind: JobKind) -> Arc<Recording> {
        Arc::new(Recording {
            kind,
            ran: Mutex::new(Vec::new()),
        })
    }

    #[tokio::test]
    async fn one_tick_serves_every_kind_and_the_wake_is_the_earliest() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        daily(&store, "repeat", JobKind::TaskRepeat, 9);
        daily(&store, "folder", JobKind::FolderWatch, 7);
        let engine = Engine::with_clock(
            store.clone(),
            Arc::new(FixedClock(local(23, 9, 1))),
            "engine-commands-test",
        );
        let repeats = recording(JobKind::TaskRepeat);
        let folders = recording(JobKind::FolderWatch);
        let handlers: Vec<Arc<dyn JobHandler>> = vec![repeats.clone(), folders.clone()];
        assert!(tick_all(&engine, &handlers).await.is_empty());
        assert_eq!(
            *repeats.ran.lock().unwrap_or_else(|e| e.into_inner()),
            ["repeat"],
            "09:00 was due"
        );
        assert!(
            folders
                .ran
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "07:00 comes first tomorrow"
        );
        let next = next_wake_all(&engine, &[JobKind::TaskRepeat, JobKind::FolderWatch]).unwrap();
        assert_eq!(next, Some(local(24, 7, 0)));
        let repeats_only = next_wake_all(&engine, &[JobKind::TaskRepeat]).unwrap();
        assert_eq!(repeats_only, Some(local(24, 9, 0)));
    }

    #[test]
    fn usage_today_puts_learning_among_the_spenders_and_resets_at_utc_midnight() {
        let calls = |spender, calls| SpenderCalls { spender, calls };
        let day = DayCalls {
            day: "2026-11-25".into(),
            total: 17,
            by_spender: vec![calls(Spender::Chat, 9), calls(Spender::Tables, 8)],
            by_pool: Default::default(),
        };
        let now = DateTime::parse_from_rfc3339("2026-11-25T18:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let usage = usage_today(day.clone(), 3, now);
        assert_eq!(usage.total, 20);
        assert_eq!(
            usage.by_spender,
            vec![
                calls(Spender::Chat, 9),
                calls(Spender::Tables, 8),
                calls(Spender::Learning, 3)
            ]
        );
        assert_eq!(usage.resets_at, "2026-11-26T00:00:00+00:00");
        assert_eq!(
            usage.sentence(),
            "Today: 20 model calls (chat 9, tables 8, learning 3). The count starts again at \
             00:00 UTC."
        );
        let quiet = usage_today(day, 0, now);
        assert_eq!(quiet.by_spender.len(), 2, "no learning, no row");
        let none = usage_today(
            DayCalls {
                day: "2026-11-25".into(),
                total: 0,
                by_spender: Vec::new(),
                by_pool: Default::default(),
            },
            0,
            now,
        );
        assert_eq!(
            none.sentence(),
            "Today: no model calls yet. The count starts again at 00:00 UTC."
        );
    }

    #[tokio::test]
    async fn no_jobs_means_no_wake() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        let engine = Engine::with_clock(
            store,
            Arc::new(FixedClock(local(23, 9, 0))),
            "engine-commands-test",
        );
        let handlers: Vec<Arc<dyn JobHandler>> = vec![recording(JobKind::FolderWatch)];
        assert!(tick_all(&engine, &handlers).await.is_empty());
        assert_eq!(
            next_wake_all(&engine, &[JobKind::FolderWatch]).unwrap(),
            None
        );
    }
}
