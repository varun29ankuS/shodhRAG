//! The steps of a run and the changes a run waits for the user's OK on, in `shodh.db`
//! (`engine_step`, `engine_approval`).
//!
//! - **Steps** are written as a run goes, one row per step at its index, so the run page
//!   reads what each step was given, what it gave and the model calls it made.
//! - **An approval** holds every change one run's agent asked to make, never run: the
//!   user picks which to make. It is decided once ([`EngineStore::decide_approval`] only
//!   changes an open one), and it expires after [`APPROVAL_DAYS`].

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;
use serde_json::Value;

use super::lease::timestamp;
use super::store::{EngineError, EngineResult, EngineStore};

/// Days an approval waits before it expires and its run is cancelled.
pub const APPROVAL_DAYS: i64 = 7;

macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident {
        $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)?
    }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            pub fn parse(text: &str) -> Option<Self> {
                match text {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

text_enum!(
    /// What a step does.
    StepKind {
        /// What an event gave the run (the files or papers it is about).
        Input => "input",
        /// An agent session follows the automation's instruction.
        Agent => "agent",
        /// Tells the user what the run found.
        Notify => "notify",
    }
);

text_enum!(
    /// Where a step stands.
    StepState {
        Running => "running",
        Done => "done",
        Failed => "failed",
        Skipped => "skipped",
    }
);

text_enum!(
    /// What the user decided about a run's changes.
    ApprovalOutcome {
        /// Every change was made.
        Approved => "approved",
        /// None was made.
        Denied => "denied",
        /// Nobody decided within [`APPROVAL_DAYS`].
        Expired => "expired",
        /// Some were made.
        Partly => "partly",
    }
);

/// One step of a run.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub run_id: String,
    pub idx: u32,
    pub kind: StepKind,
    pub state: StepState,
    pub input: Value,
    pub output: Value,
    pub calls: u32,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

/// The changes one run waits for the user's OK on.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    pub job_id: String,
    pub run_id: String,
    /// "Wants to add 3 tasks".
    pub title: String,
    /// The changes, in the order the agent asked for them.
    pub writes: Value,
    pub created_at: String,
    pub expires_at: String,
    pub decision: Option<ApprovalOutcome>,
    pub decided_at: Option<String>,
    /// What each change made came to.
    pub result: Value,
}

impl Approval {
    /// Whether it is still open at `now` but past its time.
    pub fn is_overdue(&self, now: DateTime<Utc>) -> bool {
        self.decision.is_none() && self.expires_at.as_str() <= timestamp(now).as_str()
    }
}

fn as_count(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

fn bad(index: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, message.into())
}

fn json(index: usize, text: String) -> rusqlite::Result<Value> {
    serde_json::from_str(&text).map_err(|e| bad(index, e.to_string()))
}

const STEP_COLS: &str =
    "run_id, idx, kind, state, input_json, output_json, calls, started_at, ended_at";

fn step_from(r: &Row<'_>) -> rusqlite::Result<Step> {
    let kind: String = r.get(2)?;
    let state: String = r.get(3)?;
    Ok(Step {
        run_id: r.get(0)?,
        idx: as_count(r.get(1)?),
        kind: StepKind::parse(&kind).ok_or_else(|| bad(2, format!("unknown step {kind}")))?,
        state: StepState::parse(&state).ok_or_else(|| bad(3, format!("unknown state {state}")))?,
        input: json(4, r.get(4)?)?,
        output: json(5, r.get(5)?)?,
        calls: as_count(r.get(6)?),
        started_at: r.get(7)?,
        ended_at: r.get(8)?,
    })
}

const APPROVAL_COLS: &str = "id, job_id, run_id, title, writes_json, created_at, expires_at, \
     decision, decided_at, result_json";

fn approval_from(r: &Row<'_>) -> rusqlite::Result<Approval> {
    let decision: Option<String> = r.get(7)?;
    Ok(Approval {
        id: r.get(0)?,
        job_id: r.get(1)?,
        run_id: r.get(2)?,
        title: r.get(3)?,
        writes: json(4, r.get(4)?)?,
        created_at: r.get(5)?,
        expires_at: r.get(6)?,
        decision: decision
            .map(|d| ApprovalOutcome::parse(&d).ok_or_else(|| bad(7, format!("unknown {d}"))))
            .transpose()?,
        decided_at: r.get(8)?,
        result: json(9, r.get(9)?)?,
    })
}

/// A step as it is written.
#[derive(Debug, Clone)]
pub struct StepWrite<'a> {
    pub run_id: &'a str,
    pub idx: u32,
    pub kind: StepKind,
    pub state: StepState,
    pub input: &'a Value,
    pub output: &'a Value,
    pub calls: u32,
}

