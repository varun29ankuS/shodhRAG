//! SQLite storage of generated visuals in the shared `shodh.db`.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{SecondsFormat, Utc};
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};

use super::{
    clean_note, clean_title, content_hash, fts_query, validate_params, validate_source,
    CaptureReport, NewVersion, NewVisual, SkippedBlock, VisualAuthor, VisualDetail, VisualError,
    VisualKind, VisualOrigin, VisualPage, VisualQuery, VisualRecord, VisualResult, VisualSummary,
    VisualVersionInfo, DEFAULT_LIST_LIMIT, MAX_CAPTURE_BLOCKS, MAX_INSTRUCTION_CHARS,
    MAX_LIST_LIMIT,
};
use crate::audit::{open_shared_connection, AuditKey};

/// Key of the one-time backfill of conversations saved before the gallery existed.
const BACKFILL_KEY: &str = "backfill_v1";

/// Longest id accepted from callers.
const MAX_ID_CHARS: usize = 200;

const COLUMNS: &str = "id, root_id, parent_id, version, conversation_id, message_id, thread_id, \
    turn_id, kind, title, source, params_json, content_hash, pinned, note, instruction, \
    created_by, created_at, updated_at";

/// The visuals gallery, in `shodh.db`.
///
/// Read-then-write transactions are `IMMEDIATE`: the audit writer commits to the same
/// database, and a deferred transaction upgraded after its commit would fail with
/// `SQLITE_BUSY_SNAPSHOT`, which the busy timeout does not retry.
pub struct VisualStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for VisualStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VisualStore").finish_non_exhaustive()
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn check_id(id: &str) -> VisualResult<&str> {
    let id = id.trim();
    if id.is_empty() || id.chars().count() > MAX_ID_CHARS {
        return Err(VisualError::Invalid(format!(
            "an id must be 1 to {MAX_ID_CHARS} characters"
        )));
    }
    Ok(id)
}

fn opt_id(id: Option<&str>) -> VisualResult<Option<String>> {
    match id.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(id) => check_id(id).map(|s| Some(s.to_string())),
    }
}

fn corrupt(e: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        )),
    )
}

fn read_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<VisualRecord> {
    let kind: String = r.get(8)?;
    let created_by: String = r.get(16)?;
    let params: String = r.get(11)?;
    Ok(VisualRecord {
        id: r.get(0)?,
        root_id: r.get(1)?,
        parent_id: r.get(2)?,
        version: r.get(3)?,
        conversation_id: r.get(4)?,
        message_id: r.get(5)?,
        thread_id: r.get(6)?,
        turn_id: r.get(7)?,
        kind: VisualKind::parse(&kind).ok_or_else(|| corrupt(format!("unknown kind {kind}")))?,
        title: r.get(9)?,
        source: r.get(10)?,
        params: serde_json::from_str(&params).map_err(corrupt)?,
        content_hash: r.get(12)?,
        pinned: r.get::<_, i64>(13)? != 0,
        note: r.get(14)?,
        instruction: r.get(15)?,
        created_by: VisualAuthor::parse(&created_by)
            .ok_or_else(|| corrupt(format!("unknown author {created_by}")))?,
        created_at: r.get(17)?,
        updated_at: r.get(18)?,
    })
}

fn map_read_error(e: rusqlite::Error) -> VisualError {
    match e {
        rusqlite::Error::FromSqlConversionFailure(_, _, inner) => {
            VisualError::Corrupt(inner.to_string())
        }
        other => VisualError::Sqlite(other),
    }
}

/// The chain (`root_id`) of a version, and whether the chain is deleted.
fn chain_of(conn: &Connection, id: &str) -> VisualResult<(String, bool)> {
    conn.query_row(
        "SELECT root_id, deleted_at IS NOT NULL FROM generated_visuals WHERE id = ?1",
        params![id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
    )
    .optional()?
    .ok_or_else(|| VisualError::NotFound(id.to_string()))
}

fn record_by_id(conn: &Connection, id: &str) -> VisualResult<Option<VisualRecord>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM generated_visuals WHERE id = ?1"),
        params![id],
        read_record,
    )
    .optional()
    .map_err(map_read_error)
}

