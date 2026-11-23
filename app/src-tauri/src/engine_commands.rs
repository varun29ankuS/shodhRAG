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

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use shodh_rag::audit::AuditKey;
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
    wake: Notify,
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
                wake: Notify::new(),
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
        assert_eq!(*repeats.ran.lock().unwrap(), ["repeat"], "09:00 was due");
        assert!(
            folders.ran.lock().unwrap().is_empty(),
            "07:00 comes first tomorrow"
        );
        let next = next_wake_all(&engine, &[JobKind::TaskRepeat, JobKind::FolderWatch]).unwrap();
        assert_eq!(next, Some(local(24, 7, 0)));
        let repeats_only = next_wake_all(&engine, &[JobKind::TaskRepeat]).unwrap();
        assert_eq!(repeats_only, Some(local(24, 9, 0)));
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
