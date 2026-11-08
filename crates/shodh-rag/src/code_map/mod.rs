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
//! lists the project's files, drops those that are gone, and reads again only
//! those whose size, time or hash changed. Everything across files (the
//! module tree, what an import reaches, the import graph) is computed from
//! those rows when asked ([`model`]), so an edit of one file never leaves
//! another file's facts stale.
//!
//! The files read are those git tracks or would add (`.gitignore` decides),
//! never dependency or build folders, `.git`, environment files, keys or
//! credentials ([`walk`]), each at most [`MAX_FILE_BYTES`]. A refresh is
//! paced by its caller ([`Pace`]): between files it can hold (paused, or
//! yielding to other indexing) or stop, and it holds the map only one short
//! step at a time, so another refresh is never kept waiting behind a pause.

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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
/// A file over 16 KB whose lines average more than this many bytes is
/// minified or generated: listed, not read.
pub const MAX_AVERAGE_LINE: usize = 1_000;
/// Files written to the map per step at most.
const BATCH: usize = 32;
/// A step stops taking files after this long, so one refresh never holds
/// the map for long.
const STEP_TIME: Duration = Duration::from_millis(250);
/// The time one file's parse may take.
pub const PARSE_BUDGET: Duration = Duration::from_secs(2);
/// A file changed this recently is hashed even when its size and time match
/// what the map has: a second write within the clock's step keeps both.
const RECENT: Duration = Duration::from_secs(2);
/// How long a held refresh waits before asking again.
const HOLD_STEP: Duration = Duration::from_millis(200);

/// What a refresh does before its next file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    Go,
    /// Wait (paused, or other indexing runs first) and ask again. Nothing of
    /// the map is held while waiting.
    Hold,
    /// Stop: what was written stays, the rest is read next time.
    Stop,
}

/// Limits of one refresh.
#[derive(Debug, Clone, Copy, Default)]
pub struct RefreshOptions {
    /// Stop after reading this many new or changed files (the rest is left
    /// to a background refresh).
    pub max_reads: Option<usize>,
    /// Stop at this time.
    pub deadline: Option<Instant>,
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
    /// Files listed but not read (too large, minified, unreadable, too slow
    /// to parse, reached through a link).
    pub skipped: usize,
    pub bytes_read: u64,
    pub millis: u64,
    /// It ended (by its caller, a read limit or its deadline) before the end.
    pub stopped: bool,
    /// It ended because of [`RefreshOptions::max_reads`].
    pub capped: bool,
    /// It ended because of [`RefreshOptions::deadline`].
    pub timed_out: bool,
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
    /// When a refresh last ended, whether or not it changed anything.
    pub checked_at: Option<String>,
    /// Files the last listing found (sources and configs).
    pub listed: Option<u64>,
    pub partial: bool,
}

/// One project's map.
pub struct CodeMap {
    /// The folder as given (git is asked there).
    root: PathBuf,
    canonical: PathBuf,
    store: Mutex<Store>,
    /// Held for one step of a refresh at a time (the listing's deletions, or
    /// one batch of files), never while a refresh waits.
    writer: Mutex<()>,
    /// Refreshes under way.
    active: AtomicUsize,
    limits: walk::Limits,
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

fn db_err(e: rusqlite::Error) -> String {
    format!("the code map's database: {e}")
}

/// The fields of a file to write.
struct PutFile {
    path: String,
    kind: String,
    size: i64,
    mtime: i64,
    hash: String,
    lines: u32,
    skipped: Option<String>,
    facts: Option<FileFacts>,
    config: Option<String>,
}

/// A file read for the map, ready to write.
enum Prepared {
    Put(Box<PutFile>),
    Touch { id: i64, mtime: i64 },
}

/// Counts a refresh as under way while it lives.
struct Active<'a>(&'a AtomicUsize);

impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn skipped_file(
    path: &str,
    kind: String,
    size: i64,
    mtime: i64,
    hash: &str,
    why: String,
) -> Prepared {
    Prepared::Put(Box::new(PutFile {
        path: path.to_string(),
        kind,
        size,
        mtime,
        hash: hash.to_string(),
        lines: 0,
        skipped: Some(why),
        facts: None,
        config: None,
    }))
}

impl CodeMap {
    /// Where the map of the project at `root` is kept in `data_dir`: one file
    /// per project, named by its folder's path.
    pub fn map_path(data_dir: &Path, root: &Path) -> Result<PathBuf, String> {
        let canonical = std::fs::canonicalize(root)
            .map_err(|e| format!("the code folder could not be read ({e})"))?;
        Ok(Self::map_path_of(data_dir, &canonical))
    }

    /// [`Self::map_path`] for a folder already canonical.
    pub fn map_path_of(data_dir: &Path, canonical: &Path) -> PathBuf {
        data_dir
            .join("code-map")
            .join(format!("{}.db", Self::key_of(canonical)))
    }

