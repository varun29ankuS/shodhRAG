//! The code map's SQLite file: one per project, in Shodh's data folder.
//!
//! It holds per-file facts only (each file's symbols, references, imports
//! and `mod` declarations, keyed by the file's content hash), never anything
//! resolved across files. Changing or deleting a file therefore replaces or
//! drops that file's rows and nothing else can go stale. The project's
//! module graph is computed from these rows when the map is asked
//! ([`super::model`]).

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::extract::{
    FileFacts, Import, ImportKind, ModDecl, RefKind, Reference, Symbol, SymbolKind,
};

/// Bumped whenever extraction changes what it records: a map written by
/// another version is emptied and built again.
pub const EXTRACTOR_VERSION: &str = "1";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    hash TEXT NOT NULL,
    lines INTEGER NOT NULL,
    skipped TEXT,
    package TEXT,
    had_errors INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS configs (file_id INTEGER PRIMARY KEY, text TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS symbols (
    file_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    lname TEXT NOT NULL,
    kind TEXT NOT NULL,
    keyword TEXT NOT NULL,
    line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    container TEXT,
    scope TEXT NOT NULL,
    exported INTEGER NOT NULL,
    signature TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS symbols_lname ON symbols(lname);
CREATE INDEX IF NOT EXISTS symbols_file ON symbols(file_id);
CREATE TABLE IF NOT EXISTS refs (
    file_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    qualifier TEXT,
    line INTEGER NOT NULL,
    kind TEXT NOT NULL,
    caller TEXT,
    scope TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS refs_name ON refs(name);
CREATE INDEX IF NOT EXISTS refs_file ON refs(file_id);
CREATE TABLE IF NOT EXISTS imports (
    file_id INTEGER NOT NULL,
    spec TEXT NOT NULL,
    name TEXT,
    alias TEXT,
    line INTEGER NOT NULL,
    kind TEXT NOT NULL,
    public INTEGER NOT NULL,
    scope TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS imports_file ON imports(file_id);
CREATE TABLE IF NOT EXISTS mods (
    file_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    line INTEGER NOT NULL,
    path_attr TEXT,
    scope TEXT NOT NULL,
    inline INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS mods_file ON mods(file_id);
";

/// What the map knew of a file when it last read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub id: i64,
    pub size: i64,
    pub mtime: i64,
    pub hash: String,
}

/// A file to write: its stamp and what was read from it.
#[derive(Debug, Clone)]
pub struct FileRecord<'a> {
    pub path: &'a str,
    /// A language name, or `config:<kind>`.
    pub kind: &'a str,
    pub size: i64,
    pub mtime: i64,
    pub hash: &'a str,
    pub lines: u32,
    /// Why its contents were not read (too large, unreadable).
    pub skipped: Option<&'a str>,
}

/// A file as the model reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub id: i64,
    pub path: String,
    pub kind: String,
    pub lines: u32,
    pub skipped: Option<String>,
    pub package: Option<String>,
}

/// A symbol with the file it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRow {
    pub file_id: i64,
    pub path: String,
    pub symbol: Symbol,
}

/// A reference with the file it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRow {
    pub file_id: i64,
    pub path: String,
    pub reference: Reference,
}

pub struct Store {
    conn: Connection,
}

fn sql_err(e: rusqlite::Error) -> String {
    format!("the code map's database: {e}")
}

const CLEAR: &str = "DELETE FROM symbols; DELETE FROM refs; DELETE FROM imports; DELETE FROM mods;
     DELETE FROM configs; DELETE FROM files;
     DELETE FROM meta WHERE key <> 'extractor' AND key <> 'root';";

/// A file or folder filter for SQL: (the path itself, `LIKE` pattern for
/// what is below it), both `None` for no filter.
fn path_filter(path: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(path) = path
        .map(|p| p.trim().trim_start_matches("./").trim_end_matches('/'))
        .filter(|p| !p.is_empty() && *p != ".")
    else {
        return (None, None);
    };
    // `!` escapes `LIKE`'s wildcards (and itself) in the folder's name.
    let mut escaped = String::with_capacity(path.len() + 4);
    for c in path.chars() {
        if matches!(c, '%' | '_' | '!') {
            escaped.push('!');
        }
        escaped.push(c);
    }
    (Some(path.to_string()), Some(format!("{escaped}/%")))
}

/// A file SQLite cannot read as a database.
fn is_corrupt(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
    )
}

