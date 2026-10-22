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
use tauri::{AppHandle, Emitter, State};

use crate::audit_commands::AuditState;
use crate::durable_file::{self, Loaded, Origin, ReadFailure};
use crate::store_notices::{self, NoticeKind, StoreNotice};

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
    #[error("Settings file {path} could not be read: {message}")]
    Unreadable { path: PathBuf, message: String },
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
    /// Minutes an agent runtime (one omp process per conversation, hundreds
    /// of megabytes each) may sit idle before it is stopped; the next
    /// question starts it again. `0` keeps runtimes running.
    pub agent_idle_minutes: u32,
}

/// Default for [`BackgroundPrefs::agent_idle_minutes`].
pub const DEFAULT_AGENT_IDLE_MINUTES: u32 = 5;
/// Longest idle period the user may choose (a day).
pub const MAX_AGENT_IDLE_MINUTES: u32 = 24 * 60;

impl Default for BackgroundPrefs {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            close_to_tray_explained: false,
            agent_idle_minutes: DEFAULT_AGENT_IDLE_MINUTES,
        }
    }
}

impl BackgroundPrefs {
    /// How long an idle agent runtime is kept; `None`: until the app exits.
    /// A value above the limit (a hand-edited file) is capped.
    pub fn agent_idle_timeout(&self) -> Option<std::time::Duration> {
        match self.agent_idle_minutes.min(MAX_AGENT_IDLE_MINUTES) {
            0 => None,
            minutes => Some(std::time::Duration::from_secs(u64::from(minutes) * 60)),
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
    /// Learn from conversations: off, ask (suggestions wait for the user) or auto (the
    /// default; every memory saved is shown in the chat with Undo).
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
            learn_mode: LearnMode::Auto,
            learn_model: None,
            auto_min_confidence: DEFAULT_AUTO_MIN_CONFIDENCE,
            learn_caps: LearnCaps::default(),
        }
    }
}

/// The learning caps every build wrote into the settings file while the daily default
/// was 60 model calls. Learning now sends every turn about the user to the model, so
/// the default is 10 calls (free tiers allow about 50 requests a day for everything);
/// settings that still hold exactly the old default are read as the new one.
const PREVIOUS_DEFAULT_CAPS: LearnCaps = LearnCaps {
    max_calls_per_day: 60,
    max_input_chars_per_day: 400_000,
    max_proposals_per_day: 40,
};

impl MemoryPrefs {
    /// The old default caps ([`PREVIOUS_DEFAULT_CAPS`]) become the current default; caps
    /// the user changed are kept.
    fn with_current_default_caps(mut self) -> Self {
        if self.learn_caps == PREVIOUS_DEFAULT_CAPS {
            self.learn_caps = LearnCaps::default();
        }
        self
    }

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
    /// Ask the model to reason before it answers (omp `--thinking low`
    /// instead of `off`): answers start later, and the reasoning is shown.
    /// Read when a session starts; a change starts a new session for the
    /// next answer.
    pub think_before_answering: bool,
}

impl Default for AnswerPrefs {
    fn default() -> Self {
        Self {
            auto_repair: true,
            think_before_answering: false,
        }
    }
}

/// Update checks (see [`crate::updater`]). User-only: whether the app
/// contacts the update server is the user's privacy decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdatePrefs {
    /// Ask the release server for a newer version shortly after start and
    /// once a day while the app runs. Never in Local-only mode.
    pub check_automatically: bool,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        Self {
            check_automatically: true,
        }
    }
}

/// Which desktop notifications Shodh shows (see [`crate::notifications`]).
/// User-only: whether and how the app interrupts the user is their call.
/// A notification is shown only while the main window is not in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotificationPrefs {
    /// Task reminders.
    pub reminders: bool,
    /// Something waits for the user: approvals the assistant asked for.
    pub needs_you: bool,
    /// New memory suggestions (part of "needs you", off by default).
    pub memory_suggestions: bool,
    /// An answer, a folder's indexing or sync, or a result extraction finished.
    pub work_finished: bool,
    /// Indexing or a sync failed, a background model call failed, or a data
    /// file had to be restored.
    pub problems: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        Self {
            reminders: true,
            needs_you: true,
            memory_suggestions: false,
            work_finished: true,
            problems: true,
        }
    }
}

