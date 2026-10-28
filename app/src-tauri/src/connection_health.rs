//! The connection status in the top bar: which model answers, through which
//! provider, how that provider is connected, and whether it works.
//!
//! "Connected" used to mean only "a key exists": a key the provider no
//! longer accepts ("401 User not found") still read as connected. Here each
//! provider's health comes from evidence:
//!
//! - **A check**: one request that generates nothing and spends no chat
//!   request ([`check_key`]: OpenRouter's key endpoint, a models list
//!   elsewhere), the subscription's sign-in status, or the local server's
//!   model list. It runs at startup, from "Check now", and after an answer
//!   fails with a rejected key or a used-up quota.
//! - **Answers**: a finished answer shows the provider works; a failed one
//!   shows a rejected key, a rate limit or a used-up quota.
//!
//! A check says whether the key is accepted, not whether today's free
//! allowance has requests left, so a limit seen in an answer is kept until
//! its reset time passes or an answer to that provider succeeds; a check
//! never turns it green. Each finding is tied to a fingerprint of the key it
//! was about, so a changed key starts unchecked.
//!
//! Keys never leave this module: the UI gets the last four characters at
//! most ([`mask_key`]), provider messages are cleaned of key echoes, and
//! nothing here logs a key.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use shodh_rag::harness::catalog_fetch::{answer_error_message, check_key, KeyHealth};
use shodh_rag::harness::model_catalog::{ProviderId, ProviderKind};
use shodh_rag::harness::model_choice::ActiveModel;
use shodh_rag::harness::omp_auth::SignInState;
use shodh_rag::harness::provider_error::{limit_reset_at_ms, ProviderError, ProviderErrorKind};
use shodh_rag::harness::{AgentEvent, RunStatus};
use tauri::{AppHandle, Emitter, Manager};

use crate::api_key_store::{self, KeyOrigin};
use crate::app_settings::SettingsStore;
use crate::connect_commands::{refresh_local, refresh_subscriptions, ConnectState};
use crate::llm_commands::LLMState;
use crate::model_picker_commands::{connected_providers, ollama_host, ModelPickerState};

/// Emitted with a [`ConnectionSnapshot`] whenever a provider's status changes.
pub const CONNECTION_STATUS_EVENT: &str = "connection-status-changed";

/// How long a rate limit without a reset time is shown.
const RATE_LIMIT_HOLD_MS: u64 = 2 * 60 * 1_000;
/// How long a used-up quota without a reset time is shown at most (an
/// answer that succeeds clears it sooner).
const QUOTA_HOLD_MS: u64 = 24 * 60 * 60 * 1_000;
/// Least time between two automatic checks of one provider after failed
/// answers (an error card can retry, and fall back, several times).
const AUTO_CHECK_COOLDOWN_MS: u64 = 60 * 1_000;
/// Characters of a key shown, at most.
pub const KEY_HINT_CHARS: usize = 4;
/// Keys shorter than this show no characters at all.
const MIN_KEY_FOR_HINT: usize = 12;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The last characters of a key, masked: `…a3f9`. Never more than
/// [`KEY_HINT_CHARS`] characters of the key; a short key shows none.
pub fn mask_key(key: &str) -> String {
    let key = key.trim();
    let count = key.chars().count();
    if count < MIN_KEY_FOR_HINT {
        return "…".to_string();
    }
    let tail: String = key.chars().skip(count - KEY_HINT_CHARS).collect();
    format!("…{tail}")
}

/// An in-memory fingerprint of a key, so a finding about one key is never
/// shown for another. Never stored or sent anywhere.
fn fingerprint(key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.trim().hash(&mut hasher);
    hasher.finish()
}

/// How a provider is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// The provider accepted the key or answered.
    Works,
    /// The key was refused (401/403) or the sign-in expired.
    Rejected,
    /// Rate-limited, or out of credit or quota.
    Limited,
    /// Not checked yet (or not checkable: local-only mode).
    Unchecked,
    /// The provider or local server could not be reached.
    Unreachable,
}

