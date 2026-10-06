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
use shodh_rag::harness::model_choice::ModelPrefs;
use shodh_rag::user_memory::learn::{
    LearnCaps, LearnMode, DEFAULT_AUTO_MIN_CONFIDENCE, MAX_CALLS_PER_DAY_LIMIT,
    MAX_INPUT_CHARS_LIMIT, MAX_PROPOSALS_LIMIT,
};
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

/// Background mode (see [`crate::background`]). User-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BackgroundPrefs {
    /// Closing the main window hides it to the tray, so reminders keep ringing.
    pub close_to_tray: bool,
    /// The user has been told, once, where the window went.
    pub close_to_tray_explained: bool,
}

impl Default for BackgroundPrefs {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            close_to_tray_explained: false,
        }
    }
}

/// Long-term memory. User-only: it decides what the model is told about the user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryPrefs {
    /// At the start of each answer, recall memories relevant to the question and give
    /// them to the model.
    pub inject_memories: bool,
    /// Learn from conversations: off, ask (suggestions wait for the user) or auto.
    pub learn_mode: LearnMode,
    /// Model used for learning (a cheaper one of the configured provider); `None` uses the
    /// configured model.
    pub learn_model: Option<String>,
    /// Lowest confidence applied automatically in auto mode.
    pub auto_min_confidence: f64,
    /// Daily caps on the learning model's use.
    pub learn_caps: LearnCaps,
}

impl Default for MemoryPrefs {
    fn default() -> Self {
        Self {
            inject_memories: true,
            learn_mode: LearnMode::Ask,
            learn_model: None,
            auto_min_confidence: DEFAULT_AUTO_MIN_CONFIDENCE,
            learn_caps: LearnCaps::default(),
        }
    }
}

impl MemoryPrefs {
    /// Checks the learning settings' bounds.
    pub fn validate(&self) -> Result<(), SettingsError> {
        let invalid = |key: &str, reason: &str| SettingsError::InvalidValue {
            key: key.to_string(),
            reason: reason.to_string(),
        };
        if !self.auto_min_confidence.is_finite() || !(0.5..=1.0).contains(&self.auto_min_confidence)
        {
            return Err(invalid("auto_min_confidence", "must be between 0.5 and 1"));
        }
        let caps = &self.learn_caps;
        if !(1..=MAX_CALLS_PER_DAY_LIMIT).contains(&caps.max_calls_per_day) {
            return Err(invalid(
                "learn_caps.max_calls_per_day",
                &format!("must be between 1 and {MAX_CALLS_PER_DAY_LIMIT}"),
            ));
        }
        if !(1_000..=MAX_INPUT_CHARS_LIMIT).contains(&caps.max_input_chars_per_day) {
            return Err(invalid(
                "learn_caps.max_input_chars_per_day",
                &format!("must be between 1000 and {MAX_INPUT_CHARS_LIMIT}"),
            ));
        }
        if !(1..=MAX_PROPOSALS_LIMIT).contains(&caps.max_proposals_per_day) {
            return Err(invalid(
                "learn_caps.max_proposals_per_day",
                &format!("must be between 1 and {MAX_PROPOSALS_LIMIT}"),
            ));
        }
        if let Some(model) = &self.learn_model {
            let ok = !model.trim().is_empty()
                && model.len() <= 200
                && !model.starts_with('-')
                && !model.chars().any(|c| c.is_whitespace() || c.is_control());
            if !ok {
                return Err(invalid("learn_model", "is not a valid model id"));
            }
        }
        Ok(())
    }
}

/// How agent answers are grounded. User-only: the check exists to catch the
/// assistant, so the assistant must not be able to weaken it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnswerPrefs {
    /// When the grounding check flags statements, ask the model once to
    /// re-ground or remove them (one extra turn).
    pub auto_repair: bool,
}

