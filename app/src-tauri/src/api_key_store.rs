//! Persistence of LLM provider API keys in the operating system's credential
//! store (Windows Credential Manager, macOS Keychain, Secret Service on Linux).
//!
//! Keys never touch the webview's storage and are never returned to the
//! frontend; the UI only learns which providers have a key configured.

use keyring::Entry;

/// Credential-store service name. Matches the Tauri bundle identifier so the
/// entries are easy to identify (and remove) in the OS credential manager.
pub const KEYRING_SERVICE: &str = "com.shodh.rag-app";

/// Provider ids that can hold a stored API key. These are the ids the
/// frontend and `switch_llm_mode` use.
pub const KEY_PROVIDERS: &[&str] = &[
    "openai",
    "anthropic",
    "openrouter",
    "kimi",
    "grok",
    "perplexity",
    "google",
    "baseten",
];

/// Returns true when `provider` is a known provider id.
pub fn is_known_provider(provider: &str) -> bool {
    KEY_PROVIDERS.contains(&provider)
}

fn entry(provider: &str) -> Result<Entry, String> {
    Entry::new(KEYRING_SERVICE, provider)
        .map_err(|e| format!("Cannot open OS credential store entry for {provider}: {e}"))
}

/// Store `api_key` for `provider` in the OS credential store, replacing any
/// existing value.
pub fn store(provider: &str, api_key: &str) -> Result<(), String> {
    entry(provider)?
        .set_password(api_key)
        .map_err(|e| format!("Failed to save {provider} API key to the OS credential store: {e}"))
}

/// Remove the stored key for `provider`. Removing a key that does not exist is
/// not an error.
pub fn remove(provider: &str) -> Result<(), String> {
    match entry(provider)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!(
            "Failed to remove {provider} API key from the OS credential store: {e}"
        )),
    }
}

/// Load the stored key for `provider`. `Ok(None)` means no key is stored.
pub fn load(provider: &str) -> Result<Option<String>, String> {
    match entry(provider)?.get_password() {
        Ok(key) if key.trim().is_empty() => Ok(None),
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!(
            "Failed to read {provider} API key from the OS credential store: {e}"
        )),
    }
}

/// Load every stored provider key. Providers whose entry cannot be read are
/// logged and skipped so one broken entry does not hide the others.
pub fn load_all() -> Vec<(&'static str, String)> {
    KEY_PROVIDERS
        .iter()
        .filter_map(|provider| match load(provider) {
            Ok(Some(key)) => Some((*provider, key)),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("{}", e);
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_are_unique_and_known() {
        for (i, provider) in KEY_PROVIDERS.iter().enumerate() {
            assert!(is_known_provider(provider));
            assert!(!KEY_PROVIDERS[i + 1..].contains(provider));
        }
        assert!(!is_known_provider("ollama"));
        assert!(!is_known_provider(""));
    }
}
