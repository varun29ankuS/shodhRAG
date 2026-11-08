//! Code mode's code map: where each symbol of a project is defined, who calls
//! or uses it, what each module imports and what imports it, and the cycles.
//!
//! It is native: files are read with tree-sitter grammars (Rust,
//! TypeScript/TSX, JavaScript, Python, Go) in Shodh's own process, with no
//! language server, no Python and nothing of the project run. The design
//! follows codanna (bartolli/codanna, Apache-2.0) and code-review-graph
//! (tirth8205/code-review-graph): per-file symbols, references and imports,
//! with names resolved across the project when asked. No code of either is
//! used (see NOTICE).
//!
//! Each project's map is a SQLite file in Shodh's data folder
//! ([`CodeMap::map_path`]), opened only when Code mode asks for it, so it
//! costs nothing while Code mode is not in use. It holds per-file facts keyed
//! by each file's content hash ([`store`]); a refresh ([`CodeMap::refresh`])
//! lists the project's files, reads again only those whose size, time or
//! hash changed and drops those that are gone. Everything across files (the
//! module tree, what an import reaches, the import graph) is computed from
//! those rows when asked ([`model`]), so an edit of one file never leaves
//! another file's facts stale.
//!
//! The files read are those git tracks or would add (`.gitignore` decides),
//! never dependency or build folders, `.git`, environment files, keys or
//! credentials ([`walk`]), each at most [`MAX_FILE_BYTES`]. A refresh is
//! paced by its caller ([`Pace`]): between files it can hold (paused, or
//! yielding to other indexing) or stop.

pub mod extract;
pub mod lang;
pub mod model;
pub mod query;
pub mod store;
pub mod tsconfig;
pub mod view;
pub mod walk;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};

pub use extract::{FileFacts, ImportKind, RefKind, SymbolKind};
pub use lang::Lang;
pub use model::Model;
pub use query::{
    Definitions, DepEdge, Dependencies, Outline, OutlineFile, RefHit, References, SymbolHit,
};
pub use view::MapStructure;

use lang::ConfigKind;
use store::{FileRecord, Stamp, Store};
use walk::Listed;

/// A source file larger than this is listed with its size, not read.
pub const MAX_FILE_BYTES: u64 = 512 * 1024;
/// A config file (`Cargo.toml`, `tsconfig.json`, `go.mod`) larger than this
/// is not read.
pub const MAX_CONFIG_BYTES: u64 = 256 * 1024;
/// Files written to the map per transaction.
const BATCH: usize = 32;
/// A file changed this recently is hashed even when its size and time match
/// what the map has: a second write within the clock's step keeps both.
const RECENT: Duration = Duration::from_secs(2);
/// How long a held refresh waits before asking again.
const HOLD_STEP: Duration = Duration::from_millis(200);

/// What a refresh does before its next file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    Go,
    /// Wait (paused, or other indexing runs first) and ask again.
    Hold,
    /// Stop: what was written stays, the rest is read next time.
    Stop,
}

/// What a refresh did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshReport {
    /// Files listed (sources and configs).
    pub listed: usize,
    /// Files read and written again (new or changed).
    pub read: usize,
    pub unchanged: usize,
    pub removed: usize,
    /// Files listed but not read (too large, unreadable).
    pub skipped: usize,
    pub bytes_read: u64,
    pub millis: u64,
    /// It was stopped before the end.
    pub stopped: bool,
    /// A listing limit was reached.
    pub partial: bool,
}

/// What the map holds, for the UI and the tools.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapStatus {
    pub files: u64,
    pub symbols: u64,
    pub references: u64,
    /// Source files by language.
    pub languages: Vec<(String, u64)>,
    /// When a refresh last went through every file (RFC 3339).
    pub complete_at: Option<String>,
    /// When anything last changed.
    pub updated_at: Option<String>,
    pub partial: bool,
}

