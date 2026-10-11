//! The user's model choice: what is kept in settings, which source wins
//! (this session's override, the environment, the saved choice), what the
//! picker remembers (recent, favourites, stealth consent) and the record of
//! every switch.
//!
//! Pure: no I/O. The app keeps [`ModelPrefs`] in its settings file (API keys
//! stay in the OS keychain and never appear here) and applies the resolved
//! model to the next answer.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model_catalog::{CatalogError, ModelRef, ProviderId};
use super::model_picks::Pick;

/// Models kept in the picker's "Recent" list.
pub const MAX_RECENT: usize = 6;
/// Favourites kept (the oldest are dropped past this).
pub const MAX_FAVOURITES: usize = 40;
/// Stealth models whose logging risk the user accepted (the oldest are dropped past this).
pub const MAX_STEALTH_ACCEPTED: usize = 20;

/// The model picker's settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelPrefs {
    /// The model the agent answers with (when no environment variable or
    /// session override takes precedence).
    pub chosen: Option<ModelRef>,
    /// Offered when the current model is rate-limited or unavailable;
    /// `None` lets the app pick (see `choose_fallback`).
    pub fallback: Option<ModelRef>,
    /// Retry with the fallback model without asking. Off: every fallback
    /// is one click on the error card.
    pub always_fall_back: bool,
    /// Stealth model ids the user confirmed in the picker (their provider may
    /// log prompts for training).
    pub stealth_accepted: Vec<String>,
    /// Recently chosen models, newest first.
    pub recent: Vec<ModelRef>,
    /// Starred models, in the order they were starred.
    pub favourites: Vec<ModelRef>,
    /// The pick the chosen model came from (Best quality, Fast & cheap,
    /// Free, Private); its list is the automatic fallback order. `None`
    /// for a model chosen by id.
    pub pick: Option<Pick>,
    /// The person's fallback order of providers (Advanced); providers not
    /// listed follow in the default order.
    pub provider_order: Vec<ProviderId>,
    /// Base URL overrides per provider (Advanced), passed to the runtime.
    pub base_urls: Vec<BaseUrl>,
    /// Provider keys found in the environment were copied into the OS
    /// credential store (once, at the first start that found them).
    pub env_keys_migrated: bool,
}

/// A provider's base URL override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseUrl {
    pub provider: ProviderId,
    pub url: String,
}

/// The runtime's base URL variable for a provider, for providers whose
/// address can be changed.
pub fn base_url_var(provider: ProviderId) -> Option<&'static str> {
    match provider {
        ProviderId::Anthropic => Some("ANTHROPIC_BASE_URL"),
        ProviderId::OpenAI => Some("OPENAI_BASE_URL"),
        ProviderId::OpenRouter => Some("OPENROUTER_BASE_URL"),
        ProviderId::Grok => Some("XAI_BASE_URL"),
        ProviderId::LmStudio => Some(super::model::LM_STUDIO_URL_VAR),
        _ => None,
    }
}

/// Why a base URL is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BaseUrlError {
    #[error("The address of {0} cannot be changed.")]
    NotSupported(&'static str),
    #[error("Enter a full address such as https://gateway.example.com/v1.")]
    Invalid,
    #[error("Use https for an address that is not on this computer, so the API key is not sent unencrypted.")]
    Insecure,
}

/// Check a base URL: http(s) without credentials, https unless it is on
/// this computer. Returns it trimmed, without a trailing slash.
pub fn check_base_url(provider: ProviderId, url: &str) -> Result<String, BaseUrlError> {
    base_url_var(provider).ok_or(BaseUrlError::NotSupported(provider.label()))?;
    let url = url.trim().trim_end_matches('/');
    let parsed = url::Url::parse(url).map_err(|_| BaseUrlError::Invalid)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || url.len() > 300
    {
        return Err(BaseUrlError::Invalid);
    }
    if parsed.scheme() == "http" && !super::catalog_fetch::is_loopback_url(url) {
        return Err(BaseUrlError::Insecure);
    }
    Ok(url.to_string())
}

/// Why a stored list entry is dropped while reading settings.
fn keep(model: &ModelRef) -> bool {
    model.clone().validated().is_ok()
}

