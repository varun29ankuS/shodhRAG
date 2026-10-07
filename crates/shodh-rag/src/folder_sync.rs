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
/// stamps the files had when they were last indexed.
pub fn plan(
    known: &BTreeMap<String, FileStamp>,
    on_disk: &BTreeMap<String, (PathBuf, FileStamp)>,
) -> SyncPlan {
    SyncPlan {
        index: on_disk
            .iter()
            .filter(|(key, (_, stamp))| known.get(*key) != Some(stamp))
            .map(|(_, (path, _))| path.clone())
            .collect(),
        remove: known
            .keys()
            .filter(|key| !on_disk.contains_key(*key))
            .cloned()
            .collect(),
    }
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

    fn save(&self, source_id: &str, manifest: &Manifest) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.path(source_id);
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string(manifest).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
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

/// Bring the index of `source_id` in step with `folder`. The folder must
/// exist: a folder that is missing (an unplugged drive) changes nothing.
pub async fn sync_folder(
    folder: &str,
    source_id: &str,
    store: &ManifestStore,
    rag: &RwLock<RAGEngine>,
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
    let plan = plan(&known, &on_disk);
    let mut report = SyncReport {
        files: on_disk.len(),
        ..SyncReport::default()
    };
    let mut files: BTreeMap<String, FileStamp> = known
        .into_iter()
        .filter(|(key, _)| on_disk.contains_key(key))
        .collect();

    failures.retain(|key, _| on_disk.contains_key(key));
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
    }
    for path in &plan.index {
        let key = normalize_source_path(path);
        match crate::indexing::process_file_with_options(path, source_id, rag).await {
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
    }
    report.failures = failures.values().cloned().collect();
    store.save(
        source_id,
        &Manifest {
            folder: root.display().to_string(),
            files,
            failures,
        },
    )?;
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
            plan(&known, &on_disk),
            SyncPlan {
                index: vec![PathBuf::from("B"), PathBuf::from("D")],
                remove: vec!["c".to_string()],
            }
        );
        // A size change alone counts as a change.
        let resized = BTreeMap::from([("a".to_string(), (PathBuf::from("A"), stamp(1, 11)))]);
        assert_eq!(plan(&known, &resized).index, vec![PathBuf::from("A")]);
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
        let first = sync_folder(&folder_text, "s1", &store, &rag)
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
        let second = sync_folder(&folder_text, "s1", &store, &rag)
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
        let third = sync_folder(&folder_text, "s1", &store, &rag)
            .await
            .expect("sync");
        assert_eq!((third.indexed, third.removed), (0, 0));

        // A file that cannot be indexed is reported, and tried again only
        // once it changes.
        std::fs::write(folder.join("broken.pdf"), "not a pdf").expect("broken");
        let broken = sync_folder(&folder_text, "s1", &store, &rag)
            .await
            .expect("sync");
        assert_eq!(
            (broken.indexed, broken.failures.len()),
            (0, 1),
            "{broken:?}"
        );
        assert!(broken.failures[0].file.ends_with("broken.pdf"));
        let again = sync_folder(&folder_text, "s1", &store, &rag)
            .await
            .expect("sync");
        assert_eq!(again.failures, broken.failures);
        assert_eq!((again.indexed, again.removed), (0, 0));

        // Deleted.
        std::fs::remove_file(folder.join("rules.txt")).expect("delete");
        std::fs::remove_file(folder.join("broken.pdf")).expect("delete");
        let fourth = sync_folder(&folder_text, "s1", &store, &rag)
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
        assert!(sync_folder(&gone.display().to_string(), "s1", &store, &rag)
            .await
            .is_err());
        assert_eq!(indexed(&rag, "s1").await.len(), 2);
    }
}
