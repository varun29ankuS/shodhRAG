//! The scheduler engine in the app: opened on `shodh.db` on first use, one loop that
//! plans and runs every kind of job (repeating tasks, What's new's folder watches, paper
//! alerts, automations and the app's own system work), and the one wake every change sends.
//!
//! - **One loop.** It sleeps until the earliest due time, for at most a minute (a
//!   suspended computer or a clock change is seen within a minute), and wakes early on
//!   every change ([`wake`], [`due_now`]). Each kind's tick runs on a task of its own, one
//!   tick of a kind at a time ([`Dispatcher`]): an automation's agent run takes minutes,
//!   and a reminder or a repeating task's question never waits for it. A scheduled run
//!   never moves the screen: it posts to the Inbox and rings the reminder alert.
//! - **System work** ([`crate::system_jobs`]): table fills, task reminders and the
//!   nightly code map refresh are system jobs on the same engine, never shown in
//!   Automations.
//! - **Launch.** Each kind finishes what a crash left half done and folds the times
//!   missed while Shodh was closed into each series' one question. Their alerts join the
//!   reminders' "missed while closed" summary: one desktop notification for both. The
//!   loop reports its part on every way out of the launch, so a failing engine never holds
//!   the reminders' summary up.
//! - **Without the engine** (`shodh.db` cannot be opened), the loop still rings the
//!   reminders, which need only the calendar file.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use shodh_rag::audit::AuditKey;
use shodh_rag::engine::clock::{next_utc_midnight, utc_day};
use shodh_rag::engine::ledger::{DayCalls, Ledger, Spender, SpenderCalls};
use shodh_rag::engine::plan::sleep_for;
use shodh_rag::engine::runner::{Engine, JobHandler};
use shodh_rag::engine::store::{EngineResult, EngineStore, JobKind};
use shodh_rag::inbox::NewInboxItem;
use shodh_rag::whats_new::store::FeedStore;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Notify, OnceCell};

use crate::agent_tools::{CalendarChange, HostEffects, TauriEffects};
use crate::audit_commands::AuditState;
use crate::calendar_store::{CalendarStore, TodoItem};
use crate::paper_alerts::PaperAlerts;
use crate::reminders::{DueReminder, LaunchPart, REMINDER_FIRED_EVENT};
use crate::system_jobs::{self, AppWork, SystemHandler, SystemWork, Work};
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
    /// Wakes the loop: something changed.
    wake: Notify,
    /// The app's work made due now, taken by the loop's next turn.
    due_now: Mutex<HashSet<SystemWork>>,
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
                // The reminders are planned, and the table fills checked (once the model
                // settings are loaded), as soon as the loop runs.
                due_now: Mutex::new(HashSet::from([
                    SystemWork::ReminderPlan,
                    SystemWork::TableTick,
                ])),
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