/// How a provider is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionSource {
    /// A key saved in the app.
    Saved,
    /// A key from an environment variable.
    Environment,
    /// A subscription sign-in.
    Subscription,
    /// A server on this computer (LM Studio, Ollama).
    Local,
}

impl From<KeyOrigin> for ConnectionSource {
    fn from(origin: KeyOrigin) -> Self {
        match origin {
            KeyOrigin::Saved => ConnectionSource::Saved,
            KeyOrigin::Environment => ConnectionSource::Environment,
        }
    }
}

/// Where a status was learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// A check (no chat request).
    Check,
    /// An answer in a chat.
    Answer,
}

/// One provider's status, as the top bar shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub provider: ProviderId,
    pub label: &'static str,
    pub source: Option<ConnectionSource>,
    pub health: Health,
    /// The provider's message (cleaned of anything that echoes the key), or
    /// why the status is what it is.
    pub message: Option<String>,
    /// The HTTP status behind `health`, when there was one.
    pub http_status: Option<u16>,
    /// For `limited`: out of credit or quota rather than rate-limited.
    pub quota: bool,
    /// For `limited`: when the provider said the limit resets (Unix ms).
    pub reset_at_ms: Option<u64>,
    /// When the status was learned (Unix ms).
    pub checked_at_ms: Option<u64>,
    pub learned_from: Option<Evidence>,
    /// The last characters of the key in use (`…a3f9`), for key providers.
    pub key_hint: Option<String>,
    /// The provider's environment variable that holds a key, when one does
    /// (whether or not it is the key in use: see `source`).
    pub env_var: Option<&'static str>,
    /// A check of this provider is running.
    pub checking: bool,
}

/// Everything the top bar shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSnapshot {
    /// The model the next answer uses.
    pub active: Option<ActiveModel>,
    /// The active model's provider first, then every other connected one.
    pub providers: Vec<ProviderStatus>,
    pub local_only: bool,
}

/// Whether the provider works, as last learned.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Finding {
    /// `Works`, `Rejected` or `Unreachable`.
    health: Health,
    http_status: Option<u16>,
    message: Option<String>,
    at_ms: u64,
    from: Evidence,
    /// The key it is about ([`fingerprint`]); `None` without a key.
    key: Option<u64>,
}

/// A rate limit or used-up quota, as last seen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Limit {
    quota: bool,
    http_status: Option<u16>,
    message: Option<String>,
    reset_at_ms: Option<u64>,
    at_ms: u64,
    from: Evidence,
    key: Option<u64>,
}

impl Limit {
    fn active(&self, now_ms: u64) -> bool {
        match self.reset_at_ms {
            Some(reset) => reset > now_ms,
            None => {
                let hold = if self.quota {
                    QUOTA_HOLD_MS
                } else {
                    RATE_LIMIT_HOLD_MS
                };
                now_ms.saturating_sub(self.at_ms) < hold
            }
        }
    }
}

/// What is known about one provider.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Record {
    finding: Option<Finding>,
    limit: Option<Limit>,
}

/// The status a record shows for the key in use (`key`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    health: Health,
    http_status: Option<u16>,
    message: Option<String>,
    quota: bool,
    reset_at_ms: Option<u64>,
    checked_at_ms: Option<u64>,
    learned_from: Option<Evidence>,
}

impl Shown {
    fn unchecked(message: Option<String>) -> Self {
        Shown {
            health: Health::Unchecked,
            http_status: None,
            message,
            quota: false,
            reset_at_ms: None,
            checked_at_ms: None,
            learned_from: None,
        }
    }
}

