//! Long-term memory of the user: typed statements that evolve.
//!
//! Every memory is a validated ontology statement in the [`StatementStore`] (so it has a
//! class, properties, provenance and a validity interval), written only by the user
//! (Settings) or with the user's approval (agent tools) — see [`guard`]. Memories:
//! - **strengthen with use**: a memory an answer used ([`usage`]) is reinforced under its
//!   class dynamics, and Hebbian links between the memories used together strengthen; one
//!   the user marks not relevant loses a little importance and its links to the memories
//!   it came with ([`MemoryService::record_feedback`]);
//! - **fade when unused**: strength decays lazily (exponential, then a power-law tail) at
//!   read time; pinned memories do not decay;
//! - **update when facts change**: a new value of a temporal property supersedes the old
//!   one, which is kept as history; deadlines expire facts (class expiry rules).
//!
//! Recall ranks hybrid-search candidates by relevance, current strength, importance and
//! spreading activation over the links (see [`recall`]).
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod guard;
pub mod learn;
mod recall;
#[cfg(test)]
mod tests;
pub mod usage;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use shodh_ontology::{
    EntityRef, Extractor, Provenance, RawValue, Statement, Value as OntologyValue,
};

use crate::audit::{
    AuditEventType, AuditLog, AuditQuery, AuditRecord, LOCAL_OWNER, MAX_SNIPPET_CHARS,
};
use crate::statements::{
    DynamicsState, PutIntent, PutOutcome, Scope, StatementError, StatementQuery, StatementStore,
    StoredStatement, SELF_ENTITY_ID,
};

pub use guard::{
    check_write_origin, conversation_source, Origin, WriteAuthority, CONVERSATION_SOURCE_PREFIX,
    SETTINGS_SOURCE,
};
pub use recall::{
    render_injection, render_injection_counted, RecallMode, RecallRequest, RecalledMemory,
    DEFAULT_MIN_SIMILARITY, MAX_INJECTION_CHARS, MEMORY_BLOCK_TITLE,
};
pub use usage::DEFAULT_USE_CUTOFF;

/// Most memories one "Not relevant" weakens links to (the others of the same answer).
pub const MAX_FEEDBACK_CO_INJECTED: usize = 32;

/// Longest run id accepted with feedback.
const MAX_RUN_ID_CHARS: usize = 200;

/// The user's feedback on a memory an answer used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feedback {
    /// The memory did not belong in that answer ("Not relevant").
    Irrelevant,
}

/// Provenance source prefixes of memories (as opposed to statements extracted from
/// documents, which share the statement store).
pub fn memory_source_prefixes() -> Vec<String> {
    vec![
        CONVERSATION_SOURCE_PREFIX.to_string(),
        SETTINGS_SOURCE.to_string(),
    ]
}

/// Errors of the memory layer.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// The write is not authorised (it did not come from the user).
    #[error("not allowed: {0}")]
    Forbidden(String),
    /// The request is malformed.
    #[error("{0}")]
    InvalidInput(String),
    /// No memory with this id.
    #[error("no memory with id `{0}`")]
    NotFound(String),
    /// The statement store failed or rejected the statement.
    #[error(transparent)]
    Statement(StatementError),
}

impl From<StatementError> for MemoryError {
    fn from(e: StatementError) -> Self {
        match e {
            StatementError::NotFound(id) => MemoryError::NotFound(id),
            other => MemoryError::Statement(other),
        }
    }
}

/// Result alias of the memory layer.
pub type MemoryResult<T> = Result<T, MemoryError>;

/// What to remember.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryContent {
    /// Free text (the ontology's `Note` class).
    Note {
        /// The note.
        text: String,
    },
    /// A typed fact of an ontology class.
    Fact {
        /// Class id (`Preference`, `Person`, `Project`, `Decision`, `Concept`, ...).
        class: String,
        /// The entity the fact is about, when it is one (`person:self` for the user).
        #[serde(default)]
        subject: Option<EntityRef>,
        /// Property values.
        #[serde(default)]
        properties: BTreeMap<String, RawValue>,
        /// When the fact became true, if not now.
        #[serde(default)]
        valid_from: Option<DateTime<Utc>>,
    },
}

/// How a caller acts, for the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// `agent` or `ui`.
    pub via: &'static str,
    /// Audit principal.
    pub principal: String,
    /// Conversation, when acting in one.
    pub conversation_id: Option<String>,
    /// Agent profile, when acting in a run.
    pub profile_id: Option<String>,
    /// Run, when acting in one.
    pub run_id: Option<String>,
}

impl Actor {
    /// The user in the app UI.
    pub fn ui() -> Self {
        Self {
            via: "ui",
            principal: LOCAL_OWNER.to_string(),
            conversation_id: None,
            profile_id: None,
            run_id: None,
        }
    }

    /// The agent in a run of a conversation.
    pub fn agent(principal: &str, conversation_id: &str, profile_id: &str, run_id: &str) -> Self {
        Self {
            via: "agent",
            principal: principal.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            profile_id: Some(profile_id.to_string()),
            run_id: Some(run_id.to_string()),
        }
    }
}

/// A memory as shown to the user and the agent: the statement plus its live dynamics.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecord {
    /// Statement id.
    pub id: String,
    /// Class id.
    pub class: String,
    /// Class label.
    pub class_label: String,
    /// Text rendering.
    pub text: String,
    /// Subject entity id, if any.
    pub subject: Option<String>,
    /// Property values as written (for editing).
    pub properties: BTreeMap<String, RawValue>,
    /// Property values in canonical form (for display).
    pub values: BTreeMap<String, Vec<String>>,
    /// Visibility.
    pub scope: Scope,
    /// Provenance source (`settings://memory` or `conversation://<id>/turn/<run>`).
    pub source: String,
    /// Conversation the memory was created in, if any.
    pub conversation_id: Option<String>,
    /// Who formulated it (`user` or `llm`).
    pub extractor: String,
    /// Extractor confidence.
    pub confidence: f64,
    /// When the fact became true.
    pub valid_from: DateTime<Utc>,
    /// When it stopped being true (superseded), if it did.
    pub valid_to: Option<DateTime<Utc>>,
    /// The memory that superseded it.
    pub superseded_by: Option<String>,
    /// When it expires under its class rule, if ever.
    pub expires_at: Option<DateTime<Utc>>,
    /// When it was stored.
    pub created_at: DateTime<Utc>,
    /// Current recall strength in `[0, 1]` (decayed to now; 1 when pinned).
    pub strength: f64,
    /// Importance in `[0, 1]`.
    pub importance: f64,
    /// Times recalled and used.
    pub use_count: u32,
    /// Last use.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Exempt from decay.
    pub pinned: bool,
    /// Current now (not superseded, not expired).
    pub current: bool,
    /// Past its class expiry.
    pub expired: bool,
}

