//! Map the app's LLM configuration to omp's `--model provider/id` and the
//! provider credentials passed (only) through the child's environment.

use std::fmt;

use super::error::HarnessError;
use crate::llm::{ApiProvider, LLMMode};

/// A secret value. Its `Debug` and `Display` never reveal the content.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw value, for placing into the child environment only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Environment value for the omp child: plain or secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    Plain(String),
    Secret(Secret),
}

impl EnvValue {
    pub fn as_str(&self) -> &str {
        match self {
            EnvValue::Plain(v) => v,
            EnvValue::Secret(s) => s.expose(),
        }
    }
}

/// The resolved model for an omp session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmpModel {
    /// `provider/model-id`, passed as `--model=`.
    pub model_arg: String,
    /// Human label of the provider, e.g. "Anthropic".
    pub provider_label: &'static str,
    /// Whether prompts leave the machine.
    pub is_local: bool,
    /// Variables for the child environment (credentials, endpoints).
    pub env: Vec<(String, EnvValue)>,
}

/// Default Ollama endpoint, matching the in-app Ollama provider.
pub const OLLAMA_DEFAULT_HOST: &str = "http://127.0.0.1:11434";

struct ProviderMapping {
    omp_provider: &'static str,
    label: &'static str,
    key_var: Option<&'static str>,
}

fn mapping(provider: &ApiProvider) -> Result<ProviderMapping, HarnessError> {
    let m = match provider {
        ApiProvider::OpenRouter => ProviderMapping {
            omp_provider: "openrouter",
            label: "OpenRouter",
            key_var: Some("OPENROUTER_API_KEY"),
        },
        ApiProvider::Anthropic => ProviderMapping {
            omp_provider: "anthropic",
            label: "Anthropic",
            key_var: Some("ANTHROPIC_API_KEY"),
        },
        ApiProvider::OpenAI => ProviderMapping {
            omp_provider: "openai",
            label: "OpenAI",
            key_var: Some("OPENAI_API_KEY"),
        },
        ApiProvider::Google => ProviderMapping {
            omp_provider: "google",
            label: "Google",
            key_var: Some("GEMINI_API_KEY"),
        },
        ApiProvider::Grok => ProviderMapping {
            omp_provider: "xai",
            label: "xAI",
            key_var: Some("XAI_API_KEY"),
        },
        ApiProvider::Ollama => ProviderMapping {
            omp_provider: "ollama",
            label: "Ollama",
            key_var: None,
        },
        other => return Err(HarnessError::UnsupportedProvider(provider_name(other))),
    };
    Ok(m)
}

fn provider_name(provider: &ApiProvider) -> String {
    match provider {
        ApiProvider::OpenAI => "OpenAI".into(),
        ApiProvider::Anthropic => "Anthropic".into(),
        ApiProvider::OpenRouter => "OpenRouter".into(),
        ApiProvider::Together => "Together".into(),
        ApiProvider::Grok => "xAI".into(),
        ApiProvider::Perplexity => "Perplexity".into(),
        ApiProvider::Google => "Google".into(),
        ApiProvider::Replicate => "Replicate".into(),
        ApiProvider::Baseten => "Baseten".into(),
        ApiProvider::Ollama => "Ollama".into(),
        ApiProvider::HuggingFace { .. } => "Hugging Face".into(),
        ApiProvider::Custom { .. } => "custom endpoint".into(),
    }
}

/// Model ids are passed on the command line; keep them to a safe alphabet.
fn validate_model_id(model: &str) -> Result<(), HarnessError> {
    let ok = !model.is_empty()
        && model.len() <= 200
        && !model.starts_with('-')
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/' | '@'));
    if ok {
        Ok(())
    } else {
        Err(HarnessError::InvalidModel(model.to_string()))
    }
}

/// OpenRouter stealth models log prompts for training (ADR 0001, consequence 5).
fn is_stealth(model: &str) -> bool {
    model
        .split('/')
        .next()
        .map(|first| first.eq_ignore_ascii_case("stealth"))
        .unwrap_or(false)
}

