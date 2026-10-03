//! App settings kept by the backend in `<app_data_dir>/app_settings.json`.
//!
//! Two groups with different owners:
//! - **Preferences** (theme, how many passages a search returns): the user
//!   and the agent may change them; the agent's `update_setting` can change
//!   only the keys in [`AGENT_WRITABLE`].
//! - **Policy** (Local-only mode, web access): only the user may change it,
//!   through [`set_app_policy`]. The agent can read it so it can explain why
//!   a tool is unavailable.
//!
//! The UI applies the stored preferences at startup and on every
//! [`APP_SETTINGS_CHANGED_EVENT`]; on first run it seeds them once from the
//! values it kept in local storage before this store existed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shodh_rag::audit::{AuditEventType, AuditRecord};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::audit_commands::AuditState;

pub const SETTINGS_FILE: &str = "app_settings.json";

/// Emitted with the full [`AppSettings`] after every change.
pub const APP_SETTINGS_CHANGED_EVENT: &str = "app-settings-changed";

/// Passages per search the user may choose (the search tool's own bounds).
pub const MIN_SEARCH_RESULTS: u32 = 3;
pub const MAX_SEARCH_RESULTS: u32 = 20;
const DEFAULT_SEARCH_RESULTS: u32 = 8;

static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("{key}: {reason}")]
    InvalidValue { key: String, reason: String },
    #[error("Settings file error ({path}): {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Settings file {path} is not valid: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    #[default]
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Preferences {
    pub theme: Theme,
    /// Passages each document search returns by default.
    pub search_max_results: u32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            search_max_results: DEFAULT_SEARCH_RESULTS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Policy {
    /// Nothing leaves this computer: web tools are off and the agent may
    /// only use a local model.
    pub local_only: bool,
    /// The agent may search the web and fetch pages (ignored in Local-only
    /// mode).
    pub web_access: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            local_only: false,
            web_access: true,
        }
    }
}

impl Policy {
    /// Why the agent may not use the web right now, if it may not.
    pub fn web_block_reason(&self) -> Option<&'static str> {
        if self.local_only {
            Some("Local-only mode is on, so nothing may leave this computer. The user can turn it off in Settings → Privacy.")
        } else if !self.web_access {
            Some("Web access for the agent is turned off. The user can turn it on in Settings → Privacy.")
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub preferences: Preferences,
    pub policy: Policy,
    /// The UI has copied its earlier local-storage preferences here.
    pub seeded: bool,
}

/// A preference the agent may change. Everything else is either policy
/// (user-only) or lives outside this store; see [`AGENT_DENIED`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKey {
    Theme,
    SearchMaxResults,
}

impl SettingKey {
    pub fn as_str(self) -> &'static str {
        match self {
            SettingKey::Theme => "theme",
            SettingKey::SearchMaxResults => "search_max_results",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        AGENT_WRITABLE.iter().copied().find(|k| k.as_str() == key)
    }

    pub fn describe(self) -> &'static str {
        match self {
            SettingKey::Theme => "colour theme: \"light\" or \"dark\"",
            SettingKey::SearchMaxResults => {
                "passages a document search returns by default: 3 to 20"
            }
        }
    }
}

/// The only settings `update_setting` can change.
pub const AGENT_WRITABLE: [SettingKey; 2] = [SettingKey::Theme, SettingKey::SearchMaxResults];

/// Settings that exist in the app but are never readable or writable by the
/// agent's settings tools, each with the reason. `update_setting` refuses
/// these by name (in addition to its schema only listing [`AGENT_WRITABLE`]),
/// so a prompt-injected request gets a clear refusal rather than a
/// best-effort match.
pub const AGENT_DENIED: [(&str, &str); 10] = [
    ("api_keys", "API keys are secrets; a manipulated agent could leak or replace them."),
    ("provider", "Switching the model provider changes who receives the user's documents."),
    ("model", "Switching the model changes who receives the user's documents and what it costs."),
    ("allow_stealth_models", "Stealth models may log prompts for training; only the user may accept that."),
    ("auto_approve_writes", "An agent that relaxes its own approvals makes the approval gate meaningless."),
    ("approvals", "The agent must never approve its own actions."),
    ("audit_retention_days", "Shortening retention deletes the audit trail of what the agent did."),
    ("audit_export", "The audit log is the user's record of the agent; the agent must not move or prune it."),
    ("local_only", "Local-only mode decides whether data may leave this computer; only the user may widen that."),
    ("web_access", "Web access decides whether the agent may contact the internet; only the user may grant it."),
];

