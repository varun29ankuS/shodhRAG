//! What the user chose for Recall: off until they turn it on, then only the tools and the
//! projects they picked. Kept in `recall-settings.json` beside Recall's index.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::redact::{check_patterns, PatternError};
use super::Tool;

/// The settings file's name, in Recall's folder.
pub const SETTINGS_FILE: &str = "recall-settings.json";

/// Minutes a session must be idle before it is indexed, by default: a live session is
/// indexed once its tool stopped writing to it for that long.
pub const DEFAULT_IDLE_MINUTES: u32 = 10;
pub const MIN_IDLE_MINUTES: u32 = 1;
pub const MAX_IDLE_MINUTES: u32 = 24 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecallSettings {
    /// Recall is on. Nothing is read while it is off.
    pub enabled: bool,
    /// The tools whose sessions are read.
    pub tools: BTreeSet<Tool>,
    /// Each project the user decided on, by its key ([`super::discover::project_key`]):
    /// `true` included, `false` excluded. A project not listed is not read until the
    /// user includes it.
    pub projects: BTreeMap<String, bool>,
    /// The user's own hidden words, one a line: a word or phrase, or a pattern between
    /// slashes ([`super::redact::check_patterns`]), redacted like keys.
    pub patterns: Vec<String>,
    pub idle_minutes: u32,
    /// Email addresses are kept in what is indexed (hidden by default).
    pub keep_emails: bool,
    /// Whether the assistant may read the coding sessions, and with which models: what it
    /// reads goes to the model the user chose.
    pub assistant: AssistantAccess,
}

/// Whether the assistant may read the coding sessions ([`RecallSettings::assistant`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantAccess {
    /// Never: the sessions are searched in the Library only.
    #[default]
    Off,
    /// Only while the assistant answers with a model on this computer.
    LocalModels,
    /// With any model the user chose, also one off this computer.
    AnyModel,
}

impl Default for RecallSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            tools: BTreeSet::new(),
            projects: BTreeMap::new(),
            patterns: Vec::new(),
            idle_minutes: DEFAULT_IDLE_MINUTES,
            keep_emails: false,
            assistant: AssistantAccess::Off,
        }
    }
}

/// Settings that cannot be kept.
#[derive(Debug)]
pub enum SettingsError {
    Pattern(PatternError),
    Io(std::io::Error),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::Pattern(e) => write!(f, "{e}"),
            SettingsError::Io(e) => write!(f, "Recall's settings could not be saved ({e})"),
        }
    }
}

impl std::error::Error for SettingsError {}

impl RecallSettings {
    /// The user's decision for a folder: its own, else its nearest parent's. `chain` is
    /// the folder's key and its parents' ([`super::discover::project_chain`]).
    pub fn decide(&self, chain: &[String]) -> Option<bool> {
        chain.iter().find_map(|k| self.projects.get(k).copied())
    }

    /// Whether a session of `tool` is read: its tool is chosen, the folder it started in
    /// (`chains[0]`) is included, and no folder it worked in (any of `chains`) is
    /// excluded. A project excluded excludes its subfolders and worktrees inside it.
    pub fn wants(&self, tool: Tool, chains: &[Vec<String>]) -> bool {
        self.enabled
            && self.tools.contains(&tool)
            && chains.first().is_some_and(|c| self.decide(c) == Some(true))
            && chains.iter().all(|c| self.decide(c) != Some(false))
    }

    /// The redactor these settings ask for: the built-in rules, the user's hidden words
    /// and whether emails are kept.
    pub fn redactor(&self) -> Result<super::redact::Redactor, PatternError> {
        super::redact::Redactor::with_options(
            &self.patterns,
            super::redact::RedactOptions {
                keep_emails: self.keep_emails,
            },
        )
    }

    /// Checks the settings before they are kept: the patterns compile, the idle time is
    /// in range.
    pub fn check(&mut self) -> Result<(), SettingsError> {
        self.patterns.retain(|p| !p.trim().is_empty());
        check_patterns(&self.patterns).map_err(SettingsError::Pattern)?;
        self.idle_minutes = self.idle_minutes.clamp(MIN_IDLE_MINUTES, MAX_IDLE_MINUTES);
        Ok(())
    }

    pub fn path(dir: &Path) -> PathBuf {
        dir.join(SETTINGS_FILE)
    }

    /// The saved settings; the defaults (off) when there are none. Unreadable settings
    /// are reported, never taken for "on".
    pub fn load(dir: &Path) -> std::io::Result<Self> {
        match std::fs::read(Self::path(dir)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Checks and saves the settings (written beside, then moved into place).
    pub fn save(&mut self, dir: &Path) -> Result<(), SettingsError> {
        self.check()?;
        std::fs::create_dir_all(dir).map_err(SettingsError::Io)?;
        let path = Self::path(dir);
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| SettingsError::Io(std::io::Error::other(e)))?;
        std::fs::write(&tmp, json).map_err(SettingsError::Io)?;
        std::fs::rename(&tmp, &path).map_err(SettingsError::Io)
    }
}
