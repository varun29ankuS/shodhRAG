//! The models the agent can be switched to, and which one to fall back to.
//!
//! OpenRouter publishes its model list (context length, per-token prices,
//! supported parameters) at a public endpoint; [`parse_openrouter`] turns it
//! into [`CatalogModel`]s. Direct providers (Anthropic, OpenAI, Google, xAI)
//! get a short list of known tool-capable models whose prices are looked up
//! in the OpenRouter list under their OpenRouter alias. Local models come from
//! Ollama. [`choose_fallback`] picks the model to offer when the current one
//! is rate-limited or unavailable.

use serde::{Deserialize, Serialize};

use super::model::{is_stealth, is_valid_model_id};
use crate::llm::{ApiProvider, SubscriptionService};

/// A provider the agent runtime can use: an API key, a signed-in
/// subscription or a model server on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProviderId {
    #[serde(rename = "openrouter")]
    OpenRouter,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAI,
    #[serde(rename = "google")]
    Google,
    #[serde(rename = "grok")]
    Grok,
    #[serde(rename = "ollama")]
    Ollama,
    /// Claude Pro/Max, signed in.
    #[serde(rename = "claude-sub")]
    ClaudeSub,
    /// ChatGPT Plus/Pro, signed in.
    #[serde(rename = "chatgpt-sub")]
    ChatGptSub,
    /// GitHub Copilot, signed in.
    #[serde(rename = "copilot-sub")]
    CopilotSub,
    /// Google Gemini (Gemini CLI sign-in).
    #[serde(rename = "gemini-sub")]
    GeminiSub,
    /// LM Studio on this computer.
    #[serde(rename = "lmstudio")]
    LmStudio,
}

/// How a provider is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    ApiKey,
    Subscription,
    Local,
}

impl ProviderId {
    pub const ALL: [ProviderId; 11] = [
        ProviderId::OpenRouter,
        ProviderId::Anthropic,
        ProviderId::OpenAI,
        ProviderId::Google,
        ProviderId::Grok,
        ProviderId::Ollama,
        ProviderId::ClaudeSub,
        ProviderId::ChatGptSub,
        ProviderId::CopilotSub,
        ProviderId::GeminiSub,
        ProviderId::LmStudio,
    ];

    /// Providers connected with an API key.
    pub const KEYED: [ProviderId; 5] = [
        ProviderId::OpenRouter,
        ProviderId::Anthropic,
        ProviderId::OpenAI,
        ProviderId::Google,
        ProviderId::Grok,
    ];

