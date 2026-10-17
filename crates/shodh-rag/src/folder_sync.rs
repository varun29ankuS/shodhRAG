//! Keeps an indexed folder and the index in step.
//!
//! Each folder source has a manifest (`<source id>.json` in the store's
//! directory): every indexable file with the modification time and size it
//! had when it was last indexed (or failed to index). A sync scans the folder
//! and compares: new and changed files are indexed (a file's previous chunks
//! are replaced), files that are gone are removed with `delete_by_source`.
//! A rename is a removal plus a new file. Files that failed keep their entry
//! and their reason, so they are tried again only once they change.
//!
//! A source indexed before it had a manifest is compared against the index
//! instead: indexed files that still exist are taken as current.
//!
//! Files whose chunks were indexed with an older index format
//! ([`crate::rag_engine::INDEX_FORMAT`]) are indexed again even when they did
//! not change, so a library indexed by an earlier version is brought to the
//! current format on its first sync instead of ranking old chunks against new
//! ones. Where their chunks allow it, unchanged files are brought up to date
//! in place from their stored chunks ([`RAGEngine::upgrade_in_place`]): no
//! parsing, no OCR (scans indexed before OCR was cached included), refined
//! tables kept, embedding only. Other files are read again; a scan read
//! again comes from the OCR cache.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use walkdir::WalkDir;

use crate::embeddings::SearchModelsMissing;
use crate::indexing::{is_supported_file_type, FileFailure};
use crate::rag_engine::{canonical_path, normalize_source_path, RAGEngine};

/// What a file looked like when it was last indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub modified_ms: u64,
    pub size: u64,
}

/// Files to index (on disk) and to remove (index identities).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncPlan {
    pub index: Vec<PathBuf>,
    pub remove: Vec<String>,
}

/// What one sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SyncReport {
    /// Files indexed (new or changed).
    pub indexed: usize,
    /// Files removed from the index.
    pub removed: usize,
    /// Indexable files in the folder now (with those kept in folders that
    /// could not be read).
    pub files: usize,
    /// Files of the folder that could not be indexed (this sync or before,
    /// and unchanged since).
    pub failures: Vec<FileFailure>,
    /// Folders and files the sync could not read ([`FolderScan::unreadable`]):
    /// what the index holds of them was left as it was.
    pub unreadable: Vec<FileFailure>,
}

/// Office lock files (`~$report.docx`) and partial downloads or saves.
pub fn is_temporary_file(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) else {
        return false;
    };
    name.starts_with("~$")
        || name.starts_with(".~lock.")
        || [".tmp", ".part", ".crdownload"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// Hidden entries below the folder (`.git`, `.claude`, `.env`): skipped with
/// everything inside them. The folder itself is never treated as hidden.
pub fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry.depth() > 0 && entry.file_name().to_string_lossy().starts_with('.')
}

/// Whether a file at `path` is one a folder source indexes.
pub fn is_indexable(path: &Path) -> bool {
    !is_temporary_file(path)
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| is_supported_file_type(&e.to_lowercase()))
}

/// What a scan of a folder found: its indexable files and what could not be
/// read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FolderScan {
    /// The indexable files, keyed by index identity ([`normalize_source_path`]).
    pub files: BTreeMap<String, (PathBuf, FileStamp)>,
    /// Folders and files that could not be read (permissions, a network
    /// share or a drive that went away), by index identity, with why. Files
    /// in an unreadable folder may still exist: nothing under it is taken as
    /// gone.
    pub unreadable: BTreeMap<String, FileFailure>,
}

impl FolderScan {
    /// Whether the file `key` lies in something the scan could not read:
    /// the file itself, or a folder above it.
    pub fn is_unreadable(&self, key: &str) -> bool {
        self.unreadable.keys().any(|path| is_within(key, path))
    }
}

/// Whether the index identity `key` is `path` or lies under the folder `path`.
fn is_within(key: &str, path: &str) -> bool {
    match key.strip_prefix(path) {
        Some("") => true,
        Some(rest) => rest.starts_with('/') || path.ends_with('/'),
        None => false,
    }
}

/// The indexable files under `folder`, keyed by index identity
/// ([`normalize_source_path`]), and the folders and files that could not be
/// read.
pub fn scan_folder_checked(folder: &Path) -> FolderScan {
    let mut scan = FolderScan::default();
    for entry in WalkDir::new(folder)
        .into_iter()
        .filter_entry(|e| !is_hidden(e))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                // An error names the folder or file it could not read; one
                // that names nothing leaves the whole folder unknown.
                let path = canonical_path(error.path().unwrap_or(folder));
                scan.unreadable.insert(
                    normalize_source_path(&path),
                    FileFailure {
                        file: path.display().to_string(),
                        reason: error.to_string(),
                    },
                );
                continue;
            }
        };
        if !(entry.file_type().is_file() && is_indexable(entry.path())) {
            continue;
        }
        let path = canonical_path(entry.path());
        let key = normalize_source_path(&path);
        match entry.metadata() {
            Ok(meta) => {
                let modified_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as u64);
                let stamp = FileStamp {
                    modified_ms,
                    size: meta.len(),
                };
                scan.files.insert(key, (path, stamp));
            }
            Err(error) => {
                scan.unreadable.insert(
                    key,
                    FileFailure {
                        file: path.display().to_string(),
                        reason: error.to_string(),
                    },
                );
            }
        }
    }
    scan
}

/// The indexable files under `folder` that could be read, keyed by index
/// identity ([`normalize_source_path`]); see [`scan_folder_checked`].
pub fn scan_folder(folder: &Path) -> BTreeMap<String, (PathBuf, FileStamp)> {
    scan_folder_checked(folder).files
}

/// The files of `folder` that `selected` names, as [`scan_folder`] finds
/// them, scanned on a blocking thread.
async fn scan_selected(
    folder: &Path,
    selected: &[PathBuf],
) -> Result<BTreeMap<String, (PathBuf, FileStamp)>, String> {
    let root = folder.to_path_buf();
    let mut on_disk = tokio::task::spawn_blocking(move || scan_folder(&root))
        .await
        .map_err(|e| format!("Scanning {} failed: {e}", folder.display()))?;
    let selected: BTreeSet<String> = selected
        .iter()
        .map(|path| normalize_source_path(path))
        .collect();
    on_disk.retain(|key, _| selected.contains(key));
    Ok(on_disk)
}

/// What to index and remove so the index matches `on_disk`, given the
/// stamps the files had when they were last indexed and the files whose
/// chunks are of an older index format (`stale`), which are indexed again.
pub fn plan(
    known: &BTreeMap<String, FileStamp>,
    on_disk: &BTreeMap<String, (PathBuf, FileStamp)>,
    stale: &BTreeSet<String>,
) -> SyncPlan {
    SyncPlan {
        index: on_disk
            .iter()
            .filter(|(key, (_, stamp))| known.get(*key) != Some(stamp) || stale.contains(*key))
            .map(|(_, (path, _))| path.clone())
            .collect(),
        remove: known
            .keys()
            .filter(|key| !on_disk.contains_key(*key))
            .cloned()
            .collect(),
    }
}

/// [`plan`] for a folder as `scan` found it: a file in a folder or file the
/// scan could not read is not removed, since it may still be there.
pub fn plan_for_scan(
    known: &BTreeMap<String, FileStamp>,
    scan: &FolderScan,
    stale: &BTreeSet<String>,
) -> SyncPlan {
    let mut planned = plan(known, &scan.files, stale);
    planned.remove.retain(|key| !scan.is_unreadable(key));
    planned
}

/// A manifest as saved, borrowing the sync's working state.
#[derive(Serialize)]
struct ManifestRef<'a> {
    folder: &'a str,
    files: &'a BTreeMap<String, FileStamp>,
    failures: &'a BTreeMap<String, FileFailure>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    folder: String,
    files: BTreeMap<String, FileStamp>,
    /// Why files (by key) could not be indexed.
    #[serde(default)]
    failures: BTreeMap<String, FileFailure>,
}

/// Where folder manifests are kept.
#[derive(Debug, Clone)]
pub struct ManifestStore {
    dir: PathBuf,
}

impl ManifestStore {
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The name (without `.json`) of the manifest of `source_id`.
    fn file_id(source_id: &str) -> String {
        source_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    }

    fn path(&self, source_id: &str) -> PathBuf {
        self.dir.join(format!("{}.json", Self::file_id(source_id)))
    }