/// What [`MemoryService::remember`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RememberOutcome {
    /// The store's decision (added, updated, unchanged, historical, conflict).
    pub outcome: PutOutcome,
    /// The memory that is current for this fact afterwards, if any.
    pub memory: Option<MemoryRecord>,
}

/// Filters for [`MemoryService::list`].
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListRequest {
    /// Only memories visible from this scope (`None`: every scope).
    pub scope: Option<Scope>,
    /// Only these classes (with subclasses).
    pub classes: Vec<String>,
    /// Include superseded and expired versions.
    pub include_history: bool,
    /// Hybrid search text; when set, results are ranked by relevance.
    pub search: Option<String>,
    /// Maximum results (default 500).
    pub limit: Option<usize>,
}

/// Long-term memory over a statement store.
pub struct MemoryService {
    store: Arc<StatementStore>,
    audit: Option<Arc<AuditLog>>,
    /// The suggestions inbox, cleared of a memory's words when it is forgotten.
    inbox: Option<Arc<learn::Inbox>>,
    app_version: String,
}

impl std::fmt::Debug for MemoryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryService").finish_non_exhaustive()
    }
}

const DEFAULT_LIST_LIMIT: usize = 500;

/// `statement_state` key: the one-time erase of memories and snippets only soft-deleted
/// by versions before forgetting erased them ran.
const LEGACY_ERASE_KEY: &str = "legacy_forgotten_erased";
/// `memory_learn_state` key: the one-time clearing of suggestions about memories
/// forgotten before forgetting cleared suggestions ran.
const SUGGESTION_SWEEP_KEY: &str = "forgotten_suggestions_cleared";

/// What [`MemoryService::maintain`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Maintenance {
    /// Memories and snippets only soft-deleted before, erased now (once).
    pub legacy_erased: usize,
    /// Suggestions cleared of memories forgotten before suggestions were cleared (once).
    pub suggestions_cleared: usize,
    /// Inbox approvals of the memory tools whose titles named a memory, renamed (once).
    pub approvals_renamed: usize,
    /// Whether the statement table's history was purged.
    pub purged: bool,
}

impl MemoryService {
    /// Memory over `store`, auditing to `audit` when given.
    pub fn new(
        store: Arc<StatementStore>,
        audit: Option<Arc<AuditLog>>,
        app_version: &str,
    ) -> Self {
        Self {
            store,
            audit,
            inbox: None,
            app_version: app_version.to_string(),
        }
    }

    /// The same service, clearing a forgotten memory's words from `inbox`
    /// ([`MemoryService::forget`]). The learner should share it ([`MemoryService::inbox`]).
    pub fn with_inbox(mut self, inbox: Arc<learn::Inbox>) -> Self {
        self.inbox = Some(inbox);
        self
    }

    /// The suggestions inbox forgetting clears, if one was given.
    pub fn inbox(&self) -> Option<&Arc<learn::Inbox>> {
        self.inbox.as_ref()
    }

    /// The statement store.
    pub fn store(&self) -> &Arc<StatementStore> {
        &self.store
    }

    /// The audit log, if writes are audited.
    pub fn audit_log(&self) -> Option<&Arc<AuditLog>> {
        self.audit.as_ref()
    }

    /// The app version recorded as the user's extractor version.
    pub fn app_version(&self) -> &str {
        &self.app_version
    }

    /// Stores a memory after checking it comes from the user ([`check_write_origin`]) and
    /// validating it against the ontology. Equal facts reinforce; temporal changes
    /// supersede with history; conflicts are returned without writing. Audited as
    /// `memory_write`.
    pub async fn remember(
        &self,
        content: MemoryContent,
        scope: Scope,
        origin: &Origin,
        actor: &Actor,
    ) -> MemoryResult<RememberOutcome> {
        check_write_origin(origin)?;
        let statement = self.build_statement(content, origin)?;
        let outcome = self
            .store
            .put(statement, scope.clone(), PutIntent::Auto)
            .await?;
        self.after_write("remember", &outcome, &scope, origin, actor)
            .await
    }

    /// Replaces memory `id` (an edit); the old version is closed and kept as history.
    /// With `merge`, properties given in `content` override the old ones and the rest are
    /// kept (the agent changes one detail); without it, `content` is the complete new fact
    /// (the Settings editor). Notes are replaced by the new text. `visible_from` limits
    /// which memories the caller may edit (`None`: any, for the UI). Audited as
    /// `memory_write`.
    pub async fn update(
        &self,
        id: &str,
        content: MemoryContent,
        merge: bool,
        visible_from: Option<&Scope>,
        origin: &Origin,
        actor: &Actor,
    ) -> MemoryResult<RememberOutcome> {
        let (target, statement) = self
            .prepare_update(id, content, merge, visible_from, origin)
            .await?;
        let scope = target.scope.clone();
        let outcome = self
            .store
            .put(
                statement,
                scope.clone(),
                PutIntent::Supersede {
                    target: id.to_string(),
                },
            )
            .await?;
        self.after_write("update", &outcome, &scope, origin, actor)
            .await
    }

    /// Checks and validates a write without storing it; returns the text that would be
    /// remembered (for an approval prompt).
    pub fn preview(&self, content: MemoryContent, origin: &Origin) -> MemoryResult<String> {
        check_write_origin(origin)?;
        let statement = self.build_statement(content, origin)?;
        self.render(&statement)
    }

    /// Checks and validates an edit without storing it; returns the memory as it is now
    /// and the text it would have after the edit (for an approval prompt).
    pub async fn preview_update(
        &self,
        id: &str,
        content: MemoryContent,
        merge: bool,
        visible_from: Option<&Scope>,
        origin: &Origin,
    ) -> MemoryResult<(MemoryRecord, String)> {
        let (target, statement) = self
            .prepare_update(id, content, merge, visible_from, origin)
            .await?;
        let state = self.state_of(&target).await?;
        let after = self.render(&statement)?;
        Ok((self.record(&target, &state), after))
    }

