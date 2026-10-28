//! Runs one call down its [`Plan`]: the requested model, then at most
//! [`Bounds::max_fallbacks`] others, within [`Bounds::time_cap`], never after any of the
//! answer reached the user.

use std::future::Future;
use std::time::Duration;

use serde::Serialize;

use super::health::{HealthBook, MAX_COOL_DOWN_MS};
use super::policy::{decide, Decision, Eligible, MAX_RETRY_WAIT};
use super::resolve::{Candidate, Origin, Plan};
use crate::harness::model_catalog::ModelRef;
use crate::harness::provider_error::ProviderErrorKind;
use crate::llm::ProviderFailure;

/// The time, for the time cap and the health book. Tests use a fake clock.
pub trait Clock: Send + Sync {
    /// Unix milliseconds.
    fn now_ms(&self) -> u64;
}

/// Waiting, for a rate limit's `Retry-After`. Tests use a fake that advances their clock.
#[async_trait::async_trait]
pub trait Pause: Send + Sync {
    async fn pause(&self, wait: Duration);
}

/// The computer's clock, and a real wait.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemTime;

impl Clock for SystemTime {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

#[async_trait::async_trait]
impl Pause for SystemTime {
    async fn pause(&self, wait: Duration) {
        tokio::time::sleep(wait).await;
    }
}

/// Limits of one call's fallbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// Models tried besides the requested one.
    pub max_fallbacks: usize,
    /// The whole call: no attempt runs past it, no new one (or wait) starts after it.
    pub time_cap: Duration,
    /// One attempt, while a backup could still follow it (a hung model then leaves time
    /// for the backup).
    pub attempt_timeout: Duration,
}

/// At most 2 fallbacks, 60 s per attempt while a backup could follow, 150 s in all.
pub const DEFAULT_BOUNDS: Bounds = Bounds {
    max_fallbacks: 2,
    time_cap: Duration::from_secs(150),
    attempt_timeout: Duration::from_secs(60),
};

impl Default for Bounds {
    fn default() -> Self {
        DEFAULT_BOUNDS
    }
}

/// A failed attempt: the provider's failure, and whether any of the answer had already
/// reached the user (then nothing switches).
#[derive(Debug, Clone)]
pub struct AttemptFailure {
    pub failure: ProviderFailure,
    pub streamed: bool,
}

impl From<ProviderFailure> for AttemptFailure {
    fn from(failure: ProviderFailure) -> Self {
        Self {
            failure,
            streamed: false,
        }
    }
}

/// Why a model was left for the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "why", rename_all = "snake_case")]
pub enum SwitchReason {
    /// It failed with this kind.
    Failed { kind: ProviderErrorKind },
    /// It failed recently and was skipped (this was its last failure).
    Skipped { last: Option<ProviderErrorKind> },
    /// This call cannot use it (a subscription outside the agent, a cloud model in
    /// Local-only mode, a provider no longer connected).
    Unreachable,
}

impl SwitchReason {
    /// The failure kind behind the switch, when known.
    pub fn kind(self) -> Option<ProviderErrorKind> {
        match self {
            SwitchReason::Failed { kind } => Some(kind),
            SwitchReason::Skipped { last } => last,
            SwitchReason::Unreachable => None,
        }
    }
}

/// One model left for the next.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Switch {
    pub from: ModelRef,
    pub from_name: String,
    pub reason: SwitchReason,
}

/// Which model answered, and why it was not the requested one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnsweredBy {
    pub model: ModelRef,
    pub name: String,
    pub requested: ModelRef,
    pub requested_name: String,
    /// In order; empty when the requested model answered.
    pub switches: Vec<Switch>,
}

impl AnsweredBy {
    pub fn fell_back(&self) -> bool {
        self.model != self.requested
    }

    /// "Answered by Gemma 4 31B — Nemotron Super is unavailable", or `None` when the
    /// requested model answered.
    pub fn note(&self) -> Option<String> {
        if !self.fell_back() {
            return None;
        }
        let why = self
            .switches
            .iter()
            .find(|s| s.from == self.requested)
            .or_else(|| self.switches.first())
            .map(|s| format!("{} {}", s.from_name, reason_words(s.reason)));
        Some(match why {
            Some(why) => format!("Answered by {} — {why}", self.name),
            None => format!("Answered by {}", self.name),
        })
    }
}

