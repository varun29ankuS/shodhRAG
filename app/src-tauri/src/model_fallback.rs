//! The app side of the one model fallback (`shodh_rag::harness::model_fallback`).
//!
//! - **Who uses it.** Slack, Telegram, memory learning and one-shot extraction call the
//!   model through [`fallback_text_model`]: nobody is there to ask, so they fall back by
//!   themselves. Ask asks [`plan_offer`] for the model to offer after a failure (its
//!   "always fall back" setting decides whether it is used without a click), and side
//!   runs (summaries, refinements, diagram fixes) start on a healthy model
//!   ([`side_run_override`]). Every answer of the agent updates the models' health
//!   ([`observe_agent_event`]).
//! - **What it reads.** The settings (chosen fallback, pick, provider order, Local-only
//!   mode, stealth consent), the providers connected now and the cached OpenRouter list.
//!   An answer never waits for the network: OpenRouter's live availability of free
//!   candidates is fetched in the background ([`refresh_availability`]) and only read
//!   from memory.
//! - **What it records.** A fallback is audited as `model_change` (reason
//!   `fallback_automatic`, the failure kind and who called). When the user's chosen model
//!   has been failing for over an hour, one notice (desktop and Inbox) suggests choosing
//!   another, with Open going to Settings → Model.

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::harness::model_catalog::{
    build_catalog, CatalogInputs, CatalogModel, ModelRef, ProviderId,
};
use shodh_rag::harness::model_choice::{model_change_payload, ChangeReason, ModelPrefs};
use shodh_rag::harness::model_fallback::{
    availability_ids, decide, fetch_availability, resolve, AvailabilityBook, CallNeeds, CallReport,
    Candidate, CandidateModel, Clock, Decision, FallbackObserver, FallbackTextModel, Health,
    HealthBook, Origin, Plan, Planner, ResolveInputs, SystemTime, Timing, DEFAULT_BOUNDS,
    LONG_FAILURE_MS,
};
use shodh_rag::harness::model_picks::{
    model_name, provider_order, resolve_chain, table, PickTable,
};
use shodh_rag::harness::provider_error::{classify, upstream_provider, ProviderErrorKind};
use shodh_rag::harness::{stealth_allowed_by_env, AgentEvent, RunStatus};
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
use shodh_rag::llm::failure::CallContext;
use shodh_rag::llm::{GenerationConfig, ProviderFailure, SimpleExternalProvider};
use shodh_rag::research::results::{ModelReply, TextModel};
use tauri::{AppHandle, Manager};

use crate::app_settings::SettingsStore;
use crate::audit_commands::AuditState;
use crate::connect_commands::ConnectState;
use crate::llm_commands::LLMState;
use crate::model_picker_commands::{
    connected_providers, data_dir, pick_context, provider_key, ModelPickerState,
};

/// The models' health and OpenRouter's live availability, for this app session.
#[derive(Default)]
pub struct FallbackState {
    pub health: Arc<HealthBook>,
    pub availability: Arc<AvailabilityBook>,
}

/// Who makes a call (for the audit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Slack,
    Telegram,
    Learning,
    Extraction,
    Ask,
    SideRun,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Slack => "slack",
            Purpose::Telegram => "telegram",
            Purpose::Learning => "learning",
            Purpose::Extraction => "extraction",
            Purpose::Ask => "ask",
            Purpose::SideRun => "side_run",
        }
    }
}

fn now_ms() -> u64 {
    SystemTime.now_ms()
}

/// Everything a plan is resolved from, read once per call.
pub(crate) struct PlanSource {
    catalog: Vec<CatalogModel>,
    connected: Vec<ProviderId>,
    order: Vec<ProviderId>,
    prefs: ModelPrefs,
    local_only: bool,
    allow_stealth: bool,
    table: &'static PickTable,
}

