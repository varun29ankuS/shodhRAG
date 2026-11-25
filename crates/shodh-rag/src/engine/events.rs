//! What starts a job when something happens, and the run an event queues.
//!
//! - **The trigger** ([`EventTrigger`]) is kept in the job's `trigger_json`: new files in
//!   a watched folder, or papers a paper alert found.
//! - **One run for a burst.** [`Engine::event_with`] queues one run that waits
//!   [`COALESCE_SECS`] before it starts. An event that comes while that run has not begun
//!   its work adds what it brought to the same run, so a sync that reports its files a few
//!   at a time makes one run, with every file.
//! - **What the event brought** (its subjects: the files, the papers) is the run's step 0,
//!   of kind `input`, at most [`MAX_EVENT_SUBJECTS`] of them with a count of the rest.
//! - **A job already at work** (a run of it has begun) is not started again: the event is
//!   recorded as a skipped run, as a schedule's overlap is.

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::lease::timestamp;
use super::runner::Engine;
use super::store::{
    active_in, job_from, job_in, run_in, EngineError, EngineResult, EngineStore, Job, NewRun, Run,
    RunState, Trigger, JOB_COLS,
};

/// How long an event's run waits for more of the same before it starts.
pub const COALESCE_SECS: i64 = 60;
/// The most subjects a run keeps; the rest are counted.
pub const MAX_EVENT_SUBJECTS: usize = 50;

/// What starts a job when it happens.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventTrigger {
    /// New files were indexed in the Library folder `source_id`.
    #[serde(rename_all = "camelCase")]
    FilesAdded { source_id: String },
    /// The paper alert `alert_id` found papers.
    #[serde(rename_all = "camelCase")]
    PaperFound { alert_id: String },
}

/// What an event brought to a run: its step 0.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventInput {
    /// The files or papers, oldest first, without repeats.
    #[serde(default)]
    pub subjects: Vec<Value>,
    /// How many more there were beyond [`MAX_EVENT_SUBJECTS`].
    #[serde(default)]
    pub more: u64,
}

impl EventInput {
    /// Adds `subjects`, skipping the ones it has and counting those past the limit.
    fn add(&mut self, subjects: &[Value]) {
        for subject in subjects {
            if self.subjects.contains(subject) {
                continue;
            }
            if self.subjects.len() < MAX_EVENT_SUBJECTS {
                self.subjects.push(subject.clone());
            } else {
                self.more = self.more.saturating_add(1);
            }
        }
    }
}

/// What an event did to a job.
#[derive(Debug, Clone, PartialEq)]
pub enum EventQueued {
    /// A run was queued; it starts after [`COALESCE_SECS`].
    Started(Run),
    /// The run already queued took the event's subjects in.
    Merged(Run),
    /// A run of the job had begun: the event was recorded as skipped.
    Busy(Run),
    /// The job is off, on hold or gone: nothing was recorded.
    Off,
}

fn trigger_of(text: &str) -> Option<EventTrigger> {
    serde_json::from_str(text).ok()
}