/// Why `key` is withheld from the agent, if it is.
pub fn denied_reason(key: &str) -> Option<&'static str> {
    let key = key.trim().to_ascii_lowercase();
    AGENT_DENIED
        .iter()
        .find(|(k, _)| key == *k || key.starts_with(&format!("{k}.")))
        .map(|(_, why)| *why)
}

/// The settings file of one app data directory.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn in_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(SETTINGS_FILE),
        }
    }

    fn read_unlocked(&self) -> Result<AppSettings, SettingsError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map_err(|source| SettingsError::Parse {
                path: self.path.clone(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppSettings::default()),
            Err(source) => Err(SettingsError::Io {
                path: self.path.clone(),
                source,
            }),
        }
    }

    pub fn load(&self) -> Result<AppSettings, SettingsError> {
        let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        self.read_unlocked()
    }

    /// Apply `change`, validate the result and save it.
    pub fn update<R>(
        &self,
        change: impl FnOnce(&mut AppSettings) -> Result<R, SettingsError>,
    ) -> Result<(AppSettings, R), SettingsError> {
        let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut settings = self.read_unlocked()?;
        let result = change(&mut settings)?;
        validate(&settings.preferences)?;
        let io = |source| SettingsError::Io {
            path: self.path.clone(),
            source,
        };
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(io)?;
        }
        let text =
            serde_json::to_string_pretty(&settings).map_err(|source| SettingsError::Parse {
                path: self.path.clone(),
                source,
            })?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, text).map_err(io)?;
        fs::rename(&tmp, &self.path).map_err(io)?;
        Ok((settings, result))
    }
}

fn validate(prefs: &Preferences) -> Result<(), SettingsError> {
    if !(MIN_SEARCH_RESULTS..=MAX_SEARCH_RESULTS).contains(&prefs.search_max_results) {
        return Err(SettingsError::InvalidValue {
            key: SettingKey::SearchMaxResults.as_str().to_string(),
            reason: format!("must be between {MIN_SEARCH_RESULTS} and {MAX_SEARCH_RESULTS}"),
        });
    }
    Ok(())
}

/// Current value of an agent-writable preference.
pub fn preference_value(prefs: &Preferences, key: SettingKey) -> Value {
    match key {
        SettingKey::Theme => json!(prefs.theme),
        SettingKey::SearchMaxResults => json!(prefs.search_max_results),
    }
}

/// Set one agent-writable preference from JSON. Returns the old value.
pub fn set_preference(
    prefs: &mut Preferences,
    key: SettingKey,
    value: &Value,
) -> Result<Value, SettingsError> {
    let old = preference_value(prefs, key);
    let invalid = |reason: &str| SettingsError::InvalidValue {
        key: key.as_str().to_string(),
        reason: reason.to_string(),
    };
    match key {
        SettingKey::Theme => {
            prefs.theme = serde_json::from_value(value.clone())
                .map_err(|_| invalid("must be \"light\" or \"dark\""))?;
        }
        SettingKey::SearchMaxResults => {
            let n = value
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| invalid("must be a whole number"))?;
            prefs.search_max_results = n;
        }
    }
    validate(prefs)?;
    Ok(old)
}

/// `settings_change` audit payload for a preference change.
pub fn preference_change_payload(key: SettingKey, old: &Value, new: &Value, via: &str) -> Value {
    json!({
        "action": "preference_change",
        "key": key.as_str(),
        "old": old,
        "new": new,
        "via": via,
    })
}

// ── Commands ─────────────────────────────────────────────────────

fn store(app: &AppHandle) -> Result<SettingsStore, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;
    Ok(SettingsStore::in_dir(&dir))
}

/// Tell open windows the settings changed.
pub fn broadcast(app: &AppHandle, settings: &AppSettings) {
    if let Err(e) = app.emit(APP_SETTINGS_CHANGED_EVENT, settings) {
        tracing::warn!("Failed to emit {}: {}", APP_SETTINGS_CHANGED_EVENT, e);
    }
}