    /// The id used by the key store, the settings file and the UI.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::OpenRouter => "openrouter",
            ProviderId::Anthropic => "anthropic",
            ProviderId::OpenAI => "openai",
            ProviderId::Google => "google",
            ProviderId::Grok => "grok",
            ProviderId::Ollama => "ollama",
            ProviderId::ClaudeSub => "claude-sub",
            ProviderId::ChatGptSub => "chatgpt-sub",
            ProviderId::CopilotSub => "copilot-sub",
            ProviderId::GeminiSub => "gemini-sub",
            ProviderId::LmStudio => "lmstudio",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        match text.as_str() {
            "gemini" => Some(ProviderId::Google),
            "xai" => Some(ProviderId::Grok),
            "lm-studio" => Some(ProviderId::LmStudio),
            _ => ProviderId::ALL.into_iter().find(|p| p.as_str() == text),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProviderId::OpenRouter => "OpenRouter",
            ProviderId::Anthropic => "Anthropic",
            ProviderId::OpenAI => "OpenAI",
            ProviderId::Google => "Google",
            ProviderId::Grok => "xAI",
            ProviderId::Ollama => "Ollama",
            ProviderId::LmStudio => "LM Studio",
            sub => sub
                .subscription()
                .map(SubscriptionService::label)
                .unwrap_or("Subscription"),
        }
    }

    pub fn kind(self) -> ProviderKind {
        match self {
            ProviderId::Ollama | ProviderId::LmStudio => ProviderKind::Local,
            ProviderId::ClaudeSub
            | ProviderId::ChatGptSub
            | ProviderId::CopilotSub
            | ProviderId::GeminiSub => ProviderKind::Subscription,
            _ => ProviderKind::ApiKey,
        }
    }

    /// The subscription behind a signed-in provider.
    pub fn subscription(self) -> Option<SubscriptionService> {
        match self {
            ProviderId::ClaudeSub => Some(SubscriptionService::Claude),
            ProviderId::ChatGptSub => Some(SubscriptionService::ChatGpt),
            ProviderId::CopilotSub => Some(SubscriptionService::Copilot),
            ProviderId::GeminiSub => Some(SubscriptionService::Gemini),
            _ => None,
        }
    }

    pub fn from_subscription(service: SubscriptionService) -> Self {
        match service {
            SubscriptionService::Claude => ProviderId::ClaudeSub,
            SubscriptionService::ChatGpt => ProviderId::ChatGptSub,
            SubscriptionService::Copilot => ProviderId::CopilotSub,
            SubscriptionService::Gemini => ProviderId::GeminiSub,
        }
    }

    pub fn api_provider(self) -> ApiProvider {
        match self {
            ProviderId::OpenRouter => ApiProvider::OpenRouter,
            ProviderId::Anthropic => ApiProvider::Anthropic,
            ProviderId::OpenAI => ApiProvider::OpenAI,
            ProviderId::Google => ApiProvider::Google,
            ProviderId::Grok => ApiProvider::Grok,
            ProviderId::Ollama => ApiProvider::Ollama,
            ProviderId::LmStudio => ApiProvider::LmStudio,
            sub => {
                ApiProvider::Subscription(sub.subscription().unwrap_or(SubscriptionService::Claude))
            }
        }
    }

    pub fn from_api(provider: &ApiProvider) -> Option<Self> {
        match provider {
            ApiProvider::OpenRouter => Some(ProviderId::OpenRouter),
            ApiProvider::Anthropic => Some(ProviderId::Anthropic),
            ApiProvider::OpenAI => Some(ProviderId::OpenAI),
            ApiProvider::Google => Some(ProviderId::Google),
            ApiProvider::Grok => Some(ProviderId::Grok),
            ApiProvider::Ollama => Some(ProviderId::Ollama),
            ApiProvider::LmStudio => Some(ProviderId::LmStudio),
            ApiProvider::Subscription(service) => Some(Self::from_subscription(*service)),
            _ => None,
        }
    }

    /// The runtime's (omp) provider id: a Claude Pro/Max sign-in and an
    /// Anthropic key are both `anthropic`.
    pub fn api_provider_omp_id(self) -> &'static str {
        match self {
            ProviderId::OpenRouter => "openrouter",
            ProviderId::Anthropic => "anthropic",
            ProviderId::OpenAI => "openai",
            ProviderId::Google => "google",
            ProviderId::Grok => "xai",
            ProviderId::Ollama => "ollama",
            ProviderId::LmStudio => "lm-studio",
            sub => sub
                .subscription()
                .map(SubscriptionService::omp_provider)
                .unwrap_or("anthropic"),
        }
    }

    /// Prompts stay on this computer.
    pub fn is_local(self) -> bool {
        self.kind() == ProviderKind::Local
    }

    /// Needs an API key.
    pub fn needs_key(self) -> bool {
        self.kind() == ProviderKind::ApiKey
    }

    /// Can answer through the agent. Ollama models do not work through
    /// omp 18.4.10 yet (they are shown, disabled, with that reason).
    pub fn works_with_agent(self) -> bool {
        self != ProviderId::Ollama
    }
}

/// The provider a pasted API key belongs to, from its prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KeyDetection {
    Known {
        provider: ProviderId,
    },
    /// The prefix fits several providers (or none): the person chooses.
    Ambiguous {
        candidates: Vec<ProviderId>,
    },
}

/// Detect the provider of `key` from its prefix: `sk-ant-` Anthropic,
/// `sk-or-` OpenRouter, `sk-proj-` OpenAI, `AIza` Google, `xai-` xAI. A
/// plain `sk-` key is usually OpenAI's but other providers use it too, so it
/// is ambiguous (OpenAI first); anything else is ambiguous among all key
/// providers.
pub fn detect_key_provider(key: &str) -> KeyDetection {
    let key = key.trim();
    let known = |provider| KeyDetection::Known { provider };
    if key.starts_with("sk-ant-") {
        known(ProviderId::Anthropic)
    } else if key.starts_with("sk-or-") {
        known(ProviderId::OpenRouter)
    } else if key.starts_with("sk-proj-") || key.starts_with("sk-svcacct-") {
        known(ProviderId::OpenAI)
    } else if key.starts_with("AIza") {
        known(ProviderId::Google)
    } else if key.starts_with("xai-") {
        known(ProviderId::Grok)
    } else if key.starts_with("sk-") {
        KeyDetection::Ambiguous {
            candidates: vec![
                ProviderId::OpenAI,
                ProviderId::OpenRouter,
                ProviderId::Anthropic,
            ],
        }
    } else {
        KeyDetection::Ambiguous {
            candidates: ProviderId::KEYED.to_vec(),
        }
    }
}

