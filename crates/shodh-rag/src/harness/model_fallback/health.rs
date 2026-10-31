//! Health memory of models (a circuit breaker), in memory only.
//!
//! A model that failed in a way another try soon will not fix (unavailable, a 5xx, a
//! timeout, a broken connection) is skipped for [`FIRST_COOL_DOWN_MS`]; each further
//! failure doubles the wait, up to [`MAX_COOL_DOWN_MS`]. After the wait one call tries
//! it again (half-open, the "probe"); calls made while that probe runs skip the model
//! instead of waiting for it. A success forgets the model's failures. Keyed by provider
//! and model. Every method takes the time: the book never reads a clock itself.
//!
//! - Failures that arrive while the model already cools down (calls that were in flight
//!   when it failed) do not double the wait again.
//! - A model whose failures were all broken connections is forgotten as soon as any
//!   model answers: the connection is back, and the outage was this computer's, not
//!   the model's.
//! - A streak ends after a quiet gap ([`STREAK_GAP_MS`]); entries idle for a day are
//!   dropped.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;

use crate::harness::model_catalog::{ModelRef, ProviderId};
use crate::harness::provider_error::ProviderErrorKind;

/// The first wait after a model failed.
pub const FIRST_COOL_DOWN_MS: u64 = 2 * 60 * 1_000;
/// The longest wait, however often the model keeps failing.
pub const MAX_COOL_DOWN_MS: u64 = 30 * 60 * 1_000;
/// How long one probe holds the half-open model: longer than one call may take
/// ([`super::run::DEFAULT_BOUNDS`]'s time cap), so a running probe is never doubled.
pub const PROBE_LEASE_MS: u64 = 3 * 60 * 1_000;
/// After failing this long without a success, the user's own model earns one notice.
pub const LONG_FAILURE_MS: u64 = 60 * 60 * 1_000;
/// A failure this long after the previous one starts a new streak.
pub const STREAK_GAP_MS: u64 = 2 * MAX_COOL_DOWN_MS;
/// Entries untouched this long are dropped.
pub const IDLE_ENTRY_MS: u64 = 24 * 60 * 60 * 1_000;

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

    /// A call may use the model now (healthy, or due for its one retry).
    pub fn usable(self) -> bool {
        matches!(self, Health::Healthy | Health::Retrying { .. })
    }
}

#[derive(Debug, Clone)]
struct Entry {
    failures: u32,
    open_until_ms: u64,
    failing_since_ms: u64,
    last_failure_ms: u64,
    probe_started_ms: Option<u64>,
    last: ProviderErrorKind,
    long_failure_noticed: bool,
    /// Every failure of the streak was a broken connection.
    network_only: bool,
}

impl Entry {
    fn new(kind: ProviderErrorKind, now_ms: u64) -> Self {
        Self {
            failures: 0,
            open_until_ms: 0,
            failing_since_ms: now_ms,
            last_failure_ms: now_ms,
            probe_started_ms: None,
            last: kind,
            long_failure_noticed: false,
            network_only: true,
        }
    }
}

/// The wait after the `failures`-th failure in a row.
pub fn cool_down_ms(failures: u32) -> u64 {
    let doublings = failures.saturating_sub(1).min(16);
    FIRST_COOL_DOWN_MS
        .saturating_mul(1u64 << doublings)
        .min(MAX_COOL_DOWN_MS)
}

#[derive(Debug, Default)]
struct Book {
    entries: HashMap<ModelRef, Entry>,
    /// Models that earned the long-failure notice and answered since.
    recovered: HashSet<ModelRef>,
}

/// The health of every model that failed recently.
#[derive(Debug, Default)]
pub struct HealthBook {
    book: Mutex<Book>,
}

impl HealthBook {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `model`'s health at `now_ms`.
    pub fn health(&self, model: &ModelRef, now_ms: u64) -> Health {
        match self.lock().entries.get(model) {
            None => Health::Healthy,
            Some(entry) => entry_health(entry, now_ms),
        }
    }

