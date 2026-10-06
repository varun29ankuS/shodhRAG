//! SQLite storage of the audit chain.
//!
//! Durability: WAL journal with `synchronous=FULL`, so every committed event
//! survives an OS crash or power loss, not only an app crash. Audit volume is
//! a handful of rows per answer, so the extra fsync per commit is negligible.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::Value as SqlValue;
use rusqlite::{
    params, params_from_iter, Connection, ErrorCode, OpenFlags, OptionalExtension,
    TransactionBehavior,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    canonical_json, AuditError, AuditEventType, AuditKey, AuditQuery, AuditRecord, AuditResult,
    AuditRow, AuditStats, MonthStats, VerifyReport, GENESIS_HASH, LOCAL_OWNER, SYSTEM_PRINCIPAL,
};

pub const DEFAULT_RETENTION_DAYS: u32 = 365;
pub const MIN_RETENTION_DAYS: u32 = 1;
pub const MAX_RETENTION_DAYS: u32 = 3650;

const DEFAULT_LIMIT: u32 = 200;
const MAX_LIMIT: u32 = 1_000;
const RETENTION_KEY: &str = "retention_days";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// Ordered schema migrations of `shodh.db`. Never edit an applied entry; append a new one.
/// The database is shared: the audit chain (1), the dynamics of typed statements (2), the
/// generated-visuals gallery (3), learned-memory suggestions (4), research objects (5: snippet
/// images, Result extraction records and rejections) and the citation graph (6: the scholarly
/// API cache, per-file scans and the build report) use one version sequence, so every
/// component that opens it sees the same schema.
const MIGRATIONS: &[(i64, &str)] = &[
    (
        1,
        "CREATE TABLE audit_events (
        id INTEGER PRIMARY KEY,
        ts TEXT NOT NULL,
        principal TEXT NOT NULL,
        conversation_id TEXT NULL,
        profile_id TEXT NULL,
        run_id TEXT NULL,
        event_type TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        prev_hash TEXT NOT NULL,
        hash TEXT NOT NULL
    );
    CREATE INDEX audit_events_ts ON audit_events(ts);
    CREATE INDEX audit_events_type_ts ON audit_events(event_type, ts);
    CREATE INDEX audit_events_conversation ON audit_events(conversation_id);
    CREATE TABLE audit_settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );",
    ),
    (
        2,
        // Mutable state of statements whose content lives in LanceDB: recall strength is
        // stored at an anchor time and decayed lazily at read time, so reads never rewrite
        // rows. Links are undirected (from_id < to_id) Hebbian co-activation weights.
        "CREATE TABLE statement_dynamics (
            statement_id TEXT PRIMARY KEY,
            scope TEXT NOT NULL,
            class TEXT NOT NULL,
            strength REAL NOT NULL CHECK (strength >= 0.0 AND strength <= 1.0),
            anchor_at TEXT NOT NULL,
            importance REAL NOT NULL CHECK (importance >= 0.0 AND importance <= 1.0),
            use_count INTEGER NOT NULL DEFAULT 0,
            last_used_at TEXT NULL,
            pinned INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX statement_dynamics_scope ON statement_dynamics(scope);
        CREATE TABLE statement_links (
            from_id TEXT NOT NULL,
            to_id TEXT NOT NULL,
            weight REAL NOT NULL CHECK (weight >= 0.0 AND weight <= 1.0),
            co_activations INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (from_id, to_id),
            CHECK (from_id < to_id)
        );
        CREATE INDEX statement_links_to ON statement_links(to_id);",
    ),
    (
        3,
        // Visuals generated in answers (diagrams, charts, sketches, plots, simulations,
        // equations, tables), kept so they can be found, refined and reused. A refinement is
        // a new row of the same chain (`root_id`, `version`), never an edit of an earlier one;
        // title, pin, note and deletion belong to the whole chain. Captures of one answer are
        // deduplicated by content hash (`generated_visuals_origin`); the index ignores
        // deletion so a deleted visual is not captured again. `generated_visuals_fts` is an
        // external-content FTS5 index over title, note and source, kept by triggers.
        "CREATE TABLE generated_visuals (
            seq INTEGER PRIMARY KEY,
            id TEXT NOT NULL UNIQUE,
            root_id TEXT NOT NULL,
            parent_id TEXT NULL,
            version INTEGER NOT NULL CHECK (version >= 1),
            conversation_id TEXT NOT NULL,
            message_id TEXT NULL,
            thread_id TEXT NULL,
            turn_id TEXT NULL,
            kind TEXT NOT NULL CHECK (kind IN
                ('mermaid', 'chart', 'svg', 'plot', 'simulation', 'equation', 'table')),
            title TEXT NOT NULL,
            source TEXT NOT NULL,
            params_json TEXT NOT NULL DEFAULT '{}',
            content_hash TEXT NOT NULL,
            pinned INTEGER NOT NULL DEFAULT 0,
            note TEXT NOT NULL DEFAULT '',
            instruction TEXT NULL,
            created_by TEXT NOT NULL CHECK (created_by IN ('capture', 'user', 'agent')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            deleted_at TEXT NULL,
            UNIQUE (root_id, version)
        );
        CREATE UNIQUE INDEX generated_visuals_origin ON generated_visuals(
            conversation_id, IFNULL(message_id, ''), IFNULL(thread_id, ''), IFNULL(turn_id, ''),
            content_hash
        ) WHERE parent_id IS NULL;
        CREATE INDEX generated_visuals_root ON generated_visuals(root_id, version);
        CREATE INDEX generated_visuals_conversation ON generated_visuals(conversation_id);
        CREATE VIRTUAL TABLE generated_visuals_fts USING fts5(
            title, note, source,
            content = 'generated_visuals', content_rowid = 'seq', tokenize = 'unicode61'
        );
        CREATE TRIGGER generated_visuals_fts_insert AFTER INSERT ON generated_visuals BEGIN
            INSERT INTO generated_visuals_fts(rowid, title, note, source)
            VALUES (new.seq, new.title, new.note, new.source);
        END;
        CREATE TRIGGER generated_visuals_fts_delete AFTER DELETE ON generated_visuals BEGIN
            INSERT INTO generated_visuals_fts(generated_visuals_fts, rowid, title, note, source)
            VALUES ('delete', old.seq, old.title, old.note, old.source);
        END;
        CREATE TRIGGER generated_visuals_fts_update AFTER UPDATE OF title, note, source
        ON generated_visuals BEGIN
            INSERT INTO generated_visuals_fts(generated_visuals_fts, rowid, title, note, source)
            VALUES ('delete', old.seq, old.title, old.note, old.source);
            INSERT INTO generated_visuals_fts(rowid, title, note, source)
            VALUES (new.seq, new.title, new.note, new.source);
        END;
        CREATE TABLE visual_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    ),
    (
        4,
        // Learning from conversations: memories an LLM suggests from the user's turns (and
        // from consolidation), waiting for the user's decision or applied by the user's
        // automatic-learning policy. A suggestion is never edited after it is decided;
        // `fingerprint` (class + rendered text) suppresses duplicates and re-suggesting what
        // the user rejected. `memory_learn_usage` counts the learning model's calls per UTC
        // day for the cost caps; `memory_learn_state` keeps small values such as the time of
        // the last consolidation.
        "CREATE TABLE memory_proposals (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL CHECK (kind IN
                ('remember', 'revise', 'link', 'resolve', 'archive')),
            origin TEXT NOT NULL CHECK (origin IN ('turn', 'evolve', 'consolidate')),
            status TEXT NOT NULL CHECK (status IN
                ('pending', 'accepted', 'rejected', 'learned', 'undone', 'stale', 'failed')),
            fingerprint TEXT NOT NULL,
            scope TEXT NOT NULL,
            conversation_id TEXT NULL,
            turn_id TEXT NULL,
            payload_json TEXT NOT NULL,
            confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
            sensitive_json TEXT NOT NULL DEFAULT '[]',
            outcome_json TEXT NULL,
            error TEXT NULL,
            created_at TEXT NOT NULL,
            decided_at TEXT NULL
        );
        CREATE INDEX memory_proposals_status ON memory_proposals(status, created_at);
        CREATE INDEX memory_proposals_fingerprint ON memory_proposals(fingerprint, status);
        CREATE TABLE memory_learn_usage (
            day TEXT PRIMARY KEY,
            llm_calls INTEGER NOT NULL DEFAULT 0,
            input_chars INTEGER NOT NULL DEFAULT 0,
            output_chars INTEGER NOT NULL DEFAULT 0,
            proposals INTEGER NOT NULL DEFAULT 0,
            invalid INTEGER NOT NULL DEFAULT 0,
            refused INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE memory_learn_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    ),
    (
        5,
        // Research objects. Snippet images are stored here, not in the LanceDB statement
        // rows: this database is encrypted when the app has a key (LanceDB is not), and
        // statement rows are appended on every edit, which would copy the bytes each
        // time. An image is content-addressed by the SHA-256 of its PNG bytes and referenced
        // from the Snippet statement as `shodh-blob:sha256:<hex>`. `result_extractions`
        // keeps the report of the last Result extraction per paper (what the comparison
        // coverage notes count); `result_rejections` remembers Results the user rejected,
        // by fingerprint, so a later extraction does not bring them back.
        "CREATE TABLE snippet_images (
            hash TEXT PRIMARY KEY CHECK (length(hash) = 64),
            png BLOB NOT NULL,
            width INTEGER NOT NULL CHECK (width > 0),
            height INTEGER NOT NULL CHECK (height > 0),
            created_at TEXT NOT NULL
        );
        CREATE TABLE result_extractions (
            file_path TEXT PRIMARY KEY,
            report_json TEXT NOT NULL,
            extracted_at TEXT NOT NULL
        );
        CREATE TABLE result_rejections (
            fingerprint TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            rejected_at TEXT NOT NULL
        );
        CREATE INDEX result_rejections_file ON result_rejections(file_path);",
    ),
    (
        6,
        // The citation graph. `scholarly_cache` keeps OpenAlex answers (and "not found")
        // per request until `expires_at`, so a rebuild sends nothing it already asked;
        // `citation_scans` keeps each library PDF's parsed identity and references, keyed
        // by the file's size and modification time, so a rebuild parses only changed
        // files; `citation_graph_state` keeps the last build report.
        "CREATE TABLE scholarly_cache (
            key TEXT PRIMARY KEY,
            status INTEGER NOT NULL,
            body BLOB NOT NULL,
            fetched_at TEXT NOT NULL,
            expires_at TEXT NOT NULL
        );
        CREATE TABLE citation_scans (
            file_path TEXT PRIMARY KEY,
            fingerprint TEXT NOT NULL,
            scan_json TEXT NOT NULL,
            scanned_at TEXT NOT NULL
        );
        CREATE TABLE citation_graph_state (
            key TEXT PRIMARY KEY,
            value_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );",
    ),
    (
        7,
        // The model picker's list of models (OpenRouter's public list, parsed), so the
        // picker opens instantly and shows prices offline. Refreshed after its TTL.
        "CREATE TABLE model_catalog_cache (
            source TEXT PRIMARY KEY,
            models_json TEXT NOT NULL,
            fetched_at_ms INTEGER NOT NULL CHECK (fetched_at_ms >= 0)
        );",
    ),
];