impl ModelPrefs {
    /// Drops invalid entries, duplicates and overflow, so a hand-edited
    /// settings file cannot put an unsafe id on the runtime's command line.
    pub fn sanitized(mut self) -> Self {
        self.chosen = self.chosen.and_then(|m| m.validated().ok());
        self.fallback = self.fallback.and_then(|m| m.validated().ok());
        self.recent = dedup(self.recent.into_iter().filter(keep).collect());
        self.recent.truncate(MAX_RECENT);
        self.favourites = dedup(self.favourites.into_iter().filter(keep).collect());
        let over = self.favourites.len().saturating_sub(MAX_FAVOURITES);
        self.favourites.drain(..over);
        let mut accepted: Vec<String> = Vec::new();
        for id in self.stealth_accepted {
            let id = id.trim().to_string();
            if super::model::is_valid_model_id(&id) && !accepted.contains(&id) {
                accepted.push(id);
            }
        }
        let over = accepted.len().saturating_sub(MAX_STEALTH_ACCEPTED);
        accepted.drain(..over);
        self.stealth_accepted = accepted;
        let mut order: Vec<ProviderId> = Vec::new();
        for p in self.provider_order {
            if !order.contains(&p) {
                order.push(p);
            }
        }
        self.provider_order = order;
        let mut urls: Vec<BaseUrl> = Vec::new();
        for b in self.base_urls {
            if urls.iter().any(|u| u.provider == b.provider) {
                continue;
            }
            if let Ok(url) = check_base_url(b.provider, &b.url) {
                urls.push(BaseUrl {
                    provider: b.provider,
                    url,
                });
            }
        }
        self.base_urls = urls;
        self
    }

    /// The base URL override of `provider`, if any.
    pub fn base_url(&self, provider: ProviderId) -> Option<&str> {
        self.base_urls
            .iter()
            .find(|b| b.provider == provider)
            .map(|b| b.url.as_str())
    }

    /// Set (or, with `None`, clear) the base URL of `provider`.
    pub fn set_base_url(
        &mut self,
        provider: ProviderId,
        url: Option<&str>,
    ) -> Result<(), BaseUrlError> {
        let checked = url
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(|u| check_base_url(provider, u))
            .transpose()?;
        self.base_urls.retain(|b| b.provider != provider);
        if let Some(url) = checked {
            self.base_urls.push(BaseUrl { provider, url });
        }
        Ok(())
    }

    /// Put `model` first in the recent list.
    pub fn remember(&mut self, model: &ModelRef) {
        self.recent.retain(|m| m != model);
        self.recent.insert(0, model.clone());
        self.recent.truncate(MAX_RECENT);
    }

    /// Star or unstar `model`. Returns whether it is now a favourite.
    pub fn set_favourite(&mut self, model: &ModelRef, favourite: bool) -> bool {
        self.favourites.retain(|m| m != model);
        if favourite {
            self.favourites.push(model.clone());
            let over = self.favourites.len().saturating_sub(MAX_FAVOURITES);
            self.favourites.drain(..over);
        }
        favourite
    }

    /// The user accepted that this stealth model's provider may log prompts.
    pub fn accept_stealth(&mut self, model: &ModelRef) {
        if !model.is_stealth() || self.stealth_accepted.contains(&model.model) {
            return;
        }
        self.stealth_accepted.push(model.model.clone());
        let over = self
            .stealth_accepted
            .len()
            .saturating_sub(MAX_STEALTH_ACCEPTED);
        self.stealth_accepted.drain(..over);
    }

    /// Whether `model` may be used: not stealth, or stealth and accepted
    /// here (or allowed by the environment, `env_allows`).
    pub fn stealth_ok(&self, model: &ModelRef, env_allows: bool) -> bool {
        !model.is_stealth() || env_allows || self.stealth_accepted.contains(&model.model)
    }
}

fn dedup(models: Vec<ModelRef>) -> Vec<ModelRef> {
    let mut out: Vec<ModelRef> = Vec::with_capacity(models.len());
    for m in models {
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

/// Where the active model comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// Chosen in the picker for this session while the environment sets another.
    Session,
    /// `SHODH_LLM_PROVIDER` / `SHODH_LLM_MODEL`.
    Environment,
    /// The saved choice.
    Settings,
}

/// The model the next answer uses, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveModel {
    pub model: ModelRef,
    pub source: ModelSource,
}