/// Resolve the omp model for the configured LLM mode.
///
/// `fallback_key` supplies a key from the app's key store when the mode
/// itself carries none.
pub fn select_model(
    mode: &LLMMode,
    fallback_key: impl Fn(&ApiProvider) -> Option<String>,
) -> Result<OmpModel, HarnessError> {
    let (provider, api_key, model) = match mode {
        LLMMode::Disabled => return Err(HarnessError::LlmDisabled),
        LLMMode::Local { .. } => return Err(HarnessError::UnsupportedLocalModel),
        LLMMode::External {
            provider,
            api_key,
            model,
        } => (provider, api_key, model.trim()),
    };
    let mapping = mapping(provider)?;
    validate_model_id(model)?;
    if is_stealth(model) {
        return Err(HarnessError::DisallowedModel(model.to_string()));
    }

    let mut env = Vec::new();
    match mapping.key_var {
        Some(var) => {
            let key = Some(api_key.trim().to_string())
                .filter(|k| !k.is_empty())
                .or_else(|| fallback_key(provider).map(|k| k.trim().to_string()))
                .filter(|k| !k.is_empty())
                .ok_or(HarnessError::MissingApiKey(mapping.label))?;
            env.push((var.to_string(), EnvValue::Secret(Secret::new(key))));
        }
        None => {
            let host = std::env::var("OLLAMA_HOST")
                .ok()
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| OLLAMA_DEFAULT_HOST.to_string());
            env.push(("OLLAMA_HOST".to_string(), EnvValue::Plain(host)));
        }
    }

    Ok(OmpModel {
        model_arg: format!("{}/{}", mapping.omp_provider, model),
        provider_label: mapping.label,
        is_local: mapping.key_var.is_none(),
        env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{DeviceType, LocalModel, QuantizationType};

    fn external(provider: ApiProvider, key: &str, model: &str) -> LLMMode {
        LLMMode::External {
            provider,
            api_key: key.into(),
            model: model.into(),
        }
    }

    #[test]
    fn providers_map_to_omp_model_and_key_variables() {
        let cases = [
            (
                ApiProvider::OpenRouter,
                "anthropic/claude-haiku-4.5",
                "openrouter/anthropic/claude-haiku-4.5",
                "OPENROUTER_API_KEY",
            ),
            (
                ApiProvider::Anthropic,
                "claude-haiku-4-5",
                "anthropic/claude-haiku-4-5",
                "ANTHROPIC_API_KEY",
            ),
            (
                ApiProvider::OpenAI,
                "gpt-5-mini",
                "openai/gpt-5-mini",
                "OPENAI_API_KEY",
            ),
            (
                ApiProvider::Google,
                "gemini-2.5-flash",
                "google/gemini-2.5-flash",
                "GEMINI_API_KEY",
            ),
            (ApiProvider::Grok, "grok-4", "xai/grok-4", "XAI_API_KEY"),
        ];
        for (provider, model, arg, var) in cases {
            let selected = select_model(&external(provider, "sk-test", model), |_| None).unwrap();
            assert_eq!(selected.model_arg, arg);
            assert_eq!(selected.env.len(), 1);
            assert_eq!(selected.env[0].0, var);
            assert_eq!(selected.env[0].1.as_str(), "sk-test");
            assert!(!selected.is_local);
        }
    }

    #[test]
    fn ollama_needs_no_key_and_sets_a_host() {
        let selected =
            select_model(&external(ApiProvider::Ollama, "", "qwen3:4b"), |_| None).unwrap();
        assert_eq!(selected.model_arg, "ollama/qwen3:4b");
        assert_eq!(selected.env[0].0, "OLLAMA_HOST");
        assert!(selected.is_local);
    }

    #[test]
    fn keys_fall_back_to_the_key_store_and_are_required() {
        let selected = select_model(&external(ApiProvider::Anthropic, " ", "claude-x"), |_| {
            Some("from-store".into())
        })
        .unwrap();
        assert_eq!(selected.env[0].1.as_str(), "from-store");
        assert!(matches!(
            select_model(&external(ApiProvider::Anthropic, "", "claude-x"), |_| None),
            Err(HarnessError::MissingApiKey("Anthropic"))
        ));
    }

    #[test]
    fn unsupported_modes_and_models_are_typed_errors() {
        assert!(matches!(
            select_model(&LLMMode::Disabled, |_| None),
            Err(HarnessError::LlmDisabled)
        ));
        let local = LLMMode::Local {
            model: LocalModel::Phi3Mini,
            device: DeviceType::Cpu,
            quantization: QuantizationType::Q4,
        };
        assert!(matches!(
            select_model(&local, |_| None),
            Err(HarnessError::UnsupportedLocalModel)
        ));
        assert!(matches!(
            select_model(&external(ApiProvider::Together, "k", "m"), |_| None),
            Err(HarnessError::UnsupportedProvider(_))
        ));
        assert!(matches!(
            select_model(
                &external(ApiProvider::OpenRouter, "k", "stealth/space-bunny-alpha"),
                |_| None
            ),
            Err(HarnessError::DisallowedModel(_))
        ));
        assert!(matches!(
            select_model(
                &external(ApiProvider::OpenAI, "k", "--config=/tmp/x"),
                |_| None
            ),
            Err(HarnessError::InvalidModel(_))
        ));
        assert!(matches!(
            select_model(&external(ApiProvider::OpenAI, "k", "gpt 5"), |_| None),
            Err(HarnessError::InvalidModel(_))
        ));
    }

    #[test]
    fn secrets_never_print() {
        let s = Secret::new("sk-live-123");
        assert_eq!(format!("{s:?} {s}"), "[REDACTED] [REDACTED]");
        let v = EnvValue::Secret(s);
        assert!(!format!("{v:?}").contains("sk-live"));
    }
}
