//! The model picker: which models can answer, which one answers next, and
//! the fallback offered when a model is rate-limited or unavailable.
//!
//! - The list combines OpenRouter's public model list (cached in `shodh.db`
//!   with a TTL, so prices show offline), the curated models of connected
//!   key and subscription providers (`model_picks.json`) and the models
//!   loaded in LM Studio. Models without tool calling are left out: the
//!   agent needs tools. Ollama's models are reported but cannot be chosen.
//! - The four picks (Best quality, Fast & cheap, Free, Private) resolve
//!   from the connected providers; a pick's list is its automatic fallback
//!   order.
//! - A selection is saved in the settings file ([`ModelPrefs`]; keys stay in
//!   the OS keychain) and used from the next answer: the agent session of a
//!   conversation restarts with the new model when it next starts, never in
//!   the middle of an answer.
//! - `SHODH_LLM_PROVIDER` / `SHODH_LLM_MODEL` set the model at startup. The
//!   picker shows that and can override it for this session only.
//! - Every switch is audited as `model_change` (from, to, reason; no keys).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::harness::catalog_cache::{CachedCatalog, CatalogCache, CATALOG_TTL};
use shodh_rag::harness::catalog_fetch::fetch_openrouter;
use shodh_rag::harness::model::OLLAMA_DEFAULT_HOST;
use shodh_rag::harness::model_catalog::{
    build_catalog, choose_fallback, CatalogInputs, CatalogModel, FallbackContext, ModelRef,
    ProviderId,
};
use shodh_rag::harness::model_choice::{
    check_selection, model_change_payload, resolve_active, ActiveModel, ChangeReason, ModelPrefs,
    ModelSource, Selection, SelectionContext,
};
use shodh_rag::harness::model_picks::{
    model_name, next_in_chain, provider_order, resolve_chain, table, Pick, PickContext,
};
use shodh_rag::harness::stealth_allowed_by_env;
use shodh_rag::llm::{ApiProvider, LLMMode};
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex as AsyncMutex;

use crate::app_settings::{broadcast, SettingsStore};
use crate::audit_commands::AuditState;
use crate::connect_commands::{
    key_statuses, refresh_local, refresh_subscriptions, ConnectState, KeyStatus, LocalServer,
    SubscriptionStatus, OLLAMA_NOTE,
};
use crate::llm_commands::{activate_mode, LLMState};

/// Cache key of OpenRouter's list in `model_catalog_cache`.
const OPENROUTER_SOURCE: &str = "openrouter";

/// Typed failure of a picker command, for the UI.
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct PickerError {
    /// `invalid`, `local_only`, `missing_key`, `not_connected`,
    /// `stealth_confirmation`, `environment_set` or `failed`.
    pub code: &'static str,
    pub message: String,
}

impl PickerError {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            code: "failed",
            message: message.into(),
        }
    }
}

impl From<shodh_rag::harness::model_choice::SelectionError> for PickerError {
    fn from(e: shodh_rag::harness::model_choice::SelectionError) -> Self {
        use shodh_rag::harness::model_choice::SelectionError as E;
        let code = match &e {
            E::Invalid(_) => "invalid",
            E::LocalOnly(_) => "local_only",
            E::MissingKey(_) => "missing_key",
            E::NotConnected(_) => "not_connected",
            E::Unsupported => "invalid",
            E::StealthNeedsConfirmation(_) => "stealth_confirmation",
            E::EnvironmentSet(_) => "environment_set",
        };
        Self {
            code,
            message: e.to_string(),
        }
    }
}

type PickerResult<T> = Result<T, PickerError>;

