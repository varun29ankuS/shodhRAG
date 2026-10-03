//! Long-term memory of the user: typed statements that evolve.
//!
//! Every memory is a validated ontology statement in the [`StatementStore`] (so it has a
//! class, properties, provenance and a validity interval), written only by the user
//! (Settings) or with the user's approval (agent tools) — see [`guard`]. Memories:
//! - **strengthen with use**: a recall that is used reinforces each memory under its class
//!   dynamics and strengthens Hebbian links between memories recalled together;
//! - **fade when unused**: strength decays lazily (exponential, then a power-law tail) at
//!   read time; pinned memories do not decay;
//! - **update when facts change**: a new value of a temporal property supersedes the old
//!   one, which is kept as history; deadlines expire facts (class expiry rules).
//!
//! Recall ranks hybrid-search candidates by relevance, current strength, importance and
//! spreading activation over the links (see [`recall`]).
//!
//! The legacy `crate::memory::MemorySystem` (JSON conversation log) is unrelated and not
//! used here.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod guard;
mod recall;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use shodh_ontology::{
    EntityRef, Extractor, Provenance, RawValue, Statement, Value as OntologyValue,
};

use crate::audit::{AuditEventType, AuditLog, AuditRecord, LOCAL_OWNER, MAX_SNIPPET_CHARS};
use crate::statements::{
    DynamicsState, PutIntent, PutOutcome, Scope, StatementError, StatementQuery, StatementStore,
    StoredStatement, SELF_ENTITY_ID,
};

pub use guard::{
    check_write_origin, conversation_source, Origin, WriteAuthority, CONVERSATION_SOURCE_PREFIX,
    SETTINGS_SOURCE,
};
pub use recall::{
    render_injection, RecallMode, RecallRequest, RecalledMemory, DEFAULT_MIN_SIMILARITY,
    MAX_INJECTION_CHARS, MEMORY_BLOCK_TITLE,
};

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
    app_version: String,
}

impl std::fmt::Debug for MemoryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryService").finish_non_exhaustive()
    }
}

const DEFAULT_LIST_LIMIT: usize = 500;

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
            app_version: app_version.to_string(),
        }
    }

    /// The statement store.
    pub fn store(&self) -> &Arc<StatementStore> {
        &self.store
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

    /// Forgets memory `id` and every earlier or later version of the same fact (soft
    /// delete; links removed). Returns the forgotten ids. Audited as `memory_forget`.
    pub async fn forget(
        &self,
        id: &str,
        visible_from: Option<&Scope>,
        actor: &Actor,
    ) -> MemoryResult<Vec<String>> {
        let target = self.memory_statement(id).await?;
        self.check_visible(&target, visible_from)?;
        let ids = self.store.history_ids(id).await?;
        let mut forgotten = Vec::new();
        for version in &ids {
            match self.store.forget(version).await {
                Ok(_) => forgotten.push(version.clone()),
                Err(StatementError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.audit(
            actor,
            AuditEventType::MemoryForget,
            json!({
                "action": "forget",
                "id": id,
                "ids": forgotten,
                "class": target.statement.class,
                "scope": target.scope.as_key(),
                "text": snippet(&target.text),
                "via": actor.via,
            }),
        );
        Ok(forgotten)
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
                "text": snippet(&target.text),
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
        if request.mode == RecallMode::Use && !recalled.is_empty() {
            recall::record_use(self, &recalled).await?;
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
        }
        Ok(recalled)
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
                    "text": memory.as_ref().map(|m| snippet(&m.text)),
                    "source": origin.source,
                    "extractor": origin.extractor.label(),
                    "approval": match &origin.authority {
                        WriteAuthority::UserApproval { step_id } => Some(step_id.clone()),
                        WriteAuthority::UserInterface => None,
                    },
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
        let now = self.store.now();
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

    /// A memory statement (not a document-derived statement) by id.
    async fn memory_statement(&self, id: &str) -> MemoryResult<StoredStatement> {
        let stored = self.store.get(id).await?;
        let source = stored
            .statement
            .provenance
            .as_ref()
            .map(|p| p.source.as_str())
            .unwrap_or("");
        if memory_source_prefixes()
            .iter()
            .any(|p| source.starts_with(p.as_str()))
        {
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

/// Runs a short SQLite operation off the async executor.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, StatementError> + Send + 'static,
) -> MemoryResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| MemoryError::Statement(StatementError::Task(e.to_string())))?
        .map_err(MemoryError::from)
}
