//! The engine: plans every job on each tick, runs what is due one at a time through the
//! job kind's [`JobHandler`], waits on the backoff ladder after a transient failure, and
//! puts a job on hold after [`FAILED_RUNS_BEFORE_HOLD`] failed runs in a row (never a
//! system job: no one could resume it).
//!
//! A job without a schedule runs once at a due time its owner sets
//! ([`EngineStore::set_due_at_most`]): the app's system work is due when its own state says
//! (a reminder rings, a table fill's wait ends), to the second.
//!
//! The app drives it: a tick at launch, after every wake and at least every minute
//! ([`plan::sleep_for`]), and [`Engine::event`] when something a job waits on happens.
//! Nothing here knows the app; schedules are planned in the computer's time zone.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Local, TimeDelta, Utc};

use super::backoff::BACKOFF_LADDER_MINUTES;
use super::clock::{Clock, SystemClock};
use super::lease::{lease_cutoff, process_token, timestamp, LiveRun};
use super::plan::{self, Step, FAILED_RUNS_BEFORE_HOLD};
use super::store::{
    parse_instant, End, EngineError, EngineResult, EngineStore, HoldReason, Job, JobKind, JobPatch,
    NewRun, Run, RunState, Trigger, WaitReason,
};

/// What a handler's run came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handled {
    /// Done; `scheduled_for` is the occurrence the run turned out to be for, when it chose
    /// one.
    Succeeded {
        summary: String,
        scheduled_for: Option<DateTime<Utc>>,
    },
    Skipped {
        summary: String,
    },
    /// Waiting for `reason` (the user's decision, an approval).
    Waiting {
        reason: WaitReason,
    },
    /// Nothing to do: the run leaves no record.
    Nothing,
    /// Worth trying again later (a file that could not be read, the network).
    Transient {
        code: String,
        detail: String,
    },
    /// Worth trying again at `at`, waiting as `reason` (`Backoff` or `Offline`): a rate
    /// limit's reset, a retry an hour later, the computer offline. Counts as an attempt
    /// like [`Handled::Transient`]; past `max_attempts` the run fails.
    RetryAt {
        at: DateTime<Utc>,
        reason: WaitReason,
        code: String,
        detail: String,
    },
    /// Not now, through no failure: wait as `reason` until `at` (while the user chats, until
    /// a quota resets, a moment after the computer woke), then run again. It is not an
    /// attempt, so however often it happens it never fails the run or holds the job.
    Defer {
        at: DateTime<Utc>,
        reason: WaitReason,
    },
    /// Trying again would not help.
    Permanent {
        code: String,
        detail: String,
    },
}

/// The work of one kind of job.
#[async_trait::async_trait]
pub trait JobHandler: Send + Sync {
    fn kind(&self) -> JobKind;
    /// Does the run's work. It may be called again for the same run after a crash or a
    /// lease takeover, so its effects must be idempotent.
    async fn run(&self, job: &Job, run: &Run) -> Handled;
    /// A waiting decision moved to a newer occurrence ([`Run::scheduled_for`]).
    async fn folded(&self, job: &Job, run: &Run);
    /// The job was put on hold after failing again and again; `run` is its last run.
    async fn held(&self, job: &Job, run: &Run);
}

/// What one tick did.
#[derive(Debug, Default)]
pub struct Tick {
    /// Runs started or resumed, in order.
    pub ran: Vec<String>,
    /// Waiting decisions that moved to a newer occurrence.
    pub folded: Vec<String>,
    /// Skipped runs recorded.
    pub skipped: Vec<String>,
    /// Jobs put on hold.
    pub held: Vec<String>,
}

/// The scheduler engine of this process.
pub struct Engine {
    store: Arc<EngineStore>,
    clock: Arc<dyn Clock>,
    token: String,
    /// Jobs with an event not yet served: it came while a run of the job was going, and
    /// that run serves it when it ends (so a completion is never lost to a run in flight).
    pending: Mutex<HashSet<String>>,
    /// Jobs a run of this engine is doing the work of now.
    running: Mutex<HashSet<String>>,
}

fn locked(set: &Mutex<HashSet<String>>) -> MutexGuard<'_, HashSet<String>> {
    set.lock().unwrap_or_else(|e| e.into_inner())
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").finish_non_exhaustive()
    }
}

async fn blocking<T, F>(store: &Arc<EngineStore>, work: F) -> EngineResult<T>
where
    T: Send + 'static,
    F: FnOnce(&EngineStore) -> EngineResult<T> + Send + 'static,
{
    let store = store.clone();
    tokio::task::spawn_blocking(move || work(&store))
        .await
        .map_err(|e| EngineError::Task(e.to_string()))?
}