impl Record {
    /// A rejected key wins; then an active limit; then whether it works.
    fn shown(&self, key: Option<u64>, now_ms: u64) -> Shown {
        let finding = self.finding.as_ref().filter(|f| f.key == key);
        let limit = self
            .limit
            .as_ref()
            .filter(|l| l.key == key && l.active(now_ms));
        match (finding, limit) {
            (Some(f), _) if f.health == Health::Rejected => Shown {
                health: Health::Rejected,
                http_status: f.http_status,
                message: f.message.clone(),
                quota: false,
                reset_at_ms: None,
                checked_at_ms: Some(f.at_ms),
                learned_from: Some(f.from),
            },
            (_, Some(l)) => Shown {
                health: Health::Limited,
                http_status: l.http_status,
                message: l.message.clone(),
                quota: l.quota,
                reset_at_ms: l.reset_at_ms,
                checked_at_ms: Some(finding.map_or(l.at_ms, |f| f.at_ms.max(l.at_ms))),
                learned_from: Some(l.from),
            },
            (Some(f), None) => Shown {
                health: f.health,
                http_status: f.http_status,
                message: f.message.clone(),
                quota: false,
                reset_at_ms: None,
                checked_at_ms: Some(f.at_ms),
                learned_from: Some(f.from),
            },
            (None, None) => Shown::unchecked(None),
        }
    }

    /// Record a key check. A check that the key works clears a limit an
    /// earlier check found (credit was added), but not one seen in an
    /// answer: the check cannot see a daily free-model allowance.
    fn apply_check(&mut self, health: KeyHealth, key: Option<u64>, now_ms: u64) {
        let finding = |health, http_status, message| Finding {
            health,
            http_status,
            message,
            at_ms: now_ms,
            from: Evidence::Check,
            key,
        };
        match health {
            KeyHealth::Works => {
                self.finding = Some(finding(Health::Works, Some(200), None));
                if self
                    .limit
                    .as_ref()
                    .is_some_and(|l| l.from == Evidence::Check || l.key != key)
                {
                    self.limit = None;
                }
            }
            KeyHealth::Rejected { status, message } => {
                self.finding = Some(finding(Health::Rejected, status, message));
            }
            KeyHealth::Limited {
                status,
                quota,
                message,
                reset_at_ms,
            } => {
                // The provider knew the key: it works, but is limited.
                self.finding = Some(finding(Health::Works, Some(status), None));
                self.limit = Some(Limit {
                    quota,
                    http_status: Some(status),
                    message,
                    reset_at_ms,
                    at_ms: now_ms,
                    from: Evidence::Check,
                    key,
                });
            }
            KeyHealth::Unreachable { message } => {
                self.finding = Some(finding(Health::Unreachable, None, Some(message)));
            }
        }
    }

    /// Record a finished answer: `error` is its provider failure (`None`
    /// for an answer that completed), `message` the provider's text.
    fn apply_answer(
        &mut self,
        error: Option<&ProviderError>,
        message: Option<String>,
        key: Option<u64>,
        now_ms: u64,
    ) {
        let Some(error) = error else {
            self.finding = Some(Finding {
                health: Health::Works,
                http_status: None,
                message: None,
                at_ms: now_ms,
                from: Evidence::Answer,
                key,
            });
            self.limit = None;
            return;
        };
        match error.kind {
            ProviderErrorKind::Auth => {
                self.finding = Some(Finding {
                    health: Health::Rejected,
                    http_status: error.status,
                    message,
                    at_ms: now_ms,
                    from: Evidence::Answer,
                    key,
                });
            }
            ProviderErrorKind::RateLimited | ProviderErrorKind::QuotaExhausted => {
                let quota = error.kind == ProviderErrorKind::QuotaExhausted;
                let reset_at_ms = error
                    .retry_after_secs
                    .map(|secs| now_ms.saturating_add(u64::from(secs) * 1_000));
                self.limit = Some(Limit {
                    quota,
                    http_status: error.status,
                    message,
                    reset_at_ms,
                    at_ms: now_ms,
                    from: Evidence::Answer,
                    key,
                });
            }
            // Not about the connection: the model, the request or the network.
            ProviderErrorKind::ModelUnavailable
            | ProviderErrorKind::Timeout
            | ProviderErrorKind::Network
            | ProviderErrorKind::InvalidRequest
            | ProviderErrorKind::ContextTooLong
            | ProviderErrorKind::ContentFiltered
            | ProviderErrorKind::EmptyResponse
            | ProviderErrorKind::Other => {}
        }
    }
}