impl PlanSource {
    /// Reads the settings, the connected providers and the cached model list.
    pub(crate) fn read(app: &AppHandle) -> Result<Self, String> {
        let dir = data_dir(app).map_err(|e| e.message)?;
        let settings = SettingsStore::in_dir(&dir)
            .load()
            .map_err(|e| e.to_string())?;
        let prefs = settings.models.clone().sanitized();
        let local_only = settings.policy.local_only;
        let llm = app.state::<LLMState>();
        let connect = app.state::<ConnectState>();
        let connected = connected_providers(&llm, &connect);
        let cached = app
            .state::<ModelPickerState>()
            .cached_openrouter(&app.state::<AuditState>());
        let lmstudio = connect.lmstudio();
        let catalog = build_catalog(&CatalogInputs {
            openrouter: cached.as_ref().map(|c| c.models.as_slice()),
            connected: &connected,
            lmstudio: &lmstudio.models,
            local_only,
        });
        let table = table();
        let order = provider_order(&prefs.provider_order, &table.order);
        Ok(Self {
            catalog,
            connected,
            order,
            prefs,
            local_only,
            allow_stealth: stealth_allowed_by_env(),
            table,
        })
    }

    pub(crate) fn prefs(&self) -> &ModelPrefs {
        &self.prefs
    }

    pub(crate) fn local_only(&self) -> bool {
        self.local_only
    }

    /// `model`'s display name (the catalog's, the curated table's, else its id).
    pub(crate) fn name_of(&self, model: &ModelRef) -> String {
        model_name(model, &self.catalog, self.table)
    }

    fn pick_chain(&self) -> Vec<ModelRef> {
        self.prefs
            .pick
            .map(|pick| {
                resolve_chain(
                    pick,
                    &pick_context(&self.connected, &self.order, &self.catalog, self.local_only),
                )
            })
            .unwrap_or_default()
    }

    fn inputs<'a>(
        &'a self,
        requested: &'a ModelRef,
        pick_chain: &'a [ModelRef],
        needs: CallNeeds,
    ) -> ResolveInputs<'a> {
        ResolveInputs {
            requested,
            chosen_fallback: self.prefs.fallback.as_ref(),
            pick_chain,
            catalog: &self.catalog,
            table: self.table,
            order: &self.order,
            connected: &self.connected,
            local_only: self.local_only,
            allow_stealth: self.allow_stealth,
            stealth_accepted: &self.prefs.stealth_accepted,
            allow_paid: self.prefs.allow_paid_backups,
            needs,
        }
    }

    /// The plan for a call to `requested` that needs `needs`; starts a background
    /// refresh of the free candidates' availability.
    pub(crate) fn plan(&self, app: &AppHandle, requested: &ModelRef, needs: CallNeeds) -> Plan {
        let state = app.state::<FallbackState>();
        let chain = self.pick_chain();
        let inputs = self.inputs(requested, &chain, needs);
        refresh_availability(app, availability_ids(&inputs));
        resolve(&inputs, &state.health, &state.availability, now_ms())
    }
}

/// Fetches OpenRouter's availability of `ids` in the background (those not fetched in
/// the last ten minutes), one after another. Only the model ids are sent.
pub(crate) fn refresh_availability(app: &AppHandle, ids: Vec<String>) {
    if ids.is_empty() {
        return;
    }
    let books = app.state::<FallbackState>().availability.clone();
    let due = books.claim_due(&ids, now_ms());
    if due.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        for id in due {
            match fetch_availability(&id).await {
                Ok(availability) => books.put(&id, availability, now_ms()),
                Err(e) => {
                    tracing::debug!(target: "shodh::llm", model = %id, error = %e, "model availability not fetched")
                }
            }
        }
    });
}

// ── Direct calls (Slack, Telegram, learning, extraction) ───────────

/// Plans each call from the settings of the moment.
struct AppPlanner {
    app: AppHandle,
    requested: ModelRef,
}

impl Planner for AppPlanner {
    fn plan(&self, needs: CallNeeds) -> Plan {
        match PlanSource::read(&self.app) {
            Ok(source) => source.plan(&self.app, &self.requested, needs),
            Err(e) => {
                tracing::warn!(target: "shodh::llm", error = %e, "fallback settings unreadable; only the requested model is tried");
                let state = self.app.state::<FallbackState>();
                Plan::only(Candidate {
                    model: self.requested.clone(),
                    name: self.requested.model.clone(),
                    origin: Origin::Requested,
                    context_length: None,
                    privacy: shodh_rag::harness::model_catalog::PrivacyNote::ProviderTerms,
                    free: self.requested.model.ends_with(":free"),
                    paid: self.requested.provider.needs_key()
                        && !self.requested.model.ends_with(":free"),
                    health: state.health.health(&self.requested, now_ms()),
                    availability: None,
                    reachable: true,
                })
            }
        }
    }
}