/// Process-wide picker state: what the environment set at startup, this
/// session's override and the open model-list cache.
#[derive(Default)]
pub struct ModelPickerState {
    environment: Mutex<Option<ModelRef>>,
    session_override: Mutex<Option<ModelRef>>,
    cache: Mutex<Option<Arc<CatalogCache>>>,
    /// One model-list refresh at a time.
    refresh: AsyncMutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ModelPickerState {
    fn set_environment(&self, model: Option<ModelRef>) {
        *lock(&self.environment) = model;
    }

    fn environment(&self) -> Option<ModelRef> {
        lock(&self.environment).clone()
    }

    fn session_override(&self) -> Option<ModelRef> {
        lock(&self.session_override).clone()
    }

    fn set_session_override(&self, model: Option<ModelRef>) {
        *lock(&self.session_override) = model;
    }

    /// The model the next answer uses (the app's configured model) and where
    /// that choice comes from. `None` when no picker model is configured
    /// (no model, or the llama.cpp file).
    pub fn active(&self, llm: &LLMState) -> Option<ActiveModel> {
        let model = configured_model(llm)?;
        let source = if self.session_override().as_ref() == Some(&model) {
            ModelSource::Session
        } else if self.environment().as_ref() == Some(&model) {
            ModelSource::Environment
        } else {
            ModelSource::Settings
        };
        Some(ActiveModel { model, source })
    }

    fn cache(&self, audit: &AuditState) -> Option<Arc<CatalogCache>> {
        let mut slot = lock(&self.cache);
        if slot.is_none() {
            let (path, key) = audit.database()?;
            match CatalogCache::open(&path, key.as_ref()) {
                Ok(cache) => *slot = Some(Arc::new(cache)),
                Err(e) => {
                    tracing::warn!("Model list cache unavailable: {e}");
                    return None;
                }
            }
        }
        slot.clone()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn data_dir(app: &AppHandle) -> PickerResult<PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|e| PickerError::failed(format!("The app data folder is unavailable: {e}")))
}

/// Provider key environment variables (they are used without being saved).
fn env_key_vars(provider: ProviderId) -> &'static [&'static str] {
    match provider {
        ProviderId::OpenRouter => &["OPENROUTER_API_KEY"],
        ProviderId::Anthropic => &["ANTHROPIC_API_KEY"],
        ProviderId::OpenAI => &["OPENAI_API_KEY"],
        ProviderId::Google => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        ProviderId::Grok => &["XAI_API_KEY"],
        _ => &[],
    }
}

/// The key for `provider`: environment first, then the keys loaded from the
/// OS keychain. Never logged.
fn provider_key(llm: &LLMState, provider: ProviderId) -> Option<String> {
    let from_env = env_key_vars(provider).iter().find_map(|var| {
        std::env::var(var)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    });
    from_env.or_else(|| {
        lock(&llm.api_keys)
            .get(provider.as_str())
            .filter(|k| !k.trim().is_empty())
    })
}

/// Providers that have a key.
fn keyed_providers(llm: &LLMState) -> Vec<ProviderId> {
    ProviderId::KEYED
        .into_iter()
        .filter(|p| provider_key(llm, *p).is_some())
        .collect()
}

/// Providers that can answer now: a key, a signed-in subscription, LM
/// Studio running (as last detected).
pub(crate) fn connected_providers(llm: &LLMState, connect: &ConnectState) -> Vec<ProviderId> {
    let mut out = keyed_providers(llm);
    out.extend(connect.signed_in());
    if connect.lmstudio().running {
        out.push(ProviderId::LmStudio);
    }
    out
}

fn ollama_host() -> String {
    std::env::var("OLLAMA_HOST")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .map(|h| {
            if h.contains("://") {
                h
            } else {
                format!("http://{h}")
            }
        })
        .unwrap_or_else(|| OLLAMA_DEFAULT_HOST.to_string())
}

/// The model the app's LLM is configured with right now, as a picker entry.
fn configured_model(llm: &LLMState) -> Option<ModelRef> {
    match &lock(&llm.config).mode {
        LLMMode::External {
            provider, model, ..
        } => ProviderId::from_api(provider).map(|p| ModelRef::new(p, model.clone())),
        _ => None,
    }
}

/// The LLM mode for `model`, with its key (subscriptions and local servers need none).
fn mode_for(llm: &LLMState, model: &ModelRef) -> PickerResult<LLMMode> {
    let api_key = if model.provider.needs_key() {
        provider_key(llm, model.provider).ok_or_else(|| PickerError {
            code: "missing_key",
            message: format!(
                "{} has no API key. Add one in Settings → Model.",
                model.provider.label()
            ),
        })?
    } else {
        match model.provider {
            ProviderId::Ollama => "ollama".to_string(),
            ProviderId::LmStudio => "lm-studio".to_string(),
            _ => String::new(),
        }
    };
    Ok(LLMMode::External {
        provider: model.provider.api_provider(),
        api_key,
        model: model.model.clone(),
    })
}

/// Use `model` for the next answers (the agent sessions pick it up when they
/// next start; a running answer is never interrupted).
async fn apply(llm: &LLMState, model: &ModelRef) -> PickerResult<()> {
    let mode = mode_for(llm, model)?;
    activate_mode(llm, mode)
        .await
        .map(|_| ())
        .map_err(PickerError::failed)
}

/// Where the OpenRouter list in the view came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogStatus {
    /// Fetched within the TTL.
    Fresh,
    /// An older copy: the refresh failed (offline) or was not allowed.
    Cached,
    /// No copy at all: prices are unknown.
    Unavailable,
    /// Local-only mode: the list is not fetched (nothing leaves this computer).
    LocalOnly,
}

