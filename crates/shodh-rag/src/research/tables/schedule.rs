//! Running fill jobs in the background: which jobs to run on each check, a job never run
//! twice at once by one process, and a job that cannot get a model paused for the user
//! instead of left due forever. The app drives it (a check at launch and every minute,
//! and a run at once when the user starts a fill) and receives its events through
//! [`RunEvents`]; nothing here knows the app.
//!
//! On each check ([`Scheduler::tick`]):
//! - the due jobs are read without a model ([`super::jobs::resume_due_jobs`]); with none
//!   due and none waiting for a model, no model is resolved;
//! - a runner is asked of the [`RunnerSource`]:
//!   - it gives one: jobs paused for `model_unavailable` are made due again (the user
//!     fixed the model, the key or the setting), and every due job is run;
//!   - no usable model ([`RunnerError::Blocked`]): every due job is paused with
//!     `model_unavailable` and the reason's code, and [`RunEvents::blocked`] is told once
//!     for each (a paused job is not due, so a later check does not tell it again);
//!   - another failure (the database): the jobs stay as they are; logged once.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::calls::{Clock, SystemClock};
use super::jobs::{
    block_at, due_jobs, process_token, timestamp, FillJob, JobRunner, JobState, PauseReason,
    RunReport,
};
use super::store::TableStore;
use crate::research::{blocking, ResearchResult};

/// Why no runner could be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerError {
    /// No usable model: `code` says why (`refused_reasoning`, `local_only`, `no_key`,
    /// `no_model`), `message` says it in words.
    Blocked { code: &'static str, message: String },
    /// Something else failed (the database could not be opened, …).
    Unavailable(String),
}

/// Makes a runner with the model to use now.
#[async_trait::async_trait]
pub trait RunnerSource: Send + Sync {
    async fn runner(&self) -> Result<JobRunner, RunnerError>;
}

/// What the scheduler tells the app.
pub trait RunEvents: Send + Sync {
    /// Something in the table changed (a run ended, a job was paused).
    fn changed(&self, table_id: &str);
    /// A run of `job` (as it was before the run) ended with `report`. `by_user`: the run
    /// was started by the user's Fill, not by a check.
    fn ended(&self, job: &FillJob, report: &RunReport, by_user: bool);
    /// `job` was paused because no model can be used, for `message`.
    fn blocked(&self, job: &FillJob, message: &str);
}

/// What one check did.
#[derive(Debug, Default)]
pub struct Tick {
    /// Jobs due when the check began.
    pub due: usize,
    /// A runner was asked for.
    pub asked: bool,
    /// Jobs paused because no model can be used.
    pub blocked: Vec<String>,
    /// Jobs waiting for a model that were made due again.
    pub woken: usize,
    /// The runs started.
    pub runs: Vec<tokio::task::JoinHandle<()>>,
}

/// The background runner of fill jobs of this process.
pub struct Scheduler {
    in_flight: Mutex<HashSet<String>>,
    logged: AtomicBool,
    token: String,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler").finish_non_exhaustive()
    }
}

/// A job's place in the in-flight set, given back when its run's task ends.
struct InFlight {
    scheduler: Arc<Scheduler>,
    job_id: String,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.scheduler
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.job_id);
    }
}

impl Scheduler {
    /// A scheduler of this process, on the computer's clock.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            in_flight: Mutex::new(HashSet::new()),
            logged: AtomicBool::new(false),
            token: process_token(),
            clock: Arc::new(SystemClock),
        })
    }

    /// A scheduler of another process (`token`) on `clock` (tests).
    #[cfg(test)]
    pub(crate) fn with(token: &str, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            in_flight: Mutex::new(HashSet::new()),
            logged: AtomicBool::new(false),
            token: token.to_string(),
            clock,
        })
    }

    /// Whether this scheduler has a run of the job going.
    pub fn is_running(&self, job_id: &str) -> bool {
        self.in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(job_id)
    }

    fn enter(self: &Arc<Self>, job_id: &str) -> Option<InFlight> {
        let entered = self
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(job_id.to_string());
        entered.then(|| InFlight {
            scheduler: self.clone(),
            job_id: job_id.to_string(),
        })
    }

    /// Runs the job on a task of its own, unless this scheduler runs it already (then
    /// `None`). At its end the table's change and the run's end are told.
    pub fn spawn_run(
        self: &Arc<Self>,
        runner: JobRunner,
        job_id: String,
        events: Arc<dyn RunEvents>,
        by_user: bool,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let in_flight = self.enter(&job_id)?;
        Some(tokio::spawn(async move {
            let _in_flight = in_flight;
            let job = match runner.job(&job_id).await {
                Ok(Some(job)) => job,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!(job = %job_id, error = %e, "evidence table fill job could not be read");
                    return;
                }
            };
            let report = runner.run(&job_id).await;
            events.changed(&job.table_id);
            match report {
                Ok(report) => events.ended(&job, &report, by_user),
                Err(e) => tracing::warn!(
                    job = %job_id,
                    error = %e,
                    "evidence table fill run failed; the job is given back"
                ),
            }
        }))
    }

    /// One check (see the module documentation).
    pub async fn tick(
        self: &Arc<Self>,
        store: Arc<TableStore>,
        source: &dyn RunnerSource,
        events: Arc<dyn RunEvents>,
    ) -> ResearchResult<Tick> {
        let mut tick = Tick::default();
        let now = self.clock.now();
        let due = due_jobs(store.clone(), self.token.clone(), now).await?;
        let s = store.clone();
        let waiting = blocking(move || s.paused_fill_jobs(PauseReason::ModelUnavailable)).await?;
        tick.due = due.len();
        if due.is_empty() && waiting.is_empty() {
            return Ok(tick);
        }
        tick.asked = true;
        match source.runner().await {
            Ok(runner) => {
                self.logged.store(false, Ordering::SeqCst);
                let due = if waiting.is_empty() {
                    due
                } else {
                    let (s, at) = (store.clone(), timestamp(now));
                    let woken = blocking(move || {
                        s.wake_paused_fill_jobs(PauseReason::ModelUnavailable, &at)
                    })
                    .await?;
                    tick.woken = woken.len();
                    for job in &woken {
                        events.changed(&job.table_id);
                    }
                    due_jobs(store, self.token.clone(), now).await?
                };
                for job_id in due {
                    if let Some(run) = self.spawn_run(runner.clone(), job_id, events.clone(), false)
                    {
                        tick.runs.push(run);
                    }
                }
            }
            Err(RunnerError::Blocked { code, message }) => {
                for job_id in due {
                    if self.is_running(&job_id) {
                        continue;
                    }
                    let job =
                        block_at(store.clone(), &job_id, code, self.token.clone(), now).await?;
                    let Some(job) = job else { continue };
                    if job.state == JobState::Paused
                        && job.pause_reason == Some(PauseReason::ModelUnavailable)
                    {
                        events.changed(&job.table_id);
                        events.blocked(&job, &message);
                        tick.blocked.push(job.id);
                    }
                }
            }
            Err(RunnerError::Unavailable(message)) => {
                if !self.logged.swap(true, Ordering::SeqCst) {
                    tracing::warn!(jobs = due.len(), error = %message, "evidence table fills wait: no runner could be made");
                }
            }
        }
        Ok(tick)
    }
}