    /// Lets a call use `model`, or `None` while it cools down or another call probes
    /// it. Never waits.
    pub fn admit(&self, model: &ModelRef, now_ms: u64) -> Option<Admission<'_>> {
        let mut book = self.lock();
        let probe = match book.entries.get_mut(model) {
            None => None,
            Some(entry) => match entry_health(entry, now_ms) {
                Health::Healthy => None,
                Health::CoolingDown { .. } | Health::Probing { .. } => return None,
                Health::Retrying { .. } => {
                    entry.probe_started_ms = Some(now_ms);
                    Some(now_ms)
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
    /// model is down count ([`trips_breaker`]); others leave its health as it was.
    pub fn record_failure(&self, model: &ModelRef, kind: ProviderErrorKind, now_ms: u64) {
        self.failed(model, kind, now_ms, None);
    }

    /// `probe`: the start of the probe this failure ends, if it was one.
    fn failed(&self, model: &ModelRef, kind: ProviderErrorKind, now_ms: u64, probe: Option<u64>) {
        let mut book = self.lock();
        book.entries
            .retain(|_, e| now_ms.saturating_sub(e.last_failure_ms) < IDLE_ENTRY_MS);
        if !trips_breaker(kind) {
            if let Some(entry) = book.entries.get_mut(model) {
                release(entry, probe);
            }
            return;
        }
        book.recovered.remove(model);
        let entry = book
            .entries
            .entry(model.clone())
            .or_insert_with(|| Entry::new(kind, now_ms));
        if now_ms.saturating_sub(entry.last_failure_ms) > STREAK_GAP_MS {
            // A quiet gap: this is a new streak, not the old one continuing.
            *entry = Entry::new(kind, now_ms);
        }
        entry.last_failure_ms = now_ms;
        entry.last = kind;
        entry.network_only &= kind == ProviderErrorKind::Network;
        let probing = probe.is_some() && entry.probe_started_ms == probe;
        if !probing && now_ms < entry.open_until_ms {
            // A call that was in flight when the model failed: the wait already counts it.
            return;
        }
        entry.failures = entry.failures.saturating_add(1);
        entry.open_until_ms = now_ms.saturating_add(cool_down_ms(entry.failures));
        entry.probe_started_ms = None;
    }

    /// A call to `model` succeeded: its failures are forgotten, and so are those of
    /// models that only lost their connection (the connection works again).
    pub fn record_success(&self, model: &ModelRef) {
        let mut book = self.lock();
        if let Some(entry) = book.entries.remove(model) {
            if entry.long_failure_noticed {
                book.recovered.insert(model.clone());
            }
        }
        book.entries.retain(|_, e| !e.network_only);
    }

    /// Skips `model` for `wait_ms` after a failure that is not about its health but
    /// makes it useless for now (its provider refused the key).
    pub fn bench(&self, model: &ModelRef, kind: ProviderErrorKind, now_ms: u64, wait_ms: u64) {
        let mut book = self.lock();
        let entry = book
            .entries
            .entry(model.clone())
            .or_insert_with(|| Entry::new(kind, now_ms));
        entry.failures = entry.failures.max(1);
        entry.network_only = false;
        entry.last = kind;
        entry.last_failure_ms = now_ms;
        entry.probe_started_ms = None;
        entry.open_until_ms = entry.open_until_ms.max(now_ms.saturating_add(wait_ms));
    }

    /// When `model` started failing without a success since, if it is failing.
    pub fn failing_since(&self, model: &ModelRef) -> Option<u64> {
        self.lock().entries.get(model).map(|e| e.failing_since_ms)
    }

    /// `true` once per failing streak, when `model` has failed for at least
    /// [`LONG_FAILURE_MS`] without a success.
    pub fn take_long_failure(&self, model: &ModelRef, now_ms: u64) -> bool {
        let mut book = self.lock();
        match book.entries.get_mut(model) {
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

    /// The user acted on the long-failure notices of `provider`'s models (they checked
    /// the provider): each of its failing models starts its long-failure clock again at
    /// `now_ms`, so one that keeps failing is noticed again after [`LONG_FAILURE_MS`]
    /// more. Returns how many models it applied to.
    pub fn restart_long_failure_notices(&self, provider: ProviderId, now_ms: u64) -> usize {
        let mut book = self.lock();
        let mut restarted = 0;
        for (model, entry) in book.entries.iter_mut() {
            if model.provider == provider {
                entry.failing_since_ms = now_ms;
                entry.long_failure_noticed = false;
                restarted += 1;
            }
        }
        restarted
    }

    /// `true` once after a model that earned the long-failure notice answered again
    /// (its notice can be taken back).
    pub fn take_recovered(&self, model: &ModelRef) -> bool {
        self.lock().recovered.remove(model)
    }

    /// Models with an entry.
    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn release_probe(&self, model: &ModelRef, started: u64) {
        if let Some(entry) = self.lock().entries.get_mut(model) {
            release(entry, Some(started));
        }
    }
}

/// Frees the probe `started`, if it is still the one holding the model (a later probe
/// is never freed by an earlier one).
fn release(entry: &mut Entry, started: Option<u64>) {
    if started.is_some() && entry.probe_started_ms == started {
        entry.probe_started_ms = None;
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
/// was cancelled, or its failure is judged later), a probe frees the model for the next
/// call.
#[derive(Debug)]
pub struct Admission<'a> {
    book: &'a HealthBook,
    model: ModelRef,
    /// When this call's probe started, if it is the half-open model's retry.
    probe: Option<u64>,
    settled: bool,
}

impl Admission<'_> {
    /// This call is the half-open model's one retry.
    pub fn is_probe(&self) -> bool {
        self.probe.is_some()
    }

    pub fn succeeded(mut self) {
        self.settled = true;
        self.book.record_success(&self.model);
    }

    pub fn failed(mut self, kind: ProviderErrorKind, now_ms: u64) {
        self.settled = true;
        self.book.failed(&self.model, kind, now_ms, self.probe);
    }
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        if let (false, Some(started)) = (self.settled, self.probe) {
            self.book.release_probe(&self.model, started);
        }
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;

    const MINUTE: u64 = 60 * 1_000;

    /// Fails `model` every ten minutes from `from` to `to` (one streak: the gaps are
    /// shorter than [`STREAK_GAP_MS`]).
    fn keep_failing(book: &HealthBook, model: &ModelRef, from: u64, to: u64) {
        let mut at = from;
        while at <= to {
            book.record_failure(model, ProviderErrorKind::ModelUnavailable, at);
            at += 10 * MINUTE;
        }
    }

    #[test]
    fn a_cleared_notice_comes_back_when_the_model_keeps_failing() {
        let book = HealthBook::new();
        let model = ModelRef::new(ProviderId::OpenRouter, "vendor/retired-model:free");
        let other = ModelRef::new(ProviderId::Anthropic, "claude-haiku-4-5");
        keep_failing(&book, &model, 0, LONG_FAILURE_MS);
        keep_failing(&book, &other, 0, LONG_FAILURE_MS);
        let first = LONG_FAILURE_MS;
        assert!(
            book.take_long_failure(&model, first),
            "noticed after an hour of failing"
        );
        assert!(!book.take_long_failure(&model, first), "once per streak");
        assert!(book.take_long_failure(&other, first));

        // The user checks OpenRouter (its key works): the notice is cleared and its clock restarts.
        assert_eq!(
            book.restart_long_failure_notices(ProviderId::OpenRouter, first),
            1
        );
        // It keeps failing: not noticed again before another hour...
        keep_failing(
            &book,
            &model,
            first + 10 * MINUTE,
            first + LONG_FAILURE_MS - MINUTE,
        );
        assert!(!book.take_long_failure(&model, first + LONG_FAILURE_MS - MINUTE));
        // ...and noticed again once it has.
        keep_failing(
            &book,
            &model,
            first + LONG_FAILURE_MS,
            first + LONG_FAILURE_MS,
        );
        assert!(book.take_long_failure(&model, first + LONG_FAILURE_MS));
        // Another provider's model was not touched: its notice stays taken.
        assert!(!book.take_long_failure(&other, first + LONG_FAILURE_MS));
    }
}