    async fn prepare_update(
        &self,
        id: &str,
        content: MemoryContent,
        merge: bool,
        visible_from: Option<&Scope>,
        origin: &Origin,
    ) -> MemoryResult<(StoredStatement, Statement)> {
        check_write_origin(origin)?;
        let target = self.memory_statement(id).await?;
        self.check_visible(&target, visible_from)?;
        if target.valid_to.is_some() {
            return Err(MemoryError::InvalidInput(format!(
                "memory `{id}` was already replaced by `{}`; edit the current version",
                target.superseded_by.as_deref().unwrap_or("a newer version")
            )));
        }
        let content = match content {
            MemoryContent::Fact {
                class,
                subject,
                properties,
                valid_from,
            } if merge => {
                let mut merged = target.statement.properties.clone();
                for (name, value) in properties {
                    merged.insert(name, value);
                }
                MemoryContent::Fact {
                    class,
                    subject: subject.or_else(|| target.statement.subject.clone()),
                    properties: merged,
                    valid_from,
                }
            }
            other => other,
        };
        let statement = self.build_statement(content, origin)?;
        Ok((target, statement))
    }

    fn render(&self, statement: &Statement) -> MemoryResult<String> {
        let ontology = self.store.ontology();
        let valid = ontology
            .validate(statement)
            .map_err(|v| MemoryError::Statement(StatementError::Invalid(v)))?;
        Ok(crate::statements::render_text(ontology, &valid))
    }

    /// Forgets memory `id` and every earlier or later version of the same fact, erasing
    /// it rather than hiding it, in an order that lets a failed forget be retried:
    /// 1. suggestions that stored it or are about it lose its words and are marked
    ///    forgotten, and the learning model's rejected answers of the turns it came from
    ///    are deleted ([`learn::Inbox::forget_memories`], when an inbox was given);
    /// 2. its dynamics and links are deleted and its rows deleted from the statement table
    ///    ([`StatementStore::erase`]); memories only soft-forgotten before (by earlier
    ///    versions of the app, or undone learned writes) are erased with it;
    /// 3. the table's old versions, which still hold the rows, are purged in the
    ///    background ([`StatementStore::schedule_purge`]).
    ///
    /// Returns the forgotten ids. Audited as `memory_forget` with the ids, class, scope,
    /// version count and text length, never the text. The conversation the memory came from is not changed.
    pub async fn forget(
        &self,
        id: &str,
        visible_from: Option<&Scope>,
        actor: &Actor,
    ) -> MemoryResult<Vec<String>> {
        let target = self.memory_statement(id).await?;
        self.check_visible(&target, visible_from)?;
        // Only memories: a document's statement about the same thing is not the user's.
        let versions: Vec<StoredStatement> = self
            .store
            .get_many(&self.store.history_ids(id).await?)
            .await?
            .into_iter()
            .filter(is_memory)
            .collect();
        let ids: Vec<String> = versions.iter().map(|v| v.id().to_string()).collect();
        let distinct = |values: Vec<String>| -> Vec<String> {
            values
                .into_iter()
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect()
        };
        let forgotten_memories = learn::ForgottenMemories {
            ids: ids.clone(),
            turns: distinct(versions.iter().filter_map(turn_of).collect()),
            sources: distinct(
                versions
                    .iter()
                    .filter_map(|v| v.statement.provenance.as_ref().map(|p| p.source.clone()))
                    .collect(),
            ),
        };
        if let Some(inbox) = &self.inbox {
            let (inbox, now) = (inbox.clone(), self.store.now());
            tokio::task::spawn_blocking(move || inbox.forget_memories(&forgotten_memories, now))
                .await
                .map_err(|e| MemoryError::Statement(StatementError::Task(e.to_string())))?
                .map_err(|e| MemoryError::Statement(StatementError::Sqlite(e.to_string())))?;
        }
        let forgotten = self.store.erase(&ids).await?;
        match self
            .store
            .erase_forgotten(&memory_source_prefixes(), &[])
            .await
        {
            Ok(0) => {}
            Ok(n) => {
                tracing::info!(target: "shodh::memory", erased = n, "memories forgotten earlier were erased")
            }
            Err(e) => {
                tracing::warn!(target: "shodh::memory", error = %e, "memories forgotten earlier were not erased; the next forget retries")
            }
        }
        self.store.schedule_purge();
        self.audit(
            actor,
            AuditEventType::MemoryForget,
            json!({
                "action": "forget",
                "id": id,
                "versions": forgotten.len(),
                "ids": forgotten,
                "class": target.statement.class,
                "scope": target.scope.as_key(),
                "text_chars": text_chars(&target.text),
                "via": actor.via,
            }),
        );
        Ok(forgotten)
    }