impl VisualStore {
    /// Opens `shodh.db` at `path` with the audit database key (if the database is
    /// encrypted), applying any pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> VisualResult<Self> {
        let conn = open_shared_connection(path, key)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records the visual blocks of one answer. Blocks this origin already has (same kind
    /// and normalised source) are not added again, also when they were deleted. Invalid
    /// blocks are skipped and reported; the rest are recorded in one transaction.
    pub fn capture(
        &self,
        origin: &VisualOrigin,
        blocks: &[NewVisual],
    ) -> VisualResult<CaptureReport> {
        if blocks.len() > MAX_CAPTURE_BLOCKS {
            return Err(VisualError::Invalid(format!(
                "{} visuals in one answer; at most {MAX_CAPTURE_BLOCKS} are recorded",
                blocks.len()
            )));
        }
        let conversation_id = check_id(&origin.conversation_id)?.to_string();
        let message_id = opt_id(origin.message_id.as_deref())?;
        let thread_id = opt_id(origin.thread_id.as_deref())?;
        let turn_id = opt_id(origin.turn_id.as_deref())?;
        let mut report = CaptureReport::default();
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let at = now();
        for (index, block) in blocks.iter().enumerate() {
            let prepared = validate_source(block.kind, &block.source)
                .and_then(|source| Ok((source, validate_params(block.params.as_ref())?)));
            let (source, params) = match prepared {
                Ok(v) => v,
                Err(e) => {
                    report.skipped.push(SkippedBlock {
                        index,
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            let hash = content_hash(block.kind, &source);
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM generated_visuals
                     WHERE parent_id IS NULL AND conversation_id = ?1
                       AND IFNULL(message_id, '') = IFNULL(?2, '')
                       AND IFNULL(thread_id, '') = IFNULL(?3, '')
                       AND IFNULL(turn_id, '') = IFNULL(?4, '')
                       AND content_hash = ?5",
                    params![conversation_id, message_id, thread_id, turn_id, hash],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                if !report.existing.contains(&id) {
                    report.existing.push(id);
                }
                continue;
            }
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO generated_visuals(id, root_id, parent_id, version, conversation_id,
                    message_id, thread_id, turn_id, kind, title, source, params_json, content_hash,
                    pinned, note, instruction, created_by, created_at, updated_at)
                 VALUES (?1, ?1, NULL, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, '', NULL,
                    'capture', ?11, ?11)",
                params![
                    id,
                    conversation_id,
                    message_id,
                    thread_id,
                    turn_id,
                    block.kind.as_str(),
                    clean_title(&block.title, block.kind),
                    source,
                    params,
                    hash,
                    at,
                ],
            )?;
            report.created.push(id);
        }
        tx.commit()?;
        Ok(report)
    }

    /// One page of visuals (one card per chain, showing its latest version), pinned first,
    /// then most recently changed. Deleted visuals are never listed.
    pub fn list(&self, query: &VisualQuery) -> VisualResult<VisualPage> {
        let limit = query
            .limit
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .clamp(1, MAX_LIST_LIMIT);
        let offset = query.offset.unwrap_or(0);
        let mut filters = vec!["v.deleted_at IS NULL".to_string()];
        let mut args: Vec<SqlValue> = Vec::new();
        if let Some(conversation) = query
            .conversation_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            args.push(SqlValue::Text(conversation.to_string()));
            filters.push(format!("v.conversation_id = ?{}", args.len()));
        }
        if let Some(kind) = query.kind {
            args.push(SqlValue::Text(kind.as_str().to_string()));
            filters.push(format!("v.kind = ?{}", args.len()));
        }
        if query.pinned_only {
            filters.push("v.pinned = 1".to_string());
        }
        if let Some(text) = query.text.as_deref() {
            if let Some(fts) = fts_query(text) {
                args.push(SqlValue::Text(fts));
                filters.push(format!(
                    "v.root_id IN (SELECT g.root_id FROM generated_visuals_fts f
                       JOIN generated_visuals g ON g.seq = f.rowid
                       WHERE generated_visuals_fts MATCH ?{})",
                    args.len()
                ));
            }
        }
        let latest = "v.version = (SELECT MAX(w.version) FROM generated_visuals w \
                      WHERE w.root_id = v.root_id)";
        filters.push(latest.to_string());
        let filter = filters.join(" AND ");

        let conn = self.lock();
        let total: i64 = conn.query_row(
            &format!("SELECT count(*) FROM generated_visuals v WHERE {filter}"),
            params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        let mut page_args = args.clone();
        page_args.push(SqlValue::Integer(i64::from(limit)));
        page_args.push(SqlValue::Integer(i64::from(offset)));
        let columns = COLUMNS
            .split(", ")
            .map(|c| format!("v.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {columns},
                (SELECT r.created_at FROM generated_visuals r WHERE r.id = v.root_id)
             FROM generated_visuals v WHERE {filter}
             ORDER BY v.pinned DESC, v.updated_at DESC, v.seq DESC
             LIMIT ?{} OFFSET ?{}",
            page_args.len() - 1,
            page_args.len()
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(page_args.iter()), |r| {
                let latest = read_record(r)?;
                let first: Option<String> = r.get(19)?;
                Ok(VisualSummary {
                    version_count: latest.version,
                    first_created_at: first.unwrap_or_else(|| latest.created_at.clone()),
                    latest,
                })
            })
            .map_err(map_read_error)?;
        let items = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_read_error)?;
        Ok(VisualPage {
            items,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Visuals of one conversation that are not deleted (one per chain).
    pub fn count_for_conversation(&self, conversation_id: &str) -> VisualResult<u64> {
        let conn = self.lock();
        let n: i64 = conn.query_row(
            "SELECT count(DISTINCT root_id) FROM generated_visuals
             WHERE conversation_id = ?1 AND deleted_at IS NULL",
            params![conversation_id],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// One version (by its id) with the versions of its chain. Deleted visuals are not
    /// found.
    pub fn get(&self, id: &str) -> VisualResult<VisualDetail> {
        let id = check_id(id)?;
        let conn = self.lock();
        let (root, deleted) = chain_of(&conn, id)?;
        if deleted {
            return Err(VisualError::Deleted(id.to_string()));
        }
        let visual = record_by_id(&conn, id)?.ok_or_else(|| VisualError::NotFound(id.into()))?;
        let mut stmt = conn.prepare(
            "SELECT id, version, created_by, instruction, created_at FROM generated_visuals
             WHERE root_id = ?1 ORDER BY version",
        )?;
        let versions = stmt
            .query_map(params![root], |r| {
                let by: String = r.get(2)?;
                Ok(VisualVersionInfo {
                    id: r.get(0)?,
                    version: r.get(1)?,
                    created_by: VisualAuthor::parse(&by)
                        .ok_or_else(|| corrupt(format!("unknown author {by}")))?,
                    instruction: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })
            .map_err(map_read_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_read_error)?;
        Ok(VisualDetail { visual, versions })
    }

    /// The latest version of the chain `id` belongs to.
    pub fn latest(&self, id: &str) -> VisualResult<VisualDetail> {
        let id = check_id(id)?;
        let latest_id: String = {
            let conn = self.lock();
            let (root, _) = chain_of(&conn, id)?;
            conn.query_row(
                "SELECT id FROM generated_visuals WHERE root_id = ?1 ORDER BY version DESC LIMIT 1",
                params![root],
                |r| r.get(0),
            )?
        };
        self.get(&latest_id)
    }

    /// The version `version` of the chain `id` belongs to.
    pub fn version(&self, id: &str, version: u32) -> VisualResult<VisualDetail> {
        let id = check_id(id)?;
        let version_id: String = {
            let conn = self.lock();
            let (root, _) = chain_of(&conn, id)?;
            conn.query_row(
                "SELECT id FROM generated_visuals WHERE root_id = ?1 AND version = ?2",
                params![root, version],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| VisualError::NotFound(format!("{id} version {version}")))?
        };
        self.get(&version_id)
    }

    /// Adds a refinement of version `base_id` as the chain's next version. Earlier versions
    /// are never changed. The new version has the kind, origin, title, pin and note of the
    /// chain.
    pub fn add_version(&self, base_id: &str, new: &NewVersion) -> VisualResult<VisualDetail> {
        let base_id = check_id(base_id)?;
        let instruction = new
            .instruction
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.chars().count() > MAX_INSTRUCTION_CHARS {
                    let mut out: String = s.chars().take(MAX_INSTRUCTION_CHARS - 1).collect();
                    out.push('…');
                    out
                } else {
                    s.to_string()
                }
            });
        let params = validate_params(new.params.as_ref())?;
        let id = {
            let mut conn = self.lock();
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let base = record_by_id(&tx, base_id)?
                .ok_or_else(|| VisualError::NotFound(base_id.to_string()))?;
            let (_, deleted) = chain_of(&tx, base_id)?;
            if deleted {
                return Err(VisualError::Deleted(base_id.to_string()));
            }
            let source = validate_source(base.kind, &new.source)?;
            let hash = content_hash(base.kind, &source);
            let next: u32 = tx.query_row(
                "SELECT MAX(version) + 1 FROM generated_visuals WHERE root_id = ?1",
                params![base.root_id],
                |r| r.get(0),
            )?;
            let id = uuid::Uuid::new_v4().to_string();
            let at = now();
            tx.execute(
                "INSERT INTO generated_visuals(id, root_id, parent_id, version, conversation_id,
                    message_id, thread_id, turn_id, kind, title, source, params_json, content_hash,
                    pinned, note, instruction, created_by, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                    ?17, ?18, ?18)",
                params![
                    id,
                    base.root_id,
                    base.id,
                    next,
                    base.conversation_id,
                    base.message_id,
                    base.thread_id,
                    base.turn_id,
                    base.kind.as_str(),
                    base.title,
                    source,
                    params,
                    hash,
                    base.pinned,
                    base.note,
                    instruction,
                    new.author.as_str(),
                    at,
                ],
            )?;
            // The chain changed: it moves up in "recently changed".
            tx.execute(
                "UPDATE generated_visuals SET updated_at = ?2 WHERE root_id = ?1",
                params![base.root_id, at],
            )?;
            tx.commit()?;
            id
        };
        self.get(&id)
    }

    /// Applies `set` (an `UPDATE ... SET` fragment with `?2` as its value) to every version
    /// of the chain `id` belongs to. Deleted chains are not changed.
    fn update_chain(&self, id: &str, set: &str, value: SqlValue) -> VisualResult<VisualDetail> {
        let id = check_id(id)?;
        {
            let mut conn = self.lock();
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (root, deleted) = chain_of(&tx, id)?;
            if deleted {
                return Err(VisualError::Deleted(id.to_string()));
            }
            tx.execute(
                &format!("UPDATE generated_visuals SET {set}, updated_at = ?3 WHERE root_id = ?1"),
                params![root, value, now()],
            )?;
            tx.commit()?;
        }
        self.get(id)
    }

    /// Renames the visual (every version).
    pub fn rename(&self, id: &str, title: &str) -> VisualResult<VisualDetail> {
        if title.trim().is_empty() {
            return Err(VisualError::Invalid("a title cannot be empty".to_string()));
        }
        // The kind only matters for an empty title, rejected above.
        let title = clean_title(title, VisualKind::Mermaid);
        self.update_chain(id, "title = ?2", SqlValue::Text(title))
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> VisualResult<VisualDetail> {
        self.update_chain(id, "pinned = ?2", SqlValue::Integer(i64::from(pinned)))
    }

    pub fn set_note(&self, id: &str, note: &str) -> VisualResult<VisualDetail> {
        let note = clean_note(note)?;
        self.update_chain(id, "note = ?2", SqlValue::Text(note))
    }

    /// Soft-deletes the visual (every version). Returns the chain's root id. The rows stay
    /// (so the same answer does not capture it again) until restored.
    pub fn delete(&self, id: &str) -> VisualResult<String> {
        self.set_deleted(id, true)
    }

    /// Undoes [`Self::delete`].
    pub fn restore(&self, id: &str) -> VisualResult<String> {
        self.set_deleted(id, false)
    }

    fn set_deleted(&self, id: &str, deleted: bool) -> VisualResult<String> {
        let id = check_id(id)?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (root, _) = chain_of(&tx, id)?;
        let at = now();
        if deleted {
            tx.execute(
                "UPDATE generated_visuals SET deleted_at = ?2 WHERE root_id = ?1 AND deleted_at IS NULL",
                params![root, at],
            )?;
        } else {
            tx.execute(
                "UPDATE generated_visuals SET deleted_at = NULL, updated_at = ?2 WHERE root_id = ?1",
                params![root, at],
            )?;
        }
        tx.commit()?;
        Ok(root)
    }

    /// Whether the one-time backfill of earlier conversations has run.
    pub fn backfill_done(&self) -> VisualResult<bool> {
        let conn = self.lock();
        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM visual_settings WHERE key = ?1",
                params![BACKFILL_KEY],
                |r| r.get(0),
            )
            .optional()?;
        Ok(value.is_some())
    }

    /// Records that the backfill ran.
    pub fn mark_backfill_done(&self) -> VisualResult<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO visual_settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![BACKFILL_KEY, now()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditLog;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, VisualStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = VisualStore::open(&dir.path().join("shodh.db"), None).unwrap();
        (dir, store)
    }

    fn origin(message: &str) -> VisualOrigin {
        VisualOrigin {
            conversation_id: "c1".to_string(),
            message_id: Some(message.to_string()),
            thread_id: None,
            turn_id: None,
        }
    }

    fn block(kind: VisualKind, title: &str, source: &str) -> NewVisual {
        NewVisual {
            kind,
            title: title.to_string(),
            source: source.to_string(),
            params: None,
        }
    }

    const PLOT: &str = r#"{"title":"Pendulum","params":[{"name":"L","min":0.1,"max":2}]}"#;

    #[test]
    fn migration_creates_the_tables_and_the_audit_log_still_opens() {
        let (dir, store) = store();
        drop(store);
        // The audit log opens the same file afterwards and applies nothing twice.
        let log = AuditLog::open(dir.path().join("shodh.db"), None).unwrap();
        assert!(log.verify().unwrap().ok);
        let conn = Connection::open(dir.path().join("shodh.db")).unwrap();
        let names: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE name IN
                 ('generated_visuals', 'generated_visuals_fts', 'visual_settings') ORDER BY name",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            names,
            [
                "generated_visuals",
                "generated_visuals_fts",
                "visual_settings"
            ]
        );
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3);
    }

    #[test]
    fn capture_records_blocks_and_deduplicates_per_origin() {
        let (_dir, store) = store();
        let blocks = vec![
            block(VisualKind::Mermaid, "Flow", "graph TD\nA-->B"),
            block(VisualKind::Plot, "", PLOT),
            // Same as the first apart from whitespace: one visual.
            block(VisualKind::Mermaid, "Flow again", "graph TD  \r\nA-->B\n"),
            block(VisualKind::Chart, "Broken", "not json"),
        ];
        let first = store.capture(&origin("m1"), &blocks).unwrap();
        assert_eq!(first.created.len(), 2);
        assert_eq!(first.existing.len(), 1);
        assert_eq!(first.existing[0], first.created[0]);
        assert_eq!(first.skipped.len(), 1);
        assert_eq!(first.skipped[0].index, 3);

        // Capturing the same answer again (a re-save or the backfill) adds nothing.
        let again = store.capture(&origin("m1"), &blocks).unwrap();
        assert!(again.created.is_empty());
        assert_eq!(again.existing.len(), 2);

        // The same block in another answer is another visual.
        let other = store.capture(&origin("m2"), &blocks[..1]).unwrap();
        assert_eq!(other.created.len(), 1);

        let page = store.list(&VisualQuery::default()).unwrap();
        assert_eq!(page.total, 3);
        let plot = page
            .items
            .iter()
            .find(|v| v.latest.kind == VisualKind::Plot)
            .unwrap();
        assert_eq!(plot.latest.title, "Plot");
        assert_eq!(plot.latest.params, json!({}));
        assert_eq!(plot.version_count, 1);
    }

    #[test]
    fn side_answers_are_their_own_origin() {
        let (_dir, store) = store();
        let side = VisualOrigin {
            conversation_id: "c1".to_string(),
            message_id: None,
            thread_id: Some("t1".to_string()),
            turn_id: Some("turn-1".to_string()),
        };
        let b = [block(VisualKind::Equation, "", "E = mc^2")];
        assert_eq!(store.capture(&side, &b).unwrap().created.len(), 1);
        assert_eq!(store.capture(&side, &b).unwrap().created.len(), 0);
        assert_eq!(store.capture(&origin("m1"), &b).unwrap().created.len(), 1);
        assert_eq!(store.count_for_conversation("c1").unwrap(), 2);
    }

    #[test]
    fn deleted_visuals_stay_deleted_when_captured_again_and_can_be_restored() {
        let (_dir, store) = store();
        let b = [block(VisualKind::Equation, "Energy", "E = mc^2")];
        let id = store.capture(&origin("m1"), &b).unwrap().created[0].clone();
        store.delete(&id).unwrap();
        assert!(matches!(store.get(&id), Err(VisualError::Deleted(_))));
        let again = store.capture(&origin("m1"), &b).unwrap();
        assert!(again.created.is_empty());
        assert_eq!(store.list(&VisualQuery::default()).unwrap().total, 0);
        store.restore(&id).unwrap();
        assert_eq!(store.get(&id).unwrap().visual.title, "Energy");
        assert!(matches!(
            store.get("missing"),
            Err(VisualError::NotFound(_))
        ));
    }

    #[test]
    fn versions_form_a_chain_and_never_overwrite() {
        let (_dir, store) = store();
        let v1 = store
            .capture(&origin("m1"), &[block(VisualKind::Plot, "Pendulum", PLOT)])
            .unwrap()
            .created[0]
            .clone();
        store.set_pinned(&v1, true).unwrap();
        store.set_note(&v1, "for the lecture").unwrap();
        let longer = PLOT.replace("\"max\":2", "\"max\":5");
        let v2 = store
            .add_version(
                &v1,
                &NewVersion {
                    source: longer.clone(),
                    params: Some(json!({ "values": [{ "name": "L", "value": 3.0 }] })),
                    instruction: Some("make the pendulum longer".to_string()),
                    author: VisualAuthor::User,
                },
            )
            .unwrap();
        assert_eq!(v2.visual.version, 2);
        assert_eq!(v2.visual.parent_id.as_deref(), Some(v1.as_str()));
        assert_eq!(v2.visual.root_id, v1);
        assert!(v2.visual.pinned);
        assert_eq!(v2.visual.note, "for the lecture");
        assert_eq!(v2.visual.kind, VisualKind::Plot);
        assert_eq!(v2.visual.created_by, VisualAuthor::User);
        assert_eq!(v2.versions.len(), 2);

        // Refining v1 again branches from v1 but is still the next version of the chain.
        let v3 = store
            .add_version(
                &v1,
                &NewVersion {
                    source: PLOT.replace("Pendulum", "Pendulum (labelled)"),
                    params: None,
                    instruction: None,
                    author: VisualAuthor::Agent,
                },
            )
            .unwrap();
        assert_eq!(v3.visual.version, 3);
        assert_eq!(v3.visual.parent_id.as_deref(), Some(v1.as_str()));

        // The original is untouched.
        let original = store.get(&v1).unwrap();
        assert_eq!(original.visual.source, PLOT);
        assert_eq!(
            original
                .versions
                .iter()
                .map(|v| v.version)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(store.latest(&v1).unwrap().visual.id, v3.visual.id);
        assert_eq!(
            store.version(&v3.visual.id, 2).unwrap().visual.id,
            v2.visual.id
        );

        // One card per chain, showing the latest version.
        let page = store.list(&VisualQuery::default()).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].latest.id, v3.visual.id);
        assert_eq!(page.items[0].version_count, 3);
        assert_eq!(page.items[0].first_created_at, original.visual.created_at);

        // A revision keeps the kind: a plot cannot become an invalid spec.
        let bad = store.add_version(
            &v1,
            &NewVersion {
                source: "<svg></svg>".to_string(),
                params: None,
                instruction: None,
                author: VisualAuthor::Agent,
            },
        );
        assert!(matches!(bad, Err(VisualError::Invalid(_))));

        // Renaming any version renames the chain.
        store.rename(&v2.visual.id, "Long pendulum").unwrap();
        assert_eq!(store.get(&v1).unwrap().visual.title, "Long pendulum");
        assert!(store.rename(&v1, "  ").is_err());

        // Deleting removes the whole chain; refining it is refused.
        store.delete(&v3.visual.id).unwrap();
        assert_eq!(store.list(&VisualQuery::default()).unwrap().total, 0);
        let refused = store.add_version(
            &v1,
            &NewVersion {
                source: PLOT.to_string(),
                params: None,
                instruction: None,
                author: VisualAuthor::User,
            },
        );
        assert!(matches!(refused, Err(VisualError::Deleted(_))));
    }

    #[test]
    fn search_filters_and_ordering() {
        let (_dir, store) = store();
        let o = origin("m1");
        let created = store
            .capture(
                &o,
                &[
                    block(VisualKind::Mermaid, "Login flow", "graph TD\nUser-->Server"),
                    block(
                        VisualKind::Equation,
                        "Kinetic energy",
                        "E_k = \\frac{1}{2} m v^2",
                    ),
                    block(
                        VisualKind::Table,
                        "Revenue",
                        "| Quarter | Revenue |\n|---|---|\n| Q1 | 10 |",
                    ),
                ],
            )
            .unwrap()
            .created;
        let other = VisualOrigin {
            conversation_id: "c2".to_string(),
            ..origin("m9")
        };
        store
            .capture(
                &other,
                &[block(VisualKind::Mermaid, "Deploy", "graph LR\nCI-->Prod")],
            )
            .unwrap();

        let find = |text: &str| -> Vec<String> {
            store
                .list(&VisualQuery {
                    text: Some(text.to_string()),
                    ..VisualQuery::default()
                })
                .unwrap()
                .items
                .into_iter()
                .map(|v| v.latest.title)
                .collect()
        };
        // Title, source and prefix matches.
        assert_eq!(find("login"), ["Login flow"]);
        assert_eq!(find("serv"), ["Login flow"]);
        assert_eq!(find("quarter revenue"), ["Revenue"]);
        // Operators and quotes are literal text, never FTS syntax errors.
        assert!(find("AND OR \"").is_empty());
        assert!(find("NEAR(").is_empty());
        // Notes are searchable, and the index follows edits.
        store.set_note(&created[1], "physics homework").unwrap();
        assert_eq!(find("homework"), ["Kinetic energy"]);
        store.set_note(&created[1], "").unwrap();
        assert!(find("homework").is_empty());
        store.rename(&created[0], "Sign-in flow").unwrap();
        assert!(find("login flow").is_empty());
        assert_eq!(find("sign"), ["Sign-in flow"]);

        let by_conversation = store
            .list(&VisualQuery {
                conversation_id: Some("c2".to_string()),
                ..VisualQuery::default()
            })
            .unwrap();
        assert_eq!(by_conversation.total, 1);
        let by_kind = store
            .list(&VisualQuery {
                kind: Some(VisualKind::Mermaid),
                ..VisualQuery::default()
            })
            .unwrap();
        assert_eq!(by_kind.total, 2);

        // Pinned first, then most recently changed.
        store.set_pinned(&created[2], true).unwrap();
        let all = store.list(&VisualQuery::default()).unwrap();
        assert_eq!(all.items[0].latest.title, "Revenue");
        let pinned = store
            .list(&VisualQuery {
                pinned_only: true,
                ..VisualQuery::default()
            })
            .unwrap();
        assert_eq!(pinned.total, 1);

        // Paging.
        let page = store
            .list(&VisualQuery {
                limit: Some(2),
                offset: Some(2),
                ..VisualQuery::default()
            })
            .unwrap();
        assert_eq!(page.total, 4);
        assert_eq!(page.items.len(), 2);
    }

    #[test]
    fn backfill_flag_and_capture_limits() {
        let (_dir, store) = store();
        assert!(!store.backfill_done().unwrap());
        store.mark_backfill_done().unwrap();
        store.mark_backfill_done().unwrap();
        assert!(store.backfill_done().unwrap());
        let many: Vec<NewVisual> = (0..=MAX_CAPTURE_BLOCKS)
            .map(|i| block(VisualKind::Equation, "", &format!("x_{i}")))
            .collect();
        assert!(matches!(
            store.capture(&origin("m1"), &many),
            Err(VisualError::Invalid(_))
        ));
        let blank = VisualOrigin {
            conversation_id: " ".to_string(),
            ..origin("m1")
        };
        assert!(store.capture(&blank, &many[..1]).is_err());
    }
}
