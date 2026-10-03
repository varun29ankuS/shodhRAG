//! Typed statement store: validated, versioned facts with history.
//!
//! A statement is an ontology-typed, n-ary fact with provenance
//! ([`shodh_ontology::Statement`]). This store is generic: memory is its first consumer,
//! records and the knowledge graph use the same store later.
//!
//! Storage uses the stores the app already has, nothing new:
//! - **LanceDB** table `statements` (next to the document index): content, a text rendering
//!   with its embedding, the values as plain words with a full-text index, provenance and
//!   the validity interval
//!   (`valid_from`, `valid_to`, `expires_at`, `forgotten_at`). Rows are appended; only the
//!   validity columns are ever updated (supersede and forget), never the content.
//! - **SQLite** `shodh.db` (schema version 2, next to the audit log): `statement_dynamics`
//!   (strength at an anchor time, importance, use count, last use, pin) and
//!   `statement_links` (Hebbian co-activation weights). Decay is computed at read time from
//!   the class [`shodh_ontology::Dynamics`], so reads never rewrite rows.
//!
//! Writes go through [`StatementStore::put`], which validates against the ontology and
//! applies [`shodh_ontology::Ontology::supersedes`]: temporal values supersede with history
//! (the old row gets `valid_to`), equal facts reinforce instead of duplicating, and
//! conflicting functional values are reported, never overwritten silently.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod dynamics;
mod identity;
mod lance;
mod render;
mod sqlite;
mod store;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
pub(crate) mod testing;

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shodh_ontology::{PropertyChange, Statement, Violation};

pub use dynamics::{DynamicsState, LinkState};
pub use identity::identity_tokens;
pub use render::{render_terms, render_text, SELF_ENTITY_ID};
pub use sqlite::DynamicsStore;
pub use store::{EmbedderSource, StatementStore, STATEMENTS_TABLE};

/// Errors of the statement store.
#[derive(Debug, thiserror::Error)]
pub enum StatementError {
    /// The statement does not conform to the ontology. Nothing was written.
    #[error("statement is not valid: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<Violation>),
    /// No statement with this id exists (or it was forgotten).
    #[error("no statement with id `{0}`")]
    NotFound(String),
    /// A statement with this id already exists. Ids are never reused.
    #[error("a statement with id `{0}` already exists")]
    DuplicateId(String),
    /// The requested supersede target cannot be superseded by this statement.
    #[error("cannot supersede `{target}`: {reason}")]
    InvalidSupersede {
        /// The target statement id.
        target: String,
        /// Why.
        reason: String,
    },
    /// The search models (embedder) are not installed or not loaded yet.
    #[error("{0}")]
    EmbeddingUnavailable(String),
    /// The embedder produced vectors of a different size than the table stores.
    #[error(
        "the embedding model produces {found}-dimensional vectors but statements use {expected}"
    )]
    DimensionMismatch {
        /// Table dimension.
        expected: usize,
        /// Embedder dimension.
        found: usize,
    },
    /// Embedding the statement text failed.
    #[error("embedding failed: {0}")]
    Embedding(String),
    /// LanceDB failed.
    #[error("statement table error: {0}")]
    Lance(String),
    /// SQLite (`shodh.db`) failed.
    #[error("statement dynamics database error: {0}")]
    Sqlite(String),
    /// A stored row could not be decoded.
    #[error("stored statement `{id}` is unreadable: {reason}")]
    Corrupt {
        /// Row id.
        id: String,
        /// What is wrong.
        reason: String,
    },
    /// A background task failed to complete.
    #[error("statement store task failed: {0}")]
    Task(String),
}

impl From<lancedb::Error> for StatementError {
    fn from(e: lancedb::Error) -> Self {
        StatementError::Lance(e.to_string())
    }
}

impl From<rusqlite::Error> for StatementError {
    fn from(e: rusqlite::Error) -> Self {
        StatementError::Sqlite(e.to_string())
    }
}

impl From<crate::audit::AuditError> for StatementError {
    fn from(e: crate::audit::AuditError) -> Self {
        StatementError::Sqlite(e.to_string())
    }
}

impl From<arrow_schema::ArrowError> for StatementError {
    fn from(e: arrow_schema::ArrowError) -> Self {
        StatementError::Lance(e.to_string())
    }
}

