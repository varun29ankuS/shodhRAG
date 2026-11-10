//! Recall's catalog (`recall.db`, SQLite, beside its index): every transcript found,
//! what of it is indexed (parts and their hashes), where each turn's chunks are, the
//! entities of each turn, the lines that could not be read, and the sessions the user
//! removed. Of a transcript's text it holds only short redacted pieces: each session's
//! title (its generated title or the start of its first prompt), branch and model, the
//! entities, and previews of unreadable lines. Turns are read from the transcript itself
//! when a receipt opens them.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use super::entities::{Entity, EntityKind};
use super::{LineError, Role, Tool};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    tool TEXT NOT NULL,
    session_id TEXT NOT NULL,
    thread TEXT,
    project_key TEXT NOT NULL,
    project_dir TEXT NOT NULL,
    format TEXT,
    title TEXT,
    branch TEXT,
    model TEXT,
    started_at INTEGER,
    ended_at INTEGER,
    turns INTEGER NOT NULL DEFAULT 0,
    size INTEGER NOT NULL DEFAULT 0,
    mtime INTEGER NOT NULL DEFAULT 0,
    parts INTEGER NOT NULL DEFAULT 0,
    errors INTEGER NOT NULL DEFAULT 0,
    redacted INTEGER NOT NULL DEFAULT 0,
    indexed_at INTEGER,
    fingerprint TEXT,
    state TEXT NOT NULL,
    failure TEXT,
    cwd_chains TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS files_session ON files(tool, session_id);
CREATE INDEX IF NOT EXISTS files_project ON files(tool, project_key);
CREATE TABLE IF NOT EXISTS parts (
    file_id INTEGER NOT NULL,
    part INTEGER NOT NULL,
    hash TEXT NOT NULL,
    PRIMARY KEY (file_id, part)
);
CREATE TABLE IF NOT EXISTS turns (
    file_id INTEGER NOT NULL,
    turn INTEGER NOT NULL,
    part INTEGER NOT NULL,
    chunk_first INTEGER,
    chunk_last INTEGER,
    role TEXT NOT NULL,
    tool_name TEXT,
    at INTEGER,
    line INTEGER NOT NULL,
    offset INTEGER NOT NULL DEFAULT 0,
    kept_back TEXT,
    PRIMARY KEY (file_id, turn)
);
CREATE INDEX IF NOT EXISTS turns_chunks ON turns(file_id, part, chunk_first);
CREATE TABLE IF NOT EXISTS entities (
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    file_id INTEGER NOT NULL,
    turn INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS entities_key ON entities(key);
CREATE INDEX IF NOT EXISTS entities_file ON entities(file_id);
CREATE TABLE IF NOT EXISTS errors (
    file_id INTEGER NOT NULL,
    line INTEGER NOT NULL,
    offset INTEGER NOT NULL,
    message TEXT NOT NULL,
    preview TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS errors_file ON errors(file_id);
CREATE TABLE IF NOT EXISTS removed_sessions (
    tool TEXT NOT NULL,
    session_id TEXT NOT NULL,
    removed_at INTEGER NOT NULL,
    PRIMARY KEY (tool, session_id)
);
";

/// Where a transcript stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// Found, not indexed yet.
    Found,
    Indexed,
    /// Indexing it failed (the reason is kept).
    Failed,
    /// Not wanted (its tool or project is not chosen, or the user removed its session):
    /// nothing of it is indexed.
    Excluded,
}

impl FileState {
    fn id(self) -> &'static str {
        match self {
            FileState::Found => "found",
            FileState::Indexed => "indexed",
            FileState::Failed => "failed",
            FileState::Excluded => "excluded",
        }
    }

    fn from_id(id: &str) -> Self {
        match id {
            "indexed" => FileState::Indexed,
            "failed" => FileState::Failed,
            "excluded" => FileState::Excluded,
            _ => FileState::Found,
        }
    }
}

/// A transcript in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    pub id: i64,
    pub path: String,
    pub tool: Tool,
    pub session_id: String,
    pub thread: Option<String>,
    pub project_key: String,
    pub project_dir: String,
    pub format: Option<String>,
    pub title: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
    /// Unix seconds.
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub turns: u32,
    pub size: u64,
    pub mtime: i64,
    pub parts: u32,
    pub errors: u32,
    pub redacted: u32,
    pub indexed_at: Option<i64>,
    pub fingerprint: Option<String>,
    pub state: FileState,
    pub failure: Option<String>,
    /// The key chains of the folders the session worked in, the one it started in first
    /// ([`super::settings::RecallSettings::wants`]).
    pub chains: Vec<Vec<String>>,
}