    fn load(&self, source_id: &str) -> Option<Manifest> {
        let text = std::fs::read_to_string(self.path(source_id)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Every readable manifest in the store, by [`Self::file_id`]; none when
    /// the store's directory does not exist yet. A manifest being saved
    /// (`.json.tmp`) is not one yet.
    fn load_all(&self) -> Result<BTreeMap<String, Manifest>, String> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(format!("Reading {} failed: {e}", self.dir.display())),
        };
        Ok(entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    return None;
                }
                let id = path.file_stem()?.to_str()?.to_string();
                let text = std::fs::read_to_string(&path).ok()?;
                Some((id, serde_json::from_str(&text).ok()?))
            })
            .collect())
    }

    fn save(&self, source_id: &str, manifest: &ManifestRef<'_>) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.path(source_id);
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string(manifest).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// Drops what the manifest of `source_id` records about the file `key`
    /// (its stamp and any failure), so the next sync reads it as a new file.
    /// Returns whether it was recorded. The caller keeps syncs of the source
    /// from running meanwhile.
    pub fn forget_file(&self, source_id: &str, key: &str) -> Result<bool, String> {
        let Some(mut manifest) = self.load(source_id) else {
            return Ok(false);
        };
        let had_stamp = manifest.files.remove(key).is_some();
        let had_failure = manifest.failures.remove(key).is_some();
        if !(had_stamp || had_failure) {
            return Ok(false);
        }
        self.save(
            source_id,
            &ManifestRef {
                folder: &manifest.folder,
                files: &manifest.files,
                failures: &manifest.failures,
            },
        )?;
        Ok(true)
    }

    /// The folders of the sources that have a manifest, except `except`'s:
    /// folders the Library lists, whether kept in sync now or not.
    pub fn listed_folders(&self, except: &str) -> Vec<String> {
        let skip = Self::file_id(except);
        match self.load_all() {
            Ok(manifests) => manifests
                .into_iter()
                .filter(|(id, _)| *id != skip)
                .map(|(_, manifest)| manifest.folder)
                .filter(|folder| !folder.is_empty())
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "folder manifests unreadable");
                Vec::new()
            }
        }
    }

    /// Whether `source_id` has a manifest.
    pub fn has_manifest(&self, source_id: &str) -> bool {
        self.path(source_id).is_file()
    }

    /// Forget a source (it was removed from the Library).
    pub fn forget(&self, source_id: &str) {
        // Missing is fine: the source never finished a sync.
        let _ = std::fs::remove_file(self.path(source_id));
    }
}

/// The index's files of `source_id`, stamped as `on_disk` has them: a source
/// without a manifest was indexed in full, so its indexed files that still
/// exist count as current.
async fn known_from_index(
    rag: &RwLock<RAGEngine>,
    source_id: &str,
    on_disk: &BTreeMap<String, (PathBuf, FileStamp)>,
) -> Result<BTreeMap<String, FileStamp>, String> {
    let rows = rag
        .read()
        .await
        .document_sources()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let indexed: BTreeSet<String> = rows
        .into_iter()
        .filter(|row| row.space_id == source_id)
        .map(|row| row.source)
        .collect();
    // Gone from disk: a stamp that matches nothing, so the plan removes it.
    let gone = FileStamp {
        modified_ms: 0,
        size: 0,
    };
    Ok(indexed
        .into_iter()
        .map(|key| {
            let stamp = on_disk.get(&key).map_or(gone, |(_, stamp)| *stamp);
            (key, stamp)
        })
        .collect())
}

/// The files of `source_id` whose chunks are of an older index format.
async fn stale_files(rag: &RwLock<RAGEngine>, source_id: &str) -> Result<BTreeSet<String>, String> {
    Ok(rag
        .read()
        .await
        .stale_sources()
        .await
        .map_err(|e| format!("{e:#}"))?
        .into_iter()
        .filter(|row| row.space_id == source_id)
        .map(|row| row.source)
        .collect())
}

/// What a folder source being added needs to adopt the files of its folder
/// that are already indexed ([`crate::indexing::index_folder_adopting`]).
#[derive(Debug, Clone, Copy)]
pub struct Adoption<'a> {
    /// Where the folder manifests are kept.
    pub store: &'a ManifestStore,
    /// The Library's listed folder sources. They keep their files: a listed
    /// source that gave a file away would index it again on its next sync.
    pub listed: &'a BTreeSet<String>,
}

/// The manifest of a folder source while its folder is indexed, saved after
/// every change so the source's syncs start from what was indexed.
pub(crate) struct FolderManifest<'a> {
    store: &'a ManifestStore,
    source_id: String,
    folder: String,
    files: BTreeMap<String, FileStamp>,
    failures: BTreeMap<String, FileFailure>,
}

impl FolderManifest<'_> {
    /// The file `key` was indexed as it was at `stamp`, taken before
    /// indexing it, or failed to index (`failure`). A failed file is
    /// recorded without a stamp, so the source's first sync tries it again
    /// (it may have been locked or still being written), as when the folder
    /// had no manifest; that sync stamps it if it fails again.
    pub(crate) fn record(&mut self, key: String, stamp: FileStamp, failure: Option<FileFailure>) {
        match failure {
            Some(failure) => {
                self.files.remove(&key);
                self.failures.insert(key, failure);
            }
            None => {
                self.failures.remove(&key);
                self.files.insert(key, stamp);
            }
        }
        self.save();
    }

    /// A manifest that is not saved costs work, not correctness: files it
    /// does not record are indexed again by the source's next sync.
    fn save(&self) {
        let saved = self.store.save(
            &self.source_id,
            &ManifestRef {
                folder: &self.folder,
                files: &self.files,
                failures: &self.failures,
            },
        );
        if let Err(error) = saved {
            tracing::warn!(source_id = %self.source_id, %error, "folder manifest not saved");
        }
    }
}

/// The files [`adopt_indexed`] adopted, and the source's manifest.
pub(crate) struct Adopted<'a> {
    /// Index identities of the files adopted: not to be indexed again.
    pub(crate) keys: BTreeSet<String>,
    /// The folder's files that were scanned, by index identity.
    pub(crate) on_disk: BTreeMap<String, (PathBuf, FileStamp)>,
    pub(crate) manifest: FolderManifest<'a>,
}

/// Whether the indexed file `key` is still as it was indexed: a manifest
/// recorded it indexed (not failed) as it is on disk (`stamp`). The chunks
/// are from the file's latest indexing, which came at or after that one,
/// and the file has not changed since, so they are the file's current
/// content. Without such a record (a single-file source, a file added in a
/// chat, a source whose manifest is gone) nothing shows the chunks are
/// current, and the file is indexed again.
fn indexed_as_on_disk(key: &str, stamp: FileStamp, manifests: &BTreeMap<String, Manifest>) -> bool {
    manifests
        .values()
        .any(|m| m.files.get(key) == Some(&stamp) && !m.failures.contains_key(key))
}

