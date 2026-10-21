//! Statement dynamics and Hebbian links in `shodh.db` (introduced in schema version 2).
//!
//! Strength is stored at an anchor time and decayed lazily by the reader, so rows change
//! only when something happens to a statement (written, used, pinned), never on read.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};

use super::dynamics::{DynamicsState, LinkState, LINK_PRUNE_FLOOR, MAX_LINKS_PER_STATEMENT};
use super::StatementResult;
use crate::audit::{open_shared_connection, AuditKey};

/// Rows per `IN (...)` lookup.
const LOOKUP_CHUNK: usize = 400;

/// What happened to a memory in one answer (a row of `run_memories`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMemoryKind {
    /// It was put in front of the answer's question.
    Injected,
    /// The answer used it, and it was reinforced.
    Used,
    /// The user marked it not relevant to the answer.
    Irrelevant,
    /// It was put in front of the question as a core memory (whatever the question
    /// asked), besides being recorded as [`RunMemoryKind::Injected`].
    Core,
}

impl RunMemoryKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Injected => "injected",
            Self::Used => "used",
            Self::Irrelevant => "irrelevant",
            Self::Core => "core",
        }
    }
}

/// Dynamics and links of statements, in the shared `shodh.db`.
///
/// Transactions that read before they write are `IMMEDIATE`: the audit writer commits to
/// the same database, and a deferred transaction upgraded after its commit would fail with
/// `SQLITE_BUSY_SNAPSHOT`, which the busy timeout does not retry.
pub struct DynamicsStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for DynamicsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicsStore").finish_non_exhaustive()
    }
}