/// Where a turn is: its part, its chunks in the part, and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRow {
    pub file_id: i64,
    pub turn: u32,
    pub part: u32,
    /// The turn's chunks in its part (`None`: nothing of it is indexed).
    pub chunks: Option<(u32, u32)>,
    pub role: Role,
    pub tool_name: Option<String>,
    pub at: Option<i64>,
    pub line: u64,
    /// Byte offset of its line: opening a receipt reads from there.
    pub offset: u64,
    /// Why the turn's text is never shown (a call or an output of a secret file).
    pub kept_back: Option<String>,
}

/// What a sync stores for a transcript it indexed.
#[derive(Debug, Clone, Default)]
pub struct IndexedFile {
    pub format: Option<String>,
    pub session_id: String,
    pub thread: Option<String>,
    pub title: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub size: u64,
    pub mtime: i64,
    pub parts: Vec<(u32, String)>,
    pub turns: Vec<TurnRow>,
    pub entities: Vec<(Entity, u32)>,
    pub errors: Vec<LineError>,
    pub error_count: u32,
    pub redacted: u32,
    pub fingerprint: String,
    pub chains: Vec<Vec<String>>,
}

/// A transcript found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundFile {
    pub path: String,
    pub tool: Tool,
    pub session_id: String,
    pub thread: Option<String>,
    pub project_key: String,
    pub project_dir: String,
    pub chains: Vec<Vec<String>>,
    pub size: u64,
    pub mtime: i64,
}

pub struct Catalog {
    conn: Mutex<Connection>,
    /// The catalog was made by another version and emptied when opened: whatever the
    /// index holds must go too ([`super::index::RecallIndex::open`]).
    reset: bool,
}

type Result<T> = std::result::Result<T, rusqlite::Error>;

/// Version of the catalog's tables. A catalog of another version is emptied when opened.
pub const CATALOG_VERSION: &str = "2";

const FILE_COLUMNS: &str = "id, path, tool, session_id, thread, project_key, project_dir, format, title, branch, model, started_at, ended_at, turns, size, mtime, parts, errors, redacted, indexed_at, fingerprint, state, failure, cwd_chains";

fn chains_json(chains: &[Vec<String>]) -> String {
    serde_json::to_string(chains).unwrap_or_else(|_| "[]".to_string())
}

fn file_row(row: &rusqlite::Row<'_>) -> Result<FileRecord> {
    let tool: String = row.get(2)?;
    let state: String = row.get(21)?;
    Ok(FileRecord {
        id: row.get(0)?,
        path: row.get(1)?,
        tool: Tool::from_id(&tool).unwrap_or(Tool::ClaudeCode),
        session_id: row.get(3)?,
        thread: row.get(4)?,
        project_key: row.get(5)?,
        project_dir: row.get(6)?,
        format: row.get(7)?,
        title: row.get(8)?,
        branch: row.get(9)?,
        model: row.get(10)?,
        started_at: row.get(11)?,
        ended_at: row.get(12)?,
        turns: row.get(13)?,
        size: row.get::<_, i64>(14)?.max(0) as u64,
        mtime: row.get(15)?,
        parts: row.get(16)?,
        errors: row.get(17)?,
        redacted: row.get(18)?,
        indexed_at: row.get(19)?,
        fingerprint: row.get(20)?,
        state: FileState::from_id(&state),
        failure: row.get(22)?,
        chains: serde_json::from_str(&row.get::<_, String>(23)?).unwrap_or_default(),
    })
}

fn turn_row(row: &rusqlite::Row<'_>) -> Result<TurnRow> {
    let first: Option<u32> = row.get(3)?;
    let last: Option<u32> = row.get(4)?;
    let role: String = row.get(5)?;
    Ok(TurnRow {
        file_id: row.get(0)?,
        turn: row.get(1)?,
        part: row.get(2)?,
        chunks: first.zip(last),
        role: Role::from_id(&role).unwrap_or(Role::User),
        tool_name: row.get(6)?,
        at: row.get(7)?,
        line: row.get::<_, i64>(8)?.max(0) as u64,
        offset: row.get::<_, i64>(9)?.max(0) as u64,
        kept_back: row.get(10)?,
    })
}