impl EngineStore {
    /// Writes a run's step at its index (a step run again replaces itself), keeping when
    /// it first started and setting when it ended once it is no longer running.
    pub fn put_step(&self, step: &StepWrite<'_>, now: DateTime<Utc>) -> EngineResult<Step> {
        let at = timestamp(now);
        let ended = (step.state != StepState::Running).then(|| at.clone());
        self.conn.write(|tx| -> EngineResult<Step> {
            tx.execute(
                "INSERT INTO engine_step (run_id, idx, kind, state, input_json, output_json,
                     calls, started_at, ended_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(run_id, idx) DO UPDATE SET kind = excluded.kind,
                     state = excluded.state, input_json = excluded.input_json,
                     output_json = excluded.output_json, calls = excluded.calls,
                     ended_at = excluded.ended_at",
                params![
                    step.run_id,
                    i64::from(step.idx),
                    step.kind.as_str(),
                    step.state.as_str(),
                    step.input.to_string(),
                    step.output.to_string(),
                    i64::from(step.calls),
                    at,
                    ended,
                ],
            )?;
            tx.query_row(
                &format!("SELECT {STEP_COLS} FROM engine_step WHERE run_id = ?1 AND idx = ?2"),
                params![step.run_id, i64::from(step.idx)],
                step_from,
            )
            .map_err(EngineError::from)
        })
    }

