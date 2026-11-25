//! The scheduler engine: work that runs later, on a schedule or when something happens,
//! survives sleep and restarts, and leaves a record the user can read.
//!
//! Its first parts came from the evidence-table runner (leases, the backoff ladder, the
//! clock), which imports them from here, so both share one lease and one ladder.
//!
//! - [`ledger`]: the model calls each part of Shodh made per day.
//! - [`schedule`]: typed schedules and their next times, in local time.
//! - [`store`]: jobs and runs in `shodh.db`.
//! - [`plan`]: what a job's schedule asks for now (catch-up, overlap, the decision fold).
//! - [`runner`]: the engine that ticks jobs through their kind's handler.

pub mod backoff;
pub mod clock;
pub mod lease;
pub mod ledger;
pub mod plan;
pub mod runner;
pub mod schedule;
pub mod store;
#[cfg(test)]
pub(crate) mod testkit;