impl EngineStore {
    /// Sets what starts the job when it happens (`None`: nothing does).
    pub fn set_trigger(&self, job_id: &str, trigger: Option<&EventTrigger>) -> EngineResult<()> {
        let text = trigger
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE engine_job SET trigger_json = ?2 WHERE id = ?1",
                params![job_id, text],
            )
        })?;
        Ok(())
    }

    /// What starts the job when it happens.
    pub fn trigger(&self, job_id: &str) -> EngineResult<Option<EventTrigger>> {
        let conn = self.conn.read()?;
        let text: Option<Option<String>> = conn
            .query_row(
                "SELECT trigger_json FROM engine_job WHERE id = ?1",
                params![job_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(text.flatten().as_deref().and_then(trigger_of))
    }

    /// Every job `trigger` starts, oldest first (on, off or on hold).
    pub fn jobs_triggered_by(&self, trigger: &EventTrigger) -> EngineResult<Vec<Job>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {JOB_COLS}, trigger_json FROM engine_job
              WHERE trigger_json IS NOT NULL ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([], |r| {
            let text: String = r.get(17)?;
            Ok((job_from(r)?, text))
        })?;
        let mut jobs = Vec::new();
        for row in rows {
            let (job, text) = row?;
            if trigger_of(&text).as_ref() == Some(trigger) {
                jobs.push(job);
            }
        }
        Ok(jobs)
    }

    /// Queues the run of an event for `job_id`, to start at `until`, with `subjects` as
    /// its step 0; or, when a run of the job is active, adds them to it if it is an
    /// event's run whose agent has not begun, and otherwise leaves it alone
    /// ([`EventQueued::Busy`]). One transaction, so two events never make two runs.
    pub fn queue_event_run(
        &self,
        job_id: &str,
        run_id: &str,
        subjects: &[Value],
        until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> EngineResult<EventQueued> {
        let at = timestamp(now);
        let until = timestamp(until);
        self.conn.write(|tx| -> EngineResult<EventQueued> {
            if let Some(active) = active_in(tx, job_id)? {
                if active.trigger != Trigger::Event || active.state != RunState::Waiting {
                    return Ok(EventQueued::Busy(active));
                }
                let begun: i64 = tx.query_row(
                    "SELECT count(*) FROM engine_step WHERE run_id = ?1 AND kind <> 'input'",
                    params![active.id],
                    |r| r.get(0),
                )?;
                let stored: Option<String> = tx
                    .query_row(
                        "SELECT input_json FROM engine_step
                          WHERE run_id = ?1 AND idx = 0 AND kind = 'input'",
                        params![active.id],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(stored) = stored.filter(|_| begun == 0) else {
                    return Ok(EventQueued::Busy(active));
                };
                let mut input: EventInput = serde_json::from_str(&stored).unwrap_or_default();
                input.add(subjects);
                tx.execute(
                    "UPDATE engine_step SET input_json = ?2 WHERE run_id = ?1 AND idx = 0",
                    params![active.id, json!(input).to_string()],
                )?;
                return Ok(EventQueued::Merged(active));
            }
            let mut input = EventInput::default();
            input.add(subjects);
            tx.execute(
                "INSERT INTO engine_run (id, job_id, trigger_kind, missed, state, wait_reason,
                     next_attempt_at, created_at)
                 VALUES (?1, ?2, 'event', 0, 'waiting', 'backoff', ?3, ?4)",
                params![run_id, job_id, until, at],
            )?;
            tx.execute(
                "INSERT INTO engine_step (run_id, idx, kind, state, input_json, output_json,
                     calls, started_at, ended_at)
                 VALUES (?1, 0, 'input', 'done', ?2, '{}', 0, ?3, ?3)",
                params![run_id, json!(input).to_string(), at],
            )?;
            let run =
                run_in(tx, run_id)?.ok_or_else(|| EngineError::NotFound(run_id.to_string()))?;
            Ok(EventQueued::Started(run))
        })
    }

    /// Whether the job can take an event: it is there, on and not on hold.
    fn takes_events(&self, job_id: &str) -> EngineResult<bool> {
        let conn = self.conn.read()?;
        Ok(job_in(&conn, job_id)?.is_some_and(|job| job.enabled && job.hold_reason.is_none()))
    }
}