/// The plain generation settings of a one-shot call: the configured ones, with no
/// creativity (extraction and short answers) and `max_tokens`.
pub(crate) fn plain_generation(llm: &LLMState, max_tokens: usize) -> GenerationConfig {
    let config = llm.config.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut generation = GenerationConfig::from(&config);
    generation.max_tokens = max_tokens;
    generation.reasoning = None;
    generation.temperature = 0.0;
    generation
}

/// One HTTP call to one candidate, with that provider's own key.
struct AppCandidates {
    app: AppHandle,
}

fn failure_of(candidate: &Candidate, kind: ProviderErrorKind, detail: &str) -> ProviderFailure {
    CallContext::new(
        candidate.model.provider.label(),
        candidate.model.model.clone(),
        String::new(),
    )
    .failure(kind, None, detail)
}

#[async_trait::async_trait]
impl CandidateModel for AppCandidates {
    async fn complete(
        &self,
        candidate: &Candidate,
        prompt: &str,
        max_tokens: usize,
    ) -> Result<ModelReply, ProviderFailure> {
        let provider = candidate.model.provider;
        // Re-checked per call: Local-only mode may have been turned on since.
        let local_only = data_dir(&self.app)
            .ok()
            .and_then(|dir| SettingsStore::in_dir(&dir).load().ok())
            .map(|s| s.policy.local_only)
            .unwrap_or(true);
        if local_only && !provider.is_local() {
            return Err(failure_of(
                candidate,
                ProviderErrorKind::InvalidRequest,
                "Local-only mode is on: a model off this computer is not used",
            ));
        }
        let llm = self.app.state::<LLMState>();
        let key = if provider.needs_key() {
            provider_key(&llm, provider).ok_or_else(|| {
                failure_of(
                    candidate,
                    ProviderErrorKind::Auth,
                    &format!("{} has no API key", provider.label()),
                )
            })?
        } else {
            match provider {
                ProviderId::Ollama => "ollama".to_string(),
                ProviderId::LmStudio => "lm-studio".to_string(),
                _ => String::new(),
            }
        };
        let generation = plain_generation(&llm, max_tokens);
        let client = SimpleExternalProvider::new(
            provider.api_provider(),
            key.clone(),
            candidate.model.model.clone(),
        )
        .map_err(|e| failure_of(candidate, ProviderErrorKind::Other, &e.to_string()))?;
        match client.generate_bounded(prompt, &generation).await {
            Ok(completion) => Ok(ModelReply {
                text: completion.text,
                truncated: completion.truncated,
            }),
            Err(e) => Err(match e.downcast::<ProviderFailure>() {
                Ok(failure) => failure,
                Err(other) => {
                    let text = format!("{other:#}");
                    let kind = classify(&text, None, now_ms()).kind;
                    CallContext::new(provider.label(), candidate.model.model.clone(), key)
                        .failure(kind, None, &text)
                }
            }),
        }
    }
}

/// Audits fallbacks and watches the user's chosen model.
struct AppObserver {
    app: AppHandle,
    purpose: Purpose,
}

impl FallbackObserver for AppObserver {
    fn observe(&self, report: &CallReport) {
        let requested = match report {
            CallReport::Answered(by) => {
                if by.fell_back() {
                    let failure = by.switches.first().and_then(|s| s.reason.kind());
                    audit_fallback(&self.app, &by.requested, &by.model, failure, self.purpose);
                } else {
                    take_back_notice(&self.app, &by.requested);
                }
                by.requested.clone()
            }
            CallReport::Failed { plan, .. } => plan.requested.model.clone(),
        };
        notice_long_failure(&self.app, &requested);
    }
}

/// A `model_change` audit record for a fallback (ids only, never a key).
pub(crate) fn audit_fallback(
    app: &AppHandle,
    from: &ModelRef,
    to: &ModelRef,
    failure: Option<ProviderErrorKind>,
    purpose: Purpose,
) {
    let failure = failure.map(kind_str);
    let mut payload = model_change_payload(
        Some(from),
        to,
        ChangeReason::FallbackAutomatic,
        failure.as_deref(),
    );
    if let serde_json::Value::Object(map) = &mut payload {
        map.insert("caller".into(), json!(purpose.as_str()));
    }
    app.state::<AuditState>()
        .record(AuditRecord::new(AuditEventType::ModelChange, payload));
}