fn latest_schema_version() -> i64 {
    MIGRATIONS.last().map(|(v, _)| *v).unwrap_or(0)
}

/// `ts` format: RFC 3339, UTC, milliseconds, `Z`. Fixed width, so string
/// order is time order.
fn format_ts(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The chained hash of one row.
#[allow(clippy::too_many_arguments)]
fn row_hash(
    prev_hash: &str,
    id: i64,
    ts: &str,
    principal: &str,
    conversation_id: Option<&str>,
    profile_id: Option<&str>,
    run_id: Option<&str>,
    event_type: &str,
    payload_json: &str,
) -> String {
    let row = json!({
        "id": id,
        "ts": ts,
        "principal": principal,
        "conversation_id": conversation_id,
        "profile_id": profile_id,
        "run_id": run_id,
        "event_type": event_type,
        "payload_json": payload_json,
    });
    let mut hasher = Sha256::new();
    hasher.update(prev_hash.as_bytes());
    hasher.update(canonical_json(&row).as_bytes());
    hex::encode(hasher.finalize())
}

fn is_not_a_database(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(e, _) if e.code == ErrorCode::NotADatabase)
}

fn has_plaintext_header(path: &Path) -> AuditResult<bool> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let mut header = [0u8; 16];
    let mut read = 0;
    while read < header.len() {
        let n = file.read(&mut header[read..])?;
        if n == 0 {
            return Ok(false);
        }
        read += n;
    }
    Ok(&header == SQLITE_HEADER)
}

/// Open one connection, apply the key and connection settings. Returns the
/// connection and whether SQLCipher encryption is active.
fn open_connection(path: &Path, key: Option<&AuditKey>) -> AuditResult<(Connection, bool)> {
    if key.is_some() && has_plaintext_header(path)? {
        return Err(AuditError::PlaintextDatabase);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut encrypted = false;
    if let Some(key) = key {
        // Raw-key syntax: hex digits only, so the statement cannot be injected.
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", key.to_hex()))?;
        let version: Option<String> = conn
            .query_row("PRAGMA cipher_version", [], |r| r.get(0))
            .optional()?;
        if version.as_deref().is_none_or(|v| v.trim().is_empty()) {
            return Err(AuditError::EncryptionUnavailable);
        }
        encrypted = true;
    }
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })
    .map_err(|e| {
        if is_not_a_database(&e) {
            AuditError::Undecryptable
        } else {
            AuditError::Sqlite(e)
        }
    })?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        tracing::warn!(target: "shodh::audit", journal_mode = %mode, "audit database is not in WAL mode");
    }
    conn.execute_batch("PRAGMA synchronous = FULL;")?;
    Ok((conn, encrypted))
}

