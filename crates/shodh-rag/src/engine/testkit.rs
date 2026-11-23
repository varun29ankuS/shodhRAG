//! Test helpers of the engine: a clock that moves only when told, local instants, and a
//! handler that records its calls and answers from a script.

use std::collections::VecDeque;
use std::sync::Mutex;

use chrono::{DateTime, Local, NaiveDate, Utc};

use super::clock::{resolve_local, Clock};
use super::runner::{Handled, JobHandler};
use super::store::{Job, JobKind, Run};

/// The instant of a local wall-clock time on this computer.
pub fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
    let naive = NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(h, min, 0)
        .unwrap();
    resolve_local(&Local, naive).unwrap()
}

/// A clock that moves only when told.
pub struct ManualClock {
    now: Mutex<DateTime<Utc>>,
}

impl ManualClock {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(now),
        }
    }

    pub fn set(&self, now: DateTime<Utc>) {
        *self.now.lock().unwrap_or_else(|e| e.into_inner()) = now;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A handler that records each run it is given and answers from a script (then
/// `Succeeded`).
pub struct Recording {
    kind: JobKind,
    script: Mutex<VecDeque<Handled>>,
    calls: Mutex<Vec<Run>>,
    folded: Mutex<usize>,
    held: Mutex<usize>,
}

impl Recording {
    pub fn new(kind: JobKind) -> Self {
        Self {
            kind,
            script: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
            folded: Mutex::new(0),
            held: Mutex::new(0),
        }
    }

    pub fn script(&self, answers: Vec<Handled>) {
        self.script
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(answers);
    }

    pub fn calls(&self) -> Vec<Run> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn folded(&self) -> usize {
        *self.folded.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn held(&self) -> usize {
        *self.held.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait::async_trait]
impl JobHandler for Recording {
    fn kind(&self) -> JobKind {
        self.kind
    }

    async fn run(&self, _job: &Job, run: &Run) -> Handled {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(run.clone());
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Handled::Succeeded {
                summary: "Done".to_string(),
                scheduled_for: None,
            })
    }

    async fn folded(&self, _job: &Job, _run: &Run) {
        *self.folded.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }

    async fn held(&self, _job: &Job, _run: &Run) {
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}