    /// A run's steps, in order.
    pub fn steps(&self, run_id: &str) -> EngineResult<Vec<Step>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {STEP_COLS} FROM engine_step WHERE run_id = ?1 ORDER BY idx"
        ))?;
        let rows = stmt.query_map(params![run_id], step_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Opens an approval for a run's changes, expiring [`APPROVAL_DAYS`] from `now`. A
    /// run has at most one: asking again returns the one it has.
    pub fn open_approval(
        &self,
        id: &str,
        job_id: &str,
        run_id: &str,
        title: &str,
        writes: &Value,
        now: DateTime<Utc>,
    ) -> EngineResult<Approval> {
        let at = timestamp(now);
        let expires = timestamp(now + TimeDelta::days(APPROVAL_DAYS));
        self.conn.write(|tx| -> EngineResult<Approval> {
            let existing = tx
                .query_row(
                    &format!("SELECT {APPROVAL_COLS} FROM engine_approval WHERE run_id = ?1"),
                    params![run_id],
                    approval_from,
                )
                .optional()?;
            if let Some(existing) = existing {
                return Ok(existing);
            }
            tx.execute(
                "INSERT INTO engine_approval (id, job_id, run_id, title, writes_json,
                     created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, job_id, run_id, title, writes.to_string(), at, expires],
            )?;
            tx.query_row(
                &format!("SELECT {APPROVAL_COLS} FROM engine_approval WHERE id = ?1"),
                params![id],
                approval_from,
            )
            .map_err(EngineError::from)
        })
    }

    pub fn approval(&self, id: &str) -> EngineResult<Option<Approval>> {
        let conn = self.conn.read()?;
        Ok(conn
            .query_row(
                &format!("SELECT {APPROVAL_COLS} FROM engine_approval WHERE id = ?1"),
                params![id],
                approval_from,
            )
            .optional()?)
    }

    /// The approval of a run, if it asked for one.
    pub fn approval_of_run(&self, run_id: &str) -> EngineResult<Option<Approval>> {
        let conn = self.conn.read()?;
        Ok(conn
            .query_row(
                &format!("SELECT {APPROVAL_COLS} FROM engine_approval WHERE run_id = ?1"),
                params![run_id],
                approval_from,
            )
            .optional()?)
    }

    /// Every approval nobody has decided yet, oldest first.
    pub fn open_approvals(&self) -> EngineResult<Vec<Approval>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {APPROVAL_COLS} FROM engine_approval WHERE decision IS NULL
              ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([], approval_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records what the changes of a decided approval came to.
    pub fn set_approval_result(&self, id: &str, result: &Value) -> EngineResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE engine_approval SET result_json = ?2 WHERE id = ?1",
                params![id, result.to_string()],
            )
        })?;
        Ok(())
    }

    /// Decides an open approval. `None` when it was already decided (or is not there):
    /// a second decision never changes the first.
    pub fn decide_approval(
        &self,
        id: &str,
        outcome: ApprovalOutcome,
        result: &Value,
        now: DateTime<Utc>,
    ) -> EngineResult<Option<Approval>> {
        let at = timestamp(now);
        self.conn.write(|tx| -> EngineResult<Option<Approval>> {
            let changed = tx.execute(
                "UPDATE engine_approval SET decision = ?2, decided_at = ?3, result_json = ?4
                  WHERE id = ?1 AND decision IS NULL",
                params![id, outcome.as_str(), at, result.to_string()],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            Ok(tx
                .query_row(
                    &format!("SELECT {APPROVAL_COLS} FROM engine_approval WHERE id = ?1"),
                    params![id],
                    approval_from,
                )
                .optional()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::store::{
        CatchUp, CreatedBy, JobKind, NewJob, NewRun, Overlap, RunState, Trigger,
    };
    use chrono::TimeZone;
    use serde_json::json;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, hour, 0, 0).unwrap()
    }

    fn store() -> (tempfile::TempDir, EngineStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = EngineStore::open(&dir.path().join("shodh.db"), None).unwrap();
        (dir, store)
    }

    fn job_with_run(store: &EngineStore) {
        store
            .create_job(
                &NewJob {
                    id: "j1".into(),
                    kind: JobKind::Playbook,
                    subject: "j1".into(),
                    title: "Weekly digest".into(),
                    schedule: None,
                    input: json!({}),
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 3,
                    created_by: CreatedBy::User,
                    provenance: json!({}),
                },
                at(24, 9),
            )
            .unwrap();
        store
            .insert_run(
                &NewRun {
                    id: "r1".into(),
                    job_id: "j1".into(),
                    trigger: Trigger::Manual,
                    scheduled_for: None,
                    missed: 0,
                    state: RunState::Queued,
                    summary: None,
                },
                at(24, 9),
            )
            .unwrap()
            .unwrap();
    }

    #[test]
    fn a_step_run_again_replaces_itself_and_keeps_its_start() {
        let (_dir, store) = store();
        let empty = json!({});
        let first = store
            .put_step(
                &StepWrite {
                    run_id: "r1",
                    idx: 0,
                    kind: StepKind::Agent,
                    state: StepState::Running,
                    input: &empty,
                    output: &empty,
                    calls: 0,
                },
                at(24, 9),
            )
            .unwrap();
        assert_eq!(first.ended_at, None);
        let output = json!({ "summary": "found 4 papers" });
        let done = store
            .put_step(
                &StepWrite {
                    run_id: "r1",
                    idx: 0,
                    kind: StepKind::Agent,
                    state: StepState::Done,
                    input: &empty,
                    output: &output,
                    calls: 3,
                },
                at(24, 10),
            )
            .unwrap();
        assert_eq!(done.started_at, first.started_at, "the start is kept");
        assert!(done.ended_at.is_some());
        assert_eq!(done.calls, 3);
        assert_eq!(store.steps("r1").unwrap(), vec![done]);
    }

    #[test]
    fn an_approval_is_decided_once_and_a_run_has_one() {
        let (_dir, store) = store();
        let writes = json!([{ "tool": "create_task", "label": "Add task" }]);
        let open = store
            .open_approval("a1", "j1", "r1", "Wants to add 1 task", &writes, at(24, 9))
            .unwrap();
        assert_eq!(
            open.expires_at,
            timestamp(at(24, 9) + TimeDelta::days(APPROVAL_DAYS))
        );
        let again = store
            .open_approval("a2", "j1", "r1", "other", &writes, at(24, 10))
            .unwrap();
        assert_eq!(again.id, "a1", "one approval per run");
        assert_eq!(store.open_approvals().unwrap().len(), 1);
        let decided = store
            .decide_approval(
                "a1",
                ApprovalOutcome::Approved,
                &json!({ "made": 1 }),
                at(25, 9),
            )
            .unwrap()
            .unwrap();
        assert_eq!(decided.decision, Some(ApprovalOutcome::Approved));
        assert!(store
            .decide_approval("a1", ApprovalOutcome::Denied, &json!({}), at(25, 10))
            .unwrap()
            .is_none());
        assert!(store.open_approvals().unwrap().is_empty());
        assert_eq!(
            store.approval_of_run("r1").unwrap().unwrap().decision,
            Some(ApprovalOutcome::Approved)
        );
    }

    #[test]
    fn an_open_approval_is_overdue_after_seven_days() {
        let (_dir, store) = store();
        let open = store
            .open_approval("a1", "j1", "r1", "t", &json!([]), at(1, 9))
            .unwrap();
        assert!(!open.is_overdue(at(8, 8)));
        assert!(open.is_overdue(at(8, 9)));
    }

    #[test]
    fn deleting_a_job_deletes_its_steps_and_approvals() {
        let (_dir, store) = store();
        job_with_run(&store);
        let empty = json!({});
        store
            .put_step(
                &StepWrite {
                    run_id: "r1",
                    idx: 0,
                    kind: StepKind::Agent,
                    state: StepState::Done,
                    input: &empty,
                    output: &empty,
                    calls: 1,
                },
                at(24, 9),
            )
            .unwrap();
        store
            .open_approval("a1", "j1", "r1", "t", &json!([]), at(24, 9))
            .unwrap();
        assert!(store.delete_job("j1").unwrap());
        assert!(store.steps("r1").unwrap().is_empty());
        assert!(store.approval("a1").unwrap().is_none());
    }

    #[test]
    fn a_runs_calls_and_conversation_are_read_back() {
        let (_dir, store) = store();
        job_with_run(&store);
        store.set_planned("r1", 4).unwrap();
        assert_eq!(store.add_used("r1", 1).unwrap(), 1);
        assert_eq!(store.add_used("r1", 2).unwrap(), 3);
        store.set_conversation("r1", "auto-r1").unwrap();
        let run = store.run("r1").unwrap().unwrap();
        assert_eq!(run.planned_calls, 4);
        assert_eq!(run.used_calls, 3);
        assert_eq!(run.conversation_id.as_deref(), Some("auto-r1"));
    }
}