const TURN_COLUMNS: &str =
    "file_id, turn, part, chunk_first, chunk_last, role, tool_name, at, line, offset, kept_back";

impl Catalog {
    /// Opens (or creates) the catalog at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            // The open below reports a folder that cannot be made.
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        Self::with_connection(conn)
    }

    /// A catalog in memory (tests).
    pub fn in_memory() -> Result<Self> {
        Self::with_connection(Connection::open_in_memory()?)
    }

    fn with_connection(conn: Connection) -> Result<Self> {
        // In memory the journal stays `memory`: the answer is read, not checked.
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA secure_delete = ON;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        let version: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'catalog_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let has_files: bool = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'files'",
            [],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?;
        let reset = has_files && version.as_deref() != Some(CATALOG_VERSION);
        if reset {
            conn.execute_batch(
                "DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS parts; DROP TABLE IF EXISTS turns;
                 DROP TABLE IF EXISTS entities; DROP TABLE IF EXISTS errors; DROP TABLE IF EXISTS removed_sessions;",
            )?;
        }
        conn.execute_batch(SCHEMA)?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('catalog_version', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [CATALOG_VERSION],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            reset,
        })
    }

    /// The catalog was emptied when opened (made by another version).
    pub fn was_reset(&self) -> bool {
        self.reset
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        self.conn()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Adds a transcript found on disk, or returns the record it already has (its
    /// project and session are refreshed; what is indexed is left as it is).
    pub fn upsert_found(&self, found: &FoundFile) -> Result<FileRecord> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO files (path, tool, session_id, thread, project_key, project_dir, size, mtime, state, cwd_chains)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, 'found', ?7)
             ON CONFLICT(path) DO UPDATE SET project_key = excluded.project_key, project_dir = excluded.project_dir,
                 cwd_chains = CASE WHEN files.cwd_chains = '[]' THEN excluded.cwd_chains ELSE files.cwd_chains END",
            params![
                found.path,
                found.tool.id(),
                found.session_id,
                found.thread,
                found.project_key,
                found.project_dir,
                chains_json(&found.chains)
            ],
        )?;
        conn.query_row(
            &format!("SELECT {FILE_COLUMNS} FROM files WHERE path = ?1"),
            [&found.path],
            file_row,
        )
    }

    pub fn files(&self) -> Result<Vec<FileRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT {FILE_COLUMNS} FROM files ORDER BY id"))?;
        let rows = stmt.query_map([], file_row)?;
        rows.collect()
    }

    pub fn file(&self, id: i64) -> Result<Option<FileRecord>> {
        self.conn()
            .query_row(
                &format!("SELECT {FILE_COLUMNS} FROM files WHERE id = ?1"),
                [id],
                file_row,
            )
            .optional()
    }

    /// The files of a session (the main thread first, then subagents).
    pub fn session_files(&self, tool: Tool, session_id: &str) -> Result<Vec<FileRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {FILE_COLUMNS} FROM files WHERE tool = ?1 AND session_id = ?2 ORDER BY thread IS NOT NULL, started_at, id"
        ))?;
        let rows = stmt.query_map(params![tool.id(), session_id], file_row)?;
        rows.collect()
    }

    pub fn part_hashes(&self, file_id: i64) -> Result<Vec<(u32, String)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT part, hash FROM parts WHERE file_id = ?1 ORDER BY part")?;
        let rows = stmt.query_map([file_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// Records what a sync indexed of a file, replacing what was recorded before.
    pub fn record_indexed(&self, file_id: i64, indexed: &IndexedFile, now: i64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        clear_children(&tx, file_id)?;
        tx.execute(
            "UPDATE files SET format = ?2, session_id = ?3, thread = ?4, title = ?5, branch = ?6, model = ?7,
                 started_at = ?8, ended_at = ?9, turns = ?10, size = ?11, mtime = ?12, parts = ?13,
                 errors = ?14, redacted = ?15, indexed_at = ?16, fingerprint = ?17, state = 'indexed', failure = NULL,
                 cwd_chains = CASE WHEN ?18 = '[]' THEN cwd_chains ELSE ?18 END
             WHERE id = ?1",
            params![
                file_id,
                indexed.format,
                indexed.session_id,
                indexed.thread,
                indexed.title,
                indexed.branch,
                indexed.model,
                indexed.started_at,
                indexed.ended_at,
                indexed.turns.len() as i64,
                indexed.size as i64,
                indexed.mtime,
                indexed.parts.len() as i64,
                indexed.error_count,
                indexed.redacted,
                now,
                indexed.fingerprint,
                chains_json(&indexed.chains),
            ],
        )?;
        {
            let mut part =
                tx.prepare("INSERT INTO parts (file_id, part, hash) VALUES (?1, ?2, ?3)")?;
            for (number, hash) in &indexed.parts {
                part.execute(params![file_id, number, hash])?;
            }
            let mut turn = tx.prepare(
                "INSERT INTO turns (file_id, turn, part, chunk_first, chunk_last, role, tool_name, at, line, offset, kept_back)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for t in &indexed.turns {
                turn.execute(params![
                    file_id,
                    t.turn,
                    t.part,
                    t.chunks.map(|c| c.0),
                    t.chunks.map(|c| c.1),
                    t.role.id(),
                    t.tool_name,
                    t.at,
                    t.line as i64,
                    t.offset as i64,
                    t.kept_back
                ])?;
            }
            let mut entity = tx.prepare(
                "INSERT INTO entities (kind, key, value, file_id, turn) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (e, turn) in &indexed.entities {
                entity.execute(params![e.kind.id(), e.key(), e.value, file_id, turn])?;
            }
            let mut error = tx.prepare(
                "INSERT INTO errors (file_id, line, offset, message, preview) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for e in &indexed.errors {
                error.execute(params![
                    file_id,
                    e.line as i64,
                    e.offset as i64,
                    e.message,
                    e.preview
                ])?;
            }
        }
        tx.commit()
    }

    /// Sets a file's state (found again, or excluded), forgetting what was recorded of its
    /// index: the caller removed its chunks.
    pub fn set_state(&self, file_id: i64, state: FileState, reason: Option<&str>) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        clear_children(&tx, file_id)?;
        tx.execute(
            "UPDATE files SET parts = 0, turns = 0, indexed_at = NULL, fingerprint = NULL, state = ?2, failure = ?3,
                 title = NULL, branch = NULL, model = NULL WHERE id = ?1",
            params![file_id, state.id(), reason],
        )?;
        tx.commit()
    }

    /// Records the folders a session worked in.
    pub fn set_chains(&self, file_id: i64, chains: &[Vec<String>]) -> Result<()> {
        self.conn().execute(
            "UPDATE files SET cwd_chains = ?2 WHERE id = ?1",
            params![file_id, chains_json(chains)],
        )?;
        Ok(())
    }

    /// Marks a file failed at the size and time it had (`reason` in plain words): it is
    /// tried again once it changes. The caller removed its chunks.
    pub fn record_failure(&self, file_id: i64, size: u64, mtime: i64, reason: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        clear_children(&tx, file_id)?;
        tx.execute(
            "UPDATE files SET parts = 0, turns = 0, indexed_at = NULL, fingerprint = NULL, state = 'failed', failure = ?2, size = ?3, mtime = ?4,
                 title = NULL, branch = NULL, model = NULL WHERE id = ?1",
            params![file_id, reason, size as i64, mtime],
        )?;
        tx.commit()
    }

    /// Forgets a file entirely (it left the disk).
    pub fn remove_file(&self, file_id: i64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        clear_children(&tx, file_id)?;
        tx.execute("DELETE FROM files WHERE id = ?1", [file_id])?;
        tx.commit()
    }

    /// Forgets everything (the index is rebuilt from the transcripts).
    pub fn clear(&self) -> Result<()> {
        self.conn().execute_batch(
            "DELETE FROM parts; DELETE FROM turns; DELETE FROM entities; DELETE FROM errors;
             UPDATE files SET parts = 0, turns = 0, indexed_at = NULL, fingerprint = NULL, state = 'found', failure = NULL;",
        )
    }

    /// The turn whose chunks include chunk `chunk` of part `part` of a file.
    pub fn turn_for_chunk(&self, file_id: i64, part: u32, chunk: u32) -> Result<Option<TurnRow>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {TURN_COLUMNS} FROM turns WHERE file_id = ?1 AND part = ?2 AND chunk_first <= ?3 AND chunk_last >= ?3"
                ),
                params![file_id, part, chunk],
                turn_row,
            )
            .optional()
    }

    pub fn turn(&self, file_id: i64, turn: u32) -> Result<Option<TurnRow>> {
        self.conn()
            .query_row(
                &format!("SELECT {TURN_COLUMNS} FROM turns WHERE file_id = ?1 AND turn = ?2"),
                params![file_id, turn],
                turn_row,
            )
            .optional()
    }

    /// The turns `from..=to` of a file, in order.
    pub fn turn_range(&self, file_id: i64, from: u32, to: u32) -> Result<Vec<TurnRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TURN_COLUMNS} FROM turns WHERE file_id = ?1 AND turn >= ?2 AND turn <= ?3 ORDER BY turn"
        ))?;
        let rows = stmt.query_map(params![file_id, from, to], turn_row)?;
        rows.collect()
    }

    /// The first turn read from a line of a file (a line can hold several turns).
    pub fn first_turn_of_line(&self, file_id: i64, line: u64) -> Result<Option<u32>> {
        self.conn().query_row(
            "SELECT MIN(turn) FROM turns WHERE file_id = ?1 AND line = ?2",
            params![file_id, line as i64],
            |r| r.get(0),
        )
    }

    /// The first turn of a file at or after `at` (Unix seconds).
    pub fn first_turn_from(&self, file_id: i64, at: i64) -> Result<Option<TurnRow>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {TURN_COLUMNS} FROM turns WHERE file_id = ?1 AND at >= ?2 ORDER BY turn LIMIT 1"
                ),
                params![file_id, at],
                turn_row,
            )
            .optional()
    }

    /// The (file, turn) pairs that hold an entity: exactly `key` (an entity's lookup
    /// form), or, for a file, any path ending with it (`rag_engine.rs` finds
    /// `crates/shodh-rag/src/rag_engine.rs`).
    pub fn entity_turns(
        &self,
        kind: Option<EntityKind>,
        key: &str,
        limit: usize,
    ) -> Result<Vec<(i64, u32, String)>> {
        let conn = self.conn();
        let suffix = format!("%/{}", key.replace('%', "").replace('_', "\\_"));
        let mut stmt = conn.prepare(
            "SELECT file_id, turn, value FROM entities
             WHERE (?1 IS NULL OR kind = ?1) AND (key = ?2 OR (kind = 'file' AND key LIKE ?3 ESCAPE '\\'))
             ORDER BY file_id, turn LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![kind.map(EntityKind::id), key, suffix, limit as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        rows.collect()
    }

    /// Entities of a file's turns of `kinds` (every kind when empty), as `(kind, value)`,
    /// in the order they first appear, at most `limit`.
    pub fn file_entities(
        &self,
        file_id: i64,
        kinds: &[EntityKind],
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT kind, MIN(value) FROM entities WHERE file_id = ?1 GROUP BY kind, key ORDER BY MIN(turn), kind",
        )?;
        let rows = stmt.query_map(params![file_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (kind, value) = row?;
            let wanted =
                kinds.is_empty() || EntityKind::from_id(&kind).is_some_and(|k| kinds.contains(&k));
            if wanted {
                out.push((kind, value));
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    pub fn errors(&self, file_id: i64) -> Result<Vec<LineError>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT line, offset, message, preview FROM errors WHERE file_id = ?1 ORDER BY line",
        )?;
        let rows = stmt.query_map([file_id], |r| {
            Ok(LineError {
                line: r.get::<_, i64>(0)?.max(0) as u64,
                offset: r.get::<_, i64>(1)?.max(0) as u64,
                message: r.get(2)?,
                preview: r.get(3)?,
            })
        })?;
        rows.collect()
    }

    pub fn add_removed(&self, tool: Tool, session_id: &str, now: i64) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO removed_sessions (tool, session_id, removed_at) VALUES (?1, ?2, ?3)",
            params![tool.id(), session_id, now],
        )?;
        Ok(())
    }

    pub fn removed_sessions(&self) -> Result<Vec<(Tool, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT tool, session_id FROM removed_sessions")?;
        let rows = stmt.query_map([], |r| {
            let tool: String = r.get(0)?;
            Ok((tool, r.get::<_, String>(1)?))
        })?;
        Ok(rows
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter_map(|(tool, id)| Tool::from_id(&tool).map(|t| (t, id)))
            .collect())
    }
}

fn clear_children(tx: &rusqlite::Transaction<'_>, file_id: i64) -> Result<()> {
    for table in ["parts", "turns", "entities", "errors"] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE file_id = ?1"),
            [file_id],
        )?;
    }
    Ok(())
}
