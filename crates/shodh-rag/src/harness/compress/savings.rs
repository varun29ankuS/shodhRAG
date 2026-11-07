//! Tokens saved by output compression, per Code conversation and in total.
//!
//! Counted per conversation (never per answer or per command in the UI) in
//! one JSON file of Shodh's data folder, [`SavingsStore::FILE`]. The running
//! total is kept on its own, so it survives deleting a conversation.
//!
//! **Tokens are estimated**, not counted with the model's tokenizer: Code mode
//! runs on many providers' models, each with its own tokenizer, and none is
//! available offline for all of them. The estimate is [`estimate_tokens`],
//! about four characters a token (the same estimate Shodh uses to check that
//! a prompt fits a model, `model_fallback::resolve::estimate_tokens`), and the
//! UI writes every number with "≈". Both sides of a saving are what the model
//! would get back from the tool (the result text after its head-and-tail
//! cut), so a 1 MB log cut to 18,000 characters either way saves nothing.
//!
//! Each conversation also keeps its compression switch (on unless the user
//! turned it off for that conversation); a session reads it at every command,
//! so a change applies to the next command.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// About four characters a token (see the module docs).
pub fn estimate_tokens(text: &str) -> u64 {
    u64::try_from(text.chars().count().div_ceil(4)).unwrap_or(u64::MAX)
}

/// How the estimate is made, in the user's words.
pub const ESTIMATE_METHOD: &str = "Estimated at about four characters a token; each model's own tokenizer counts a little differently.";

/// Token counts of commands whose output went to the model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tally {
    /// What the model would have got without compression.
    #[serde(default)]
    pub original_tokens: u64,
    /// What it got.
    #[serde(default)]
    pub sent_tokens: u64,
    /// Commands counted.
    #[serde(default)]
    pub commands: u64,
    /// Of those, the ones compression shortened.
    #[serde(default)]
    pub shortened: u64,
}

impl Tally {
    /// Tokens saved (never negative: compression never lengthens).
    pub fn saved_tokens(&self) -> u64 {
        self.original_tokens.saturating_sub(self.sent_tokens)
    }

    fn add(&mut self, original: u64, sent: u64) {
        let sent = sent.min(original);
        self.original_tokens = self.original_tokens.saturating_add(original);
        self.sent_tokens = self.sent_tokens.saturating_add(sent);
        self.commands = self.commands.saturating_add(1);
        if sent < original {
            self.shortened = self.shortened.saturating_add(1);
        }
    }
}

/// One conversation's record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSavings {
    #[serde(default)]
    pub tally: Tally,
    /// The user turned compression off for this conversation.
    #[serde(default)]
    pub compression_off: bool,
    /// When a command was last counted (RFC 3339).
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// The whole file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavingsFile {
    #[serde(default)]
    conversations: BTreeMap<String, ConversationSavings>,
    /// Every conversation's, deleted ones included.
    #[serde(default)]
    total: Tally,
    /// Since when the total counts (RFC 3339).
    #[serde(default)]
    since: Option<String>,
}

/// The running total.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TotalSavings {
    pub tally: Tally,
    /// Conversations counted that still exist.
    pub conversations: usize,
    pub since: Option<String>,
}

/// The savings file of a data folder.
#[derive(Debug, Clone)]
pub struct SavingsStore {
    path: PathBuf,
}

/// One read-modify-write of the file at a time in this process.
static LOCK: Mutex<()> = Mutex::new(());

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

impl SavingsStore {
    pub const FILE: &'static str = "code_savings.json";