/// `rate_limited`, `model_unavailable`, … (the kind's serialized name).
pub(crate) fn kind_str(kind: ProviderErrorKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "other".to_string())
}

/// The model that answers `purpose`'s direct calls: the configured `requested` model
/// with the app's fallback.
pub(crate) fn fallback_text_model(
    app: &AppHandle,
    requested: ModelRef,
    purpose: Purpose,
) -> FallbackTextModel {
    let state = app.state::<FallbackState>();
    FallbackTextModel::new(
        Arc::new(AppPlanner {
            app: app.clone(),
            requested,
        }),
        Arc::new(AppCandidates { app: app.clone() }),
        state.health.clone(),
        Timing {
            clock: Arc::new(SystemTime),
            pause: Arc::new(SystemTime),
            bounds: DEFAULT_BOUNDS,
        },
        Some(Arc::new(AppObserver {
            app: app.clone(),
            purpose,
        })),
    )
}

/// `TextModel` for `purpose` (see [`fallback_text_model`]).
pub(crate) fn text_model(
    app: &AppHandle,
    requested: ModelRef,
    purpose: Purpose,
) -> Arc<dyn TextModel> {
    Arc::new(fallback_text_model(app, requested, purpose))
}

// ── The agent (Ask and side runs) ──────────────────────────────────

/// The picker model of an agent session's mode.
pub(crate) fn session_model(mode: &shodh_rag::llm::LLMMode) -> Option<ModelRef> {
    match mode {
        shodh_rag::llm::LLMMode::External {
            provider, model, ..
        } => ProviderId::from_api(provider).map(|p| ModelRef::new(p, model.trim())),
        _ => None,
    }
}

/// An agent run of `model` finished: its health follows (a completed answer clears it,
/// a provider failure that says the model is down counts against it).
pub(crate) fn observe_agent_event(app: &AppHandle, model: Option<&ModelRef>, event: &AgentEvent) {
    let (
        Some(model),
        AgentEvent::RunFinished {
            status,
            provider_error,
            ..
        },
    ) = (model, event)
    else {
        return;
    };
    let Some(state) = app.try_state::<FallbackState>() else {
        return;
    };
    match (status, provider_error) {
        (RunStatus::Completed, _) => {
            state.health.record_success(model);
            take_back_notice(app, model);
        }
        (RunStatus::Error, Some(error)) => {
            state.health.record_failure(model, error.kind, now_ms());
            notice_long_failure(app, model);
        }
        _ => {}
    }
}

/// The model a side run (summary, refinement, diagram fix) starts with instead of the
/// configured one, when that one is skipped for failing moments ago: the first healthy
/// candidate of its plan. Nobody is asked: side runs fall back by themselves.
pub(crate) fn side_run_override(
    app: &AppHandle,
    configured: Option<&ModelRef>,
) -> Option<(ModelRef, Option<ProviderErrorKind>)> {
    let configured = configured?;
    let state = app.try_state::<FallbackState>()?;
    let health = state.health.health(configured, now_ms());
    if matches!(health, Health::Healthy | Health::Retrying { .. }) {
        return None;
    }
    let source = PlanSource::read(app).ok()?;
    let plan = source.plan(app, configured, CallNeeds::agent());
    plan.fallbacks
        .iter()
        .find(|c| matches!(c.health, Health::Healthy | Health::Retrying { .. }))
        .map(|c| (c.model.clone(), health.last_failure()))
}

/// What a main Ask run starts on, from its model's health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MainStart {
    /// The configured model: healthy, or due for its one retry (this run is that retry).
    Configured,
    /// The configured model failed moments ago (or another call is retrying it now): the
    /// run does not call it again.
    Cooling(ProviderErrorKind),
}

pub(crate) fn main_start(health: Health) -> MainStart {
    if health.usable() {
        MainStart::Configured
    } else {
        MainStart::Cooling(
            health
                .last_failure()
                .unwrap_or(ProviderErrorKind::ModelUnavailable),
        )
    }
}

/// The configured model and why, when a main Ask run must not start on it now.
pub(crate) fn main_run_cooling(
    app: &AppHandle,
    configured: Option<&ModelRef>,
) -> Option<(ModelRef, ProviderErrorKind)> {
    let configured = configured?;
    let state = app.try_state::<FallbackState>()?;
    match main_start(state.health.health(configured, now_ms())) {
        MainStart::Configured => None,
        MainStart::Cooling(kind) => Some((configured.clone(), kind)),
    }
}