/// Gives the folder source `source_id` (the folder `root`) the files of
/// `selected` that are already indexed under another source, unchanged and
/// in the current index format, instead of indexing them again (adding a
/// folder whose files were indexed by a source the Library lost).
///
/// A file is adopted only when its chunks all belong to one other source,
/// one not `adoption.listed`, and a folder manifest recorded it indexed as
/// it is on disk ([`indexed_as_on_disk`]). Its chunks move with
/// [`RAGEngine::move_source_to_space`], one LanceDB update per file, so a
/// file is either wholly adopted or left as it is; a file whose move fails
/// or finds no chunks (indexed again meanwhile) is not adopted and is
/// indexed as usual. Then the manifest of `source_id` is saved with the
/// adopted files' stamps, and the manifests of unlisted sources stop naming
/// them. A manifest that cannot be saved only means its source's next sync
/// indexes the files again.
pub(crate) async fn adopt_indexed<'a>(
    root: &Path,
    source_id: &str,
    selected: &[PathBuf],
    adoption: Adoption<'a>,
    rag: &RwLock<RAGEngine>,
) -> Result<Adopted<'a>, String> {
    let on_disk = scan_selected(root, selected).await?;
    let (rows, stale) = {
        let engine = rag.read().await;
        let rows = engine
            .document_sources()
            .await
            .map_err(|e| format!("{e:#}"))?;
        let stale: BTreeSet<String> = engine
            .stale_sources()
            .await
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|row| row.source)
            .collect();
        (rows, stale)
    };
    // The sources each indexed file's chunks belong to.
    let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows {
        owners.entry(row.source).or_default().insert(row.space_id);
    }
    let mut manifests = adoption.store.load_all()?;

    let mut adopted: BTreeMap<String, FileStamp> = BTreeMap::new();
    for (key, (_, stamp)) in &on_disk {
        // Chunks in more than one source: moving one source's would leave
        // the file half owned.
        let donor = match owners.get(key) {
            Some(spaces) if spaces.len() == 1 => spaces.iter().next(),
            _ => None,
        };
        let Some(donor) = donor else {
            continue;
        };
        if donor == source_id
            || adoption.listed.contains(donor)
            || stale.contains(key)
            || !indexed_as_on_disk(key, *stamp, &manifests)
        {
            continue;
        }
        let moved = rag
            .write()
            .await
            .move_source_to_space(key, donor, source_id)
            .await;
        match moved {
            Ok(0) => {}
            Ok(_) => {
                adopted.insert(key.clone(), *stamp);
            }
            Err(e) => {
                tracing::warn!(
                    source = %key,
                    from = %donor,
                    to = source_id,
                    error = %format!("{e:#}"),
                    "indexed file not adopted; indexing it again"
                );
            }
        }
    }

    // The rows the moves replaced leave the disk once, not per file.
    if !adopted.is_empty() {
        if let Err(e) = rag.write().await.compact_chunks().await {
            tracing::warn!(error = %format!("{e:#}"), "chunk store not compacted after adoption");
        }
    }

    // Where the source's own manifest left off (a source being indexed
    // again), else what it owns in the index, now with the adopted files.
    let (mut files, mut failures) = match manifests.remove(&ManifestStore::file_id(source_id)) {
        Some(own) => (own.files, own.failures),
        None => (
            known_from_index(rag, source_id, &on_disk).await?,
            BTreeMap::new(),
        ),
    };
    for (key, stamp) in &adopted {
        files.insert(key.clone(), *stamp);
        failures.remove(key);
    }
    let manifest = FolderManifest {
        store: adoption.store,
        source_id: source_id.to_string(),
        folder: root.display().to_string(),
        files,
        failures,
    };
    manifest.save();

    // The sources the files came from no longer claim them; listed sources
    // keep their manifests as they are (they gave nothing away).
    let listed: BTreeSet<String> = adoption
        .listed
        .iter()
        .map(|id| ManifestStore::file_id(id))
        .collect();
    for (id, mut other) in manifests {
        if listed.contains(&id) {
            continue;
        }
        let before = (other.files.len(), other.failures.len());
        other.files.retain(|key, _| !adopted.contains_key(key));
        other.failures.retain(|key, _| !adopted.contains_key(key));
        if (other.files.len(), other.failures.len()) == before {
            continue;
        }
        let saved = adoption.store.save(
            &id,
            &ManifestRef {
                folder: &other.folder,
                files: &other.files,
                failures: &other.failures,
            },
        );
        if let Err(error) = saved {
            tracing::warn!(source_id = %id, %error, "folder manifest not updated");
        }
    }

    if !adopted.is_empty() {
        tracing::info!(
            source_id,
            adopted = adopted.len(),
            "indexed files adopted instead of indexed again"
        );
    }
    Ok(Adopted {
        keys: adopted.into_keys().collect(),
        on_disk,
        manifest,
    })
}

/// Brings the unchanged file of index identity `key`, of an older index
/// format, up to date from its stored chunks ([`RAGEngine::upgrade_in_place`]).
/// Returns whether it did; otherwise the file is to be indexed again from disk.
async fn upgrade_unchanged_in_place(rag: &RwLock<RAGEngine>, key: &str) -> Result<bool, String> {
    rag.write()
        .await
        .upgrade_in_place(key)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Index the Library source `source_id` that is the single file `path` again
/// when its chunks are of an older index format (folder sources are brought
/// up to date by [`sync_folder`]). Returns whether it was indexed. A file that
/// is gone is left as it is: removing a source is the Library's decision.
pub async fn upgrade_file_source(
    path: &str,
    source_id: &str,
    rag: &RwLock<RAGEngine>,
) -> Result<bool, String> {
    if !rag.read().await.has_search_models() {
        return Err(SearchModelsMissing.to_string());
    }
    let file = canonical_path(Path::new(path));
    if !file.is_file() {
        return Ok(false);
    }
    let key = normalize_source_path(&file);
    if !stale_files(rag, source_id).await?.contains(&key) {
        return Ok(false);
    }
    if !upgrade_unchanged_in_place(rag, &key).await? {
        crate::indexing::index_single_file(path, source_id, rag, None).await?;
    }
    tracing::info!(
        source_id,
        path,
        "file source indexed again in the current format"
    );
    Ok(true)
}

/// Chunk metadata a file was indexed with by its caller (not read from the
/// file, not about one chunk): kept when the file is indexed again by
/// [`upgrade_unowned`].
const DOCUMENT_KEYS: [&str; 11] = [
    "space_id",
    "title",
    "display_path",
    "doc_type",
    "file_name",
    "filename",
    "file_type",
    "file_extension",
    "file_size_mb",
    "uploaded_at",
    "indexed_at",
];

/// What [`upgrade_unowned`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct UpgradeReport {
    /// Documents brought to the current format from their stored chunks.
    pub in_place: usize,
    /// Documents indexed again from their files.
    pub reindexed: usize,
    /// Documents left in the older format: their chunks cannot be remade
    /// without the file, and the file is gone. They stay searchable.
    pub left: Vec<String>,
}

/// Brings the documents of an older index format that no Library source
/// owns up to date: files indexed outside the Library's sources, such as
/// files added in a chat. `owned` are the Library's source ids (their files
/// are upgraded by [`sync_folder`] and [`upgrade_file_source`]); calendar
/// and other `scheme://` sources are upgraded by their own indexers. A
/// document is upgraded in place where its chunks allow it (no parsing, no
/// OCR, ids kept), else indexed again from its file with the metadata it was
/// indexed with; one whose file is gone is left as it is and reported.
pub async fn upgrade_unowned(
    rag: &RwLock<RAGEngine>,
    owned: &BTreeSet<String>,
) -> Result<UpgradeReport, String> {
    if !rag.read().await.has_search_models() {
        return Err(SearchModelsMissing.to_string());
    }
    let rows = rag
        .read()
        .await
        .stale_sources()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let mut report = UpgradeReport::default();
    for row in rows {
        if owned.contains(&row.space_id) || row.source.contains("://") {
            continue;
        }
        let upgraded = rag
            .write()
            .await
            .upgrade_in_place(&row.source)
            .await
            .map_err(|e| format!("{e:#}"))?;
        if upgraded {
            report.in_place += 1;
            continue;
        }
        let path = Path::new(&row.source);
        if !path.is_file() {
            tracing::warn!(
                source = %row.source,
                "indexed in an older format and its file is gone: left as it is"
            );
            report.left.push(row.source);
            continue;
        }
        let metadata = rag
            .read()
            .await
            .document_metadata(&row.source)
            .await
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .filter(|(key, _)| DOCUMENT_KEYS.contains(&key.as_str()))
            .collect();
        crate::indexing::index_file(rag, path, metadata).await?;
        report.reindexed += 1;
    }
    if report.in_place + report.reindexed + report.left.len() > 0 {
        tracing::info!(
            in_place = report.in_place,
            reindexed = report.reindexed,
            left = report.left.len(),
            "documents outside the Library's sources brought to the current index format"
        );
    }
    Ok(report)
}

/// A file indexed by OCR before the OCR engine was recorded with its chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyScan {
    /// The file (its index identity, a path).
    pub source: String,
    /// The Library source it belongs to (empty for files outside the Library).
    pub space_id: String,
    /// The metadata it was indexed with by its caller, to index it again with.
    pub metadata: std::collections::HashMap<String, String>,
}

/// Scans indexed before the OCR engine was recorded with their chunks: then
/// every scan was read with the system OCR, since there was no scan reader.
/// They are files whose chunks name no OCR engine and that the parser reads
/// with OCR (images, and PDFs without a text layer, checked on disk). A scan
/// whose chunks name the system OCR was read with it on purpose (JBIG2 or
/// JPEG 2000 images) or is recorded for reading again; those are not listed.
/// Files that are gone are skipped; `scheme://` sources are not files.
pub async fn legacy_ocr_scans(rag: &RwLock<RAGEngine>) -> Result<Vec<LegacyScan>, String> {
    let rows = rag
        .read()
        .await
        .documents_without_ocr_engine()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let mut scans = Vec::new();
    for row in rows {
        if row.source.contains("://") {
            continue;
        }
        let path = PathBuf::from(&row.source);
        let check = path.clone();
        let needs_ocr = tokio::task::spawn_blocking(move || {
            check.is_file() && crate::processing::parser::needs_ocr(&check)
        })
        .await
        .map_err(|e| format!("Checking {} failed: {e}", row.source))?;
        if !needs_ocr {
            continue;
        }
        let metadata = rag
            .read()
            .await
            .document_metadata(&row.source)
            .await
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .filter(|(key, _)| DOCUMENT_KEYS.contains(&key.as_str()))
            .collect();
        scans.push(LegacyScan {
            source: row.source,
            space_id: row.space_id,
            metadata,
        });
    }
    Ok(scans)
}

