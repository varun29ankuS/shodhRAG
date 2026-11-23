//! Waiting after failures: the ladder of growing waits and the panic rule.

/// Runs of one job that may panic before the job fails instead of being resumed again.
pub const MAX_PANICS: usize = 2;
/// The waits of the backoff ladder, in minutes: rate limits that say nothing of when they
/// reset, and an unreachable model. One rung per pause in a row; past the last the job
/// waits for the user. The ladder tops out at 30 minutes and holds there twice more.
pub const BACKOFF_LADDER_MINUTES: [i64; 7] = [1, 2, 5, 15, 30, 30, 30];
/// The wait after a rate limit whose reset time reads as already past (a fast clock).
pub const STALE_RESET_WAIT_SECS: i64 = 60;
