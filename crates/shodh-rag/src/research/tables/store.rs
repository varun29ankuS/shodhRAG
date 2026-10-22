//! The evidence tables of `shodh.db` (schema 14): tables, columns, rows, cells and the
//! fill cache. One connection, like [`crate::research::ResearchDb`]; every write that
//! touches several tables runs in one immediate transaction.
//!
//! The fill cache is not owned by a table: two tables asking the same question of the
//! same paper with the same model share an entry. A cache entry is removed when the last
//! cell that uses it goes (its table, column or row deleted, its column's key changed, or
//! its paper removed from the library), so nothing of a paper's text outlives the cells
//! that showed it.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{SecondsFormat, Utc};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

use super::validate::parse_edit;
use super::{
    collapse, Cell, CellStatus, CellValue, Column, ColumnKind, ColumnSpec, EvidenceTable, TableRow,
    MAX_NAME_CHARS,
};
use crate::audit::{open_shared_connection, AuditKey};
use crate::research::{path_key, ResearchError, ResearchResult};

/// A model's raw answer for one column of one paper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub paper_hash: String,
    pub column_key: String,
    pub model: String,
    pub response_json: String,
}

/// The evidence tables of `shodh.db`.
pub struct TableStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for TableStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TableStore").finish_non_exhaustive()
    }
}

fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn bad_column(index: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, Type::Text, message.into())
}