/// Bring the index of `source_id` in step with `folder`. The folder must
/// exist: a folder that is missing (an unplugged drive) changes nothing.
/// What the scan could not read is reported and left as it is: its files are
/// neither removed from the index nor forgotten by the manifest.
/// `stop` is asked before each file; when it says so the sync ends early with
/// its progress saved, and the next sync continues from there.
pub async fn sync_folder(
    folder: &str,
    source_id: &str,
    store: &ManifestStore,
    rag: &RwLock<RAGEngine>,
    stop: impl Fn() -> bool,
) -> Result<SyncReport, String> {
    sync_folder_scanned(folder, source_id, store, rag, stop, |root| {
        scan_folder_checked(&root)
    })
    .await
}

/// [`sync_folder`] with the folder scanned by `scan` (on a blocking thread).
async fn sync_folder_scanned(
    folder: &str,
    source_id: &str,
    store: &ManifestStore,
    rag: &RwLock<RAGEngine>,
    stop: impl Fn() -> bool,
    scan: impl FnOnce(PathBuf) -> FolderScan + Send + 'static,
) -> Result<SyncReport, String> {
    if !rag.read().await.has_search_models() {
        return Err(SearchModelsMissing.to_string());
    }
    let root = canonical_path(Path::new(folder));
    if !root.is_dir() {
        return Err(format!("{folder} is not available"));
    }
    let scanned = tokio::task::spawn_blocking({
        let root = root.clone();
        move || scan(root)
    })
    .await
    .map_err(|e| format!("Scanning {folder} failed: {e}"))?;
    let on_disk = &scanned.files;
    let (known, mut failures) = match store.load(source_id) {
        Some(manifest) => (manifest.files, manifest.failures),
        None => (
            known_from_index(rag, source_id, on_disk).await?,
            BTreeMap::new(),
        ),
    };
    let stale = stale_files(rag, source_id).await?;
    let plan = plan_for_scan(&known, &scanned, &stale);
    // Kept: files on disk, and files in what could not be read.
    let kept = |key: &String| on_disk.contains_key(key) || scanned.is_unreadable(key);
    let mut files: BTreeMap<String, FileStamp> =
        known.into_iter().filter(|(key, _)| kept(key)).collect();
    failures.retain(|key, _| kept(key));
    let unread: BTreeSet<&String> = files
        .keys()
        .chain(failures.keys())
        .filter(|key| !on_disk.contains_key(*key))
        .collect();
    let mut report = SyncReport {
        files: on_disk.len() + unread.len(),
        unreadable: scanned.unreadable.values().cloned().collect(),
        ..SyncReport::default()
    };

    let folder_name = root.display().to_string();
    // Progress is saved after every file, so a sync that is interrupted (the
    // app closed, a crash) resumes where it stopped instead of starting over.
    let save = |files: &BTreeMap<String, FileStamp>, failures: &BTreeMap<String, FileFailure>| {
        store.save(
            source_id,
            &ManifestRef {
                folder: &folder_name,
                files,
                failures,
            },
        )
    };
    let mut stopped = false;
    for key in &plan.remove {
        if stop() {
            stopped = true;
            break;
        }
        let deleted = rag
            .write()
            .await
            .delete_by_source(key)
            .await
            .map_err(|e| format!("{e:#}"))?;
        // A file that never indexed (it failed) had nothing to remove.
        if deleted > 0 {
            report.removed += 1;
        }
        save(&files, &failures)?;
    }
    for path in &plan.index {
        if stopped || stop() {
            break;
        }
        let key = normalize_source_path(path);
        let unchanged = files.get(&key).is_some_and(|stamp| {
            on_disk
                .get(&key)
                .is_some_and(|(_, on_disk)| on_disk == stamp)
        });
        let indexed = if unchanged && stale.contains(&key) {
            match upgrade_unchanged_in_place(rag, &key).await {
                Ok(true) => Ok(0),
                Ok(false) => {
                    crate::indexing::process_file_with_options(path, source_id, rag, None).await
                }
                Err(reason) => Err(reason),
            }
        } else {
            crate::indexing::process_file_with_options(path, source_id, rag, None).await
        };
        match indexed {
            Ok(_) => {
                report.indexed += 1;
                failures.remove(&key);
            }
            Err(reason) => {
                let file = path.display().to_string();
                failures.insert(key.clone(), FileFailure { file, reason });
            }
        }
        // The stamp from before indexing: a change made meanwhile is seen next time.
        if let Some((_, stamp)) = on_disk.get(&key) {
            files.insert(key, *stamp);
        }
        save(&files, &failures)?;
    }
    report.failures = failures.values().cloned().collect();
    save(&files, &failures)?;
    if report.indexed + report.removed > 0 || !report.unreadable.is_empty() {
        tracing::info!(
            source_id,
            indexed = report.indexed,
            removed = report.removed,
            failed = report.failures.len(),
            unreadable = report.unreadable.len(),
            "folder synced"
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn stamp(modified_ms: u64, size: u64) -> FileStamp {
        FileStamp { modified_ms, size }
    }

    #[test]
    fn temporary_and_lock_files_are_not_indexed() {
        for name in [
            "~$report.docx",
            "draft.tmp",
            "paper.pdf.part",
            "paper.pdf.crdownload",
            ".~lock.notes.odt#",
            "notes.TMP",
        ] {
            assert!(!is_indexable(Path::new(name)), "{name}");
        }
        for name in ["report.docx", "paper.pdf", "notes.md", "tmp.md", "part.txt"] {
            assert!(is_indexable(Path::new(name)), "{name}");
        }
        assert!(!is_indexable(Path::new("binary.exe")));
    }

    #[test]
    fn the_plan_indexes_new_and_changed_files_and_removes_missing_ones() {
        let known = BTreeMap::from([
            ("a".to_string(), stamp(1, 10)),
            ("b".to_string(), stamp(1, 10)),
            ("c".to_string(), stamp(1, 10)),
        ]);
        let on_disk = BTreeMap::from([
            ("a".to_string(), (PathBuf::from("A"), stamp(1, 10))),
            ("b".to_string(), (PathBuf::from("B"), stamp(2, 10))),
            ("d".to_string(), (PathBuf::from("D"), stamp(1, 5))),
        ]);
        assert_eq!(
            plan(&known, &on_disk, &BTreeSet::new()),
            SyncPlan {
                index: vec![PathBuf::from("B"), PathBuf::from("D")],
                remove: vec!["c".to_string()],
            }
        );
        // A size change alone counts as a change.
        let resized = BTreeMap::from([("a".to_string(), (PathBuf::from("A"), stamp(1, 11)))]);
        assert_eq!(
            plan(&known, &resized, &BTreeSet::new()).index,
            vec![PathBuf::from("A")]
        );
        // An unchanged file of an older index format is indexed again; one
        // that is gone is only removed.
        let stale = BTreeSet::from(["a".to_string(), "c".to_string()]);
        assert_eq!(
            plan(&known, &on_disk, &stale),
            SyncPlan {
                index: vec![PathBuf::from("A"), PathBuf::from("B"), PathBuf::from("D")],
                remove: vec!["c".to_string()],
            }
        );
    }

    fn unreadable(path: &str) -> FileFailure {
        FileFailure {
            file: path.to_string(),
            reason: "Access is denied.".to_string(),
        }
    }

    #[test]
    fn nothing_in_what_could_not_be_read_is_removed() {
        let known = BTreeMap::from([
            ("c:/papers/a.pdf".to_string(), stamp(1, 10)),
            ("c:/papers/private/b.pdf".to_string(), stamp(1, 10)),
            ("c:/papers/private/deep/c.pdf".to_string(), stamp(1, 10)),
            ("c:/papers/private 2/d.pdf".to_string(), stamp(1, 10)),
            ("c:/papers/locked.pdf".to_string(), stamp(1, 10)),
            ("c:/papers/gone.pdf".to_string(), stamp(1, 10)),
        ]);
        let scan = FolderScan {
            files: BTreeMap::from([(
                "c:/papers/a.pdf".to_string(),
                (PathBuf::from("A"), stamp(1, 10)),
            )]),
            unreadable: BTreeMap::from([
                (
                    "c:/papers/private".to_string(),
                    unreadable("C:/papers/private"),
                ),
                (
                    "c:/papers/locked.pdf".to_string(),
                    unreadable("C:/papers/locked.pdf"),
                ),
            ]),
        };
        // Only what is known to be gone goes: "private 2" is another folder.
        assert_eq!(
            plan_for_scan(&known, &scan, &BTreeSet::new()),
            SyncPlan {
                index: Vec::new(),
                remove: vec![
                    "c:/papers/gone.pdf".to_string(),
                    "c:/papers/private 2/d.pdf".to_string(),
                ],
            }
        );
        assert!(scan.is_unreadable("c:/papers/private/deep/c.pdf"));
        assert!(!scan.is_unreadable("c:/papers/private 2/d.pdf"));
        assert!(!scan.is_unreadable("c:/papers/privatex.pdf"));
        // A folder at a drive's root covers what is below it.
        let root = FolderScan {
            unreadable: BTreeMap::from([("d:/".to_string(), unreadable("D:/"))]),
            ..FolderScan::default()
        };
        assert!(root.is_unreadable("d:/papers/a.pdf"));
        assert!(!root.is_unreadable("e:/papers/a.pdf"));
    }

    /// A subfolder that cannot be read (permissions, a share that dropped)
    /// is not taken as deleted: its files keep their chunks and their
    /// manifest entries, and the sync reports it. The walk's error is
    /// simulated: the scan finds the folder as a walk that could not open
    /// it does.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unreadable_subfolder_is_not_taken_as_deleted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = dir.path().join("papers");
        std::fs::create_dir_all(folder.join("private")).expect("folder");
        std::fs::write(
            folder.join("lease.txt"),
            "The lease notice period is sixty days for either party. Rent is reviewed every spring, and the landlord repairs the roof and the heating when they fail.",
        )
        .expect("file");
        std::fs::write(
            folder.join("private").join("parking.md"),
            "Visitors park on the street; residents use the garage below the courtyard, and bicycles are kept in the shed next to the main entrance of the building.",
        )
        .expect("file");
        let folder_text = folder.display().to_string();
        let first = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((first.indexed, first.files), (2, 2), "{first:?}");

        let private = canonical_path(&folder.join("private"));
        let private_key = normalize_source_path(&private);
        let unreadable_scan = move |root: PathBuf| {
            let mut scan = scan_folder_checked(&root);
            scan.files.retain(|key, _| !is_within(key, &private_key));
            scan.unreadable.insert(
                private_key.clone(),
                FileFailure {
                    file: private.display().to_string(),
                    reason: "Access is denied. (os error 5)".to_string(),
                },
            );
            scan
        };
        let blocked = sync_folder_scanned(
            &folder_text,
            "s1",
            &store,
            &rag,
            || false,
            unreadable_scan.clone(),
        )
        .await
        .expect("sync");
        assert_eq!((blocked.indexed, blocked.removed), (0, 0), "{blocked:?}");
        assert_eq!(blocked.files, 2, "the unread file still counts");
        assert_eq!(blocked.unreadable.len(), 1);
        assert!(blocked.unreadable[0].file.ends_with("private"));
        assert_eq!(
            indexed(&rag, "s1").await.keys().collect::<Vec<_>>(),
            ["lease.txt", "parking.md"]
        );

        // Kept in the manifest too: readable again and unchanged, nothing to do.
        let again =
            sync_folder_scanned(&folder_text, "s1", &store, &rag, || false, unreadable_scan)
                .await
                .expect("sync");
        assert_eq!((again.indexed, again.removed), (0, 0));
        let readable = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!(
            (
                readable.indexed,
                readable.removed,
                readable.unreadable.len()
            ),
            (0, 0, 0),
            "{readable:?}"
        );
    }

    async fn engine(dir: &Path) -> Arc<RwLock<RAGEngine>> {
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.join("data");
        config.embedding.model_dir = dir.join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        config.search.min_score_threshold = 0.0;
        let mut engine = RAGEngine::new(config).await.expect("engine");
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(Arc::new(
                crate::statements::testing::WordEmbedder::default(),
            )))
            .expect("models");
        Arc::new(RwLock::new(engine))
    }

    async fn indexed(rag: &RwLock<RAGEngine>, source_id: &str) -> BTreeMap<String, usize> {
        rag.read()
            .await
            .document_sources()
            .await
            .expect("sources")
            .into_iter()
            .filter(|row| row.space_id == source_id)
            .map(|row| {
                let name = row
                    .source
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                (name, row.chunks)
            })
            .collect()
    }

    async fn texts(rag: &RwLock<RAGEngine>, query: &str) -> Vec<String> {
        rag.read()
            .await
            .search(query, 10)
            .await
            .expect("search")
            .into_iter()
            .map(|r| r.text)
            .collect()
    }

    /// Changes the file's modification time without waiting for the clock.
    fn touch(path: &Path, text: &str, offset_secs: u64) {
        std::fs::write(path, text).expect("write");
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open");
        file.set_modified(
            std::time::SystemTime::now() + std::time::Duration::from_secs(offset_secs),
        )
        .expect("mtime");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_folder_stays_in_step_with_the_index() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = dir.path().join("papers");
        std::fs::create_dir_all(folder.join("sub")).expect("folder");
        std::fs::write(
            folder.join("lease.txt"),
            "The lease notice period is sixty days for either party. Rent is reviewed every spring, and the landlord repairs the roof and the heating when they fail.",
        )
        .expect("file");
        std::fs::write(
            folder.join("sub").join("parking.md"),
            "Visitors park on the street; residents use the garage below the courtyard, and bicycles are kept in the shed next to the main entrance of the building.",
        )
        .expect("file");
        std::fs::write(folder.join("~$lease.docx"), "lock").expect("lock file");
        std::fs::write(folder.join("download.pdf.crdownload"), "partial").expect("partial");
        let folder_text = folder.display().to_string();

        // Indexed in full before manifests existed (the lock file and the
        // partial download are skipped, not failed): nothing to do.
        let full = crate::indexing::index_folder(
            &folder_text,
            "s1",
            &crate::indexing::IndexingOptions {
                skip_indexed: false,
                watch_changes: false,
                process_subdirs: true,
                priority: "normal".into(),
                file_types: Vec::new(),
            },
            &rag,
            &crate::indexing::IndexingState::default(),
            None,
        )
        .await
        .expect("indexed");
        assert_eq!((full.files_processed, full.failures.len()), (2, 0));
        assert_eq!(
            indexed(&rag, "s1").await.keys().collect::<Vec<_>>(),
            ["lease.txt", "parking.md"]
        );
        let first = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!(
            first,
            SyncReport {
                files: 2,
                ..SyncReport::default()
            }
        );

        // Added, changed, deleted and renamed.
        std::fs::write(
            folder.join("rules.txt"),
            "Quiet hours begin at ten in the evening and end at seven in the morning; parties need a week of notice to the neighbours and the building manager.",
        )
        .expect("add");
        touch(
            &folder.join("lease.txt"),
            "The lease notice period is ninety days for either party. Rent is reviewed every spring, and the landlord repairs the roof and the heating when they fail.",
            60,
        );
        std::fs::rename(
            folder.join("sub").join("parking.md"),
            folder.join("sub").join("garage.md"),
        )
        .expect("rename");
        std::fs::write(folder.join("~$rules.docx"), "lock").expect("lock file");
        let second = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!(second.indexed, 3, "{second:?}");
        assert_eq!(second.removed, 1, "{second:?}");
        assert_eq!(second.files, 3);
        assert_eq!(
            indexed(&rag, "s1").await.keys().collect::<Vec<_>>(),
            ["garage.md", "lease.txt", "rules.txt"]
        );
        let lease = texts(&rag, "lease notice period").await;
        assert!(lease.iter().any(|t| t.contains("ninety")), "{lease:?}");
        assert!(!lease.iter().any(|t| t.contains("sixty")), "{lease:?}");

        // Unchanged: nothing is indexed again.
        let third = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((third.indexed, third.removed), (0, 0));

        // A file that cannot be indexed is reported, and tried again only
        // once it changes.
        std::fs::write(folder.join("broken.pdf"), "not a pdf").expect("broken");
        let broken = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!(
            (broken.indexed, broken.failures.len()),
            (0, 1),
            "{broken:?}"
        );
        assert!(broken.failures[0].file.ends_with("broken.pdf"));
        let again = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!(again.failures, broken.failures);
        assert_eq!((again.indexed, again.removed), (0, 0));

        // A file whose record is dropped is read again on the next sync (as
        // after the scan reader was installed).
        let broken_key = normalize_source_path(&folder.join("broken.pdf"));
        assert_eq!(store.forget_file("s1", &broken_key), Ok(true));
        assert_eq!(store.forget_file("s1", &broken_key), Ok(false));
        assert_eq!(store.forget_file("unknown", &broken_key), Ok(false));
        let retried = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!(
            retried.failures, broken.failures,
            "tried again: {retried:?}"
        );
        let lease_key = normalize_source_path(&folder.join("lease.txt"));
        assert_eq!(store.forget_file("s1", &lease_key), Ok(true));
        let reread = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((reread.indexed, reread.removed), (1, 0), "{reread:?}");

        // Deleted.
        std::fs::remove_file(folder.join("rules.txt")).expect("delete");
        std::fs::remove_file(folder.join("broken.pdf")).expect("delete");
        let fourth = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((fourth.indexed, fourth.removed), (0, 1));
        assert!(fourth.failures.is_empty());
        assert_eq!(
            indexed(&rag, "s1").await.keys().collect::<Vec<_>>(),
            ["garage.md", "lease.txt"]
        );

        // A missing folder (an unplugged drive) removes nothing.
        let gone = dir.path().join("unplugged");
        assert!(
            sync_folder(&gone.display().to_string(), "s1", &store, &rag, || false)
                .await
                .is_err()
        );
        assert_eq!(indexed(&rag, "s1").await.len(), 2);
    }

    const PASSAGE: &str = "Quiet hours begin at ten in the evening and end at seven in the morning; parties need a week of notice to the neighbours and the building manager.";

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hidden_folders_and_files_are_skipped_and_dropped() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = dir.path().join("docs");
        std::fs::create_dir_all(folder.join(".claude")).expect("hidden dir");
        std::fs::create_dir_all(folder.join(".git").join("logs")).expect("hidden dir");
        std::fs::write(folder.join("rules.txt"), PASSAGE).expect("file");
        std::fs::write(folder.join(".claude").join("notes.md"), PASSAGE).expect("hidden file");
        std::fs::write(folder.join(".git").join("logs").join("head.txt"), PASSAGE)
            .expect("hidden file");
        std::fs::write(folder.join(".draft.txt"), PASSAGE).expect("hidden file");
        let folder_text = folder.display().to_string();

        assert_eq!(scan_folder(&canonical_path(&folder)).len(), 1);
        let report = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((report.indexed, report.files), (1, 1));
        assert_eq!(
            indexed(&rag, "s1").await.keys().collect::<Vec<_>>(),
            ["rules.txt"]
        );

        // A hidden file indexed before hidden entries were skipped is removed.
        crate::indexing::process_file_with_options(
            &folder.join(".claude").join("notes.md"),
            "s1",
            &rag,
            None,
        )
        .await
        .expect("indexed");
        let mut manifest = store.load("s1").expect("manifest");
        let hidden_key =
            normalize_source_path(&canonical_path(&folder.join(".claude").join("notes.md")));
        manifest.files.insert(
            hidden_key,
            FileStamp {
                modified_ms: 1,
                size: 1,
            },
        );
        store
            .save(
                "s1",
                &ManifestRef {
                    folder: &manifest.folder,
                    files: &manifest.files,
                    failures: &manifest.failures,
                },
            )
            .expect("save");
        let again = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((again.indexed, again.removed), (0, 1));
        assert_eq!(
            indexed(&rag, "s1").await.keys().collect::<Vec<_>>(),
            ["rules.txt"]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_interrupted_sync_resumes_where_it_stopped() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = dir.path().join("docs");
        std::fs::create_dir_all(&folder).expect("folder");
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(folder.join(name), format!("{name}: {PASSAGE}")).expect("file");
        }
        let folder_text = folder.display().to_string();

        // Stopped after the first file: that file's progress is kept.
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let stop_after_one = || asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1;
        let partial = sync_folder(&folder_text, "s1", &store, &rag, stop_after_one)
            .await
            .expect("sync");
        assert_eq!((partial.indexed, partial.files), (1, 3));
        assert_eq!(indexed(&rag, "s1").await.len(), 1);

        // The next sync indexes only what is left, then nothing.
        let rest = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!(rest.indexed, 2);
        assert_eq!(indexed(&rag, "s1").await.len(), 3);
        let settled = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((settled.indexed, settled.removed), (0, 0));
    }

    /// Indexes `path` into `source_id` as a build of index format 1 did.
    async fn index_as_format_one(rag: &RwLock<RAGEngine>, path: &Path, source_id: &str) {
        let metadata =
            std::collections::HashMap::from([("space_id".to_string(), source_id.to_string())]);
        let prepared = rag
            .read()
            .await
            .file_preparer()
            .expect("preparer")
            .prepare(&canonical_path(path), metadata)
            .expect("prepared");
        rag.write()
            .await
            .commit_file_as_format_one(prepared)
            .await
            .expect("stored");
    }

    async fn stale_count(rag: &RwLock<RAGEngine>) -> usize {
        rag.read().await.stale_sources().await.expect("stale").len()
    }

    /// A library indexed by an earlier version (unchanged files, a manifest
    /// that matches the disk) is indexed again in the current format on its
    /// first sync, scans from the OCR cache; after that, nothing to do.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_folder_indexed_in_an_older_format_is_indexed_again_once() {
        let dir = tempfile::tempdir().expect("temp dir");
        // PP-OCRv6 only, and it is not installed: a scan can be read only from
        // the OCR cache, so reading it again with OCR would fail the file.
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.path().join("data");
        config.embedding.model_dir = dir.path().join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        config.search.min_score_threshold = 0.0;
        config.ocr.engine = crate::config::OcrEngineChoice::PpOcr;
        let cache = crate::processing::ocr::cache::OcrCache::new(
            config.data_dir.join(crate::rag_engine::OCR_CACHE_DIR),
        );
        let mut engine = RAGEngine::new(config).await.expect("engine");
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(Arc::new(
                crate::statements::testing::WordEmbedder::default(),
            )))
            .expect("models");
        let rag = Arc::new(RwLock::new(engine));
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = dir.path().join("library");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("lease.txt"), PASSAGE).expect("file");
        let scan = folder.join("lease-scan.pdf");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(
                "../shodh-eval/data/scanned/corpus/contracts/lease-saffron-tidewater-scan.pdf",
            ),
            &scan,
        )
        .expect("scan");
        cache
            .put(&crate::processing::parser::tests::cached_scan(
                &canonical_path(&scan),
                "Monthly rent is INR 4,85,000, payable by the 5th day of each month.",
            ))
            .expect("cached");
        // A scan indexed before OCR was cached: read from the cache here,
        // which is then emptied of it.
        let old_scan = folder.join("msa-scan.pdf");
        std::fs::copy(&scan, &old_scan).expect("scan");
        let old_key = normalize_source_path(&canonical_path(&old_scan));
        cache
            .put(&crate::processing::parser::tests::cached_scan(
                &canonical_path(&old_scan),
                "The courts at Bengaluru have exclusive jurisdiction.",
            ))
            .expect("cached");
        let folder_text = folder.display().to_string();

        for file in ["lease.txt", "lease-scan.pdf", "msa-scan.pdf"] {
            index_as_format_one(&rag, &folder.join(file), "s1").await;
        }
        assert!(cache.remove(&old_key));
        let on_disk = scan_folder(&canonical_path(&folder));
        let files: BTreeMap<String, FileStamp> = on_disk
            .iter()
            .map(|(key, (_, stamp))| (key.clone(), *stamp))
            .collect();
        store
            .save(
                "s1",
                &ManifestRef {
                    folder: &folder_text,
                    files: &files,
                    failures: &BTreeMap::new(),
                },
            )
            .expect("manifest");
        assert_eq!(stale_count(&rag).await, 3);

        // Reading either scan with OCR would fail the file: PP-OCRv6 is not installed.
        let upgraded = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!(
            (upgraded.indexed, upgraded.removed, upgraded.failures.len()),
            (3, 0, 0),
            "{upgraded:?}"
        );
        assert_eq!(stale_count(&rag).await, 0);
        assert_eq!(indexed(&rag, "s1").await.len(), 3, "replaced, not added");
        let rent = texts(&rag, "monthly rent").await;
        assert!(rent.iter().any(|t| t.contains("4,85,000")), "{rent:?}");
        let courts = texts(&rag, "courts jurisdiction").await;
        assert!(courts.iter().any(|t| t.contains("Bengaluru")), "{courts:?}");
        assert!(
            cache
                .get(&normalize_source_path(&canonical_path(&scan)))
                .is_some(),
            "the cached scan stays cached"
        );

        let settled = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((settled.indexed, settled.removed), (0, 0));
    }

    /// Scans indexed before the OCR engine was recorded with their chunks are
    /// found by looking at the files; text files, text PDFs, scans whose
    /// chunks name their engine and files that are gone are not listed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scans_indexed_before_the_ocr_engine_was_recorded_are_found() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.path().join("data");
        config.embedding.model_dir = dir.path().join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        config.ocr.engine = crate::config::OcrEngineChoice::PpOcr;
        let cache = crate::processing::ocr::cache::OcrCache::new(
            config.data_dir.join(crate::rag_engine::OCR_CACHE_DIR),
        );
        let mut engine = RAGEngine::new(config).await.expect("engine");
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(Arc::new(
                crate::statements::testing::WordEmbedder::default(),
            )))
            .expect("models");
        let rag = Arc::new(RwLock::new(engine));
        let folder = dir.path().join("library");
        std::fs::create_dir_all(&folder).expect("folder");
        let corpus_scan = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../shodh-eval/data/scanned/corpus/contracts/lease-saffron-tidewater-scan.pdf");
        let text_pdf = || {
            let lines: Vec<_> = (0..30)
                .map(|i| {
                    crate::processing::pdf_fixtures::text(
                        72.0,
                        740.0 - 20.0 * i as f32,
                        11.0,
                        "Either party may end this agreement with sixty days written notice.",
                    )
                })
                .collect();
            crate::processing::pdf_fixtures::build_pdf(&[lines], None)
        };

        std::fs::write(folder.join("lease.txt"), PASSAGE).expect("file");
        std::fs::write(folder.join("notice.pdf"), text_pdf()).expect("text pdf");
        // A scan read now: its chunks name the engine that read it.
        let recorded = folder.join("lease-scan.pdf");
        std::fs::copy(&corpus_scan, &recorded).expect("scan");
        cache
            .put(&crate::processing::parser::tests::cached_scan(
                &canonical_path(&recorded),
                "Monthly rent is INR 4,85,000, payable by the 5th day of each month.",
            ))
            .expect("cached");
        // A scan as an earlier build indexed it: chunks without an engine.
        // Indexed here from a text PDF, then the scan takes its place on disk.
        let legacy = folder.join("msa-scan.pdf");
        std::fs::write(&legacy, text_pdf()).expect("legacy");
        for file in ["lease.txt", "notice.pdf", "lease-scan.pdf", "msa-scan.pdf"] {
            let metadata = std::collections::HashMap::from([
                ("space_id".to_string(), "s1".to_string()),
                ("title".to_string(), file.to_string()),
            ]);
            crate::indexing::index_file(&rag, &canonical_path(&folder.join(file)), metadata)
                .await
                .expect("indexed");
        }
        std::fs::copy(&corpus_scan, &legacy).expect("scan in place");

        let scans = legacy_ocr_scans(&rag).await.expect("listed");
        assert_eq!(
            scans
                .iter()
                .map(|s| s.source.rsplit('/').next().unwrap_or_default())
                .collect::<Vec<_>>(),
            ["msa-scan.pdf"]
        );
        assert_eq!(scans[0].space_id, "s1");
        assert_eq!(
            scans[0].metadata.get("title").map(String::as_str),
            Some("msa-scan.pdf"),
            "indexed again with the metadata it was indexed with"
        );

        std::fs::remove_file(&legacy).expect("delete");
        assert!(legacy_ocr_scans(&rag).await.expect("listed").is_empty());
    }

    /// A folder indexed by an earlier version before manifests existed is
    /// brought up to date the same way.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_folder_without_a_manifest_is_upgraded_too() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = dir.path().join("docs");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("a.txt"), PASSAGE).expect("file");
        index_as_format_one(&rag, &folder.join("a.txt"), "s1").await;
        let folder_text = folder.display().to_string();

        let upgraded = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((upgraded.indexed, upgraded.files), (1, 1));
        assert_eq!(stale_count(&rag).await, 0);
        let settled = sync_folder(&folder_text, "s1", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!(settled.indexed, 0);
    }

    /// A Library source that is one file is indexed again only when it is of
    /// an older format.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_file_source_is_indexed_again_only_in_an_older_format() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let file = dir.path().join("lease.txt");
        std::fs::write(&file, PASSAGE).expect("file");
        let path = file.display().to_string();
        index_as_format_one(&rag, &file, "f1").await;
        // Another source's stale file is not this source's business.
        let other = dir.path().join("other.txt");
        std::fs::write(&other, PASSAGE).expect("file");
        index_as_format_one(&rag, &other, "f2").await;

        assert_eq!(upgrade_file_source(&path, "f1", &rag).await, Ok(true));
        assert_eq!(stale_count(&rag).await, 1, "only f1 was indexed again");
        assert_eq!(indexed(&rag, "f1").await.len(), 1);
        assert_eq!(upgrade_file_source(&path, "f1", &rag).await, Ok(false));
        let gone = dir.path().join("gone.txt").display().to_string();
        assert_eq!(upgrade_file_source(&gone, "f3", &rag).await, Ok(false));
    }

    /// Documents no Library source owns are upgraded in place where their
    /// chunks allow, else from their file; one whose file is gone is left
    /// and reported. Owned sources and calendar items are not touched.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn documents_outside_the_library_are_upgraded_too() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(&uploads).expect("folder");
        // Structure chunks (Markdown): upgraded in place even once the file is gone.
        let notes = uploads.join("notes.md");
        std::fs::write(&notes, format!("# Notes\n\n{PASSAGE}\n")).expect("file");
        index_as_format_one(&rag, &notes, "").await;
        std::fs::remove_file(&notes).expect("removed");
        // Window chunks (plain text): indexed again from the file.
        let text = uploads.join("rules.txt");
        std::fs::write(&text, PASSAGE).expect("file");
        index_as_format_one(&rag, &text, "").await;
        // Window chunks whose file is gone: left.
        let gone = uploads.join("gone.txt");
        std::fs::write(&gone, PASSAGE).expect("file");
        index_as_format_one(&rag, &gone, "").await;
        std::fs::remove_file(&gone).expect("removed");
        // A Library source's file: its sync's business.
        let owned = uploads.join("owned.txt");
        std::fs::write(&owned, PASSAGE).expect("file");
        index_as_format_one(&rag, &owned, "lib1").await;

        let report = upgrade_unowned(&rag, &BTreeSet::from(["lib1".to_string()]))
            .await
            .expect("upgrade");
        assert_eq!(
            report,
            UpgradeReport {
                in_place: 1,
                reindexed: 1,
                left: vec![normalize_source_path(&canonical_path(&gone))],
            }
        );
        let stale: BTreeSet<String> = rag
            .read()
            .await
            .stale_sources()
            .await
            .expect("stale")
            .into_iter()
            .map(|row| row.source)
            .collect();
        assert_eq!(
            stale,
            BTreeSet::from([
                normalize_source_path(&canonical_path(&gone)),
                normalize_source_path(&canonical_path(&owned)),
            ])
        );
    }

    fn add_options(skip_indexed: bool) -> crate::indexing::IndexingOptions {
        crate::indexing::IndexingOptions {
            skip_indexed,
            watch_changes: false,
            process_subdirs: true,
            priority: "normal".into(),
            file_types: Vec::new(),
        }
    }

    /// Adds `folder` to the Library as the folder source `source_id`.
    async fn add_folder(
        rag: &RwLock<RAGEngine>,
        store: &ManifestStore,
        folder: &Path,
        source_id: &str,
        skip_indexed: bool,
        listed: &BTreeSet<String>,
    ) -> crate::indexing::IndexingResult {
        crate::indexing::index_folder_adopting(
            &folder.display().to_string(),
            source_id,
            &add_options(skip_indexed),
            rag,
            &crate::indexing::IndexingState::default(),
            None,
            Some(Adoption { store, listed }),
        )
        .await
        .expect("indexed")
    }

    /// Chunk ids by file name: a file indexed again gets new ones.
    async fn chunk_ids(rag: &RwLock<RAGEngine>) -> BTreeMap<String, BTreeSet<String>> {
        let hits = rag
            .read()
            .await
            .list_documents_raw(None, 100_000)
            .await
            .expect("chunks");
        let mut ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for hit in hits {
            let name = hit
                .source
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            ids.entry(name).or_default().insert(hit.id);
        }
        ids
    }

    /// A folder of two files synced by the source `old`, which has their
    /// manifest: a source the Library has since lost.
    async fn folder_of_old_source(
        dir: &Path,
        rag: &RwLock<RAGEngine>,
        store: &ManifestStore,
    ) -> PathBuf {
        let folder = dir.join("papers");
        std::fs::create_dir_all(&folder).expect("folder");
        for name in ["a.txt", "b.txt"] {
            std::fs::write(folder.join(name), format!("{name}: {PASSAGE}")).expect("file");
        }
        let synced = sync_folder(&folder.display().to_string(), "old", store, rag, || false)
            .await
            .expect("sync");
        assert_eq!(synced.indexed, 2);
        folder
    }

    fn in_space(space_id: &str) -> crate::types::MetadataFilter {
        crate::types::MetadataFilter {
            space_id: Some(space_id.to_string()),
            ..Default::default()
        }
    }

    /// Adding a folder whose files a lost source indexed takes them over:
    /// nothing is indexed again, the files and their manifest entries move
    /// to the new source, its first sync has nothing to do, and a search
    /// limited to it finds them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_folder_added_again_adopts_its_indexed_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = folder_of_old_source(dir.path(), &rag, &store).await;
        let before = chunk_ids(&rag).await;

        let added = add_folder(&rag, &store, &folder, "new", true, &BTreeSet::new()).await;
        assert_eq!(
            (
                added.files_processed,
                added.files_unchanged,
                added.failures.len()
            ),
            (0, 2, 0),
            "{added:?}"
        );
        assert_eq!(chunk_ids(&rag).await, before, "nothing was indexed again");
        assert_eq!(
            indexed(&rag, "new").await.keys().collect::<Vec<_>>(),
            ["a.txt", "b.txt"]
        );
        assert!(indexed(&rag, "old").await.is_empty());
        let a_key = normalize_source_path(&canonical_path(&folder.join("a.txt")));
        let metadata = rag
            .read()
            .await
            .document_metadata(&a_key)
            .await
            .expect("metadata");
        assert_eq!(
            metadata.get("space_id").map(String::as_str),
            Some("new"),
            "the chunks' metadata names the new owner too"
        );

        let stamps: BTreeMap<String, FileStamp> = scan_folder(&canonical_path(&folder))
            .into_iter()
            .map(|(key, (_, stamp))| (key, stamp))
            .collect();
        assert_eq!(store.load("new").expect("manifest").files, stamps);
        let old = store.load("old").expect("manifest");
        assert!(old.files.is_empty() && old.failures.is_empty());

        let synced = sync_folder(&folder.display().to_string(), "new", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((synced.indexed, synced.removed, synced.files), (0, 0, 2));

        let found = rag
            .read()
            .await
            .search_comprehensive("quiet hours evening", 10, Some(in_space("new")))
            .await
            .expect("search");
        assert!(!found.is_empty());
        assert!(found
            .iter()
            .all(|r| r.metadata.get("space_id").map(String::as_str) == Some("new")));
        let outside = rag
            .read()
            .await
            .search_comprehensive("quiet hours evening", 10, Some(in_space("old")))
            .await
            .expect("search");
        assert!(outside.is_empty(), "{outside:?}");
    }

    /// A file changed since it was indexed is indexed again; the others are
    /// adopted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_changed_file_is_indexed_again_when_its_folder_is_added() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = folder_of_old_source(dir.path(), &rag, &store).await;
        let before = chunk_ids(&rag).await;
        touch(
            &folder.join("b.txt"),
            &format!("b.txt: {PASSAGE} The garden gate is locked at night."),
            60,
        );

        let added = add_folder(&rag, &store, &folder, "new", true, &BTreeSet::new()).await;
        assert_eq!(
            (added.files_processed, added.files_unchanged),
            (1, 1),
            "{added:?}"
        );
        let after = chunk_ids(&rag).await;
        assert_eq!(after["a.txt"], before["a.txt"]);
        assert_ne!(after["b.txt"], before["b.txt"]);
        assert_eq!(indexed(&rag, "new").await.len(), 2);
        assert!(indexed(&rag, "old").await.is_empty());
        let synced = sync_folder(&folder.display().to_string(), "new", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((synced.indexed, synced.removed), (0, 0));
    }

    /// Saves a manifest for `source_id` recording the files of `folder` as
    /// they are on disk now.
    fn record_as_on_disk(store: &ManifestStore, source_id: &str, folder: &Path) {
        let files: BTreeMap<String, FileStamp> = scan_folder(&canonical_path(folder))
            .into_iter()
            .map(|(key, (_, stamp))| (key, stamp))
            .collect();
        store
            .save(
                source_id,
                &ManifestRef {
                    folder: &folder.display().to_string(),
                    files: &files,
                    failures: &BTreeMap::new(),
                },
            )
            .expect("manifest");
    }

    /// A file of an older index format is indexed again, not adopted, even
    /// when its manifest shows it unchanged; its current neighbour is adopted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_file_of_an_older_format_is_not_adopted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = dir.path().join("docs");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("a.txt"), format!("a.txt: {PASSAGE}")).expect("file");
        std::fs::write(folder.join("b.txt"), format!("b.txt: {PASSAGE}")).expect("file");
        index_as_format_one(&rag, &folder.join("a.txt"), "old").await;
        crate::indexing::process_file_with_options(
            &canonical_path(&folder.join("b.txt")),
            "old",
            &rag,
            None,
        )
        .await
        .expect("indexed");
        record_as_on_disk(&store, "old", &folder);
        let before = chunk_ids(&rag).await;

        let added = add_folder(&rag, &store, &folder, "new", true, &BTreeSet::new()).await;
        assert_eq!(
            (added.files_processed, added.files_unchanged),
            (1, 1),
            "{added:?}"
        );
        let after = chunk_ids(&rag).await;
        assert_ne!(after["a.txt"], before["a.txt"], "indexed again");
        assert_eq!(after["b.txt"], before["b.txt"], "adopted");
        assert_eq!(stale_count(&rag).await, 0);
        assert_eq!(indexed(&rag, "new").await.len(), 2);
        assert!(indexed(&rag, "old").await.is_empty());
    }

    /// Without a manifest nothing shows that indexed files are current (a
    /// single-file source, a file added in a chat, a source whose manifest
    /// is gone): they are indexed again, a changed one with its new content,
    /// exactly as before adoption existed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn files_of_a_source_without_a_manifest_are_indexed_again() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = dir.path().join("docs");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("a.txt"), format!("a.txt: {PASSAGE}")).expect("file");
        std::fs::write(
            folder.join("b.txt"),
            "The lease notice period is sixty days for either party. Rent is reviewed every spring, and the landlord repairs the roof and the heating when they fail.",
        )
        .expect("file");
        for (name, owner) in [("a.txt", "file-a"), ("b.txt", "")] {
            crate::indexing::process_file_with_options(
                &canonical_path(&folder.join(name)),
                owner,
                &rag,
                None,
            )
            .await
            .expect("indexed");
        }
        // Changed after it was indexed.
        touch(
            &folder.join("b.txt"),
            "The lease notice period is ninety days for either party. Rent is reviewed every spring, and the landlord repairs the roof and the heating when they fail.",
            60,
        );
        let before = chunk_ids(&rag).await;

        let added = add_folder(&rag, &store, &folder, "new", true, &BTreeSet::new()).await;
        assert_eq!(
            (added.files_processed, added.files_unchanged),
            (2, 0),
            "{added:?}"
        );
        let after = chunk_ids(&rag).await;
        for name in ["a.txt", "b.txt"] {
            assert!(
                after[name].is_disjoint(&before[name]),
                "{name} indexed again"
            );
        }
        assert_eq!(indexed(&rag, "new").await.len(), 2);
        let lease = texts(&rag, "lease notice period").await;
        assert!(lease.iter().any(|t| t.contains("ninety")), "{lease:?}");
        assert!(!lease.iter().any(|t| t.contains("sixty")), "{lease:?}");
        let synced = sync_folder(&folder.display().to_string(), "new", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!((synced.indexed, synced.removed), (0, 0));
    }

    /// A source the Library lists keeps its files, and indexing a source
    /// again (without `skip_indexed`) indexes every file and leaves the
    /// manifests alone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn listed_sources_and_forced_runs_adopt_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = engine(dir.path()).await;
        let store = ManifestStore::in_dir(dir.path().join("sync"));
        let folder = folder_of_old_source(dir.path(), &rag, &store).await;

        let listed = BTreeSet::from(["old".to_string()]);
        let kept = add_folder(&rag, &store, &folder, "new", true, &listed).await;
        assert_eq!((kept.files_processed, kept.files_unchanged), (2, 0));
        assert_eq!(
            store.load("old").expect("manifest").files.len(),
            2,
            "a listed source's manifest is left as it is"
        );
        assert_eq!(
            store.load("new").expect("manifest").files.len(),
            2,
            "the files indexed are recorded"
        );
        let synced = sync_folder(&folder.display().to_string(), "new", &store, &rag, || false)
            .await
            .expect("sync");
        assert_eq!(synced.indexed, 0);

        let before = chunk_ids(&rag).await;
        let forced = add_folder(&rag, &store, &folder, "other", false, &BTreeSet::new()).await;
        assert_eq!((forced.files_processed, forced.files_unchanged), (2, 0));
        let after = chunk_ids(&rag).await;
        for name in ["a.txt", "b.txt"] {
            assert!(
                after[name].is_disjoint(&before[name]),
                "{name} indexed again"
            );
        }
        assert!(store.load("other").is_none(), "no manifest written");
        assert_eq!(store.load("new").expect("manifest").files.len(), 2);
        assert_eq!(indexed(&rag, "other").await.len(), 2);
    }
}
