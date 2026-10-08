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

/// Credential-store user of the audit database key (hex-encoded 256-bit key).
/// Not a provider id: `is_known_provider` rejects it, so the provider
/// commands can never read, overwrite or delete it.
pub const AUDIT_DB_KEY_USER: &str = "audit-db-key";

/// Load a non-provider secret (e.g. [`AUDIT_DB_KEY_USER`]). `Ok(None)` means
/// none is stored.
pub fn load_secret(user: &str) -> Result<Option<String>, String> {
    match entry(user)?.get_password() {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!(
            "Failed to read {user} from the OS credential store: {e}"
        )),
    }
}

/// Store a non-provider secret, replacing any existing value.
pub fn store_secret(user: &str, value: &str) -> Result<(), String> {
    entry(user)?
        .set_password(value)
        .map_err(|e| format!("Failed to save {user} to the OS credential store: {e}"))
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

/// Where provider keys are kept. [`OsVault`] is the OS credential store;
/// tests use an in-memory vault (never the real one).
pub trait KeyVault {
    fn load(&self, provider: &str) -> Result<Option<String>, String>;
    fn store(&self, provider: &str, key: &str) -> Result<(), String>;
}

/// The OS credential store (Windows Credential Manager, macOS Keychain,
/// Secret Service), under [`KEYRING_SERVICE`].
pub struct OsVault;

impl KeyVault for OsVault {
    fn load(&self, provider: &str) -> Result<Option<String>, String> {
        load(provider)
    }
    fn store(&self, provider: &str, key: &str) -> Result<(), String> {
        store(provider, key)
    }
}

/// Provider key environment variables, by the key store's provider id.
pub const ENV_KEY_VARS: &[(&str, &[&str])] = &[
    ("openrouter", &["OPENROUTER_API_KEY"]),
    ("anthropic", &["ANTHROPIC_API_KEY"]),
    ("openai", &["OPENAI_API_KEY"]),
    ("google", &["GEMINI_API_KEY", "GOOGLE_API_KEY"]),
    ("grok", &["XAI_API_KEY"]),
];

/// The first non-empty environment key of `provider`.
pub fn env_key(provider: &str, env: &impl Fn(&str) -> Option<String>) -> Option<String> {
    ENV_KEY_VARS
        .iter()
        .find(|(id, _)| *id == provider)
        .and_then(|(_, vars)| {
            vars.iter().find_map(|var| {
                env(var)
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            })
        })
}

/// One-time copy of provider keys found in the environment
/// (`OPENROUTER_API_KEY`, …) into the vault, so the app keeps them after
/// the variables are gone. A key already in the vault is never replaced.
/// Returns the providers whose key was copied. The environment still wins
/// at run time (an override for power users); the caller records that the
/// migration ran so a key the person removes is not copied back.
pub fn migrate_env_keys(
    vault: &impl KeyVault,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<&'static str>, String> {
    let mut copied = Vec::new();
    let mut failures = Vec::new();
    for (provider, _) in ENV_KEY_VARS {
        let Some(key) = env_key(provider, &env) else {
            continue;
        };
        match vault.load(provider) {
            Ok(Some(_)) => continue,
            Ok(None) => match vault.store(provider, &key) {
                Ok(()) => copied.push(*provider),
                Err(e) => failures.push(e),
            },
            Err(e) => failures.push(e),
        }
    }
    if failures.is_empty() {
        Ok(copied)
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(test)]
pub(crate) mod test_vault {
    use super::KeyVault;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// An in-memory vault for tests.
    #[derive(Default)]
    pub struct MemoryVault {
        pub keys: Mutex<HashMap<String, String>>,
        pub fail_store: bool,
    }

    impl KeyVault for MemoryVault {
        fn load(&self, provider: &str) -> Result<Option<String>, String> {
            Ok(self
                .keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(provider)
                .cloned())
        }
        fn store(&self, provider: &str, key: &str) -> Result<(), String> {
            if self.fail_store {
                return Err(format!("store refused for {provider}"));
            }
            self.keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(provider.to_string(), key.to_string());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_vault::MemoryVault;
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn environment_keys_are_copied_once_and_never_replace_a_stored_key() {
        let vault = MemoryVault::default();
        vault.store("openai", "sk-proj-stored").unwrap();
        let copied = migrate_env_keys(
            &vault,
            env(&[
                ("OPENROUTER_API_KEY", " sk-or-env "),
                ("OPENAI_API_KEY", "sk-proj-env"),
                ("GOOGLE_API_KEY", "AIza-env"),
                ("XAI_API_KEY", "  "),
            ]),
        )
        .unwrap();
        assert_eq!(copied, vec!["openrouter", "google"]);
        assert_eq!(
            vault.load("openrouter").unwrap().as_deref(),
            Some("sk-or-env")
        );
        assert_eq!(
            vault.load("openai").unwrap().as_deref(),
            Some("sk-proj-stored"),
            "a stored key is kept"
        );
        assert_eq!(vault.load("google").unwrap().as_deref(), Some("AIza-env"));
        assert_eq!(
            vault.load("grok").unwrap(),
            None,
            "blank values are ignored"
        );
        // A second run copies nothing new.
        assert!(
            migrate_env_keys(&vault, env(&[("OPENROUTER_API_KEY", "sk-or-env")]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_vault_failure_is_reported() {
        let vault = MemoryVault {
            fail_store: true,
            ..MemoryVault::default()
        };
        let err =
            migrate_env_keys(&vault, env(&[("OPENROUTER_API_KEY", "sk-or-secret")])).unwrap_err();
        assert!(err.contains("openrouter"));
        assert!(!err.contains("sk-or-secret"), "never the key");
    }

    #[test]
    fn environment_keys_are_read_in_order() {
        let e = env(&[("GEMINI_API_KEY", "g1"), ("GOOGLE_API_KEY", "g2")]);
        assert_eq!(env_key("google", &e).as_deref(), Some("g1"));
        assert_eq!(env_key("openai", &e), None);
        for (provider, _) in ENV_KEY_VARS {
            assert!(is_known_provider(provider));
        }
    }

    #[test]
    fn provider_ids_are_unique_and_known() {
        for (i, provider) in KEY_PROVIDERS.iter().enumerate() {
            assert!(is_known_provider(provider));
            assert!(!KEY_PROVIDERS[i + 1..].contains(provider));
        }
        assert!(!is_known_provider("ollama"));
        assert!(!is_known_provider(""));
        assert!(!is_known_provider(AUDIT_DB_KEY_USER));
    }
}