/// A stored keyword back as the `'static` one extraction uses.
fn keyword_of(text: &str) -> &'static str {
    const KEYWORDS: [&str; 16] = [
        "fn",
        "struct",
        "union",
        "enum",
        "trait",
        "mod",
        "const",
        "static",
        "type",
        "macro_rules!",
        "function",
        "class",
        "interface",
        "namespace",
        "def",
        "func",
    ];
    const MORE: [&str; 2] = ["let", "var"];
    KEYWORDS
        .into_iter()
        .chain(MORE)
        .find(|k| *k == text)
        .unwrap_or("")
}

fn symbol_from_row(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Symbol> {
    let kind: String = row.get(at + 1)?;
    let keyword: String = row.get(at + 2)?;
    Ok(Symbol {
        name: row.get(at)?,
        kind: SymbolKind::parse(&kind).unwrap_or(SymbolKind::Function),
        keyword: keyword_of(&keyword),
        line: row.get(at + 3)?,
        end_line: row.get(at + 4)?,
        container: row.get(at + 5)?,
        scope: row.get(at + 6)?,
        exported: row.get::<_, i64>(at + 7)? != 0,
        signature: row.get(at + 8)?,
    })
}

const SYMBOL_COLUMNS: &str =
    "s.name, s.kind, s.keyword, s.line, s.end_line, s.container, s.scope, s.exported, s.signature";

impl Store {
    /// Open (or create) the map at `path`. A map of another extractor
    /// version is emptied. A file SQLite cannot read as a database (cut off
    /// mid-write, overwritten) is moved aside and a new map started, so the
    /// map can always be built again.
    pub fn open(path: &Path) -> Result<Store, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("the code map's folder could not be made: {e}"))?;
        }
        match Self::open_at(path) {
            Ok(store) => Ok(store),
            Err(e) if is_corrupt(&e) => {
                let aside = path.with_extension(format!(
                    "corrupt-{}.db",
                    chrono::Utc::now().format("%Y%m%d%H%M%S")
                ));
                std::fs::rename(path, &aside)
                    .map_err(|e| format!("the damaged code map could not be moved aside ({e})"))?;
                for side in ["-wal", "-shm"] {
                    let mut name = path.as_os_str().to_os_string();
                    name.push(side);
                    let _ = std::fs::remove_file(std::path::PathBuf::from(name));
                }
                tracing::warn!(target: "shodh::code_map", aside = %aside.display(), "a damaged code map was moved aside; it is built again");
                Self::open_at(path).map_err(sql_err)
            }
            Err(e) => Err(sql_err(e)),
        }
    }

    fn open_at(path: &Path) -> Result<Store, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY;")?;
        conn.execute_batch(SCHEMA)?;
        let store = Store { conn };
        let version: Option<String> = store
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'extractor'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if version.as_deref() != Some(EXTRACTOR_VERSION) {
            store.conn.execute_batch(CLEAR)?;
            store.conn.execute(
                "INSERT INTO meta (key, value) VALUES ('extractor', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [EXTRACTOR_VERSION],
            )?;
        }
        Ok(store)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .map_err(sql_err)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(sql_err)
    }

    /// Everything but the extractor version: the map is built again.
    pub fn clear(&self) -> Result<(), String> {
        self.conn.execute_batch(CLEAR).map_err(sql_err)
    }

    /// Give the space of what was deleted back to the disk.
    pub fn vacuum(&self) -> Result<(), String> {
        self.conn.execute_batch("VACUUM").map_err(sql_err)
    }

    /// What the map knew of the file at `path` when it last read it.
    pub fn stamp(&self, path: &str) -> Result<Option<Stamp>, String> {
        self.conn
            .query_row(
                "SELECT id, size, mtime, hash FROM files WHERE path = ?1",
                [path],
                |r| {
                    Ok(Stamp {
                        id: r.get(0)?,
                        size: r.get(1)?,
                        mtime: r.get(2)?,
                        hash: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(sql_err)
    }

    pub fn stamps(&self) -> Result<HashMap<String, Stamp>, String> {
        let mut statement = self
            .conn
            .prepare("SELECT id, path, size, mtime, hash FROM files")
            .map_err(sql_err)?;
        let rows = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    Stamp {
                        id: r.get(0)?,
                        size: r.get(2)?,
                        mtime: r.get(3)?,
                        hash: r.get(4)?,
                    },
                ))
            })
            .map_err(sql_err)?;
        rows.collect::<Result<HashMap<_, _>, _>>().map_err(sql_err)
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>, String> {
        self.conn.transaction().map_err(sql_err)
    }

    fn delete_rows(tx: &Transaction<'_>, id: i64) -> Result<(), String> {
        for table in ["symbols", "refs", "imports", "mods"] {
            tx.execute(&format!("DELETE FROM {table} WHERE file_id = ?1"), [id])
                .map_err(sql_err)?;
        }
        tx.execute("DELETE FROM configs WHERE file_id = ?1", [id])
            .map_err(sql_err)?;
        Ok(())
    }

    /// Replace what the map holds of `file` with `facts` (a source file) or
    /// `config` (a config file's text).
    pub fn put_file(
        tx: &Transaction<'_>,
        file: &FileRecord<'_>,
        facts: Option<&FileFacts>,
        config: Option<&str>,
    ) -> Result<(), String> {
        let existing: Option<i64> = tx
            .query_row("SELECT id FROM files WHERE path = ?1", [file.path], |r| {
                r.get(0)
            })
            .optional()
            .map_err(sql_err)?;
        let package = facts.and_then(|f| f.package.as_deref());
        let had_errors = facts.is_some_and(|f| f.had_errors);
        let id = match existing {
            Some(id) => {
                Self::delete_rows(tx, id)?;
                tx.execute(
                    "UPDATE files SET kind = ?2, size = ?3, mtime = ?4, hash = ?5, lines = ?6,
                     skipped = ?7, package = ?8, had_errors = ?9 WHERE id = ?1",
                    params![
                        id,
                        file.kind,
                        file.size,
                        file.mtime,
                        file.hash,
                        file.lines,
                        file.skipped,
                        package,
                        had_errors
                    ],
                )
                .map_err(sql_err)?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO files (path, kind, size, mtime, hash, lines, skipped, package, had_errors)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        file.path,
                        file.kind,
                        file.size,
                        file.mtime,
                        file.hash,
                        file.lines,
                        file.skipped,
                        package,
                        had_errors
                    ],
                )
                .map_err(sql_err)?;
                tx.last_insert_rowid()
            }
        };
        if let Some(text) = config {
            tx.execute(
                "INSERT INTO configs (file_id, text) VALUES (?1, ?2)",
                params![id, text],
            )
            .map_err(sql_err)?;
        }
        let Some(facts) = facts else {
            return Ok(());
        };
        {
            let mut insert = tx
                .prepare_cached(
                    "INSERT INTO symbols (file_id, name, lname, kind, keyword, line, end_line, container, scope, exported, signature)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                )
                .map_err(sql_err)?;
            for s in &facts.symbols {
                insert
                    .execute(params![
                        id,
                        s.name,
                        s.name.to_lowercase(),
                        s.kind.as_str(),
                        s.keyword,
                        s.line,
                        s.end_line,
                        s.container,
                        s.scope,
                        s.exported,
                        s.signature
                    ])
                    .map_err(sql_err)?;
            }
        }
        {
            let mut insert = tx
                .prepare_cached(
                    "INSERT INTO refs (file_id, name, qualifier, line, kind, caller, scope)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .map_err(sql_err)?;
            for r in &facts.refs {
                insert
                    .execute(params![
                        id,
                        r.name,
                        r.qualifier,
                        r.line,
                        r.kind.as_str(),
                        r.caller,
                        r.scope
                    ])
                    .map_err(sql_err)?;
            }
        }
        {
            let mut insert = tx
                .prepare_cached(
                    "INSERT INTO imports (file_id, spec, name, alias, line, kind, public, scope)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .map_err(sql_err)?;
            for i in &facts.imports {
                insert
                    .execute(params![
                        id,
                        i.spec,
                        i.name,
                        i.alias,
                        i.line,
                        i.kind.as_str(),
                        i.public,
                        i.scope
                    ])
                    .map_err(sql_err)?;
            }
        }
        {
            let mut insert = tx
                .prepare_cached(
                    "INSERT INTO mods (file_id, name, line, path_attr, scope, inline)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(sql_err)?;
            for m in &facts.mods {
                insert
                    .execute(params![id, m.name, m.line, m.path_attr, m.scope, m.inline])
                    .map_err(sql_err)?;
            }
        }
        Ok(())
    }

    /// The file is unchanged but was touched: only its time is new.
    pub fn touch(tx: &Transaction<'_>, id: i64, mtime: i64) -> Result<(), String> {
        tx.execute(
            "UPDATE files SET mtime = ?2 WHERE id = ?1",
            params![id, mtime],
        )
        .map(|_| ())
        .map_err(sql_err)
    }

    pub fn delete_file(tx: &Transaction<'_>, id: i64) -> Result<(), String> {
        Self::delete_rows(tx, id)?;
        tx.execute("DELETE FROM files WHERE id = ?1", [id])
            .map(|_| ())
            .map_err(sql_err)
    }

    pub fn files(&self) -> Result<Vec<FileRow>, String> {
        let mut statement = self
            .conn
            .prepare("SELECT id, path, kind, lines, skipped, package FROM files ORDER BY path")
            .map_err(sql_err)?;
        let rows = statement
            .query_map([], |r| {
                Ok(FileRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    kind: r.get(2)?,
                    lines: r.get(3)?,
                    skipped: r.get(4)?,
                    package: r.get(5)?,
                })
            })
            .map_err(sql_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
    }

    /// Every config file's text by path.
    pub fn configs(&self) -> Result<HashMap<String, String>, String> {
        let mut statement = self
            .conn
            .prepare("SELECT f.path, c.text FROM configs c JOIN files f ON f.id = c.file_id")
            .map_err(sql_err)?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(sql_err)?;
        rows.collect::<Result<HashMap<_, _>, _>>().map_err(sql_err)
    }

    /// Every import, by file id, in line order.
    pub fn imports(&self) -> Result<HashMap<i64, Vec<Import>>, String> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT file_id, spec, name, alias, line, kind, public, scope FROM imports
                 ORDER BY file_id, line",
            )
            .map_err(sql_err)?;
        let rows = statement
            .query_map([], |r| {
                let kind: String = r.get(5)?;
                Ok((
                    r.get::<_, i64>(0)?,
                    Import {
                        spec: r.get(1)?,
                        name: r.get(2)?,
                        alias: r.get(3)?,
                        line: r.get(4)?,
                        kind: ImportKind::parse(&kind).unwrap_or(ImportKind::Use),
                        public: r.get::<_, i64>(6)? != 0,
                        scope: r.get(7)?,
                    },
                ))
            })
            .map_err(sql_err)?;
        let mut out: HashMap<i64, Vec<Import>> = HashMap::new();
        for row in rows {
            let (id, import) = row.map_err(sql_err)?;
            out.entry(id).or_default().push(import);
        }
        Ok(out)
    }

    /// Every `mod` declaration, by file id.
    pub fn mods(&self) -> Result<HashMap<i64, Vec<ModDecl>>, String> {
        let mut statement = self
            .conn
            .prepare("SELECT file_id, name, line, path_attr, scope, inline FROM mods ORDER BY file_id, line")
            .map_err(sql_err)?;
        let rows = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    ModDecl {
                        name: r.get(1)?,
                        line: r.get(2)?,
                        path_attr: r.get(3)?,
                        scope: r.get(4)?,
                        inline: r.get::<_, i64>(5)? != 0,
                    },
                ))
            })
            .map_err(sql_err)?;
        let mut out: HashMap<i64, Vec<ModDecl>> = HashMap::new();
        for row in rows {
            let (id, decl) = row.map_err(sql_err)?;
            out.entry(id).or_default().push(decl);
        }
        Ok(out)
    }

    /// The exported symbols at a file's own level (for the structural view).
    pub fn public_symbols(&self) -> Result<HashMap<i64, Vec<Symbol>>, String> {
        let sql = format!(
            "SELECT s.file_id, {SYMBOL_COLUMNS} FROM symbols s
             WHERE s.exported = 1 AND s.container IS NULL AND s.scope = ''
             ORDER BY s.file_id, s.line"
        );
        let mut statement = self.conn.prepare(&sql).map_err(sql_err)?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, symbol_from_row(r, 1)?)))
            .map_err(sql_err)?;
        let mut out: HashMap<i64, Vec<Symbol>> = HashMap::new();
        for row in rows {
            let (id, symbol) = row.map_err(sql_err)?;
            out.entry(id).or_default().push(symbol);
        }
        Ok(out)
    }

    /// Symbols named exactly `name`, of `kind` and in the file or folder
    /// `path` when given, filtered in SQL before `limit`.
    pub fn symbols_named(
        &self,
        name: &str,
        kind: Option<&str>,
        path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SymbolRow>, String> {
        let (exact, below) = path_filter(path);
        let sql = format!(
            "SELECT s.file_id, f.path, {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id
             WHERE s.name = ?1 AND (?2 IS NULL OR s.kind = ?2)
               AND (?3 IS NULL OR f.path = ?3 OR f.path LIKE ?4 ESCAPE '!')
             ORDER BY f.path, s.line LIMIT ?5"
        );
        self.symbol_rows(&sql, params![name, kind, exact, below, limit as i64])
    }

    /// Symbols whose name starts with `prefix`, ignoring case, at most `limit`
    /// (exact names first, then shorter names).
    pub fn symbols_starting(&self, prefix: &str, limit: usize) -> Result<Vec<SymbolRow>, String> {
        let low = prefix.to_lowercase();
        let mut high = low.clone();
        high.push(char::MAX);
        let sql = format!(
            "SELECT s.file_id, f.path, {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id
             WHERE s.lname >= ?1 AND s.lname < ?2
             ORDER BY (s.lname = ?1) DESC, length(s.name), s.exported DESC, f.path, s.line LIMIT ?3"
        );
        self.symbol_rows(&sql, params![low, high, limit as i64])
    }

    /// Every symbol of the file `path`, in line order.
    pub fn symbols_of_file(&self, path: &str) -> Result<Vec<SymbolRow>, String> {
        let sql = format!(
            "SELECT s.file_id, f.path, {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id
             WHERE f.path = ?1 ORDER BY s.line, s.name"
        );
        self.symbol_rows(&sql, params![path])
    }

    fn symbol_rows(
        &self,
        sql: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<SymbolRow>, String> {
        let mut statement = self.conn.prepare(sql).map_err(sql_err)?;
        let rows = statement
            .query_map(args, |r| {
                Ok(SymbolRow {
                    file_id: r.get(0)?,
                    path: r.get(1)?,
                    symbol: symbol_from_row(r, 2)?,
                })
            })
            .map_err(sql_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
    }

    /// Every reference to `name` (in the file or folder `within` when
    /// given), in path and line order, handed to `each` one at a time until
    /// it returns false or `cap` rows were read. Returns how many were read.
    pub fn each_ref_named(
        &self,
        name: &str,
        within: Option<&str>,
        cap: usize,
        each: &mut dyn FnMut(RefRow) -> bool,
    ) -> Result<usize, String> {
        let (exact, below) = path_filter(within);
        let mut statement = self
            .conn
            .prepare(
                "SELECT r.file_id, f.path, r.name, r.qualifier, r.line, r.kind, r.caller, r.scope
                 FROM refs r JOIN files f ON f.id = r.file_id
                 WHERE r.name = ?1
                   AND (?2 IS NULL OR f.path = ?2 OR f.path LIKE ?3 ESCAPE '!')
                 ORDER BY f.path, r.line",
            )
            .map_err(sql_err)?;
        let mut rows = statement
            .query(params![name, exact, below])
            .map_err(sql_err)?;
        let mut read = 0usize;
        while let Some(r) = rows.next().map_err(sql_err)? {
            if read >= cap {
                break;
            }
            read += 1;
            let kind: String = r.get(5).map_err(sql_err)?;
            let row = RefRow {
                file_id: r.get(0).map_err(sql_err)?,
                path: r.get(1).map_err(sql_err)?,
                reference: Reference {
                    name: r.get(2).map_err(sql_err)?,
                    qualifier: r.get(3).map_err(sql_err)?,
                    line: r.get(4).map_err(sql_err)?,
                    kind: RefKind::parse(&kind).unwrap_or(RefKind::Use),
                    caller: r.get(6).map_err(sql_err)?,
                    scope: r.get(7).map_err(sql_err)?,
                },
            };
            if !each(row) {
                break;
            }
        }
        Ok(read)
    }

    /// How many references to `name` the map holds (in `within`).
    pub fn count_refs_named(&self, name: &str, within: Option<&str>) -> Result<u64, String> {
        let (exact, below) = path_filter(within);
        self.conn
            .query_row(
                "SELECT count(*) FROM refs r JOIN files f ON f.id = r.file_id
                 WHERE r.name = ?1 AND (?2 IS NULL OR f.path = ?2 OR f.path LIKE ?3 ESCAPE '!')",
                params![name, exact, below],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| u64::try_from(n).unwrap_or(0))
            .map_err(sql_err)
    }

    /// (source files, symbols, references) in the map.
    pub fn counts(&self) -> Result<(u64, u64, u64), String> {
        let count = |sql: &str| -> Result<u64, String> {
            self.conn
                .query_row(sql, [], |r| r.get::<_, i64>(0))
                .map(|n| u64::try_from(n).unwrap_or(0))
                .map_err(sql_err)
        };
        Ok((
            count("SELECT count(*) FROM files WHERE kind NOT LIKE 'config:%'")?,
            count("SELECT count(*) FROM symbols")?,
            count("SELECT count(*) FROM refs")?,
        ))
    }

    /// Source files by language name.
    pub fn languages(&self) -> Result<Vec<(String, u64)>, String> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT kind, count(*) FROM files WHERE kind NOT LIKE 'config:%' GROUP BY kind ORDER BY kind",
            )
            .map_err(sql_err)?;
        let rows = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                ))
            })
            .map_err(sql_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
    }
}
