//! The language model for answer runs and question generation, chosen with
//! environment variables only (the eval never reads the app's settings or
//! key store):
//!
//! - `SHODH_EVAL_PROVIDER`: `anthropic`, `openai`, `openrouter`, `google`,
//!   `xai`, `ollama` or `lm-studio`;
//! - `SHODH_EVAL_MODEL`: the provider's model id;
//! - the provider's usual key variable (`ANTHROPIC_API_KEY`,
//!   `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`);
//!   local providers need none (`OLLAMA_HOST` / `LM_STUDIO_BASE_URL` as in
//!   the app).

use std::fmt;

use anyhow::{anyhow, bail, Result};
use shodh_rag::llm::{ApiProvider, LLMMode};

pub const PROVIDER_ENV: &str = "SHODH_EVAL_PROVIDER";
pub const MODEL_ENV: &str = "SHODH_EVAL_MODEL";

/// A configured model. The key never appears in `Debug` output.
#[derive(Clone)]
pub struct ProviderChoice {
    pub name: String,
    pub provider: ApiProvider,
    pub model: String,
    api_key: String,
    /// Prompts stay on this computer.
    pub is_local: bool,
}

impl fmt::Debug for ProviderChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderChoice")
            .field("name", &self.name)
            .field("model", &self.model)
            .field("api_key", &"[REDACTED]")
            .field("is_local", &self.is_local)
            .finish()
    }
}

/// Provider name -> (provider, key variable, local).
fn lookup_provider(name: &str) -> Option<(ApiProvider, Option<&'static str>, bool)> {
    Some(match name {
        "anthropic" => (ApiProvider::Anthropic, Some("ANTHROPIC_API_KEY"), false),
        "openai" => (ApiProvider::OpenAI, Some("OPENAI_API_KEY"), false),
        "openrouter" => (ApiProvider::OpenRouter, Some("OPENROUTER_API_KEY"), false),
        "google" => (ApiProvider::Google, Some("GEMINI_API_KEY"), false),
        "xai" => (ApiProvider::Grok, Some("XAI_API_KEY"), false),
        "ollama" => (ApiProvider::Ollama, None, true),
        "lm-studio" => (ApiProvider::LmStudio, None, true),
        _ => return None,
    })
}

impl ProviderChoice {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] over any variable source (tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let value = |name: &str| {
            get(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let name = value(PROVIDER_ENV)
            .ok_or_else(|| anyhow!("set {PROVIDER_ENV} (and {MODEL_ENV}) to choose the model"))?
            .to_lowercase();
        let (provider, key_var, is_local) = lookup_provider(&name).ok_or_else(|| {
            anyhow!(
                "{PROVIDER_ENV}={name} is not supported; use anthropic, openai, openrouter, \
                 google, xai, ollama or lm-studio"
            )
        })?;
        let model = value(MODEL_ENV).ok_or_else(|| anyhow!("set {MODEL_ENV} to the model id"))?;
        let api_key = match key_var {
            Some(var) => match value(var) {
                Some(key) => key,
                None => bail!("{name} needs its API key in {var}"),
            },
            None => String::new(),
        };
        Ok(Self {
            name,
            provider,
            model,
            api_key,
            is_local,
        })
    }

    pub fn mode(&self) -> LLMMode {
        LLMMode::External {
            provider: self.provider.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
        }
    }

    /// "anthropic/claude-haiku-4-5 (cloud)" for notices and reports.
    pub fn describe(&self) -> String {
        let place = if self.is_local {
            "on this computer"
        } else {
            "cloud"
        };
        format!("{}/{} ({place})", self.name, self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn cloud_providers_need_their_key() {
        let missing = ProviderChoice::from_lookup(vars(&[
            (PROVIDER_ENV, "Anthropic"),
            (MODEL_ENV, "claude-haiku-4-5"),
        ]));
        assert!(missing
            .unwrap_err()
            .to_string()
            .contains("ANTHROPIC_API_KEY"));
        let choice = ProviderChoice::from_lookup(vars(&[
            (PROVIDER_ENV, "anthropic"),
            (MODEL_ENV, "claude-haiku-4-5"),
            ("ANTHROPIC_API_KEY", "sk-secret"),
        ]))
        .unwrap();
        assert_eq!(choice.describe(), "anthropic/claude-haiku-4-5 (cloud)");
        assert!(!format!("{choice:?}").contains("sk-secret"));
        assert!(
            matches!(choice.mode(), LLMMode::External { api_key, .. } if api_key == "sk-secret")
        );
    }

    #[test]
    fn local_providers_need_no_key_and_unknown_ones_fail() {
        let local =
            ProviderChoice::from_lookup(vars(&[(PROVIDER_ENV, "ollama"), (MODEL_ENV, "qwen3:8b")]))
                .unwrap();
        assert!(local.is_local);
        assert!(
            ProviderChoice::from_lookup(vars(&[(PROVIDER_ENV, "acme"), (MODEL_ENV, "m")])).is_err()
        );
        assert!(ProviderChoice::from_lookup(vars(&[(PROVIDER_ENV, "ollama")])).is_err());
        assert!(ProviderChoice::from_lookup(vars(&[])).is_err());
    }
}