/// Everything the picker shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerView {
    pub active: Option<ActiveModel>,
    pub environment: Option<ModelRef>,
    pub models: Vec<CatalogModel>,
    pub catalog_status: CatalogStatus,
    pub catalog_fetched_at_ms: Option<u64>,
    /// Why the list could not be refreshed, when it could not.
    pub catalog_error: Option<String>,
    /// Ollama answered on this computer.
    pub ollama_running: bool,
    /// File name of the llama.cpp model, when one is configured. The agent
    /// cannot use it (it needs tool calling through a runtime); shown so the
    /// person knows why it is not offered.
    pub llama_cpp_file: Option<String>,
    pub keyed: Vec<ProviderId>,
    /// Providers that can answer now (keys, signed-in subscriptions, LM Studio running).
    pub connected: Vec<ProviderId>,
    pub local_only: bool,
    /// `SHODH_ALLOW_STEALTH_MODELS=1` is set.
    pub env_allows_stealth: bool,
    pub prefs: ModelPrefs,
    /// The four picks, resolved from the connected providers.
    pub picks: Vec<PickOption>,
    pub subscriptions: Vec<SubscriptionStatus>,
    pub keys: Vec<KeyStatus>,
    pub lmstudio: LocalServer,
    pub ollama: LocalServer,
    /// Why Ollama's models cannot be chosen.
    pub ollama_note: &'static str,
    /// The fallback order of providers (saved order first, then the default).
    pub provider_order: Vec<ProviderId>,
}

/// One of the four picks, resolved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickOption {
    pub pick: Pick,
    pub label: &'static str,
    /// The model that answers for this pick, `None` when no connected
    /// provider offers it.
    pub model: Option<ModelRef>,
    pub name: Option<String>,
    /// The models tried after it, in order.
    pub fallbacks: Vec<ModelRef>,
}

/// The pick chains for `catalog` and the connection state.
fn pick_context<'a>(
    connected: &'a [ProviderId],
    order: &'a [ProviderId],
    catalog: &'a [CatalogModel],
    local_only: bool,
) -> PickContext<'a> {
    PickContext {
        table: table(),
        connected,
        order,
        catalog,
        local_only,
    }
}

fn pick_options(ctx: &PickContext<'_>) -> Vec<PickOption> {
    Pick::ALL
        .into_iter()
        .map(|pick| {
            let mut chain = resolve_chain(pick, ctx);
            let model = (!chain.is_empty()).then(|| chain.remove(0));
            PickOption {
                pick,
                label: pick.label(),
                name: model
                    .as_ref()
                    .map(|m| model_name(m, ctx.catalog, ctx.table)),
                model,
                fallbacks: chain,
            }
        })
        .collect()
}

async fn openrouter_list(
    picker: &ModelPickerState,
    audit: &AuditState,
    local_only: bool,
    force: bool,
) -> (Option<CachedCatalog>, CatalogStatus, Option<String>) {
    let cache = picker.cache(audit);
    let cached = cache.as_ref().and_then(|c| match c.get(OPENROUTER_SOURCE) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Cached model list unreadable: {e}");
            None
        }
    });
    if local_only {
        return (cached, CatalogStatus::LocalOnly, None);
    }
    let now = now_ms();
    if !force {
        if let Some(c) = cached.as_ref().filter(|c| c.is_fresh(now, CATALOG_TTL)) {
            return (Some(c.clone()), CatalogStatus::Fresh, None);
        }
    }
    let _refreshing = picker.refresh.lock().await;
    // Another request may have refreshed the list while this one waited.
    if !force {
        let latest = cache
            .as_ref()
            .and_then(|c| c.get(OPENROUTER_SOURCE).ok().flatten())
            .filter(|c| c.is_fresh(now_ms(), CATALOG_TTL));
        if let Some(latest) = latest {
            return (Some(latest), CatalogStatus::Fresh, None);
        }
    }
    match fetch_openrouter().await {
        Ok(models) => {
            let fetched = CachedCatalog {
                models,
                fetched_at_ms: now,
            };
            if let Some(cache) = &cache {
                if let Err(e) = cache.put(OPENROUTER_SOURCE, &fetched.models, now) {
                    tracing::warn!("Model list not cached: {e}");
                }
            }
            (Some(fetched), CatalogStatus::Fresh, None)
        }
        Err(e) => {
            let status = if cached.is_some() {
                CatalogStatus::Cached
            } else {
                CatalogStatus::Unavailable
            };
            (cached, status, Some(e.to_string()))
        }
    }
}

