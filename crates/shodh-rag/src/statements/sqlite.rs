//! Statement dynamics and Hebbian links in `shodh.db` (schema version 2).
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

    /// Removes every link of a statement (it was forgotten).
    pub fn remove_links(&self, id: &str) -> StatementResult<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "DELETE FROM statement_links WHERE from_id = ?1 OR to_id = ?1",
            params![id],
        )?)
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
}