/// One project's map.
pub struct CodeMap {
    /// The folder as given (git is asked there).
    root: PathBuf,
    canonical: PathBuf,
    store: Mutex<Store>,
    refreshing: Mutex<()>,
    generation: AtomicU64,
    model: Mutex<Option<(u64, Arc<Model>)>>,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn recent(meta: &std::fs::Metadata) -> bool {
    meta.modified()
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_none_or(|age| age < RECENT)
}

fn line_count(bytes: &[u8]) -> u32 {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|b| **b == b'\n').count();
    let last = usize::from(bytes.last() != Some(&b'\n'));
    u32::try_from(newlines + last).unwrap_or(u32::MAX)
}

fn hash_of(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(&digest[..16])
}

/// A file read for the map, ready to write.
enum Prepared {
    Put {
        path: String,
        kind: String,
        size: i64,
        mtime: i64,
        hash: String,
        lines: u32,
        skipped: Option<String>,
        facts: Option<FileFacts>,
        config: Option<String>,
    },
    Touch {
        id: i64,
        mtime: i64,
    },
}

impl CodeMap {
    /// Where the map of the project at `root` is kept in `data_dir`: one file
    /// per project, named by its folder's path.
    pub fn map_path(data_dir: &Path, root: &Path) -> Result<PathBuf, String> {
        let canonical = std::fs::canonicalize(root)
            .map_err(|e| format!("the code folder could not be read ({e})"))?;
        Ok(data_dir
            .join("code-map")
            .join(format!("{}.db", Self::key_of(&canonical))))
    }

    /// The key of a (canonical) project folder.
    pub fn key_of(canonical: &Path) -> String {
        let text = canonical.to_string_lossy();
        let text = if cfg!(windows) {
            text.to_lowercase()
        } else {
            text.into_owned()
        };
        hex::encode(&Sha256::digest(text.as_bytes())[..12])
    }

    /// Open (or create) the map of the project at `root`.
    pub fn open(data_dir: &Path, root: &Path) -> Result<CodeMap, String> {
        let canonical = std::fs::canonicalize(root)
            .map_err(|e| format!("the code folder could not be read ({e})"))?;
        if !canonical.is_dir() {
            return Err("The code folder is not a folder.".to_string());
        }
        let path = Self::map_path(data_dir, root)?;
        let store = Store::open(&path)?;
        store.set_meta("root", &canonical.to_string_lossy())?;
        Ok(CodeMap {
            root: root.to_path_buf(),
            canonical,
            store: Mutex::new(store),
            refreshing: Mutex::new(()),
            generation: AtomicU64::new(0),
            model: Mutex::new(None),
        })
    }