impl Engine {
    /// Something `job_id` is started by happened, bringing `subjects` (the files, the
    /// papers): one run is queued to start [`COALESCE_SECS`] from now, or the run already
    /// queued takes them in. While a run of the job is at work, the event is recorded as a
    /// skipped run. Nothing runs here: the caller wakes the loop that ticks the job's kind.
    pub fn event_with(&self, job_id: &str, subjects: &[Value]) -> EngineResult<EventQueued> {
        let store = self.store();
        if !store.takes_events(job_id)? {
            return Ok(EventQueued::Off);
        }
        let now = self.now();
        let queued = store.queue_event_run(
            job_id,
            &uuid::Uuid::new_v4().to_string(),
            subjects,
            now + TimeDelta::seconds(COALESCE_SECS),
            now,
        )?;
        if let EventQueued::Busy(_) = &queued {
            let skipped = NewRun {
                id: uuid::Uuid::new_v4().to_string(),
                job_id: job_id.to_string(),
                trigger: Trigger::Event,
                scheduled_for: None,
                missed: 0,
                state: RunState::Skipped,
                summary: Some("Skipped: the previous run was still going.".to_string()),
            };
            // A finished run is always inserted (only an active one can be refused).
            let _ = store.insert_run(&skipped, now)?;
        }
        Ok(queued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::runner::{Handled, JobHandler};
    use crate::engine::steps::{StepKind, StepState, StepWrite};
    use crate::engine::store::{
        CatchUp, CreatedBy, HoldReason, JobKind, JobPatch, NewJob, Overlap, WaitReason,
    };
    use crate::engine::testkit::{local, ManualClock, Recording};
    use std::sync::Arc;

    struct Env {
        _dir: tempfile::TempDir,
        clock: Arc<ManualClock>,
        engine: Engine,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        let clock = Arc::new(ManualClock::new(local(2026, 11, 25, 9, 0)));
        let engine = Engine::with_clock(store, clock.clone(), "events-test");
        Env {
            _dir: dir,
            clock,
            engine,
        }
    }

    fn job(e: &Env, id: &str, trigger: Option<&EventTrigger>) {
        e.engine
            .store()
            .create_job(
                &NewJob {
                    id: id.into(),
                    kind: JobKind::Playbook,
                    subject: id.into(),
                    title: "Summarise new papers".into(),
                    schedule: None,
                    input: json!({}),
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 3,
                    created_by: CreatedBy::User,
                    provenance: json!({}),
                },
                e.engine.now(),
            )
            .unwrap();
        e.engine.store().set_trigger(id, trigger).unwrap();
    }

    fn files(source: &str) -> EventTrigger {
        EventTrigger::FilesAdded {
            source_id: source.into(),
        }
    }

    fn input_of(e: &Env, run_id: &str) -> EventInput {
        let steps = e.engine.store().steps(run_id).unwrap();
        assert_eq!(steps[0].kind, StepKind::Input);
        assert_eq!(steps[0].idx, 0);
        serde_json::from_value(steps[0].input.clone()).unwrap()
    }

    #[test]
    fn a_trigger_is_stored_as_tagged_json_and_finds_its_jobs() {
        let e = env();
        let papers = EventTrigger::PaperFound {
            alert_id: "a1".into(),
        };
        assert_eq!(
            serde_json::to_value(&papers).unwrap(),
            json!({ "kind": "paper_found", "alertId": "a1" })
        );
        assert_eq!(
            serde_json::to_value(files("s1")).unwrap(),
            json!({ "kind": "files_added", "sourceId": "s1" })
        );
        job(&e, "j1", Some(&files("s1")));
        job(&e, "j2", Some(&papers));
        job(&e, "j3", Some(&files("s1")));
        job(&e, "j4", None);
        let store = e.engine.store();
        assert_eq!(store.trigger("j1").unwrap(), Some(files("s1")));
        assert_eq!(store.trigger("j4").unwrap(), None);
        assert_eq!(store.trigger("gone").unwrap(), None);
        let ids = |trigger: &EventTrigger| -> Vec<String> {
            store
                .jobs_triggered_by(trigger)
                .unwrap()
                .into_iter()
                .map(|j| j.id)
                .collect()
        };
        assert_eq!(ids(&files("s1")), vec!["j1", "j3"]);
        assert_eq!(ids(&papers), vec!["j2"]);
        assert!(ids(&files("s2")).is_empty());
        store.set_trigger("j1", None).unwrap();
        assert_eq!(ids(&files("s1")), vec!["j3"]);
    }

    #[tokio::test]
    async fn events_within_a_minute_make_one_run_with_every_subject() {
        let e = env();
        job(&e, "j1", Some(&files("s1")));
        let first = e
            .engine
            .event_with(
                "j1",
                &[json!({ "path": "a.pdf" }), json!({ "path": "b.pdf" })],
            )
            .unwrap();
        let EventQueued::Started(run) = first else {
            panic!("the first event queues a run: {first:?}");
        };
        assert_eq!(run.trigger, Trigger::Event);
        assert_eq!(run.state, RunState::Waiting);
        assert_eq!(run.wait_reason, Some(WaitReason::Backoff));
        assert_eq!(
            run.next_attempt_at,
            Some(timestamp(
                e.engine.now() + TimeDelta::seconds(COALESCE_SECS)
            ))
        );
        assert_eq!(run.attempt, 0, "waiting for more is not an attempt");
        e.clock
            .set(local(2026, 11, 25, 9, 0) + TimeDelta::seconds(30));
        let second = e
            .engine
            .event_with(
                "j1",
                &[json!({ "path": "b.pdf" }), json!({ "path": "c.pdf" })],
            )
            .unwrap();
        assert!(matches!(&second, EventQueued::Merged(r) if r.id == run.id));
        assert_eq!(e.engine.store().runs("j1", 10).unwrap().len(), 1, "one run");
        let input = input_of(&e, &run.id);
        assert_eq!(
            input.subjects,
            vec![
                json!({ "path": "a.pdf" }),
                json!({ "path": "b.pdf" }),
                json!({ "path": "c.pdf" })
            ],
            "every subject once, oldest first"
        );
        assert_eq!(input.more, 0);
        // Not due before its minute is over; then the job's handler runs it once.
        let recording = Arc::new(Recording::new(JobKind::Playbook));
        let handler: Arc<dyn JobHandler> = recording.clone();
        assert!(e.engine.tick(&handler).await.unwrap().ran.is_empty());
        e.clock.set(local(2026, 11, 25, 9, 1));
        let tick = e.engine.tick(&handler).await.unwrap();
        assert_eq!(tick.ran, vec![run.id.clone()]);
        let done = e.engine.store().run(&run.id).unwrap().unwrap();
        assert_eq!(done.state, RunState::Succeeded);
        assert_eq!(done.attempt, 0);
    }

    #[test]
    fn a_run_keeps_fifty_subjects_and_counts_the_rest() {
        let e = env();
        job(&e, "j1", Some(&files("s1")));
        let many: Vec<Value> = (0..MAX_EVENT_SUBJECTS + 7)
            .map(|i| json!({ "path": format!("{i}.pdf") }))
            .collect();
        let EventQueued::Started(run) = e.engine.event_with("j1", &many).unwrap() else {
            panic!("queued");
        };
        e.engine
            .event_with("j1", &[json!({ "path": "late.pdf" })])
            .unwrap();
        let input = input_of(&e, &run.id);
        assert_eq!(input.subjects.len(), MAX_EVENT_SUBJECTS);
        assert_eq!(input.more, 8);
    }

    #[test]
    fn an_event_for_a_job_at_work_is_recorded_as_skipped() {
        let e = env();
        job(&e, "j1", Some(&files("s1")));
        let EventQueued::Started(run) = e
            .engine
            .event_with("j1", &[json!({ "path": "a.pdf" })])
            .unwrap()
        else {
            panic!("queued");
        };
        // Its agent has begun: the run no longer takes more in.
        let empty = json!({});
        e.engine
            .store()
            .put_step(
                &StepWrite {
                    run_id: &run.id,
                    idx: 1,
                    kind: StepKind::Agent,
                    state: StepState::Running,
                    input: &empty,
                    output: &empty,
                    calls: 0,
                },
                e.engine.now(),
            )
            .unwrap();
        let busy = e
            .engine
            .event_with("j1", &[json!({ "path": "b.pdf" })])
            .unwrap();
        assert!(matches!(&busy, EventQueued::Busy(r) if r.id == run.id));
        assert_eq!(
            input_of(&e, &run.id).subjects,
            vec![json!({ "path": "a.pdf" })]
        );
        let runs = e.engine.store().runs("j1", 10).unwrap();
        assert_eq!(runs.len(), 2);
        let skipped = runs.iter().find(|r| r.id != run.id).unwrap();
        assert_eq!(skipped.state, RunState::Skipped);
        assert_eq!(skipped.trigger, Trigger::Event);
        assert_eq!(
            skipped.summary.as_deref(),
            Some("Skipped: the previous run was still going.")
        );
    }

    #[test]
    fn a_job_that_is_off_on_hold_or_gone_takes_no_event() {
        let e = env();
        job(&e, "off", Some(&files("s1")));
        job(&e, "held", Some(&files("s1")));
        let now = e.engine.now();
        e.engine
            .store()
            .update_job(
                "off",
                &JobPatch {
                    enabled: Some(false),
                    ..JobPatch::default()
                },
                now,
            )
            .unwrap();
        e.engine
            .store()
            .update_job(
                "held",
                &JobPatch {
                    hold_reason: Some(Some(HoldReason::Failing)),
                    ..JobPatch::default()
                },
                now,
            )
            .unwrap();
        for id in ["off", "held", "gone"] {
            assert_eq!(
                e.engine
                    .event_with(id, &[json!({ "path": "a.pdf" })])
                    .unwrap(),
                EventQueued::Off,
                "{id}"
            );
            assert!(e.engine.store().runs(id, 10).unwrap().is_empty(), "{id}");
        }
    }

    #[tokio::test]
    async fn a_scripted_wait_does_not_lose_what_the_event_brought() {
        let e = env();
        job(&e, "j1", Some(&files("s1")));
        let EventQueued::Started(run) = e
            .engine
            .event_with("j1", &[json!({ "path": "a.pdf" })])
            .unwrap()
        else {
            panic!("queued");
        };
        // The run started and put itself off (the user is chatting): more files still
        // join it, since its agent has not begun.
        let recording = Arc::new(Recording::new(JobKind::Playbook));
        recording.script(vec![Handled::Defer {
            at: local(2026, 11, 25, 9, 5),
            reason: WaitReason::Yield,
        }]);
        let handler: Arc<dyn JobHandler> = recording.clone();
        e.clock.set(local(2026, 11, 25, 9, 1));
        e.engine.tick(&handler).await.unwrap();
        let waiting = e.engine.store().run(&run.id).unwrap().unwrap();
        assert_eq!(waiting.wait_reason, Some(WaitReason::Yield));
        let merged = e
            .engine
            .event_with("j1", &[json!({ "path": "b.pdf" })])
            .unwrap();
        assert!(matches!(merged, EventQueued::Merged(_)));
        assert_eq!(input_of(&e, &run.id).subjects.len(), 2);
    }
}