fn new_run_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Engine {
    /// The engine of this process, on the computer's clock.
    pub fn new(store: Arc<EngineStore>) -> Self {
        Self {
            store,
            clock: Arc::new(SystemClock),
            token: process_token(),
            pending: Mutex::new(HashSet::new()),
            running: Mutex::new(HashSet::new()),
        }
    }

    /// An engine on `clock` holding `token` (tests simulate another process with it).
    pub fn with_clock(store: Arc<EngineStore>, clock: Arc<dyn Clock>, token: &str) -> Self {
        Self {
            store,
            clock,
            token: token.to_string(),
            pending: Mutex::new(HashSet::new()),
            running: Mutex::new(HashSet::new()),
        }
    }

    pub fn store(&self) -> &Arc<EngineStore> {
        &self.store
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// The latest occurrence a job has handled: its newest run's occurrence, or when it
    /// was made.
    pub fn planned_through(&self, job: &Job) -> EngineResult<DateTime<Utc>> {
        let created = parse_instant(&job.created_at).unwrap_or_else(|| self.clock.now());
        let latest = self.store.latest_scheduled_for(&job.id)?;
        Ok(latest.map_or(created, |latest| latest.max(created)))
    }

    /// The job's next due time after now, as planned.
    pub fn next_due(&self, job: &Job) -> EngineResult<Option<DateTime<Utc>>> {
        let Some(schedule) = &job.schedule else {
            return Ok(None);
        };
        let through = self.planned_through(job)?;
        Ok(schedule.next_after(&Local, through.max(self.clock.now())))
    }

    /// Whether this engine could take the run up now.
    fn resumable(&self, run: &Run, now: DateTime<Utc>) -> bool {
        match run.state {
            RunState::Queued => true,
            RunState::Waiting => run
                .next_attempt_at
                .as_deref()
                .and_then(parse_instant)
                .is_some_and(|next| next <= now),
            RunState::Running => {
                if run.run_token.as_deref() == Some(self.token.as_str()) {
                    !LiveRun::is_live(&self.token, &run.id)
                } else {
                    let cutoff = lease_cutoff(now);
                    run.heartbeat_at
                        .as_deref()
                        .is_none_or(|beat| beat <= cutoff.as_str())
                }
            }
            _ => false,
        }
    }

    /// One tick for the jobs of `handler`'s kind (see the module documentation).
    pub async fn tick(&self, handler: &Arc<dyn JobHandler>) -> EngineResult<Tick> {
        self.tick_where(handler, &|_: &Job| true).await
    }

    /// One tick for the jobs of `handler`'s kind that `wanted` picks: the app ticks each of
    /// its own pieces of work apart, so a slow one never holds another up.
    pub async fn tick_where(
        &self,
        handler: &Arc<dyn JobHandler>,
        wanted: &(dyn Fn(&Job) -> bool + Send + Sync),
    ) -> EngineResult<Tick> {
        let mut tick = Tick::default();
        let now = self.clock.now();
        let kind = handler.kind();
        let jobs = blocking(&self.store, move |s| s.jobs(kind)).await?;
        let mut starts: Vec<(DateTime<Utc>, String, String)> = Vec::new();
        for job in jobs {
            if !job.enabled || job.hold_reason.is_some() || !wanted(&job) {
                continue;
            }
            let id = job.id.clone();
            let active = blocking(&self.store, move |s| s.active_run(&id)).await?;
            if let Some(run) = &active {
                if self.resumable(run, now) {
                    let at = run.scheduled_at().unwrap_or(now);
                    starts.push((at, job.id.clone(), run.id.clone()));
                }
            }
            let Some(schedule) = &job.schedule else {
                // A due time its owner set: taken when it has come, and served by the
                // active run when there is one (it reads the same state). Most jobs without
                // a schedule have none, and are not written to.
                let come = job
                    .next_due_at
                    .as_deref()
                    .and_then(parse_instant)
                    .is_some_and(|due| due <= now);
                if !come {
                    continue;
                }
                let id = job.id.clone();
                let due = blocking(&self.store, move |s| s.take_due(&id, now)).await?;
                if let (Some(due), None) = (due, &active) {
                    let step = Step::Start {
                        trigger: Trigger::Schedule,
                        scheduled_for: due,
                        missed: 0,
                    };
                    self.apply_step(handler, &job, step, now, &mut tick, &mut starts)
                        .await?;
                }
                continue;
            };
            let through = self.planned_through(&job)?;
            let step = plan::plan(
                &Local,
                schedule,
                job.overlap,
                job.catch_up,
                active.as_ref(),
                through,
                now,
            );
            self.apply_step(handler, &job, step, now, &mut tick, &mut starts)
                .await?;
            let next = self.next_due(&job)?.map(timestamp);
            if next != job.next_due_at {
                let id = job.id.clone();
                blocking(&self.store, move |s| {
                    s.update_job(
                        &id,
                        &JobPatch {
                            next_due_at: Some(next),
                            ..JobPatch::default()
                        },
                        now,
                    )
                })
                .await?;
            }
        }
        starts.sort();
        starts.dedup_by(|a, b| a.2 == b.2);
        for (_, job_id, run_id) in starts {
            if let Some(run) = self.execute(handler, &job_id, &run_id, false).await? {
                tick.ran.push(run.id.clone());
                let job = blocking(&self.store, move |s| s.job(&job_id)).await?;
                if job.is_some_and(|j| j.hold_reason == Some(HoldReason::Failing))
                    && run.state == RunState::Failed
                {
                    tick.held.push(run.job_id.clone());
                }
            }
        }
        blocking(&self.store, move |s| s.prune(now)).await?;
        Ok(tick)
    }

    async fn apply_step(
        &self,
        handler: &Arc<dyn JobHandler>,
        job: &Job,
        step: Step,
        now: DateTime<Utc>,
        tick: &mut Tick,
        starts: &mut Vec<(DateTime<Utc>, String, String)>,
    ) -> EngineResult<()> {
        match step {
            Step::Nothing => {}
            Step::Fold {
                run_id,
                to,
                add_missed,
            } => {
                let folded =
                    blocking(&self.store, move |s| s.fold(&run_id, to, add_missed)).await?;
                if let Some(run) = folded {
                    // Written first, told after: a crash can lose an alert, never ring
                    // one twice.
                    handler.folded(job, &run).await;
                    tick.folded.push(run.id);
                }
            }
            Step::RecordSkipped {
                trigger,
                scheduled_for,
                missed,
                summary,
            } => {
                let new = NewRun {
                    id: new_run_id(),
                    job_id: job.id.clone(),
                    trigger,
                    scheduled_for: Some(scheduled_for),
                    missed,
                    state: RunState::Skipped,
                    summary: Some(summary),
                };
                if let Ok(run) = blocking(&self.store, move |s| s.insert_run(&new, now)).await? {
                    tick.skipped.push(run.id);
                }
            }
            Step::Start {
                trigger,
                scheduled_for,
                missed,
            } => {
                let new = NewRun {
                    id: new_run_id(),
                    job_id: job.id.clone(),
                    trigger,
                    scheduled_for: Some(scheduled_for),
                    missed,
                    state: RunState::Queued,
                    summary: None,
                };
                if let Ok(run) = blocking(&self.store, move |s| s.insert_run(&new, now)).await? {
                    starts.push((scheduled_for, job.id.clone(), run.id));
                }
            }
        }
        Ok(())
    }

    /// Claims the run and does its work; `wake` claims a waiting run too (the user or an
    /// event answered it). `None` when it could not be claimed (another run holds it).
    async fn execute(
        &self,
        handler: &Arc<dyn JobHandler>,
        job_id: &str,
        run_id: &str,
        wake: bool,
    ) -> EngineResult<Option<Run>> {
        locked(&self.running).insert(job_id.to_string());
        let result = self.execute_once(handler, job_id, run_id, wake).await;
        locked(&self.running).remove(job_id);
        let mut result = result?;
        // An event that came while the run went (its marker is set before it looks for a
        // run in flight, and this looks after the run is no longer in flight).
        while self.take_pending(job_id) {
            match self.serve_event(handler, job_id).await? {
                Some(run) => result = Some(run),
                None => break,
            }
        }
        Ok(result)
    }

    fn take_pending(&self, job_id: &str) -> bool {
        locked(&self.pending).remove(job_id)
    }

    async fn execute_once(
        &self,
        handler: &Arc<dyn JobHandler>,
        job_id: &str,
        run_id: &str,
        wake: bool,
    ) -> EngineResult<Option<Run>> {
        let Some(_slot) = LiveRun::enter(&self.token, run_id) else {
            return Ok(None);
        };
        let Some((job, run)) = self.claim(job_id, run_id, wake).await? else {
            return Ok(None);
        };
        let worker = handler.clone();
        let (j, r) = (job.clone(), run.clone());
        let handled = match tokio::spawn(async move { worker.run(&j, &r).await }).await {
            Ok(handled) => handled,
            Err(e) => Handled::Permanent {
                code: "panicked".to_string(),
                detail: e.to_string(),
            },
        };
        self.apply(handler, &job, &run, handled).await
    }

    async fn claim(
        &self,
        job_id: &str,
        run_id: &str,
        wake: bool,
    ) -> EngineResult<Option<(Job, Run)>> {
        let now = self.clock.now();
        let cutoff = lease_cutoff(now);
        let (token, id) = (self.token.clone(), run_id.to_string());
        let claimed = blocking(&self.store, move |s| {
            s.claim(&id, &token, false, wake, now, &cutoff)
        })
        .await?;
        let Some(run) = claimed else {
            return Ok(None);
        };
        let id = job_id.to_string();
        let Some(job) = blocking(&self.store, move |s| s.job(&id)).await? else {
            return Ok(None);
        };
        Ok(Some((job, run)))
    }

    /// Writes what the run came to.
    async fn apply(
        &self,
        handler: &Arc<dyn JobHandler>,
        job: &Job,
        run: &Run,
        handled: Handled,
    ) -> EngineResult<Option<Run>> {
        let now = self.clock.now();
        let (token, id) = (self.token.clone(), run.id.clone());
        match handled {
            Handled::Succeeded {
                summary,
                scheduled_for,
            } => {
                let end = End::Succeeded {
                    summary,
                    scheduled_for,
                };
                let ended =
                    blocking(&self.store, move |s| s.finish(&id, &token, &end, now)).await?;
                let job_id = job.id.clone();
                blocking(&self.store, move |s| {
                    s.update_job(
                        &job_id,
                        &JobPatch {
                            reset_failures: true,
                            ..JobPatch::default()
                        },
                        now,
                    )
                })
                .await?;
                Ok(ended)
            }
            Handled::Skipped { summary } => {
                let end = End::Skipped { summary };
                blocking(&self.store, move |s| s.finish(&id, &token, &end, now)).await
            }
            Handled::Nothing => {
                blocking(&self.store, move |s| s.delete_run(&id, &token)).await?;
                Ok(None)
            }
            Handled::Waiting { reason } => {
                let attempt = run.attempt;
                blocking(&self.store, move |s| {
                    s.wait(&id, &token, reason, None, attempt, None)
                })
                .await
            }
            Handled::Transient { code, detail } => {
                let attempt = run.attempt.saturating_add(1);
                if attempt < job.max_attempts {
                    let rung = usize::try_from(attempt - 1)
                        .unwrap_or(usize::MAX)
                        .min(BACKOFF_LADDER_MINUTES.len() - 1);
                    let until = now + TimeDelta::minutes(BACKOFF_LADDER_MINUTES[rung]);
                    blocking(&self.store, move |s| {
                        s.wait(
                            &id,
                            &token,
                            WaitReason::Backoff,
                            Some(until),
                            attempt,
                            Some((code.as_str(), detail.as_str())),
                        )
                    })
                    .await
                } else {
                    self.fail(handler, job, run, code, detail).await
                }
            }
            Handled::RetryAt {
                at,
                reason,
                code,
                detail,
            } => {
                let attempt = run.attempt.saturating_add(1);
                if attempt < job.max_attempts {
                    blocking(&self.store, move |s| {
                        s.wait(
                            &id,
                            &token,
                            reason,
                            Some(at),
                            attempt,
                            Some((code.as_str(), detail.as_str())),
                        )
                    })
                    .await
                } else {
                    self.fail(handler, job, run, code, detail).await
                }
            }
            Handled::Defer { at, reason } => {
                let attempt = run.attempt;
                blocking(&self.store, move |s| {
                    s.wait(&id, &token, reason, Some(at), attempt, None)
                })
                .await
            }
            Handled::Permanent { code, detail } => self.fail(handler, job, run, code, detail).await,
        }
    }

    async fn fail(
        &self,
        handler: &Arc<dyn JobHandler>,
        job: &Job,
        run: &Run,
        code: String,
        detail: String,
    ) -> EngineResult<Option<Run>> {
        let now = self.clock.now();
        let (token, id) = (self.token.clone(), run.id.clone());
        let end = End::Failed { code, detail };
        let ended = blocking(&self.store, move |s| s.finish(&id, &token, &end, now)).await?;
        let Some(ended) = ended else {
            return Ok(None);
        };
        let job_id = job.id.clone();
        let streak = blocking(&self.store, move |s| s.record_failure(&job_id)).await?;
        // A system job is the app's own work, which no one sees and so no one could
        // resume: it is never put on hold, and its next due time runs it again.
        if streak >= FAILED_RUNS_BEFORE_HOLD && job.kind != JobKind::System {
            let job_id = job.id.clone();
            let held = blocking(&self.store, move |s| {
                s.update_job(
                    &job_id,
                    &JobPatch {
                        hold_reason: Some(Some(HoldReason::Failing)),
                        ..JobPatch::default()
                    },
                    now,
                )
            })
            .await?;
            if let Some(held) = held {
                handler.held(&held, &ended).await;
            }
        }
        Ok(Some(ended))
    }

    /// Something the job waits on happened (a repeating task's latest copy was done): its
    /// waiting decision is taken up again, or a run is started for the event. Nothing is
    /// started while another run of the job is active (queued or running), since that run
    /// reads the same state.
    pub async fn event(
        &self,
        handler: &Arc<dyn JobHandler>,
        job_id: &str,
    ) -> EngineResult<Option<Run>> {
        let id = job_id.to_string();
        let job = blocking(&self.store, move |s| s.job(&id)).await?;
        if job.is_none_or(|j| !j.enabled || j.hold_reason.is_some()) {
            // A job on hold runs again when it is resumed, which looks at the state anew.
            return Ok(None);
        }
        locked(&self.pending).insert(job_id.to_string());
        if locked(&self.running).contains(job_id) {
            // The run in flight serves it when it ends.
            return Ok(None);
        }
        let mut result = None;
        while self.take_pending(job_id) {
            match self.serve_event(handler, job_id).await? {
                Some(run) => result = Some(run),
                None => break,
            }
        }
        Ok(result)
    }

    /// Serves one event of a job: its waiting decision is taken up again, or a run is
    /// started for it. While another run of the job is active (waiting on the ladder, or in
    /// another process), the event stays pending for that run.
    async fn serve_event(
        &self,
        handler: &Arc<dyn JobHandler>,
        job_id: &str,
    ) -> EngineResult<Option<Run>> {
        let id = job_id.to_string();
        let active = blocking(&self.store, move |s| s.active_run(&id)).await?;
        match active {
            Some(run)
                if run.state == RunState::Waiting
                    && run.wait_reason == Some(WaitReason::Decision) =>
            {
                self.execute_once(handler, job_id, &run.id, true).await
            }
            Some(_) => {
                locked(&self.pending).insert(job_id.to_string());
                Ok(None)
            }
            None => {
                let new = NewRun {
                    id: new_run_id(),
                    job_id: job_id.to_string(),
                    trigger: Trigger::Event,
                    scheduled_for: None,
                    missed: 0,
                    state: RunState::Queued,
                    summary: None,
                };
                let now = self.clock.now();
                match blocking(&self.store, move |s| s.insert_run(&new, now)).await? {
                    Ok(run) => self.execute_once(handler, job_id, &run.id, false).await,
                    Err(_) => Ok(None),
                }
            }
        }
    }

    /// Ends a waiting run with what the user decided (`handled`), without calling the
    /// handler. `None` when the run no longer waits.
    pub async fn settle(
        &self,
        handler: &Arc<dyn JobHandler>,
        job_id: &str,
        run_id: &str,
        handled: Handled,
    ) -> EngineResult<Option<Run>> {
        let Some(_slot) = LiveRun::enter(&self.token, run_id) else {
            return Ok(None);
        };
        let id = run_id.to_string();
        let current = blocking(&self.store, move |s| s.run(&id)).await?;
        if current.is_none_or(|r| r.state != RunState::Waiting) {
            return Ok(None);
        }
        let Some((job, run)) = self.claim(job_id, run_id, true).await? else {
            return Ok(None);
        };
        self.apply(handler, &job, &run, handled).await
    }

    /// Takes a run that waits for the user (an approval, a decision) up again through the
    /// handler, which reads what the user decided. `None` when it no longer waits or another
    /// run holds it.
    pub async fn resume(
        &self,
        handler: &Arc<dyn JobHandler>,
        job_id: &str,
        run_id: &str,
    ) -> EngineResult<Option<Run>> {
        let id = run_id.to_string();
        let current = blocking(&self.store, move |s| s.run(&id)).await?;
        if current.is_none_or(|r| r.state != RunState::Waiting) {
            return Ok(None);
        }
        locked(&self.running).insert(job_id.to_string());
        let result = self.execute_once(handler, job_id, run_id, true).await;
        locked(&self.running).remove(job_id);
        result
    }

    /// Runs a queued run of a job now (a manual run), without waiting for a tick.
    pub async fn run_now(
        &self,
        handler: &Arc<dyn JobHandler>,
        job_id: &str,
        run_id: &str,
    ) -> EngineResult<Option<Run>> {
        self.execute(handler, job_id, run_id, false).await
    }

    /// When the next tick of `kind` should be: the earliest due time of an enabled, unheld
    /// job of that kind, or of a retry of one (a tick of one kind never takes up another's).
    pub fn next_wake(&self, kind: JobKind) -> EngineResult<Option<DateTime<Utc>>> {
        let mut next = self.store.next_retry_at(kind)?;
        for job in self.store.jobs(kind)? {
            if !job.enabled || job.hold_reason.is_some() {
                continue;
            }
            if let Some(due) = job.next_due_at.as_deref().and_then(parse_instant) {
                next = Some(next.map_or(due, |n| n.min(due)));
            }
        }
        Ok(next)
    }

    /// [`Self::next_wake`] for the jobs of `kind` that `wanted` picks: their due times, and
    /// the retry times of their waiting runs.
    pub fn next_wake_where(
        &self,
        kind: JobKind,
        wanted: &(dyn Fn(&Job) -> bool + Send + Sync),
    ) -> EngineResult<Option<DateTime<Utc>>> {
        let mut next: Option<DateTime<Utc>> = None;
        for job in self.store.jobs(kind)? {
            if !job.enabled || job.hold_reason.is_some() || !wanted(&job) {
                continue;
            }
            let retry = self
                .store
                .active_run(&job.id)?
                .filter(|run| run.state == RunState::Waiting)
                .and_then(|run| run.next_attempt_at.as_deref().and_then(parse_instant));
            let due = job.next_due_at.as_deref().and_then(parse_instant);
            for at in [due, retry].into_iter().flatten() {
                next = Some(next.map_or(at, |n| n.min(at)));
            }
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::schedule::Schedule;
    use crate::engine::store::{CatchUp, CreatedBy, NewJob, Overlap};
    use crate::engine::testkit::{local, ManualClock, Recording};
    use chrono::NaiveTime;
    use serde_json::json;

    struct Env {
        _dir: tempfile::TempDir,
        store: Arc<EngineStore>,
        clock: Arc<ManualClock>,
        engine: Engine,
        recording: Arc<Recording>,
        handler: Arc<dyn JobHandler>,
    }

    fn env(start: DateTime<Utc>) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        let clock = Arc::new(ManualClock::new(start));
        let engine = Engine::with_clock(store.clone(), clock.clone(), "engine-a");
        let recording = Arc::new(Recording::new(JobKind::TaskRepeat));
        let handler: Arc<dyn JobHandler> = recording.clone();
        Env {
            _dir: dir,
            store,
            clock,
            engine,
            recording,
            handler,
        }
    }

    fn daily_at(h: u32) -> Schedule {
        Schedule::Daily {
            at: NaiveTime::from_hms_opt(h, 0, 0).unwrap(),
        }
    }

    fn add_job(e: &Env, id: &str, schedule: Option<Schedule>, overlap: Overlap, catch_up: CatchUp) {
        e.store
            .create_job(
                &NewJob {
                    id: id.to_string(),
                    kind: JobKind::TaskRepeat,
                    subject: format!("task-{id}"),
                    title: id.to_string(),
                    schedule,
                    input: json!({}),
                    overlap,
                    catch_up,
                    max_attempts: 3,
                    created_by: CreatedBy::User,
                    provenance: json!({}),
                },
                e.clock.now(),
            )
            .unwrap();
    }

    fn decision() -> Handled {
        Handled::Waiting {
            reason: WaitReason::Decision,
        }
    }

    fn transient() -> Handled {
        Handled::Transient {
            code: "calendar_unreadable".into(),
            detail: "locked".into(),
        }
    }

    /// An engine with one system job (no schedule) and a recording System handler.
    fn system_env(start: DateTime<Utc>) -> (Env, Arc<Recording>, Arc<dyn JobHandler>) {
        let e = env(start);
        e.store
            .create_job(
                &NewJob {
                    id: "table_tick".to_string(),
                    kind: JobKind::System,
                    subject: "table_tick".to_string(),
                    title: "table_tick".to_string(),
                    schedule: None,
                    input: json!({ "system": "table_tick" }),
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 3,
                    created_by: CreatedBy::System,
                    provenance: json!({}),
                },
                start,
            )
            .unwrap();
        let recording = Arc::new(Recording::new(JobKind::System));
        let handler: Arc<dyn JobHandler> = recording.clone();
        (e, recording, handler)
    }

    #[tokio::test]
    async fn a_due_time_set_by_its_owner_runs_at_that_time_and_once() {
        let start = local(2026, 11, 20, 10, 0);
        let (e, recording, handler) = system_env(start);
        let due = start + TimeDelta::seconds(42);
        assert!(e.store.set_due_at_most("table_tick", due).unwrap());
        assert_eq!(
            e.engine.next_wake(JobKind::System).unwrap(),
            Some(due),
            "the loop wakes at the due time itself, not at the next minute"
        );
        e.clock.set(due - TimeDelta::milliseconds(1));
        e.engine.tick(&handler).await.unwrap();
        assert!(recording.calls().is_empty(), "not before it");
        e.clock.set(due);
        let tick = e.engine.tick(&handler).await.unwrap();
        assert_eq!(tick.ran.len(), 1);
        assert_eq!(recording.calls()[0].scheduled_at(), Some(due));
        e.clock.set(due + TimeDelta::minutes(5));
        e.engine.tick(&handler).await.unwrap();
        assert_eq!(recording.calls().len(), 1, "a due time runs once");
        assert_eq!(e.engine.next_wake(JobKind::System).unwrap(), None);
    }

    #[tokio::test]
    async fn a_tick_of_some_jobs_leaves_the_others_due() {
        let start = local(2026, 11, 20, 10, 0);
        let (e, recording, handler) = system_env(start);
        e.store
            .create_job(
                &NewJob {
                    id: "reminder_plan".to_string(),
                    kind: JobKind::System,
                    subject: "reminder_plan".to_string(),
                    title: "reminder_plan".to_string(),
                    schedule: None,
                    input: json!({ "system": "reminder_plan" }),
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 1,
                    created_by: CreatedBy::System,
                    provenance: json!({}),
                },
                start,
            )
            .unwrap();
        let due = start + TimeDelta::seconds(5);
        e.store.set_due_at_most("table_tick", due).unwrap();
        e.store.set_due_at_most("reminder_plan", due).unwrap();
        let reminders = |job: &Job| job.id == "reminder_plan";
        assert_eq!(
            e.engine
                .next_wake_where(JobKind::System, &reminders)
                .unwrap(),
            Some(due)
        );
        e.clock.set(due);
        e.engine.tick_where(&handler, &reminders).await.unwrap();
        let ran: Vec<String> = recording.calls().into_iter().map(|r| r.job_id).collect();
        assert_eq!(ran, ["reminder_plan"], "only the work asked for");
        assert_eq!(
            e.engine
                .next_wake_where(JobKind::System, &reminders)
                .unwrap(),
            None
        );
        let tables = |job: &Job| job.id == "table_tick";
        assert_eq!(
            e.engine.next_wake_where(JobKind::System, &tables).unwrap(),
            Some(due),
            "the other is still due"
        );
    }

    #[tokio::test]
    async fn a_failing_system_job_is_never_held() {
        let start = local(2026, 11, 20, 10, 0);
        let (e, recording, handler) = system_env(start);
        let permanent = || Handled::Permanent {
            code: "broken".into(),
            detail: "it broke".into(),
        };
        recording.script(vec![permanent(), permanent(), permanent(), permanent()]);
        for minute in 1..=4 {
            let at = start + TimeDelta::minutes(minute);
            e.clock.set(at);
            e.store.set_due_at_most("table_tick", at).unwrap();
            e.engine.tick(&handler).await.unwrap();
        }
        assert_eq!(recording.calls().len(), 4, "it ran every time it was due");
        assert_eq!(recording.held(), 0);
        let job = e.store.job("table_tick").unwrap().unwrap();
        assert_eq!(job.hold_reason, None);
        assert_eq!(job.failure_streak, 4, "the failures are still counted");
    }

    #[tokio::test]
    async fn a_three_day_sleep_makes_one_run_with_two_missed() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        e.clock.set(local(2026, 11, 23, 10, 0));
        let tick = e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(tick.ran.len(), 1);
        let calls = e.recording.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].trigger, Trigger::CatchUp);
        assert_eq!(calls[0].missed, 2);
        assert_eq!(calls[0].scheduled_at(), Some(local(2026, 11, 23, 9, 0)));
        let runs = e.store.runs("j", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Succeeded);
        // Nothing more until the next occurrence.
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(e.recording.calls().len(), 1);
        let job = e.store.job("j").unwrap().unwrap();
        assert_eq!(
            job.next_due_at.as_deref().and_then(parse_instant),
            Some(local(2026, 11, 24, 9, 0))
        );
    }

    #[tokio::test]
    async fn catch_up_skip_records_one_skipped_run() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Skip);
        e.clock.set(local(2026, 11, 23, 10, 0));
        let tick = e.engine.tick(&e.handler).await.unwrap();
        assert!(tick.ran.is_empty());
        assert_eq!(tick.skipped.len(), 1);
        assert!(e.recording.calls().is_empty());
        let runs = e.store.runs("j", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Skipped);
        assert_eq!(runs[0].missed, 2);
        assert!(runs[0]
            .summary
            .as_deref()
            .is_some_and(|s| s.starts_with("Skipped: Shodh wasn't running")));
    }

    #[tokio::test]
    async fn an_occurrence_while_a_decision_waits_folds_into_it() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        e.recording.script(vec![decision()]);
        e.clock
            .set(local(2026, 11, 21, 9, 0) + TimeDelta::seconds(30));
        e.engine.tick(&e.handler).await.unwrap();
        e.clock.set(local(2026, 11, 22, 9, 1));
        e.engine.tick(&e.handler).await.unwrap();
        e.clock.set(local(2026, 11, 23, 9, 1));
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(e.recording.calls().len(), 1, "the handler ran once");
        assert_eq!(e.recording.folded(), 2);
        let runs = e.store.runs("j", 10).unwrap();
        assert_eq!(runs.len(), 1, "one question, never a pile");
        assert_eq!(runs[0].state, RunState::Waiting);
        assert_eq!(runs[0].missed, 2);
        assert_eq!(runs[0].scheduled_at(), Some(local(2026, 11, 23, 9, 0)));
    }

    #[tokio::test]
    async fn skip_overlap_records_the_previous_run_was_still_going() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        e.recording.script(vec![Handled::Waiting {
            reason: WaitReason::Approval,
        }]);
        e.clock.set(local(2026, 11, 21, 9, 1));
        e.engine.tick(&e.handler).await.unwrap();
        e.clock.set(local(2026, 11, 22, 9, 1));
        let tick = e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(tick.skipped.len(), 1);
        let runs = e.store.runs("j", 10).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].state, RunState::Skipped);
        assert_eq!(
            runs[0].summary.as_deref(),
            Some("Skipped: the previous run was still going.")
        );
        assert_eq!(runs[1].state, RunState::Waiting);
    }

    #[tokio::test]
    async fn queue_one_keeps_at_most_one_behind() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::QueueOne, CatchUp::Once);
        e.recording.script(vec![Handled::Waiting {
            reason: WaitReason::Approval,
        }]);
        e.clock.set(local(2026, 11, 21, 9, 1));
        e.engine.tick(&e.handler).await.unwrap();
        e.clock.set(local(2026, 11, 22, 9, 1));
        e.engine.tick(&e.handler).await.unwrap();
        e.clock.set(local(2026, 11, 23, 9, 1));
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(
            e.store.runs("j", 10).unwrap().len(),
            1,
            "nothing recorded behind it"
        );
        let waiting = e.store.active_run("j").unwrap().unwrap();
        e.engine
            .settle(
                &e.handler,
                "j",
                &waiting.id,
                Handled::Succeeded {
                    summary: "Approved".into(),
                    scheduled_for: None,
                },
            )
            .await
            .unwrap()
            .unwrap();
        e.engine.tick(&e.handler).await.unwrap();
        let calls = e.recording.calls();
        assert_eq!(calls.len(), 2, "exactly one more run");
        assert_eq!(calls[1].scheduled_at(), Some(local(2026, 11, 23, 9, 0)));
        assert_eq!(calls[1].missed, 1);
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(e.recording.calls().len(), 2);
    }

    #[tokio::test]
    async fn transient_failures_climb_the_ladder_then_fail() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        e.recording
            .script(vec![transient(), transient(), transient()]);
        let start = local(2026, 11, 21, 9, 0);
        e.clock.set(start);
        e.engine.tick(&e.handler).await.unwrap();
        let run = e.store.active_run("j").unwrap().unwrap();
        assert_eq!(run.wait_reason, Some(WaitReason::Backoff));
        assert_eq!(
            run.next_attempt_at.as_deref().and_then(parse_instant),
            Some(start + TimeDelta::minutes(BACKOFF_LADDER_MINUTES[0]))
        );
        e.clock.set(start + TimeDelta::seconds(59));
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(e.recording.calls().len(), 1, "not before its time");
        let second = start + TimeDelta::minutes(1);
        e.clock.set(second);
        e.engine.tick(&e.handler).await.unwrap();
        let run = e.store.active_run("j").unwrap().unwrap();
        assert_eq!(run.attempt, 2);
        assert_eq!(
            run.next_attempt_at.as_deref().and_then(parse_instant),
            Some(second + TimeDelta::minutes(BACKOFF_LADDER_MINUTES[1]))
        );
        e.clock.set(second + TimeDelta::minutes(2));
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(e.recording.calls().len(), 3);
        assert!(e.store.active_run("j").unwrap().is_none());
        let runs = e.store.runs("j", 10).unwrap();
        assert_eq!(runs[0].state, RunState::Failed);
        assert_eq!(runs[0].error_code.as_deref(), Some("calendar_unreadable"));
        assert_eq!(e.store.job("j").unwrap().unwrap().failure_streak, 1);
    }

    #[tokio::test]
    async fn retry_at_waits_until_the_time_given_then_fails_past_its_attempts() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        let start = local(2026, 11, 21, 9, 0);
        let retry = |at: DateTime<Utc>| Handled::RetryAt {
            at,
            reason: WaitReason::Backoff,
            code: "api_unavailable".into(),
            detail: "Semantic Scholar answered 503".into(),
        };
        let first = start + TimeDelta::hours(1);
        let second = first + TimeDelta::hours(1);
        e.recording.script(vec![
            retry(first),
            retry(second),
            retry(second + TimeDelta::hours(1)),
        ]);
        e.clock.set(start);
        e.engine.tick(&e.handler).await.unwrap();
        let run = e.store.active_run("j").unwrap().unwrap();
        assert_eq!(run.wait_reason, Some(WaitReason::Backoff));
        assert_eq!(run.attempt, 1);
        assert_eq!(
            run.next_attempt_at.as_deref().and_then(parse_instant),
            Some(first)
        );
        // Not a minute before its time, though the ladder's first rung has passed.
        e.clock.set(first - TimeDelta::minutes(1));
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(e.recording.calls().len(), 1);
        e.clock.set(first);
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(e.recording.calls().len(), 2);
        let run = e.store.active_run("j").unwrap().unwrap();
        assert_eq!(run.attempt, 2);
        assert_eq!(
            run.next_attempt_at.as_deref().and_then(parse_instant),
            Some(second)
        );
        // The third attempt is the job's last (max_attempts 3): the run fails.
        e.clock.set(second);
        e.engine.tick(&e.handler).await.unwrap();
        assert!(e.store.active_run("j").unwrap().is_none());
        let runs = e.store.runs("j", 10).unwrap();
        assert_eq!(runs[0].state, RunState::Failed);
        assert_eq!(runs[0].error_code.as_deref(), Some("api_unavailable"));
    }

    #[tokio::test]
    async fn a_defer_never_counts_an_attempt_or_holds() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        let start = local(2026, 11, 21, 9, 0);
        // Five times more than the job's attempts: a busy chat day, then it runs.
        let defers: Vec<Handled> = (1..=5)
            .map(|n| Handled::Defer {
                at: start + TimeDelta::seconds(30 * n),
                reason: WaitReason::Yield,
            })
            .collect();
        e.recording.script(defers);
        e.clock.set(start);
        for n in 0..=5 {
            e.clock.set(start + TimeDelta::seconds(30 * n));
            e.engine.tick(&e.handler).await.unwrap();
            if n < 5 {
                let run = e.store.active_run("j").unwrap().unwrap();
                assert_eq!(run.state, RunState::Waiting);
                assert_eq!(run.wait_reason, Some(WaitReason::Yield));
                assert_eq!(run.attempt, 0, "a deferral is not an attempt");
                assert_eq!(
                    run.next_attempt_at.as_deref().and_then(parse_instant),
                    Some(start + TimeDelta::seconds(30 * (n + 1)))
                );
            }
        }
        assert_eq!(e.recording.calls().len(), 6);
        let runs = e.store.runs("j", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Succeeded);
        let job = e.store.job("j").unwrap().unwrap();
        assert_eq!(job.failure_streak, 0);
        assert_eq!(job.hold_reason, None);
    }

    #[tokio::test]
    async fn resume_takes_up_an_approval_wait_through_the_handler() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", None, Overlap::Skip, CatchUp::Once);
        e.recording.script(vec![Handled::Waiting {
            reason: WaitReason::Approval,
        }]);
        let run = e
            .store
            .insert_run(
                &NewRun {
                    id: "r1".into(),
                    job_id: "j".into(),
                    trigger: Trigger::Manual,
                    scheduled_for: None,
                    missed: 0,
                    state: RunState::Queued,
                    summary: None,
                },
                e.clock.now(),
            )
            .unwrap()
            .unwrap();
        e.engine.run_now(&e.handler, "j", &run.id).await.unwrap();
        let waiting = e.store.run("r1").unwrap().unwrap();
        assert_eq!(waiting.wait_reason, Some(WaitReason::Approval));
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(
            e.recording.calls().len(),
            1,
            "a tick leaves an approval waiting"
        );
        let done = e
            .engine
            .resume(&e.handler, "j", "r1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(done.state, RunState::Succeeded);
        assert_eq!(e.recording.calls().len(), 2);
        assert!(
            e.engine
                .resume(&e.handler, "j", "r1")
                .await
                .unwrap()
                .is_none(),
            "a run that no longer waits is not taken up"
        );
    }

    #[tokio::test]
    async fn a_deferred_wait_folds_into_the_next_occurrence() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        let start = local(2026, 11, 21, 9, 0);
        e.recording.script(vec![Handled::Defer {
            at: start + TimeDelta::days(3),
            reason: WaitReason::Quota,
        }]);
        e.clock.set(start);
        e.engine.tick(&e.handler).await.unwrap();
        e.clock.set(local(2026, 11, 22, 9, 1));
        let tick = e.engine.tick(&e.handler).await.unwrap();
        assert!(
            tick.skipped.is_empty(),
            "no row for every occurrence it waits through"
        );
        let runs = e.store.runs("j", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Waiting);
        assert_eq!(runs[0].missed, 1);
        assert_eq!(runs[0].scheduled_at(), Some(local(2026, 11, 22, 9, 0)));
    }

    #[tokio::test]
    async fn an_offline_wait_reads_as_offline() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        let start = local(2026, 11, 21, 9, 0);
        e.recording.script(vec![Handled::RetryAt {
            at: start + TimeDelta::minutes(15),
            reason: WaitReason::Offline,
            code: "offline".into(),
            detail: "no connection".into(),
        }]);
        e.clock.set(start);
        e.engine.tick(&e.handler).await.unwrap();
        let run = e.store.active_run("j").unwrap().unwrap();
        assert_eq!(run.wait_reason, Some(WaitReason::Offline));
        assert_eq!(run.error_code.as_deref(), Some("offline"));
    }

    #[tokio::test]
    async fn three_failed_runs_hold_the_job_and_tell_once() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        let permanent = || Handled::Permanent {
            code: "broken".into(),
            detail: "no".into(),
        };
        e.recording
            .script(vec![permanent(), permanent(), permanent()]);
        for day in 21..=23 {
            e.clock.set(local(2026, 11, day, 9, 1));
            e.engine.tick(&e.handler).await.unwrap();
        }
        let job = e.store.job("j").unwrap().unwrap();
        assert_eq!(job.hold_reason, Some(HoldReason::Failing));
        assert_eq!(e.recording.held(), 1);
        e.clock.set(local(2026, 11, 24, 9, 1));
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(e.recording.calls().len(), 3, "a held job does not run");
        assert_eq!(e.recording.held(), 1);
    }

    #[tokio::test]
    async fn a_run_abandoned_mid_way_is_resumed_once_after_the_lease() {
        let start = local(2026, 11, 21, 9, 0);
        let e = env(start);
        add_job(&e, "j", None, Overlap::Skip, CatchUp::Once);
        let run = e
            .store
            .insert_run(
                &NewRun {
                    id: "r1".into(),
                    job_id: "j".into(),
                    trigger: Trigger::Manual,
                    scheduled_for: None,
                    missed: 0,
                    state: RunState::Queued,
                    summary: None,
                },
                start,
            )
            .unwrap()
            .unwrap();
        // Another process claimed it and died before writing its end.
        e.store
            .claim(&run.id, "dead", false, false, start, &lease_cutoff(start))
            .unwrap()
            .unwrap();
        e.clock.set(start + TimeDelta::minutes(9));
        e.engine.tick(&e.handler).await.unwrap();
        assert!(
            e.recording.calls().is_empty(),
            "a live lease is never taken"
        );
        e.clock.set(start + TimeDelta::minutes(10));
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(e.recording.calls().len(), 1);
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(e.recording.calls().len(), 1, "resumed exactly once");
        assert_eq!(
            e.store.run("r1").unwrap().unwrap().state,
            RunState::Succeeded
        );
    }

    #[tokio::test]
    async fn an_event_answers_a_waiting_decision_instead_of_starting_a_run() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        e.recording.script(vec![decision()]);
        e.clock.set(local(2026, 11, 21, 9, 1));
        e.engine.tick(&e.handler).await.unwrap();
        let waiting = e.store.active_run("j").unwrap().unwrap();
        let answered = e.engine.event(&e.handler, "j").await.unwrap().unwrap();
        assert_eq!(answered.id, waiting.id);
        assert_eq!(answered.state, RunState::Succeeded);
        assert_eq!(e.store.runs("j", 10).unwrap().len(), 1);
        // With nothing waiting, an event starts its own run.
        let own = e.engine.event(&e.handler, "j").await.unwrap().unwrap();
        assert_eq!(own.trigger, Trigger::Event);
        assert_eq!(e.store.runs("j", 10).unwrap().len(), 2);
        // A run with nothing to do leaves no record.
        e.recording.script(vec![Handled::Nothing]);
        assert!(e.engine.event(&e.handler, "j").await.unwrap().is_none());
        assert_eq!(e.store.runs("j", 10).unwrap().len(), 2);
    }

    /// A handler that holds its first run until released.
    struct Gated {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        calls: Mutex<Vec<Run>>,
        script: Mutex<std::collections::VecDeque<Handled>>,
    }

    #[async_trait::async_trait]
    impl JobHandler for Gated {
        fn kind(&self) -> JobKind {
            JobKind::TaskRepeat
        }
        async fn run(&self, _job: &Job, run: &Run) -> Handled {
            let first = {
                let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
                calls.push(run.clone());
                calls.len() == 1
            };
            if first {
                self.entered.notify_one();
                self.release.notified().await;
            }
            let next = self
                .script
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front();
            next.unwrap_or(Handled::Succeeded {
                summary: "Done".to_string(),
                scheduled_for: None,
            })
        }
        async fn folded(&self, _job: &Job, _run: &Run) {}
        async fn held(&self, _job: &Job, _run: &Run) {}
    }

    #[tokio::test]
    async fn an_event_during_a_run_that_then_asks_is_not_lost() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        e.clock.set(local(2026, 11, 21, 9, 1));
        let gated = Arc::new(Gated {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            calls: Mutex::new(Vec::new()),
            script: Mutex::new([decision()].into_iter().collect()),
        });
        let handler: Arc<dyn JobHandler> = gated.clone();
        let engine = Arc::new(Engine::with_clock(
            e.store.clone(),
            e.clock.clone(),
            "engine-g",
        ));
        let ticking = {
            let (engine, handler) = (engine.clone(), handler.clone());
            tokio::spawn(async move { engine.tick(&handler).await })
        };
        // The scheduled run is reading the task list when the latest copy is completed.
        gated.entered.notified().await;
        assert!(engine.event(&handler, "j").await.unwrap().is_none());
        gated.release.notify_one();
        ticking.await.unwrap().unwrap();
        // The run asked about a task that is done by now: the completion takes the
        // question up at once instead of leaving it stale.
        let calls = gated.calls.lock().unwrap_or_else(|e| e.into_inner()).len();
        assert_eq!(calls, 2);
        assert!(e.store.active_run("j").unwrap().is_none());
        let runs = e.store.runs("j", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Succeeded);
    }

    #[tokio::test]
    async fn an_event_of_a_held_job_waits_for_resume() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "j", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        e.store
            .update_job(
                "j",
                &JobPatch {
                    hold_reason: Some(Some(HoldReason::Failing)),
                    ..JobPatch::default()
                },
                e.clock.now(),
            )
            .unwrap();
        assert!(e.engine.event(&e.handler, "j").await.unwrap().is_none());
        assert!(e.recording.calls().is_empty());
        assert!(e.store.runs("j", 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn due_runs_start_one_at_a_time_in_order() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "late", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        add_job(&e, "early", Some(daily_at(8)), Overlap::Skip, CatchUp::Once);
        e.clock.set(local(2026, 11, 21, 9, 1));
        e.engine.tick(&e.handler).await.unwrap();
        let order: Vec<String> = e.recording.calls().into_iter().map(|r| r.job_id).collect();
        assert_eq!(order, vec!["early".to_string(), "late".to_string()]);
    }

    #[tokio::test]
    async fn the_next_wake_is_the_earliest_due_time() {
        let e = env(local(2026, 11, 20, 10, 0));
        add_job(&e, "a", Some(daily_at(9)), Overlap::Skip, CatchUp::Once);
        add_job(&e, "b", Some(daily_at(8)), Overlap::Skip, CatchUp::Once);
        e.engine.tick(&e.handler).await.unwrap();
        assert_eq!(
            e.engine.next_wake(JobKind::TaskRepeat).unwrap(),
            Some(local(2026, 11, 21, 8, 0))
        );
    }
}