async fn build_view(
    app: &AppHandle,
    picker: &ModelPickerState,
    llm: &LLMState,
    audit: &AuditState,
    force_refresh: bool,
) -> PickerResult<PickerView> {
    let settings = SettingsStore::in_dir(&data_dir(app)?)
        .load()
        .map_err(|e| PickerError::failed(e.to_string()))?;
    let prefs = settings.models.clone().sanitized();
    let local_only = settings.policy.local_only;
    let keyed = keyed_providers(llm);
    let connect = app.state::<ConnectState>();

    let host = ollama_host();
    let (openrouter, (), ()) = tokio::join!(
        openrouter_list(picker, audit, local_only, force_refresh),
        refresh_local(app, &connect, &host),
        refresh_subscriptions(app, &connect, force_refresh),
    );
    let (list, catalog_status, catalog_error) = openrouter;
    let lmstudio = connect.lmstudio();
    let ollama = connect.ollama();
    let connected = connected_providers(llm, &connect);
    let models = build_catalog(&CatalogInputs {
        openrouter: list.as_ref().map(|c| c.models.as_slice()),
        connected: &connected,
        lmstudio: &lmstudio.models,
        local_only,
    });
    let order = provider_order(&prefs.provider_order, &table().order);
    let picks = pick_options(&pick_context(&connected, &order, &models, local_only));
    let llama_cpp_file = lock(&llm.custom_model_path)
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned());

    Ok(PickerView {
        active: picker.active(llm),
        environment: picker.environment(),
        models,
        catalog_status,
        catalog_fetched_at_ms: list.map(|c| c.fetched_at_ms),
        catalog_error,
        ollama_running: ollama.running,
        llama_cpp_file,
        keyed,
        connected,
        local_only,
        env_allows_stealth: stealth_allowed_by_env(),
        prefs,
        picks,
        subscriptions: connect.subscription_statuses(),
        keys: key_statuses(llm),
        lmstudio,
        ollama,
        ollama_note: OLLAMA_NOTE,
        provider_order: order,
    })
}

/// The picker's contents. `refresh` fetches the model list even when the
/// cached copy is still fresh.
#[tauri::command]
pub async fn model_picker_view(
    app: AppHandle,
    refresh: Option<bool>,
    picker: State<'_, ModelPickerState>,
    llm: State<'_, LLMState>,
    audit: State<'_, AuditState>,
) -> PickerResult<PickerView> {
    build_view(&app, &picker, &llm, &audit, refresh.unwrap_or(false)).await
}

/// A model chosen in the picker.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectRequest {
    pub model: ModelRef,
    /// The person confirmed the stealth-model warning for this model.
    #[serde(default)]
    pub confirm_stealth: bool,
    /// The person chose to override the environment's model for this session.
    #[serde(default)]
    pub session_override: bool,
}

/// Use `request.model` from the next answer. Saves it as the choice, or (when
/// the environment sets the model) overrides that for this session.
#[tauri::command]
pub async fn model_select(
    app: AppHandle,
    request: SelectRequest,
    picker: State<'_, ModelPickerState>,
    llm: State<'_, LLMState>,
    audit: State<'_, AuditState>,
) -> PickerResult<PickerView> {
    select_model(
        &app,
        &picker,
        &llm,
        &audit,
        request.model,
        None,
        request.confirm_stealth,
        request.session_override,
    )
    .await?;
    build_view(&app, &picker, &llm, &audit, false).await
}