/// Process-wide connection status.
#[derive(Default)]
pub struct ConnectionHealthState {
    records: Mutex<HashMap<ProviderId, Record>>,
    /// Providers with a check running (one at a time each).
    checking: Mutex<HashSet<ProviderId>>,
    /// When each provider was last checked automatically after a failure.
    auto_checked: Mutex<HashMap<ProviderId, u64>>,
}

impl ConnectionHealthState {
    fn update(&self, provider: ProviderId, change: impl FnOnce(&mut Record)) {
        change(lock(&self.records).entry(provider).or_default());
    }

    fn record(&self, provider: ProviderId) -> Record {
        lock(&self.records)
            .get(&provider)
            .cloned()
            .unwrap_or_default()
    }

    /// Mark a check of `provider` as running; false when one already is.
    fn begin_check(&self, provider: ProviderId) -> bool {
        lock(&self.checking).insert(provider)
    }

    fn end_check(&self, provider: ProviderId) {
        lock(&self.checking).remove(&provider);
    }

    fn is_checking(&self, provider: ProviderId) -> bool {
        lock(&self.checking).contains(&provider)
    }

    /// Whether an automatic check may run now (and note it if so).
    fn take_auto_check(&self, provider: ProviderId, now_ms: u64) -> bool {
        let mut last = lock(&self.auto_checked);
        match last.get(&provider) {
            Some(at) if now_ms.saturating_sub(*at) < AUTO_CHECK_COOLDOWN_MS => false,
            _ => {
                last.insert(provider, now_ms);
                true
            }
        }
    }
}

/// The key `provider` uses (saved in the app, else the environment), and
/// where it comes from. In memory only.
fn key_in_use(llm: &LLMState, provider: ProviderId) -> Option<(String, KeyOrigin)> {
    let id = provider.as_str();
    let saved = lock(&llm.api_keys).get(id);
    api_key_store::choose_key(saved, api_key_store::process_env_key(id))
}

/// Whether Local-only mode is on. Settings that cannot be read count as on
/// (fail closed, like the research and web tools): a cloud provider is
/// neither contacted nor shown as usable while the policy is unknown.
fn local_only(app: &AppHandle) -> bool {
    local_only_from(
        crate::profile::app_data_dir(app)
            .ok()
            .and_then(|dir| SettingsStore::in_dir(&dir).load().ok()),
    )
}

fn local_only_from(settings: Option<crate::app_settings::AppSettings>) -> bool {
    settings.is_none_or(|settings| settings.policy.local_only)
}

/// The providers the top bar lists: the active model's first, then every
/// connected one, then subscriptions whose sign-in expired.
fn listed_providers(
    active: Option<ProviderId>,
    llm: &LLMState,
    connect: &ConnectState,
) -> Vec<ProviderId> {
    let mut out: Vec<ProviderId> = active.into_iter().collect();
    out.extend(connected_providers(llm, connect));
    out.extend(
        connect
            .subscription_statuses()
            .into_iter()
            .filter(|s| s.state == SignInState::Expired)
            .map(|s| s.provider),
    );
    let mut seen = HashSet::new();
    out.retain(|p| seen.insert(*p));
    out
}