/// What Ask says when its model failed moments ago (the card's details).
pub(crate) fn cooling_message(kind: ProviderErrorKind) -> String {
    let why = match kind {
        ProviderErrorKind::Timeout => "did not answer in time",
        ProviderErrorKind::Network => "could not be reached",
        ProviderErrorKind::Auth => "refused the API key",
        _ => "was unavailable",
    };
    format!("The model {why} moments ago, so Shodh did not call it again yet; it is tried again after a short wait.")
}

/// The model Ask offers after `failed` failed with `kind` (`detail` is the provider's
/// message), skipping the models this answer already `tried`. `None` when no other
/// model can help with this failure.
pub(crate) fn plan_offer(
    app: &AppHandle,
    source: &PlanSource,
    failed: &ModelRef,
    kind: Option<ProviderErrorKind>,
    detail: Option<&str>,
    tried: &[ModelRef],
) -> Option<Candidate> {
    let plan = source.plan(app, failed, CallNeeds::agent());
    let eligible = match kind {
        Some(kind) => {
            let detail = detail.unwrap_or_default();
            let failure = ProviderFailure {
                kind,
                provider: failed.provider.label().to_string(),
                upstream: upstream_provider(detail),
                model: failed.model.clone(),
                status: None,
                detail: detail.to_string(),
            };
            match decide(&failure, &plan.requested, now_ms()) {
                Decision::Stop => return None,
                Decision::Switch { eligible, .. } => Some(eligible),
            }
        }
        None => None,
    };
    plan.fallbacks
        .into_iter()
        .filter(|c| !tried.contains(&c.model))
        .filter(|c| eligible.as_ref().is_none_or(|e| e.allows(c)))
        .find(|c| {
            !matches!(
                c.health,
                Health::CoolingDown { .. } | Health::Probing { .. }
            )
        })
}

// ── Notices ────────────────────────────────────────────────────────

/// The Inbox item of a chosen model that keeps failing.
fn long_failure_item(model: &ModelRef, name: &str) -> NewInboxItem {
    NewInboxItem {
        id: long_failure_id(model),
        kind: InboxKind::Model,
        status: InboxStatus::Failed,
        title: format!("{name} has been failing for over an hour"),
        detail: Some(
            "Shodh answered with other models where it could. Choose another model in Settings → Model.".to_string(),
        ),
        link: Some(InboxLink::new(
            "settings",
            Some(json!({ "kind": "settings", "section": "models" })),
        )),
        data: json!({ "provider": model.provider.as_str(), "model": model.model }),
    }
}

fn long_failure_id(model: &ModelRef) -> String {
    format!("model:failing:{}:{}", model.provider.as_str(), model.model)
}

/// The model answered again after its long-failure notice: the Inbox item goes.
fn take_back_notice(app: &AppHandle, model: &ModelRef) {
    let Some(state) = app.try_state::<FallbackState>() else {
        return;
    };
    if !state.health.take_recovered(model) {
        return;
    }
    let app = app.clone();
    let id = long_failure_id(model);
    tauri::async_runtime::spawn(async move {
        crate::inbox_commands::resolve(&app, id).await;
    });
}

/// One notice when the user's chosen model has been failing for over an hour.
fn notice_long_failure(app: &AppHandle, model: &ModelRef) {
    let Some(state) = app.try_state::<FallbackState>() else {
        return;
    };
    // Cheap first: a model that is not failing needs no settings read.
    let failing_long = state
        .health
        .failing_since(model)
        .is_some_and(|since| now_ms().saturating_sub(since) >= LONG_FAILURE_MS);
    if !failing_long {
        return;
    }
    let source = PlanSource::read(app).ok();
    let chosen = source
        .as_ref()
        .and_then(|s| s.prefs().chosen.clone())
        .or_else(|| crate::model_picker_commands::configured_model(&app.state::<LLMState>()));
    if chosen.as_ref() != Some(model) || !state.health.take_long_failure(model, now_ms()) {
        return;
    }
    let name = source
        .as_ref()
        .map(|s| s.name_of(model))
        .unwrap_or_else(|| model.model.clone());
    let item = long_failure_item(model, &name);
    crate::notifications::problem(
        app,
        item.id.clone(),
        item.title.clone(),
        "Open Settings → Model to choose another model.",
    );
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::inbox_commands::post(&app, item).await;
    });
}

