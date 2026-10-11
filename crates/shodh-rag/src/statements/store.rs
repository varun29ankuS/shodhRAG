//! [`StatementStore`]: validated writes with supersede semantics, reads, hybrid search and
//! history over the LanceDB table, with dynamics in SQLite.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use shodh_ontology::{
    Cardinality, Ontology, PropertyChange, RawValue, Statement, SupersedeDecision, ValidStatement,
};

use super::dynamics::{self, DynamicsState};
use super::identity::identity_tokens;
use super::lance::{from_micros, identity_like, micros, quote, Row, StatementTable};
use super::render::{render_terms, render_text};
use super::sqlite::DynamicsStore;
use super::{
    Clock, HistoryEntry, PutIntent, PutOutcome, Scope, StatementError, StatementHit,
    StatementQuery, StatementResult, StoredStatement,
};
use crate::embeddings::EmbeddingModel;

/// Name of the LanceDB table.
pub const STATEMENTS_TABLE: &str = "statements";

/// Reciprocal-rank-fusion constant (the document search uses the same value).
const RRF_K: f64 = 60.0;

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 5_000;
/// Upper bound on rows read when following one fact's history.
const HISTORY_SCAN_LIMIT: usize = 1_000;
/// Upper bound on supersede candidates examined per write.
const CANDIDATE_LIMIT: usize = 200;

/// Where the store gets its embedding model. The app's model can be installed after start,
/// so it is looked up per call.
#[async_trait::async_trait]
pub trait EmbedderSource: Send + Sync {
    /// The embedding model, or [`StatementError::EmbeddingUnavailable`].
    async fn embedder(&self) -> StatementResult<Arc<dyn EmbeddingModel>>;
}

/// The typed statement store.
pub struct StatementStore {
    ontology: Arc<Ontology>,
    table: StatementTable,
    dynamics: Arc<DynamicsStore>,
    embedder: Arc<dyn EmbedderSource>,
    clock: Arc<dyn Clock>,
    /// Serialises writes: supersede decisions read, then write; two writers must not
    /// interleave between the two.
    write_lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for StatementStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatementStore")
            .field("dimension", &self.table.dimension())
            .finish_non_exhaustive()
    }
}

/// Writes decided in a [`StatementStore::put_many`] call and not stored yet.
#[derive(Default)]
struct Pending {
    /// New rows; embedded and appended together.
    rows: Vec<Row>,
    /// Stored rows to close: `(id, valid_to, superseded_by)`.
    closes: Vec<(String, i64, Option<String>)>,
    /// Dynamics to write: `(id, scope, class, state)`.
    states: Vec<(String, String, String, DynamicsState)>,
    /// Ids of the new rows.
    inserted: HashSet<String>,
    /// Ids of every row a pending write creates or changes.
    touched: HashSet<String>,
    /// Scope, class, identity tokens and text of those rows: what a later statement's
    /// supersede candidates are matched on.
    keys: Vec<(String, String, Vec<String>, String)>,
}

impl Pending {
    fn insert(&mut self, row: Row, state: DynamicsState) {
        self.inserted.insert(row.id.clone());
        self.states
            .push((row.id.clone(), row.scope.clone(), row.class.clone(), state));
        self.touch(&row);
        self.rows.push(row);
    }

    fn close(&mut self, row: &Row, valid_to: i64, superseded_by: Option<&str>) {
        self.closes
            .push((row.id.clone(), valid_to, superseded_by.map(str::to_string)));
        self.touch(row);
    }

    fn reinforce(&mut self, row: &Row, state: DynamicsState) {
        self.states
            .push((row.id.clone(), row.scope.clone(), row.class.clone(), state));
        self.touch(row);
    }

    fn touch(&mut self, row: &Row) {
        self.touched.insert(row.id.clone());
        let tokens = row
            .identity
            .split('|')
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        self.keys.push((
            row.scope.clone(),
            row.class.clone(),
            tokens,
            row.text.clone(),
        ));
    }

    /// Whether deciding `valid` (in `scope`, superseding `target` if given) would read a
    /// row a pending write creates or changes, so the pending writes must be stored
    /// first. Mirrors what `put_superseding` reads (the target and its dynamics) and
    /// what `put_auto` reads (current rows in the scope and class family that share an
    /// identity token, or have the same class and text when there are no tokens).
    fn affects(
        &self,
        store: &StatementStore,
        valid: &ValidStatement,
        scope: &Scope,
        target: Option<&str>,
    ) -> bool {
        if let Some(target) = target {
            return self.touched.contains(target);
        }
        if self.keys.is_empty() {
            return false;
        }
        let scope = scope.as_key();
        let class = valid.class();
        let tokens = identity_tokens(&store.ontology, valid);
        let text = tokens
            .is_empty()
            .then(|| render_text(&store.ontology, valid));
        self.keys.iter().any(|(s, c, t, x)| {
            *s == scope
                && (c == class || store.related(c, class))
                && match &text {
                    None => t.iter().any(|token| tokens.contains(token)),
                    Some(text) => c == class && x == text,
                }
        })
    }
}