/// Precedence: an override made in this session, then the environment, then
/// the saved choice. `None` when no model is configured at all.
pub fn resolve_active(
    session: Option<&ModelRef>,
    environment: Option<&ModelRef>,
    saved: Option<&ModelRef>,
) -> Option<ActiveModel> {
    let pick = |model: &ModelRef, source| ActiveModel {
        model: model.clone(),
        source,
    };
    session
        .map(|m| pick(m, ModelSource::Session))
        .or_else(|| environment.map(|m| pick(m, ModelSource::Environment)))
        .or_else(|| saved.map(|m| pick(m, ModelSource::Settings)))
}

/// What a selection in the picker does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Saved as the choice and used from the next answer.
    Saved,
    /// The environment sets the model: used for this session only (the
    /// environment wins again at the next start).
    SessionOverride,
}

/// Why a selection is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectionError {
    #[error(transparent)]
    Invalid(#[from] CatalogError),
    #[error("{0} sends prompts off this computer, and Local-only mode is on. Choose a local model, or turn off Local-only mode in Settings → Privacy.")]
    LocalOnly(String),
    #[error("{0} has no API key. Add one in Settings → Model.")]
    MissingKey(&'static str),
    #[error("{0} is not connected. Connect it in Settings → Model.")]
    NotConnected(&'static str),
    #[error("Ollama models do not work through the assistant yet (a known issue in its runtime). Use LM Studio for a local model.")]
    Unsupported,
    #[error("{0} is a stealth model: its provider may log prompts and use them for training. Confirm that you accept this to use it.")]
    StealthNeedsConfirmation(String),
    #[error("The environment sets the model for this computer ({0}). Choose \"Use for this session\" to override it until Shodh restarts.")]
    EnvironmentSet(String),
}

/// What [`check_selection`] decides from.
#[derive(Debug, Clone)]
pub struct SelectionContext<'a> {
    pub prefs: &'a ModelPrefs,
    /// Providers that can answer: a key, a signed-in subscription, LM Studio running.
    pub connected: &'a [ProviderId],
    pub local_only: bool,
    /// `SHODH_ALLOW_STEALTH_MODELS=1`.
    pub env_allows_stealth: bool,
    /// The model set by the environment, if any.
    pub environment: Option<&'a ModelRef>,
}

/// Decide whether `model` may be selected and what selecting it does.
/// `confirm_stealth`: the user confirmed the stealth warning in this
/// selection. `session_override`: the user chose to override the
/// environment for this session.
pub fn check_selection(
    model: ModelRef,
    ctx: &SelectionContext<'_>,
    confirm_stealth: bool,
    session_override: bool,
) -> Result<(ModelRef, Selection), SelectionError> {
    let model = model.validated()?;
    if ctx.local_only && !model.provider.is_local() {
        return Err(SelectionError::LocalOnly(model.describe()));
    }
    if !model.provider.works_with_agent() {
        return Err(SelectionError::Unsupported);
    }
    if !ctx.connected.contains(&model.provider) {
        return Err(if model.provider.needs_key() {
            SelectionError::MissingKey(model.provider.label())
        } else {
            SelectionError::NotConnected(model.provider.label())
        });
    }
    if !confirm_stealth && !ctx.prefs.stealth_ok(&model, ctx.env_allows_stealth) {
        return Err(SelectionError::StealthNeedsConfirmation(model.model));
    }
    match ctx.environment {
        Some(env) if env != &model && !session_override => {
            Err(SelectionError::EnvironmentSet(env.describe()))
        }
        Some(_) => Ok((model, Selection::SessionOverride)),
        None => Ok((model, Selection::Saved)),
    }
}

/// Why the model changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReason {
    /// Chosen in the picker and saved.
    UserChoice,
    /// Chosen in the picker for this session over the environment's model.
    SessionOverride,
    /// One answer retried with the fallback model (one click).
    FallbackOnce,
    /// One answer retried with the fallback model by the "always fall back" setting.
    FallbackAutomatic,
    /// Applied at startup from the environment or the saved choice.
    Startup,
}

impl ChangeReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeReason::UserChoice => "user_choice",
            ChangeReason::SessionOverride => "session_override",
            ChangeReason::FallbackOnce => "fallback_once",
            ChangeReason::FallbackAutomatic => "fallback_automatic",
            ChangeReason::Startup => "startup",
        }
    }
}