    /// The project's folder as given.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn canonical_root(&self) -> &Path {
        &self.canonical
    }

    /// A refresh is running now.
    pub fn is_refreshing(&self) -> bool {
        self.refreshing.is_locked()
    }

    /// Bring the map up to date with the folder: list its files, read those
    /// that are new or changed, drop those that are gone. Blocking; one at a
    /// time (a second waits for the first). `pace` is asked before each file;
    /// `progress` hears (files looked at, files listed).
    pub fn refresh(
        &self,
        pace: &dyn Fn() -> Pace,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<RefreshReport, String> {
        let _one = self.refreshing.lock();
        self.refresh_locked(pace, progress)
    }

    /// [`Self::refresh`], unless another refresh holds the map for longer
    /// than `wait`: then `None`, and the map is read as it is.
    pub fn refresh_within(
        &self,
        wait: Duration,
        pace: &dyn Fn() -> Pace,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<Option<RefreshReport>, String> {
        let Some(_one) = self.refreshing.try_lock_for(wait) else {
            return Ok(None);
        };
        self.refresh_locked(pace, progress).map(Some)
    }

    /// Empty the map, so the next refresh reads every file again.
    pub fn clear(&self) -> Result<(), String> {
        let _one = self.refreshing.lock();
        self.store.lock().clear()?;
        self.bump();
        Ok(())
    }

    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn write(&self, batch: &mut Vec<Prepared>) -> Result<(), String> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut store = self.store.lock();
        let tx = store.transaction()?;
        for item in batch.iter() {
            match item {
                Prepared::Put {
                    path,
                    kind,
                    size,
                    mtime,
                    hash,
                    lines,
                    skipped,
                    facts,
                    config,
                } => Store::put_file(
                    &tx,
                    &FileRecord {
                        path,
                        kind,
                        size: *size,
                        mtime: *mtime,
                        hash,
                        lines: *lines,
                        skipped: skipped.as_deref(),
                    },
                    facts.as_ref(),
                    config.as_deref(),
                )?,
                Prepared::Touch { id, mtime } => Store::touch(&tx, *id, *mtime)?,
            }
        }
        tx.commit()
            .map_err(|e| format!("the code map's database: {e}"))?;
        drop(store);
        batch.clear();
        self.bump();
        Ok(())
    }

    /// Read one listed file, or `None` when it is unchanged.
    fn prepare(
        &self,
        listed: &Listed,
        stamp: Option<&Stamp>,
        report: &mut RefreshReport,
    ) -> Option<Prepared> {
        let path = listed.path();
        let full = self.root.join(path);
        let meta = std::fs::symlink_metadata(&full).ok()?;
        if !meta.file_type().is_file() {
            return None;
        }
        let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
        let mtime = mtime_ns(&meta);
        if let Some(stamp) = stamp {
            if stamp.size == size && stamp.mtime == mtime && !recent(&meta) {
                report.unchanged += 1;
                return None;
            }
        }
        let (kind, limit) = match listed {
            Listed::Source(_, lang) => (lang.name().to_string(), MAX_FILE_BYTES),
            Listed::Config(_, config) => (format!("config:{}", config.name()), MAX_CONFIG_BYTES),
        };
        if meta.len() > limit {
            report.skipped += 1;
            let hash = format!("size:{size}");
            if stamp.is_some_and(|s| s.hash == hash) {
                return Some(Prepared::Touch {
                    id: stamp.map(|s| s.id).unwrap_or_default(),
                    mtime,
                });
            }
            return Some(Prepared::Put {
                path: path.to_string(),
                kind,
                size,
                mtime,
                hash,
                lines: 0,
                skipped: Some(format!(
                    "larger than {} KB, so it is not read",
                    limit / 1024
                )),
                facts: None,
                config: None,
            });
        }
        let bytes = match std::fs::read(&full) {
            Ok(bytes) => bytes,
            Err(e) => {
                report.skipped += 1;
                return Some(Prepared::Put {
                    path: path.to_string(),
                    kind,
                    size,
                    mtime,
                    hash: "unreadable".to_string(),
                    lines: 0,
                    skipped: Some(format!("it could not be read ({e})")),
                    facts: None,
                    config: None,
                });
            }
        };
        report.bytes_read += bytes.len() as u64;
        let hash = hash_of(&bytes);
        if let Some(stamp) = stamp.filter(|s| s.hash == hash) {
            report.unchanged += 1;
            return Some(Prepared::Touch {
                id: stamp.id,
                mtime,
            });
        }
        report.read += 1;
        let lines = line_count(&bytes);
        match listed {
            Listed::Source(_, lang) => {
                let (facts, skipped) = match extract::extract(*lang, &bytes) {
                    Ok(facts) => (Some(facts), None),
                    Err(e) => {
                        report.skipped += 1;
                        (None, Some(e))
                    }
                };
                Some(Prepared::Put {
                    path: path.to_string(),
                    kind,
                    size,
                    mtime,
                    hash,
                    lines,
                    skipped,
                    facts,
                    config: None,
                })
            }
            Listed::Config(..) => Some(Prepared::Put {
                path: path.to_string(),
                kind,
                size,
                mtime,
                hash,
                lines,
                skipped: None,
                facts: None,
                config: Some(String::from_utf8_lossy(&bytes).into_owned()),
            }),
        }
    }

    fn refresh_locked(
        &self,
        pace: &dyn Fn() -> Pace,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<RefreshReport, String> {
        let started = Instant::now();
        let mut report = RefreshReport::default();
        let listing = walk::list(&self.root);
        report.listed = listing.files.len();
        report.partial = listing.partial;
        let stamps = self.store.lock().stamps()?;
        let total = listing.files.len();
        let mut seen: HashSet<&str> = HashSet::with_capacity(total);
        let mut batch: Vec<Prepared> = Vec::with_capacity(BATCH);
        progress(0, total);
        for (done, listed) in listing.files.iter().enumerate() {
            loop {
                match pace() {
                    Pace::Go => break,
                    Pace::Hold => {
                        // Nothing is held while waiting: what was read is written.
                        self.write(&mut batch)?;
                        std::thread::sleep(HOLD_STEP);
                    }
                    Pace::Stop => {
                        self.write(&mut batch)?;
                        report.stopped = true;
                        report.millis = started.elapsed().as_millis() as u64;
                        self.store.lock().set_meta("updated_at", &now_rfc3339())?;
                        return Ok(report);
                    }
                }
            }
            seen.insert(listed.path());
            if let Some(item) = self.prepare(listed, stamps.get(listed.path()), &mut report) {
                batch.push(item);
                if batch.len() >= BATCH {
                    self.write(&mut batch)?;
                }
            }
            if done % 16 == 15 {
                progress(done + 1, total);
            }
            // Leave the processor to others between files.
            std::thread::yield_now();
        }
        self.write(&mut batch)?;
        let gone: Vec<i64> = stamps
            .iter()
            .filter(|(path, _)| !seen.contains(path.as_str()))
            .map(|(_, stamp)| stamp.id)
            .collect();
        report.removed = gone.len();
        {
            let mut store = self.store.lock();
            if !gone.is_empty() {
                let tx = store.transaction()?;
                for id in &gone {
                    Store::delete_file(&tx, *id)?;
                }
                tx.commit()
                    .map_err(|e| format!("the code map's database: {e}"))?;
            }
            let now = now_rfc3339();
            if report.read > 0 || report.removed > 0 || store.meta("updated_at")?.is_none() {
                store.set_meta("updated_at", &now)?;
            }
            store.set_meta("complete_at", &now)?;
            store.set_meta("partial", if report.partial { "1" } else { "0" })?;
        }
        self.bump();
        progress(total, total);
        report.millis = started.elapsed().as_millis() as u64;
        Ok(report)
    }

    /// What the map holds now.
    pub fn status(&self) -> Result<MapStatus, String> {
        let store = self.store.lock();
        let (files, symbols, references) = store.counts()?;
        Ok(MapStatus {
            files,
            symbols,
            references,
            languages: store.languages()?,
            complete_at: store.meta("complete_at")?,
            updated_at: store.meta("updated_at")?,
            partial: store.meta("partial")?.as_deref() == Some("1"),
        })
    }

    /// A refresh has gone through every file at least once.
    pub fn is_complete(&self) -> bool {
        self.store
            .lock()
            .meta("complete_at")
            .ok()
            .flatten()
            .is_some()
    }

    /// The project model of the map as it is now (computed once per change).
    pub fn model(&self) -> Result<Arc<Model>, String> {
        let generation = self.generation.load(Ordering::SeqCst);
        if let Some((at, model)) = self.model.lock().as_ref() {
            if *at == generation {
                return Ok(model.clone());
            }
        }
        let model = {
            let store = self.store.lock();
            Arc::new(Model::build(&store)?)
        };
        *self.model.lock() = Some((generation, model.clone()));
        Ok(model)
    }

    /// Run `read` on the map's store.
    pub(crate) fn with_store<T>(
        &self,
        read: impl FnOnce(&Store) -> Result<T, String>,
    ) -> Result<T, String> {
        let store = self.store.lock();
        read(&store)
    }
}

/// The config kind a listed path is (for tests and callers that list files).
pub fn config_kind(path: &str) -> Option<&'static str> {
    ConfigKind::of_path(path).map(ConfigKind::name)
}