/// Plain words for why a model was left ("is unavailable").
pub fn reason_words(reason: SwitchReason) -> &'static str {
    match reason {
        SwitchReason::Skipped { .. } => "has been failing, so it was skipped",
        SwitchReason::Unreachable => "cannot answer here",
        SwitchReason::Failed { kind } => match kind {
            ProviderErrorKind::RateLimited => "is rate-limited",
            ProviderErrorKind::QuotaExhausted => "has used up its quota",
            ProviderErrorKind::ModelUnavailable => "is unavailable",
            ProviderErrorKind::Timeout => "did not answer in time",
            ProviderErrorKind::Network => "could not be reached",
            ProviderErrorKind::ContextTooLong => "cannot take a request this long",
            _ => "could not answer",
        },
    }
}

/// Why a call ended without an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Another model would fail the same way (a bad key, a refusal, a bad request).
    NotHelpful,
    /// Part of the answer had reached the user.
    Streamed,
    /// No model fitting the failure was left.
    NoCandidate,
    /// [`Bounds::max_fallbacks`] models were tried after the requested one.
    MaxFallbacks,
    /// [`Bounds::time_cap`] passed.
    TimeCap,
    /// Every model was skipped for failing recently; none was called.
    AllSkipped,
}

/// A call that got no answer.
#[derive(Debug, Clone)]
pub struct Exhausted {
    /// The last failure (for [`StopReason::AllSkipped`], one made up from the requested
    /// model's health).
    pub failure: ProviderFailure,
    pub reason: StopReason,
    pub switches: Vec<Switch>,
    /// Models actually called, in order.
    pub tried: Vec<ModelRef>,
}

/// What a run of a plan gave.
#[derive(Debug, Clone)]
pub struct Answered<T> {
    pub value: T,
    pub by: AnsweredBy,
    pub tried: Vec<ModelRef>,
}

fn skipped_failure(candidate: &Candidate) -> ProviderFailure {
    if !candidate.reachable {
        return ProviderFailure {
            kind: ProviderErrorKind::ModelUnavailable,
            provider: candidate.model.provider.label().to_string(),
            upstream: None,
            model: candidate.model.model.clone(),
            status: None,
            detail: "This model is temporarily unavailable here (it is not connected, or it \
                     answers only through the assistant in Ask), and no other model is set \
                     up to answer instead."
                .to_string(),
        };
    }
    let kind = candidate
        .health
        .last_failure()
        .unwrap_or(ProviderErrorKind::ModelUnavailable);
    // Worded so the failure classifies as its kind again from text alone.
    let what = match kind {
        ProviderErrorKind::Timeout => "it timed out moments ago",
        ProviderErrorKind::Network => "a connection error ended its last call moments ago",
        ProviderErrorKind::Auth => "its provider refused the API key moments ago",
        _ => "it is temporarily unavailable (it failed moments ago)",
    };
    ProviderFailure {
        kind,
        provider: candidate.model.provider.label().to_string(),
        upstream: None,
        model: candidate.model.model.clone(),
        status: None,
        detail: format!("Skipped: {what}; it is tried again after a short wait."),
    }
}

/// An attempt that did not answer within `limit`.
fn timed_out(candidate: &Candidate, limit: Duration) -> ProviderFailure {
    ProviderFailure {
        kind: ProviderErrorKind::Timeout,
        provider: candidate.model.provider.label().to_string(),
        upstream: None,
        model: candidate.model.model.clone(),
        status: None,
        detail: format!(
            "The model did not answer in time (timed out after {} s).",
            limit.as_secs()
        ),
    }
}

