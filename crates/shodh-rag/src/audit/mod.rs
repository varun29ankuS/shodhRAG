//! Tamper-evident audit log (spec §8 "Audit").
//!
//! Every event is one row of `audit_events` in `<app_data_dir>/shodh.db`.
//! Rows form a hash chain: `hash = SHA-256(prev_hash || canonical(row))`,
//! where `canonical(row)` is the canonical JSON of every column except
//! `prev_hash` and `hash`. Editing, deleting or reordering a row breaks the
//! chain and [`AuditLog::verify`] reports the first row that no longer links.
//! Retention trims a prefix of the chain and appends a `retention_checkpoint`
//! event carrying the hash of the last deleted row, so the remaining chain
//! still verifies.
//!
//! All writes go through one writer thread; callers on async or UI paths use
//! [`AuditLog::submit`], which never blocks on disk.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
// CI runs clippy on this module with every warning (rustc + default clippy set) denied.
// Gated on `clippy` so a new compiler lint never breaks a normal build.
#![cfg_attr(clippy, deny(warnings))]

mod canonical;
pub mod payload;
mod store;
pub mod tap;

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use canonical::canonical_json;
pub use store::{
    open_shared_connection, AuditLog, DEFAULT_RETENTION_DAYS, MAX_RETENTION_DAYS,
    MIN_RETENTION_DAYS,
};
pub use tap::RunAuditTap;

/// The principal of every event recorded by the desktop app.
pub const LOCAL_OWNER: &str = "local-owner";

/// Principal of events the app records on its own (timeouts, retention).
pub const SYSTEM_PRINCIPAL: &str = "system";

/// `prev_hash` of the first event ever written.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Longest document snippet stored in an event. Only text the user was
/// already shown is stored, and never more than this.
pub const MAX_SNIPPET_CHARS: usize = 300;

/// Errors of the audit store.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("Audit database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Audit file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Audit data could not be encoded: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Audit CSV export failed: {0}")]
    Csv(#[from] csv::Error),
    #[error(
        "The audit database could not be opened with the stored key. Restore the \
         'audit-db-key' entry in the OS credential store, or move shodh.db aside to start a new log."
    )]
    Undecryptable,
    #[error(
        "An encryption key was supplied but this build has no SQLCipher support; \
         rebuild with the `sqlcipher` feature or open the log without a key."
    )]
    EncryptionUnavailable,
    #[error(
        "shodh.db is an unencrypted database but an encryption key was supplied. Export the log, \
         move shodh.db aside and restart to begin an encrypted log."
    )]
    PlaintextDatabase,
    #[error("Invalid audit key: {0}")]
    InvalidKey(String),
    #[error("Retention must be between {min} and {max} days, got {got}")]
    InvalidRetention { min: u32, max: u32, got: u32 },
    #[error("Unknown audit event type: {0}")]
    UnknownEventType(String),
    #[error("Audit database schema version {found} is newer than this app supports ({supported})")]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("The audit writer has stopped; restart the app to resume auditing")]
    WriterStopped,
}

pub type AuditResult<T> = Result<T, AuditError>;

/// Kind of audited event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    Question,
    ToolCall,
    Approval,
    Retrieval,
    Answer,
    SourceChange,
    SettingsChange,
    RuntimeInstall,
    RetentionCheckpoint,
    /// A memory statement was stored (added, updated or superseded).
    MemoryWrite,
    /// A memory was forgotten (soft-deleted).
    MemoryForget,
    /// Memories were recalled and used in an answer.
    MemoryUse,
    /// The agent's model changed (user choice, fallback for one answer, environment).
    ModelChange,
}

impl AuditEventType {
    pub const ALL: [AuditEventType; 13] = [
        AuditEventType::Question,
        AuditEventType::ToolCall,
        AuditEventType::Approval,
        AuditEventType::Retrieval,
        AuditEventType::Answer,
        AuditEventType::SourceChange,
        AuditEventType::SettingsChange,
        AuditEventType::RuntimeInstall,
        AuditEventType::RetentionCheckpoint,
        AuditEventType::MemoryWrite,
        AuditEventType::MemoryForget,
        AuditEventType::MemoryUse,
        AuditEventType::ModelChange,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AuditEventType::Question => "question",
            AuditEventType::ToolCall => "tool_call",
            AuditEventType::Approval => "approval",
            AuditEventType::Retrieval => "retrieval",
            AuditEventType::Answer => "answer",
            AuditEventType::SourceChange => "source_change",
            AuditEventType::SettingsChange => "settings_change",
            AuditEventType::RuntimeInstall => "runtime_install",
            AuditEventType::RetentionCheckpoint => "retention_checkpoint",
            AuditEventType::MemoryWrite => "memory_write",
            AuditEventType::MemoryForget => "memory_forget",
            AuditEventType::MemoryUse => "memory_use",
            AuditEventType::ModelChange => "model_change",
        }
    }
}

impl fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AuditEventType {
    type Err = AuditError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        AuditEventType::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| AuditError::UnknownEventType(s.to_string()))
    }
}

/// An event to append. The store assigns `id`, `ts` and the hashes.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditRecord {
    pub principal: String,
    pub conversation_id: Option<String>,
    pub profile_id: Option<String>,
    pub run_id: Option<String>,
    pub event_type: AuditEventType,
    pub payload: Value,
}