/// Whether `key` looks like a key at all (one line of printable ASCII).
pub fn is_plausible_key(key: &str) -> bool {
    let key = key.trim();
    (8..=512).contains(&key.len()) && key.chars().all(|c| c.is_ascii_graphic())
}

/// One model of one provider, as chosen by the user.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider: ProviderId,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    #[error("The model id {0:?} is not valid.")]
    InvalidModel(String),
    #[error("The model list could not be read: {0}")]
    Parse(String),
}

impl ModelRef {
    pub fn new(provider: ProviderId, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }

    /// Trimmed, with a model id that is safe to pass to the runtime.
    pub fn validated(self) -> Result<Self, CatalogError> {
        let model = self.model.trim().to_string();
        if is_valid_model_id(&model) {
            Ok(Self { model, ..self })
        } else {
            Err(CatalogError::InvalidModel(model))
        }
    }

    pub fn is_stealth(&self) -> bool {
        self.provider == ProviderId::OpenRouter && is_stealth(&self.model)
    }

    /// "OpenRouter · nvidia/nemotron…" for logs and audit payloads.
    pub fn describe(&self) -> String {
        format!("{} · {}", self.provider.label(), self.model)
    }
}

/// Whether a model accepts tool definitions (the agent needs tools).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSupport {
    Yes,
    No,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Free,
    Paid,
    Local,
    /// Included in a signed-in subscription.
    Subscription,
}

/// What happens to prompts sent to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyNote {
    /// Free and stealth endpoints may log prompts (and train on them).
    MayLogPrompts,
    /// Sent to the provider under its API terms.
    ProviderTerms,
    /// Stays on this device.
    OnDevice,
}

/// A model the picker can show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub provider: ProviderId,
    pub id: String,
    pub name: String,
    pub context_length: Option<u32>,
    /// USD per million input tokens; `None` when unknown or variable.
    pub prompt_per_million: Option<f64>,
    /// USD per million output tokens; `None` when unknown or variable.
    pub completion_per_million: Option<f64>,
    pub tools: ToolSupport,
    pub tier: ModelTier,
    pub privacy: PrivacyNote,
    /// OpenRouter stealth model: needs the user's explicit opt-in.
    pub stealth: bool,
}

impl CatalogModel {
    pub fn model_ref(&self) -> ModelRef {
        ModelRef::new(self.provider, self.id.clone())
    }

    /// Usable by the agent: tool calling is not known to be missing.
    pub fn agent_capable(&self) -> bool {
        self.tools != ToolSupport::No
    }
}

#[derive(Deserialize)]
struct OpenRouterList {
    data: Vec<OpenRouterModel>,
}

#[derive(Deserialize)]
struct OpenRouterModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context_length: Option<f64>,
    #[serde(default)]
    pricing: Option<OpenRouterPricing>,
    #[serde(default)]
    supported_parameters: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct OpenRouterPricing {
    #[serde(default)]
    prompt: Option<serde_json::Value>,
    #[serde(default)]
    completion: Option<serde_json::Value>,
}

/// USD per token (a string such as "0.000003", or a number) as USD per
/// million tokens. Negative values mark variable pricing (`openrouter/auto`)
/// and, like anything unreadable, are unknown.
fn per_million(value: Option<&serde_json::Value>) -> Option<f64> {
    let per_token = match value? {
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
        serde_json::Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    if !per_token.is_finite() || per_token < 0.0 {
        return None;
    }
    // Round away float noise (0.000003 * 1e6 = 2.9999999999999996).
    Some((per_token * 1_000_000.0 * 1e6).round() / 1e6)
}

/// Parse OpenRouter's `GET /api/v1/models` response. Entries with an id that
/// could not be passed to the runtime are skipped.
pub fn parse_openrouter(json: &str) -> Result<Vec<CatalogModel>, CatalogError> {
    let list: OpenRouterList =
        serde_json::from_str(json).map_err(|e| CatalogError::Parse(e.to_string()))?;
    let mut models: Vec<CatalogModel> = list
        .data
        .into_iter()
        .filter(|m| is_valid_model_id(&m.id))
        .map(|m| {
            let prompt = per_million(m.pricing.as_ref().and_then(|p| p.prompt.as_ref()));
            let completion = per_million(m.pricing.as_ref().and_then(|p| p.completion.as_ref()));
            let free = m.id.ends_with(":free") || (prompt == Some(0.0) && completion == Some(0.0));
            let stealth = is_stealth(&m.id);
            let tools = match &m.supported_parameters {
                Some(params) if params.iter().any(|p| p == "tools") => ToolSupport::Yes,
                Some(_) => ToolSupport::No,
                None => ToolSupport::Unknown,
            };
            CatalogModel {
                provider: ProviderId::OpenRouter,
                name: m
                    .name
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| m.id.clone()),
                context_length: m
                    .context_length
                    .filter(|c| c.is_finite() && *c > 0.0 && *c <= f64::from(u32::MAX))
                    .map(|c| c as u32),
                prompt_per_million: prompt,
                completion_per_million: completion,
                tools,
                tier: if free {
                    ModelTier::Free
                } else {
                    ModelTier::Paid
                },
                privacy: if free || stealth {
                    PrivacyNote::MayLogPrompts
                } else {
                    PrivacyNote::ProviderTerms
                },
                stealth,
                id: m.id,
            }
        })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}

