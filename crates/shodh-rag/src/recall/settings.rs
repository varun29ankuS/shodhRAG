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
    /// The user's own sensitive patterns (regular expressions), redacted like keys.
    pub patterns: Vec<String>,
    pub idle_minutes: u32,
}

impl Default for RecallSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            tools: BTreeSet::new(),
            projects: BTreeMap::new(),
            patterns: Vec::new(),
            idle_minutes: DEFAULT_IDLE_MINUTES,
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
    /// Whether the sessions of `tool` in the project `project_key` are read.
    pub fn wants(&self, tool: Tool, project_key: &str) -> bool {
        self.enabled && self.tools.contains(&tool) && self.projects.get(project_key) == Some(&true)
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