/// Apply pending migrations in one transaction. `IMMEDIATE` takes the write lock before the
/// version is read, so two components opening the database at once cannot both apply the
/// same migration.
fn migrate(conn: &mut Connection) -> AuditResult<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )?;
    let current: i64 = tx.query_row(
        "SELECT IFNULL(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    let supported = latest_schema_version();
    if current > supported {
        return Err(AuditError::SchemaTooNew {
            found: current,
            supported,
        });
    }
    for (version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v > current) {
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
            params![version, format_ts(Utc::now())],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Open `shodh.db` for a component other than the audit log (the statement store's
/// dynamics tables): the same key and connection settings, with every migration applied.
/// The connection is independent of the audit writer; SQLite's WAL and busy timeout
/// serialise the two writers.
pub fn open_shared_connection(path: &Path, key: Option<&AuditKey>) -> AuditResult<Connection> {
    let (mut conn, _) = open_connection(path, key)?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn read_retention(conn: &Connection) -> AuditResult<u32> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM audit_settings WHERE key = ?1",
            [RETENTION_KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|d| (MIN_RETENTION_DAYS..=MAX_RETENTION_DAYS).contains(d))
        .unwrap_or(DEFAULT_RETENTION_DAYS))
}

fn check_retention(days: u32) -> AuditResult<()> {
    if (MIN_RETENTION_DAYS..=MAX_RETENTION_DAYS).contains(&days) {
        Ok(())
    } else {
        Err(AuditError::InvalidRetention {
            min: MIN_RETENTION_DAYS,
            max: MAX_RETENTION_DAYS,
            got: days,
        })
    }
}

type Reply<T> = mpsc::Sender<AuditResult<T>>;

enum WriterMsg {
    Append(AuditRecord, Option<Reply<i64>>),
    Retention {
        days: u32,
        now: DateTime<Utc>,
        reply: Reply<u64>,
    },
    SetRetention {
        days: u32,
        reply: Reply<()>,
    },
}

/// The single writer: owns the write connection and the chain head.
struct Writer {
    conn: Connection,
    head_id: i64,
    head_hash: String,
}

impl Writer {
    fn new(conn: Connection) -> AuditResult<Self> {
        let head: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, hash FROM audit_events ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (head_id, head_hash) = head.unwrap_or((0, GENESIS_HASH.to_string()));
        Ok(Self {
            conn,
            head_id,
            head_hash,
        })
    }

    /// Insert `record` after (`head_id`, `head_hash`) inside `tx`. Returns the
    /// new head.
    fn insert(
        tx: &rusqlite::Transaction<'_>,
        head_id: i64,
        head_hash: &str,
        record: &AuditRecord,
        ts: DateTime<Utc>,
    ) -> AuditResult<(i64, String)> {
        let id = head_id + 1;
        let ts = format_ts(ts);
        let payload_json = canonical_json(&record.payload);
        let event_type = record.event_type.as_str();
        let hash = row_hash(
            head_hash,
            id,
            &ts,
            &record.principal,
            record.conversation_id.as_deref(),
            record.profile_id.as_deref(),
            record.run_id.as_deref(),
            event_type,
            &payload_json,
        );
        tx.execute(
            "INSERT INTO audit_events (id, ts, principal, conversation_id, profile_id, run_id, \
             event_type, payload_json, prev_hash, hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                ts,
                record.principal,
                record.conversation_id,
                record.profile_id,
                record.run_id,
                event_type,
                payload_json,
                head_hash,
                hash
            ],
        )?;
        Ok((id, hash))
    }

    fn append(&mut self, record: &AuditRecord) -> AuditResult<i64> {
        let tx = self.conn.transaction()?;
        let (id, hash) = Self::insert(&tx, self.head_id, &self.head_hash, record, Utc::now())?;
        tx.commit()?;
        self.head_id = id;
        self.head_hash = hash;
        Ok(id)
    }

    fn set_retention(&mut self, days: u32) -> AuditResult<()> {
        check_retention(days)?;
        let old = read_retention(&self.conn)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO audit_settings(key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![RETENTION_KEY, days.to_string()],
        )?;
        let record = AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({"setting": "audit_retention_days", "old": old, "new": days}),
        )
        .principal(LOCAL_OWNER);
        let (id, hash) = Self::insert(&tx, self.head_id, &self.head_hash, &record, Utc::now())?;
        tx.commit()?;
        self.head_id = id;
        self.head_hash = hash;
        Ok(())
    }

    /// Delete every event older than `days` before `now` (a prefix of the
    /// chain) and append a checkpoint, atomically. Returns rows deleted.
    fn retention(&mut self, days: u32, now: DateTime<Utc>) -> AuditResult<u64> {
        check_retention(days)?;
        let cutoff = now - chrono::Duration::days(i64::from(days));
        let cutoff_ts = format_ts(cutoff);
        let tx = self.conn.transaction()?;
        let last: Option<i64> = tx.query_row(
            "SELECT MAX(id) FROM audit_events WHERE ts < ?1",
            [&cutoff_ts],
            |r| r.get(0),
        )?;
        let Some(last_id) = last else {
            return Ok(0);
        };
        let last_hash: String = tx.query_row(
            "SELECT hash FROM audit_events WHERE id = ?1",
            [last_id],
            |r| r.get(0),
        )?;
        let deleted = tx.execute("DELETE FROM audit_events WHERE id <= ?1", [last_id])?;
        let record = AuditRecord::new(
            AuditEventType::RetentionCheckpoint,
            json!({
                "retention_days": days,
                "cutoff": cutoff_ts,
                "deleted": deleted,
                "last_deleted_id": last_id,
                "last_deleted_hash": last_hash,
            }),
        )
        .principal(SYSTEM_PRINCIPAL);
        let (id, hash) = Self::insert(&tx, self.head_id, &self.head_hash, &record, now)?;
        tx.commit()?;
        self.head_id = id;
        self.head_hash = hash;
        Ok(u64::try_from(deleted).unwrap_or(u64::MAX))
    }

    fn run(mut self, inbox: mpsc::Receiver<WriterMsg>) {
        for msg in inbox {
            match msg {
                WriterMsg::Append(record, reply) => {
                    let result = self.append(&record);
                    match reply {
                        Some(reply) => {
                            let _ = reply.send(result);
                        }
                        None => {
                            if let Err(e) = result {
                                tracing::error!(target: "shodh::audit", event_type = %record.event_type, error = %e, "audit event was not recorded");
                            }
                        }
                    }
                }
                WriterMsg::Retention { days, now, reply } => {
                    let _ = reply.send(self.retention(days, now));
                }
                WriterMsg::SetRetention { days, reply } => {
                    let _ = reply.send(self.set_retention(days));
                }
            }
        }
    }
}

/// Row ordering for reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    NewestFirst,
    OldestFirst,
}