    pub fn in_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(Self::FILE),
        }
    }

    fn read(&self) -> Result<SavingsFile, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("{} is not readable: {e}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SavingsFile::default()),
            Err(e) => Err(format!("{} is not readable: {e}", self.path.display())),
        }
    }

    fn write(&self, file: &SavingsFile) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    /// Change the file with `edit`. An unreadable file is left as it is (its
    /// total would be lost), and the change fails.
    fn update<T>(&self, edit: impl FnOnce(&mut SavingsFile) -> T) -> Result<T, String> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = self.read()?;
        let out = edit(&mut file);
        self.write(&file)?;
        Ok(out)
    }

    /// Count one command of `conversation_id`: `original` tokens without
    /// compression, `sent` with it. Returns the conversation's record.
    pub fn record(
        &self,
        conversation_id: &str,
        original: u64,
        sent: u64,
    ) -> Result<ConversationSavings, String> {
        self.update(|file| {
            let at = now();
            file.since.get_or_insert_with(|| at.clone());
            file.total.add(original, sent);
            let record = file
                .conversations
                .entry(conversation_id.to_string())
                .or_default();
            record.tally.add(original, sent);
            record.updated_at = Some(at);
            record.clone()
        })
    }

    /// The record of `conversation_id` (empty when nothing was counted).
    pub fn get(&self, conversation_id: &str) -> Result<ConversationSavings, String> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self
            .read()?
            .conversations
            .remove(conversation_id)
            .unwrap_or_default())
    }

    /// Whether compression is on for `conversation_id`. On when the file is
    /// unreadable too: compression never drops failures, so on is safe.
    pub fn compression_on(&self, conversation_id: &str) -> bool {
        match self.get(conversation_id) {
            Ok(record) => !record.compression_off,
            Err(e) => {
                tracing::warn!(target: "shodh::harness", error = %e, "savings file unreadable; output compression stays on");
                true
            }
        }
    }

    /// Turn compression on or off for `conversation_id`.
    pub fn set_compression(
        &self,
        conversation_id: &str,
        on: bool,
    ) -> Result<ConversationSavings, String> {
        self.update(|file| {
            let record = file
                .conversations
                .entry(conversation_id.to_string())
                .or_default();
            record.compression_off = !on;
            record.clone()
        })
    }

    /// The running total.
    pub fn total(&self) -> Result<TotalSavings, String> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let file = self.read()?;
        Ok(TotalSavings {
            tally: file.total,
            conversations: file
                .conversations
                .values()
                .filter(|c| c.tally.commands > 0)
                .count(),
            since: file.since,
        })
    }

    /// Forget a deleted conversation's record (the total keeps its savings).
    /// Returns whether it had one.
    pub fn forget(&self, conversation_id: &str) -> Result<bool, String> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = self.read()?;
        if file.conversations.remove(conversation_id).is_none() {
            return Ok(false);
        }
        self.write(&file)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_estimate_is_four_characters_a_token_rounded_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        // Characters, not bytes.
        assert_eq!(estimate_tokens("éééé"), 1);
    }

    #[test]
    fn savings_add_up_per_conversation_and_in_total() {
        let dir = tempfile::tempdir().unwrap();
        let store = SavingsStore::in_dir(dir.path());
        assert_eq!(store.get("c1").unwrap(), ConversationSavings::default());
        store.record("c1", 1_000, 400).unwrap();
        store.record("c1", 500, 500).unwrap();
        let c1 = store.record("c1", 300, 100).unwrap();
        assert_eq!(c1.tally.original_tokens, 1_800);
        assert_eq!(c1.tally.sent_tokens, 1_000);
        assert_eq!(c1.tally.saved_tokens(), 800);
        assert_eq!(c1.tally.commands, 3);
        assert_eq!(c1.tally.shortened, 2);
        store.record("c2", 200, 50).unwrap();
        assert_eq!(store.get("c2").unwrap().tally.saved_tokens(), 150);
        let total = store.total().unwrap();
        assert_eq!(total.tally.saved_tokens(), 950);
        assert_eq!(total.tally.commands, 4);
        assert_eq!(total.conversations, 2);
        assert!(total.since.is_some());
    }

    #[test]
    fn savings_persist_and_survive_a_deleted_conversation() {
        let dir = tempfile::tempdir().unwrap();
        SavingsStore::in_dir(dir.path())
            .record("c1", 900, 300)
            .unwrap();
        // A new store over the same folder (the app restarted) reads them back.
        let store = SavingsStore::in_dir(dir.path());
        assert_eq!(store.get("c1").unwrap().tally.saved_tokens(), 600);
        assert!(store.forget("c1").unwrap());
        assert!(!store.forget("c1").unwrap());
        assert_eq!(store.get("c1").unwrap().tally.commands, 0);
        let total = store.total().unwrap();
        assert_eq!(total.tally.saved_tokens(), 600, "the total keeps them");
        assert_eq!(total.conversations, 0);
    }

    #[test]
    fn a_sent_copy_never_counts_as_more_than_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let store = SavingsStore::in_dir(dir.path());
        let record = store.record("c1", 100, 140).unwrap();
        assert_eq!(record.tally.sent_tokens, 100);
        assert_eq!(record.tally.saved_tokens(), 0);
        assert_eq!(record.tally.shortened, 0);
    }

    #[test]
    fn the_switch_is_per_conversation_and_on_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = SavingsStore::in_dir(dir.path());
        assert!(store.compression_on("c1"));
        store.set_compression("c1", false).unwrap();
        assert!(!store.compression_on("c1"));
        assert!(store.compression_on("c2"));
        store.set_compression("c1", true).unwrap();
        assert!(store.compression_on("c1"));
    }

    #[test]
    fn an_unreadable_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SavingsStore::FILE);
        std::fs::write(&path, "{ not json").unwrap();
        let store = SavingsStore::in_dir(dir.path());
        assert!(store.record("c1", 10, 5).is_err());
        assert!(store.total().is_err());
        assert!(store.compression_on("c1"), "compression stays on");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }
}
