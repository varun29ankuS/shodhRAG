//! Health memory of models (a circuit breaker), in memory only.
//!
//! A model that failed in a way another try soon will not fix (unavailable, a 5xx, a
//! timeout, a broken connection) is skipped for [`FIRST_COOL_DOWN_MS`]; each further
//! failure doubles the wait, up to [`MAX_COOL_DOWN_MS`]. After the wait one call tries
//! it again (half-open, the "probe"); calls made while that probe runs skip the model
//! instead of waiting for it. A success forgets the model's failures. Keyed by provider
//! and model. Every method takes the time: the book never reads a clock itself.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;

use crate::harness::model_catalog::ModelRef;
use crate::harness::provider_error::ProviderErrorKind;

/// The first wait after a model failed.
pub const FIRST_COOL_DOWN_MS: u64 = 2 * 60 * 1_000;
/// The longest wait, however often the model keeps failing.
pub const MAX_COOL_DOWN_MS: u64 = 30 * 60 * 1_000;
/// How long one probe holds the half-open model. A probe that never reports (its call
/// was dropped without a guard, which [`Admission`] prevents) frees the model after this.
pub const PROBE_LEASE_MS: u64 = 2 * 60 * 1_000;
/// After failing this long without a success, the user's own model earns one notice.
pub const LONG_FAILURE_MS: u64 = 60 * 60 * 1_000;

/// Whether a failure of `kind` says the model is down (and skipping it helps). Rate
/// limits, quotas, refusals and bad requests are about the account or the request, not
/// the model's health.
pub fn trips_breaker(kind: ProviderErrorKind) -> bool {
    matches!(
        kind,
        ProviderErrorKind::ModelUnavailable
            | ProviderErrorKind::Timeout
            | ProviderErrorKind::Network
    )
}

/// A model's health now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Health {
    /// No recent failure.
    Healthy,
    /// Failed recently: skipped until `until_ms`.
    #[serde(rename_all = "camelCase")]
    CoolingDown {
        until_ms: u64,
        failures: u32,
        last: ProviderErrorKind,
    },
    /// The wait is over: the next call tries it once.
    #[serde(rename_all = "camelCase")]
    Retrying {
        failures: u32,
        last: ProviderErrorKind,
    },
    /// One call is trying it now; others skip it meanwhile.
    #[serde(rename_all = "camelCase")]
    Probing {
        failures: u32,
        last: ProviderErrorKind,
    },
}

impl Health {
    /// Ordering for ranking: healthy first, then a model due for its retry, then the
    /// rest.
    pub fn rank(self) -> u8 {
        match self {
            Health::Healthy => 0,
            Health::Retrying { .. } => 1,
            Health::Probing { .. } | Health::CoolingDown { .. } => 2,
        }
    }

    /// The failure that made it unhealthy.
    pub fn last_failure(self) -> Option<ProviderErrorKind> {
        match self {
            Health::Healthy => None,
            Health::CoolingDown { last, .. }
            | Health::Retrying { last, .. }
            | Health::Probing { last, .. } => Some(last),
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    failures: u32,
    open_until_ms: u64,
    failing_since_ms: u64,
    probe_started_ms: Option<u64>,
    last: ProviderErrorKind,
    long_failure_noticed: bool,
}

/// The wait after the `failures`-th failure in a row.
pub fn cool_down_ms(failures: u32) -> u64 {
    let doublings = failures.saturating_sub(1).min(16);
    FIRST_COOL_DOWN_MS
        .saturating_mul(1u64 << doublings)
        .min(MAX_COOL_DOWN_MS)
}

/// The health of every model that failed recently.
#[derive(Debug, Default)]
pub struct HealthBook {
    entries: Mutex<HashMap<ModelRef, Entry>>,
}

impl HealthBook {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<ModelRef, Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `model`'s health at `now_ms`.
    pub fn health(&self, model: &ModelRef, now_ms: u64) -> Health {
        match self.lock().get(model) {
            None => Health::Healthy,
            Some(entry) => entry_health(entry, now_ms),
        }
    }

    /// Lets a call use `model`, or `None` while it cools down or another call probes
    /// it. Never waits.
    pub fn admit<'a>(&'a self, model: &ModelRef, now_ms: u64) -> Option<Admission<'a>> {
        let mut entries = self.lock();
        let probe = match entries.get_mut(model) {
            None => false,
            Some(entry) => match entry_health(entry, now_ms) {
                Health::Healthy => false,
                Health::CoolingDown { .. } | Health::Probing { .. } => return None,
                Health::Retrying { .. } => {
                    entry.probe_started_ms = Some(now_ms);
                    true
                }
            },
        };
        Some(Admission {
            book: self,
            model: model.clone(),
            probe,
            settled: false,
        })
    }