/// The engine's store when the engine is open already (it opens with the loop); `None`
/// before, or when it could not be opened. Never opens it.
pub fn store_if_open(app: &AppHandle) -> Option<Arc<EngineStore>> {
    let state = app.try_state::<EngineState>()?;
    state
        .inner
        .engine
        .get()
        .map(|engine| engine.store().clone())
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

/// One kind's tick: its handler's, with what the kind does first.
#[async_trait]
pub trait Ticker: Send + Sync {
    fn kind(&self) -> JobKind;
    /// A due time of its own besides its jobs' (an approval that expires).
    fn own_due(&self) -> Option<DateTime<Utc>> {
        None
    }
    async fn tick(&self, engine: &Engine) -> EngineResult<()>;
}

/// The tick of a handler's kind.
pub struct HandlerTicker(pub Arc<dyn JobHandler>);

#[async_trait]
impl Ticker for HandlerTicker {
    fn kind(&self) -> JobKind {
        self.0.kind()
    }

    async fn tick(&self, engine: &Engine) -> EngineResult<()> {
        engine.tick(&self.0).await.map(|_| ())
    }
}

/// Automations' tick: the runs whose approval expired end first.
struct AutomationsTicker {
    automations: Arc<crate::automations::Automations>,
    handler: Arc<dyn JobHandler>,
}

#[async_trait]
impl Ticker for AutomationsTicker {
    fn kind(&self) -> JobKind {
        JobKind::Playbook
    }

    fn own_due(&self) -> Option<DateTime<Utc>> {
        self.automations.next_expiry()
    }

    async fn tick(&self, engine: &Engine) -> EngineResult<()> {
        self.automations.sweep().await;
        engine.tick(&self.handler).await.map(|_| ())
    }
}

/// Starts each kind's tick on a task of its own, one tick of a kind at a time: an
/// automation's agent run takes minutes, and a reminder or a repeating task's question
/// never waits for it.
pub struct Dispatcher {
    engine: Arc<Engine>,
    tickers: Mutex<Vec<Arc<dyn Ticker>>>,
    ticking: Arc<Mutex<HashSet<JobKind>>>,
    /// Kinds to tick at the next turn whether or not anything is due (something changed,
    /// or the loop slept its longest).
    dirty: Mutex<HashSet<JobKind>>,
    ticked: Arc<Notify>,
}

/// A kind's place among the ticking, given back when its tick's task ends, however.
struct Ticking {
    kind: JobKind,
    ticking: Arc<Mutex<HashSet<JobKind>>>,
    ticked: Arc<Notify>,
}

impl Drop for Ticking {
    fn drop(&mut self) {
        locked(&self.ticking).remove(&self.kind);
        self.ticked.notify_one();
    }
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Dispatcher {
    pub fn new(engine: Arc<Engine>) -> Self {
        Self {
            engine,
            tickers: Mutex::new(Vec::new()),
            ticking: Arc::new(Mutex::new(HashSet::new())),
            dirty: Mutex::new(HashSet::new()),
            ticked: Arc::new(Notify::new()),
        }
    }

    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// Adds a kind; its first tick is at the next turn.
    pub fn add(&self, ticker: Arc<dyn Ticker>) {
        locked(&self.dirty).insert(ticker.kind());
        locked(&self.tickers).push(ticker);
        self.ticked.notify_one();
    }

    /// Every kind ticks at the next turn.
    pub fn mark_all(&self) {
        let kinds: Vec<JobKind> = locked(&self.tickers).iter().map(|t| t.kind()).collect();
        locked(&self.dirty).extend(kinds);
    }

    /// Told when a tick ends (and when a kind is added).
    pub fn ticked(&self) -> Arc<Notify> {
        self.ticked.clone()
    }

    pub fn is_ticking(&self, kind: JobKind) -> bool {
        locked(&self.ticking).contains(&kind)
    }

    /// Starts the tick of every kind not ticking now that is marked, or due at `now`.
    /// Returns the earliest due time of the kinds left waiting; a ticking kind's is looked
    /// at again when its tick ends.
    pub fn dispatch(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let tickers: Vec<Arc<dyn Ticker>> = locked(&self.tickers).clone();
        let mut next: Option<DateTime<Utc>> = None;
        for ticker in tickers {
            let kind = ticker.kind();
            if self.is_ticking(kind) {
                continue;
            }
            let due = self
                .engine
                .next_wake(kind)
                .unwrap_or_else(|e| {
                    tracing::warn!(kind = kind.as_str(), error = %e, "scheduled work: the next due time could not be read");
                    None
                })
                .into_iter()
                .chain(ticker.own_due())
                .min();
            let marked = locked(&self.dirty).remove(&kind);
            if marked || due.is_some_and(|due| due <= now) {
                self.start(ticker);
            } else if let Some(due) = due {
                next = Some(next.map_or(due, |n| n.min(due)));
            }
        }
        next
    }

    fn start(&self, ticker: Arc<dyn Ticker>) {
        let kind = ticker.kind();
        locked(&self.ticking).insert(kind);
        let place = Ticking {
            kind,
            ticking: self.ticking.clone(),
            ticked: self.ticked.clone(),
        };
        let engine = self.engine.clone();
        tokio::spawn(async move {
            let _place = place;
            if let Err(e) = ticker.tick(&engine).await {
                tracing::warn!(kind = kind.as_str(), error = %e, "scheduled work could not be planned");
                // A database that keeps failing is not tried again in a tight loop,
                // however often things change meanwhile.
                tokio::time::sleep(TICK_ERROR_WAIT).await;
            }
        });
    }
}

/// Plan again now (a task, a repeat or an automation changed). A no-op before the loop
/// exists.
pub fn wake(app: &AppHandle) {
    if let Some(state) = app.try_state::<EngineState>() {
        state.inner.wake.notify_one();
    }
}

/// Makes a piece of the app's work due now (its state changed: the calendar, a table's
/// fill), and wakes the loop. A no-op before the loop exists.
pub fn due_now(app: &AppHandle, work: SystemWork) {
    if let Some(state) = app.try_state::<EngineState>() {
        locked(&state.inner.due_now).insert(work);
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

/// Work the loop runs itself, without the engine: what must go on when `shodh.db` cannot
/// be opened (reminders ring from the calendar file alone).
struct Direct {
    work: SystemWork,
    run: Arc<dyn Work>,
    due: Option<DateTime<Utc>>,
}

/// Start the loop. Call once, after [`EngineState`], the reminders' and the tables' state
/// are managed.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let report = LaunchReport {
            app: app.clone(),
            missed: Vec::new(),
        };
        let Some(state) = app.try_state::<EngineState>().map(|s| s.inner().clone()) else {
            return;
        };
        let engine = match state.engine().await {
            Ok(engine) => Some(engine),
            Err(e) => {
                tracing::warn!(error = %e, "scheduled work is off; reminders still ring");
                None
            }
        };
        let (dispatcher, direct) = match engine {
            Some(engine) => {
                let dispatcher = Arc::new(Dispatcher::new(engine.clone()));
                let store = engine.store().clone();
                let now = engine.now();
                let ensured =
                    tokio::task::spawn_blocking(move || system_jobs::ensure_jobs(&store, now))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|made| made.map_err(|e| e.to_string()));
                let direct = match ensured {
                    Ok(()) => {
                        dispatcher.add(Arc::new(HandlerTicker(Arc::new(SystemHandler::new(
                            engine.store().clone(),
                            AppWork::all(&app),
                        )))));
                        Vec::new()
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "system jobs could not be made; reminders ring without the engine");
                        direct_works(&app)
                    }
                };
                launch_kinds(app.clone(), state.clone(), dispatcher.clone(), report);
                (Some(dispatcher), direct)
            }
            None => {
                drop(report);
                (None, direct_works(&app))
            }
        };
        run_loop(app, state, dispatcher, direct).await;
    });
}