/// The fast OpenRouter model offered as a paid fallback through an OpenRouter key.
pub const OPENROUTER_FAST_FALLBACK: &str = "anthropic/claude-haiku-4.5";

/// What the catalog is built from.
#[derive(Debug, Clone, Default)]
pub struct CatalogInputs<'a> {
    /// The OpenRouter list, when fetched (or cached).
    pub openrouter: Option<&'a [CatalogModel]>,
    /// Providers that can answer: a key, a signed-in subscription, LM Studio running.
    pub connected: &'a [ProviderId],
    /// Models loaded in LM Studio, when it answered.
    pub lmstudio: &'a [String],
    /// Local-only mode: only models on this device.
    pub local_only: bool,
}

/// Every model the picker offers: the models loaded in LM Studio, the
/// curated models of connected key and subscription providers
/// (`model_picks.json`; key providers priced from the OpenRouter list) and
/// OpenRouter's list (when keyed). Models without tool calling are left out
/// (the agent needs tools); unknown support is kept and shown as unknown.
/// Ollama models are never listed: they do not work through the agent yet.
pub fn build_catalog(inputs: &CatalogInputs<'_>) -> Vec<CatalogModel> {
    let mut out = Vec::new();
    if inputs.connected.contains(&ProviderId::LmStudio) {
        for id in inputs.lmstudio {
            if !is_valid_model_id(id) {
                continue;
            }
            out.push(CatalogModel {
                provider: ProviderId::LmStudio,
                id: id.clone(),
                name: id.clone(),
                context_length: None,
                prompt_per_million: Some(0.0),
                completion_per_million: Some(0.0),
                tools: ToolSupport::Unknown,
                tier: ModelTier::Local,
                privacy: PrivacyNote::OnDevice,
                stealth: false,
            });
        }
    }
    if !inputs.local_only {
        let lookup = |alias: &str| {
            inputs
                .openrouter
                .and_then(|list| list.iter().find(|m| m.id == alias))
        };
        let table = super::model_picks::table();
        for (provider, row) in &table.providers {
            if provider.is_local() || !inputs.connected.contains(provider) {
                continue;
            }
            let subscription = provider.kind() == ProviderKind::Subscription;
            for known in &row.models {
                let priced = known.openrouter.as_deref().and_then(lookup);
                out.push(CatalogModel {
                    provider: *provider,
                    id: known.id.clone(),
                    name: known.name.clone(),
                    context_length: priced.and_then(|m| m.context_length),
                    prompt_per_million: if subscription {
                        None
                    } else {
                        priced.and_then(|m| m.prompt_per_million)
                    },
                    completion_per_million: if subscription {
                        None
                    } else {
                        priced.and_then(|m| m.completion_per_million)
                    },
                    tools: ToolSupport::Yes,
                    tier: if subscription {
                        ModelTier::Subscription
                    } else {
                        ModelTier::Paid
                    },
                    privacy: PrivacyNote::ProviderTerms,
                    stealth: false,
                });
            }
        }
        if inputs.connected.contains(&ProviderId::OpenRouter) {
            if let Some(list) = inputs.openrouter {
                out.extend(list.iter().filter(|m| m.agent_capable()).cloned());
            }
        }
    }
    out.retain(CatalogModel::agent_capable);
    out
}

/// What [`choose_fallback`] decides from.
#[derive(Debug, Clone)]
pub struct FallbackContext<'a> {
    /// The model that failed.
    pub current: &'a ModelRef,
    /// The user's chosen fallback model, if any.
    pub preferred: Option<&'a ModelRef>,
    /// Providers that can answer (see [`CatalogInputs::connected`]).
    pub connected: &'a [ProviderId],
    /// The catalog from [`build_catalog`].
    pub catalog: &'a [CatalogModel],
    pub local_only: bool,
    pub allow_stealth: bool,
}