/// Failures of a backup that say nothing about the call itself (its key was refused, it
/// refused this request shape, it sent nothing): that backup is passed over and the
/// next one tried. A content filter's refusal is about the request: it stops the call.
fn skips_backup(kind: ProviderErrorKind) -> bool {
    matches!(
        kind,
        ProviderErrorKind::Auth
            | ProviderErrorKind::InvalidRequest
            | ProviderErrorKind::EmptyResponse
            | ProviderErrorKind::Other
    )
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// What one run of a plan has seen so far.
struct Trail {
    switches: Vec<Switch>,
    tried: Vec<ModelRef>,
    /// The requested model's failure: the one the caller reports (it is the model the
    /// user chose), whatever the backups did after it.
    requested_failure: Option<ProviderFailure>,
    last: Option<ProviderFailure>,
    /// Broken connections, judged once the call ends: when no provider was reached at
    /// all, the outage is this computer's and no model's health is touched.
    network: Vec<ModelRef>,
    reached_provider: bool,
}

impl Trail {
    fn left(&mut self, candidate: &Candidate, reason: SwitchReason) {
        self.switches.push(Switch {
            from: candidate.model.clone(),
            from_name: candidate.name.clone(),
            reason,
        });
    }

    fn end(
        self,
        plan: &Plan,
        health: &HealthBook,
        now_ms: u64,
        reason: StopReason,
    ) -> Box<Exhausted> {
        if self.reached_provider {
            for model in &self.network {
                health.record_failure(model, ProviderErrorKind::Network, now_ms);
            }
        }
        let reason = if self.tried.is_empty() && reason == StopReason::NoCandidate {
            StopReason::AllSkipped
        } else {
            reason
        };
        Box::new(Exhausted {
            failure: self
                .requested_failure
                .or(self.last)
                .unwrap_or_else(|| skipped_failure(&plan.requested)),
            reason,
            switches: self.switches,
            tried: self.tried,
        })
    }
}

/// Runs `call` down `plan` (see the module docs and [`super::policy`]), recording each
/// outcome in `health`. Each attempt is bounded: [`Bounds::attempt_timeout`] while a
/// backup could still follow, and never past [`Bounds::time_cap`]; an attempt that runs
/// out of time is a timeout, so a hung model falls back like an unavailable one.
pub async fn run_plan<T, F, Fut>(
    plan: &Plan,
    health: &HealthBook,
    clock: &dyn Clock,
    pause: &dyn Pause,
    bounds: Bounds,
    mut call: F,
) -> Result<Answered<T>, Box<Exhausted>>
where
    F: FnMut(Candidate) -> Fut,
    Fut: Future<Output = Result<T, AttemptFailure>>,
{
    let started = clock.now_ms();
    let cap_ms = ms(bounds.time_cap);
    let candidates: Vec<&Candidate> = plan.candidates().collect();
    let mut trail = Trail {
        switches: Vec::new(),
        tried: Vec::new(),
        requested_failure: None,
        last: None,
        network: Vec::new(),
        reached_provider: false,
    };
    // Every failure so far narrows what may follow (a used-up free quota, then an
    // unavailable paid model, still rules out the other free models).
    let mut eligible: Vec<Eligible> = Vec::new();
    let mut retry_after: Option<Duration> = None;
    let mut waited = false;
    let mut fallbacks = 0usize;
    let mut index = 0usize;
    // The model a rate limit may wait out and retry (once), when nothing else fits.
    let mut retry_same: Option<&Candidate> = None;

    loop {
        let candidate = match retry_same.take() {
            Some(same) => same,
            None => {
                let next = candidates[index..]
                    .iter()
                    .position(|c| index == 0 || eligible.iter().all(|e| e.allows(c)))
                    .map(|offset| index + offset);
                match next {
                    Some(i) => {
                        index = i + 1;
                        candidates[i]
                    }
                    None => {
                        // Nothing else fits: a short rate-limit wait, once, on the same model.
                        let elapsed = clock.now_ms().saturating_sub(started);
                        let wait_ms = retry_after.filter(|w| *w <= MAX_RETRY_WAIT).map(ms);
                        let same = trail
                            .tried
                            .last()
                            .and_then(|m| candidates.iter().copied().find(|c| &c.model == m));
                        match (wait_ms, same) {
                            (Some(wait_ms), Some(same))
                                if !waited && elapsed.saturating_add(wait_ms) < cap_ms =>
                            {
                                waited = true;
                                retry_after = None;
                                pause.pause(Duration::from_millis(wait_ms)).await;
                                retry_same = Some(same);
                                continue;
                            }
                            _ => {
                                return Err(trail.end(
                                    plan,
                                    health,
                                    clock.now_ms(),
                                    StopReason::NoCandidate,
                                ))
                            }
                        }
                    }
                }
            }
        };
        let is_fallback = candidate.model != plan.requested.model;
        let new_model = !trail.tried.contains(&candidate.model);
        let elapsed = clock.now_ms().saturating_sub(started);
        if is_fallback && new_model {
            if fallbacks >= bounds.max_fallbacks {
                return Err(trail.end(plan, health, clock.now_ms(), StopReason::MaxFallbacks));
            }
            if elapsed >= cap_ms {
                return Err(trail.end(plan, health, clock.now_ms(), StopReason::TimeCap));
            }
        }
        if !candidate.reachable {
            trail.left(candidate, SwitchReason::Unreachable);
            continue;
        }
        let Some(admission) = health.admit(&candidate.model, clock.now_ms()) else {
            let last = health
                .health(&candidate.model, clock.now_ms())
                .last_failure();
            trail.left(candidate, SwitchReason::Skipped { last });
            continue;
        };
        if is_fallback && new_model {
            fallbacks += 1;
        }
        if new_model {
            trail.tried.push(candidate.model.clone());
        }
        // While a backup could still follow, one attempt may not use up the whole call.
        let remaining = Duration::from_millis(cap_ms.saturating_sub(elapsed).max(1_000));
        let backups_left = fallbacks < bounds.max_fallbacks && index < candidates.len();
        let limit = if backups_left {
            remaining.min(bounds.attempt_timeout)
        } else {
            remaining
        };
        let attempt = call(candidate.clone());
        let outcome = tokio::select! {
            biased;
            result = attempt => result,
            () = pause.pause(limit) => Err(AttemptFailure::from(timed_out(candidate, limit))),
        };
        let attempt = match outcome {
            Ok(value) => {
                admission.succeeded();
                for model in &trail.network {
                    health.record_failure(model, ProviderErrorKind::Network, clock.now_ms());
                }
                return Ok(Answered {
                    value,
                    by: AnsweredBy {
                        model: candidate.model.clone(),
                        name: candidate.name.clone(),
                        requested: plan.requested.model.clone(),
                        requested_name: plan.requested.name.clone(),
                        switches: trail.switches,
                    },
                    tried: trail.tried,
                });
            }
            Err(attempt) => attempt,
        };
        let now = clock.now_ms();
        let kind = attempt.failure.kind;
        if kind == ProviderErrorKind::Network {
            // Judged when the call ends (see `Trail::network`); the probe is freed.
            drop(admission);
            if !trail.network.contains(&candidate.model) {
                trail.network.push(candidate.model.clone());
            }
        } else {
            trail.reached_provider = true;
            admission.failed(kind, now);
        }
        if !is_fallback && trail.requested_failure.is_none() {
            trail.requested_failure = Some(attempt.failure.clone());
        }
        if attempt.streamed {
            trail.last = Some(attempt.failure);
            return Err(trail.end(plan, health, now, StopReason::Streamed));
        }
        if is_fallback && skips_backup(kind) {
            // This backup cannot answer; the call goes on as it was.
            if kind == ProviderErrorKind::Auth && candidate.origin == Origin::Automatic {
                health.bench(&candidate.model, kind, now, MAX_COOL_DOWN_MS);
            }
            trail.left(candidate, SwitchReason::Failed { kind });
            continue;
        }
        match decide(&attempt.failure, candidate, now) {
            Decision::Stop => {
                trail.last = Some(attempt.failure);
                return Err(trail.end(plan, health, now, StopReason::NotHelpful));
            }
            Decision::Switch {
                eligible: next,
                retry_after: wait,
            } => {
                trail.left(candidate, SwitchReason::Failed { kind });
                if next != Eligible::Any && !eligible.contains(&next) {
                    eligible.push(next);
                }
                retry_after = wait;
                trail.last = Some(attempt.failure);
            }
        }
    }
}