/// SQL `WHERE` clause and parameters for a query.
fn filter_sql(q: &AuditQuery) -> (String, Vec<SqlValue>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<SqlValue> = Vec::new();
    if !q.types.is_empty() {
        let marks = vec!["?"; q.types.len()].join(",");
        clauses.push(format!("event_type IN ({marks})"));
        params.extend(
            q.types
                .iter()
                .map(|t| SqlValue::Text(t.as_str().to_string())),
        );
    }
    if let Some(from) = q.from {
        clauses.push("ts >= ?".to_string());
        params.push(SqlValue::Text(format_ts(from)));
    }
    if let Some(to) = q.to {
        clauses.push("ts <= ?".to_string());
        params.push(SqlValue::Text(format_ts(to)));
    }
    if let Some(conversation) = q
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        clauses.push("conversation_id = ?".to_string());
        params.push(SqlValue::Text(conversation.to_string()));
    }
    if let Some(tool) = q.tool.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        clauses.push("json_extract(payload_json, '$.tool') = ?".to_string());
        params.push(SqlValue::Text(tool.to_string()));
    }
    if let Some(text) = q.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        clauses.push(
            "(payload_json LIKE ? ESCAPE '\\' OR event_type LIKE ? ESCAPE '\\' \
             OR principal LIKE ? ESCAPE '\\')"
                .to_string(),
        );
        for _ in 0..3 {
            params.push(SqlValue::Text(pattern.clone()));
        }
    }
    let sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (sql, params)
}

const ROW_COLUMNS: &str = "id, ts, principal, conversation_id, profile_id, run_id, event_type, \
                           payload_json, prev_hash, hash";

/// A row exactly as stored.
struct RawRow {
    id: i64,
    ts: String,
    principal: String,
    conversation_id: Option<String>,
    profile_id: Option<String>,
    run_id: Option<String>,
    event_type: String,
    payload_json: String,
    prev_hash: String,
    hash: String,
}

impl RawRow {
    fn from_sql(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            ts: r.get(1)?,
            principal: r.get(2)?,
            conversation_id: r.get(3)?,
            profile_id: r.get(4)?,
            run_id: r.get(5)?,
            event_type: r.get(6)?,
            payload_json: r.get(7)?,
            prev_hash: r.get(8)?,
            hash: r.get(9)?,
        })
    }

    fn computed_hash(&self) -> String {
        row_hash(
            &self.prev_hash,
            self.id,
            &self.ts,
            &self.principal,
            self.conversation_id.as_deref(),
            self.profile_id.as_deref(),
            self.run_id.as_deref(),
            &self.event_type,
            &self.payload_json,
        )
    }

    fn into_row(self) -> AuditRow {
        let payload = serde_json::from_str(&self.payload_json)
            .unwrap_or_else(|_| Value::String(self.payload_json.clone()));
        AuditRow {
            id: self.id,
            ts: self.ts,
            principal: self.principal,
            conversation_id: self.conversation_id,
            profile_id: self.profile_id,
            run_id: self.run_id,
            event_type: self.event_type,
            payload,
            prev_hash: self.prev_hash,
            hash: self.hash,
        }
    }
}

/// The audit log: one writer thread, one read connection.
pub struct AuditLog {
    path: PathBuf,
    encrypted: bool,
    writer: Mutex<Option<mpsc::Sender<WriterMsg>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    reader: Mutex<Connection>,
}