#[tauri::command]
pub async fn get_app_settings(app: AppHandle) -> Result<AppSettings, String> {
    store(&app)?.load().map_err(|e| e.to_string())
}

/// Change preferences from the UI. With `seed`, the values come from the
/// UI's local storage and are applied only if the store was never seeded.
#[tauri::command]
pub async fn update_app_preferences(
    app: AppHandle,
    preferences: Preferences,
    seed: Option<bool>,
    audit: State<'_, AuditState>,
) -> Result<AppSettings, String> {
    let seed = seed.unwrap_or(false);
    let (settings, before) = store(&app)?
        .update(|s| {
            let before = s.preferences.clone();
            if !seed || !s.seeded {
                s.preferences = preferences;
            }
            s.seeded = true;
            Ok(before)
        })
        .map_err(|e| e.to_string())?;
    if !seed {
        for key in AGENT_WRITABLE {
            let old = preference_value(&before, key);
            let new = preference_value(&settings.preferences, key);
            if old != new {
                audit.record(AuditRecord::new(
                    AuditEventType::SettingsChange,
                    preference_change_payload(key, &old, &new, "ui"),
                ));
            }
        }
    }
    broadcast(&app, &settings);
    Ok(settings)
}

/// Change Local-only mode or web access. User-only: no agent tool calls
/// this, and the agent's settings tools refuse these keys.
#[tauri::command]
pub async fn set_app_policy(
    app: AppHandle,
    policy: Policy,
    audit: State<'_, AuditState>,
) -> Result<AppSettings, String> {
    let (settings, before) = store(&app)?
        .update(|s| Ok(std::mem::replace(&mut s.policy, policy)))
        .map_err(|e| e.to_string())?;
    if before != settings.policy {
        audit.record(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({
                "action": "policy_change",
                "old": before,
                "new": settings.policy,
                "via": "ui",
            }),
        ));
    }
    broadcast(&app, &settings);
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::in_dir(dir.path());
        let s = store.load().unwrap();
        assert_eq!(s.preferences.theme, Theme::Dark);
        assert!(s.policy.web_access && !s.policy.local_only);
        assert!(s.policy.web_block_reason().is_none());
        let (saved, ()) = store
            .update(|s| {
                s.policy.local_only = true;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.load().unwrap(), saved);
        assert!(saved
            .policy
            .web_block_reason()
            .unwrap()
            .contains("Local-only"));
    }

    #[test]
    fn preferences_validate() {
        let mut prefs = Preferences::default();
        assert_eq!(
            set_preference(&mut prefs, SettingKey::Theme, &json!("light")).unwrap(),
            json!("dark")
        );
        assert_eq!(prefs.theme, Theme::Light);
        assert!(set_preference(&mut prefs, SettingKey::Theme, &json!("sepia")).is_err());
        assert!(set_preference(&mut prefs, SettingKey::SearchMaxResults, &json!(2)).is_err());
        assert!(set_preference(&mut prefs, SettingKey::SearchMaxResults, &json!(21)).is_err());
        assert!(set_preference(&mut prefs, SettingKey::SearchMaxResults, &json!("10")).is_err());
        set_preference(&mut prefs, SettingKey::SearchMaxResults, &json!(12)).unwrap();
        assert_eq!(prefs.search_max_results, 12);

        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::in_dir(dir.path());
        let bad = store.update(|s| {
            s.preferences.search_max_results = 99;
            Ok(())
        });
        assert!(bad.is_err());
        assert_eq!(
            store.load().unwrap(),
            AppSettings::default(),
            "nothing saved"
        );
    }

    #[test]
    fn denied_keys_are_never_writable() {
        for (key, why) in AGENT_DENIED {
            assert!(
                SettingKey::parse(key).is_none(),
                "{key} must not be writable"
            );
            assert!(!why.is_empty());
            assert_eq!(denied_reason(key), Some(why));
        }
        assert!(denied_reason("api_keys.openrouter").is_some());
        assert!(denied_reason("LOCAL_ONLY").is_some());
        assert!(denied_reason("theme").is_none());
        for key in AGENT_WRITABLE {
            assert!(denied_reason(key.as_str()).is_none());
        }
    }
}
