//! The app's own scheduled work, as system jobs of the engine (`JobKind::System`): one job
//! per piece of work, one handler that runs them by their `system` input, never shown in
//! Automations, and their runs kept [`SYSTEM_RETENTION_DAYS`] days for support.
//!
//! - **`table_tick`**: the evidence-table scheduler's check ([`crate::table_commands`]),
//!   due when a fill job can go on (its own due time, to the second), not every minute.
//! - **`reminder_plan`**: task reminders ([`crate::reminders`]): due when the next reminder
//!   rings, and at once after a calendar change; reminders missed in a sleep are told in
//!   one notification.
//! - **`code_map_reconcile`**: every code map refreshed nightly at [`NIGHTLY_AT`] local
//!   time, once on wake when the computer slept through it ([`crate::code_map_commands`]).
//!
//! The first two have no schedule: each run says when the work is next due, and anything
//! that changes their state makes them due now ([`crate::engine_commands::due_now`]). A
//! system job is never put on hold (no one could resume it) and does not wait on the
//! backoff ladder: its next due time runs it again.
//!
//! [`SYSTEM_RETENTION_DAYS`]: shodh_rag::engine::store::SYSTEM_RETENTION_DAYS

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveTime, Utc};
use serde_json::json;
use shodh_rag::engine::runner::{Handled, JobHandler};
use shodh_rag::engine::schedule::Schedule;
use shodh_rag::engine::store::{
    CatchUp, CreatedBy, EngineResult, EngineStore, Job, JobKind, JobPatch, NewJob, Overlap, Run,
    RunState,
};
use tauri::AppHandle;

/// When the code maps are refreshed each night, in local time.
pub const NIGHTLY_AT: (u32, u32) = (2, 0);

/// A piece of the app's own scheduled work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemWork {
    TableTick,
    ReminderPlan,
    CodeMapReconcile,
}

impl SystemWork {
    pub const ALL: [SystemWork; 3] = [
        SystemWork::TableTick,
        SystemWork::ReminderPlan,
        SystemWork::CodeMapReconcile,
    ];

    /// Its name, the job's `system` input.
    pub fn name(self) -> &'static str {
        match self {
            Self::TableTick => "table_tick",
            Self::ReminderPlan => "reminder_plan",
            Self::CodeMapReconcile => "code_map_reconcile",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|work| work.name() == name)
    }

    /// The id of its job.
    pub fn job_id(self) -> String {
        format!("system:{}", self.name())
    }

    /// What its job is, for the support log.
    fn title(self) -> &'static str {
        match self {
            Self::TableTick => "Evidence table fills",
            Self::ReminderPlan => "Task reminders",
            Self::CodeMapReconcile => "Nightly code map refresh",
        }
    }

    /// Its schedule: none for work due when its own state says.
    pub fn schedule(self) -> Option<Schedule> {
        match self {
            Self::TableTick | Self::ReminderPlan => None,
            Self::CodeMapReconcile => Some(Schedule::Daily {
                at: NaiveTime::from_hms_opt(NIGHTLY_AT.0, NIGHTLY_AT.1, 0)
                    .unwrap_or(NaiveTime::MIN),
            }),
        }
    }
}

/// What a run of a piece of work came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkDone {
    /// `Handled::Nothing` when it found nothing to do (the run leaves no record).
    pub handled: Handled,
    /// When it is next due, for work without a schedule; `None` until something changes.
    pub next_due: Option<DateTime<Utc>>,
}

impl WorkDone {
    pub fn nothing(next_due: Option<DateTime<Utc>>) -> Self {
        Self {
            handled: Handled::Nothing,
            next_due,
        }
    }

    pub fn did(summary: String, next_due: Option<DateTime<Utc>>) -> Self {
        Self {
            handled: Handled::Succeeded {
                summary,
                scheduled_for: None,
            },
            next_due,
        }
    }
}

/// A piece of work's run.
#[async_trait]
pub trait Work: Send + Sync {
    async fn run(&self) -> WorkDone;
}

/// A piece of this app's work.
pub struct AppWork {
    pub app: AppHandle,
    pub work: SystemWork,
}

impl AppWork {
    /// Every piece of the app's work, for the system handler.
    pub fn all(app: &AppHandle) -> Vec<(SystemWork, Arc<dyn Work>)> {
        SystemWork::ALL
            .into_iter()
            .map(|work| {
                let run: Arc<dyn Work> = Arc::new(AppWork {
                    app: app.clone(),
                    work,
                });
                (work, run)
            })
            .collect()
    }
}

#[async_trait]
impl Work for AppWork {
    async fn run(&self) -> WorkDone {
        match self.work {
            SystemWork::TableTick => crate::table_commands::system_tick(&self.app).await,
            SystemWork::ReminderPlan => crate::reminders::system_plan(&self.app).await,
            SystemWork::CodeMapReconcile => crate::code_map_commands::nightly(&self.app).await,
        }
    }
}