impl std::fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLog")
            .field("path", &self.path)
            .field("encrypted", &self.encrypted)
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AuditLog {
    /// Open (or create) the log at `path`, run migrations and start the
    /// writer. With `key`, the database is opened with SQLCipher; a build
    /// without SQLCipher fails with [`AuditError::EncryptionUnavailable`]
    /// rather than silently writing plaintext.
    pub fn open(path: impl AsRef<Path>, key: Option<&AuditKey>) -> AuditResult<Self> {
        let path = path.as_ref().to_path_buf();
        let (mut write_conn, encrypted) = open_connection(&path, key)?;
        migrate(&mut write_conn)?;
        let (reader, _) = open_connection(&path, key)?;
        reader.execute_batch("PRAGMA query_only = 1;")?;
        let writer = Writer::new(write_conn)?;
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("shodh-audit-writer".to_string())
            .spawn(move || writer.run(rx))?;
        Ok(Self {
            path,
            encrypted,
            writer: Mutex::new(Some(tx)),
            thread: Mutex::new(Some(thread)),
            reader: Mutex::new(reader),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// SQLCipher encryption was confirmed when the database was opened.
    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    fn send(&self, msg: WriterMsg) -> AuditResult<()> {
        lock(&self.writer)
            .as_ref()
            .ok_or(AuditError::WriterStopped)?
            .send(msg)
            .map_err(|_| AuditError::WriterStopped)
    }

    fn request<T>(&self, make: impl FnOnce(Reply<T>) -> WriterMsg) -> AuditResult<T> {
        let (reply, response) = mpsc::channel();
        self.send(make(reply))?;
        response.recv().map_err(|_| AuditError::WriterStopped)?
    }

    /// Append and wait until the event is committed. Returns its id. Blocks:
    /// call from a blocking context.
    pub fn append(&self, record: AuditRecord) -> AuditResult<i64> {
        self.request(|reply| WriterMsg::Append(record, Some(reply)))
    }

    /// Queue an event without waiting. Write failures are logged by the
    /// writer. Safe on async and UI threads.
    pub fn submit(&self, record: AuditRecord) {
        let event_type = record.event_type;
        if let Err(e) = self.send(WriterMsg::Append(record, None)) {
            tracing::error!(target: "shodh::audit", %event_type, error = %e, "audit event was not queued");
        }
    }

    /// Delete events older than `days` and record a checkpoint. Returns the
    /// number of events deleted (no checkpoint when nothing was deleted).
    pub fn apply_retention(&self, days: u32) -> AuditResult<u64> {
        self.apply_retention_at(days, Utc::now())
    }

    /// [`apply_retention`](Self::apply_retention) as of `now`.
    pub fn apply_retention_at(&self, days: u32, now: DateTime<Utc>) -> AuditResult<u64> {
        self.request(|reply| WriterMsg::Retention { days, now, reply })
    }

    pub fn retention_days(&self) -> AuditResult<u32> {
        read_retention(&lock(&self.reader))
    }

    /// Persist the retention period; the change itself is audited.
    pub fn set_retention_days(&self, days: u32) -> AuditResult<()> {
        check_retention(days)?;
        self.request(|reply| WriterMsg::SetRetention { days, reply })
    }

    fn for_each_row(
        &self,
        q: &AuditQuery,
        order: Order,
        paginate: bool,
        mut f: impl FnMut(RawRow) -> AuditResult<()>,
    ) -> AuditResult<()> {
        let (filter, mut params) = filter_sql(q);
        let direction = match order {
            Order::NewestFirst => "DESC",
            Order::OldestFirst => "ASC",
        };
        let mut sql =
            format!("SELECT {ROW_COLUMNS} FROM audit_events{filter} ORDER BY id {direction}");
        if paginate {
            let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
            sql.push_str(" LIMIT ? OFFSET ?");
            params.push(SqlValue::Integer(i64::from(limit)));
            params.push(SqlValue::Integer(i64::from(q.offset)));
        } else if let Some(limit) = q.limit {
            sql.push_str(" LIMIT ?");
            params.push(SqlValue::Integer(i64::from(limit.max(1))));
        }
        let conn = lock(&self.reader);
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        while let Some(row) = rows.next()? {
            f(RawRow::from_sql(row)?)?;
        }
        Ok(())
    }

    /// Matching events, newest first, paginated by `limit`/`offset`.
    pub fn query(&self, q: &AuditQuery) -> AuditResult<Vec<AuditRow>> {
        let mut out = Vec::new();
        self.for_each_row(q, Order::NewestFirst, true, |row| {
            out.push(row.into_row());
            Ok(())
        })?;
        Ok(out)
    }

    /// Number of events matching `q` (ignores `limit`/`offset`).
    pub fn count(&self, q: &AuditQuery) -> AuditResult<u64> {
        let (filter, params) = filter_sql(q);
        let conn = lock(&self.reader);
        let n: i64 = conn.query_row(
            &format!("SELECT count(*) FROM audit_events{filter}"),
            params_from_iter(params),
            |r| r.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Recompute the whole chain.
    pub fn verify(&self) -> AuditResult<VerifyReport> {
        let conn = lock(&self.reader);
        let tx = conn.unchecked_transaction()?;
        // Retention anchors the first surviving row on the newest checkpoint.
        let anchor: Option<(i64, String)> = tx
            .query_row(
                "SELECT payload_json FROM audit_events WHERE event_type = ?1 \
                 ORDER BY id DESC LIMIT 1",
                [AuditEventType::RetentionCheckpoint.as_str()],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|payload| serde_json::from_str::<Value>(&payload).ok())
            .and_then(|p| {
                Some((
                    p.get("last_deleted_id")?.as_i64()?,
                    p.get("last_deleted_hash")?.as_str()?.to_string(),
                ))
            });

        let mut stmt = tx.prepare(&format!(
            "SELECT {ROW_COLUMNS} FROM audit_events ORDER BY id ASC"
        ))?;
        let mut rows = stmt.query([])?;
        let mut checked = 0u64;
        let mut previous: Option<(i64, String)> = None;
        let fail = |checked: u64, id: i64, reason: String| VerifyReport {
            ok: false,
            checked,
            first_bad_id: Some(id),
            reason: Some(reason),
        };
        while let Some(row) = rows.next()? {
            let row = RawRow::from_sql(row)?;
            checked += 1;
            match &previous {
                None => {
                    let from_start = row.id == 1 && row.prev_hash == GENESIS_HASH;
                    let from_checkpoint = anchor.as_ref().is_some_and(|(last_id, last_hash)| {
                        row.id == last_id + 1 && &row.prev_hash == last_hash
                    });
                    if !from_start && !from_checkpoint {
                        return Ok(fail(
                            checked,
                            row.id,
                            format!(
                                "Event #{} does not continue from the start of the log or from the last retention checkpoint",
                                row.id
                            ),
                        ));
                    }
                }
                Some((prev_id, prev_hash)) => {
                    if row.id != prev_id + 1 {
                        let missing = if row.id == prev_id + 2 {
                            format!("#{}", prev_id + 1)
                        } else {
                            format!("#{}–#{}", prev_id + 1, row.id - 1)
                        };
                        return Ok(fail(
                            checked,
                            row.id,
                            format!("Event {missing} is missing before event #{}", row.id),
                        ));
                    }
                    if &row.prev_hash != prev_hash {
                        return Ok(fail(
                            checked,
                            row.id,
                            format!("Event #{} does not link to event #{prev_id}", row.id),
                        ));
                    }
                }
            }
            if row.computed_hash() != row.hash {
                return Ok(fail(
                    checked,
                    row.id,
                    format!("Event #{} was changed after it was written", row.id),
                ));
            }
            previous = Some((row.id, row.hash));
        }
        Ok(VerifyReport {
            ok: true,
            checked,
            first_bad_id: None,
            reason: None,
        })
    }

    /// Write matching events (oldest first, all pages) as JSON Lines. The file
    /// is replaced atomically. Returns the number of events written.
    pub fn export_jsonl(&self, path: &Path, q: &AuditQuery) -> AuditResult<u64> {
        self.export_with(path, |out| {
            let mut count = 0u64;
            self.for_each_row(q, Order::OldestFirst, false, |row| {
                serde_json::to_writer(&mut *out, &row.into_row())?;
                out.write_all(b"\n")?;
                count += 1;
                Ok(())
            })?;
            Ok(count)
        })
    }

    /// Write matching events (oldest first, all pages) as CSV with the raw
    /// stored columns, so the export can be re-verified. Returns the count.
    pub fn export_csv(&self, path: &Path, q: &AuditQuery) -> AuditResult<u64> {
        self.export_with(path, |out| {
            let mut writer = csv::Writer::from_writer(out);
            writer.write_record([
                "id",
                "ts",
                "principal",
                "conversation_id",
                "profile_id",
                "run_id",
                "event_type",
                "payload_json",
                "prev_hash",
                "hash",
            ])?;
            let mut count = 0u64;
            self.for_each_row(q, Order::OldestFirst, false, |row| {
                writer.write_record([
                    row.id.to_string().as_str(),
                    &row.ts,
                    &row.principal,
                    row.conversation_id.as_deref().unwrap_or(""),
                    row.profile_id.as_deref().unwrap_or(""),
                    row.run_id.as_deref().unwrap_or(""),
                    &row.event_type,
                    &row.payload_json,
                    &row.prev_hash,
                    &row.hash,
                ])?;
                count += 1;
                Ok(())
            })?;
            writer.flush()?;
            Ok(count)
        })
    }

    fn export_with(
        &self,
        path: &Path,
        write: impl FnOnce(&mut BufWriter<File>) -> AuditResult<u64>,
    ) -> AuditResult<u64> {
        let mut partial = path.as_os_str().to_owned();
        partial.push(".partial");
        let partial = PathBuf::from(partial);
        let result = (|| {
            let mut out = BufWriter::new(File::create(&partial)?);
            let count = write(&mut out)?;
            let file = out
                .into_inner()
                .map_err(|e| AuditError::Io(e.into_error()))?;
            file.sync_all()?;
            Ok(count)
        })();
        match result {
            Ok(count) => {
                std::fs::rename(&partial, path)?;
                Ok(count)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&partial);
                Err(e)
            }
        }
    }

    /// Counts for the Usage & Audit page. `month_start`/`month_end` bound the
    /// current calendar month (the caller picks the time zone).
    pub fn stats(
        &self,
        month_start: DateTime<Utc>,
        month_end: DateTime<Utc>,
    ) -> AuditResult<AuditStats> {
        let retention_days = self.retention_days()?;
        let conn = lock(&self.reader);
        let tx = conn.unchecked_transaction()?;
        let mut by_type = BTreeMap::new();
        let mut total = 0u64;
        {
            let mut stmt =
                tx.prepare("SELECT event_type, count(*) FROM audit_events GROUP BY event_type")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let n = u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0);
                total += n;
                by_type.insert(row.get::<_, String>(0)?, n);
            }
        }
        let (oldest, newest): (Option<String>, Option<String>) =
            tx.query_row("SELECT MIN(ts), MAX(ts) FROM audit_events", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        let start = format_ts(month_start);
        let end = format_ts(month_end);
        let count_type = |t: AuditEventType| -> AuditResult<u64> {
            let n: i64 = tx.query_row(
                "SELECT count(*) FROM audit_events WHERE event_type = ?1 AND ts >= ?2 AND ts < ?3",
                params![t.as_str(), start, end],
                |r| r.get(0),
            )?;
            Ok(u64::try_from(n).unwrap_or(0))
        };
        let questions = count_type(AuditEventType::Question)?;
        let tool_calls = count_type(AuditEventType::ToolCall)?;
        let approvals = count_type(AuditEventType::Approval)?;
        let (input, output, cost): (i64, i64, f64) = tx.query_row(
            "SELECT IFNULL(SUM(json_extract(payload_json, '$.tokens_in')), 0), \
                    IFNULL(SUM(json_extract(payload_json, '$.tokens_out')), 0), \
                    IFNULL(SUM(json_extract(payload_json, '$.cost_usd')), 0.0) \
             FROM audit_events WHERE event_type = ?1 AND ts >= ?2 AND ts < ?3 \
               AND json_extract(payload_json, '$.cloud') = 1",
            params![AuditEventType::Answer.as_str(), start, end],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(AuditStats {
            total,
            by_type,
            oldest,
            newest,
            month: MonthStats {
                questions,
                tool_calls,
                approvals,
                cloud_input_tokens: u64::try_from(input).unwrap_or(0),
                cloud_output_tokens: u64::try_from(output).unwrap_or(0),
                cloud_cost_usd: cost,
            },
            retention_days,
            encrypted: self.encrypted,
        })
    }
}

impl Drop for AuditLog {
    /// Close the queue and wait for queued events to be committed.
    fn drop(&mut self) {
        lock(&self.writer).take();
        if let Some(thread) = lock(&self.thread).take() {
            if thread.join().is_err() {
                tracing::error!(target: "shodh::audit", "audit writer thread panicked");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::payload;
    use crate::llm::{ApiProvider, LLMMode};

    fn temp_log() -> (tempfile::TempDir, AuditLog) {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::open(dir.path().join("shodh.db"), None).unwrap();
        (dir, log)
    }

    fn raw(dir: &tempfile::TempDir) -> Connection {
        Connection::open(dir.path().join("shodh.db")).unwrap()
    }

    fn question(n: usize) -> AuditRecord {
        AuditRecord::new(
            AuditEventType::Question,
            json!({"text": format!("question {n}"), "model": "anthropic/claude"}),
        )
        .conversation(if n.is_multiple_of(2) {
            "conv-even"
        } else {
            "conv-odd"
        })
        .profile("assistant")
        .run(format!("run-{n}"))
    }

    fn fill(log: &AuditLog, n: usize) {
        for i in 1..=n {
            assert_eq!(log.append(question(i)).unwrap(), i as i64);
        }
    }

    #[test]
    fn appended_events_form_a_verifiable_chain() {
        let (_dir, log) = temp_log();
        fill(&log, 5);
        let report = log.verify().unwrap();
        assert!(report.ok, "{report:?}");
        assert_eq!(report.checked, 5);
        let rows = log.query(&AuditQuery::default()).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].id, 5, "newest first");
        assert_eq!(rows[4].prev_hash, GENESIS_HASH);
        for pair in rows.windows(2) {
            assert_eq!(pair[1].hash, pair[0].prev_hash);
        }
        assert_eq!(rows[0].payload["text"], "question 5");
        assert_eq!(rows[0].principal, LOCAL_OWNER);
    }

    #[test]
    fn submitted_events_are_committed_before_drop_returns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        {
            let log = AuditLog::open(&path, None).unwrap();
            for i in 1..=3 {
                log.submit(question(i));
            }
        }
        let log = AuditLog::open(&path, None).unwrap();
        assert_eq!(log.count(&AuditQuery::default()).unwrap(), 3);
        // The chain continues across reopen.
        assert_eq!(log.append(question(4)).unwrap(), 4);
        assert!(log.verify().unwrap().ok);
    }

    #[test]
    fn modified_row_is_detected_at_that_row() {
        let (dir, log) = temp_log();
        fill(&log, 5);
        raw(&dir)
            .execute(
                "UPDATE audit_events SET payload_json = '{\"text\":\"edited\"}' WHERE id = 3",
                [],
            )
            .unwrap();
        let report = log.verify().unwrap();
        assert!(!report.ok);
        assert_eq!(report.first_bad_id, Some(3));
        assert!(report.reason.unwrap().contains("changed"));
    }

    #[test]
    fn modified_row_with_recomputed_hash_breaks_the_next_link() {
        let (dir, log) = temp_log();
        fill(&log, 5);
        let conn = raw(&dir);
        let mut row = conn
            .query_row(
                &format!("SELECT {ROW_COLUMNS} FROM audit_events WHERE id = 3"),
                [],
                RawRow::from_sql,
            )
            .unwrap();
        row.principal = "intruder".into();
        let forged = row.computed_hash();
        conn.execute(
            "UPDATE audit_events SET principal = 'intruder', hash = ?1 WHERE id = 3",
            [&forged],
        )
        .unwrap();
        let report = log.verify().unwrap();
        assert_eq!(report.first_bad_id, Some(4));
        assert!(report.reason.unwrap().contains("does not link"));
    }

    #[test]
    fn deleted_row_reports_the_first_surviving_row_after_the_gap() {
        let (dir, log) = temp_log();
        fill(&log, 5);
        raw(&dir)
            .execute("DELETE FROM audit_events WHERE id = 3", [])
            .unwrap();
        let report = log.verify().unwrap();
        assert!(!report.ok);
        assert_eq!(report.first_bad_id, Some(4));
        assert!(report.reason.unwrap().contains("#3 is missing"));
    }

    #[test]
    fn deleted_first_row_without_checkpoint_is_detected() {
        let (dir, log) = temp_log();
        fill(&log, 3);
        raw(&dir)
            .execute("DELETE FROM audit_events WHERE id = 1", [])
            .unwrap();
        assert_eq!(log.verify().unwrap().first_bad_id, Some(2));
    }

    #[test]
    fn reordered_rows_are_detected() {
        let (dir, log) = temp_log();
        fill(&log, 5);
        let conn = raw(&dir);
        // Swap rows 2 and 3 by exchanging their ids.
        conn.execute_batch(
            "UPDATE audit_events SET id = -2 WHERE id = 2;
             UPDATE audit_events SET id = 2 WHERE id = 3;
             UPDATE audit_events SET id = 3 WHERE id = -2;",
        )
        .unwrap();
        let report = log.verify().unwrap();
        assert!(!report.ok);
        assert_eq!(report.first_bad_id, Some(2));
    }

    #[test]
    fn retention_checkpoint_keeps_the_chain_verifiable() {
        let (_dir, log) = temp_log();
        fill(&log, 4);
        // Nothing is older than 30 days yet: no rows deleted, no checkpoint.
        assert_eq!(log.apply_retention(30).unwrap(), 0);
        assert_eq!(log.count(&AuditQuery::default()).unwrap(), 4);

        // Forty days on, every event is older than the 30-day window.
        let later = Utc::now() + chrono::Duration::days(40);
        assert_eq!(log.apply_retention_at(30, later).unwrap(), 4);
        let rows = log.query(&AuditQuery::default()).unwrap();
        assert_eq!(rows.len(), 1);
        let checkpoint = &rows[0];
        assert_eq!(checkpoint.event_type, "retention_checkpoint");
        assert_eq!(checkpoint.id, 5);
        assert_eq!(checkpoint.payload["last_deleted_id"], 4);
        assert_eq!(checkpoint.payload["deleted"], 4);
        assert_eq!(
            checkpoint.payload["last_deleted_hash"].as_str(),
            Some(checkpoint.prev_hash.as_str())
        );
        assert_eq!(checkpoint.principal, SYSTEM_PRINCIPAL);

        fill_more(&log, 2);
        let report = log.verify().unwrap();
        assert!(report.ok, "{report:?}");
        assert_eq!(report.checked, 3);
    }

    fn fill_more(log: &AuditLog, n: usize) {
        for i in 0..n {
            log.append(question(100 + i)).unwrap();
        }
    }

    #[test]
    fn deleting_the_newest_checkpoint_unanchors_the_chain() {
        let (dir, log) = temp_log();
        fill(&log, 3);
        let later = Utc::now() + chrono::Duration::days(40);
        log.apply_retention_at(30, later).unwrap();
        fill_more(&log, 2);
        assert!(log.verify().unwrap().ok);
        // Deleting the checkpoint leaves the first row without an anchor.
        raw(&dir)
            .execute(
                "DELETE FROM audit_events WHERE event_type = 'retention_checkpoint'",
                [],
            )
            .unwrap();
        assert_eq!(log.verify().unwrap().first_bad_id, Some(5));
    }

    #[test]
    fn retention_bounds_are_enforced_and_changes_are_audited() {
        let (_dir, log) = temp_log();
        assert_eq!(log.retention_days().unwrap(), DEFAULT_RETENTION_DAYS);
        assert!(matches!(
            log.set_retention_days(0),
            Err(AuditError::InvalidRetention { .. })
        ));
        assert!(log.apply_retention(MAX_RETENTION_DAYS + 1).is_err());
        log.set_retention_days(90).unwrap();
        assert_eq!(log.retention_days().unwrap(), 90);
        let rows = log
            .query(&AuditQuery {
                types: vec![AuditEventType::SettingsChange],
                ..AuditQuery::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].payload["old"], DEFAULT_RETENTION_DAYS);
        assert_eq!(rows[0].payload["new"], 90);
        assert!(log.verify().unwrap().ok);
    }

    #[test]
    fn query_filters_by_type_time_conversation_and_text() {
        let (_dir, log) = temp_log();
        fill(&log, 6);
        log.append(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({"action": "model_switch", "provider": "anthropic"}),
        ))
        .unwrap();

        let by_type = log
            .query(&AuditQuery {
                types: vec![AuditEventType::SettingsChange],
                ..AuditQuery::default()
            })
            .unwrap();
        assert_eq!(by_type.len(), 1);

        let by_conversation = AuditQuery {
            conversation_id: Some("conv-even".into()),
            ..AuditQuery::default()
        };
        assert_eq!(log.query(&by_conversation).unwrap().len(), 3);
        assert_eq!(log.count(&by_conversation).unwrap(), 3);

        let text = log
            .query(&AuditQuery {
                text: Some("QUESTION 4".into()),
                ..AuditQuery::default()
            })
            .unwrap();
        assert_eq!(text.len(), 1);
        assert_eq!(text[0].id, 4);

        // LIKE wildcards in the search text are literal.
        let literal = AuditQuery {
            text: Some("%".into()),
            ..AuditQuery::default()
        };
        assert_eq!(log.count(&literal).unwrap(), 0);

        let future = AuditQuery {
            from: Some(Utc::now() + chrono::Duration::hours(1)),
            ..AuditQuery::default()
        };
        assert_eq!(log.count(&future).unwrap(), 0);
        let past = AuditQuery {
            to: Some(Utc::now() - chrono::Duration::hours(1)),
            ..AuditQuery::default()
        };
        assert_eq!(log.count(&past).unwrap(), 0);
        let window = AuditQuery {
            from: Some(Utc::now() - chrono::Duration::hours(1)),
            to: Some(Utc::now() + chrono::Duration::hours(1)),
            ..AuditQuery::default()
        };
        assert_eq!(log.count(&window).unwrap(), 7);

        let page = log
            .query(&AuditQuery {
                limit: Some(2),
                offset: 2,
                ..AuditQuery::default()
            })
            .unwrap();
        assert_eq!(page.iter().map(|r| r.id).collect::<Vec<_>>(), vec![5, 4]);

        log.append(AuditRecord::new(
            AuditEventType::ToolCall,
            json!({"tool": "web_search", "args": {"query": "list_tasks"}}),
        ))
        .unwrap();
        log.append(AuditRecord::new(
            AuditEventType::ToolCall,
            json!({"tool": "list_tasks", "args": {}}),
        ))
        .unwrap();
        let by_tool = AuditQuery {
            tool: Some("list_tasks".into()),
            ..AuditQuery::default()
        };
        let rows = log.query(&by_tool).unwrap();
        assert_eq!(rows.len(), 1, "matches the tool, not a mention in the args");
        assert_eq!(rows[0].payload["tool"], "list_tasks");
    }

    #[test]
    fn exports_contain_every_matching_row_oldest_first() {
        let (dir, log) = temp_log();
        fill(&log, 3);
        let query = AuditQuery {
            conversation_id: Some("conv-odd".into()),
            ..AuditQuery::default()
        };

        let jsonl = dir.path().join("audit.jsonl");
        assert_eq!(log.export_jsonl(&jsonl, &query).unwrap(), 2);
        let text = std::fs::read_to_string(&jsonl).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[1]["id"], 3);
        assert_eq!(lines[0]["eventType"], "question");
        assert_eq!(lines[0]["payload"]["text"], "question 1");
        assert!(lines[0]["hash"].as_str().is_some_and(|h| h.len() == 64));
        assert!(!dir.path().join("audit.jsonl.partial").exists());

        let csv_path = dir.path().join("audit.csv");
        assert_eq!(
            log.export_csv(&csv_path, &AuditQuery::default()).unwrap(),
            3
        );
        let mut reader = csv::Reader::from_path(&csv_path).unwrap();
        let headers = reader.headers().unwrap().clone();
        assert_eq!(&headers[0], "id");
        assert_eq!(&headers[7], "payload_json");
        let records: Vec<csv::StringRecord> = reader.records().map(|r| r.unwrap()).collect();
        assert_eq!(records.len(), 3);
        // The exported columns re-verify: hash = H(prev_hash || canonical(row)).
        for record in &records {
            let recomputed = row_hash(
                &record[8],
                record[0].parse().unwrap(),
                &record[1],
                &record[2],
                Some(&record[3]).filter(|s| !s.is_empty()),
                Some(&record[4]).filter(|s| !s.is_empty()),
                Some(&record[5]).filter(|s| !s.is_empty()),
                &record[6],
                &record[7],
            );
            assert_eq!(recomputed, &record[9]);
        }
    }

    #[test]
    fn stats_count_month_usage_and_cloud_cost() {
        let (_dir, log) = temp_log();
        fill(&log, 2);
        log.append(AuditRecord::new(
            AuditEventType::Answer,
            json!({"cloud": true, "tokens_in": 1200, "tokens_out": 300, "cost_usd": 0.25}),
        ))
        .unwrap();
        log.append(AuditRecord::new(
            AuditEventType::Answer,
            json!({"cloud": false, "tokens_in": 999, "tokens_out": 999, "cost_usd": 9.0}),
        ))
        .unwrap();
        let start = Utc::now() - chrono::Duration::days(1);
        let end = Utc::now() + chrono::Duration::days(1);
        let stats = log.stats(start, end).unwrap();
        assert_eq!(stats.total, 4);
        assert_eq!(stats.by_type.get("question"), Some(&2));
        assert_eq!(stats.month.questions, 2);
        assert_eq!(stats.month.cloud_input_tokens, 1200);
        assert_eq!(stats.month.cloud_output_tokens, 300);
        assert!((stats.month.cloud_cost_usd - 0.25).abs() < 1e-9);
        assert!(!stats.encrypted);
        assert_eq!(stats.retention_days, DEFAULT_RETENTION_DAYS);
        assert!(stats.oldest.is_some() && stats.newest.is_some());
    }

    #[test]
    fn migrations_are_recorded_and_idempotent() {
        let (dir, log) = temp_log();
        drop(log);
        let log = AuditLog::open(dir.path().join("shodh.db"), None).unwrap();
        let versions: i64 = raw(&dir)
            .query_row("SELECT count(*) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(versions, latest_schema_version());
        assert!(log.verify().unwrap().ok);
    }

    #[test]
    fn a_version_1_database_migrates_to_statement_tables_with_its_chain_intact() {
        let (dir, log) = temp_log();
        fill(&log, 3);
        drop(log);
        // Reduce the database to exactly what a version-1 app left behind.
        raw(&dir)
            .execute_batch(
                "DROP TABLE statement_dynamics;
                 DROP TABLE statement_links;
                 DROP TABLE generated_visuals_fts;
                 DROP TABLE generated_visuals;
                 DROP TABLE visual_settings;
                 DROP TABLE memory_proposals;
                 DROP TABLE memory_learn_usage;
                 DROP TABLE memory_learn_state;
                 DROP TABLE snippet_images;
                 DROP TABLE result_extractions;
                 DROP TABLE result_rejections;
                 DROP TABLE scholarly_cache;
                 DROP TABLE citation_scans;
                 DROP TABLE citation_graph_state;
                 DELETE FROM schema_version WHERE version > 1;",
            )
            .unwrap();
        let path = dir.path().join("shodh.db");
        // The statement store may open the database before the audit log does.
        let shared = open_shared_connection(&path, None).unwrap();
        let version: i64 = shared
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, latest_schema_version());
        let tables: i64 = shared
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'                  AND name IN ('statement_dynamics', 'statement_links')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2);
        let learning: i64 = shared
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'
                 AND name IN ('memory_proposals', 'memory_learn_usage', 'memory_learn_state')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(learning, 3);
        let research: i64 = shared
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'
                 AND name IN ('snippet_images', 'result_extractions', 'result_rejections')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(research, 3);
        let citations: i64 = shared
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'
                 AND name IN ('scholarly_cache', 'citation_scans', 'citation_graph_state')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(citations, 3);
        // Opening again (the audit log after the statement store) applies nothing twice.
        let log = AuditLog::open(&path, None).unwrap();
        let versions: i64 = raw(&dir)
            .query_row("SELECT count(*) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(versions, latest_schema_version());
        let report = log.verify().unwrap();
        assert!(report.ok, "{report:?}");
        assert_eq!(report.checked, 3);
        // Links are stored once per unordered pair.
        let reversed = shared.execute(
            "INSERT INTO statement_links(from_id, to_id, weight, updated_at) VALUES ('b', 'a', 0.5, 'x')",
            [],
        );
        assert!(reversed.is_err());
    }

    #[test]
    fn a_key_without_sqlcipher_is_refused_not_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let result = AuditLog::open(dir.path().join("shodh.db"), Some(&AuditKey::generate()));
        if crate::audit::encryption_compiled() {
            assert!(result.unwrap().is_encrypted());
        } else {
            assert!(matches!(result, Err(AuditError::EncryptionUnavailable)));
        }
    }

    #[test]
    fn settings_change_payload_never_contains_the_key() {
        let (dir, log) = temp_log();
        const SENTINEL: &str = "sk-ant-SENTINEL-0123456789abcdef";
        let mode = LLMMode::External {
            provider: ApiProvider::Anthropic,
            api_key: SENTINEL.to_string(),
            model: "claude-x".to_string(),
        };
        log.append(AuditRecord::new(
            AuditEventType::SettingsChange,
            payload::model_switch(&mode),
        ))
        .unwrap();
        log.append(AuditRecord::new(
            AuditEventType::SettingsChange,
            payload::api_key_change("anthropic", payload::KeyAction::Set),
        ))
        .unwrap();
        // The main file and the WAL hold every byte written so far.
        let mut dump = Vec::new();
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            dump.extend(std::fs::read(entry.unwrap().path()).unwrap());
        }
        let rows = log.query(&AuditQuery::default()).unwrap();
        let serialised = serde_json::to_string(&rows).unwrap();
        assert!(!serialised.contains(SENTINEL));
        assert!(!String::from_utf8_lossy(&dump).contains(SENTINEL));
        assert_eq!(rows[1].payload["provider"], "anthropic");
        assert_eq!(rows[1].payload["model"], "claude-x");
        assert_eq!(rows[0].payload["action"], "api_key_set");
    }
}