fn provider_status(
    app: &AppHandle,
    provider: ProviderId,
    local_only: bool,
    now_ms: u64,
) -> ProviderStatus {
    let health = app.state::<ConnectionHealthState>();
    let llm = app.state::<LLMState>();
    let connect = app.state::<ConnectState>();
    let record = health.record(provider);
    let id = provider.as_str();
    let mut status = ProviderStatus {
        provider,
        label: provider.label(),
        source: None,
        health: Health::Unchecked,
        message: None,
        http_status: None,
        quota: false,
        reset_at_ms: None,
        checked_at_ms: None,
        learned_from: None,
        key_hint: None,
        env_var: None,
        checking: health.is_checking(provider),
    };
    let shown = match provider.kind() {
        ProviderKind::ApiKey => {
            status.env_var = api_key_store::process_env_key_var(id);
            match key_in_use(&llm, provider) {
                Some((key, origin)) => {
                    status.source = Some(origin.into());
                    status.key_hint = Some(mask_key(&key));
                    if local_only {
                        Shown::unchecked(Some(
                            "Local-only mode is on: cloud providers are not used or checked."
                                .to_string(),
                        ))
                    } else {
                        record.shown(Some(fingerprint(&key)), now_ms)
                    }
                }
                None => Shown::unchecked(Some("No key is connected.".to_string())),
            }
        }
        ProviderKind::Subscription => {
            status.source = Some(ConnectionSource::Subscription);
            let state = connect
                .subscription_statuses()
                .into_iter()
                .find(|s| s.provider == provider)
                .map(|s| s.state);
            if local_only {
                Shown::unchecked(Some(
                    "Local-only mode is on: cloud providers are not used or checked.".to_string(),
                ))
            } else {
                match state {
                    Some(SignInState::Expired) => Shown {
                        health: Health::Rejected,
                        message: Some("The sign-in expired. Sign in again.".to_string()),
                        ..record.shown(None, now_ms)
                    },
                    Some(SignInState::Connected) => record.shown(None, now_ms),
                    _ => Shown::unchecked(Some("Not signed in.".to_string())),
                }
            }
        }
        ProviderKind::Local => {
            status.source = Some(ConnectionSource::Local);
            record.shown(None, now_ms)
        }
    };
    status.health = shown.health;
    status.message = shown.message;
    status.http_status = shown.http_status;
    status.quota = shown.quota;
    status.reset_at_ms = shown.reset_at_ms;
    status.checked_at_ms = shown.checked_at_ms;
    status.learned_from = shown.learned_from;
    status
}

/// The current status of every listed provider.
pub fn snapshot(app: &AppHandle) -> ConnectionSnapshot {
    let llm = app.state::<LLMState>();
    let connect = app.state::<ConnectState>();
    let active = app.state::<ModelPickerState>().active(&llm);
    let local_only = local_only(app);
    let now = now_ms();
    let providers = listed_providers(active.as_ref().map(|a| a.model.provider), &llm, &connect)
        .into_iter()
        .map(|provider| provider_status(app, provider, local_only, now))
        .collect();
    ConnectionSnapshot {
        active,
        providers,
        local_only,
    }
}

/// Tell the top bar the status changed.
pub fn notify(app: &AppHandle) {
    if let Err(e) = app.emit(CONNECTION_STATUS_EVENT, snapshot(app)) {
        tracing::debug!(target: "shodh::connect", error = %e, "emitting the connection status failed");
    }
}