    /// A call to `model` failed with `kind` at `now_ms`. Only failures that say the
    /// model is down count ([`trips_breaker`]); others leave its health as it was (a
    /// probe that hit one is released).
    pub fn record_failure(&self, model: &ModelRef, kind: ProviderErrorKind, now_ms: u64) {
        let mut entries = self.lock();
        if !trips_breaker(kind) {
            if let Some(entry) = entries.get_mut(model) {
                entry.probe_started_ms = None;
            }
            return;
        }
        let entry = entries.entry(model.clone()).or_insert(Entry {
            failures: 0,
            open_until_ms: 0,
            failing_since_ms: now_ms,
            probe_started_ms: None,
            last: kind,
            long_failure_noticed: false,
        });
        entry.failures = entry.failures.saturating_add(1);
        entry.open_until_ms = now_ms.saturating_add(cool_down_ms(entry.failures));
        entry.probe_started_ms = None;
        entry.last = kind;
    }

    /// A call to `model` succeeded: its failures are forgotten.
    pub fn record_success(&self, model: &ModelRef) {
        self.lock().remove(model);
    }

    /// When `model` started failing without a success since, if it is failing.
    pub fn failing_since(&self, model: &ModelRef) -> Option<u64> {
        self.lock().get(model).map(|e| e.failing_since_ms)
    }

    /// `true` once per failing streak, when `model` has failed for at least
    /// [`LONG_FAILURE_MS`] without a success.
    pub fn take_long_failure(&self, model: &ModelRef, now_ms: u64) -> bool {
        let mut entries = self.lock();
        match entries.get_mut(model) {
            Some(entry)
                if !entry.long_failure_noticed
                    && now_ms.saturating_sub(entry.failing_since_ms) >= LONG_FAILURE_MS =>
            {
                entry.long_failure_noticed = true;
                true
            }
            _ => false,
        }
    }

    fn release_probe(&self, model: &ModelRef) {
        if let Some(entry) = self.lock().get_mut(model) {
            entry.probe_started_ms = None;
        }
    }
}

fn entry_health(entry: &Entry, now_ms: u64) -> Health {
    let (failures, last) = (entry.failures, entry.last);
    if now_ms < entry.open_until_ms {
        return Health::CoolingDown {
            until_ms: entry.open_until_ms,
            failures,
            last,
        };
    }
    match entry.probe_started_ms {
        Some(started) if now_ms.saturating_sub(started) < PROBE_LEASE_MS => {
            Health::Probing { failures, last }
        }
        _ => Health::Retrying { failures, last },
    }
}

/// One call's permission to use a model. Report how it went with
/// [`Admission::succeeded`] or [`Admission::failed`]; dropped without either (the call
/// was cancelled), a probe frees the model for the next call.
#[derive(Debug)]
pub struct Admission<'a> {
    book: &'a HealthBook,
    model: ModelRef,
    probe: bool,
    settled: bool,
}

impl Admission<'_> {
    /// This call is the half-open model's one retry.
    pub fn is_probe(&self) -> bool {
        self.probe
    }

    pub fn succeeded(mut self) {
        self.settled = true;
        self.book.record_success(&self.model);
    }

    pub fn failed(mut self, kind: ProviderErrorKind, now_ms: u64) {
        self.settled = true;
        self.book.record_failure(&self.model, kind, now_ms);
    }
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        if !self.settled && self.probe {
            self.book.release_probe(&self.model);
        }
    }
}