fn clean_name(name: &str) -> ResearchResult<String> {
    let name = collapse(name);
    if name.is_empty() {
        return Err(ResearchError::Invalid("A table needs a name.".to_string()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ResearchError::Invalid(format!(
            "A table name is at most {MAX_NAME_CHARS} characters."
        )));
    }
    Ok(name)
}

const TABLE_COLS: &str = "id, workspace_id, name, created_at, updated_at";

fn table_from(r: &Row<'_>) -> rusqlite::Result<EvidenceTable> {
    Ok(EvidenceTable {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        name: r.get(2)?,
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
    })
}

const COLUMN_COLS: &str = "id, table_id, position, question, kind, options_json, instructions, \
     column_key, revision, created_at, updated_at";

fn column_from(r: &Row<'_>) -> rusqlite::Result<Column> {
    let kind: String = r.get(4)?;
    let kind =
        ColumnKind::parse(&kind).ok_or_else(|| bad_column(4, format!("unknown kind {kind}")))?;
    let options_json: String = r.get(5)?;
    let options: Vec<String> =
        serde_json::from_str(&options_json).map_err(|e| bad_column(5, e.to_string()))?;
    Ok(Column {
        id: r.get(0)?,
        table_id: r.get(1)?,
        position: r.get(2)?,
        spec: ColumnSpec {
            question: r.get(3)?,
            kind,
            options,
            instructions: r.get(6)?,
        },
        key: r.get(7)?,
        revision: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

const ROW_COLS: &str = "id, table_id, position, document_id, added_at";

fn row_from(r: &Row<'_>) -> rusqlite::Result<TableRow> {
    Ok(TableRow {
        id: r.get(0)?,
        table_id: r.get(1)?,
        position: r.get(2)?,
        document_id: r.get(3)?,
        added_at: r.get(4)?,
    })
}

const CELL_COLS: &str = "table_id, row_id, column_id, status, value_json, quote, chunk_id, page, \
     bbox_json, model, column_revision, paper_hash, filled_at, edited_at, error";

fn cell_from(r: &Row<'_>) -> rusqlite::Result<Cell> {
    let status: String = r.get(3)?;
    let status = CellStatus::parse(&status)
        .ok_or_else(|| bad_column(3, format!("unknown status {status}")))?;
    let value_json: Option<String> = r.get(4)?;
    let value = value_json
        .map(|json| serde_json::from_str::<CellValue>(&json))
        .transpose()
        .map_err(|e| bad_column(4, e.to_string()))?;
    Ok(Cell {
        table_id: r.get(0)?,
        row_id: r.get(1)?,
        column_id: r.get(2)?,
        status,
        value,
        quote: r.get(5)?,
        chunk_id: r.get(6)?,
        page: r.get(7)?,
        bboxes: r.get(8)?,
        model: r.get(9)?,
        column_revision: r.get(10)?,
        paper_hash: r.get(11)?,
        filled_at: r.get(12)?,
        edited_at: r.get(13)?,
        error: r.get(14)?,
    })
}

fn value_json(value: &Option<CellValue>) -> ResearchResult<Option<String>> {
    value
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| ResearchError::Invalid(e.to_string()))
}

/// Removes every cache entry that no cell uses (a cell uses the entry of its paper hash
/// and its column's key). Called inside the transaction of every write that deletes or
/// replaces cells or changes a column's key, so no model answer, and the quote of the
/// user's paper in it, outlives the cells that showed it. An entry two tables share stays
/// until the last of their cells goes.
fn sweep_cache(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM fill_cache WHERE NOT EXISTS (
            SELECT 1 FROM table_cell c JOIN table_column k ON k.id = c.column_id
             WHERE c.paper_hash = fill_cache.paper_hash
               AND k.column_key = fill_cache.column_key)",
        [],
    )
}

/// Deletes one table with its columns, rows, cells, jobs and unshared cache entries.
fn delete_table_in(conn: &Connection, table_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM table_cell WHERE table_id = ?1",
        params![table_id],
    )?;
    conn.execute(
        "DELETE FROM table_row WHERE table_id = ?1",
        params![table_id],
    )?;
    conn.execute(
        "DELETE FROM table_column WHERE table_id = ?1",
        params![table_id],
    )?;
    conn.execute(
        "DELETE FROM table_fill_job WHERE table_id = ?1",
        params![table_id],
    )?;
    let removed = conn.execute(
        "DELETE FROM evidence_table WHERE id = ?1",
        params![table_id],
    )?;
    sweep_cache(conn)?;
    Ok(removed)
}

/// Deletes one row with its cells and the cache entries only it used.
fn delete_row_in(conn: &Connection, row_id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM table_cell WHERE row_id = ?1", params![row_id])?;
    let removed = conn.execute("DELETE FROM table_row WHERE id = ?1", params![row_id])?;
    sweep_cache(conn)?;
    Ok(removed)
}

/// Deletes every table of a workspace (called inside the workspace's own delete
/// transaction, so a workspace never goes without its tables or the other way round).
pub(crate) fn delete_workspace_tables(
    conn: &Connection,
    workspace_id: &str,
) -> rusqlite::Result<usize> {
    let ids: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM evidence_table WHERE workspace_id = ?1")?;
        let rows = stmt.query_map(params![workspace_id], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for id in &ids {
        delete_table_in(conn, id)?;
    }
    Ok(ids.len())
}

impl TableStore {
    /// Opens `shodh.db` at `path` with the audit database key (if it is encrypted),
    /// applying pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> ResearchResult<Self> {
        Ok(Self {
            conn: Mutex::new(open_shared_connection(path, key)?),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn create_table(&self, workspace_id: &str, name: &str) -> ResearchResult<EvidenceTable> {
        let name = clean_name(name)?;
        let now = now_text();
        let table = EvidenceTable {
            id: new_id(),
            workspace_id: workspace_id.to_string(),
            name,
            created_at: now.clone(),
            updated_at: now,
        };
        // One statement: the workspace is checked and the table inserted in the same
        // write, so a workspace deleted meanwhile never keeps a table.
        let inserted = self.lock().execute(
            "INSERT INTO evidence_table(id, workspace_id, name, created_at, updated_at)
             SELECT ?1, ?2, ?3, ?4, ?5
              WHERE EXISTS (SELECT 1 FROM workspaces WHERE id = ?2)",
            params![
                table.id,
                table.workspace_id,
                table.name,
                table.created_at,
                table.updated_at
            ],
        )?;
        if inserted == 0 {
            return Err(ResearchError::NotFound(format!(
                "No workspace {workspace_id}."
            )));
        }
        Ok(table)
    }

    pub fn tables(&self, workspace_id: &str) -> ResearchResult<Vec<EvidenceTable>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TABLE_COLS} FROM evidence_table WHERE workspace_id = ?1 ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map(params![workspace_id], table_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn table(&self, id: &str) -> ResearchResult<Option<EvidenceTable>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {TABLE_COLS} FROM evidence_table WHERE id = ?1"),
                params![id],
                table_from,
            )
            .optional()?)
    }

    pub fn rename_table(&self, id: &str, name: &str) -> ResearchResult<EvidenceTable> {
        let name = clean_name(name)?;
        let changed = self.lock().execute(
            "UPDATE evidence_table SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, name, now_text()],
        )?;
        if changed == 0 {
            return Err(ResearchError::NotFound(format!("No table {id}.")));
        }
        self.table(id)?
            .ok_or_else(|| ResearchError::NotFound(format!("No table {id}.")))
    }

    pub fn delete_table(&self, id: &str) -> ResearchResult<bool> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = delete_table_in(&tx, id)?;
        tx.commit()?;
        Ok(removed > 0)
    }

    pub fn add_column(&self, table_id: &str, spec: &ColumnSpec) -> ResearchResult<Column> {
        let spec = spec.cleaned()?;
        let options_json = serde_json::to_string(&spec.options)
            .map_err(|e| ResearchError::Invalid(e.to_string()))?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM evidence_table WHERE id = ?1)",
            params![table_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(ResearchError::NotFound(format!("No table {table_id}.")));
        }
        let position: i64 = tx.query_row(
            "SELECT IFNULL(MAX(position), -1) + 1 FROM table_column WHERE table_id = ?1",
            params![table_id],
            |r| r.get(0),
        )?;
        let now = now_text();
        let column = Column {
            id: new_id(),
            table_id: table_id.to_string(),
            position,
            key: spec.key(),
            spec,
            revision: 1,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        tx.execute(
            &format!("INSERT INTO table_column({COLUMN_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
            params![
                column.id,
                column.table_id,
                column.position,
                column.spec.question,
                column.spec.kind.as_str(),
                options_json,
                column.spec.instructions,
                column.key,
                column.revision,
                column.created_at,
                column.updated_at
            ],
        )?;
        tx.execute(
            "UPDATE evidence_table SET updated_at = ?2 WHERE id = ?1",
            params![table_id, now],
        )?;
        tx.commit()?;
        Ok(column)
    }

    pub fn column(&self, column_id: &str) -> ResearchResult<Option<Column>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {COLUMN_COLS} FROM table_column WHERE id = ?1"),
                params![column_id],
                column_from,
            )
            .optional()?)
    }

    pub fn columns(&self, table_id: &str) -> ResearchResult<Vec<Column>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMN_COLS} FROM table_column WHERE table_id = ?1 ORDER BY position, created_at"
        ))?;
        let rows = stmt.query_map(params![table_id], column_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Changes a column. A change of its key (question, kind, options or instructions,
    /// whitespace aside) raises its revision, so its cells are stale, and removes the
    /// cache entries only its cells used.
    pub fn update_column(&self, column_id: &str, spec: &ColumnSpec) -> ResearchResult<Column> {
        let spec = spec.cleaned()?;
        let key = spec.key();
        let options_json = serde_json::to_string(&spec.options)
            .map_err(|e| ResearchError::Invalid(e.to_string()))?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = tx
            .query_row(
                &format!("SELECT {COLUMN_COLS} FROM table_column WHERE id = ?1"),
                params![column_id],
                column_from,
            )
            .optional()?
            .ok_or_else(|| ResearchError::NotFound(format!("No column {column_id}.")))?;
        if current.key == key {
            return Ok(current);
        }
        let now = now_text();
        tx.execute(
            "UPDATE table_column SET question = ?2, kind = ?3, options_json = ?4,
               instructions = ?5, column_key = ?6, revision = revision + 1, updated_at = ?7
             WHERE id = ?1",
            params![
                column_id,
                spec.question,
                spec.kind.as_str(),
                options_json,
                spec.instructions,
                key,
                now
            ],
        )?;
        sweep_cache(&tx)?;
        let updated = tx.query_row(
            &format!("SELECT {COLUMN_COLS} FROM table_column WHERE id = ?1"),
            params![column_id],
            column_from,
        )?;
        tx.commit()?;
        Ok(updated)
    }

    pub fn delete_column(&self, column_id: &str) -> ResearchResult<bool> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM table_cell WHERE column_id = ?1",
            params![column_id],
        )?;
        let removed = tx.execute("DELETE FROM table_column WHERE id = ?1", params![column_id])?;
        sweep_cache(&tx)?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Adds papers (file paths, stored by their library identity) as rows; a paper the
    /// table already has is skipped. Returns the rows added.
    pub fn add_rows(&self, table_id: &str, documents: &[String]) -> ResearchResult<Vec<TableRow>> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM evidence_table WHERE id = ?1)",
            params![table_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(ResearchError::NotFound(format!("No table {table_id}.")));
        }
        let mut position: i64 = tx.query_row(
            "SELECT IFNULL(MAX(position), -1) + 1 FROM table_row WHERE table_id = ?1",
            params![table_id],
            |r| r.get(0),
        )?;
        let now = now_text();
        let mut added = Vec::new();
        for document in documents {
            let document_id = path_key(document);
            if document_id.is_empty() {
                continue;
            }
            let row = TableRow {
                id: new_id(),
                table_id: table_id.to_string(),
                position,
                document_id,
                added_at: now.clone(),
            };
            let inserted = tx.execute(
                &format!("INSERT OR IGNORE INTO table_row({ROW_COLS}) VALUES (?1, ?2, ?3, ?4, ?5)"),
                params![
                    row.id,
                    row.table_id,
                    row.position,
                    row.document_id,
                    row.added_at
                ],
            )?;
            if inserted > 0 {
                position += 1;
                added.push(row);
            }
        }
        tx.commit()?;
        Ok(added)
    }

    pub fn rows(&self, table_id: &str) -> ResearchResult<Vec<TableRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {ROW_COLS} FROM table_row WHERE table_id = ?1 ORDER BY position, added_at"
        ))?;
        let rows = stmt.query_map(params![table_id], row_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn remove_row(&self, row_id: &str) -> ResearchResult<bool> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = delete_row_in(&tx, row_id)?;
        tx.commit()?;
        Ok(removed > 0)
    }

    pub fn cells(&self, table_id: &str) -> ResearchResult<Vec<Cell>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {CELL_COLS} FROM table_cell WHERE table_id = ?1"
        ))?;
        let rows = stmt.query_map(params![table_id], cell_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn cell(&self, row_id: &str, column_id: &str) -> ResearchResult<Option<Cell>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {CELL_COLS} FROM table_cell WHERE row_id = ?1 AND column_id = ?2"),
                params![row_id, column_id],
                cell_from,
            )
            .optional()?)
    }

    /// Stores a filled cell and, with it, its cache entry. Nothing is written (false) when
    /// the row or column is gone, the column changed since the fill started, or the user
    /// edited the cell meanwhile.
    pub fn store_fill(&self, cell: &Cell, cache: Option<&CacheEntry>) -> ResearchResult<bool> {
        let value = value_json(&cell.value)?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT k.revision FROM table_column k JOIN table_row r ON r.table_id = k.table_id
                 WHERE k.id = ?1 AND r.id = ?2 AND k.table_id = ?3",
                params![cell.column_id, cell.row_id, cell.table_id],
                |r| r.get(0),
            )
            .optional()?;
        if current.is_none() || current != cell.column_revision {
            return Ok(false);
        }
        let written = tx.execute(
            &format!(
                "INSERT INTO table_cell({CELL_COLS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, NULL, ?14)
                 ON CONFLICT(row_id, column_id) DO UPDATE SET
                   status = excluded.status, value_json = excluded.value_json,
                   quote = excluded.quote, chunk_id = excluded.chunk_id, page = excluded.page,
                   bbox_json = excluded.bbox_json, model = excluded.model,
                   column_revision = excluded.column_revision, paper_hash = excluded.paper_hash,
                   filled_at = excluded.filled_at, error = excluded.error
                 WHERE table_cell.status <> 'edited'"
            ),
            params![
                cell.table_id,
                cell.row_id,
                cell.column_id,
                cell.status.as_str(),
                value,
                cell.quote,
                cell.chunk_id,
                cell.page,
                cell.bboxes,
                cell.model,
                cell.column_revision,
                cell.paper_hash,
                cell.filled_at,
                cell.error
            ],
        )?;
        if written == 0 {
            return Ok(false);
        }
        if let Some(entry) = cache {
            tx.execute(
                "INSERT OR REPLACE INTO fill_cache(paper_hash, column_key, model, response_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![entry.paper_hash, entry.column_key, entry.model, entry.response_json, now_text()],
            )?;
        }
        // A refill with a new paper hash leaves the old hash's entries unused.
        sweep_cache(&tx)?;
        tx.commit()?;
        Ok(true)
    }

    /// The user's value for a cell, under its column's type rule. The model's quote and
    /// passage no longer back it and are cleared; the paper hash stays.
    pub fn edit_cell(&self, row_id: &str, column_id: &str, input: &str) -> ResearchResult<Cell> {
        let column = self
            .column(column_id)?
            .ok_or_else(|| ResearchError::NotFound(format!("No column {column_id}.")))?;
        let value = parse_edit(&column.spec, input)?;
        let value = value_json(&Some(value))?;
        let conn = self.lock();
        let row_table: Option<String> = conn
            .query_row(
                "SELECT table_id FROM table_row WHERE id = ?1",
                params![row_id],
                |r| r.get(0),
            )
            .optional()?;
        if row_table.as_deref() != Some(column.table_id.as_str()) {
            return Err(ResearchError::NotFound(format!(
                "No row {row_id} in this table."
            )));
        }
        conn.execute(
            "INSERT INTO table_cell(table_id, row_id, column_id, status, value_json, column_revision, edited_at)
             VALUES (?1, ?2, ?3, 'edited', ?4, ?5, ?6)
             ON CONFLICT(row_id, column_id) DO UPDATE SET status = 'edited',
               value_json = excluded.value_json, quote = NULL, chunk_id = NULL, page = NULL,
               bbox_json = NULL, model = NULL, column_revision = excluded.column_revision,
               edited_at = excluded.edited_at, error = NULL",
            params![column.table_id, row_id, column_id, value, column.revision, now_text()],
        )?;
        drop(conn);
        self.cell(row_id, column_id)?
            .ok_or_else(|| ResearchError::NotFound(format!("No cell {row_id}/{column_id}.")))
    }

    /// Empties a cell (an edit included); the next fill fills it.
    pub fn clear_cell(&self, row_id: &str, column_id: &str) -> ResearchResult<bool> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = tx.execute(
            "DELETE FROM table_cell WHERE row_id = ?1 AND column_id = ?2",
            params![row_id, column_id],
        )?;
        sweep_cache(&tx)?;
        tx.commit()?;
        Ok(removed > 0)
    }

    pub fn cached(
        &self,
        paper_hash: &str,
        column_key: &str,
        model: &str,
    ) -> ResearchResult<Option<String>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT response_json FROM fill_cache
                 WHERE paper_hash = ?1 AND column_key = ?2 AND model = ?3",
                params![paper_hash, column_key, model],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Forgets papers removed from the library: their rows in every table, their cells,
    /// and every cache entry no remaining cell uses. Returns the rows removed.
    pub fn forget_documents(&self, documents: &[String]) -> ResearchResult<usize> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut removed = 0;
        for document in documents {
            let row_ids: Vec<String> = {
                let mut stmt = tx.prepare("SELECT id FROM table_row WHERE document_id = ?1")?;
                let rows = stmt.query_map(params![path_key(document)], |r| r.get(0))?;
                rows.collect::<Result<_, _>>()?
            };
            for id in &row_ids {
                removed += delete_row_in(&tx, id)?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::workspaces::{NewWorkspace, WorkspaceAuthor, WorkspaceStore};

    struct Fixture {
        _dir: tempfile::TempDir,
        path: PathBuf,
        store: TableStore,
        workspace: String,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let workspace = WorkspaceStore::open(&path, None)
            .unwrap()
            .create(
                &NewWorkspace {
                    name: "Survey".to_string(),
                    ..Default::default()
                },
                WorkspaceAuthor::User,
            )
            .unwrap()
            .id;
        let store = TableStore::open(&path, None).unwrap();
        Fixture {
            _dir: dir,
            path,
            store,
            workspace,
        }
    }

    fn spec(question: &str, kind: ColumnKind) -> ColumnSpec {
        ColumnSpec {
            question: question.to_string(),
            kind,
            options: Vec::new(),
            instructions: String::new(),
        }
    }

    fn filled(column: &Column, row: &TableRow, hash: &str) -> Cell {
        Cell {
            table_id: column.table_id.clone(),
            row_id: row.id.clone(),
            column_id: column.id.clone(),
            status: CellStatus::Verified,
            value: Some(CellValue::Text {
                text: "LAION-400M".to_string(),
            }),
            quote: Some("We pre-train on LAION-400M".to_string()),
            chunk_id: Some("chunk-1".to_string()),
            page: Some(3),
            bboxes: Some(r#"[{"page":3,"x0":1.0,"y0":2.0,"x1":3.0,"y1":4.0}]"#.to_string()),
            model: Some("test-model".to_string()),
            column_revision: Some(column.revision),
            paper_hash: Some(hash.to_string()),
            filled_at: Some(now_text()),
            edited_at: None,
            error: None,
        }
    }

    fn entry(column: &Column, hash: &str) -> CacheEntry {
        CacheEntry {
            paper_hash: hash.to_string(),
            column_key: column.key.clone(),
            model: "test-model".to_string(),
            response_json:
                r#"{"answer":"LAION-400M","quote":"We pre-train on LAION-400M","passage":1}"#
                    .to_string(),
        }
    }

    fn cache_rows(f: &Fixture) -> i64 {
        f.store
            .lock()
            .query_row("SELECT count(*) FROM fill_cache", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn no_delete_or_replace_path_leaves_an_unused_cache_entry() {
        let f = fixture();
        let one = f.store.create_table(&f.workspace, "A").unwrap();
        let two = f.store.create_table(&f.workspace, "B").unwrap();
        let q = spec("Dataset?", ColumnKind::Text);
        let c1 = f.store.add_column(&one.id, &q).unwrap();
        let c2 = f.store.add_column(&two.id, &q).unwrap();
        let r1 = f
            .store
            .add_rows(&one.id, &["/p/a.pdf".to_string()])
            .unwrap()
            .remove(0);
        let r2 = f
            .store
            .add_rows(&two.id, &["/p/a.pdf".to_string()])
            .unwrap()
            .remove(0);

        // A cleared cell, then its table.
        f.store
            .store_fill(&filled(&c1, &r1, "h"), Some(&entry(&c1, "h")))
            .unwrap();
        assert_eq!(cache_rows(&f), 1);
        assert!(f.store.clear_cell(&r1.id, &c1.id).unwrap());
        assert_eq!(cache_rows(&f), 0, "clearing the only cell that used it");
        f.store
            .store_fill(&filled(&c1, &r1, "h"), Some(&entry(&c1, "h")))
            .unwrap();
        f.store.clear_cell(&r1.id, &c1.id).unwrap();
        assert!(f.store.delete_table(&one.id).unwrap());
        assert_eq!(cache_rows(&f), 0);

        // An entry two tables share stays until the last cell goes.
        let three = f.store.create_table(&f.workspace, "C").unwrap();
        let c3 = f.store.add_column(&three.id, &q).unwrap();
        let r3 = f
            .store
            .add_rows(&three.id, &["/p/a.pdf".to_string()])
            .unwrap()
            .remove(0);
        f.store
            .store_fill(&filled(&c3, &r3, "h"), Some(&entry(&c3, "h")))
            .unwrap();
        f.store
            .store_fill(&filled(&c2, &r2, "h"), Some(&entry(&c2, "h")))
            .unwrap();
        assert_eq!(
            cache_rows(&f),
            1,
            "one entry for the same paper and question"
        );
        f.store.clear_cell(&r3.id, &c3.id).unwrap();
        assert_eq!(cache_rows(&f), 1, "table B still uses it");
        f.store.clear_cell(&r2.id, &c2.id).unwrap();
        assert_eq!(cache_rows(&f), 0);

        // A refill with a new paper hash leaves no entry of the old one.
        f.store
            .store_fill(&filled(&c2, &r2, "old"), Some(&entry(&c2, "old")))
            .unwrap();
        f.store
            .store_fill(&filled(&c2, &r2, "new"), Some(&entry(&c2, "new")))
            .unwrap();
        assert!(f
            .store
            .cached("old", &c2.key, "test-model")
            .unwrap()
            .is_none());
        assert!(f
            .store
            .cached("new", &c2.key, "test-model")
            .unwrap()
            .is_some());

        // Forgetting a paper after one of its cells was cleared.
        let other = f
            .store
            .add_column(&two.id, &spec("Method?", ColumnKind::Text))
            .unwrap();
        f.store
            .store_fill(&filled(&other, &r2, "new"), Some(&entry(&other, "new")))
            .unwrap();
        f.store.clear_cell(&r2.id, &c2.id).unwrap();
        assert_eq!(cache_rows(&f), 1);
        assert_eq!(
            f.store.forget_documents(&["/p/a.pdf".to_string()]).unwrap(),
            2
        );
        assert_eq!(cache_rows(&f), 0);
    }

    #[test]
    fn a_table_cannot_be_created_in_a_missing_workspace() {
        let f = fixture();
        assert!(matches!(
            f.store.create_table("gone", "T"),
            Err(ResearchError::NotFound(_))
        ));
        assert!(f.store.tables("gone").unwrap().is_empty());
    }

    #[test]
    fn tables_columns_and_rows_round_trip_in_order() {
        let f = fixture();
        assert!(matches!(
            f.store.create_table("ws-missing", "T"),
            Err(ResearchError::NotFound(_))
        ));
        let table = f
            .store
            .create_table(&f.workspace, "  Retrieval  papers ")
            .unwrap();
        assert_eq!(table.name, "Retrieval papers");
        let a = f
            .store
            .add_column(&table.id, &spec("Method?", ColumnKind::Text))
            .unwrap();
        let b = f
            .store
            .add_column(&table.id, &spec("Sample size?", ColumnKind::Number))
            .unwrap();
        assert_eq!((a.position, b.position), (0, 1));
        assert_eq!(f.store.columns(&table.id).unwrap(), vec![a.clone(), b]);
        let rows = f
            .store
            .add_rows(
                &table.id,
                &[
                    "/papers/A.pdf".to_string(),
                    "/papers/B.pdf".to_string(),
                    "/papers/A.pdf".to_string(),
                ],
            )
            .unwrap();
        assert_eq!(rows.len(), 2, "a paper is a row once");
        assert_eq!(rows[0].document_id, path_key("/papers/A.pdf"));
        let again = f
            .store
            .add_rows(&table.id, &["/papers/A.pdf".to_string()])
            .unwrap();
        assert!(again.is_empty());
        assert_eq!(f.store.rows(&table.id).unwrap(), rows);
        let renamed = f.store.rename_table(&table.id, "Dense retrieval").unwrap();
        assert_eq!(renamed.name, "Dense retrieval");
        assert_eq!(f.store.tables(&f.workspace).unwrap(), vec![renamed]);
    }

    #[test]
    fn editing_a_column_bumps_its_revision_only_when_its_key_changes() {
        let f = fixture();
        let table = f.store.create_table(&f.workspace, "T").unwrap();
        let column = f
            .store
            .add_column(&table.id, &spec("Method?", ColumnKind::Text))
            .unwrap();
        let same = f
            .store
            .update_column(&column.id, &spec("  Method? ", ColumnKind::Text))
            .unwrap();
        assert_eq!(same.revision, 1);
        let changed = f
            .store
            .update_column(&column.id, &spec("Main method?", ColumnKind::Text))
            .unwrap();
        assert_eq!(changed.revision, 2);
        assert_ne!(changed.key, column.key);
    }

    #[test]
    fn a_fill_never_overwrites_an_edit_or_a_newer_column() {
        let f = fixture();
        let table = f.store.create_table(&f.workspace, "T").unwrap();
        let column = f
            .store
            .add_column(&table.id, &spec("Dataset?", ColumnKind::Text))
            .unwrap();
        let row = f
            .store
            .add_rows(&table.id, &["/p/a.pdf".to_string()])
            .unwrap()
            .remove(0);

        assert!(f
            .store
            .store_fill(&filled(&column, &row, "h1"), Some(&entry(&column, "h1")))
            .unwrap());
        assert!(f
            .store
            .cached("h1", &column.key, "test-model")
            .unwrap()
            .is_some());

        let edited = f
            .store
            .edit_cell(&row.id, &column.id, "  my value ")
            .unwrap();
        assert_eq!(edited.status, CellStatus::Edited);
        assert_eq!(
            edited.value,
            Some(CellValue::Text {
                text: "my value".to_string()
            })
        );
        assert_eq!(edited.quote, None);
        assert_eq!(edited.paper_hash.as_deref(), Some("h1"));

        // A fill that finishes after the edit leaves the edit and writes no cache entry.
        let late = filled(&column, &row, "h2");
        assert!(!f
            .store
            .store_fill(&late, Some(&entry(&column, "h2")))
            .unwrap());
        assert_eq!(
            f.store.cell(&row.id, &column.id).unwrap().unwrap().status,
            CellStatus::Edited
        );
        assert!(f
            .store
            .cached("h2", &column.key, "test-model")
            .unwrap()
            .is_none());

        // A fill for an older revision of the column is not stored.
        f.store.clear_cell(&row.id, &column.id).unwrap();
        let newer = f
            .store
            .update_column(&column.id, &spec("Largest dataset?", ColumnKind::Text))
            .unwrap();
        assert!(!f
            .store
            .store_fill(&filled(&column, &row, "h1"), Some(&entry(&column, "h1")))
            .unwrap());
        assert!(f.store.cell(&row.id, &column.id).unwrap().is_none());
        assert!(f
            .store
            .store_fill(&filled(&newer, &row, "h1"), Some(&entry(&newer, "h1")))
            .unwrap());

        // A cell of a deleted row is not stored.
        f.store.remove_row(&row.id).unwrap();
        assert!(!f
            .store
            .store_fill(&filled(&newer, &row, "h1"), None)
            .unwrap());
    }

    #[test]
    fn edits_follow_the_column_type() {
        let f = fixture();
        let table = f.store.create_table(&f.workspace, "T").unwrap();
        let column = f
            .store
            .add_column(&table.id, &spec("Released?", ColumnKind::YesNo))
            .unwrap();
        let row = f
            .store
            .add_rows(&table.id, &["/p/a.pdf".to_string()])
            .unwrap()
            .remove(0);
        assert!(matches!(
            f.store.edit_cell(&row.id, &column.id, "perhaps"),
            Err(ResearchError::Invalid(_))
        ));
        let cell = f.store.edit_cell(&row.id, &column.id, "No").unwrap();
        assert_eq!(cell.value, Some(CellValue::YesNo { yes: false }));
        let other = f.store.create_table(&f.workspace, "U").unwrap();
        let foreign = f
            .store
            .add_rows(&other.id, &["/p/b.pdf".to_string()])
            .unwrap()
            .remove(0);
        assert!(matches!(
            f.store.edit_cell(&foreign.id, &column.id, "yes"),
            Err(ResearchError::NotFound(_))
        ));
    }

    #[test]
    fn deleting_a_table_removes_everything_but_cache_entries_another_table_uses() {
        let f = fixture();
        let first = f.store.create_table(&f.workspace, "A").unwrap();
        let second = f.store.create_table(&f.workspace, "B").unwrap();
        let q = spec("Dataset?", ColumnKind::Text);
        let only = spec("Licence?", ColumnKind::Text);
        let c1 = f.store.add_column(&first.id, &q).unwrap();
        let c1b = f.store.add_column(&first.id, &only).unwrap();
        let c2 = f.store.add_column(&second.id, &q).unwrap();
        let r1 = f
            .store
            .add_rows(&first.id, &["/p/a.pdf".to_string()])
            .unwrap()
            .remove(0);
        let r2 = f
            .store
            .add_rows(&second.id, &["/p/a.pdf".to_string()])
            .unwrap()
            .remove(0);
        f.store
            .store_fill(&filled(&c1, &r1, "h"), Some(&entry(&c1, "h")))
            .unwrap();
        f.store
            .store_fill(&filled(&c1b, &r1, "h"), Some(&entry(&c1b, "h")))
            .unwrap();
        f.store
            .store_fill(&filled(&c2, &r2, "h"), Some(&entry(&c2, "h")))
            .unwrap();

        assert!(f.store.delete_table(&first.id).unwrap());
        assert!(f.store.table(&first.id).unwrap().is_none());
        assert!(f.store.columns(&first.id).unwrap().is_empty());
        assert!(f.store.rows(&first.id).unwrap().is_empty());
        assert!(f.store.cells(&first.id).unwrap().is_empty());
        assert!(
            f.store
                .cached("h", &c1.key, "test-model")
                .unwrap()
                .is_some(),
            "table B still uses it"
        );
        assert!(f
            .store
            .cached("h", &c1b.key, "test-model")
            .unwrap()
            .is_none());
        assert!(!f.store.delete_table(&first.id).unwrap());
    }

    #[test]
    fn changing_or_deleting_a_column_removes_its_cache_entries() {
        let f = fixture();
        let table = f.store.create_table(&f.workspace, "T").unwrap();
        let column = f
            .store
            .add_column(&table.id, &spec("Dataset?", ColumnKind::Text))
            .unwrap();
        let row = f
            .store
            .add_rows(&table.id, &["/p/a.pdf".to_string()])
            .unwrap()
            .remove(0);
        f.store
            .store_fill(&filled(&column, &row, "h"), Some(&entry(&column, "h")))
            .unwrap();
        let changed = f
            .store
            .update_column(&column.id, &spec("Datasets?", ColumnKind::Text))
            .unwrap();
        assert!(f
            .store
            .cached("h", &column.key, "test-model")
            .unwrap()
            .is_none());
        f.store
            .store_fill(&filled(&changed, &row, "h"), Some(&entry(&changed, "h")))
            .unwrap();
        assert!(f.store.delete_column(&changed.id).unwrap());
        assert!(f
            .store
            .cached("h", &changed.key, "test-model")
            .unwrap()
            .is_none());
        assert!(f.store.cells(&table.id).unwrap().is_empty());
    }

    #[test]
    fn forgetting_a_paper_removes_its_rows_cells_and_cache_in_every_table() {
        let f = fixture();
        let one = f.store.create_table(&f.workspace, "A").unwrap();
        let two = f.store.create_table(&f.workspace, "B").unwrap();
        let c1 = f
            .store
            .add_column(&one.id, &spec("Dataset?", ColumnKind::Text))
            .unwrap();
        let c2 = f
            .store
            .add_column(&two.id, &spec("Method?", ColumnKind::Text))
            .unwrap();
        let gone = vec!["/p/gone.pdf".to_string()];
        let kept = vec!["/p/kept.pdf".to_string()];
        let r1 = f.store.add_rows(&one.id, &gone).unwrap().remove(0);
        let r2 = f.store.add_rows(&two.id, &gone).unwrap().remove(0);
        let r3 = f.store.add_rows(&two.id, &kept).unwrap().remove(0);
        f.store
            .store_fill(&filled(&c1, &r1, "hg"), Some(&entry(&c1, "hg")))
            .unwrap();
        f.store
            .store_fill(&filled(&c2, &r2, "hg"), Some(&entry(&c2, "hg")))
            .unwrap();
        f.store
            .store_fill(&filled(&c2, &r3, "hk"), Some(&entry(&c2, "hk")))
            .unwrap();

        assert_eq!(f.store.forget_documents(&gone).unwrap(), 2);
        assert!(f.store.rows(&one.id).unwrap().is_empty());
        assert_eq!(f.store.rows(&two.id).unwrap(), vec![r3.clone()]);
        assert!(f
            .store
            .cached("hg", &c1.key, "test-model")
            .unwrap()
            .is_none());
        assert!(f
            .store
            .cached("hg", &c2.key, "test-model")
            .unwrap()
            .is_none());
        assert!(f
            .store
            .cached("hk", &c2.key, "test-model")
            .unwrap()
            .is_some());
        assert_eq!(f.store.cells(&two.id).unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_workspace_deletes_its_tables() {
        let f = fixture();
        let table = f.store.create_table(&f.workspace, "T").unwrap();
        let column = f
            .store
            .add_column(&table.id, &spec("Dataset?", ColumnKind::Text))
            .unwrap();
        let row = f
            .store
            .add_rows(&table.id, &["/p/a.pdf".to_string()])
            .unwrap()
            .remove(0);
        f.store
            .store_fill(&filled(&column, &row, "h"), Some(&entry(&column, "h")))
            .unwrap();
        let workspaces = WorkspaceStore::open(&f.path, None).unwrap();
        assert!(workspaces.delete(&f.workspace).unwrap());
        assert!(f.store.tables(&f.workspace).unwrap().is_empty());
        assert!(f.store.cells(&table.id).unwrap().is_empty());
        assert!(f
            .store
            .cached("h", &column.key, "test-model")
            .unwrap()
            .is_none());
    }
}