/// Check one provider without spending a chat request. Skipped when a
/// check of it is already running, and for cloud providers in local-only
/// mode.
async fn check_provider(app: &AppHandle, provider: ProviderId) {
    let health = app.state::<ConnectionHealthState>();
    if !health.begin_check(provider) {
        return;
    }
    notify(app);
    let now = now_ms();
    match provider.kind() {
        ProviderKind::ApiKey if !local_only(app) => {
            let key = key_in_use(&app.state::<LLMState>(), provider);
            if let Some((key, _)) = key {
                if let Ok(result) = check_key(provider, &key).await {
                    let fp = fingerprint(&key);
                    health.update(provider, |r| r.apply_check(result, Some(fp), now_ms()));
                }
            }
        }
        ProviderKind::Subscription if !local_only(app) => {
            let connect = app.state::<ConnectState>();
            refresh_subscriptions(app, &connect, true).await;
            let state = connect
                .subscription_statuses()
                .into_iter()
                .find(|s| s.provider == provider)
                .map(|s| s.state);
            let result = match state {
                Some(SignInState::Connected) => KeyHealth::Works,
                Some(SignInState::Expired) => KeyHealth::Rejected {
                    status: None,
                    message: Some("The sign-in expired. Sign in again.".to_string()),
                },
                _ => KeyHealth::Rejected {
                    status: None,
                    message: Some("Not signed in.".to_string()),
                },
            };
            health.update(provider, |r| r.apply_check(result, None, now));
        }
        ProviderKind::Local => {
            let connect = app.state::<ConnectState>();
            refresh_local(app, &connect, &ollama_host()).await;
            let running = match provider {
                ProviderId::LmStudio => connect.lmstudio().running,
                _ => connect.ollama().running,
            };
            let result = if running {
                KeyHealth::Works
            } else {
                KeyHealth::Unreachable {
                    message: format!("{} isn't running on this computer.", provider.label()),
                }
            };
            health.update(provider, |r| r.apply_check(result, None, now));
        }
        // Cloud providers in local-only mode are not contacted.
        _ => {}
    }
    health.end_check(provider);
    notify(app);
}

/// Check `only`, or every listed provider, at once.
pub async fn check(app: &AppHandle, only: Option<ProviderId>) {
    let providers = match only {
        Some(provider) => vec![provider],
        None => {
            let llm = app.state::<LLMState>();
            let connect = app.state::<ConnectState>();
            let active = app.state::<ModelPickerState>().active(&llm);
            listed_providers(active.map(|a| a.model.provider), &llm, &connect)
        }
    };
    futures::future::join_all(providers.into_iter().map(|p| check_provider(app, p))).await;
}

/// At startup, once keys are loaded: learn which local servers and
/// subscriptions are there, then check every listed provider. Runs in the
/// background.
pub fn check_at_startup(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let connect = app.state::<ConnectState>();
        let host = ollama_host();
        tokio::join!(
            refresh_local(&app, &connect, &host),
            refresh_subscriptions(&app, &connect, false),
        );
        check(&app, None).await;
    });
}

/// A pasted key was just verified and saved: it works (no second request).
/// The caller notifies once the key is the one in use.
pub fn record_verified_key(app: &AppHandle, provider: ProviderId, key: &str) {
    let fp = fingerprint(key);
    app.state::<ConnectionHealthState>().update(provider, |r| {
        r.apply_check(KeyHealth::Works, Some(fp), now_ms())
    });
}

/// A finished answer of a session whose model is `provider`'s: record what
/// it shows about the connection, and check the provider after a rejected
/// key or a used-up quota (at most once a minute per provider).
pub fn observe_answer(app: &AppHandle, provider: Option<ProviderId>, event: &AgentEvent) {
    let AgentEvent::RunFinished {
        status,
        error,
        provider_error,
        ..
    } = event
    else {
        return;
    };
    let Some(provider) = provider else {
        return;
    };
    let failure = match (status, provider_error) {
        (RunStatus::Completed, _) => None,
        (RunStatus::Error, Some(failure)) => Some(failure),
        _ => return,
    };
    let key = match provider.kind() {
        ProviderKind::ApiKey => key_in_use(&app.state::<LLMState>(), provider).map(|(k, _)| k),
        _ => None,
    };
    let now = now_ms();
    let message = error
        .as_deref()
        .and_then(|text| answer_error_message(text, key.as_deref().unwrap_or_default()));
    let reset = error
        .as_deref()
        .and_then(|text| limit_reset_at_ms(text, now));
    let fp = key.as_deref().map(fingerprint);
    let health = app.state::<ConnectionHealthState>();
    health.update(provider, |r| {
        r.apply_answer(failure, message, fp, now);
        // A daily limit's reset, which the classifier does not turn into a wait.
        if let (Some(limit), Some(reset)) = (r.limit.as_mut(), reset) {
            if limit.reset_at_ms.is_none() && limit.from == Evidence::Answer {
                limit.reset_at_ms = Some(reset);
            }
        }
    });
    notify(app);
    let needs_check = failure.is_some_and(|f| {
        matches!(
            f.kind,
            ProviderErrorKind::Auth | ProviderErrorKind::QuotaExhausted
        )
    });
    if needs_check && health.take_auto_check(provider, now) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            check_provider(&app, provider).await;
        });
    }
}

