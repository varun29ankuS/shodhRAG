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
    /// Indexable files in the folder now.
    pub files: usize,
    /// Files of the folder that could not be indexed (this sync or before,
    /// and unchanged since).
    pub failures: Vec<FileFailure>,
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

/// The indexable files under `folder`, keyed by index identity
/// ([`normalize_source_path`]).
pub fn scan_folder(folder: &Path) -> BTreeMap<String, (PathBuf, FileStamp)> {
    WalkDir::new(folder)
        .into_iter()
        .filter_entry(|e| !is_hidden(e))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && is_indexable(e.path()))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            let path = canonical_path(entry.path());
            let stamp = FileStamp {
                modified_ms,
                size: meta.len(),
            };
            Some((normalize_source_path(&path), (path, stamp)))
        })
        .collect()
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

    fn path(&self, source_id: &str) -> PathBuf {
        let safe: String = source_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        self.dir.join(format!("{safe}.json"))
    }

    fn load(&self, source_id: &str) -> Option<Manifest> {
        let text = std::fs::read_to_string(self.path(source_id)).ok()?;
        serde_json::from_str(&text).ok()
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
/// `stop` is asked before each file; when it says so the sync ends early with
/// its progress saved, and the next sync continues from there.
pub async fn sync_folder(
    folder: &str,
    source_id: &str,
    store: &ManifestStore,
    rag: &RwLock<RAGEngine>,
    stop: impl Fn() -> bool,
) -> Result<SyncReport, String> {
    if !rag.read().await.has_search_models() {
        return Err(SearchModelsMissing.to_string());
    }
    let root = canonical_path(Path::new(folder));
    if !root.is_dir() {
        return Err(format!("{folder} is not available"));
    }
    let scan_root = root.clone();
    let on_disk = tokio::task::spawn_blocking(move || scan_folder(&scan_root))
        .await
        .map_err(|e| format!("Scanning {folder} failed: {e}"))?;
    let (known, mut failures) = match store.load(source_id) {
        Some(manifest) => (manifest.files, manifest.failures),
        None => (
            known_from_index(rag, source_id, &on_disk).await?,
            BTreeMap::new(),
        ),
    };
    let stale = stale_files(rag, source_id).await?;
    let plan = plan(&known, &on_disk, &stale);
    let mut report = SyncReport {
        files: on_disk.len(),
        ..SyncReport::default()
    };
    let mut files: BTreeMap<String, FileStamp> = known
        .into_iter()
        .filter(|(key, _)| on_disk.contains_key(key))
        .collect();

    failures.retain(|key, _| on_disk.contains_key(key));
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
    for key in &plan.remove {
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
        if stop() {
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
    if report.indexed + report.removed > 0 {
        tracing::info!(
            source_id,
            indexed = report.indexed,
            removed = report.removed,
            failed = report.failures.len(),
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
}