/// Result alias of the statement store.
pub type StatementResult<T> = Result<T, StatementError>;

/// Where a statement is visible. A workspace sees its own statements and global ones.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// Visible everywhere.
    Global,
    /// Visible only in one workspace (a source / space id).
    Workspace(String),
}

impl Scope {
    /// Scope for an optional workspace id: `None` or blank means global.
    pub fn for_workspace(workspace: Option<&str>) -> Self {
        match workspace.map(str::trim).filter(|w| !w.is_empty()) {
            Some(id) => Scope::Workspace(id.to_string()),
            None => Scope::Global,
        }
    }

    /// Stored form: `global` or `workspace:<id>`.
    pub fn as_key(&self) -> String {
        match self {
            Scope::Global => "global".to_string(),
            Scope::Workspace(id) => format!("workspace:{id}"),
        }
    }

    /// The scopes visible from this one (itself and, for a workspace, global).
    pub fn visible(&self) -> Vec<Scope> {
        match self {
            Scope::Global => vec![Scope::Global],
            Scope::Workspace(_) => vec![self.clone(), Scope::Global],
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_key())
    }
}

impl FromStr for Scope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "global" => Ok(Scope::Global),
            other => other
                .strip_prefix("workspace:")
                .filter(|id| !id.is_empty())
                .map(|id| Scope::Workspace(id.to_string()))
                .ok_or_else(|| format!("unknown scope `{other}`")),
        }
    }
}

impl Serialize for Scope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_key())
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// How a write relates to what is stored. This is the vocabulary an extractor (user,
/// rule or LLM) uses to express ADD / UPDATE / SUPERSEDE / NOOP decisions.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PutIntent {
    /// Let the ontology decide: a new fact is added, a temporal change supersedes with
    /// history, an equal fact is a no-op that reinforces, a conflicting functional value is
    /// reported and nothing is written.
    #[default]
    Auto,
    /// Explicitly replace `target` (an edit): the target is closed and the new statement,
    /// which must be complete on its own, becomes current. The classes must be related.
    Supersede {
        /// Id of the statement being replaced.
        target: String,
    },
}

/// What a [`StatementStore::put`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PutOutcome {
    /// ADD: stored as a new current statement.
    Added {
        /// The stored statement id.
        id: String,
    },
    /// UPDATE / SUPERSEDE: stored as the current statement; the listed statements were
    /// closed (`valid_to` set) and kept as history.
    Updated {
        /// The stored statement id.
        id: String,
        /// Statements it superseded.
        superseded: Vec<String>,
    },
    /// The incoming fact is older than what is stored: kept as history only.
    Historical {
        /// The stored (already closed) statement id.
        id: String,
        /// The current statement it predates.
        current: String,
    },
    /// NOOP: the same fact is already current. Nothing was stored; the caller may
    /// reinforce `existing`.
    Unchanged {
        /// The current statement that already says this.
        existing: String,
    },
    /// A functional, non-temporal property would change. Nothing was stored; resolve it
    /// explicitly with [`PutIntent::Supersede`].
    Conflict {
        /// The current statement it conflicts with.
        existing: String,
        /// The conflicting properties.
        conflicts: Vec<PropertyChange>,
    },
}

impl PutOutcome {
    /// The id of the statement this write stored, if it stored one.
    pub fn stored_id(&self) -> Option<&str> {
        match self {
            PutOutcome::Added { id } | PutOutcome::Updated { id, .. } => Some(id),
            PutOutcome::Historical { id, .. } => Some(id),
            PutOutcome::Unchanged { .. } | PutOutcome::Conflict { .. } => None,
        }
    }

    /// The id of the statement that is current for this fact after the write.
    pub fn current_id(&self) -> Option<&str> {
        match self {
            PutOutcome::Added { id } | PutOutcome::Updated { id, .. } => Some(id),
            PutOutcome::Historical { current, .. } => Some(current),
            PutOutcome::Unchanged { existing } => Some(existing),
            PutOutcome::Conflict { .. } => None,
        }
    }
}