/// The connection status the top bar shows.
#[tauri::command]
pub fn connection_status(app: AppHandle) -> ConnectionSnapshot {
    snapshot(&app)
}

/// Check `provider` (or every listed provider) now, without spending a chat
/// request, and return the result.
#[tauri::command]
pub async fn connection_check(app: AppHandle, provider: Option<ProviderId>) -> ConnectionSnapshot {
    check(&app, provider).await;
    snapshot(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_759_750_000_000;

    #[test]
    fn a_key_hint_never_shows_more_than_four_characters() {
        let key = "sk-or-v1-0123456789abcdefa3f9";
        assert_eq!(mask_key(key), "…a3f9");
        assert_eq!(mask_key(&format!("  {key}\n")), "…a3f9");
        for len in 0..80 {
            let key: String = "k7".repeat(len).chars().take(len).collect();
            let hint = mask_key(&key);
            let shown: String = hint.chars().filter(|c| *c != '…').collect();
            assert!(shown.chars().count() <= KEY_HINT_CHARS, "{len}: {hint}");
            assert!(hint.starts_with('…'));
            if len < MIN_KEY_FOR_HINT {
                assert!(shown.is_empty(), "a short key shows nothing: {hint}");
            }
        }
        // Characters, not bytes: a multi-byte key is cut on a character.
        assert_eq!(mask_key("ключ-ключ-ключ-ёжик"), "…ёжик");
        assert_eq!(mask_key(""), "…");
    }

    fn error(kind: ProviderErrorKind, status: u16, retry: Option<u32>) -> ProviderError {
        ProviderError {
            kind,
            status: Some(status),
            retry_after_secs: retry,
            upstream: None,
            resets_at_ms: None,
        }
    }

    #[test]
    fn checks_and_answers_decide_the_status_of_the_key_in_use() {
        let key = Some(fingerprint("sk-or-v1-key-one"));
        let mut r = Record::default();
        assert_eq!(r.shown(key, NOW).health, Health::Unchecked);

        r.apply_check(KeyHealth::Works, key, NOW);
        let shown = r.shown(key, NOW);
        assert_eq!(shown.health, Health::Works);
        assert_eq!(shown.checked_at_ms, Some(NOW));
        assert_eq!(shown.learned_from, Some(Evidence::Check));

        // A chat fails with the user's "401 User not found".
        r.apply_answer(
            Some(&error(ProviderErrorKind::Auth, 401, None)),
            Some("User not found.".to_string()),
            key,
            NOW + 1,
        );
        let shown = r.shown(key, NOW + 1);
        assert_eq!(shown.health, Health::Rejected);
        assert_eq!(shown.message.as_deref(), Some("User not found."));
        assert_eq!(shown.http_status, Some(401));

        // A different key (saved after "Fix key") starts unchecked.
        let other = Some(fingerprint("sk-or-v1-key-two"));
        assert_eq!(r.shown(other, NOW + 2).health, Health::Unchecked);
        r.apply_check(KeyHealth::Works, other, NOW + 2);
        assert_eq!(r.shown(other, NOW + 2).health, Health::Works);
    }

    #[test]
    fn a_limit_seen_in_an_answer_outlives_a_passing_check_until_it_resets() {
        let key = Some(fingerprint("sk-or-v1-key"));
        let mut r = Record::default();
        let reset = NOW + 3_600_000;
        r.apply_answer(
            Some(&error(ProviderErrorKind::QuotaExhausted, 429, None)),
            Some("Rate limit exceeded: free-models-per-day.".to_string()),
            key,
            NOW,
        );
        r.limit.as_mut().unwrap().reset_at_ms = Some(reset);
        // The key endpoint still accepts the key: the daily limit stays.
        r.apply_check(KeyHealth::Works, key, NOW + 1_000);
        let shown = r.shown(key, NOW + 1_000);
        assert_eq!(shown.health, Health::Limited);
        assert!(shown.quota);
        assert_eq!(shown.reset_at_ms, Some(reset));
        // Past the reset it works again.
        assert_eq!(r.shown(key, reset + 1).health, Health::Works);
        // An answer that succeeds clears a limit at once.
        r.apply_answer(
            Some(&error(ProviderErrorKind::RateLimited, 429, Some(30))),
            None,
            key,
            NOW,
        );
        let shown = r.shown(key, NOW);
        assert_eq!(shown.health, Health::Limited);
        assert!(!shown.quota);
        assert_eq!(shown.reset_at_ms, Some(NOW + 30_000));
        r.apply_answer(None, None, key, NOW + 5);
        let shown = r.shown(key, NOW + 5);
        assert_eq!(shown.health, Health::Works);
        assert_eq!(shown.learned_from, Some(Evidence::Answer));
    }

    #[test]
    fn limits_without_a_reset_time_expire_and_a_check_can_clear_its_own() {
        let key = Some(fingerprint("k-1234567890ab"));
        let mut r = Record::default();
        r.apply_answer(
            Some(&error(ProviderErrorKind::RateLimited, 429, None)),
            None,
            key,
            NOW,
        );
        assert_eq!(r.shown(key, NOW + 1).health, Health::Limited);
        assert_ne!(
            r.shown(key, NOW + RATE_LIMIT_HOLD_MS).health,
            Health::Limited
        );

        // A check that found the credit used up, then credit was added.
        let mut r = Record::default();
        r.apply_check(
            KeyHealth::Limited {
                status: 200,
                quota: true,
                message: Some("The key's credit limit is used up.".to_string()),
                reset_at_ms: None,
            },
            key,
            NOW,
        );
        assert_eq!(r.shown(key, NOW).health, Health::Limited);
        r.apply_check(KeyHealth::Works, key, NOW + 10);
        assert_eq!(r.shown(key, NOW + 10).health, Health::Works);

        // A network failure reads as unreachable, not as a bad key.
        r.apply_check(
            KeyHealth::Unreachable {
                message: "connection refused".to_string(),
            },
            key,
            NOW + 20,
        );
        assert_eq!(r.shown(key, NOW + 20).health, Health::Unreachable);
        // Model and request failures say nothing about the connection.
        let before = r.clone();
        r.apply_answer(
            Some(&error(ProviderErrorKind::ModelUnavailable, 503, None)),
            None,
            key,
            NOW + 30,
        );
        assert_eq!(r, before);
    }

    #[test]
    fn unreadable_settings_count_as_local_only() {
        assert!(local_only_from(None), "fails closed");
        let mut settings = crate::app_settings::AppSettings::default();
        settings.policy.local_only = false;
        assert!(!local_only_from(Some(settings.clone())));
        settings.policy.local_only = true;
        assert!(local_only_from(Some(settings)));
    }

    #[test]
    fn automatic_checks_after_failures_wait_a_minute() {
        let state = ConnectionHealthState::default();
        assert!(state.take_auto_check(ProviderId::OpenRouter, NOW));
        assert!(!state.take_auto_check(ProviderId::OpenRouter, NOW + 1_000));
        assert!(state.take_auto_check(ProviderId::Anthropic, NOW + 1_000));
        assert!(state.take_auto_check(ProviderId::OpenRouter, NOW + AUTO_CHECK_COOLDOWN_MS));
        assert!(state.begin_check(ProviderId::OpenRouter));
        assert!(!state.begin_check(ProviderId::OpenRouter), "one at a time");
        state.end_check(ProviderId::OpenRouter);
        assert!(state.begin_check(ProviderId::OpenRouter));
    }
}