    /// Delete the map file at `path` with SQLite's side files. The map must
    /// not be open.
    pub fn delete_files(path: &Path) -> Result<(), String> {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            match std::fs::remove_file(PathBuf::from(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("the code map could not be deleted ({e})")),
            }
        }
        Ok(())
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
        let path = Self::map_path_of(data_dir, &canonical);
        let store = Store::open(&path)?;
        store.set_meta("root", &canonical.to_string_lossy())?;
        Ok(CodeMap {
            root: root.to_path_buf(),
            canonical,
            store: Mutex::new(store),
            writer: Mutex::new(()),
            active: AtomicUsize::new(0),
            limits: walk::Limits::default(),
            generation: AtomicU64::new(0),
            model: Mutex::new(None),
        })
    }

    /// This map with other listing limits (the defaults are [`walk::Limits::default`]).
    pub fn with_limits(mut self, limits: walk::Limits) -> Self {
        self.limits = limits;
        self
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
        self.active.load(Ordering::SeqCst) > 0
    }

    /// Bring the map up to date with the folder: list its files, drop those
    /// that are gone, read those that are new or changed. Blocking. `pace` is
    /// asked before each file (and during a long parse); `progress` hears
    /// (files looked at, files listed).
    pub fn refresh(
        &self,
        pace: &dyn Fn() -> Pace,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<RefreshReport, String> {
        self.refresh_with(RefreshOptions::default(), pace, progress)
    }

    /// Empty the map, so the next refresh reads every file again.
    pub fn clear(&self) -> Result<(), String> {
        let _step = self.writer.lock();
        let store = self.store.lock();
        store.clear()?;
        store.vacuum()?;
        drop(store);
        self.bump();
        Ok(())
    }

    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Write `batch`; returns whether any file's facts changed.
    fn write(&self, batch: &mut Vec<Prepared>) -> Result<bool, String> {
        if batch.is_empty() {
            return Ok(false);
        }
        let mut changed = false;
        let mut store = self.store.lock();
        let tx = store.transaction()?;
        for item in batch.iter() {
            match item {
                Prepared::Put(file) => {
                    changed = true;
                    Store::put_file(
                        &tx,
                        &FileRecord {
                            path: &file.path,
                            kind: &file.kind,
                            size: file.size,
                            mtime: file.mtime,
                            hash: &file.hash,
                            lines: file.lines,
                            skipped: file.skipped.as_deref(),
                        },
                        file.facts.as_ref(),
                        file.config.as_deref(),
                    )?
                }
                Prepared::Touch { id, mtime } => Store::touch(&tx, *id, *mtime)?,
            }
        }
        tx.commit().map_err(db_err)?;
        drop(store);
        batch.clear();
        // A touched time changes no answer: the model stays.
        if changed {
            self.bump();
        }
        Ok(changed)
    }

    /// Read one listed file, or `None` when it is unchanged.
    fn prepare(
        &self,
        listed: &Listed,
        stamp: Option<&Stamp>,
        report: &mut RefreshReport,
        should_stop: &dyn Fn() -> bool,
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
            if let Some(stamp) = stamp.filter(|s| s.hash == hash) {
                return Some(Prepared::Touch {
                    id: stamp.id,
                    mtime,
                });
            }
            let why = format!("larger than {} KB, so it is not read", limit / 1024);
            return Some(skipped_file(path, kind, size, mtime, &hash, why));
        }
        // A folder on the way that is a link (to outside the project) is
        // never followed.
        let inside =
            std::fs::canonicalize(&full).is_ok_and(|real| real.starts_with(&self.canonical));
        if !inside {
            report.skipped += 1;
            let why = "it is reached through a link out of the code folder, so it is not read"
                .to_string();
            return Some(skipped_file(path, kind, size, mtime, "outside", why));
        }
        let bytes = match std::fs::read(&full) {
            Ok(bytes) => bytes,
            Err(e) => {
                report.skipped += 1;
                let why = format!("it could not be read ({e})");
                return Some(skipped_file(path, kind, size, mtime, "unreadable", why));
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
                let average = bytes.len() / (lines.max(1) as usize);
                let (facts, skipped) = if bytes.len() > 16 * 1024 && average > MAX_AVERAGE_LINE {
                    report.skipped += 1;
                    (
                        None,
                        Some(
                            "it looks minified or generated (very long lines), so it is not read"
                                .to_string(),
                        ),
                    )
                } else {
                    match extract::extract_within(*lang, &bytes, PARSE_BUDGET, should_stop) {
                        Ok(facts) => (Some(facts), None),
                        Err(e) => {
                            report.skipped += 1;
                            (None, Some(e))
                        }
                    }
                };
                Some(Prepared::Put(Box::new(PutFile {
                    path: path.to_string(),
                    kind,
                    size,
                    mtime,
                    hash,
                    lines,
                    skipped,
                    facts,
                    config: None,
                })))
            }
            Listed::Config(..) => Some(Prepared::Put(Box::new(PutFile {
                path: path.to_string(),
                kind,
                size,
                mtime,
                hash,
                lines,
                skipped: None,
                facts: None,
                config: Some(String::from_utf8_lossy(&bytes).into_owned()),
            }))),
        }
    }

    /// [`Self::refresh`] within `options`' limits.
    ///
    /// The map is held only one step at a time: first the listing's
    /// deletions (so a renamed file never shows twice), then batches of at
    /// most [`BATCH`] files or [`STEP_TIME`]. While `pace` says Hold nothing
    /// is held, so another refresh (an agent tool's) goes ahead meanwhile.
    pub fn refresh_with(
        &self,
        options: RefreshOptions,
        pace: &dyn Fn() -> Pace,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<RefreshReport, String> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(&self.active);
        let started = Instant::now();
        let mut report = RefreshReport::default();
        let listing = walk::list_with(&self.root, &self.limits)?;
        report.listed = listing.files.len();
        report.partial = listing.partial;
        let total = listing.files.len();
        let mut changed = false;
        // First step: what is gone goes. When the listing was cut, only files
        // before the cut are known to be gone.
        {
            let _step = self.writer.lock();
            let mut store = self.store.lock();
            let stamps = store.stamps()?;
            let listed: HashSet<&str> = listing.files.iter().map(Listed::path).collect();
            let gone: Vec<i64> = stamps
                .iter()
                .filter(|(path, _)| !listed.contains(path.as_str()))
                .filter(|(path, _)| listing.cut.as_deref().is_none_or(|cut| path.as_str() < cut))
                .map(|(_, stamp)| stamp.id)
                .collect();
            report.removed = gone.len();
            if !gone.is_empty() {
                let tx = store.transaction()?;
                for id in &gone {
                    Store::delete_file(&tx, *id)?;
                }
                tx.commit().map_err(db_err)?;
                changed = true;
            }
            store.set_meta("listed", &total.to_string())?;
        }
        if changed {
            self.bump();
        }
        progress(0, total);
        let past_deadline = || options.deadline.is_some_and(|d| Instant::now() >= d);
        let stop_now = || past_deadline() || pace() == Pace::Stop;
        let mut next = 0usize;
        'files: while next < total {
            // Waits happen here, outside any step.
            loop {
                if past_deadline() {
                    report.timed_out = true;
                    report.stopped = true;
                    break 'files;
                }
                match pace() {
                    Pace::Go => break,
                    Pace::Hold => std::thread::sleep(HOLD_STEP),
                    Pace::Stop => {
                        report.stopped = true;
                        break 'files;
                    }
                }
            }
            let step = self.writer.lock();
            let step_started = Instant::now();
            let mut batch: Vec<Prepared> = Vec::with_capacity(BATCH);
            while next < total && batch.len() < BATCH && step_started.elapsed() < STEP_TIME {
                if options.max_reads.is_some_and(|max| report.read >= max) {
                    report.capped = true;
                    report.stopped = true;
                    break;
                }
                let listed = &listing.files[next];
                let stamp = self.store.lock().stamp(listed.path())?;
                if let Some(item) = self.prepare(listed, stamp.as_ref(), &mut report, &stop_now) {
                    batch.push(item);
                }
                next += 1;
                if next % 16 == 0 {
                    progress(next, total);
                }
                // Leave the processor to others between files.
                std::thread::yield_now();
            }
            changed |= self.write(&mut batch)?;
            drop(step);
            if report.capped {
                break;
            }
        }
        let now = now_rfc3339();
        {
            let store = self.store.lock();
            if changed || store.meta("updated_at")?.is_none() {
                store.set_meta("updated_at", &now)?;
            }
            store.set_meta("checked_at", &now)?;
            // Complete: every listed file was looked at, and the listing was
            // not cut short by time or entries (a cut by the file limit is
            // lasting, and the map says it is partial).
            if !report.stopped && !listing.timed_out {
                store.set_meta("complete_at", &now)?;
            }
            store.set_meta("partial", if report.partial { "1" } else { "0" })?;
        }
        progress(next.min(total), total);
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
            checked_at: store.meta("checked_at")?,
            listed: store.meta("listed")?.and_then(|n| n.parse().ok()),
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

    /// The project model of the map as it is now, computed again only after
    /// a file's facts changed.
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

    /// Whether the cached model is current (it is reused, not computed again).
    pub fn model_is_current(&self) -> bool {
        let generation = self.generation.load(Ordering::SeqCst);
        self.model
            .lock()
            .as_ref()
            .is_some_and(|(at, _)| *at == generation)
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