// ── Settings: the chain used now ───────────────────────────────────

/// The fallback chain used now, for the configured model.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainView {
    /// The model calls ask for, with the models tried after it (Ask's chain).
    pub plan: Option<Plan>,
    /// The chain of connectors and background tasks (direct calls: no subscriptions).
    pub direct: Option<Plan>,
    pub always_fall_back: bool,
    /// A free model may fall back to a model billed per token.
    pub allow_paid_backups: bool,
    pub local_only: bool,
    pub now_ms: u64,
}

/// The fallback chains used now, with each model's health and uptime.
#[tauri::command]
pub async fn model_fallback_chain(app: AppHandle) -> Result<ChainView, String> {
    // Reads the settings file and the model-list cache: off the async workers.
    tauri::async_runtime::spawn_blocking(move || chain_view(&app))
        .await
        .map_err(|e| e.to_string())?
}

fn chain_view(app: &AppHandle) -> Result<ChainView, String> {
    let app = app.clone();
    let source = PlanSource::read(&app)?;
    let configured = crate::model_picker_commands::configured_model(&app.state::<LLMState>());
    let plan = configured
        .as_ref()
        .map(|m| source.plan(&app, m, CallNeeds::agent()));
    let direct = configured.as_ref().map(|m| {
        source.plan(
            &app,
            m,
            CallNeeds {
                prompt_tokens: None,
                ..CallNeeds::direct("")
            },
        )
    });
    Ok(ChainView {
        plan,
        direct,
        always_fall_back: source.prefs().always_fall_back,
        allow_paid_backups: source.prefs().allow_paid_backups,
        local_only: source.local_only(),
        now_ms: now_ms(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_long_failure_item_opens_model_settings_and_names_the_model() {
        let model = ModelRef::new(
            ProviderId::OpenRouter,
            "nvidia/nemotron-3-super-120b-a12b:free",
        );
        let item = long_failure_item(&model, "Nemotron Super");
        assert_eq!(item.kind, InboxKind::Model);
        assert_eq!(
            item.title,
            "Nemotron Super has been failing for over an hour"
        );
        let link = item.link.unwrap();
        assert_eq!(link.view, "settings");
        assert_eq!(link.target.unwrap()["section"], "models");
        assert_eq!(
            item.id, "model:failing:openrouter:nvidia/nemotron-3-super-120b-a12b:free",
            "one item per model: a later notice replaces it"
        );
    }

    #[test]
    fn a_main_ask_run_never_starts_on_a_model_that_just_failed() {
        let kind = ProviderErrorKind::ModelUnavailable;
        assert_eq!(main_start(Health::Healthy), MainStart::Configured);
        assert_eq!(
            main_start(Health::Retrying {
                failures: 1,
                last: kind
            }),
            MainStart::Configured,
            "the wait is over: this run is the one retry"
        );
        assert_eq!(
            main_start(Health::CoolingDown {
                until_ms: 1,
                failures: 1,
                last: kind
            }),
            MainStart::Cooling(kind)
        );
        assert_eq!(
            main_start(Health::Probing {
                failures: 2,
                last: ProviderErrorKind::Timeout
            }),
            MainStart::Cooling(ProviderErrorKind::Timeout),
            "another call is retrying it: no second probe"
        );
        assert!(cooling_message(kind).contains("unavailable"));
    }

    #[test]
    fn failure_kinds_are_written_as_the_ui_names_them() {
        assert_eq!(
            kind_str(ProviderErrorKind::ModelUnavailable),
            "model_unavailable"
        );
        assert_eq!(kind_str(ProviderErrorKind::RateLimited), "rate_limited");
    }

    #[test]
    fn a_session_model_is_its_picker_entry() {
        let mode = shodh_rag::llm::LLMMode::External {
            provider: shodh_rag::llm::ApiProvider::Subscription(
                shodh_rag::llm::SubscriptionService::Claude,
            ),
            api_key: String::new(),
            model: "claude-opus-5-5".into(),
        };
        assert_eq!(
            session_model(&mode),
            Some(ModelRef::new(ProviderId::ClaudeSub, "claude-opus-5-5"))
        );
        assert_eq!(session_model(&shodh_rag::llm::LLMMode::Disabled), None);
    }
}
