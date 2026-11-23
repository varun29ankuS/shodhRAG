//! The scheduler engine in the app: opened on `shodh.db` on first use, one loop that
//! plans and runs the repeating tasks, and the wake every task change sends.
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
use chrono::Utc;
use shodh_rag::audit::AuditKey;
use shodh_rag::engine::plan::sleep_for;
use shodh_rag::engine::runner::Engine;
use shodh_rag::engine::store::EngineStore;
use shodh_rag::inbox::NewInboxItem;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Notify, OnceCell};

use crate::agent_tools::{CalendarChange, HostEffects, TauriEffects};
use crate::audit_commands::AuditState;
use crate::calendar_store::{CalendarStore, TodoItem};
use crate::reminders::{DueReminder, LaunchPart, REMINDER_FIRED_EVENT};
use crate::task_repeat::{RepeatSink, Repeats, REPEAT_CHANGED_EVENT};

/// The engine's state, managed by Tauri.
#[derive(Clone)]
pub struct EngineState {
    inner: Arc<Inner>,
}

struct Inner {
    database: Option<(PathBuf, Option<AuditKey>)>,
    repeats: OnceCell<Arc<Repeats>>,
    wake: Notify,
}

impl EngineState {
    /// State over the database the audit log opened (`None` when it could not be opened).
    pub fn new(audit: &AuditState) -> Self {
        Self {
            inner: Arc::new(Inner {
                database: audit.database(),
                repeats: OnceCell::new(),
                wake: Notify::new(),
            }),
        }
    }

    async fn repeats(&self, app: &AppHandle) -> Result<Arc<Repeats>, String> {
        self.inner
            .repeats
            .get_or_try_init(|| async {
                let (path, key) = self.inner.database.clone().ok_or_else(|| {
                    "Repeating tasks need the app database (shodh.db), which could not be \
                     opened; see the audit page."
                        .to_string()
                })?;
                let store =
                    tokio::task::spawn_blocking(move || EngineStore::open(&path, key.as_ref()))
                        .await
                        .map_err(|e| e.to_string())?
                        .map_err(|e| e.to_string())?;
                let dir = crate::profile::app_data_dir(app)
                    .map_err(|e| format!("Failed to get app data directory: {e}"))?;
                let engine = Arc::new(Engine::new(Arc::new(store)));
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
        loop {
            // Registered before planning, so a change made meanwhile still wakes the sleep.
            let woken = state.inner.wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            let next = repeats.next_wake().unwrap_or_else(|e| {
                tracing::warn!(error = %e, "repeating tasks: the next due time could not be read");
                None
            });
            tokio::select! {
                _ = tokio::time::sleep(sleep_for(next, Utc::now())) => {}
                _ = &mut woken => {}
            }
            if let Err(e) = repeats.tick().await {
                tracing::warn!(error = %e, "repeating tasks could not be planned");
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