impl AuditRecord {
    /// An event by the local owner.
    pub fn new(event_type: AuditEventType, payload: Value) -> Self {
        Self {
            principal: LOCAL_OWNER.to_string(),
            conversation_id: None,
            profile_id: None,
            run_id: None,
            event_type,
            payload,
        }
    }

    pub fn principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = principal.into();
        self
    }

    pub fn conversation(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    pub fn profile(mut self, profile_id: impl Into<String>) -> Self {
        self.profile_id = Some(profile_id.into());
        self
    }

    pub fn run(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }
}

/// Who an agent run belongs to; attached to every event the harness records
/// for that conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditScope {
    pub principal: String,
    pub conversation_id: String,
    pub profile_id: String,
}

impl AuditScope {
    /// Start a record for `run_id` in this scope.
    pub fn record(&self, run_id: &str, event_type: AuditEventType, payload: Value) -> AuditRecord {
        AuditRecord::new(event_type, payload)
            .principal(self.principal.clone())
            .conversation(self.conversation_id.clone())
            .profile(self.profile_id.clone())
            .run(run_id.to_string())
    }
}

/// One stored event.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRow {
    pub id: i64,
    pub ts: String,
    pub principal: String,
    pub conversation_id: Option<String>,
    pub profile_id: Option<String>,
    pub run_id: Option<String>,
    pub event_type: String,
    pub payload: Value,
    pub prev_hash: String,
    pub hash: String,
}

/// Filters for [`AuditLog::query`]. Every field is optional; results are
/// newest first.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuditQuery {
    /// Event types to include; empty means all.
    pub types: Vec<AuditEventType>,
    /// Inclusive lower bound on `ts`.
    pub from: Option<DateTime<Utc>>,
    /// Inclusive upper bound on `ts`.
    pub to: Option<DateTime<Utc>>,
    pub conversation_id: Option<String>,
    /// Events whose payload names this tool (`tool_call`, `approval`,
    /// `retrieval`).
    pub tool: Option<String>,
    /// Case-insensitive substring of the payload, type or principal.
    pub text: Option<String>,
    pub limit: Option<u32>,
    pub offset: u32,
}

/// Result of [`AuditLog::verify`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub ok: bool,
    /// Rows examined (all rows when `ok`).
    pub checked: u64,
    /// First row whose contents or link to its predecessor do not verify.
    pub first_bad_id: Option<i64>,
    /// Why that row failed.
    pub reason: Option<String>,
}

/// Usage this calendar month (bounds supplied by the caller, local time).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthStats {
    pub questions: u64,
    pub tool_calls: u64,
    pub approvals: u64,
    pub cloud_input_tokens: u64,
    pub cloud_output_tokens: u64,
    pub cloud_cost_usd: f64,
}

/// Summary for the Usage & Audit page.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditStats {
    pub total: u64,
    pub by_type: BTreeMap<String, u64>,
    pub oldest: Option<String>,
    pub newest: Option<String>,
    pub month: MonthStats,
    pub retention_days: u32,
    /// The database is encrypted (SQLCipher confirmed at open).
    pub encrypted: bool,
}

/// A 256-bit database key. Never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct AuditKey([u8; 32]);

impl AuditKey {
    /// A fresh random key from the operating system's CSPRNG.
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn from_hex(text: &str) -> AuditResult<Self> {
        let bytes = hex::decode(text.trim())
            .map_err(|e| AuditError::InvalidKey(format!("not hexadecimal: {e}")))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| AuditError::InvalidKey("expected 32 bytes".to_string()))?;
        Ok(Self(bytes))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Debug for AuditKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuditKey([REDACTED])")
    }
}

/// Whether this build links SQLCipher. Decides whether a key is worth
/// creating; whether a database *is* encrypted is confirmed at open.
pub const fn encryption_compiled() -> bool {
    cfg!(feature = "sqlcipher")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types_round_trip() {
        for t in AuditEventType::ALL {
            assert_eq!(t.as_str().parse::<AuditEventType>().unwrap(), t);
            assert_eq!(
                serde_json::to_value(t).unwrap(),
                Value::String(t.as_str().to_string())
            );
        }
        assert!("profile".parse::<AuditEventType>().is_err());
    }

    #[test]
    fn keys_round_trip_and_never_print() {
        let key = AuditKey::generate();
        assert_eq!(AuditKey::from_hex(&key.to_hex()).unwrap(), key);
        assert_eq!(format!("{key:?}"), "AuditKey([REDACTED])");
        assert!(AuditKey::from_hex("abcd").is_err());
        assert!(AuditKey::from_hex("zz").is_err());
        assert_ne!(AuditKey::generate(), key);
    }

    #[test]
    fn queries_deserialise_from_the_ui_shape() {
        let q: AuditQuery = serde_json::from_value(serde_json::json!({
            "types": ["question", "tool_call"],
            "from": "2026-10-01T00:00:00Z",
            "conversationId": "c1",
            "text": "contract",
            "limit": 50
        }))
        .unwrap();
        assert_eq!(
            q.types,
            vec![AuditEventType::Question, AuditEventType::ToolCall]
        );
        assert_eq!(q.conversation_id.as_deref(), Some("c1"));
        assert_eq!(q.offset, 0);
        assert!(q.to.is_none());
    }
}