/// Runs system jobs: the piece of work their `system` input names. A run that says when
/// the work is next due makes its job due then.
pub struct SystemHandler {
    store: Arc<EngineStore>,
    works: Vec<(SystemWork, Arc<dyn Work>)>,
}

impl SystemHandler {
    pub fn new(store: Arc<EngineStore>, works: Vec<(SystemWork, Arc<dyn Work>)>) -> Self {
        Self { store, works }
    }

    fn work(&self, job: &Job) -> Option<(SystemWork, &Arc<dyn Work>)> {
        let name = job.input.get("system")?.as_str()?;
        let wanted = SystemWork::parse(name)?;
        self.works
            .iter()
            .find(|(work, _)| *work == wanted)
            .map(|(work, run)| (*work, run))
    }
}

#[async_trait]
impl JobHandler for SystemHandler {
    fn kind(&self) -> JobKind {
        JobKind::System
    }

    async fn run(&self, job: &Job, _run: &Run) -> Handled {
        let Some((work, runner)) = self.work(job) else {
            return Handled::Permanent {
                code: "unknown_system_work".to_string(),
                detail: format!("No system work is named by {}.", job.input),
            };
        };
        let done = runner.run().await;
        if let Some(next) = done.next_due.filter(|_| work.schedule().is_none()) {
            let (store, id) = (self.store.clone(), job.id.clone());
            let set = tokio::task::spawn_blocking(move || store.set_due_at_most(&id, next)).await;
            match set {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    tracing::warn!(work = work.name(), error = %e, "the next due time of system work could not be written")
                }
                Err(e) => {
                    tracing::warn!(work = work.name(), error = %e, "writing the next due time of system work stopped")
                }
            }
        }
        match done.handled {
            // A scheduled run that leaves no record would be planned again at once: it is
            // recorded as done.
            Handled::Nothing if work.schedule().is_some() => Handled::Succeeded {
                summary: "Nothing to do".to_string(),
                scheduled_for: None,
            },
            handled => handled,
        }
    }

    async fn folded(&self, _job: &Job, _run: &Run) {}

    async fn held(&self, _job: &Job, _run: &Run) {}
}

/// Makes the system jobs as this version runs them: each there, on and unheld, with its
/// schedule. Blocking.
pub fn ensure_jobs(store: &EngineStore, now: DateTime<Utc>) -> EngineResult<()> {
    for work in SystemWork::ALL {
        let id = work.job_id();
        let Some(job) = store.job(&id)? else {
            store.create_job(
                &NewJob {
                    id,
                    kind: JobKind::System,
                    subject: work.name().to_string(),
                    title: work.title().to_string(),
                    schedule: work.schedule(),
                    input: json!({ "system": work.name() }),
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 1,
                    created_by: CreatedBy::System,
                    provenance: json!({}),
                },
                now,
            )?;
            continue;
        };
        match work.schedule() {
            Some(schedule) if job.schedule.as_ref() != Some(&schedule) => {
                store.update_job(
                    &id,
                    &JobPatch {
                        schedule: Some(schedule),
                        ..JobPatch::default()
                    },
                    now,
                )?;
            }
            None if job.schedule.is_some() => store.clear_schedule(&id)?,
            _ => {}
        }
        if !job.enabled || job.hold_reason.is_some() {
            store.update_job(
                &id,
                &JobPatch {
                    enabled: Some(true),
                    hold_reason: Some(None),
                    ..JobPatch::default()
                },
                now,
            )?;
        }
    }
    Ok(())
}