fn format_ts(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_ts(text: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}

impl DynamicsStore {
    /// Opens `shodh.db` at `path` with the audit database key (if the database is
    /// encrypted), applying any pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> StatementResult<Self> {
        let conn = open_shared_connection(path, key)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stored state of one statement, if any.
    pub fn get(&self, id: &str) -> StatementResult<Option<DynamicsState>> {
        let conn = self.lock();
        conn.query_row(
            "SELECT strength, anchor_at, importance, use_count, last_used_at, pinned
             FROM statement_dynamics WHERE statement_id = ?1",
            params![id],
            read_state,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Stored states of several statements. Statements without a row are absent.
    pub fn get_many(&self, ids: &[String]) -> StatementResult<HashMap<String, DynamicsState>> {
        let conn = self.lock();
        let mut out = HashMap::new();
        for chunk in ids.chunks(LOOKUP_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let mut stmt = conn.prepare(&format!(
                "SELECT statement_id, strength, anchor_at, importance, use_count, last_used_at, pinned
                 FROM statement_dynamics WHERE statement_id IN ({placeholders})"
            ))?;
            let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
                let id: String = r.get(0)?;
                let state = DynamicsState {
                    strength: r.get(1)?,
                    anchor_at: parse_ts(&r.get::<_, String>(2)?)?,
                    importance: r.get(3)?,
                    use_count: r.get(4)?,
                    last_used_at: r
                        .get::<_, Option<String>>(5)?
                        .map(|t| parse_ts(&t))
                        .transpose()?,
                    pinned: r.get::<_, i64>(6)? != 0,
                };
                Ok((id, state))
            })?;
            for row in rows {
                let (id, state) = row?;
                out.insert(id, state);
            }
        }
        Ok(out)
    }

    /// Inserts or replaces the state of one statement.
    pub fn put(
        &self,
        id: &str,
        scope: &str,
        class: &str,
        state: &DynamicsState,
        now: DateTime<Utc>,
    ) -> StatementResult<()> {
        let conn = self.lock();
        write_state(&conn, id, scope, class, state, now)?;
        Ok(())
    }

    /// Inserts or replaces several states in one transaction.
    pub fn put_many(
        &self,
        rows: &[(String, String, String, DynamicsState)],
        now: DateTime<Utc>,
    ) -> StatementResult<()> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (id, scope, class, state) in rows {
            write_state(&tx, id, scope, class, state, now)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Records `scope` (its stored form) as the scope of the statements `ids`, in one
    /// transaction. Statements without dynamics are skipped.
    pub fn set_scope(&self, ids: &[String], scope: &str) -> StatementResult<()> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for chunk in ids.chunks(LOOKUP_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let mut values: Vec<&str> = Vec::with_capacity(chunk.len() + 1);
            values.push(scope);
            values.extend(chunk.iter().map(String::as_str));
            tx.execute(
                &format!(
                    "UPDATE statement_dynamics SET scope = ? WHERE statement_id IN ({placeholders})"
                ),
                params_from_iter(values),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Links touching any of `ids`, as `(from, to, state)` with `from < to`.
    pub fn links_touching(
        &self,
        ids: &[String],
    ) -> StatementResult<Vec<(String, String, LinkState)>> {
        let conn = self.lock();
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for chunk in ids.chunks(LOOKUP_CHUNK / 2) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let mut stmt = conn.prepare(&format!(
                "SELECT from_id, to_id, weight, co_activations, updated_at FROM statement_links
                 WHERE from_id IN ({placeholders}) OR to_id IN ({placeholders})"
            ))?;
            let args = chunk.iter().chain(chunk.iter());
            let rows = stmt.query_map(params_from_iter(args), read_link)?;
            for row in rows {
                let (from, to, link) = row?;
                if seen.insert((from.clone(), to.clone())) {
                    out.push((from, to, link));
                }
            }
        }
        Ok(out)
    }

    /// Strengthens the link of every pair (Hebbian co-activation), then prunes links that
    /// faded below [`LINK_PRUNE_FLOOR`] and keeps at most [`MAX_LINKS_PER_STATEMENT`] per
    /// statement involved. One transaction.
    pub fn strengthen_links(
        &self,
        pairs: &[(String, String)],
        importance: &HashMap<String, f64>,
        now: DateTime<Utc>,
    ) -> StatementResult<()> {
        if pairs.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut touched = BTreeSet::new();
        for (from, to) in pairs {
            if from == to {
                continue;
            }
            let (from, to) = super::dynamics::ordered_pair(from, to);
            let existing = tx
                .query_row(
                    "SELECT from_id, to_id, weight, co_activations, updated_at
                     FROM statement_links WHERE from_id = ?1 AND to_id = ?2",
                    params![from, to],
                    read_link,
                )
                .optional()?
                .map(|(_, _, link)| link);
            // The pair's boost is scaled by the more important of the two memories.
            let scale = importance
                .get(&from)
                .copied()
                .unwrap_or(0.0)
                .max(importance.get(&to).copied().unwrap_or(0.0));
            let link = LinkState::strengthen(existing.as_ref(), scale, now);
            tx.execute(
                "INSERT INTO statement_links(from_id, to_id, weight, co_activations, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(from_id, to_id) DO UPDATE SET
                    weight = excluded.weight,
                    co_activations = excluded.co_activations,
                    updated_at = excluded.updated_at",
                params![from, to, link.weight, link.co_activations, format_ts(now)],
            )?;
            touched.insert(from);
            touched.insert(to);
        }
        for id in &touched {
            let mut stmt = tx.prepare(
                "SELECT from_id, to_id, weight, co_activations, updated_at FROM statement_links
                 WHERE from_id = ?1 OR to_id = ?1",
            )?;
            let mut links: Vec<(String, String, f64)> = stmt
                .query_map(params![id], read_link)?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .map(|(from, to, link)| {
                    let weight = link.weight_at(now);
                    (from, to, weight)
                })
                .collect();
            drop(stmt);
            links.sort_by(|a, b| b.2.total_cmp(&a.2));
            for (i, (from, to, weight)) in links.iter().enumerate() {
                if i >= MAX_LINKS_PER_STATEMENT || *weight < LINK_PRUNE_FLOOR {
                    tx.execute(
                        "DELETE FROM statement_links WHERE from_id = ?1 AND to_id = ?2",
                        params![from, to],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The link between `a` and `b`, if any.
    pub fn link(&self, a: &str, b: &str) -> StatementResult<Option<LinkState>> {
        let (from, to) = super::dynamics::ordered_pair(a, b);
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT from_id, to_id, weight, co_activations, updated_at
                 FROM statement_links WHERE from_id = ?1 AND to_id = ?2",
                params![from, to],
                read_link,
            )
            .optional()?
            .map(|(_, _, link)| link))
    }

    /// Sets the link between `a` and `b` exactly (`None` removes it). Used to undo a
    /// learned link change; ordinary use goes through [`DynamicsStore::strengthen_links`].
    pub fn set_link(&self, a: &str, b: &str, link: Option<&LinkState>) -> StatementResult<()> {
        if a == b {
            return Ok(());
        }
        let (from, to) = super::dynamics::ordered_pair(a, b);
        let conn = self.lock();
        match link {
            None => {
                conn.execute(
                    "DELETE FROM statement_links WHERE from_id = ?1 AND to_id = ?2",
                    params![from, to],
                )?;
            }
            Some(link) => {
                conn.execute(
                    "INSERT INTO statement_links(from_id, to_id, weight, co_activations, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(from_id, to_id) DO UPDATE SET
                        weight = excluded.weight,
                        co_activations = excluded.co_activations,
                        updated_at = excluded.updated_at",
                    params![
                        from,
                        to,
                        link.weight.clamp(0.0, 1.0),
                        link.co_activations,
                        format_ts(link.updated_at)
                    ],
                )?;
            }
        }
        Ok(())
    }

    /// Applies the user's "Not relevant" on memory `id` in run `run_id` once, in one
    /// transaction: records the feedback (`run_memories`), lowers the memory's
    /// importance ([`DynamicsState::mark_irrelevant`]; `fallback` is its state when it has
    /// no row yet) and weakens its links to `co_injected`, the memories that went into the
    /// same answer ([`LinkState::weaken`]; links that fall below [`LINK_PRUNE_FLOOR`] are
    /// removed, missing links are not created). Returns `None` when this feedback was
    /// recorded before (nothing changes), else the state written.
    #[allow(clippy::too_many_arguments)] // One transaction over the row it writes.
    pub fn mark_irrelevant(
        &self,
        run_id: &str,
        id: &str,
        scope: &str,
        class: &str,
        fallback: &DynamicsState,
        co_injected: &[String],
        now: DateTime<Utc>,
    ) -> StatementResult<Option<DynamicsState>> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted = tx.execute(
            "INSERT INTO run_memories(run_id, statement_id, kind, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(run_id, statement_id, kind) DO NOTHING",
            params![
                run_id,
                id,
                RunMemoryKind::Irrelevant.as_str(),
                format_ts(now)
            ],
        )?;
        if inserted == 0 {
            tx.commit()?;
            return Ok(None);
        }
        let mut state = tx
            .query_row(
                "SELECT strength, anchor_at, importance, use_count, last_used_at, pinned
                 FROM statement_dynamics WHERE statement_id = ?1",
                params![id],
                read_state,
            )
            .optional()?
            .unwrap_or_else(|| fallback.clone());
        state.mark_irrelevant();
        write_state(&tx, id, scope, class, &state, now)?;
        let others: BTreeSet<&String> = co_injected.iter().filter(|o| o.as_str() != id).collect();
        for other in others {
            let (from, to) = super::dynamics::ordered_pair(id, other.as_str());
            let existing = tx
                .query_row(
                    "SELECT from_id, to_id, weight, co_activations, updated_at
                     FROM statement_links WHERE from_id = ?1 AND to_id = ?2",
                    params![from, to],
                    read_link,
                )
                .optional()?;
            let Some((_, _, link)) = existing else {
                continue;
            };
            let weakened = link.weaken(now);
            if weakened.weight < LINK_PRUNE_FLOOR {
                tx.execute(
                    "DELETE FROM statement_links WHERE from_id = ?1 AND to_id = ?2",
                    params![from, to],
                )?;
            } else {
                tx.execute(
                    "UPDATE statement_links SET weight = ?3, updated_at = ?4
                     WHERE from_id = ?1 AND to_id = ?2",
                    params![from, to, weakened.weight, format_ts(now)],
                )?;
            }
        }
        tx.commit()?;
        Ok(Some(state))
    }

    /// Records that `ids` had `kind` in run `run_id` (already recorded ones are kept).
    pub fn record_run_memories(
        &self,
        run_id: &str,
        kind: RunMemoryKind,
        ids: &[String],
        now: DateTime<Utc>,
    ) -> StatementResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for id in ids {
            tx.execute(
                "INSERT INTO run_memories(run_id, statement_id, kind, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(run_id, statement_id, kind) DO NOTHING",
                params![run_id, id, kind.as_str(), format_ts(now)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The memories recorded with `kind` in any of the runs `run_ids`.
    pub fn run_memories(
        &self,
        run_ids: &[String],
        kind: RunMemoryKind,
    ) -> StatementResult<BTreeSet<String>> {
        let conn = self.lock();
        let mut out = BTreeSet::new();
        for chunk in run_ids.chunks(LOOKUP_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let mut stmt = conn.prepare(&format!(
                "SELECT statement_id FROM run_memories
                 WHERE kind = ? AND run_id IN ({placeholders})"
            ))?;
            let args = std::iter::once(kind.as_str()).chain(chunk.iter().map(String::as_str));
            let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
            for row in rows {
                out.insert(row?);
            }
        }
        Ok(out)
    }

    /// Removes every link of a statement (it was forgotten).
    pub fn remove_links(&self, id: &str) -> StatementResult<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "DELETE FROM statement_links WHERE from_id = ?1 OR to_id = ?1",
            params![id],
        )?)
    }

    /// A small stored value of the statement store (`statement_state`), if set.
    pub fn state(&self, key: &str) -> StatementResult<Option<String>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT value FROM statement_state WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Sets a small stored value of the statement store.
    pub fn set_state(&self, key: &str, value: &str) -> StatementResult<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO statement_state(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Removes a stored value of the statement store.
    pub fn clear_state(&self, key: &str) -> StatementResult<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM statement_state WHERE key = ?1", params![key])?;
        Ok(())
    }

    /// Deletes the dynamics (strength, use count, pin), every link and the feedback of
    /// statements `ids` (they were erased), in one transaction. Deleted rows are zeroed in the database
    /// file (`secure_delete`). Returns the dynamics rows deleted.
    pub fn remove_statements(&self, ids: &[String]) -> StatementResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut conn = self.lock();
        // The pragma answers with its new value, so it is read, not executed.
        conn.query_row("PRAGMA secure_delete = ON", [], |r| r.get::<_, i64>(0))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut removed = 0;
        for chunk in ids.chunks(LOOKUP_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            removed += tx.execute(
                &format!("DELETE FROM statement_dynamics WHERE statement_id IN ({placeholders})"),
                params_from_iter(chunk.iter()),
            )?;
            tx.execute(
                &format!(
                    "DELETE FROM statement_links
                     WHERE from_id IN ({placeholders}) OR to_id IN ({placeholders})"
                ),
                params_from_iter(chunk.iter().chain(chunk.iter())),
            )?;
            tx.execute(
                &format!("DELETE FROM run_memories WHERE statement_id IN ({placeholders})"),
                params_from_iter(chunk.iter()),
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }
}

fn write_state(
    conn: &Connection,
    id: &str,
    scope: &str,
    class: &str,
    state: &DynamicsState,
    now: DateTime<Utc>,
) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO statement_dynamics(statement_id, scope, class, strength, anchor_at,
            importance, use_count, last_used_at, pinned, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(statement_id) DO UPDATE SET
            scope = excluded.scope,
            class = excluded.class,
            strength = excluded.strength,
            anchor_at = excluded.anchor_at,
            importance = excluded.importance,
            use_count = excluded.use_count,
            last_used_at = excluded.last_used_at,
            pinned = excluded.pinned,
            updated_at = excluded.updated_at",
        params![
            id,
            scope,
            class,
            state.strength.clamp(0.0, 1.0),
            format_ts(state.anchor_at),
            state.importance.clamp(0.0, 1.0),
            state.use_count,
            state.last_used_at.map(format_ts),
            i64::from(state.pinned),
            format_ts(now),
        ],
    )
}

fn read_state(r: &rusqlite::Row<'_>) -> rusqlite::Result<DynamicsState> {
    Ok(DynamicsState {
        strength: r.get(0)?,
        anchor_at: parse_ts(&r.get::<_, String>(1)?)?,
        importance: r.get(2)?,
        use_count: r.get(3)?,
        last_used_at: r
            .get::<_, Option<String>>(4)?
            .map(|t| parse_ts(&t))
            .transpose()?,
        pinned: r.get::<_, i64>(5)? != 0,
    })
}

fn read_link(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, LinkState)> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        LinkState {
            weight: r.get(2)?,
            co_activations: r.get(3)?,
            updated_at: parse_ts(&r.get::<_, String>(4)?)?,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn store() -> (tempfile::TempDir, DynamicsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = DynamicsStore::open(&dir.path().join("shodh.db"), None).unwrap();
        (dir, store)
    }

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).unwrap()
    }

    #[test]
    fn states_round_trip() {
        let (_dir, store) = store();
        let mut state = DynamicsState::fresh(t0(), 0.4);
        state.use_count = 3;
        state.last_used_at = Some(t0() + Duration::hours(1));
        state.pinned = true;
        store.put("m1", "global", "Note", &state, t0()).unwrap();
        assert_eq!(store.get("m1").unwrap(), Some(state.clone()));
        assert_eq!(store.get("missing").unwrap(), None);
        let many = store
            .get_many(&["m1".to_string(), "missing".to_string()])
            .unwrap();
        assert_eq!(many.len(), 1);
        assert_eq!(many["m1"], state);
    }

    #[test]
    fn links_are_strengthened_bounded_and_pruned() {
        let (_dir, store) = store();
        let importance: HashMap<String, f64> =
            [("a".to_string(), 1.0), ("b".to_string(), 0.0)].into();
        for _ in 0..50 {
            store
                .strengthen_links(&[("b".into(), "a".into())], &importance, t0())
                .unwrap();
        }
        let links = store.links_touching(&["a".to_string()]).unwrap();
        assert_eq!(links.len(), 1);
        let (from, to, link) = &links[0];
        assert_eq!((from.as_str(), to.as_str()), ("a", "b"));
        assert_eq!(link.co_activations, 50);
        assert!(link.weight > 0.99 && link.weight <= 1.0);

        // A hub keeps only its strongest links.
        let hub_importance: HashMap<String, f64> = HashMap::new();
        for i in 0..(MAX_LINKS_PER_STATEMENT + 10) {
            let pairs = vec![("hub".to_string(), format!("n{i:03}"))];
            let repeats = if i < 5 { 3 } else { 1 };
            for _ in 0..repeats {
                store
                    .strengthen_links(&pairs, &hub_importance, t0())
                    .unwrap();
            }
        }
        let hub = store.links_touching(&["hub".to_string()]).unwrap();
        assert_eq!(hub.len(), MAX_LINKS_PER_STATEMENT);
        for i in 0..5 {
            let id = format!("n{i:03}");
            assert!(hub.iter().any(|(_, to, _)| *to == id), "{id} was pruned");
        }

        // Links that faded below the floor are dropped on the next rewrite.
        let much_later = t0() + Duration::days(3650);
        store
            .strengthen_links(
                &[("hub".into(), "fresh".into())],
                &hub_importance,
                much_later,
            )
            .unwrap();
        let hub = store.links_touching(&["hub".to_string()]).unwrap();
        assert_eq!(hub.len(), 1);
        assert_eq!(store.remove_links("hub").unwrap(), 1);
    }

    #[test]
    fn not_relevant_applies_once_per_run_and_weakens_only_existing_links() {
        let (_dir, store) = store();
        let state = DynamicsState::fresh(t0(), 0.3);
        store.put("m", "global", "Note", &state, t0()).unwrap();
        let importance: HashMap<String, f64> = HashMap::new();
        store
            .strengthen_links(&[("m".into(), "x".into())], &importance, t0())
            .unwrap();
        let before = store.link("m", "x").unwrap().unwrap();
        let co = ["x".to_string(), "y".to_string(), "m".to_string()];

        let written = store
            .mark_irrelevant("run-1", "m", "global", "Note", &state, &co, t0())
            .unwrap()
            .expect("first mark applies");
        assert!((written.importance - 0.25).abs() < 1e-12);
        assert_eq!(
            store.get("m").unwrap().unwrap().importance,
            written.importance
        );
        let after = store.link("m", "x").unwrap().unwrap();
        assert!(after.weight < before.weight);
        assert_eq!(after.co_activations, before.co_activations);
        // No link is created to a memory it was never linked with.
        assert!(store.link("m", "y").unwrap().is_none());

        // The same answer's mark again changes nothing.
        assert!(store
            .mark_irrelevant("run-1", "m", "global", "Note", &state, &co, t0())
            .unwrap()
            .is_none());
        assert_eq!(
            store.get("m").unwrap().unwrap().importance,
            written.importance
        );
        assert_eq!(store.link("m", "x").unwrap().unwrap(), after);
        let marked = store
            .run_memories(&["run-1".to_string()], RunMemoryKind::Irrelevant)
            .unwrap();
        assert_eq!(
            marked.into_iter().collect::<Vec<_>>(),
            vec!["m".to_string()]
        );
        assert!(store
            .run_memories(&["run-2".to_string()], RunMemoryKind::Irrelevant)
            .unwrap()
            .is_empty());

        // Another answer's mark applies; a memory without dynamics starts from `fallback`.
        assert!(store
            .mark_irrelevant("run-2", "m", "global", "Note", &state, &co, t0())
            .unwrap()
            .is_some());
        let fresh = store
            .mark_irrelevant("run-2", "n", "global", "Note", &state, &[], t0())
            .unwrap()
            .unwrap();
        assert!((fresh.importance - 0.25).abs() < 1e-12);

        // Forgetting erases the run rows with the dynamics.
        store
            .record_run_memories("run-1", RunMemoryKind::Injected, &["m".to_string()], t0())
            .unwrap();
        store.remove_statements(&["m".to_string()]).unwrap();
        for kind in [RunMemoryKind::Irrelevant, RunMemoryKind::Injected] {
            assert!(store
                .run_memories(&["run-1".to_string()], kind)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn run_memories_are_recorded_once_per_run_and_kind() {
        let (_dir, store) = store();
        let ids = ["a".to_string(), "b".to_string()];
        for _ in 0..2 {
            store
                .record_run_memories("r1", RunMemoryKind::Used, &ids, t0())
                .unwrap();
        }
        store
            .record_run_memories("r2", RunMemoryKind::Used, &["c".to_string()], t0())
            .unwrap();
        let used = store
            .run_memories(&["r1".to_string(), "r2".to_string()], RunMemoryKind::Used)
            .unwrap();
        assert_eq!(used.into_iter().collect::<Vec<_>>(), vec!["a", "b", "c"]);
        assert!(store
            .run_memories(&["r1".to_string()], RunMemoryKind::Injected)
            .unwrap()
            .is_empty());
    }
}