/// The work that runs without the engine.
fn direct_works(app: &AppHandle) -> Vec<Direct> {
    let run: Arc<dyn Work> = Arc::new(AppWork {
        app: app.clone(),
        work: SystemWork::ReminderPlan,
    });
    vec![Direct {
        work: SystemWork::ReminderPlan,
        run,
        due: None,
    }]
}

/// Launches each kind (what a crash left half done, the times missed while Shodh was
/// closed) and adds it to the loop, on a task of its own: reminders ring meanwhile. The
/// repeating tasks' part of the launch summary goes out as soon as they have launched,
/// however that ends.
fn launch_kinds(
    app: AppHandle,
    state: EngineState,
    dispatcher: Arc<Dispatcher>,
    mut report: LaunchReport,
) {
    tauri::async_runtime::spawn(async move {
        match state.repeats(&app).await {
            Ok(repeats) => {
                report.missed = repeats.launch().await;
                dispatcher.add(Arc::new(HandlerTicker(repeats.handler())));
            }
            Err(e) => tracing::warn!(error = %e, "repeating tasks are off"),
        }
        drop(report);
        match state.whats_new(&app).await {
            Ok(whats_new) => {
                whats_new.launch().await;
                dispatcher.add(Arc::new(HandlerTicker(whats_new.handler())));
            }
            Err(e) => tracing::warn!(error = %e, "What's new is off"),
        }
        match state.paper_alerts(&app).await {
            Ok(alerts) => dispatcher.add(Arc::new(HandlerTicker(alerts.handler()))),
            Err(e) => tracing::warn!(error = %e, "paper alerts are off"),
        }
        match state.automations(&app).await {
            Ok(automations) => {
                automations.launch().await;
                let handler = automations.handler();
                dispatcher.add(Arc::new(AutomationsTicker {
                    automations,
                    handler,
                }));
            }
            Err(e) => tracing::warn!(error = %e, "automations are off"),
        }
    });
}