impl Default for AnswerPrefs {
    fn default() -> Self {
        Self { auto_repair: true }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub preferences: Preferences,
    pub policy: Policy,
    pub background: BackgroundPrefs,
    pub memory: MemoryPrefs,
    pub answers: AnswerPrefs,
    /// The model picker: chosen model, fallback, recent and starred models.
    /// User-only (no keys; those stay in the OS keychain).
    pub models: ModelPrefs,
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
pub const AGENT_DENIED: [(&str, &str); 21] = [
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
    ("close_to_tray", "Whether Shodh keeps running after its window closes is the user's call about their computer."),
    ("start_with_windows", "Adding a program to Windows startup changes the user's system; only the user may do that."),
    ("inject_memories", "Whether remembered facts about the user are given to the model is the user's privacy decision."),
    ("learn_mode", "Whether the assistant learns memories from conversations, and whether it may store them without asking, is the user's decision."),
    ("learn_model", "The learning model receives what the user says; choosing who receives it is the user's decision."),
    ("auto_min_confidence", "Lowering the bar for storing memories without asking weakens the user's approval."),
    ("learn_caps", "The daily limits bound what learning costs; only the user may raise them."),
    ("auto_repair", "Re-checking flagged statements guards the assistant's own answers; only the user may turn it off."),
    ("models", "The model picker's settings (chosen and fallback models, stealth consent) decide who receives the user's documents; only the user may change them."),
    ("fallback_model", "The fallback model receives the user's documents when the main one fails; choosing who receives them is the user's decision."),
    ("always_fall_back", "Switching models without asking changes who receives the user's documents; only the user may allow that."),
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

/// Preference fields the UI changes; absent fields stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreferencesPatch {
    pub theme: Option<Theme>,
    pub search_max_results: Option<u32>,
}

impl PreferencesPatch {
    pub fn apply(&self, prefs: &mut Preferences) {
        if let Some(theme) = self.theme {
            prefs.theme = theme;
        }
        if let Some(n) = self.search_max_results {
            prefs.search_max_results = n;
        }
    }
}

/// Change preferences from the UI. With `seed`, the values come from the
/// UI's local storage and are applied only if the store was never seeded.
#[tauri::command]
pub async fn update_app_preferences(
    app: AppHandle,
    preferences: PreferencesPatch,
    seed: Option<bool>,
    audit: State<'_, AuditState>,
) -> Result<AppSettings, String> {
    let seed = seed.unwrap_or(false);
    let (settings, before) = store(&app)?
        .update(|s| {
            let before = s.preferences.clone();
            if !seed || !s.seeded {
                preferences.apply(&mut s.preferences);
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

/// Change the memory preferences. User-only: no agent tool calls this, and the
/// agent's settings tools refuse `inject_memories`.
#[tauri::command]
pub async fn set_memory_preferences(
    app: AppHandle,
    memory: MemoryPrefs,
    audit: State<'_, AuditState>,
) -> Result<AppSettings, String> {
    let memory = MemoryPrefs {
        learn_model: memory
            .learn_model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty()),
        ..memory
    };
    memory.validate().map_err(|e| e.to_string())?;
    let (settings, before) = store(&app)?
        .update(|s| Ok(std::mem::replace(&mut s.memory, memory)))
        .map_err(|e| e.to_string())?;
    if before != settings.memory {
        audit.record(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({
                "action": "memory_preferences_change",
                "old": before,
                "new": settings.memory,
                "via": "ui",
            }),
        ));
    }
    broadcast(&app, &settings);
    Ok(settings)
}

/// Change how answers are grounded. User-only: no agent tool calls this, and
/// the agent's settings tools refuse `auto_repair`.
#[tauri::command]
pub async fn set_answer_preferences(
    app: AppHandle,
    answers: AnswerPrefs,
    audit: State<'_, AuditState>,
) -> Result<AppSettings, String> {
    let (settings, before) = store(&app)?
        .update(|s| Ok(std::mem::replace(&mut s.answers, answers)))
        .map_err(|e| e.to_string())?;
    if before != settings.answers {
        audit.record(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({
                "action": "answer_preferences_change",
                "old": before,
                "new": settings.answers,
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
    fn answer_preferences_default_to_auto_repair_and_are_user_only() {
        assert!(AppSettings::default().answers.auto_repair);
        let old: AppSettings = serde_json::from_str(r#"{"preferences":{"theme":"dark"}}"#).unwrap();
        assert!(
            old.answers.auto_repair,
            "settings from before the field get the default"
        );
        let off: AppSettings = serde_json::from_str(r#"{"answers":{"autoRepair":false}}"#).unwrap();
        assert!(!off.answers.auto_repair);
        assert!(denied_reason("auto_repair").is_some());
        assert!(SettingKey::parse("auto_repair").is_none());
    }

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
    fn the_model_choice_is_saved_without_keys_and_withheld_from_the_agent() {
        use shodh_rag::harness::model_catalog::{ModelRef, ProviderId};
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::in_dir(dir.path());
        assert_eq!(store.load().unwrap().models, ModelPrefs::default());
        let chosen = ModelRef::new(ProviderId::OpenRouter, "anthropic/claude-haiku-4.5");
        store
            .update(|s| {
                s.models.chosen = Some(chosen.clone());
                s.models.remember(&chosen);
                s.models.always_fall_back = true;
                Ok(())
            })
            .unwrap();
        let reloaded = SettingsStore::in_dir(dir.path()).load().unwrap();
        assert_eq!(reloaded.models.chosen, Some(chosen.clone()));
        assert_eq!(reloaded.models.recent, vec![chosen]);
        assert!(reloaded.models.always_fall_back);
        let text = std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(text.contains("\"models\""));
        assert!(
            !text.to_ascii_lowercase().contains("key\":"),
            "no key is stored: {text}"
        );
        // Settings written before the picker existed load with an empty model section.
        let old: AppSettings = serde_json::from_str(r#"{"seeded": true}"#).unwrap();
        assert_eq!(old.models, ModelPrefs::default());
        for key in [
            "models",
            "models.chosen",
            "fallback_model",
            "always_fall_back",
            "model",
        ] {
            assert!(denied_reason(key).is_some(), "{key} must be user-only");
        }
    }

    #[test]
    fn settings_without_background_mode_default_to_close_to_tray() {
        let old: AppSettings =
            serde_json::from_str(r#"{"preferences": {"theme": "light"}, "seeded": true}"#).unwrap();
        assert!(old.background.close_to_tray);
        assert!(!old.background.close_to_tray_explained);
        assert_eq!(old.preferences.theme, Theme::Light);
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
    fn memory_preferences_default_to_asking_and_validate_their_bounds() {
        let old: AppSettings =
            serde_json::from_str(r#"{"memory": {"injectMemories": false}}"#).unwrap();
        assert!(!old.memory.inject_memories);
        assert_eq!(old.memory.learn_mode, LearnMode::Ask);
        assert!(old.memory.validate().is_ok());
        let mut prefs = MemoryPrefs::default();
        prefs.auto_min_confidence = 0.2;
        assert!(prefs.validate().is_err());
        prefs.auto_min_confidence = 0.9;
        prefs.learn_caps.max_calls_per_day = 0;
        assert!(prefs.validate().is_err());
        prefs.learn_caps = LearnCaps::default();
        prefs.learn_model = Some("bad model".into());
        assert!(prefs.validate().is_err());
        prefs.learn_model = Some("claude-haiku-4-5".into());
        assert!(prefs.validate().is_ok());
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