/// Whether `error` concerns one statement of a batch (the others are still written)
/// rather than the store.
fn concerns_one_statement(error: &StatementError) -> bool {
    matches!(
        error,
        StatementError::Invalid(_)
            | StatementError::NotFound(_)
            | StatementError::DuplicateId(_)
            | StatementError::InvalidSupersede { .. }
    )
}

/// How one candidate relates to the incoming statement.
enum Relation {
    Unchanged,
    Update,
    Historical { valid_to: DateTime<Utc> },
    Conflict(Vec<PropertyChange>),
}

impl StatementStore {
    /// Opens (creating if needed) the `statements` table in the LanceDB directory
    /// `lance_dir`, with vectors of `dimension` floats.
    pub async fn open(
        lance_dir: &Path,
        dimension: usize,
        ontology: Arc<Ontology>,
        dynamics: Arc<DynamicsStore>,
        embedder: Arc<dyn EmbedderSource>,
        clock: Arc<dyn Clock>,
    ) -> StatementResult<Self> {
        std::fs::create_dir_all(lance_dir).map_err(|e| StatementError::Lance(e.to_string()))?;
        let uri = lance_dir.to_string_lossy().to_string();
        let db = lancedb::connect(&uri).execute().await?;
        let table = StatementTable::open(&db, STATEMENTS_TABLE, dimension).await?;
        Ok(Self {
            ontology,
            table,
            dynamics,
            embedder,
            clock,
            write_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// The ontology statements are validated against.
    pub fn ontology(&self) -> &Ontology {
        &self.ontology
    }

    /// The statement table's version: every append or update makes a new one.
    #[cfg(test)]
    pub(crate) async fn table_version(&self) -> StatementResult<u64> {
        self.table.version().await
    }

    /// The dynamics store (strength, use, links).
    pub fn dynamics(&self) -> &Arc<DynamicsStore> {
        &self.dynamics
    }

    /// The store's clock.
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// Validates `statement` and stores it in `scope`, applying supersede semantics.
    /// See [`PutIntent`] and [`PutOutcome`].
    pub async fn put(
        &self,
        statement: Statement,
        scope: Scope,
        intent: PutIntent,
    ) -> StatementResult<PutOutcome> {
        let mut outcomes = self.put_many(vec![(statement, scope, intent)]).await?;
        outcomes
            .pop()
            .unwrap_or_else(|| Err(StatementError::Task("a write returned no outcome".into())))
    }

    /// [`put`](Self::put) for several statements, in order, with the same validation and
    /// supersede semantics: each statement is decided against the store as the earlier
    /// ones in `items` left it. The writes are stored together (one embedding call, one
    /// table append, one dynamics transaction) instead of one by one; a statement that
    /// must see an earlier one of the batch first stores what is pending. Closing a
    /// superseded row stays one table update per row.
    ///
    /// Returns each statement's outcome in order. A statement the ontology rejects, or
    /// whose id or supersede target is wrong, gets its error there and the others are
    /// still written. `Err` is a storage failure: writes of this call may then be
    /// partially stored.
    pub async fn put_many(
        &self,
        items: Vec<(Statement, Scope, PutIntent)>,
    ) -> StatementResult<Vec<StatementResult<PutOutcome>>> {
        let _guard = self.write_lock.lock().await;
        let now = self.clock.now();
        let mut pending = Pending::default();
        let mut outcomes = Vec::with_capacity(items.len());
        for (statement, scope, intent) in items {
            match self
                .put_one(statement, scope, intent, now, &mut pending)
                .await
            {
                Err(e) if !concerns_one_statement(&e) => return Err(e),
                outcome => outcomes.push(outcome),
            }
        }
        self.flush(&mut pending, now).await?;
        Ok(outcomes)
    }

    async fn put_one(
        &self,
        statement: Statement,
        scope: Scope,
        intent: PutIntent,
        now: DateTime<Utc>,
        pending: &mut Pending,
    ) -> StatementResult<PutOutcome> {
        let valid = self
            .ontology
            .validate(&statement)
            .map_err(StatementError::Invalid)?;
        let target = match &intent {
            PutIntent::Supersede { target } => Some(target.as_str()),
            PutIntent::Auto => None,
        };
        if pending.affects(self, &valid, &scope, target) {
            self.flush(pending, now).await?;
        }
        if pending.inserted.contains(&statement.id) || self.row(&statement.id).await?.is_some() {
            return Err(StatementError::DuplicateId(statement.id.clone()));
        }
        match intent {
            PutIntent::Supersede { target } => {
                self.put_superseding(statement, valid, scope, &target, now, pending)
                    .await
            }
            PutIntent::Auto => self.put_auto(statement, valid, scope, now, pending).await,
        }
    }

    /// Stores the pending writes: one embedding call and one append for the new rows,
    /// the closes of the rows they supersede, then all dynamics in one transaction.
    async fn flush(&self, pending: &mut Pending, now: DateTime<Utc>) -> StatementResult<()> {
        let Pending {
            rows,
            closes,
            states,
            ..
        } = std::mem::take(pending);
        if !rows.is_empty() {
            let embedder = self.embedder.embedder().await?;
            if embedder.dimension() != self.table.dimension() {
                return Err(StatementError::DimensionMismatch {
                    expected: self.table.dimension(),
                    found: embedder.dimension(),
                });
            }
            let texts: Vec<String> = rows.iter().map(|r| r.text.clone()).collect();
            let vectors = tokio::task::spawn_blocking(move || {
                let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
                embedder.embed_documents(&texts)
            })
            .await
            .map_err(|e| StatementError::Task(e.to_string()))?
            .map_err(|e| StatementError::Embedding(format!("{e:#}")))?;
            self.table.insert_many(&rows, &vectors).await?;
        }
        for (id, valid_to, superseded_by) in &closes {
            self.table
                .close(id, *valid_to, superseded_by.as_deref())
                .await?;
        }
        if !states.is_empty() {
            self.dynamics.put_many(&states, now)?;
        }
        Ok(())
    }

    async fn put_superseding(
        &self,
        statement: Statement,
        valid: ValidStatement,
        scope: Scope,
        target: &str,
        now: DateTime<Utc>,
        pending: &mut Pending,
    ) -> StatementResult<PutOutcome> {
        let invalid = |reason: &str| StatementError::InvalidSupersede {
            target: target.to_string(),
            reason: reason.to_string(),
        };
        let old = self
            .row(target)
            .await?
            .filter(|r| r.forgotten_at.is_none())
            .ok_or_else(|| StatementError::NotFound(target.to_string()))?;
        if old.valid_to.is_some() {
            return Err(invalid("it was already superseded"));
        }
        if old.scope != scope.as_key() {
            return Err(invalid("it belongs to a different scope"));
        }
        if !self.related(&old.class, valid.class()) {
            return Err(invalid(&format!(
                "`{}` and `{}` are unrelated classes",
                old.class,
                valid.class()
            )));
        }
        let old_from = from_micros(old.valid_from).unwrap_or(now);
        let valid_to = valid
            .effective_from()
            .max(old_from + Duration::microseconds(1));
        let carried = self.dynamics.get(&old.id)?;
        let id = self.stage(
            &statement,
            &valid,
            &scope,
            None,
            carried.as_ref(),
            now,
            pending,
        )?;
        pending.close(&old, micros(valid_to), Some(&id));
        Ok(PutOutcome::Updated {
            id,
            superseded: vec![old.id],
        })
    }

    async fn put_auto(
        &self,
        statement: Statement,
        valid: ValidStatement,
        scope: Scope,
        now: DateTime<Utc>,
        pending: &mut Pending,
    ) -> StatementResult<PutOutcome> {
        let tokens = identity_tokens(&self.ontology, &valid);
        let text = render_text(&self.ontology, &valid);
        let current = current_predicate(now);
        let family = self.family(valid.class());
        let base = format!(
            "scope = {} AND class IN ({}) AND {current}",
            quote(&scope.as_key()),
            family
                .iter()
                .map(|c| quote(c))
                .collect::<Vec<_>>()
                .join(", ")
        );

        if tokens.is_empty() {
            // No identity to compare (a note): the same text in the same class is the
            // same fact.
            let predicate = format!(
                "{base} AND class = {} AND text = {}",
                quote(valid.class()),
                quote(&text)
            );
            if let Some(existing) = self
                .table
                .scan(Some(&predicate), 1)
                .await?
                .into_iter()
                .next()
            {
                self.reinforce(&existing, now, pending)?;
                return Ok(PutOutcome::Unchanged {
                    existing: existing.id,
                });
            }
            let id = self.stage(&statement, &valid, &scope, None, None, now, pending)?;
            return Ok(PutOutcome::Added { id });
        }

        let identity = tokens
            .iter()
            .map(|t| identity_like(t))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut candidates = self
            .table
            .scan(Some(&format!("{base} AND ({identity})")), CANDIDATE_LIMIT)
            .await?;
        candidates.sort_by(|a, b| b.valid_from.cmp(&a.valid_from).then(a.id.cmp(&b.id)));

        let mut unchanged: Option<Row> = None;
        let mut updates: Vec<(Row, Statement)> = Vec::new();
        let mut historical: Option<(Row, DateTime<Utc>)> = None;
        for row in candidates {
            let Some((existing, existing_valid)) = self.revalidate(&row) else {
                continue;
            };
            let decision = self.ontology.supersedes(&existing_valid, &valid);
            match classify(&decision) {
                None => {}
                Some(Relation::Conflict(conflicts)) => {
                    return Ok(PutOutcome::Conflict {
                        existing: row.id,
                        conflicts,
                    });
                }
                Some(Relation::Unchanged) => {
                    unchanged.get_or_insert(row);
                }
                Some(Relation::Update) => updates.push((row, existing)),
                Some(Relation::Historical { valid_to })
                    if historical.as_ref().is_none_or(|(_, t)| valid_to < *t) =>
                {
                    historical = Some((row, valid_to));
                }
                Some(Relation::Historical { .. }) => {}
            }
        }

        if !updates.is_empty() {
            let mut merged = statement.clone();
            for (_, existing) in &updates {
                merged = self.merge(existing, &merged);
            }
            let merged_valid = self
                .ontology
                .validate(&merged)
                .map_err(StatementError::Invalid)?;
            // Carry pin and use history over from the newest superseded version.
            let carried = self.dynamics.get(&updates[0].0.id)?;
            let id = self.stage(
                &merged,
                &merged_valid,
                &scope,
                None,
                carried.as_ref(),
                now,
                pending,
            )?;
            let closed_at = valid.effective_from();
            let mut superseded = Vec::new();
            for (row, _) in updates {
                let from = from_micros(row.valid_from).unwrap_or(closed_at);
                let end = closed_at.max(from + Duration::microseconds(1));
                pending.close(&row, micros(end), Some(&id));
                superseded.push(row.id);
            }
            return Ok(PutOutcome::Updated { id, superseded });
        }
        if let Some((current_row, valid_to)) = historical {
            let id = self.stage(
                &statement,
                &valid,
                &scope,
                Some((valid_to, current_row.id.clone())),
                None,
                now,
                pending,
            )?;
            return Ok(PutOutcome::Historical {
                id,
                current: current_row.id,
            });
        }
        if let Some(existing) = unchanged {
            self.reinforce(&existing, now, pending)?;
            return Ok(PutOutcome::Unchanged {
                existing: existing.id,
            });
        }
        let id = self.stage(&statement, &valid, &scope, None, None, now, pending)?;
        Ok(PutOutcome::Added { id })
    }

    /// The existing statement's values overlaid with the incoming ones: functional
    /// properties take the incoming value, many-valued properties accumulate. The result
    /// carries the incoming id, provenance and validity and the more specific class.
    fn merge(&self, existing: &Statement, incoming: &Statement) -> Statement {
        let mut merged = incoming.clone();
        if self
            .ontology
            .is_subclass_of(&existing.class, &incoming.class)
        {
            merged.class = existing.class.clone();
            merged.ontology_version = existing.ontology_version.clone();
        }
        if merged.subject.is_none() {
            merged.subject = existing.subject.clone();
        }
        let mut properties: BTreeMap<String, RawValue> = existing.properties.clone();
        for (name, value) in &incoming.properties {
            let many = self
                .ontology
                .property(name)
                .is_some_and(|p| p.cardinality == Cardinality::Many);
            let combined = match (many, properties.remove(name)) {
                (true, Some(old)) => {
                    let mut items = flatten(old);
                    for item in flatten(value.clone()) {
                        if !items.contains(&item) {
                            items.push(item);
                        }
                    }
                    RawValue::List(items)
                }
                _ => value.clone(),
            };
            properties.insert(name.clone(), combined);
        }
        merged.properties = properties;
        merged
    }

    /// Builds the row and dynamics of a new statement and adds them to `pending`.
    #[allow(clippy::too_many_arguments)]
    fn stage(
        &self,
        statement: &Statement,
        valid: &ValidStatement,
        scope: &Scope,
        closed: Option<(DateTime<Utc>, String)>,
        carried: Option<&DynamicsState>,
        now: DateTime<Utc>,
        pending: &mut Pending,
    ) -> StatementResult<String> {
        let text = render_text(&self.ontology, valid);
        let class = self.ontology.class(valid.class());
        let ontology_source = class.map(|c| c.source.clone()).unwrap_or_default();
        let expires_at = class.and_then(|c| dynamics::expires_at(&c.dynamics, valid));
        let tokens = identity_tokens(&self.ontology, valid);
        let provenance = valid.provenance();
        let row = Row {
            id: statement.id.clone(),
            class: valid.class().to_string(),
            subject: valid.subject().map(|s| s.id.clone()).unwrap_or_default(),
            identity: if tokens.is_empty() {
                String::new()
            } else {
                format!("|{}|", tokens.join("|"))
            },
            scope: scope.as_key(),
            text: text.clone(),
            terms: render_terms(&self.ontology, valid),
            statement_json: encode(&statement.id, statement)?,
            properties_json: encode(&statement.id, valid.properties())?,
            provenance_json: encode(&statement.id, provenance)?,
            extractor: provenance.extractor.kind.label().to_string(),
            source: provenance.source.clone(),
            ontology_source,
            ontology_version: statement.ontology_version.to_string(),
            valid_from: micros(valid.effective_from()),
            valid_to: closed.as_ref().map(|(at, _)| micros(*at)),
            superseded_by: closed.map(|(_, by)| by),
            expires_at: expires_at.map(micros),
            forgotten_at: None,
            created_at: micros(now),
        };
        let importance = dynamics::importance(&self.ontology, valid, &text);
        let mut state = DynamicsState::fresh(now, importance);
        if let Some(carried) = carried {
            state.pinned = carried.pinned;
            state.use_count = carried.use_count;
            state.last_used_at = carried.last_used_at;
        }
        let id = row.id.clone();
        pending.insert(row, state);
        Ok(id)
    }

    /// Re-assertion of a current fact: reinforce it under its class dynamics.
    fn reinforce(
        &self,
        row: &Row,
        now: DateTime<Utc>,
        pending: &mut Pending,
    ) -> StatementResult<()> {
        let Some(class) = self.ontology.class(&row.class) else {
            return Ok(());
        };
        let created = from_micros(row.created_at).unwrap_or(now);
        let mut state = self
            .dynamics
            .get(&row.id)?
            .unwrap_or_else(|| DynamicsState::fresh(created, dynamics::IMPORTANCE_FLOOR));
        state.reinforce(&class.dynamics, now);
        pending.reinforce(row, state);
        Ok(())
    }

    /// The class, its ancestors and its descendants: every class a statement of `class`
    /// can supersede or be superseded by.
    fn family(&self, class: &str) -> Vec<String> {
        let mut out: BTreeSet<String> = self
            .ontology
            .ancestors(class)
            .iter()
            .map(|c| c.id.clone())
            .collect();
        out.extend(
            self.ontology
                .descendants(class)
                .iter()
                .map(|c| c.id.clone()),
        );
        out.insert(class.to_string());
        out.into_iter().collect()
    }

    fn related(&self, a: &str, b: &str) -> bool {
        self.ontology.is_subclass_of(a, b) || self.ontology.is_subclass_of(b, a)
    }

    /// The stored statement re-validated under the current ontology. A row that no longer
    /// validates (the ontology changed) is skipped and logged, never fatal.
    fn revalidate(&self, row: &Row) -> Option<(Statement, ValidStatement)> {
        let statement: Statement = match serde_json::from_str(&row.statement_json) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(id = %row.id, error = %e, "stored statement is unreadable; skipped");
                return None;
            }
        };
        match self.ontology.validate(&statement) {
            Ok(valid) => Some((statement, valid)),
            Err(violations) => {
                tracing::warn!(
                    id = %row.id,
                    violations = violations.len(),
                    "stored statement no longer validates against the ontology; skipped"
                );
                None
            }
        }
    }

    async fn row(&self, id: &str) -> StatementResult<Option<Row>> {
        Ok(self
            .table
            .scan(Some(&format!("id = {}", quote(id))), 1)
            .await?
            .into_iter()
            .next())
    }

    /// One statement by id (forgotten statements are not found).
    pub async fn get(&self, id: &str) -> StatementResult<StoredStatement> {
        let row = self
            .row(id)
            .await?
            .filter(|r| r.forgotten_at.is_none())
            .ok_or_else(|| StatementError::NotFound(id.to_string()))?;
        stored(&row)
    }

    /// Several statements by id, in the given order; unknown and forgotten ids are skipped.
    pub async fn get_many(&self, ids: &[String]) -> StatementResult<Vec<StoredStatement>> {
        let mut found: HashMap<String, StoredStatement> = HashMap::new();
        for chunk in ids.chunks(100) {
            let list = chunk
                .iter()
                .map(|id| quote(id))
                .collect::<Vec<_>>()
                .join(", ");
            let predicate = format!("id IN ({list}) AND forgotten_at IS NULL");
            for row in self.table.scan(Some(&predicate), chunk.len()).await? {
                found.insert(row.id.clone(), stored(&row)?);
            }
        }
        Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
    }

    /// Statements matching `query`, newest first.
    pub async fn query(&self, query: &StatementQuery) -> StatementResult<Vec<StoredStatement>> {
        let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
        let predicate = self.predicate(query);
        // Property filters are applied after reading, so read more than the limit.
        let read = if query.properties.is_empty() {
            limit
        } else {
            MAX_LIMIT
        };
        let mut rows = self.table.scan(Some(&predicate), read).await?;
        rows.sort_by(|a, b| b.valid_from.cmp(&a.valid_from).then(a.id.cmp(&b.id)));
        let mut out = Vec::new();
        for row in rows {
            if !self.matches_properties(&row, query) {
                continue;
            }
            out.push(stored(&row)?);
            if out.len() == limit {
                break;
            }
        }
        Ok(out)
    }

    /// Hybrid search: vector (cosine) ranking over the text renderings and full-text (BM25)
    /// ranking over the values (`terms`), fused by reciprocal rank. Only statements matching
    /// `query` (current ones by default) are searched. Best first, at most `k`.
    pub async fn search(
        &self,
        text: &str,
        query: &StatementQuery,
        k: usize,
    ) -> StatementResult<Vec<StatementHit>> {
        if k == 0 || text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let embedder = self.embedder.embedder().await?;
        if embedder.dimension() != self.table.dimension() {
            return Err(StatementError::DimensionMismatch {
                expected: self.table.dimension(),
                found: embedder.dimension(),
            });
        }
        let to_embed = text.to_string();
        let vector = tokio::task::spawn_blocking(move || embedder.embed_query(&to_embed))
            .await
            .map_err(|e| StatementError::Task(e.to_string()))?
            .map_err(|e| StatementError::Embedding(format!("{e:#}")))?;
        let predicate = self.predicate(query);
        let candidates = (k * 3).max(20);
        let by_vector = self
            .table
            .vector_search(&vector, Some(&predicate), candidates)
            .await?;
        let by_text = self
            .table
            .text_search(text, Some(&predicate), candidates)
            .await?;

        let mut fused: HashMap<String, (Row, f64, Option<f64>, bool)> = HashMap::new();
        for (rank, (row, similarity)) in by_vector.into_iter().enumerate() {
            let score = 1.0 / (RRF_K + rank as f64 + 1.0);
            fused.insert(row.id.clone(), (row, score, Some(similarity), false));
        }
        for (rank, row) in by_text.into_iter().enumerate() {
            let score = 1.0 / (RRF_K + rank as f64 + 1.0);
            fused
                .entry(row.id.clone())
                .and_modify(|entry| {
                    entry.1 += score;
                    entry.3 = true;
                })
                .or_insert((row, score, None, true));
        }
        let mut hits = Vec::new();
        for (_, (row, rrf, similarity, lexical)) in fused {
            if !self.matches_properties(&row, query) {
                continue;
            }
            hits.push(StatementHit {
                stored: stored(&row)?,
                rrf,
                similarity,
                lexical,
            });
        }
        hits.sort_by(|a, b| {
            b.rrf
                .total_cmp(&a.rrf)
                .then_with(|| a.stored.id().cmp(b.stored.id()))
        });
        hits.truncate(k);
        Ok(hits)
    }

    /// Every version of the fact `id` belongs to (same scope, same subject or identity key,
    /// or linked by supersede), oldest first, projected onto `property` (all properties when
    /// `None`).
    pub async fn history(
        &self,
        id: &str,
        property: Option<&str>,
    ) -> StatementResult<Vec<HistoryEntry>> {
        let rows = self.history_rows(id).await?;
        let mut out = Vec::new();
        for row in rows {
            let valid = self.revalidate(&row).map(|(_, v)| v);
            let values = match (&valid, property) {
                (Some(valid), Some(property)) => valid
                    .values(property)
                    .iter()
                    .map(|v| format!("{property} = {v}"))
                    .collect(),
                (Some(valid), None) => valid
                    .properties()
                    .iter()
                    .flat_map(|(name, values)| values.iter().map(move |v| format!("{name} = {v}")))
                    .collect(),
                (None, _) => Vec::new(),
            };
            if property.is_some() && values.is_empty() {
                continue;
            }
            let stored = stored(&row)?;
            out.push(HistoryEntry {
                statement_id: row.id.clone(),
                values,
                valid_from: stored.valid_from,
                valid_to: stored.valid_to,
                superseded_by: stored.superseded_by.clone(),
                text: row.text.clone(),
            });
        }
        Ok(out)
    }

    /// Ids of every version of the fact `id` belongs to, oldest first.
    pub async fn history_ids(&self, id: &str) -> StatementResult<Vec<String>> {
        Ok(self
            .history_rows(id)
            .await?
            .into_iter()
            .map(|r| r.id)
            .collect())
    }

    async fn history_rows(&self, id: &str) -> StatementResult<Vec<Row>> {
        let start = self
            .row(id)
            .await?
            .filter(|r| r.forgotten_at.is_none())
            .ok_or_else(|| StatementError::NotFound(id.to_string()))?;
        let mut rows: BTreeMap<String, Row> = BTreeMap::new();
        let tokens: Vec<&str> = start
            .identity
            .split('|')
            .filter(|t| !t.is_empty())
            .collect();
        if !tokens.is_empty() {
            let family = self.family(&start.class);
            let predicate = format!(
                "scope = {} AND forgotten_at IS NULL AND class IN ({}) AND ({})",
                quote(&start.scope),
                family
                    .iter()
                    .map(|c| quote(c))
                    .collect::<Vec<_>>()
                    .join(", "),
                tokens
                    .iter()
                    .map(|t| identity_like(t))
                    .collect::<Vec<_>>()
                    .join(" OR ")
            );
            for row in self
                .table
                .scan(Some(&predicate), HISTORY_SCAN_LIMIT)
                .await?
            {
                rows.insert(row.id.clone(), row);
            }
        }
        // Follow explicit supersede links both ways (edits of facts without identity).
        let mut frontier = vec![start.clone()];
        rows.insert(start.id.clone(), start);
        while let Some(row) = frontier.pop() {
            if rows.len() >= HISTORY_SCAN_LIMIT {
                break;
            }
            let mut linked = self
                .table
                .scan(
                    Some(&format!(
                        "superseded_by = {} AND forgotten_at IS NULL",
                        quote(&row.id)
                    )),
                    HISTORY_SCAN_LIMIT,
                )
                .await?;
            if let Some(next) = &row.superseded_by {
                linked.extend(self.row(next).await?.filter(|r| r.forgotten_at.is_none()));
            }
            for other in linked {
                if !rows.contains_key(&other.id) {
                    rows.insert(other.id.clone(), other.clone());
                    frontier.push(other);
                }
            }
        }
        let mut rows: Vec<Row> = rows.into_values().collect();
        rows.sort_by(|a, b| {
            a.valid_from
                .cmp(&b.valid_from)
                .then(a.created_at.cmp(&b.created_at))
        });
        Ok(rows)
    }

    /// Soft-deletes one statement: it is no longer returned, searched or linked. The row is
    /// kept with `forgotten_at` set. Returns the statement as it was.
    pub async fn forget(&self, id: &str) -> StatementResult<StoredStatement> {
        let _guard = self.write_lock.lock().await;
        let row = self
            .row(id)
            .await?
            .filter(|r| r.forgotten_at.is_none())
            .ok_or_else(|| StatementError::NotFound(id.to_string()))?;
        let before = stored(&row)?;
        self.table.forget(id, micros(self.clock.now())).await?;
        self.dynamics.remove_links(id)?;
        Ok(before)
    }

    /// Undoes the write that stored `id`: soft-forgets it and reopens every statement it
    /// superseded, so the fact is as it was before. Refused when `id` has since been
    /// superseded by a newer version (an edit after the write): undoing it would silently
    /// drop that edit. Returns the reopened ids.
    pub async fn revert(&self, id: &str) -> StatementResult<Vec<String>> {
        let _guard = self.write_lock.lock().await;
        let row = self
            .row(id)
            .await?
            .filter(|r| r.forgotten_at.is_none())
            .ok_or_else(|| StatementError::NotFound(id.to_string()))?;
        if let Some(successor) = &row.superseded_by {
            // A historical write points at the older current statement; a later edit
            // points at a newer one.
            let later = self
                .row(successor)
                .await?
                .is_some_and(|s| s.forgotten_at.is_none() && s.created_at > row.created_at);
            if later {
                return Err(StatementError::InvalidSupersede {
                    target: id.to_string(),
                    reason: format!("it was changed again since (by `{successor}`)"),
                });
            }
        }
        let predicate = format!(
            "superseded_by = {} AND forgotten_at IS NULL",
            quote(&row.id)
        );
        let superseded = self.table.scan(Some(&predicate), CANDIDATE_LIMIT).await?;
        self.table.forget(&row.id, micros(self.clock.now())).await?;
        self.dynamics.remove_links(&row.id)?;
        let mut reopened = Vec::new();
        for old in superseded {
            self.table.reopen(&old.id).await?;
            reopened.push(old.id);
        }
        Ok(reopened)
    }

    /// Ends a current statement now, without a successor (an archived memory). It stays as
    /// history and [`StatementStore::unarchive`] reopens it.
    pub async fn archive(&self, id: &str) -> StatementResult<StoredStatement> {
        let _guard = self.write_lock.lock().await;
        let now = self.clock.now();
        let row = self.live_row(id).await?;
        if row.valid_to.is_some() {
            return Err(StatementError::InvalidSupersede {
                target: id.to_string(),
                reason: "it is no longer current".to_string(),
            });
        }
        let from = from_micros(row.valid_from).unwrap_or(now);
        let end = now.max(from + Duration::microseconds(1));
        self.table.close(&row.id, micros(end), None).await?;
        stored(&row)
    }

    /// Reopens a statement [`StatementStore::archive`] ended. Statements superseded by a
    /// newer version are not reopened (that is [`StatementStore::revert`] of the newer one).
    pub async fn unarchive(&self, id: &str) -> StatementResult<StoredStatement> {
        let _guard = self.write_lock.lock().await;
        let row = self.live_row(id).await?;
        if row.valid_to.is_none() || row.superseded_by.is_some() {
            return Err(StatementError::InvalidSupersede {
                target: id.to_string(),
                reason: "it is not archived".to_string(),
            });
        }
        self.table.reopen(&row.id).await?;
        stored(&row)
    }

    /// Resolves a contradiction between two current statements: `retire` is closed as
    /// superseded by `keep` (kept as history). The classes must be related.
    pub async fn retire(&self, retire: &str, keep: &str) -> StatementResult<()> {
        let _guard = self.write_lock.lock().await;
        let old = self.live_row(retire).await?;
        let new = self.live_row(keep).await?;
        let invalid = |reason: &str| StatementError::InvalidSupersede {
            target: retire.to_string(),
            reason: reason.to_string(),
        };
        if old.valid_to.is_some() || new.valid_to.is_some() {
            return Err(invalid("one of the two is no longer current"));
        }
        if retire == keep {
            return Err(invalid("a statement cannot supersede itself"));
        }
        if !self.related(&old.class, &new.class) {
            return Err(invalid("the classes are unrelated"));
        }
        let now = self.clock.now();
        let from = from_micros(old.valid_from).unwrap_or(now);
        let at = from_micros(new.valid_from)
            .unwrap_or(now)
            .max(from + Duration::microseconds(1));
        self.table.close(&old.id, micros(at), Some(&new.id)).await
    }

    /// Reopens `id` after [`StatementStore::retire`] closed it in favour of `keep`.
    pub async fn unretire(&self, id: &str, keep: &str) -> StatementResult<()> {
        let _guard = self.write_lock.lock().await;
        let row = self.live_row(id).await?;
        if row.superseded_by.as_deref() != Some(keep) {
            return Err(StatementError::InvalidSupersede {
                target: id.to_string(),
                reason: format!("it is not superseded by `{keep}`"),
            });
        }
        self.table.reopen(&row.id).await
    }

    async fn live_row(&self, id: &str) -> StatementResult<Row> {
        self.row(id)
            .await?
            .filter(|r| r.forgotten_at.is_none())
            .ok_or_else(|| StatementError::NotFound(id.to_string()))
    }

    fn predicate(&self, query: &StatementQuery) -> String {
        let mut parts = vec!["forgotten_at IS NULL".to_string()];
        if !query.include_history {
            parts.push(current_predicate(
                query.as_of.unwrap_or_else(|| self.clock.now()),
            ));
        }
        if !query.classes.is_empty() {
            let mut classes = BTreeSet::new();
            for class in &query.classes {
                classes.insert(class.clone());
                classes.extend(
                    self.ontology
                        .descendants(class)
                        .iter()
                        .map(|c| c.id.clone()),
                );
            }
            parts.push(format!(
                "class IN ({})",
                classes
                    .iter()
                    .map(|c| quote(c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(subject) = &query.subject {
            parts.push(format!("subject = {}", quote(subject)));
        }
        if !query.source_prefixes.is_empty() {
            let any = query
                .source_prefixes
                .iter()
                .map(|prefix| {
                    format!(
                        "source LIKE {}",
                        quote(&format!("{}%", like_escape(prefix)))
                    )
                })
                .collect::<Vec<_>>()
                .join(" OR ");
            parts.push(format!("({any})"));
        }
        if !query.scopes.is_empty() {
            parts.push(format!(
                "scope IN ({})",
                query
                    .scopes
                    .iter()
                    .map(|s| quote(&s.as_key()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        parts.join(" AND ")
    }

    fn matches_properties(&self, row: &Row, query: &StatementQuery) -> bool {
        if query.properties.is_empty() {
            return true;
        }
        let Some((_, valid)) = self.revalidate(row) else {
            return false;
        };
        query.properties.iter().all(|filter| {
            valid
                .values(&filter.property)
                .iter()
                .any(|v| v.to_string() == filter.equals)
        })
    }
}

/// Escapes `LIKE` wildcards (`%`, `_`) and the escape character (backslash, the default
/// escape of LanceDB's SQL filters, as in `LanceStore::delete_by_source_prefix`).
fn like_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Rows current at `at`: started, not superseded, not expired.
fn current_predicate(at: DateTime<Utc>) -> String {
    let t = micros(at);
    format!(
        "valid_from <= {t} AND (valid_to IS NULL OR valid_to > {t}) \
         AND (expires_at IS NULL OR expires_at > {t})"
    )
}

fn classify(decision: &SupersedeDecision) -> Option<Relation> {
    let SupersedeDecision::Merge { changes, .. } = decision else {
        return None;
    };
    let conflicts: Vec<PropertyChange> = decision.conflicts().into_iter().cloned().collect();
    if !conflicts.is_empty() {
        return Some(Relation::Conflict(conflicts));
    }
    let historical = changes.iter().find_map(|c| match c {
        PropertyChange::Historical { valid_to, .. } => Some(*valid_to),
        _ => None,
    });
    if let Some(valid_to) = historical {
        return Some(Relation::Historical { valid_to });
    }
    if changes.iter().any(|c| {
        matches!(
            c,
            PropertyChange::Superseded { .. } | PropertyChange::Added { .. }
        )
    }) {
        return Some(Relation::Update);
    }
    Some(Relation::Unchanged)
}

fn flatten(value: RawValue) -> Vec<RawValue> {
    match value {
        RawValue::List(items) => items,
        single => vec![single],
    }
}

fn stored(row: &Row) -> StatementResult<StoredStatement> {
    let corrupt = |reason: String| StatementError::Corrupt {
        id: row.id.clone(),
        reason,
    };
    let statement: Statement =
        serde_json::from_str(&row.statement_json).map_err(|e| corrupt(e.to_string()))?;
    let time =
        |micros: i64| from_micros(micros).ok_or_else(|| corrupt(format!("bad time {micros}")));
    let opt_time = |micros: Option<i64>| micros.map(time).transpose();
    Ok(StoredStatement {
        statement,
        text: row.text.clone(),
        scope: row.scope.parse().map_err(corrupt)?,
        ontology_source: row.ontology_source.clone(),
        valid_from: time(row.valid_from)?,
        valid_to: opt_time(row.valid_to)?,
        superseded_by: row.superseded_by.clone(),
        expires_at: opt_time(row.expires_at)?,
        forgotten_at: opt_time(row.forgotten_at)?,
        created_at: time(row.created_at)?,
    })
}

/// JSON text of a stored column.
fn encode<T: serde::Serialize + ?Sized>(id: &str, value: &T) -> StatementResult<String> {
    serde_json::to_string(value).map_err(|e| StatementError::Corrupt {
        id: id.to_string(),
        reason: format!("could not encode: {e}"),
    })
}