impl FallbackContext<'_> {
    fn usable(&self, candidate: &ModelRef) -> bool {
        candidate != self.current
            && candidate.provider.works_with_agent()
            && self.connected.contains(&candidate.provider)
            && (!self.local_only || candidate.provider.is_local())
            && (self.allow_stealth || !candidate.is_stealth())
            && is_valid_model_id(&candidate.model)
    }

    fn in_catalog(&self, candidate: &ModelRef) -> Option<&CatalogModel> {
        self.catalog
            .iter()
            .find(|m| m.provider == candidate.provider && m.id == candidate.model)
    }
}

/// The model to offer when the current one is rate-limited or unavailable:
/// 1. the user's fallback model, if usable;
/// 2. in Local-only mode, another local model;
/// 3. a fast model of a connected provider (the curated Fast pick of
///    subscriptions and direct providers first, then through OpenRouter);
/// 4. another free OpenRouter model with tool calling (largest context first).
///
/// `None` when nothing else is usable. Never the failing model itself.
pub fn choose_fallback(ctx: &FallbackContext<'_>) -> Option<ModelRef> {
    if let Some(preferred) = ctx.preferred {
        if ctx.usable(preferred) {
            return Some(preferred.clone());
        }
    }
    if ctx.local_only {
        return ctx
            .catalog
            .iter()
            .filter(|m| m.tier == ModelTier::Local)
            .map(CatalogModel::model_ref)
            .find(|r| ctx.usable(r));
    }
    let table = super::model_picks::table();
    let fast_direct = table
        .order
        .iter()
        .filter(|p| **p != ProviderId::OpenRouter && !p.is_local())
        .filter_map(|p| {
            table
                .providers
                .get(p)
                .and_then(|row| row.fast.first())
                .map(|id| ModelRef::new(*p, id.clone()))
        })
        .find(|r| ctx.usable(r));
    if fast_direct.is_some() {
        return fast_direct;
    }
    let via_openrouter = ModelRef::new(ProviderId::OpenRouter, OPENROUTER_FAST_FALLBACK);
    let openrouter_listed = ctx
        .catalog
        .iter()
        .any(|m| m.provider == ProviderId::OpenRouter);
    if ctx.usable(&via_openrouter)
        && (!openrouter_listed || ctx.in_catalog(&via_openrouter).is_some())
    {
        return Some(via_openrouter);
    }
    let mut free: Vec<&CatalogModel> = ctx
        .catalog
        .iter()
        .filter(|m| m.tier == ModelTier::Free && m.tools == ToolSupport::Yes && !m.stealth)
        .filter(|m| ctx.usable(&m.model_ref()))
        .collect();
    free.sort_by(|a, b| {
        b.context_length
            .unwrap_or(0)
            .cmp(&a.context_length.unwrap_or(0))
            .then_with(|| a.id.cmp(&b.id))
    });
    free.first().map(|m| m.model_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENROUTER_FIXTURE: &str = r#"{"data":[
        {"id":"nvidia/nemotron-3-super-120b-a12b:free","name":"NVIDIA: Nemotron 3 Super (free)","context_length":262144,
         "pricing":{"prompt":"0","completion":"0","request":"0"},
         "supported_parameters":["max_tokens","temperature","tools","tool_choice"]},
        {"id":"anthropic/claude-haiku-4.5","name":"Anthropic: Claude Haiku 4.5","context_length":200000,
         "pricing":{"prompt":"0.000001","completion":"0.000005"},
         "supported_parameters":["tools","tool_choice","max_tokens"]},
        {"id":"openrouter/auto","name":"Auto Router","context_length":2000000,
         "pricing":{"prompt":"-1","completion":"-1"},
         "supported_parameters":["tools"]},
        {"id":"meta-llama/llama-guard-4-12b","name":"Llama Guard 4","context_length":163840,
         "pricing":{"prompt":"0.00000018","completion":"0.00000018"},
         "supported_parameters":["max_tokens","temperature"]},
        {"id":"qwen/qwen3-coder:free","name":"Qwen3 Coder (free)","context_length":262000,
         "pricing":{"prompt":"0","completion":"0"},
         "supported_parameters":["tools"]},
        {"id":"z-ai/glm-4.5-air:free","name":"","context_length":131072,
         "pricing":{"prompt":"0","completion":"0"}},
        {"id":"stealth/space-bunny-alpha","name":"Space Bunny","context_length":128000,
         "pricing":{"prompt":"0","completion":"0"},"supported_parameters":["tools"]},
        {"id":"--evil","name":"bad","pricing":{"prompt":"0","completion":"0"}}
    ]}"#;

    fn parsed() -> Vec<CatalogModel> {
        parse_openrouter(OPENROUTER_FIXTURE).unwrap()
    }

    fn find<'a>(models: &'a [CatalogModel], id: &str) -> &'a CatalogModel {
        models.iter().find(|m| m.id == id).unwrap()
    }

    #[test]
    fn openrouter_list_is_parsed_with_prices_tools_and_tiers() {
        let models = parsed();
        assert_eq!(models.len(), 7, "the invalid id is skipped");
        let nemotron = find(&models, "nvidia/nemotron-3-super-120b-a12b:free");
        assert_eq!(nemotron.tier, ModelTier::Free);
        assert_eq!(nemotron.tools, ToolSupport::Yes);
        assert_eq!(nemotron.context_length, Some(262_144));
        assert_eq!(nemotron.privacy, PrivacyNote::MayLogPrompts);
        let haiku = find(&models, "anthropic/claude-haiku-4.5");
        assert_eq!(haiku.tier, ModelTier::Paid);
        assert_eq!(haiku.prompt_per_million, Some(1.0));
        assert_eq!(haiku.completion_per_million, Some(5.0));
        assert_eq!(haiku.privacy, PrivacyNote::ProviderTerms);
        let auto = find(&models, "openrouter/auto");
        assert_eq!(auto.prompt_per_million, None, "variable pricing is unknown");
        assert_eq!(auto.tier, ModelTier::Paid);
        let guard = find(&models, "meta-llama/llama-guard-4-12b");
        assert_eq!(guard.tools, ToolSupport::No);
        assert_eq!(guard.prompt_per_million, Some(0.18));
        let glm = find(&models, "z-ai/glm-4.5-air:free");
        assert_eq!(glm.tools, ToolSupport::Unknown);
        assert_eq!(
            glm.name, "z-ai/glm-4.5-air:free",
            "empty names fall back to the id"
        );
        assert!(find(&models, "stealth/space-bunny-alpha").stealth);
        assert!(parse_openrouter("{\"nope\":1}").is_err());
    }

    #[test]
    fn catalog_leaves_out_models_without_tools_and_unconnected_providers() {
        let list = parsed();
        let connected = [
            ProviderId::OpenRouter,
            ProviderId::Anthropic,
            ProviderId::ClaudeSub,
        ];
        let catalog = build_catalog(&CatalogInputs {
            openrouter: Some(&list),
            connected: &connected,
            lmstudio: &["qwen3-8b".to_string()],
            local_only: false,
        });
        assert!(catalog
            .iter()
            .all(|m| m.id != "meta-llama/llama-guard-4-12b"));
        assert!(
            catalog.iter().any(|m| m.id == "z-ai/glm-4.5-air:free"),
            "unknown tool support is kept"
        );
        assert!(catalog.iter().all(|m| m.provider != ProviderId::OpenAI));
        assert!(
            catalog.iter().all(|m| m.provider != ProviderId::LmStudio),
            "LM Studio is listed only while it is connected (running)"
        );
        let haiku = catalog
            .iter()
            .find(|m| m.provider == ProviderId::Anthropic && m.id == "claude-haiku-4-5")
            .unwrap();
        assert_eq!(
            haiku.prompt_per_million,
            Some(1.0),
            "priced from the OpenRouter alias"
        );
        assert_eq!(haiku.context_length, Some(200_000));
        let opus = catalog
            .iter()
            .find(|m| m.provider == ProviderId::Anthropic && m.id == "claude-opus-5-5")
            .unwrap();
        assert_eq!(
            opus.prompt_per_million, None,
            "not in the fixture: price unknown"
        );
        let sub = catalog
            .iter()
            .find(|m| m.provider == ProviderId::ClaudeSub && m.id == "claude-haiku-4-5")
            .unwrap();
        assert_eq!(sub.tier, ModelTier::Subscription);
        assert_eq!(
            sub.prompt_per_million, None,
            "a subscription has no per-token price"
        );

        let with_lm = build_catalog(&CatalogInputs {
            openrouter: None,
            connected: &[ProviderId::LmStudio, ProviderId::Anthropic],
            lmstudio: &["qwen3-8b".to_string(), "--bad".to_string()],
            local_only: false,
        });
        let local: Vec<&CatalogModel> = with_lm
            .iter()
            .filter(|m| m.provider == ProviderId::LmStudio)
            .collect();
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].tier, ModelTier::Local);
        assert_eq!(local[0].privacy, PrivacyNote::OnDevice);
        assert!(with_lm.iter().any(|m| m.id == "claude-haiku-4-5"));
        assert!(with_lm.iter().all(|m| m.provider != ProviderId::OpenRouter));

        let local_only = build_catalog(&CatalogInputs {
            openrouter: Some(&list),
            connected: &[ProviderId::LmStudio, ProviderId::OpenRouter],
            lmstudio: &["qwen3-8b".to_string()],
            local_only: true,
        });
        assert_eq!(local_only.len(), 1);
        assert_eq!(local_only[0].provider, ProviderId::LmStudio);
    }

    fn catalog_for(connected: &[ProviderId]) -> Vec<CatalogModel> {
        let list = parsed();
        build_catalog(&CatalogInputs {
            openrouter: Some(&list),
            connected,
            lmstudio: &[],
            local_only: false,
        })
    }

    #[test]
    fn fallback_prefers_the_users_choice_then_a_fast_model() {
        let nemotron = ModelRef::new(
            ProviderId::OpenRouter,
            "nvidia/nemotron-3-super-120b-a12b:free",
        );
        let connected = [ProviderId::OpenRouter, ProviderId::OpenAI];
        let catalog = catalog_for(&connected);
        let base = FallbackContext {
            current: &nemotron,
            preferred: None,
            connected: &connected,
            catalog: &catalog,
            local_only: false,
            allow_stealth: false,
        };
        let fast_openai = ModelRef::new(ProviderId::OpenAI, "gpt-5.4-mini");
        assert_eq!(
            choose_fallback(&base),
            Some(fast_openai.clone()),
            "a direct key gives its fast pick"
        );
        let qwen = ModelRef::new(ProviderId::OpenRouter, "qwen/qwen3-coder:free");
        assert_eq!(
            choose_fallback(&FallbackContext {
                preferred: Some(&qwen),
                ..base.clone()
            }),
            Some(qwen.clone())
        );
        let unconnected = ModelRef::new(ProviderId::Google, "gemini-2.5-flash");
        assert_eq!(
            choose_fallback(&FallbackContext {
                preferred: Some(&unconnected),
                ..base.clone()
            }),
            Some(fast_openai.clone()),
            "a preferred model of an unconnected provider is skipped"
        );
        assert_eq!(
            choose_fallback(&FallbackContext {
                preferred: Some(&nemotron),
                ..base.clone()
            }),
            Some(fast_openai),
            "never the failing model"
        );
        let ollama = ModelRef::new(ProviderId::Ollama, "qwen3:4b");
        let with_ollama = [ProviderId::Ollama];
        assert_eq!(
            choose_fallback(&FallbackContext {
                preferred: Some(&ollama),
                connected: &with_ollama,
                ..base
            }),
            None,
            "Ollama does not work through the agent"
        );
    }

    #[test]
    fn subscriptions_come_before_keys_in_the_fast_fallback() {
        let failed = ModelRef::new(ProviderId::OpenAI, "gpt-5.5");
        let connected = [ProviderId::OpenAI, ProviderId::CopilotSub];
        let catalog = catalog_for(&connected);
        let chosen = choose_fallback(&FallbackContext {
            current: &failed,
            preferred: None,
            connected: &connected,
            catalog: &catalog,
            local_only: false,
            allow_stealth: false,
        });
        assert_eq!(
            chosen,
            Some(ModelRef::new(ProviderId::CopilotSub, "gpt-5-mini"))
        );
    }

    #[test]
    fn fallback_through_openrouter_then_free_models() {
        let nemotron = ModelRef::new(
            ProviderId::OpenRouter,
            "nvidia/nemotron-3-super-120b-a12b:free",
        );
        let connected = [ProviderId::OpenRouter];
        let catalog = catalog_for(&connected);
        let ctx = FallbackContext {
            current: &nemotron,
            preferred: None,
            connected: &connected,
            catalog: &catalog,
            local_only: false,
            allow_stealth: false,
        };
        assert_eq!(
            choose_fallback(&ctx),
            Some(ModelRef::new(
                ProviderId::OpenRouter,
                OPENROUTER_FAST_FALLBACK
            ))
        );
        // When the fast paid model itself failed: the free tools model with the largest context.
        let haiku = ModelRef::new(ProviderId::OpenRouter, OPENROUTER_FAST_FALLBACK);
        let from_haiku = FallbackContext {
            current: &haiku,
            ..ctx.clone()
        };
        assert_eq!(
            choose_fallback(&from_haiku),
            Some(ModelRef::new(
                ProviderId::OpenRouter,
                "nvidia/nemotron-3-super-120b-a12b:free"
            ))
        );
        // Without the paid model in the list, free models are next (never stealth).
        let free_only: Vec<CatalogModel> = catalog
            .iter()
            .filter(|m| m.id != OPENROUTER_FAST_FALLBACK)
            .cloned()
            .collect();
        let chosen = choose_fallback(&FallbackContext {
            catalog: &free_only,
            ..ctx
        })
        .unwrap();
        assert_eq!(chosen.model, "qwen/qwen3-coder:free");
    }

    #[test]
    fn fallback_in_local_only_mode_stays_local() {
        let local = ModelRef::new(ProviderId::LmStudio, "qwen3-8b");
        let models = vec!["qwen3-8b".to_string(), "gemma-3-4b".to_string()];
        let connected = [ProviderId::OpenAI, ProviderId::LmStudio];
        let catalog = build_catalog(&CatalogInputs {
            openrouter: None,
            connected: &connected,
            lmstudio: &models,
            local_only: true,
        });
        let cloud = ModelRef::new(ProviderId::OpenAI, "gpt-5-mini");
        let ctx = FallbackContext {
            current: &local,
            preferred: Some(&cloud),
            connected: &connected,
            catalog: &catalog,
            local_only: true,
            allow_stealth: false,
        };
        assert_eq!(
            choose_fallback(&ctx),
            Some(ModelRef::new(ProviderId::LmStudio, "gemma-3-4b"))
        );
        let only_one: Vec<CatalogModel> = catalog.iter().take(1).cloned().collect();
        assert_eq!(
            choose_fallback(&FallbackContext {
                catalog: &only_one,
                ..ctx
            }),
            None
        );
    }

    #[test]
    fn key_prefixes_name_their_provider() {
        let known = |p| KeyDetection::Known { provider: p };
        assert_eq!(
            detect_key_provider("sk-ant-api03-abc"),
            known(ProviderId::Anthropic)
        );
        assert_eq!(
            detect_key_provider(" sk-or-v1-abc "),
            known(ProviderId::OpenRouter)
        );
        assert_eq!(
            detect_key_provider("sk-proj-abc"),
            known(ProviderId::OpenAI)
        );
        assert_eq!(
            detect_key_provider("sk-svcacct-abc"),
            known(ProviderId::OpenAI)
        );
        assert_eq!(detect_key_provider("AIzaSyAbc"), known(ProviderId::Google));
        assert_eq!(detect_key_provider("xai-abc"), known(ProviderId::Grok));
        match detect_key_provider("sk-abc123") {
            KeyDetection::Ambiguous { candidates } => {
                assert_eq!(candidates[0], ProviderId::OpenAI, "OpenAI is the likeliest");
                assert!(candidates.contains(&ProviderId::OpenRouter));
            }
            other => panic!("{other:?}"),
        }
        match detect_key_provider("gsk_something") {
            KeyDetection::Ambiguous { candidates } => {
                assert_eq!(candidates, ProviderId::KEYED.to_vec())
            }
            other => panic!("{other:?}"),
        }
        // `sk-ant-` and `sk-or-` win over the generic `sk-`.
        assert_ne!(detect_key_provider("sk-ant-x"), detect_key_provider("sk-x"));
        assert!(is_plausible_key("sk-or-v1-0123456789"));
        assert!(!is_plausible_key("short"));
        assert!(!is_plausible_key("sk-or-v1 with spaces"));
        assert!(!is_plausible_key(
            "sk-or-v1-
two-lines"
        ));
        let json = serde_json::to_value(detect_key_provider("xai-1")).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "known", "provider": "grok"})
        );
    }

    #[test]
    fn model_refs_validate_and_serialise() {
        assert!(ModelRef::new(ProviderId::OpenAI, " gpt-5 ")
            .validated()
            .is_ok());
        assert!(ModelRef::new(ProviderId::OpenAI, "--config=x")
            .validated()
            .is_err());
        assert!(ModelRef::new(ProviderId::OpenRouter, "stealth/x").is_stealth());
        let v = serde_json::to_value(ModelRef::new(ProviderId::OpenRouter, "a/b")).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"provider": "openrouter", "model": "a/b"})
        );
        assert_eq!(ProviderId::parse("Gemini"), Some(ProviderId::Google));
        assert_eq!(ProviderId::parse("xai"), Some(ProviderId::Grok));
        assert_eq!(ProviderId::parse("acme"), None);
        assert_eq!(ProviderId::parse("claude-sub"), Some(ProviderId::ClaudeSub));
        assert_eq!(ProviderId::parse("lm-studio"), Some(ProviderId::LmStudio));
        for p in ProviderId::ALL {
            let json = serde_json::to_value(p).unwrap();
            assert_eq!(json, serde_json::json!(p.as_str()));
            assert_eq!(ProviderId::parse(p.as_str()), Some(p));
            assert_eq!(ProviderId::from_api(&p.api_provider()), Some(p));
        }
        assert!(ProviderId::ClaudeSub.subscription().is_some());
        assert!(!ProviderId::ClaudeSub.needs_key());
        assert!(ProviderId::LmStudio.is_local());
        assert!(!ProviderId::Ollama.works_with_agent());
    }
}
