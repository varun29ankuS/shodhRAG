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
//! would get back from the tool (the result text after its cut), so a 1 MB
//! log cut either way saves nothing.
//!
//! The compression switch: each conversation may set its own; otherwise the
//! default for Code conversations applies (on unless the user turned it off
//! in Settings → Code), so it can be chosen before a conversation exists. A
//! session reads it at every command, so a change applies to the next one.
//!
//! A file that cannot be parsed is moved aside (to `code_savings.json.bad`,
//! for the user to look at) and counting starts afresh, with a warning in
//! the log, so a broken file never locks the switch.

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
    /// The user's choice for this conversation; `None`: the default.
    #[serde(default)]
    pub compression: Option<bool>,
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
    /// Conversations ever counted, deleted ones included.
    #[serde(default)]
    counted_conversations: u64,
    /// Since when the total counts (RFC 3339).
    #[serde(default)]
    since: Option<String>,
    /// The switch for conversations that did not set their own; `None`: on.
    #[serde(default)]
    default_on: Option<bool>,
}

impl SavingsFile {
    fn default_on(&self) -> bool {
        self.default_on.unwrap_or(true)
    }
}

/// The running total.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TotalSavings {
    pub tally: Tally,
    /// Conversations ever counted (deleted ones included, as in `tally`).
    pub conversations: u64,
    pub since: Option<String>,
    /// Compression in conversations that did not choose for themselves.
    pub default_on: bool,
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
    /// Where a file that could not be parsed is moved.
    pub const SET_ASIDE: &'static str = "code_savings.json.bad";

    pub fn in_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(Self::FILE),
        }
    }

    /// The file; one that cannot be parsed is moved aside and counting
    /// starts afresh. Fails only when the file cannot be read at all.
    fn read(&self) -> Result<SavingsFile, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SavingsFile::default()),
            Err(e) => return Err(format!("{} is not readable: {e}", self.path.display())),
        };
        match serde_json::from_str(&text) {
            Ok(file) => Ok(file),
            Err(e) => {
                let aside = self.path.with_file_name(Self::SET_ASIDE);
                let moved = std::fs::rename(&self.path, &aside);
                tracing::warn!(target: "shodh::harness", error = %e, moved = moved.is_ok(), aside = %aside.display(), "the tokens-saved file could not be parsed; it was set aside and counting starts afresh");
                moved
                    .map_err(|e| format!("{} could not be set aside: {e}", self.path.display()))?;
                Ok(SavingsFile::default())
            }
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

    fn update<T>(&self, edit: impl FnOnce(&mut SavingsFile) -> T) -> Result<T, String> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = self.read()?;
        let out = edit(&mut file);
        self.write(&file)?;
        Ok(out)
    }

    fn view<T>(&self, look: impl FnOnce(SavingsFile) -> T) -> Result<T, String> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Ok(look(self.read()?))
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
            if record.tally.commands == 0 {
                file.counted_conversations = file.counted_conversations.saturating_add(1);
            }
            record.tally.add(original, sent);
            record.updated_at = Some(at);
            record.clone()
        })
    }

    /// The record of `conversation_id` (empty when nothing was counted) and
    /// whether compression is on there.
    pub fn get(&self, conversation_id: &str) -> Result<(ConversationSavings, bool), String> {
        self.view(|mut file| {
            let default_on = file.default_on();
            let record = file
                .conversations
                .remove(conversation_id)
                .unwrap_or_default();
            let on = record.compression.unwrap_or(default_on);
            (record, on)
        })
    }

    /// Whether compression is on for `conversation_id`. On when the file
    /// cannot be read at all: compression never drops failures.
    pub fn compression_on(&self, conversation_id: &str) -> bool {
        match self.get(conversation_id) {
            Ok((_, on)) => on,
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
            record.compression = Some(on);
            record.clone()
        })
    }

    /// Set the switch for conversations that did not choose for themselves.
    pub fn set_default(&self, on: bool) -> Result<(), String> {
        self.update(|file| file.default_on = Some(on))
    }

    /// The running total.
    pub fn total(&self) -> Result<TotalSavings, String> {
        self.view(|file| {
            let default_on = file.default_on();
            // A file from before the counter: the conversations it still has.
            let listed = file
                .conversations
                .values()
                .filter(|c| c.tally.commands > 0)
                .count();
            TotalSavings {
                tally: file.total,
                conversations: file
                    .counted_conversations
                    .max(u64::try_from(listed).unwrap_or(u64::MAX)),
                since: file.since,
                default_on,
            }
        })
    }

    /// Forget a deleted conversation's record (the total keeps its savings).
    /// Returns whether it had one.
    pub fn forget(&self, conversation_id: &str) -> Result<bool, String> {
        self.update(|file| file.conversations.remove(conversation_id).is_some())
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
        assert_eq!(store.get("c1").unwrap().0, ConversationSavings::default());
        store.record("c1", 1_000, 400).unwrap();
        store.record("c1", 500, 500).unwrap();
        let c1 = store.record("c1", 300, 100).unwrap();
        assert_eq!(c1.tally.original_tokens, 1_800);
        assert_eq!(c1.tally.sent_tokens, 1_000);
        assert_eq!(c1.tally.saved_tokens(), 800);
        assert_eq!(c1.tally.commands, 3);
        assert_eq!(c1.tally.shortened, 2);
        store.record("c2", 200, 50).unwrap();
        assert_eq!(store.get("c2").unwrap().0.tally.saved_tokens(), 150);
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
        assert_eq!(store.get("c1").unwrap().0.tally.saved_tokens(), 600);
        assert!(store.forget("c1").unwrap());
        assert!(!store.forget("c1").unwrap());
        assert_eq!(store.get("c1").unwrap().0.tally.commands, 0);
        let total = store.total().unwrap();
        assert_eq!(total.tally.saved_tokens(), 600, "the total keeps them");
        // Its commands still count, and so does the conversation they ran in.
        assert_eq!(total.tally.commands, 1);
        assert_eq!(total.conversations, 1);
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
    fn the_switch_is_per_conversation_over_a_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = SavingsStore::in_dir(dir.path());
        assert!(store.compression_on("c1"));
        assert!(store.total().unwrap().default_on);
        store.set_compression("c1", false).unwrap();
        assert!(!store.compression_on("c1"));
        assert!(store.compression_on("c2"));
        // Off by default: new conversations start off, a conversation's own choice wins.
        store.set_compression("c3", true).unwrap();
        store.set_default(false).unwrap();
        assert!(!store.compression_on("c2"));
        assert!(!store.compression_on("new"));
        assert!(store.compression_on("c3"));
        assert!(!store.total().unwrap().default_on);
    }

    #[test]
    fn an_unparsable_file_is_set_aside_and_never_locks_the_switch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SavingsStore::FILE);
        std::fs::write(&path, "{ not json").unwrap();
        let store = SavingsStore::in_dir(dir.path());
        store.set_compression("c1", false).unwrap();
        assert!(!store.compression_on("c1"), "the switch works again");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(SavingsStore::SET_ASIDE)).unwrap(),
            "{ not json",
            "the broken file is kept for the user"
        );
        assert_eq!(store.record("c1", 10, 5).unwrap().tally.commands, 1);
    }
}