/// Evidence tables (Settings → Tables). User-only: the cap keeps table fills from
/// spending the quota the user's conversations need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TablesPrefs {
    /// The most model calls table fills make per UTC day (the provider's daily window).
    pub daily_call_cap: usize,
    /// Diagnostics: filling one paper by hand with its model calls kept (prompts and raw
    /// replies, in `shodh.db`). Off by default.
    pub diagnostics: bool,
}

impl Default for TablesPrefs {
    fn default() -> Self {
        Self {
            daily_call_cap: shodh_rag::research::tables::calls::DEFAULT_DAILY_CAP,
            diagnostics: false,
        }
    }
}

impl TablesPrefs {
    /// The cap to use: the stored one when it is in range (a file edited by hand may hold
    /// any number), else the default.
    pub fn daily_cap(&self) -> usize {
        match shodh_rag::research::tables::calls::check_cap(self.daily_call_cap) {
            Ok(()) => self.daily_call_cap,
            Err(e) => {
                tracing::warn!(cap = self.daily_call_cap, error = %e, "the stored table call cap is out of range; the default is used");
                Self::default().daily_call_cap
            }
        }
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
    pub updates: UpdatePrefs,
    /// The model picker: chosen model, fallback, recent and starred models.
    /// User-only (no keys; those stay in the OS keychain).
    pub models: ModelPrefs,
    /// Desktop notifications by category. User-only.
    pub notifications: NotificationPrefs,
    /// Evidence tables: the daily cap of table model calls. User-only.
    pub tables: TablesPrefs,
    /// The UI has copied its earlier local-storage preferences here.
    pub seeded: bool,
    /// Learning from conversations was moved, once, from Ask (the earlier default) to
    /// Automatic ([`with_automatic_learning`]). Kept outside `memory`, which the UI saves
    /// whole, so a user who chooses Ask afterwards is never moved again.
    pub learn_auto_migrated: bool,
}

/// The one-time move to automatic learning: settings that still say Ask (the default every
/// build wrote before automatic learning became the default, when every saved memory began
/// to be shown in the chat with Undo) read as Automatic, once; Off is never changed. Marks
/// the move done whatever the mode, so a later choice of Ask is kept.
fn with_automatic_learning(mut settings: AppSettings) -> AppSettings {
    if !settings.learn_auto_migrated {
        if settings.memory.learn_mode == LearnMode::Ask {
            settings.memory.learn_mode = LearnMode::Auto;
        }
        settings.learn_auto_migrated = true;
    }
    settings
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
pub const AGENT_DENIED: [(&str, &str); 25] = [
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
    ("agent_idle_minutes", "How long the agent runtime stays in memory is the user's call about their computer's resources."),
    ("inject_memories", "Whether remembered facts about the user are given to the model is the user's privacy decision."),
    ("learn_mode", "Whether the assistant learns memories from conversations, and whether it may store them without asking, is the user's decision."),
    ("learn_model", "The learning model receives what the user says; choosing who receives it is the user's decision."),
    ("auto_min_confidence", "Lowering the bar for storing memories without asking weakens the user's approval."),
    ("learn_caps", "The daily limits bound what learning costs; only the user may raise them."),
    ("auto_repair", "Re-checking flagged statements guards the assistant's own answers; only the user may turn it off."),
    ("think_before_answering", "Thinking before answering makes every answer slower and costs more tokens; only the user may turn it on."),
    ("models", "The model picker's settings (chosen and fallback models, stealth consent) decide who receives the user's documents; only the user may change them."),
    ("fallback_model", "The fallback model receives the user's documents when the main one fails; choosing who receives them is the user's decision."),
    ("always_fall_back", "Switching models without asking changes who receives the user's documents; only the user may allow that."),
    ("check_for_updates", "Whether Shodh contacts the update server, and which version of itself it runs, is the user's decision."),
    ("notifications", "Whether and when Shodh interrupts the user with desktop notifications is the user's decision."),
];

/// Why `key` is withheld from the agent, if it is.
pub fn denied_reason(key: &str) -> Option<&'static str> {
    let key = key.trim().to_ascii_lowercase();
    AGENT_DENIED
        .iter()
        .find(|(k, _)| key == *k || key.starts_with(&format!("{k}.")))
        .map(|(_, why)| *why)
}

/// Settings read back from a backup, or defaults after the file was
/// damaged. The latest change may be missing, and it may have been turning
/// Local-only mode on, so Local-only mode is on until the user sees it in
/// Settings (failing closed, like every reader of an unreadable policy).
/// Keys found in the environment are not copied into the credential store
/// again: a key the user removed stays removed.
fn recovered(mut settings: AppSettings) -> AppSettings {
    settings.policy.local_only = true;
    settings.models.env_keys_migrated = true;
    settings
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

    /// The settings and the file they came from. A damaged file never stops
    /// the app: its backup is used, else the defaults ([`recovered`]), each
    /// with a notice, and the damaged file is moved aside (never
    /// overwritten) at the next save. Fails only when the file cannot be
    /// opened at all (nothing says it is damaged).
    fn read_unlocked(&self) -> Result<Loaded<AppSettings>, SettingsError> {
        let read = durable_file::read_with_backup(
            &self.path,
            |text| {
                serde_json::from_str::<AppSettings>(text)
                    .map(|mut settings| {
                        settings.memory = settings.memory.with_current_default_caps();
                        with_automatic_learning(settings)
                    })
                    .map_err(|e| e.to_string())
            },
            // Marked moved, so the first save of a new profile records it.
            || with_automatic_learning(AppSettings::default()),
        );
        match read {
            Ok(Loaded {
                value,
                origin: Origin::Backup { problem },
            }) => {
                store_notices::report(StoreNotice::new(
                    "settings",
                    NoticeKind::Restored,
                    &self.path,
                    "Settings restored from the backup",
                    format!(
                        "The settings file could not be read ({problem}), so the copy kept before \
                         the last change is used. Local-only mode is on until you check Settings → \
                         Privacy, because the latest change may be missing."
                    ),
                ));
                Ok(Loaded {
                    value: recovered(value),
                    origin: Origin::Backup { problem },
                })
            }
            Ok(loaded) => Ok(loaded),
            Err(ReadFailure::Io { path, message }) => {
                Err(SettingsError::Unreadable { path, message })
            }
            Err(ReadFailure::Damaged {
                problem,
                backup_problem,
                ..
            }) => {
                store_notices::report(StoreNotice::new(
                    "settings",
                    NoticeKind::Reset,
                    &self.path,
                    "Settings were reset",
                    format!(
                        "The settings file could not be read ({problem}) and its backup could not \
                         be used ({backup_problem}), so the default settings are used. Local-only \
                         mode is on until you check Settings → Privacy. The damaged file is kept \
                         next to it ({SETTINGS_FILE}.damaged-…) when settings are next saved."
                    ),
                ));
                Ok(Loaded {
                    value: recovered(with_automatic_learning(AppSettings::default())),
                    origin: Origin::Damaged { problem },
                })
            }
        }
    }

    pub fn load(&self) -> Result<AppSettings, SettingsError> {
        let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        self.read_unlocked().map(|loaded| loaded.value)
    }

    /// Apply `change`, validate the result and save it (durably, keeping the
    /// previous file as its backup).
    pub fn update<R>(
        &self,
        change: impl FnOnce(&mut AppSettings) -> Result<R, SettingsError>,
    ) -> Result<(AppSettings, R), SettingsError> {
        let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Loaded {
            value: mut settings,
            origin,
        } = self.read_unlocked()?;
        let result = change(&mut settings)?;
        validate(&settings.preferences)?;
        let text =
            serde_json::to_string_pretty(&settings).map_err(|source| SettingsError::Parse {
                path: self.path.clone(),
                source,
            })?;
        durable_file::write_with_backup(&self.path, text.as_bytes(), &origin).map_err(
            |source| SettingsError::Io {
                path: self.path.clone(),
                source,
            },
        )?;
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
    let dir = crate::profile::app_data_dir(app)
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;
    Ok(SettingsStore::in_dir(&dir))
}

/// Tell open windows the settings changed.
pub fn broadcast(app: &AppHandle, settings: &AppSettings) {
    crate::notifications::settings_changed(app, &settings.notifications);
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
    fn the_table_call_cap_defaults_to_30_and_an_out_of_range_one_reads_as_the_default() {
        assert_eq!(AppSettings::default().tables.daily_call_cap, 30);
        assert!(
            !AppSettings::default().tables.diagnostics,
            "diagnostics are off"
        );
        let old: AppSettings = serde_json::from_str(r#"{"preferences":{"theme":"dark"}}"#).unwrap();
        assert_eq!(old.tables.daily_cap(), 30, "settings from before the field");
        let set: AppSettings = serde_json::from_str(r#"{"tables":{"dailyCallCap":120}}"#).unwrap();
        assert_eq!(set.tables.daily_cap(), 120);
        for out_of_range in [0, 2, 1_001] {
            let prefs = TablesPrefs {
                daily_call_cap: out_of_range,
                diagnostics: false,
            };
            assert_eq!(prefs.daily_cap(), 30, "{out_of_range}");
        }
    }

    #[test]
    fn update_checks_default_on_and_are_user_only() {
        assert!(AppSettings::default().updates.check_automatically);
        let old: AppSettings = serde_json::from_str(r#"{"preferences":{"theme":"dark"}}"#).unwrap();
        assert!(
            old.updates.check_automatically,
            "settings from before the field get the default"
        );
        let off: AppSettings =
            serde_json::from_str(r#"{"updates":{"checkAutomatically":false}}"#).unwrap();
        assert!(!off.updates.check_automatically);
        assert!(denied_reason("check_for_updates").is_some());
        assert!(SettingKey::parse("check_for_updates").is_none());
    }

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
    fn thinking_before_answering_defaults_off_persists_and_is_user_only() {
        assert!(!AppSettings::default().answers.think_before_answering);
        // Settings written before the switch existed: off, the rest kept.
        let old: AppSettings = serde_json::from_str(r#"{"answers":{"autoRepair":false}}"#).unwrap();
        assert!(!old.answers.think_before_answering);
        assert!(!old.answers.auto_repair);
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::in_dir(dir.path());
        store
            .update(|s| {
                s.answers.think_before_answering = true;
                Ok(())
            })
            .unwrap();
        let reloaded = SettingsStore::in_dir(dir.path()).load().unwrap();
        assert!(reloaded.answers.think_before_answering);
        assert!(reloaded.answers.auto_repair);
        let json = serde_json::to_value(&reloaded.answers).unwrap();
        assert_eq!(json["thinkBeforeAnswering"], true);
        assert!(denied_reason("think_before_answering").is_some());
        assert!(SettingKey::parse("think_before_answering").is_none());
    }

    #[test]
    fn the_old_default_learning_caps_are_read_as_the_new_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        let write = |caps: serde_json::Value| {
            std::fs::write(&path, json!({"memory": {"learnCaps": caps}}).to_string()).unwrap();
        };
        let store = SettingsStore::in_dir(dir.path());
        // Written by a build whose default was 60 calls a day.
        write(
            json!({"maxCallsPerDay": 60, "maxInputCharsPerDay": 400000, "maxProposalsPerDay": 40}),
        );
        let caps = store.load().unwrap().memory.learn_caps;
        assert_eq!(caps, LearnCaps::default());
        assert_eq!(caps.max_calls_per_day, 10);
        // Caps the user chose are kept, even with 60 calls.
        write(
            json!({"maxCallsPerDay": 60, "maxInputCharsPerDay": 500000, "maxProposalsPerDay": 40}),
        );
        assert_eq!(
            store.load().unwrap().memory.learn_caps.max_calls_per_day,
            60
        );
        write(
            json!({"maxCallsPerDay": 25, "maxInputCharsPerDay": 400000, "maxProposalsPerDay": 40}),
        );
        assert_eq!(
            store.load().unwrap().memory.learn_caps.max_calls_per_day,
            25
        );
    }

    #[test]
    fn asking_moves_to_automatic_learning_once_and_a_later_choice_of_ask_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        let store = SettingsStore::in_dir(dir.path());
        // Written by a build whose default was Ask (no marker).
        std::fs::write(
            &path,
            json!({"memory": {"learnMode": "ask", "injectMemories": false}}).to_string(),
        )
        .unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.memory.learn_mode, LearnMode::Auto);
        assert!(!loaded.memory.inject_memories, "the rest is kept");
        assert!(loaded.learn_auto_migrated);
        // The user chooses Ask again (Settings saves the memory section whole).
        let mut prefs = loaded.memory.clone();
        prefs.learn_mode = LearnMode::Ask;
        store
            .update(|s| {
                s.memory = prefs;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.load().unwrap().memory.learn_mode, LearnMode::Ask);
        assert_eq!(
            SettingsStore::in_dir(dir.path())
                .load()
                .unwrap()
                .memory
                .learn_mode,
            LearnMode::Ask,
            "never moved again"
        );

        // Off is never changed, and the move is marked done.
        std::fs::write(&path, json!({"memory": {"learnMode": "off"}}).to_string()).unwrap();
        let off = store.load().unwrap();
        assert_eq!(off.memory.learn_mode, LearnMode::Off);
        assert!(off.learn_auto_migrated);

        // A new profile: automatic, and its first save records the move, so choosing Ask
        // then is kept too.
        let fresh = tempfile::tempdir().unwrap();
        let store = SettingsStore::in_dir(fresh.path());
        assert_eq!(store.load().unwrap().memory.learn_mode, LearnMode::Auto);
        store
            .update(|s| {
                s.memory.learn_mode = LearnMode::Ask;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.load().unwrap().memory.learn_mode, LearnMode::Ask);
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
        assert_eq!(
            old.background.agent_idle_minutes,
            DEFAULT_AGENT_IDLE_MINUTES
        );
    }

    #[test]
    fn agent_idle_timeout_defaults_to_five_minutes_and_zero_keeps_runtimes() {
        let mut prefs = BackgroundPrefs::default();
        assert_eq!(
            prefs.agent_idle_timeout(),
            Some(std::time::Duration::from_secs(300))
        );
        prefs.agent_idle_minutes = 0;
        assert_eq!(prefs.agent_idle_timeout(), None);
        prefs.agent_idle_minutes = u32::MAX;
        assert_eq!(
            prefs.agent_idle_timeout(),
            Some(std::time::Duration::from_secs(
                u64::from(MAX_AGENT_IDLE_MINUTES) * 60
            ))
        );
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
        assert!(!dir.path().join(SETTINGS_FILE).exists(), "nothing saved");
        assert_eq!(
            store.load().unwrap(),
            with_automatic_learning(AppSettings::default()),
            "the settings of a new profile"
        );
    }

    #[test]
    fn memory_preferences_default_to_automatic_learning_and_validate_their_bounds() {
        let old: AppSettings =
            serde_json::from_str(r#"{"memory": {"injectMemories": false}}"#).unwrap();
        assert!(!old.memory.inject_memories);
        assert_eq!(old.memory.learn_mode, LearnMode::Auto);
        assert_eq!(MemoryPrefs::default().learn_mode, LearnMode::Auto);
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

    fn notice_for(path: &Path, kind: NoticeKind) -> bool {
        let path = path.display().to_string();
        store_notices::store_notices()
            .iter()
            .any(|n| n.store == "settings" && n.path == path && n.kind == kind)
    }

    fn damaged_copies(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().contains(".damaged-"))
            })
            .collect()
    }

    #[test]
    fn a_truncated_settings_file_uses_the_backup_and_turns_local_only_on() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::in_dir(dir.path());
        let path = dir.path().join(SETTINGS_FILE);
        store
            .update(|s| {
                s.preferences.theme = Theme::Light;
                Ok(())
            })
            .unwrap();
        store
            .update(|s| {
                s.preferences.search_max_results = 12;
                Ok(())
            })
            .unwrap();
        let full = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, &full.as_bytes()[..full.len() / 2]).unwrap();

        // The backup (one change older) answers; nothing stops.
        let loaded = store.load().unwrap();
        assert_eq!(loaded.preferences.theme, Theme::Light);
        assert_eq!(
            loaded.preferences.search_max_results,
            DEFAULT_SEARCH_RESULTS
        );
        assert!(
            loaded.policy.local_only,
            "fails closed until the user checks"
        );
        assert!(notice_for(&path, NoticeKind::Restored));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            &full.as_bytes()[..full.len() / 2],
            "reading never rewrites the damaged file"
        );

        // The user turns Local-only off again: saved; the damaged file is kept.
        let (saved, ()) = store
            .update(|s| {
                s.policy.local_only = false;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.load().unwrap(), saved);
        assert!(!saved.policy.local_only);
        let damaged = damaged_copies(dir.path());
        assert_eq!(damaged.len(), 1);
        assert_eq!(
            std::fs::read(&damaged[0]).unwrap(),
            &full.as_bytes()[..full.len() / 2]
        );
    }

    #[test]
    fn a_damaged_settings_file_without_a_backup_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::in_dir(dir.path());
        let path = dir.path().join(SETTINGS_FILE);
        std::fs::write(&path, "{not json").unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.preferences, Preferences::default());
        assert!(
            loaded.policy.local_only,
            "fails closed until the user checks"
        );
        assert!(
            loaded.models.env_keys_migrated,
            "a removed key is not copied back"
        );
        assert!(notice_for(&path, NoticeKind::Reset));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");

        store
            .update(|s| {
                s.preferences.theme = Theme::Light;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.load().unwrap().preferences.theme, Theme::Light);
        let damaged = damaged_copies(dir.path());
        assert_eq!(
            damaged.len(),
            1,
            "the damaged file is kept, not overwritten"
        );
        assert_eq!(std::fs::read_to_string(&damaged[0]).unwrap(), "{not json");
    }

    #[test]
    fn notification_preferences_default_round_trip_in_camel_case_and_are_user_only() {
        let defaults = AppSettings::default().notifications;
        assert!(defaults.reminders && defaults.needs_you && defaults.work_finished);
        assert!(defaults.problems);
        assert!(
            !defaults.memory_suggestions,
            "memory suggestions are off by default"
        );
        // Settings written before notifications existed get the defaults.
        let old: AppSettings = serde_json::from_str(r#"{"seeded": true}"#).unwrap();
        assert_eq!(old.notifications, NotificationPrefs::default());
        let partial: AppSettings =
            serde_json::from_str(r#"{"notifications": {"workFinished": false}}"#).unwrap();
        assert!(!partial.notifications.work_finished);
        assert!(partial.notifications.reminders);

        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::in_dir(dir.path());
        let chosen = NotificationPrefs {
            reminders: false,
            needs_you: true,
            memory_suggestions: true,
            work_finished: false,
            problems: false,
        };
        store
            .update(|s| {
                s.notifications = chosen;
                Ok(())
            })
            .unwrap();
        let reloaded = SettingsStore::in_dir(dir.path()).load().unwrap();
        assert_eq!(reloaded.notifications, chosen);
        let json = serde_json::to_value(reloaded.notifications).unwrap();
        assert_eq!(
            json,
            json!({
                "reminders": false,
                "needsYou": true,
                "memorySuggestions": true,
                "workFinished": false,
                "problems": false,
            })
        );
        assert!(denied_reason("notifications").is_some());
        assert!(denied_reason("notifications.reminders").is_some());
        assert!(SettingKey::parse("notifications").is_none());
    }
}