/// Check, apply and save a selection. `pick`: the pick it came from
/// (`None`: chosen by id).
#[allow(clippy::too_many_arguments)]
async fn select_model(
    app: &AppHandle,
    picker: &ModelPickerState,
    llm: &LLMState,
    audit: &AuditState,
    requested: ModelRef,
    pick: Option<Pick>,
    confirm_stealth: bool,
    session_override: bool,
) -> PickerResult<ModelRef> {
    let store = SettingsStore::in_dir(&data_dir(app)?);
    let settings = store
        .load()
        .map_err(|e| PickerError::failed(e.to_string()))?;
    let prefs = settings.models.clone().sanitized();
    let connected = connected_providers(llm, &app.state::<ConnectState>());
    let environment = picker.environment();
    let (model, selection) = check_selection(
        requested,
        &SelectionContext {
            prefs: &prefs,
            connected: &connected,
            local_only: settings.policy.local_only,
            env_allows_stealth: stealth_allowed_by_env(),
            environment: environment.as_ref(),
        },
        confirm_stealth,
        session_override,
    )?;
    let from = configured_model(llm);

    apply(llm, &model).await?;

    let reason = match selection {
        Selection::Saved => ChangeReason::UserChoice,
        Selection::SessionOverride => {
            // Re-selecting the environment's model ends the override.
            let override_model = (environment.as_ref() != Some(&model)).then(|| model.clone());
            picker.set_session_override(override_model);
            ChangeReason::SessionOverride
        }
    };
    let confirmed_stealth = confirm_stealth && model.is_stealth();
    let (saved, _) = store
        .update(|s| {
            let mut prefs = std::mem::take(&mut s.models).sanitized();
            if selection == Selection::Saved {
                prefs.chosen = Some(model.clone());
                // A pick falls back down its own list without asking
                // (`model_fallback_offer`); a model chosen by id keeps the
                // "ask first" default.
                prefs.pick = pick;
            }
            if confirmed_stealth {
                prefs.accept_stealth(&model);
            }
            prefs.remember(&model);
            s.models = prefs;
            Ok(())
        })
        .map_err(|e| PickerError::failed(e.to_string()))?;
    broadcast(app, &saved);
    if from.as_ref() != Some(&model) {
        audit.record(AuditRecord::new(
            AuditEventType::ModelChange,
            model_change_payload(from.as_ref(), &model, reason, None),
        ));
    }
    Ok(model)
}

/// Use one of the four picks: its first model answers from the next answer
/// and the rest of its list is the automatic fallback.
#[tauri::command]
pub async fn model_use_pick(
    app: AppHandle,
    pick: Pick,
    session_override: Option<bool>,
    picker: State<'_, ModelPickerState>,
    llm: State<'_, LLMState>,
    audit: State<'_, AuditState>,
) -> PickerResult<PickerView> {
    let view = build_view(&app, &picker, &llm, &audit, false).await?;
    let model = view
        .picks
        .iter()
        .find(|p| p.pick == pick)
        .and_then(|p| p.model.clone())
        .ok_or_else(|| PickerError {
            code: "not_connected",
            message: format!(
                "No connected provider offers {}. Connect one above.",
                pick.label()
            ),
        })?;
    select_model(
        &app,
        &picker,
        &llm,
        &audit,
        model,
        Some(pick),
        false,
        session_override.unwrap_or(false),
    )
    .await?;
    build_view(&app, &picker, &llm, &audit, false).await
}

/// The fallback order of providers (Advanced).
#[tauri::command]
pub async fn model_set_provider_order(
    app: AppHandle,
    order: Vec<ProviderId>,
) -> PickerResult<ModelPrefs> {
    let (saved, _) = SettingsStore::in_dir(&data_dir(&app)?)
        .update(|s| {
            let mut prefs = std::mem::take(&mut s.models).sanitized();
            prefs.provider_order = order.clone();
            s.models = prefs.sanitized();
            Ok(())
        })
        .map_err(|e| PickerError::failed(e.to_string()))?;
    broadcast(&app, &saved);
    Ok(saved.models)
}

/// A provider's base URL (Advanced); empty or `None` clears it.
#[tauri::command]
pub async fn model_set_base_url(
    app: AppHandle,
    provider: ProviderId,
    url: Option<String>,
) -> PickerResult<ModelPrefs> {
    let store = SettingsStore::in_dir(&data_dir(&app)?);
    let mut prefs = store
        .load()
        .map_err(|e| PickerError::failed(e.to_string()))?
        .models
        .sanitized();
    prefs
        .set_base_url(provider, url.as_deref())
        .map_err(|e| PickerError {
            code: "invalid",
            message: e.to_string(),
        })?;
    let (saved, _) = store
        .update(|s| {
            s.models = prefs.clone();
            Ok(())
        })
        .map_err(|e| PickerError::failed(e.to_string()))?;
    broadcast(&app, &saved);
    Ok(saved.models)
}

/// Star or unstar a model.
#[tauri::command]
pub async fn model_set_favourite(
    app: AppHandle,
    model: ModelRef,
    favourite: bool,
) -> PickerResult<ModelPrefs> {
    let model = model.validated().map_err(|e| PickerError {
        code: "invalid",
        message: e.to_string(),
    })?;
    let (saved, _) = SettingsStore::in_dir(&data_dir(&app)?)
        .update(|s| {
            let mut prefs = std::mem::take(&mut s.models).sanitized();
            prefs.set_favourite(&model, favourite);
            s.models = prefs;
            Ok(())
        })
        .map_err(|e| PickerError::failed(e.to_string()))?;
    broadcast(&app, &saved);
    Ok(saved.models)
}