/// When the nightly code map refresh last ran (UTC, RFC 3339), or `None` when it has not
/// yet. Blocking.
pub fn last_nightly(store: &EngineStore) -> EngineResult<Option<String>> {
    let runs = store.runs(&SystemWork::CodeMapReconcile.job_id(), 20)?;
    Ok(runs
        .into_iter()
        .find(|run| run.state == RunState::Succeeded)
        .and_then(|run| run.ended_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::engine::clock::Clock;
    use shodh_rag::engine::runner::Engine;
    use std::sync::Mutex;

    struct FixedClock(Mutex<DateTime<Utc>>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    /// Work that counts its runs and says it is next due at `next`.
    struct Counting {
        runs: Mutex<usize>,
        next: Option<DateTime<Utc>>,
    }

    #[async_trait]
    impl Work for Counting {
        async fn run(&self) -> WorkDone {
            *self.runs.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            WorkDone::did("Checked".into(), self.next)
        }
    }

    fn at(h: u32, m: u32, s: u32) -> DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(2026, 11, 28, h, m, s).unwrap()
    }

    #[test]
    fn the_jobs_are_made_once_and_put_back_as_this_version_runs_them() {
        let dir = tempfile::tempdir().unwrap();
        let store = EngineStore::open(&dir.path().join("shodh.db"), None).unwrap();
        ensure_jobs(&store, at(9, 0, 0)).unwrap();
        ensure_jobs(&store, at(9, 1, 0)).unwrap();
        let jobs = store.jobs(JobKind::System).unwrap();
        assert_eq!(jobs.len(), 3);
        let nightly = store
            .job(&SystemWork::CodeMapReconcile.job_id())
            .unwrap()
            .unwrap();
        assert_eq!(
            nightly.schedule,
            Some(Schedule::Daily {
                at: NaiveTime::from_hms_opt(2, 0, 0).unwrap()
            })
        );
        assert_eq!(
            nightly.catch_up,
            CatchUp::Once,
            "once on wake after a missed night"
        );
        // Switched off or held by an older version: put back.
        store
            .update_job(
                &SystemWork::TableTick.job_id(),
                &JobPatch {
                    enabled: Some(false),
                    hold_reason: Some(Some(shodh_rag::engine::store::HoldReason::Failing)),
                    ..JobPatch::default()
                },
                at(9, 2, 0),
            )
            .unwrap();
        ensure_jobs(&store, at(9, 3, 0)).unwrap();
        let tick = store.job(&SystemWork::TableTick.job_id()).unwrap().unwrap();
        assert!(tick.enabled);
        assert_eq!(tick.hold_reason, None);
        assert_eq!(tick.schedule, None);
    }

    #[tokio::test]
    async fn a_run_makes_its_job_due_when_the_work_says() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        ensure_jobs(&store, at(9, 0, 0)).unwrap();
        let clock = Arc::new(FixedClock(Mutex::new(at(9, 0, 0))));
        let engine = Engine::with_clock(store.clone(), clock.clone(), "system-jobs-test");
        let table = Arc::new(Counting {
            runs: Mutex::new(0),
            next: Some(at(9, 0, 37)),
        });
        let handler: Arc<dyn JobHandler> = Arc::new(SystemHandler::new(
            store.clone(),
            vec![(SystemWork::TableTick, table.clone() as Arc<dyn Work>)],
        ));
        store
            .set_due_at_most(&SystemWork::TableTick.job_id(), at(9, 0, 0))
            .unwrap();
        engine.tick(&handler).await.unwrap();
        assert_eq!(*table.runs.lock().unwrap(), 1);
        assert_eq!(
            engine.next_wake(JobKind::System).unwrap(),
            Some(at(9, 0, 37)),
            "a table job due at 09:00:37 runs at 09:00:37, not at the next minute"
        );
        *clock.0.lock().unwrap() = at(9, 0, 36);
        engine.tick(&handler).await.unwrap();
        assert_eq!(*table.runs.lock().unwrap(), 1);
        *clock.0.lock().unwrap() = at(9, 0, 37);
        engine.tick(&handler).await.unwrap();
        assert_eq!(*table.runs.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn the_nightly_refresh_runs_at_two_once_after_a_missed_night_and_never_twice() {
        use chrono::Local;
        use shodh_rag::engine::clock::resolve_local;
        let local = |d: u32, h: u32, m: u32| {
            let naive = chrono::NaiveDate::from_ymd_opt(2026, 11, d)
                .unwrap()
                .and_hms_opt(h, m, 0)
                .unwrap();
            resolve_local(&Local, naive).unwrap()
        };
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        ensure_jobs(&store, local(20, 12, 0)).unwrap();
        let clock = Arc::new(FixedClock(Mutex::new(local(20, 12, 0))));
        let engine = Engine::with_clock(store.clone(), clock.clone(), "system-jobs-test");
        let nightly = Arc::new(Counting {
            runs: Mutex::new(0),
            next: None,
        });
        let handler: Arc<dyn JobHandler> = Arc::new(SystemHandler::new(
            store.clone(),
            vec![(
                SystemWork::CodeMapReconcile,
                nightly.clone() as Arc<dyn Work>,
            )],
        ));
        let runs = || *nightly.runs.lock().unwrap();
        engine.tick(&handler).await.unwrap();
        assert_eq!(runs(), 0, "nothing before 02:00");
        *clock.0.lock().unwrap() = local(21, 2, 0);
        engine.tick(&handler).await.unwrap();
        assert_eq!(runs(), 1, "at 02:00");
        *clock.0.lock().unwrap() = local(21, 2, 1);
        engine.tick(&handler).await.unwrap();
        assert_eq!(runs(), 1, "never twice a night");
        // Asleep through two nights: one refresh on wake.
        *clock.0.lock().unwrap() = local(23, 8, 30);
        engine.tick(&handler).await.unwrap();
        assert_eq!(runs(), 2);
        engine.tick(&handler).await.unwrap();
        assert_eq!(runs(), 2, "one catch-up, not one per missed night");
        assert_eq!(
            last_nightly(&store).unwrap().as_deref(),
            Some(shodh_rag::engine::lease::timestamp(local(23, 8, 30)).as_str()),
            "when it ran, not the night it stood for"
        );
    }
}
