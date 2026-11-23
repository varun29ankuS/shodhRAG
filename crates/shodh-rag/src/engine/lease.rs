//! Leases: one run per job. A run claims its job under this process's token and a
//! heartbeat; another process may take it over only once the heartbeat is older than
//! [`LEASE_MINUTES`]. Within one process a run holds a slot for its job ([`LiveRun`]).

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};

/// How long a run's heartbeat holds its job against another process. The heartbeat is
/// refreshed before every paper and every model call, so the longest gap between two
/// beats is one model call (the HTTP client's timeout is 300 s) plus the local work of a
/// paper (hashing, retrieval, the grounding model: seconds). Ten minutes is twice the
/// call timeout: a live run is never taken over, and a dead one is resumed within ten
/// minutes of its last beat.
pub const LEASE_MINUTES: i64 = 10;

/// This process's claim on the jobs it runs.
pub(crate) static PROCESS_TOKEN: LazyLock<String> =
    LazyLock::new(|| uuid::Uuid::new_v4().to_string());

/// This process's run token.
pub(crate) fn process_token() -> String {
    PROCESS_TOKEN.clone()
}

/// The jobs this process is running now, by (token, job id).
static LIVE_RUNS: LazyLock<Mutex<HashSet<(String, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn live_runs() -> std::sync::MutexGuard<'static, HashSet<(String, String)>> {
    LIVE_RUNS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A run's slot for its job in this process; given back when the run ends, however.
pub(crate) struct LiveRun {
    key: (String, String),
}

impl LiveRun {
    pub(crate) fn enter(token: &str, job_id: &str) -> Option<Self> {
        let key = (token.to_string(), job_id.to_string());
        // The set's lock is let go before a slot exists: a slot dropped under it would
        // lock it again, and a refused caller must not free the holder's slot.
        let entered = live_runs().insert(key.clone());
        if entered {
            Some(Self { key })
        } else {
            None
        }
    }

    pub(crate) fn is_live(token: &str, job_id: &str) -> bool {
        live_runs().contains(&(token.to_string(), job_id.to_string()))
    }
}

impl Drop for LiveRun {
    fn drop(&mut self) {
        live_runs().remove(&self.key);
    }
}

/// Fixed-width UTC with milliseconds and `Z`: string order is time order.
pub fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Heartbeats at or before this instant belong to a run that is gone.
pub fn lease_cutoff(now: DateTime<Utc>) -> String {
    timestamp(now - TimeDelta::minutes(LEASE_MINUTES))
}