/// The fallback model (`None`: the app picks one) and whether to use it
/// without asking.
#[tauri::command]
pub async fn model_set_fallback(
    app: AppHandle,
    fallback: Option<ModelRef>,
    always_fall_back: bool,
) -> PickerResult<ModelPrefs> {
    let fallback = fallback
        .map(|m| m.validated())
        .transpose()
        .map_err(|e| PickerError {
            code: "invalid",
            message: e.to_string(),
        })?;
    let (saved, _) = SettingsStore::in_dir(&data_dir(&app)?)
        .update(|s| {
            let mut prefs = std::mem::take(&mut s.models).sanitized();
            prefs.fallback = fallback.clone();
            prefs.always_fall_back = always_fall_back;
            s.models = prefs;
            Ok(())
        })
        .map_err(|e| PickerError::failed(e.to_string()))?;
    broadcast(&app, &saved);
    Ok(saved.models)
}

/// The model offered instead of `failed`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FallbackOffer {
    pub model: ModelRef,
    /// Display name (the catalog's, else the id).
    pub name: String,
    /// The "always fall back" setting is on: retry without asking.
    pub automatic: bool,
}

/// The fallback for an answer that failed with `failed` (the model it ran
/// with), or `None` when no other model is usable. Uses the cached model list
/// only (never waits for the network).
#[tauri::command]
pub async fn model_fallback_offer(
    app: AppHandle,
    failed: ModelRef,
    picker: State<'_, ModelPickerState>,
    llm: State<'_, LLMState>,
    audit: State<'_, AuditState>,
) -> PickerResult<Option<FallbackOffer>> {
    let failed = run_model(failed, configured_model(&llm).as_ref());
    let settings = SettingsStore::in_dir(&data_dir(&app)?)
        .load()
        .map_err(|e| PickerError::failed(e.to_string()))?;
    let prefs = settings.models.clone().sanitized();
    let local_only = settings.policy.local_only;
    let connect = app.state::<ConnectState>();
    let connected = connected_providers(&llm, &connect);
    let cached = picker
        .cache(&audit)
        .and_then(|c| c.get(OPENROUTER_SOURCE).ok().flatten());
    let lmstudio = connect.lmstudio();
    let catalog = build_catalog(&CatalogInputs {
        openrouter: cached.as_ref().map(|c| c.models.as_slice()),
        connected: &connected,
        lmstudio: &lmstudio.models,
        local_only,
    });
    let allow_stealth = stealth_allowed_by_env();
    // A pick falls back down its own list, automatically.
    let order = provider_order(&prefs.provider_order, &table().order);
    let from_pick = prefs.pick.and_then(|pick| {
        let chain = resolve_chain(
            pick,
            &pick_context(&connected, &order, &catalog, local_only),
        );
        next_in_chain(&chain, &failed).filter(|m| *m != failed)
    });
    let automatic = from_pick.is_some() || prefs.always_fall_back;
    let offer = from_pick
        .or_else(|| {
            choose_fallback(&FallbackContext {
                current: &failed,
                preferred: prefs.fallback.as_ref(),
                connected: &connected,
                catalog: &catalog,
                local_only,
                allow_stealth,
            })
        })
        .filter(|m| prefs.stealth_ok(m, allow_stealth));
    Ok(offer.map(|model| {
        let name = catalog
            .iter()
            .find(|c| c.provider == model.provider && c.id == model.model)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| model.model.clone());
        FallbackOffer {
            model,
            name,
            automatic,
        }
    }))
}

/// The picker entry of a run's model. A run names its runtime provider
/// (`anthropic/…`), which a Claude Pro/Max sign-in shares with the Anthropic
/// API key: when the active model is the subscription's, it is that one.
fn run_model(failed: ModelRef, active: Option<&ModelRef>) -> ModelRef {
    match active {
        Some(active)
            if active.model == failed.model
                && active.provider != failed.provider
                && active.provider.api_provider_omp_id()
                    == failed.provider.api_provider_omp_id() =>
        {
            active.clone()
        }
        _ => failed,
    }
}

/// One answer run with another model than the active one (a fallback).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOverride {
    pub model: ModelRef,
    /// Retried by the "always fall back" setting rather than a click.
    #[serde(default)]
    pub automatic: bool,
    /// The classified failure of the model it replaces (`rate_limited`, …).
    #[serde(default)]
    pub failure: Option<String>,
}

