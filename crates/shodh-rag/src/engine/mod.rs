//! The scheduler engine: work that runs later, on a schedule or when something happens,
//! survives sleep and restarts, and leaves a record the user can read.
//!
//! Its first parts came from the evidence-table runner (leases, the backoff ladder, the
//! clock), which imports them from here, so both share one lease and one ladder.

pub mod backoff;
pub mod clock;
pub mod lease;