/// The one loop (see the module documentation).
async fn run_loop(
    app: AppHandle,
    state: EngineState,
    dispatcher: Option<Arc<Dispatcher>>,
    mut direct: Vec<Direct>,
) {
    let ticked = dispatcher
        .as_ref()
        .map_or_else(|| Arc::new(Notify::new()), |d| d.ticked());
    let mut offset = local_offset();
    loop {
        // Registered before planning, so a change made meanwhile still wakes the sleep.
        let woken = state.inner.wake.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();
        let ended = ticked.notified();
        tokio::pin!(ended);
        ended.as_mut().enable();
        // A change of time zone moves every reminder's instant: they are planned again.
        let now_offset = local_offset();
        if now_offset != offset {
            offset = now_offset;
            locked(&state.inner.due_now).insert(SystemWork::ReminderPlan);
        }
        let due: Vec<SystemWork> = locked(&state.inner.due_now).drain().collect();
        let now = Utc::now();
        if let Some(dispatcher) = &dispatcher {
            make_due(dispatcher.engine(), &due, now).await;
        }
        let mut next = dispatcher.as_ref().and_then(|d| d.dispatch(now));
        for work in &mut direct {
            if due.contains(&work.work) || work.due.is_some_and(|at| at <= now) {
                work.due = work.run.run().await.next_due;
            }
            if let Some(at) = work.due {
                next = Some(next.map_or(at, |n| n.min(at)));
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(sleep_for(next, Utc::now())) => {
                if let Some(dispatcher) = &dispatcher {
                    dispatcher.mark_all();
                }
            }
            _ = &mut woken => {
                if let Some(dispatcher) = &dispatcher {
                    dispatcher.mark_all();
                }
            }
            _ = &mut ended => {}
        }
    }
}

/// The computer's offset from UTC now.
fn local_offset() -> chrono::FixedOffset {
    *chrono::Local::now().offset()
}

/// Makes the jobs of `works` due at `now`.
async fn make_due(engine: &Arc<Engine>, works: &[SystemWork], now: DateTime<Utc>) {
    if works.is_empty() {
        return;
    }
    let store = engine.store().clone();
    let ids: Vec<String> = works.iter().map(|w| w.job_id()).collect();
    let made = tokio::task::spawn_blocking(move || {
        ids.iter()
            .try_for_each(|id| store.set_due_at_most(id, now).map(|_| ()))
    })
    .await;
    match made {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "system work could not be made due"),
        Err(e) => tracing::warn!(error = %e, "making system work due stopped"),
    }
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

    /// The kinds' ticks started by one turn of the loop, once they have all ended.
    async fn turn(dispatcher: &Dispatcher, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let next = dispatcher.dispatch(now);
        settle(dispatcher, &[]).await;
        next
    }

    /// Waits until no kind but those of `busy` is ticking.
    async fn settle(dispatcher: &Dispatcher, busy: &[JobKind]) {
        let kinds = [
            JobKind::TaskRepeat,
            JobKind::FolderWatch,
            JobKind::PaperAlert,
            JobKind::Playbook,
            JobKind::System,
        ];
        for _ in 0..500 {
            if kinds
                .iter()
                .filter(|k| !busy.contains(k))
                .all(|k| !dispatcher.is_ticking(*k))
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("a tick did not end");
    }

    #[tokio::test]
    async fn one_turn_serves_every_kind_and_the_wake_is_the_earliest() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        daily(&store, "repeat", JobKind::TaskRepeat, 9);
        daily(&store, "folder", JobKind::FolderWatch, 7);
        let engine = Arc::new(Engine::with_clock(
            store.clone(),
            Arc::new(FixedClock(local(23, 9, 1))),
            "engine-commands-test",
        ));
        let repeats = recording(JobKind::TaskRepeat);
        let folders = recording(JobKind::FolderWatch);
        let dispatcher = Dispatcher::new(engine);
        dispatcher.add(Arc::new(HandlerTicker(repeats.clone())));
        dispatcher.add(Arc::new(HandlerTicker(folders.clone())));
        // The first turn ticks every kind (each was just added).
        turn(&dispatcher, local(23, 9, 1)).await;
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
        // Nothing marked and nothing due: no tick, and the wake is the earliest due time.
        let next = turn(&dispatcher, local(23, 9, 2)).await;
        assert_eq!(next, Some(local(24, 7, 0)));
        assert_eq!(
            sleep_for(next, local(23, 9, 2)),
            std::time::Duration::from_secs(60),
            "it sleeps a minute at most"
        );
    }

    #[tokio::test]
    async fn no_jobs_means_no_wake_and_a_minute_of_sleep() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        let engine = Arc::new(Engine::with_clock(
            store,
            Arc::new(FixedClock(local(23, 9, 0))),
            "engine-commands-test",
        ));
        let dispatcher = Dispatcher::new(engine);
        dispatcher.add(Arc::new(HandlerTicker(recording(JobKind::FolderWatch))));
        assert_eq!(turn(&dispatcher, local(23, 9, 0)).await, None);
        assert_eq!(turn(&dispatcher, local(23, 9, 0)).await, None);
        assert_eq!(
            sleep_for(None, local(23, 9, 0)),
            std::time::Duration::from_secs(60)
        );
    }

    /// A Playbook handler whose run waits until it is let go (an agent run of minutes).
    struct Slow {
        started: Notify,
        go: Notify,
    }

    #[async_trait]
    impl JobHandler for Slow {
        fn kind(&self) -> JobKind {
            JobKind::Playbook
        }
        async fn run(&self, _job: &Job, _run: &Run) -> Handled {
            self.started.notify_one();
            self.go.notified().await;
            Handled::Succeeded {
                summary: "done".into(),
                scheduled_for: None,
            }
        }
        async fn folded(&self, _job: &Job, _run: &Run) {}
        async fn held(&self, _job: &Job, _run: &Run) {}
    }

    #[tokio::test]
    async fn a_long_automation_never_holds_a_repeating_task_up() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        daily(&store, "automation", JobKind::Playbook, 9);
        let engine = Arc::new(Engine::with_clock(
            store.clone(),
            Arc::new(FixedClock(local(23, 9, 30))),
            "engine-commands-test",
        ));
        let slow = Arc::new(Slow {
            started: Notify::new(),
            go: Notify::new(),
        });
        let repeats = recording(JobKind::TaskRepeat);
        let dispatcher = Dispatcher::new(engine);
        dispatcher.add(Arc::new(HandlerTicker(slow.clone())));
        dispatcher.add(Arc::new(HandlerTicker(repeats.clone())));
        dispatcher.dispatch(local(23, 9, 30));
        slow.started.notified().await;
        settle(&dispatcher, &[JobKind::Playbook]).await;
        // A repeating task falls due while the automation runs: its tick goes ahead.
        daily(&store, "repeat", JobKind::TaskRepeat, 9);
        dispatcher.mark_all();
        dispatcher.dispatch(local(23, 9, 30));
        settle(&dispatcher, &[JobKind::Playbook]).await;
        assert_eq!(
            *repeats.ran.lock().unwrap_or_else(|e| e.into_inner()),
            ["repeat"]
        );
        assert!(
            dispatcher.is_ticking(JobKind::Playbook),
            "the automation is still running"
        );
        // One automation tick at a time: a second is not started beside it.
        dispatcher.mark_all();
        dispatcher.dispatch(local(23, 9, 30));
        slow.go.notify_one();
        settle(&dispatcher, &[]).await;
        assert_eq!(
            store.runs("automation", 10).unwrap().len(),
            1,
            "one run of the automation"
        );
    }

    /// The text of every Rust file under `dir`, with its path.
    fn sources(dir: &std::path::Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push((path.clone(), std::fs::read_to_string(&path).unwrap()));
            }
        }
    }

    #[test]
    fn scheduled_work_has_one_clock() {
        let mut files = Vec::new();
        sources(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        assert!(files.len() > 50, "the app's sources are read");
        // Built so that this test does not find itself.
        let interval = concat!("time::", "interval(");
        let planned_sleep = concat!("sleep(", "sleep_for(");
        for (path, text) in &files {
            assert!(
                !text.contains(interval),
                "{} keeps a timer of its own; scheduled work runs on the engine's loop",
                path.display()
            );
        }
        let loops: Vec<String> = files
            .iter()
            .filter(|(_, text)| text.contains(planned_sleep))
            .map(|(path, _)| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            loops,
            ["engine_commands.rs"],
            "one loop sleeps until work is due"
        );
        let (_, this) = files
            .iter()
            .find(|(path, _)| path.ends_with("engine_commands.rs"))
            .unwrap();
        assert_eq!(this.matches(planned_sleep).count(), 1, "and only once");
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
}