    /// Starts [`MemoryService::maintain`] in the background (when the memory opens), so
    /// nothing waits for it. Failures are logged; the next start tries again.
    pub fn start_maintenance(self: &Arc<Self>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(target: "shodh::memory", "no runtime: memory maintenance skipped");
            return;
        };
        let service = Arc::clone(self);
        runtime.spawn(async move {
            match service.maintain().await {
                Ok(done) => {
                    tracing::info!(target: "shodh::memory", legacy_erased = done.legacy_erased, suggestions_cleared = done.suggestions_cleared, approvals_renamed = done.approvals_renamed, purged = done.purged, "memory maintenance done")
                }
                Err(e) => {
                    tracing::warn!(target: "shodh::memory", error = %e, "memory maintenance failed; the next start retries")
                }
            }
        });
    }

    /// The cleanups that finish what forgetting started, for data an earlier run (or an
    /// earlier version) left:
    /// 1. once: memories and snippets only soft-deleted by versions before forgetting
    ///    erased them are erased ([`StatementStore::erase_forgotten`]);
    /// 2. once, with an inbox: the Inbox's approval items of the memory tools get wordless
    ///    titles, and (with an audit log) suggestions about memories forgotten before
    ///    suggestions were cleared lose their words. The ids come from the audit log's
    ///    `memory_forget` events (they always carried them); a forget whose event retention
    ///    removed is cleared when the user presses Undo on its suggestion instead;
    /// 3. the statement table's history is purged when an erase still waits for it (the
    ///    marker [`StatementStore::purge_pending`], also set by a crash between an erase
    ///    and its purge) or the one-time erase had not run before; otherwise nothing is
    ///    compacted.
    pub async fn maintain(&self) -> MemoryResult<Maintenance> {
        let mut done = Maintenance::default();
        let dynamics = self.store.dynamics().clone();
        let legacy_ran = blocking({
            let dynamics = dynamics.clone();
            move || dynamics.state(LEGACY_ERASE_KEY)
        })
        .await?
        .is_some();
        if !legacy_ran {
            done.legacy_erased = self
                .store
                .erase_forgotten(&memory_source_prefixes(), &[])
                .await?
                + self
                    .store
                    .erase_forgotten(&[], &[crate::research::snippets::SNIPPET_CLASS])
                    .await?;
            let at = self.store.now().to_rfc3339();
            blocking(move || dynamics.set_state(LEGACY_ERASE_KEY, &at)).await?;
        }
        if let Some(inbox) = self.inbox.clone() {
            let log = self.audit.clone();
            let now = self.store.now();
            (done.suggestions_cleared, done.approvals_renamed) =
                tokio::task::spawn_blocking(move || -> learn::LearnResult<(usize, usize)> {
                    if inbox.state(SUGGESTION_SWEEP_KEY)?.is_some() {
                        return Ok((0, 0));
                    }
                    let renamed = inbox.clear_memory_approval_titles()?;
                    let cleared = match &log {
                        Some(log) => {
                            let forgotten = learn::ForgottenMemories {
                                ids: forgotten_ids(log)?,
                                ..Default::default()
                            };
                            inbox.forget_memories(&forgotten, now)?
                        }
                        None => 0,
                    };
                    inbox.set_state(SUGGESTION_SWEEP_KEY, &now.to_rfc3339())?;
                    Ok((cleared, renamed))
                })
                .await
                .map_err(|e| MemoryError::Statement(StatementError::Task(e.to_string())))?
                .map_err(|e| MemoryError::Statement(StatementError::Sqlite(e.to_string())))?;
        }
        if !legacy_ran || self.store.purge_pending()? {
            self.store.purge_history().await?;
            done.purged = true;
        }
        Ok(done)
    }

    /// Whether the audit log still holds words of memory `id` (of any of its versions):
    /// events written before memory events stopped carrying the text. The forget
    /// confirmation says so. `false` when writes are not audited.
    pub async fn audit_holds_words(&self, id: &str) -> MemoryResult<bool> {
        let Some(log) = self.audit.clone() else {
            return Ok(false);
        };
        self.memory_statement(id).await?;
        let ids = self.store.history_ids(id).await?;
        tokio::task::spawn_blocking(move || -> Result<bool, crate::audit::AuditError> {
            for version in &ids {
                let rows = log.query(&AuditQuery {
                    types: vec![AuditEventType::MemoryWrite, AuditEventType::MemoryForget],
                    text: Some(version.clone()),
                    limit: Some(1_000),
                    ..Default::default()
                })?;
                let worded = rows.iter().any(|row| {
                    ["text", "reason"].iter().any(|key| {
                        row.payload
                            .get(*key)
                            .and_then(JsonValue::as_str)
                            .is_some_and(|t| !t.trim().is_empty())
                    })
                });
                if worded {
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .await
        .map_err(|e| MemoryError::Statement(StatementError::Task(e.to_string())))?
        .map_err(|e| MemoryError::Statement(StatementError::Sqlite(e.to_string())))
    }

    /// Pins (exempts from decay) or unpins memory `id`. Audited as `memory_write`.
    pub async fn set_pinned(
        &self,
        id: &str,
        pinned: bool,
        actor: &Actor,
    ) -> MemoryResult<MemoryRecord> {
        let target = self.memory_statement(id).await?;
        let now = self.store.now();
        let dynamics = self.store.dynamics().clone();
        let state = self.state_of(&target).await?;
        let mut state = state;
        state.set_pinned(pinned, now);
        let (sid, scope, class) = (
            target.id().to_string(),
            target.scope.as_key(),
            target.statement.class.clone(),
        );
        let to_write = state.clone();
        blocking(move || dynamics.put(&sid, &scope, &class, &to_write, now)).await?;
        self.audit(
            actor,
            AuditEventType::MemoryWrite,
            json!({
                "action": if pinned { "pin" } else { "unpin" },
                "id": id,
                "class": target.statement.class,
                "scope": target.scope.as_key(),
                "text_chars": text_chars(&target.text),
                "via": actor.via,
            }),
        );
        Ok(self.record(&target, &state))
    }

    /// One memory with its live dynamics.
    pub async fn get(&self, id: &str) -> MemoryResult<MemoryRecord> {
        let stored = self.memory_statement(id).await?;
        let state = self.state_of(&stored).await?;
        Ok(self.record(&stored, &state))
    }

    /// Every version of the fact memory `id` belongs to, oldest first.
    pub async fn history(&self, id: &str) -> MemoryResult<Vec<MemoryRecord>> {
        self.memory_statement(id).await?;
        let ids = self.store.history_ids(id).await?;
        let stored = self.store.get_many(&ids).await?;
        self.records(stored).await
    }

    /// Memories matching `request`. Listing is not use: nothing is reinforced.
    pub async fn list(&self, request: &ListRequest) -> MemoryResult<Vec<MemoryRecord>> {
        let limit = request.limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, 5_000);
        let query = StatementQuery {
            classes: request.classes.clone(),
            scopes: request
                .scope
                .as_ref()
                .map(Scope::visible)
                .unwrap_or_default(),
            source_prefixes: memory_source_prefixes(),
            include_history: request.include_history,
            limit: Some(limit),
            ..Default::default()
        };
        let stored = match request
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(text) => self
                .store
                .search(text, &query, limit)
                .await?
                .into_iter()
                .map(|hit| hit.stored)
                .collect(),
            None => self.store.query(&query).await?,
        };
        self.records(stored).await
    }

    /// Every memory (current and history) as a JSON document for export.
    pub async fn export(&self) -> MemoryResult<JsonValue> {
        let memories = self
            .list(&ListRequest {
                include_history: true,
                limit: Some(5_000),
                ..Default::default()
            })
            .await?;
        let ontology = self.store.ontology();
        Ok(json!({
            "format": "shodh.memory.export",
            "version": 1,
            "exportedAt": self.store.now(),
            "ontology": { "id": ontology.id(), "version": ontology.version().to_string() },
            "memories": memories,
        }))
    }

    /// Recall: see [`recall`]. In [`RecallMode::Use`], the recalled memories are
    /// reinforced, their links strengthened and the use audited as `memory_use`.
    pub async fn recall(
        &self,
        request: &RecallRequest,
        actor: &Actor,
    ) -> MemoryResult<Vec<RecalledMemory>> {
        let recalled = recall::rank(self, request).await?;
        if request.mode == RecallMode::Use {
            self.record_use(&recalled, request, actor).await?;
        }
        Ok(recalled)
    }

    /// Records that `recalled` (from a [`RecallMode::Inspect`] recall for `request`) was
    /// used: reinforces each memory, strengthens links between them and audits
    /// `memory_use`. Lets a caller bound the read-only ranking with a timeout without ever
    /// cancelling a half-recorded use.
    pub async fn record_use(
        &self,
        recalled: &[RecalledMemory],
        request: &RecallRequest,
        actor: &Actor,
    ) -> MemoryResult<()> {
        if recalled.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = recalled.iter().map(|m| m.memory.id.clone()).collect();
        recall::record_use(self, &ids).await?;
        self.audit(
            actor,
            AuditEventType::MemoryUse,
            json!({
                "action": "recall",
                "ids": recalled.iter().map(|m| m.memory.id.clone()).collect::<Vec<_>>(),
                "query": snippet(&request.query),
                "scope": request.scope.as_key(),
                "via": actor.via,
            }),
        );
        Ok(())
    }

    /// The end of an answer that memories `injected` (ids, in injection order) were put in
    /// front of: the ones `answer` used ([`usage::judge_use`] at
    /// [`DEFAULT_USE_CUTOFF`]) are reinforced and linked to each other, and the use is
    /// audited as `memory_use` with their ids only. Returns the ids used.
    ///
    /// Fails closed: when the embedder is unavailable or fails, nothing is reinforced (and
    /// the failure is only logged), so a missing model never strengthens a memory that did
    /// not help. Memories forgotten during the answer are skipped.
    pub async fn record_answer_use(
        &self,
        injected: &[String],
        answer: &str,
        scope: &Scope,
        actor: &Actor,
    ) -> MemoryResult<Vec<String>> {
        if injected.is_empty() || answer.trim().is_empty() {
            return Ok(Vec::new());
        }
        let memories: Vec<(String, String)> = self
            .store
            .get_many(injected)
            .await?
            .into_iter()
            .filter(is_memory)
            .map(|s| (s.id().to_string(), s.text.clone()))
            .collect();
        if memories.is_empty() {
            return Ok(Vec::new());
        }
        let embedder = match self.store.embedder().await {
            Ok(embedder) => embedder,
            Err(e) => {
                tracing::debug!(target: "shodh::memory", error = %e, "no embedder at the end of the answer; no memory reinforced");
                return Ok(Vec::new());
            }
        };
        let answer = answer.to_string();
        let judged = tokio::task::spawn_blocking(move || {
            usage::judge_use(&*embedder, &memories, &answer, DEFAULT_USE_CUTOFF)
        })
        .await
        .map_err(|e| MemoryError::Statement(StatementError::Task(e.to_string())))?;
        let judged = match judged {
            Ok(judged) => judged,
            Err(e) => {
                tracing::debug!(target: "shodh::memory", error = %e, "embedding the answer failed; no memory reinforced");
                return Ok(Vec::new());
            }
        };
        let used: Vec<String> = judged
            .into_iter()
            .filter(|j| j.used)
            .map(|j| j.id)
            .collect();
        if used.is_empty() {
            return Ok(used);
        }
        recall::record_use(self, &used).await?;
        self.audit(
            actor,
            AuditEventType::MemoryUse,
            json!({
                "action": "used_in_answer",
                "ids": used,
                "considered": injected.len(),
                "scope": scope.as_key(),
                "via": actor.via,
            }),
        );
        Ok(used)
    }

    /// The user's `feedback` on memory `id` as used in run `run_id`, with `co_injected`
    /// the other memories put in front of the same answer. [`Feedback::Irrelevant`] lowers
    /// the memory's importance a little (bounded by the importance floor,
    /// [`DynamicsState::mark_irrelevant`]) and weakens its links to `co_injected`; it never
    /// edits, hides or deletes the memory. Applied once per run and memory: returns whether
    /// this call applied it (`false` when it was recorded before). Audited as
    /// `memory_write` with ids only.
    pub async fn record_feedback(
        &self,
        run_id: &str,
        id: &str,
        feedback: Feedback,
        co_injected: &[String],
        actor: &Actor,
    ) -> MemoryResult<bool> {
        let run_id = run_id.trim();
        if run_id.is_empty() || run_id.chars().count() > MAX_RUN_ID_CHARS {
            return Err(MemoryError::InvalidInput("invalid answer id".to_string()));
        }
        let target = self.memory_statement(id).await?;
        let co: Vec<String> = co_injected
            .iter()
            .filter(|other| other.as_str() != id)
            .take(MAX_FEEDBACK_CO_INJECTED)
            .cloned()
            .collect();
        let fallback = default_state(&target);
        let now = self.store.now();
        let dynamics = self.store.dynamics().clone();
        let (run, sid, scope, class) = (
            run_id.to_string(),
            target.id().to_string(),
            target.scope.as_key(),
            target.statement.class.clone(),
        );
        let links = co.clone();
        let applied = match feedback {
            Feedback::Irrelevant => {
                blocking(move || {
                    dynamics.mark_irrelevant(&run, &sid, &scope, &class, &fallback, &links, now)
                })
                .await?
            }
        };
        let Some(state) = applied else {
            return Ok(false);
        };
        self.audit(
            actor,
            AuditEventType::MemoryWrite,
            json!({
                "action": "not_relevant",
                "id": id,
                "answer": run_id,
                "co_injected": co,
                "importance": state.importance,
                "class": target.statement.class,
                "scope": target.scope.as_key(),
                "via": actor.via,
            }),
        );
        Ok(true)
    }

    /// Which of `ids` the user marked not relevant in run `run_id`.
    pub async fn marked_irrelevant(
        &self,
        run_id: &str,
        ids: &[String],
    ) -> MemoryResult<BTreeSet<String>> {
        let dynamics = self.store.dynamics().clone();
        let (run, ids) = (run_id.to_string(), ids.to_vec());
        blocking(move || dynamics.marked_irrelevant(&run, &ids)).await
    }

    /// The memories `ids` as they are now, in order: `None` for one that was forgotten
    /// (or is not a memory). Reading is not use: nothing is reinforced.
    pub async fn lookup(&self, ids: &[String]) -> MemoryResult<Vec<Option<MemoryRecord>>> {
        let stored: Vec<StoredStatement> = self
            .store
            .get_many(ids)
            .await?
            .into_iter()
            .filter(is_memory)
            .collect();
        let states = self.states_of(&stored).await?;
        let by_id: HashMap<&str, &StoredStatement> = stored.iter().map(|s| (s.id(), s)).collect();
        Ok(ids
            .iter()
            .map(|id| {
                by_id.get(id.as_str()).map(|s| {
                    let state = states
                        .get(s.id())
                        .cloned()
                        .unwrap_or_else(|| default_state(s));
                    self.record(s, &state)
                })
            })
            .collect())
    }

    async fn after_write(
        &self,
        action: &str,
        outcome: &PutOutcome,
        scope: &Scope,
        origin: &Origin,
        actor: &Actor,
    ) -> MemoryResult<RememberOutcome> {
        let memory = match outcome.current_id() {
            Some(id) => Some(self.get(id).await?),
            None => None,
        };
        if outcome.stored_id().is_some() || matches!(outcome, PutOutcome::Unchanged { .. }) {
            let (decision, superseded) = match outcome {
                PutOutcome::Added { .. } => ("added", Vec::new()),
                PutOutcome::Updated { superseded, .. } => ("updated", superseded.clone()),
                PutOutcome::Historical { .. } => ("historical", Vec::new()),
                PutOutcome::Unchanged { .. } => ("unchanged", Vec::new()),
                PutOutcome::Conflict { .. } => ("conflict", Vec::new()),
            };
            self.audit(
                actor,
                AuditEventType::MemoryWrite,
                json!({
                    "action": action,
                    "outcome": decision,
                    "id": outcome.stored_id().or(outcome.current_id()),
                    "superseded": superseded,
                    "class": memory.as_ref().map(|m| m.class.clone()),
                    "scope": scope.as_key(),
                    "text_chars": memory.as_ref().map(|m| text_chars(&m.text)),
                    "source": origin.source,
                    "extractor": origin.extractor.label(),
                    "approval": match &origin.authority {
                        WriteAuthority::UserApproval { step_id } => Some(step_id.clone()),
                        WriteAuthority::LearnPolicy { proposal_id } => Some(proposal_id.clone()),
                        WriteAuthority::UserInterface => None,
                    },
                    "authority": origin.authority.label(),
                    "via": actor.via,
                }),
            );
        }
        Ok(RememberOutcome {
            outcome: outcome.clone(),
            memory,
        })
    }

    fn build_statement(&self, content: MemoryContent, origin: &Origin) -> MemoryResult<Statement> {
        self.build_statement_at(content, origin, self.store.now())
    }

    /// The statement for `content`, extracted at `extracted_at` (a learned memory keeps
    /// the time of the turn it came from, not the time it was accepted).
    pub(crate) fn build_statement_at(
        &self,
        content: MemoryContent,
        origin: &Origin,
        extracted_at: DateTime<Utc>,
    ) -> MemoryResult<Statement> {
        let now = extracted_at;
        let (class, subject, mut properties, valid_from) = match content {
            MemoryContent::Note { text } => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    return Err(MemoryError::InvalidInput("the note is empty".to_string()));
                }
                let mut properties = BTreeMap::new();
                properties.insert("noteText".to_string(), RawValue::Text(text));
                ("Note".to_string(), None, properties, None)
            }
            MemoryContent::Fact {
                class,
                subject,
                properties,
                valid_from,
            } => (class.trim().to_string(), subject, properties, valid_from),
        };
        let ontology = self.store.ontology();
        let Some(class_def) = ontology.class(&class) else {
            return Err(MemoryError::InvalidInput(format!(
                "unknown memory class `{class}`"
            )));
        };
        for value in properties.values_mut() {
            normalise(value);
        }
        if ontology.is_subclass_of(&class, "Preference") {
            // The holder defaults to the user; topics compare case-insensitively.
            properties
                .entry("preferenceHolder".to_string())
                .or_insert_with(|| RawValue::Entity(EntityRef::typed(SELF_ENTITY_ID, "Person")));
            if let Some(RawValue::Text(topic)) = properties.get_mut("preferenceTopic") {
                *topic = topic.to_lowercase();
            }
        }
        let version = ontology
            .source(&class_def.source)
            .map(|s| s.version.clone())
            .unwrap_or_else(|| ontology.version().clone());
        Ok(Statement {
            id: format!("mem-{}", uuid::Uuid::new_v4()),
            class,
            subject,
            properties,
            ontology_version: version,
            valid_from,
            provenance: Some(Provenance {
                source: origin.source.clone(),
                generation: 0,
                page: None,
                span: None,
                extractor: Extractor {
                    kind: origin.extractor,
                    version: origin.extractor_version.clone(),
                },
                confidence: origin.confidence,
                extracted_at: now,
            }),
        })
    }

    /// Stores a statement formulated by the learning pipeline (see [`learn`]), after
    /// [`check_write_origin`]. A write under the user's automatic-learning policy
    /// ([`WriteAuthority::LearnPolicy`]) is refused here when the statement is sensitive
    /// ([`learn::sensitivity::classify`]), whatever the caller decided: sensitive memories
    /// are only ever stored with the user's explicit approval. Audited as `memory_write`
    /// with action `learn`.
    pub async fn put_learned(
        &self,
        statement: Statement,
        scope: Scope,
        intent: PutIntent,
        origin: &Origin,
        actor: &Actor,
    ) -> MemoryResult<RememberOutcome> {
        self.check_learned(&statement, origin)?;
        let outcome = self.store.put(statement, scope.clone(), intent).await?;
        self.after_write("learn", &outcome, &scope, origin, actor)
            .await
    }

    /// [`put_learned`](Self::put_learned) for several statements, stored together
    /// ([`StatementStore::put_many`]); each is checked and audited as if written alone.
    /// Returns each one's result in order.
    pub async fn put_learned_many(
        &self,
        items: Vec<(Statement, Scope, PutIntent, Origin)>,
        actor: &Actor,
    ) -> MemoryResult<Vec<MemoryResult<RememberOutcome>>> {
        let mut results: Vec<Option<MemoryResult<RememberOutcome>>> = Vec::new();
        let mut batch = Vec::new();
        let mut written = Vec::new();
        for (index, (statement, scope, intent, origin)) in items.into_iter().enumerate() {
            match self.check_learned(&statement, &origin) {
                Ok(()) => {
                    batch.push((statement, scope.clone(), intent));
                    written.push((index, scope, origin));
                    results.push(None);
                }
                Err(e) => results.push(Some(Err(e))),
            }
        }
        let outcomes = self.store.put_many(batch).await?;
        for ((index, scope, origin), outcome) in written.into_iter().zip(outcomes) {
            results[index] = Some(match outcome {
                Ok(outcome) => {
                    self.after_write("learn", &outcome, &scope, &origin, actor)
                        .await
                }
                Err(e) => Err(e.into()),
            });
        }
        Ok(results
            .into_iter()
            // The store returns one outcome per statement, so every slot is filled.
            .map(|r| r.unwrap_or_else(|| Err(MemoryError::InvalidInput("not written".into()))))
            .collect())
    }

    /// The checks of a learned write: its origin ([`check_write_origin`]), the ontology,
    /// and under the automatic-learning policy, that it is not sensitive.
    fn check_learned(&self, statement: &Statement, origin: &Origin) -> MemoryResult<()> {
        check_write_origin(origin)?;
        let ontology = self.store.ontology();
        let valid = ontology
            .validate(statement)
            .map_err(|v| MemoryError::Statement(StatementError::Invalid(v)))?;
        if matches!(origin.authority, WriteAuthority::LearnPolicy { .. }) {
            let reasons = learn::sensitivity::classify(ontology, &valid);
            if !reasons.is_empty() {
                return Err(MemoryError::Forbidden(format!(
                    "sensitive memories ({}) are never stored automatically; they need the user's approval",
                    reasons
                        .iter()
                        .map(|r| r.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        Ok(())
    }

    /// Undoes a learned write: forgets the stored memory `id` and reopens what it
    /// superseded. Refused when the memory changed since. Audited as `memory_write`.
    pub async fn revert_learned(&self, id: &str, actor: &Actor) -> MemoryResult<Vec<String>> {
        let target = self.memory_statement(id).await?;
        let reopened = self.store.revert(id).await?;
        self.audit_change(
            actor,
            "undo_learned",
            &target,
            json!({ "reopened": reopened }),
        );
        Ok(reopened)
    }

    /// Archives a current memory (soft and reversible: it is closed without a successor
    /// and kept as history). Audited as `memory_write`.
    pub async fn archive(&self, id: &str, actor: &Actor) -> MemoryResult<()> {
        let target = self.memory_statement(id).await?;
        self.store.archive(id).await?;
        self.audit_change(actor, "archive", &target, json!({}));
        Ok(())
    }

    /// Reopens an archived memory. Audited as `memory_write`.
    pub async fn unarchive(&self, id: &str, actor: &Actor) -> MemoryResult<()> {
        let target = self.memory_statement(id).await?;
        self.store.unarchive(id).await?;
        self.audit_change(actor, "unarchive", &target, json!({}));
        Ok(())
    }

    /// Resolves a contradiction: memory `retire` is closed as superseded by `keep`.
    /// Audited as `memory_write`.
    pub async fn retire(&self, retire: &str, keep: &str, actor: &Actor) -> MemoryResult<()> {
        let target = self.memory_statement(retire).await?;
        self.memory_statement(keep).await?;
        self.store.retire(retire, keep).await?;
        self.audit_change(actor, "resolve_conflict", &target, json!({ "keep": keep }));
        Ok(())
    }

    /// Undoes [`MemoryService::retire`]. Audited as `memory_write`.
    pub async fn unretire(&self, retire: &str, keep: &str, actor: &Actor) -> MemoryResult<()> {
        let target = self.memory_statement(retire).await?;
        self.store.unretire(retire, keep).await?;
        self.audit_change(
            actor,
            "undo_resolve_conflict",
            &target,
            json!({ "keep": keep }),
        );
        Ok(())
    }

    fn audit_change(
        &self,
        actor: &Actor,
        action: &str,
        target: &StoredStatement,
        extra: JsonValue,
    ) {
        let mut payload = json!({
            "action": action,
            "id": target.id(),
            "class": target.statement.class,
            "scope": target.scope.as_key(),
            "text_chars": text_chars(&target.text),
            "extractor": "llm",
            "via": actor.via,
        });
        if let (Some(map), JsonValue::Object(extra)) = (payload.as_object_mut(), extra) {
            map.extend(extra);
        }
        self.audit(actor, AuditEventType::MemoryWrite, payload);
    }

    /// A memory statement (not a document-derived statement) by id.
    pub(crate) async fn memory_statement(&self, id: &str) -> MemoryResult<StoredStatement> {
        let stored = self.store.get(id).await?;
        if is_memory(&stored) {
            Ok(stored)
        } else {
            Err(MemoryError::NotFound(id.to_string()))
        }
    }

    fn check_visible(
        &self,
        target: &StoredStatement,
        visible_from: Option<&Scope>,
    ) -> MemoryResult<()> {
        match visible_from {
            Some(scope) if !scope.visible().contains(&target.scope) => {
                Err(MemoryError::NotFound(target.id().to_string()))
            }
            _ => Ok(()),
        }
    }

    async fn state_of(&self, stored: &StoredStatement) -> MemoryResult<DynamicsState> {
        let dynamics = self.store.dynamics().clone();
        let id = stored.id().to_string();
        let state = blocking(move || dynamics.get(&id)).await?;
        Ok(state.unwrap_or_else(|| default_state(stored)))
    }

    pub(crate) async fn states_of(
        &self,
        stored: &[StoredStatement],
    ) -> MemoryResult<HashMap<String, DynamicsState>> {
        let dynamics = self.store.dynamics().clone();
        let ids: Vec<String> = stored.iter().map(|s| s.id().to_string()).collect();
        let mut states = blocking(move || dynamics.get_many(&ids)).await?;
        for s in stored {
            states
                .entry(s.id().to_string())
                .or_insert_with(|| default_state(s));
        }
        Ok(states)
    }

    async fn records(&self, stored: Vec<StoredStatement>) -> MemoryResult<Vec<MemoryRecord>> {
        let states = self.states_of(&stored).await?;
        Ok(stored
            .iter()
            .map(|s| {
                let state = states
                    .get(s.id())
                    .cloned()
                    .unwrap_or_else(|| default_state(s));
                self.record(s, &state)
            })
            .collect())
    }

    pub(crate) fn record(&self, stored: &StoredStatement, state: &DynamicsState) -> MemoryRecord {
        let ontology = self.store.ontology();
        let now = self.store.now();
        let class = ontology.class(&stored.statement.class);
        let strength = class
            .map(|c| state.strength_at(&c.dynamics, now))
            .unwrap_or(state.strength);
        let values = match ontology.validate(&stored.statement) {
            Ok(valid) => valid
                .properties()
                .iter()
                .map(|(name, values)| {
                    (
                        name.clone(),
                        values.iter().map(display_value).collect::<Vec<_>>(),
                    )
                })
                .collect(),
            Err(_) => BTreeMap::new(),
        };
        let provenance = stored.statement.provenance.as_ref();
        let source = provenance.map(|p| p.source.clone()).unwrap_or_default();
        let conversation_id = source
            .strip_prefix(CONVERSATION_SOURCE_PREFIX)
            .and_then(|rest| rest.split("/turn/").next())
            .map(str::to_string);
        MemoryRecord {
            id: stored.id().to_string(),
            class: stored.statement.class.clone(),
            class_label: class
                .map(|c| c.label.clone())
                .unwrap_or_else(|| stored.statement.class.clone()),
            text: stored.text.clone(),
            subject: stored.statement.subject.as_ref().map(|s| s.id.clone()),
            properties: stored.statement.properties.clone(),
            values,
            scope: stored.scope.clone(),
            source,
            conversation_id,
            extractor: provenance
                .map(|p| p.extractor.kind.label().to_string())
                .unwrap_or_default(),
            confidence: provenance.map(|p| p.confidence).unwrap_or(0.0),
            valid_from: stored.valid_from,
            valid_to: stored.valid_to,
            superseded_by: stored.superseded_by.clone(),
            expires_at: stored.expires_at,
            created_at: stored.created_at,
            strength,
            importance: state.importance,
            use_count: state.use_count,
            last_used_at: state.last_used_at,
            pinned: state.pinned,
            current: stored.is_current_at(now),
            expired: stored.expires_at.is_some_and(|at| at <= now),
        }
    }

    fn audit(&self, actor: &Actor, event_type: AuditEventType, payload: JsonValue) {
        let Some(log) = &self.audit else {
            return;
        };
        let mut record = AuditRecord::new(event_type, payload).principal(actor.principal.clone());
        if let Some(c) = &actor.conversation_id {
            record = record.conversation(c.clone());
        }
        if let Some(p) = &actor.profile_id {
            record = record.profile(p.clone());
        }
        if let Some(r) = &actor.run_id {
            record = record.run(r.clone());
        }
        log.submit(record);
    }
}

/// Dynamics of a statement whose SQLite row is missing (the two stores are not written
/// in one transaction): full strength from creation.
fn default_state(stored: &StoredStatement) -> DynamicsState {
    DynamicsState::fresh(
        stored.created_at,
        crate::statements::dynamics::IMPORTANCE_FLOOR,
    )
}

fn display_value(value: &OntologyValue) -> String {
    match value {
        OntologyValue::Entity(entity) if entity.id == SELF_ENTITY_ID => "you".to_string(),
        other => other.to_string(),
    }
}

/// Trims text values (recursively in lists).
fn normalise(value: &mut RawValue) {
    match value {
        RawValue::Text(text) => *text = text.trim().to_string(),
        RawValue::List(items) => items.iter_mut().for_each(normalise),
        _ => {}
    }
}

pub(crate) fn snippet(text: &str) -> String {
    crate::harness::truncate_chars(text, MAX_SNIPPET_CHARS)
}

/// Length of `text` in characters: what memory audit events record of a memory's words
/// (no hash: a short memory could be guessed from one).
pub(crate) fn text_chars(text: &str) -> usize {
    text.chars().count()
}

/// Ids of every memory version a `memory_forget` event in `log` names.
fn forgotten_ids(log: &AuditLog) -> Result<Vec<String>, crate::audit::AuditError> {
    let mut ids = BTreeSet::new();
    let mut offset = 0;
    loop {
        let rows = log.query(&AuditQuery {
            types: vec![AuditEventType::MemoryForget],
            limit: Some(1_000),
            offset,
            ..Default::default()
        })?;
        if rows.is_empty() {
            break;
        }
        offset += u32::try_from(rows.len()).unwrap_or(u32::MAX);
        for row in &rows {
            let named = row.payload.get("ids").and_then(JsonValue::as_array);
            ids.extend(
                named
                    .into_iter()
                    .flatten()
                    .chain(row.payload.get("id"))
                    .filter_map(JsonValue::as_str)
                    .map(str::to_string),
            );
        }
    }
    Ok(ids.into_iter().collect())
}

/// Whether `stored` is a memory (written by or for the user), not a statement extracted
/// from a document.
fn is_memory(stored: &StoredStatement) -> bool {
    let source = stored
        .statement
        .provenance
        .as_ref()
        .map(|p| p.source.as_str())
        .unwrap_or("");
    memory_source_prefixes()
        .iter()
        .any(|p| source.starts_with(p.as_str()))
}

/// The run (turn) a memory came from, for one learned or approved in a conversation.
fn turn_of(stored: &StoredStatement) -> Option<String> {
    let source = &stored.statement.provenance.as_ref()?.source;
    let (_, run) = source
        .strip_prefix(CONVERSATION_SOURCE_PREFIX)?
        .split_once("/turn/")?;
    (!run.is_empty()).then(|| run.to_string())
}

/// Runs a short SQLite operation off the async executor.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, StatementError> + Send + 'static,
) -> MemoryResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| MemoryError::Statement(StatementError::Task(e.to_string())))?
        .map_err(MemoryError::from)
}