/// One stored statement with its validity interval.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredStatement {
    /// The statement as written (validated against the ontology before storing).
    pub statement: Statement,
    /// Plain-text rendering used for search and display.
    pub text: String,
    /// Visibility.
    pub scope: Scope,
    /// Id of the ontology source that defines the class (`shodh.core`, a pack, ...).
    pub ontology_source: String,
    /// When the fact became true.
    pub valid_from: DateTime<Utc>,
    /// When it stopped being true (superseded), if it did.
    pub valid_to: Option<DateTime<Utc>>,
    /// The statement that superseded it.
    pub superseded_by: Option<String>,
    /// When its class's expiry rule ends it (a task's deadline plus grace), if ever.
    pub expires_at: Option<DateTime<Utc>>,
    /// When it was forgotten (soft-deleted), if it was.
    pub forgotten_at: Option<DateTime<Utc>>,
    /// When it was stored.
    pub created_at: DateTime<Utc>,
}

impl StoredStatement {
    /// Statement id.
    pub fn id(&self) -> &str {
        &self.statement.id
    }

    /// Whether the statement is current at `at`: started, not superseded, not expired, not
    /// forgotten.
    pub fn is_current_at(&self, at: DateTime<Utc>) -> bool {
        self.forgotten_at.is_none()
            && self.valid_from <= at
            && self.valid_to.is_none_or(|end| end > at)
            && self.expires_at.is_none_or(|end| end > at)
    }
}

/// A property value filter: the property has a value whose canonical text equals `equals`
/// (decimals and money in canonical form, dates `YYYY-MM-DD`, entities `@id`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyFilter {
    /// Property id.
    pub property: String,
    /// Expected canonical value.
    pub equals: String,
}

/// Which statements to return from [`StatementStore::query`] / [`StatementStore::search`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatementQuery {
    /// Only these classes (each including its subclasses). Empty means all.
    pub classes: Vec<String>,
    /// Only statements about this subject entity id.
    pub subject: Option<String>,
    /// Only statements matching every filter.
    pub properties: Vec<PropertyFilter>,
    /// Only these scopes. Empty means all scopes.
    pub scopes: Vec<Scope>,
    /// Only statements whose provenance source starts with one of these prefixes (for
    /// example `conversation://`). Empty means any source.
    pub source_prefixes: Vec<String>,
    /// Current at this time instead of now. Ignored with `include_history`.
    pub as_of: Option<DateTime<Utc>>,
    /// Include superseded and expired statements (forgotten ones are never returned).
    pub include_history: bool,
    /// Maximum rows (default 200, at most 5 000).
    pub limit: Option<usize>,
}

/// A search hit with its fused relevance.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementHit {
    /// The statement.
    pub stored: StoredStatement,
    /// Reciprocal-rank-fusion score of the vector and full-text rankings.
    pub rrf: f64,
    /// Cosine similarity to the query, when the vector search returned the row.
    pub similarity: Option<f64>,
    /// Whether the full-text search matched the row.
    pub lexical: bool,
}

/// One version of a fact in a statement's history, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// Statement id of this version.
    pub statement_id: String,
    /// Canonical values of the requested property (all properties when none was named),
    /// as `property = value` text.
    pub values: Vec<String>,
    /// Start of validity.
    pub valid_from: DateTime<Utc>,
    /// End of validity, if superseded.
    pub valid_to: Option<DateTime<Utc>>,
    /// The statement that superseded this version.
    pub superseded_by: Option<String>,
    /// Plain-text rendering.
    pub text: String,
}

/// Source of "now" for every time-dependent rule; injected so tests control time.
pub trait Clock: Send + Sync {
    /// The current time.
    fn now(&self) -> DateTime<Utc>;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_round_trip_and_see_global() {
        for scope in [Scope::Global, Scope::Workspace("space-1".into())] {
            assert_eq!(scope.as_key().parse::<Scope>().unwrap(), scope);
            let json = serde_json::to_string(&scope).unwrap();
            assert_eq!(serde_json::from_str::<Scope>(&json).unwrap(), scope);
        }
        assert_eq!(Scope::for_workspace(None), Scope::Global);
        assert_eq!(Scope::for_workspace(Some("  ")), Scope::Global);
        assert_eq!(
            Scope::Workspace("w".into()).visible(),
            vec![Scope::Workspace("w".into()), Scope::Global]
        );
        assert!("workspace:".parse::<Scope>().is_err());
        assert!("team".parse::<Scope>().is_err());
    }
}