/// `model_change` audit payload: the provider and model ids only (never a
/// key, never an endpoint with credentials). `failure` is the classified
/// error that led to a fallback.
pub fn model_change_payload(
    from: Option<&ModelRef>,
    to: &ModelRef,
    reason: ChangeReason,
    failure: Option<&str>,
) -> Value {
    let describe = |m: &ModelRef| json!({"provider": m.provider.as_str(), "model": m.model});
    let mut payload = json!({
        "from": from.map(ModelRef::describe).unwrap_or_else(|| "none".to_string()),
        "to": to.describe(),
        "reason": reason.as_str(),
        "from_model": from.map(describe),
        "to_model": describe(to),
        "cloud": !to.provider.is_local(),
    });
    if let (Some(kind), Value::Object(map)) = (failure, &mut payload) {
        map.insert("failure".to_string(), Value::String(kind.to_string()));
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    fn or(model: &str) -> ModelRef {
        ModelRef::new(ProviderId::OpenRouter, model)
    }

    #[test]
    fn session_override_beats_environment_beats_saved() {
        let session = or("a/session");
        let env = ModelRef::new(ProviderId::Anthropic, "claude-haiku-4-5");
        let saved = or("a/saved");
        let all = resolve_active(Some(&session), Some(&env), Some(&saved)).unwrap();
        assert_eq!((all.model, all.source), (session, ModelSource::Session));
        let env_wins = resolve_active(None, Some(&env), Some(&saved)).unwrap();
        assert_eq!(
            (env_wins.model, env_wins.source),
            (env, ModelSource::Environment)
        );
        let saved_only = resolve_active(None, None, Some(&saved)).unwrap();
        assert_eq!(saved_only.source, ModelSource::Settings);
        assert_eq!(resolve_active(None, None, None), None);
    }

    fn ctx<'a>(prefs: &'a ModelPrefs, connected: &'a [ProviderId]) -> SelectionContext<'a> {
        SelectionContext {
            prefs,
            connected,
            local_only: false,
            env_allows_stealth: false,
            environment: None,
        }
    }

    #[test]
    fn selections_are_checked_before_they_apply() {
        let prefs = ModelPrefs::default();
        let keyed = [ProviderId::OpenRouter];
        let base = ctx(&prefs, &keyed);
        assert_eq!(
            check_selection(or(" qwen/qwen3-coder:free "), &base, false, false).unwrap(),
            (or("qwen/qwen3-coder:free"), Selection::Saved)
        );
        assert!(matches!(
            check_selection(or("--config=evil"), &base, false, false),
            Err(SelectionError::Invalid(_))
        ));
        assert_eq!(
            check_selection(
                ModelRef::new(ProviderId::OpenAI, "gpt-5-mini"),
                &base,
                false,
                false
            ),
            Err(SelectionError::MissingKey("OpenAI"))
        );
        // Ollama does not work through the agent; LM Studio needs it running.
        assert_eq!(
            check_selection(
                ModelRef::new(ProviderId::Ollama, "qwen3:4b"),
                &base,
                false,
                false
            ),
            Err(SelectionError::Unsupported)
        );
        assert_eq!(
            check_selection(
                ModelRef::new(ProviderId::LmStudio, "qwen3-8b"),
                &base,
                false,
                false
            ),
            Err(SelectionError::NotConnected("LM Studio"))
        );
        assert_eq!(
            check_selection(
                ModelRef::new(ProviderId::ClaudeSub, "claude-opus-5-5"),
                &base,
                false,
                false
            ),
            Err(SelectionError::NotConnected("Claude (Pro/Max)"))
        );
        let signed_in = [
            ProviderId::OpenRouter,
            ProviderId::ClaudeSub,
            ProviderId::LmStudio,
        ];
        let with_sub = ctx(&prefs, &signed_in);
        assert!(check_selection(
            ModelRef::new(ProviderId::ClaudeSub, "claude-opus-5-5"),
            &with_sub,
            false,
            false
        )
        .is_ok());

        let local = SelectionContext {
            local_only: true,
            ..with_sub.clone()
        };
        assert!(matches!(
            check_selection(or("a/b"), &local, false, false),
            Err(SelectionError::LocalOnly(_))
        ));
        assert!(matches!(
            check_selection(
                ModelRef::new(ProviderId::ClaudeSub, "claude-opus-5-5"),
                &local,
                false,
                false
            ),
            Err(SelectionError::LocalOnly(_))
        ));
        assert!(check_selection(
            ModelRef::new(ProviderId::LmStudio, "qwen3-8b"),
            &local,
            false,
            false
        )
        .is_ok());
    }

    #[test]
    fn stealth_models_need_an_in_app_confirmation_once() {
        let mut prefs = ModelPrefs::default();
        let keyed = [ProviderId::OpenRouter];
        let stealth = or("stealth/space-bunny-alpha");
        let refused =
            check_selection(stealth.clone(), &ctx(&prefs, &keyed), false, false).unwrap_err();
        assert!(matches!(
            refused,
            SelectionError::StealthNeedsConfirmation(_)
        ));
        assert!(refused.to_string().contains("may log prompts"));
        assert!(check_selection(stealth.clone(), &ctx(&prefs, &keyed), true, false).is_ok());
        prefs.accept_stealth(&stealth);
        prefs.accept_stealth(&stealth);
        assert_eq!(
            prefs.stealth_accepted,
            vec!["stealth/space-bunny-alpha".to_string()]
        );
        assert!(check_selection(stealth.clone(), &ctx(&prefs, &keyed), false, false).is_ok());
        assert!(
            prefs.stealth_ok(&or("stealth/other"), true),
            "the environment opt-in allows all"
        );
        assert!(!prefs.stealth_ok(&or("stealth/other"), false));
        prefs.accept_stealth(&or("a/not-stealth"));
        assert_eq!(
            prefs.stealth_accepted.len(),
            1,
            "only stealth models are recorded"
        );
    }

    #[test]
    fn the_environment_is_overridden_only_on_request_and_only_for_the_session() {
        let prefs = ModelPrefs::default();
        let keyed = [ProviderId::OpenRouter, ProviderId::Anthropic];
        let env = ModelRef::new(ProviderId::Anthropic, "claude-haiku-4-5");
        let with_env = SelectionContext {
            environment: Some(&env),
            ..ctx(&prefs, &keyed)
        };
        let refused = check_selection(or("a/b"), &with_env, false, false).unwrap_err();
        assert!(matches!(refused, SelectionError::EnvironmentSet(_)));
        assert!(refused.to_string().contains("Anthropic · claude-haiku-4-5"));
        assert_eq!(
            check_selection(or("a/b"), &with_env, false, true)
                .unwrap()
                .1,
            Selection::SessionOverride
        );
        assert_eq!(
            check_selection(env.clone(), &with_env, false, false)
                .unwrap()
                .1,
            Selection::SessionOverride,
            "re-selecting the environment's model clears the override"
        );
    }

    #[test]
    fn recent_and_favourites_are_bounded_and_unique() {
        let mut prefs = ModelPrefs::default();
        for i in 0..10 {
            prefs.remember(&or(&format!("a/m{i}")));
        }
        prefs.remember(&or("a/m5"));
        assert_eq!(prefs.recent.len(), MAX_RECENT);
        assert_eq!(prefs.recent[0], or("a/m5"));
        assert_eq!(prefs.recent.iter().filter(|m| **m == or("a/m5")).count(), 1);

        assert!(prefs.set_favourite(&or("a/x"), true));
        prefs.set_favourite(&or("a/x"), true);
        assert_eq!(prefs.favourites, vec![or("a/x")]);
        assert!(!prefs.set_favourite(&or("a/x"), false));
        assert!(prefs.favourites.is_empty());
        for i in 0..(MAX_FAVOURITES + 5) {
            prefs.set_favourite(&or(&format!("f/{i}")), true);
        }
        assert_eq!(prefs.favourites.len(), MAX_FAVOURITES);
        assert_eq!(prefs.favourites[0], or("f/5"), "the oldest are dropped");
    }

    #[test]
    fn stored_prefs_are_sanitized_and_round_trip() {
        let json = r#"{
            "chosen": {"provider": "openrouter", "model": "--exec"},
            "fallback": {"provider": "anthropic", "model": " claude-haiku-4-5 "},
            "alwaysFallBack": true,
            "stealthAccepted": ["stealth/x", "stealth/x", "bad id"],
            "recent": [{"provider": "openai", "model": "gpt-5"}, {"provider": "openai", "model": "gpt-5"}],
            "favourites": [{"provider": "ollama", "model": "qwen3:4b"}],
            "pick": "fast",
            "providerOrder": ["openai", "claude-sub", "openai"],
            "baseUrls": [
                {"provider": "openai", "url": "https://gateway.example.com/v1/"},
                {"provider": "openai", "url": "https://second.example.com"},
                {"provider": "anthropic", "url": "http://10.0.0.5:8080"},
                {"provider": "lmstudio", "url": "http://127.0.0.1:4321/v1"},
                {"provider": "google", "url": "https://x.example.com"}
            ],
            "envKeysMigrated": true,
            "unknownField": 1
        }"#;
        let prefs: ModelPrefs = serde_json::from_str(json).unwrap();
        let prefs = prefs.sanitized();
        assert_eq!(prefs.chosen, None, "an unsafe id is dropped");
        assert_eq!(
            prefs.fallback,
            Some(ModelRef::new(ProviderId::Anthropic, "claude-haiku-4-5"))
        );
        assert!(prefs.always_fall_back);
        assert_eq!(prefs.stealth_accepted, vec!["stealth/x".to_string()]);
        assert_eq!(prefs.recent.len(), 1);
        assert_eq!(prefs.pick, Some(Pick::Fast));
        assert_eq!(
            prefs.provider_order,
            vec![ProviderId::OpenAI, ProviderId::ClaudeSub]
        );
        assert_eq!(
            prefs.base_urls,
            vec![
                BaseUrl {
                    provider: ProviderId::OpenAI,
                    url: "https://gateway.example.com/v1".into()
                },
                BaseUrl {
                    provider: ProviderId::LmStudio,
                    url: "http://127.0.0.1:4321/v1".into()
                },
            ],
            "duplicates, plain http off this computer and unsupported providers are dropped"
        );
        assert!(prefs.env_keys_migrated);
        let again: ModelPrefs =
            serde_json::from_value(serde_json::to_value(&prefs).unwrap()).unwrap();
        assert_eq!(again, prefs);
        assert_eq!(
            serde_json::from_str::<ModelPrefs>("{}").unwrap(),
            ModelPrefs::default()
        );
    }

    #[test]
    fn base_urls_are_checked() {
        let mut prefs = ModelPrefs::default();
        prefs
            .set_base_url(ProviderId::OpenAI, Some(" https://gw.example.com/v1/ "))
            .unwrap();
        assert_eq!(
            prefs.base_url(ProviderId::OpenAI),
            Some("https://gw.example.com/v1")
        );
        assert_eq!(
            prefs.set_base_url(ProviderId::OpenAI, Some("http://gw.example.com")),
            Err(BaseUrlError::Insecure)
        );
        assert_eq!(
            prefs.set_base_url(ProviderId::Google, Some("https://x.example.com")),
            Err(BaseUrlError::NotSupported("Google"))
        );
        assert_eq!(
            prefs.set_base_url(ProviderId::OpenAI, Some("https://user:pw@gw.example.com")),
            Err(BaseUrlError::Invalid)
        );
        assert_eq!(
            prefs.set_base_url(ProviderId::OpenAI, Some("not a url")),
            Err(BaseUrlError::Invalid)
        );
        assert_eq!(
            prefs.base_url(ProviderId::OpenAI),
            Some("https://gw.example.com/v1")
        );
        prefs.set_base_url(ProviderId::OpenAI, None).unwrap();
        assert_eq!(prefs.base_url(ProviderId::OpenAI), None);
        assert_eq!(base_url_var(ProviderId::Grok), Some("XAI_BASE_URL"));
        assert_eq!(base_url_var(ProviderId::ClaudeSub), None);
    }

    #[test]
    fn model_change_payloads_name_models_and_never_keys() {
        let from = or("nvidia/nemotron:free");
        let to = ModelRef::new(ProviderId::Anthropic, "claude-haiku-4-5");
        let p = model_change_payload(
            Some(&from),
            &to,
            ChangeReason::FallbackOnce,
            Some("rate_limited"),
        );
        assert_eq!(p["from"], "OpenRouter · nvidia/nemotron:free");
        assert_eq!(p["to"], "Anthropic · claude-haiku-4-5");
        assert_eq!(p["reason"], "fallback_once");
        assert_eq!(p["failure"], "rate_limited");
        assert_eq!(p["to_model"]["provider"], "anthropic");
        assert_eq!(p["cloud"], true);
        let text = p.to_string().to_ascii_lowercase();
        assert!(!text.contains("key") && !text.contains("sk-"), "{text}");
        let first = model_change_payload(None, &to, ChangeReason::Startup, None);
        assert_eq!(first["from"], "none");
        assert!(first.get("failure").is_none());
    }
}