/// The LLM mode for a one-answer override, checked like a selection (key,
/// Local-only mode, stealth consent), with its `model_change` record (the
/// caller records it once the answer's session is ready).
pub(crate) fn override_mode(
    app: &AppHandle,
    llm: &LLMState,
    request: &ModelOverride,
) -> PickerResult<(LLMMode, AuditRecord)> {
    let settings = SettingsStore::in_dir(&data_dir(app)?)
        .load()
        .map_err(|e| PickerError::failed(e.to_string()))?;
    let prefs = settings.models.sanitized();
    let connected = connected_providers(llm, &app.state::<ConnectState>());
    let (model, _) = check_selection(
        request.model.clone(),
        &SelectionContext {
            prefs: &prefs,
            connected: &connected,
            local_only: settings.policy.local_only,
            env_allows_stealth: stealth_allowed_by_env(),
            environment: None,
        },
        false,
        false,
    )?;
    let mode = mode_for(llm, &model)?;
    let from = configured_model(llm);
    let reason = if request.automatic {
        ChangeReason::FallbackAutomatic
    } else {
        ChangeReason::FallbackOnce
    };
    let failure = request
        .failure
        .as_deref()
        .filter(|f| f.len() <= 32 && f.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
    let record = AuditRecord::new(
        AuditEventType::ModelChange,
        model_change_payload(from.as_ref(), &model, reason, failure),
    );
    Ok((mode, record))
}

/// At startup, after the environment was applied (`environment` is the model
/// it set): apply the saved choice unless the environment takes precedence.
pub async fn apply_startup_choice(app: &AppHandle, environment: Option<ModelRef>) {
    let picker = app.state::<ModelPickerState>();
    picker.set_environment(environment.clone());
    let Ok(dir) = data_dir(app) else {
        return;
    };
    let chosen = match SettingsStore::in_dir(&dir).load() {
        Ok(settings) => settings.models.sanitized().chosen,
        Err(e) => {
            tracing::warn!("Saved model choice not read: {e}");
            return;
        }
    };
    let Some(ActiveModel {
        model,
        source: ModelSource::Settings,
    }) = resolve_active(None, environment.as_ref(), chosen.as_ref())
    else {
        // The environment's model (already applied), or none at all.
        return;
    };
    let llm = app.state::<LLMState>();
    match apply(&llm, &model).await {
        Ok(()) => tracing::info!("Model from settings: {}", model.describe()),
        Err(e) => tracing::warn!(
            "Saved model {} not applied: {}",
            model.describe(),
            e.message
        ),
    }
}

/// Keep the picker in step with a switch made in the older model settings
/// panel (`switch_llm_mode`): saved as the choice, or a session override
/// when the environment sets the model.
pub fn note_settings_switch(app: &AppHandle, mode: &LLMMode) {
    let model = match mode {
        LLMMode::External {
            provider, model, ..
        } => ProviderId::from_api(provider)
            .map(|p| ModelRef::new(p, model.clone()))
            .and_then(|m| m.validated().ok()),
        _ => None,
    };
    let picker = app.state::<ModelPickerState>();
    if let Some(env) = picker.environment() {
        picker.set_session_override(model.filter(|m| *m != env));
        return;
    }
    let Ok(dir) = data_dir(app) else {
        return;
    };
    match SettingsStore::in_dir(&dir).update(|s| {
        let mut prefs = std::mem::take(&mut s.models).sanitized();
        if let Some(m) = &model {
            prefs.remember(m);
        }
        prefs.chosen = model.clone();
        s.models = prefs;
        Ok(())
    }) {
        Ok((saved, _)) => broadcast(app, &saved),
        Err(e) => tracing::warn!("Model choice not saved: {e}"),
    }
}

/// The picker entry for an `ApiProvider` and model id (the environment's
/// model). Not validated: an environment model with an unusable id must still
/// take precedence over the saved choice (the agent then reports the bad id).
pub fn model_ref_for(provider: &ApiProvider, model: &str) -> Option<ModelRef> {
    ProviderId::from_api(provider).map(|p| ModelRef::new(p, model.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::harness::model_choice::SelectionError;
    use shodh_rag::llm::LLMConfig;
    use tokio::sync::RwLock as AsyncRwLock;

    fn llm_with(mode: LLMMode) -> LLMState {
        let config = LLMConfig {
            mode,
            ..LLMConfig::default()
        };
        LLMState {
            manager: Arc::new(AsyncRwLock::new(None)),
            config: Arc::new(Mutex::new(config)),
            api_keys: Arc::new(Mutex::new(crate::llm_commands::ApiKeys::default())),
            custom_model_path: Arc::new(Mutex::new(None)),
        }
    }

    fn external(provider: ApiProvider, model: &str) -> LLMMode {
        LLMMode::External {
            provider,
            api_key: "sk-test".into(),
            model: model.into(),
        }
    }

    #[test]
    fn the_active_model_says_where_it_was_set() {
        let picker = ModelPickerState::default();
        let env = ModelRef::new(ProviderId::Anthropic, "claude-haiku-4-5");
        picker.set_environment(Some(env.clone()));

        let from_env = llm_with(external(ApiProvider::Anthropic, "claude-haiku-4-5"));
        assert_eq!(
            picker.active(&from_env),
            Some(ActiveModel {
                model: env,
                source: ModelSource::Environment
            })
        );

        let overridden = ModelRef::new(ProviderId::OpenRouter, "qwen/qwen3-coder:free");
        picker.set_session_override(Some(overridden.clone()));
        let session = llm_with(external(ApiProvider::OpenRouter, "qwen/qwen3-coder:free"));
        assert_eq!(
            picker.active(&session).unwrap().source,
            ModelSource::Session
        );
        assert_eq!(picker.active(&session).unwrap().model, overridden);

        let saved = llm_with(external(ApiProvider::OpenAI, "gpt-5-mini"));
        assert_eq!(picker.active(&saved).unwrap().source, ModelSource::Settings);

        assert_eq!(picker.active(&llm_with(LLMMode::Disabled)), None);
        let local = LLMMode::Local {
            model_path: PathBuf::from("C:/models/qwen.gguf"),
        };
        assert_eq!(
            picker.active(&llm_with(local)),
            None,
            "llama.cpp is not a picker model"
        );
    }

    #[test]
    fn keys_come_from_the_keychain_copy_and_ollama_needs_none() {
        let llm = llm_with(LLMMode::Disabled);
        lock(&llm.api_keys).openrouter = Some("sk-or-test".into());
        if std::env::var("OPENROUTER_API_KEY").is_err() {
            assert_eq!(
                provider_key(&llm, ProviderId::OpenRouter).as_deref(),
                Some("sk-or-test")
            );
        }
        assert!(keyed_providers(&llm).contains(&ProviderId::OpenRouter));
        let ollama = mode_for(&llm, &ModelRef::new(ProviderId::Ollama, "qwen3:4b")).unwrap();
        assert!(matches!(
            ollama,
            LLMMode::External {
                provider: ApiProvider::Ollama,
                ..
            }
        ));
        if std::env::var("ANTHROPIC_API_KEY").is_err() {
            let missing = mode_for(
                &llm,
                &ModelRef::new(ProviderId::Anthropic, "claude-haiku-4-5"),
            )
            .unwrap_err();
            assert_eq!(missing.code, "missing_key");
            assert!(!missing.message.contains("sk-"));
        }
    }

    #[test]
    fn the_environment_model_keeps_precedence_even_with_an_odd_id() {
        assert_eq!(
            model_ref_for(&ApiProvider::OpenRouter, " a/b "),
            Some(ModelRef::new(ProviderId::OpenRouter, "a/b"))
        );
        assert!(model_ref_for(&ApiProvider::OpenRouter, "--odd id").is_some());
        assert_eq!(model_ref_for(&ApiProvider::Perplexity, "x"), None);
    }

    #[test]
    fn refusals_carry_a_code_for_the_ui() {
        let e: PickerError = SelectionError::StealthNeedsConfirmation("stealth/x".into()).into();
        assert_eq!(e.code, "stealth_confirmation");
        let e: PickerError = SelectionError::MissingKey("OpenAI").into();
        assert_eq!(e.code, "missing_key");
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["code"], "missing_key");
        assert!(json["message"].as_str().unwrap().contains("OpenAI"));
    }

    #[test]
    fn a_failed_run_is_matched_to_the_subscription_it_used() {
        let sub = ModelRef::new(ProviderId::ClaudeSub, "claude-opus-5-5");
        let from_run = ModelRef::new(ProviderId::Anthropic, "claude-opus-5-5");
        assert_eq!(run_model(from_run.clone(), Some(&sub)), sub);
        let keyed = ModelRef::new(ProviderId::Anthropic, "claude-opus-5-5");
        assert_eq!(run_model(from_run.clone(), Some(&keyed)), keyed);
        let other = ModelRef::new(ProviderId::ClaudeSub, "claude-haiku-4-5");
        assert_eq!(run_model(from_run.clone(), Some(&other)), from_run);
        assert_eq!(run_model(from_run.clone(), None), from_run);
    }

    #[test]
    fn ollama_hosts_get_a_scheme() {
        // Read only when OLLAMA_HOST is not set in the test environment.
        if std::env::var("OLLAMA_HOST").is_err() {
            assert_eq!(ollama_host(), OLLAMA_DEFAULT_HOST);
        }
    }
}
