//! Batch indexing pipeline for files and folders.
//!
//! Provides folder preview, single-file indexing, and batch indexing with
//! pause/resume/cancel support. Progress is emitted via the EventEmitter trait
//! so the caller (Tauri, HTTP server, CLI) can deliver updates to its UI.

use chrono::Utc;
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use walkdir::WalkDir;

use crate::chat::EventEmitter;
use crate::embeddings::SearchModelsMissing;
use crate::rag_engine::RAGEngine;

// ── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub name: String,
    #[serde(rename = "type")]
    pub file_type: String,
    pub size: u64,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingProgress {
    /// The source being indexed, so concurrent jobs can be told apart.
    pub space_id: String,
    pub current_file: String,
    pub processed_files: usize,
    pub total_files: usize,
    pub percentage: f32,
    pub current_action: String,
    pub eta_seconds: f32,
    pub speed: f32,
    /// While OCR reads a scanned PDF: the page being read (1-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_page: Option<usize>,
    /// While OCR reads a scanned PDF: its pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_pages: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingOptions {
    pub skip_indexed: bool,
    pub watch_changes: bool,
    pub process_subdirs: bool,
    pub priority: String,
    pub file_types: Vec<String>,
}

/// A file that could not be indexed, with the reason it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFailure {
    pub file: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingResult {
    pub files_processed: usize,
    pub total_chunks: usize,
    /// Paths of files that failed. Kept for compatibility; see `failures` for reasons.
    pub failed_files: Vec<String>,
    /// Per-file failure reasons, in the same order as `failed_files`.
    #[serde(default)]
    pub failures: Vec<FileFailure>,
    /// Files that were already indexed, unchanged, and were not indexed
    /// again (adopted from another source, see [`index_folder_adopting`]).
    /// Not counted in `files_processed`.
    #[serde(default)]
    pub files_unchanged: usize,
    pub duration: u64,
    /// The run was cancelled, or its source removed, before every file was
    /// indexed: the files counted were stored, the rest were not read.
    #[serde(default)]
    pub stopped: bool,
}

impl IndexingResult {
    fn empty(duration: u64) -> Self {
        Self {
            files_processed: 0,
            total_chunks: 0,
            failed_files: Vec::new(),
            failures: Vec::new(),
            files_unchanged: 0,
            duration,
            stopped: false,
        }
    }
}

/// Whether indexing is paused: one switch for every run (folders, single
/// files, the agent's jobs, folder syncs), shared by the engine's
/// [`IndexingState`] and the app's tray. The app keeps it in its settings,
/// so indexing that was paused stays paused after a restart.
#[derive(Debug, Clone)]
pub struct PauseSwitch(Arc<tokio::sync::watch::Sender<bool>>);

impl Default for PauseSwitch {
    fn default() -> Self {
        Self::new(false)
    }
}

impl PauseSwitch {
    pub fn new(paused: bool) -> Self {
        Self(Arc::new(tokio::sync::watch::Sender::new(paused)))
    }

    pub fn is_paused(&self) -> bool {
        *self.0.borrow()
    }

    /// Pauses or resumes; returns whether that changed anything.
    pub fn set(&self, paused: bool) -> bool {
        self.0.send_if_modified(|current| {
            let changed = *current != paused;
            *current = paused;
            changed
        })
    }

    /// Waits while indexing is paused.
    pub async fn wait_until_resumed(&self) {
        let mut paused = self.0.subscribe();
        // The sender lives in `self`, so the wait ends only on a resume.
        let _ = paused.wait_for(|paused| !*paused).await;
    }
}

/// One run registered in [`IndexingState`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct RunEntry {
    id: u64,
    path: Option<String>,
}

/// Shared state for pause/cancel signalling across async boundaries.
///
/// Pause ([`PauseSwitch`]) holds every run before its next file or batch.
/// Cancelling stops the runs in progress, of one source
/// ([`Self::cancel_source`]) or all of them ([`Self::cancel`]), before their
/// next batch; what they stored stays stored, and a run started afterwards
/// is not affected. A Library source that is removed stops only its own runs
/// ([`Self::stop_source`]) and none of its runs starts again: each run of a
/// folder registers under its source while it lasts ([`Self::begin_run`]).
#[derive(Debug)]
pub struct IndexingState {
    pause: PauseSwitch,
    /// The runs cancelled while in progress (their ids); an id goes when its
    /// run ends.
    cancelled: Mutex<HashSet<u64>>,
    /// Sources removed from the Library: their runs stop before their next
    /// batch and none of theirs starts again.
    removed: Mutex<HashSet<String>>,
    /// The runs in progress by source, each with the folder or file it
    /// indexes when it said so ([`Self::begin_run_at`]).
    runs: tokio::sync::watch::Sender<HashMap<String, Vec<RunEntry>>>,
    next_run: std::sync::atomic::AtomicU64,
}

impl Default for IndexingState {
    fn default() -> Self {
        Self::with_pause(PauseSwitch::default())
    }
}

/// A run of [`IndexingState::begin_run`]: registered under its source until
/// it is dropped.
#[derive(Debug)]
pub struct IndexingRun<'a> {
    state: &'a IndexingState,
    source_id: String,
    id: u64,
}

impl IndexingRun<'_> {
    /// Whether this run stops: it was cancelled, or its source was removed.
    pub fn stopped(&self) -> bool {
        self.state.is_removed(&self.source_id)
            || self
                .state
                .cancelled
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&self.id)
    }

    /// Waits while indexing is paused; returns early when the run stops.
    /// Returns whether the run stopped.
    pub async fn wait_while_paused(&self) -> bool {
        while self.state.is_paused() {
            if self.stopped() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.stopped()
    }
}

impl Drop for IndexingRun<'_> {
    fn drop(&mut self) {
        self.state
            .cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        self.state.runs.send_modify(|runs| {
            if let Some(entries) = runs.get_mut(&self.source_id) {
                entries.retain(|e| e.id != self.id);
                if entries.is_empty() {
                    runs.remove(&self.source_id);
                }
            }
        });
    }
}

impl IndexingState {
    /// A state whose pause is `pause` (shared with the app's tray).
    pub fn with_pause(pause: PauseSwitch) -> Self {
        Self {
            pause,
            cancelled: Mutex::new(HashSet::new()),
            removed: Mutex::new(HashSet::new()),
            runs: tokio::sync::watch::Sender::new(HashMap::new()),
            next_run: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// The pause switch.
    pub fn pause_switch(&self) -> &PauseSwitch {
        &self.pause
    }

    pub fn pause(&self) {
        self.pause.set(true);
    }

    pub fn resume(&self) {
        self.pause.set(false);
    }

    pub fn is_paused(&self) -> bool {
        self.pause.is_paused()
    }

    /// Cancels every run in progress (each stops before its next batch).
    /// Returns how many there were.
    pub fn cancel(&self) -> usize {
        let ids: Vec<u64> = self
            .runs
            .borrow()
            .values()
            .flatten()
            .map(|e| e.id)
            .collect();
        let mut cancelled = self.cancelled.lock().unwrap_or_else(|e| e.into_inner());
        cancelled.extend(ids.iter().copied());
        ids.len()
    }

    /// Cancels the runs of `source_id` in progress (each stops before its
    /// next batch); its later runs are not affected. Returns how many there
    /// were.
    pub fn cancel_source(&self, source_id: &str) -> usize {
        let ids: Vec<u64> = self
            .runs
            .borrow()
            .get(source_id)
            .map(|entries| entries.iter().map(|e| e.id).collect())
            .unwrap_or_default();
        let mut cancelled = self.cancelled.lock().unwrap_or_else(|e| e.into_inner());
        cancelled.extend(ids.iter().copied());
        ids.len()
    }

    /// Registers a run of `source_id` until the returned guard is dropped;
    /// `None` when the source was removed (the run must not start).
    pub fn begin_run(&self, source_id: &str) -> Option<IndexingRun<'_>> {
        self.register(source_id, None)
    }

    /// [`Self::begin_run`] for a run that indexes the folder (or file)
    /// `path`, which [`Self::running_paths`] reports while it lasts.
    pub fn begin_run_at(&self, source_id: &str, path: &str) -> Option<IndexingRun<'_>> {
        self.register(source_id, Some(path.to_string()))
    }

    fn register(&self, source_id: &str, path: Option<String>) -> Option<IndexingRun<'_>> {
        // Checked and registered under the same lock as `stop_source`
        // records a removal, so a removal waits for every run it can miss.
        let removed = self.removed.lock().unwrap_or_else(|e| e.into_inner());
        if removed.contains(source_id) {
            return None;
        }
        let id = self
            .next_run
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.runs.send_modify(|runs| {
            runs.entry(source_id.to_string())
                .or_default()
                .push(RunEntry { id, path });
        });
        drop(removed);
        Some(IndexingRun {
            state: self,
            source_id: source_id.to_string(),
            id,
        })
    }

    /// The folders (or files) being indexed now, with their sources, for the
    /// runs that said which ([`Self::begin_run_at`]).
    pub fn running_paths(&self) -> Vec<(String, String)> {
        self.runs
            .borrow()
            .iter()
            .flat_map(|(source_id, entries)| {
                entries
                    .iter()
                    .filter_map(|e| e.path.clone())
                    .map(move |path| (source_id.clone(), path))
            })
            .collect()
    }

    /// Whether `source_id` was removed from the Library.
    pub fn is_removed(&self, source_id: &str) -> bool {
        self.removed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(source_id)
    }

    /// Whether every run of `source_id` in progress stops: it was removed,
    /// or each of its runs was cancelled.
    pub fn should_stop(&self, source_id: &str) -> bool {
        if self.is_removed(source_id) {
            return true;
        }
        let runs = self.runs.borrow();
        let Some(entries) = runs.get(source_id).filter(|e| !e.is_empty()) else {
            return false;
        };
        let cancelled = self.cancelled.lock().unwrap_or_else(|e| e.into_inner());
        entries.iter().all(|e| cancelled.contains(&e.id))
    }

    /// Stops the runs of `source_id`, a source removed from the Library, and
    /// waits until they have ended: each ends before its next batch, a
    /// paused one at once. Runs of other sources go on. No run of
    /// `source_id` starts afterwards.
    pub async fn stop_source(&self, source_id: &str) {
        self.removed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(source_id.to_string());
        let mut runs = self.runs.subscribe();
        // The sender lives in `self`, so the wait ends only when the runs do.
        let _ = runs.wait_for(|runs| !runs.contains_key(source_id)).await;
    }

    /// Records `source_id` as removed from the Library (in an earlier run):
    /// none of its runs starts. Unlike [`Self::stop_source`] it waits for
    /// nothing; it is for sources removed before any run could start.
    pub fn mark_removed(&self, source_id: &str) {
        self.removed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(source_id.to_string());
    }

    /// Lets `source_id` be indexed again after a removal that failed
    /// ([`Self::stop_source`]).
    pub fn restore_source(&self, source_id: &str) {
        self.removed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(source_id);
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Check if a path is a file or directory.
pub fn check_path_type(path: &str) -> Result<(bool, bool), String> {
    let path_buf = PathBuf::from(path);
    if !path_buf.exists() {
        return Err(format!("Path does not exist: {}", path));
    }
    Ok((path_buf.is_dir(), path_buf.is_file()))
}

/// Index a single file into a space.
///
/// Returns (chunks_created, duration_ms).
pub async fn index_single_file(
    file_path: &str,
    space_id: &str,
    rag: &RwLock<RAGEngine>,
    emitter: Option<&dyn EventEmitter>,
) -> Result<IndexingResult, String> {
    if !rag.read().await.has_search_models() {
        return Err(SearchModelsMissing.to_string());
    }
    let start_time = Instant::now();
    let path = crate::rag_engine::canonical_path(Path::new(file_path));

    if !path.exists() {
        return Err(format!("File does not exist: {}", file_path));
    }
    if !path.is_file() {
        return Err(format!("Path is not a file: {}", file_path));
    }

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("unknown")
        .to_lowercase();

    if !is_supported_file_type(&extension) {
        return Err(format!("Unsupported file type: {}", extension));
    }

    emit_progress(emitter, space_id, file_path, 0, 1, 0.0, "Reading file...");
    emit_progress(emitter, space_id, file_path, 0, 1, 50.0, "Indexing...");

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown");

    let mut metadata = HashMap::new();
    metadata.insert("space_id".to_string(), space_id.to_string());
    metadata.insert("file_path".to_string(), file_path.to_string());
    metadata.insert("file_name".to_string(), file_name.to_string());
    metadata.insert("file_type".to_string(), extension.clone());
    metadata.insert("file_extension".to_string(), extension.clone());
    metadata.insert("filename".to_string(), file_name.to_string());
    metadata.insert("doc_type".to_string(), "document".to_string());
    metadata.insert("indexed_at".to_string(), Utc::now().to_rfc3339());

    // While a scan is read the bar moves from half way to the end.
    let on_page = |page: usize, pages: usize| {
        let read = (page - 1) as f32 / pages.max(1) as f32;
        emit_scanned_page(
            emitter,
            space_id,
            file_path,
            (0, 1),
            50.0 + 50.0 * read,
            (page, pages),
        );
    };
    let ids = index_file_with_progress(rag, &path, metadata, Some(&on_page))
        .await
        .map_err(|e| format!("Failed to index file: {e}"))?;

    let chunks_created = ids.len();

    emit_progress(emitter, space_id, file_path, 1, 1, 100.0, "Complete!");

    let duration = start_time.elapsed().as_millis() as u64;

    Ok(IndexingResult {
        files_processed: 1,
        total_chunks: chunks_created,
        failed_files: Vec::new(),
        failures: Vec::new(),
        files_unchanged: 0,
        duration,
        stopped: false,
    })
}

/// Batch-index a folder into a space with pause/resume/cancel support.
/// Every file is indexed; `options.skip_indexed` needs the folder manifests
/// ([`index_folder_adopting`]).
pub async fn index_folder(
    folder_path: &str,
    space_id: &str,
    options: &IndexingOptions,
    rag: &RwLock<RAGEngine>,
    indexing_state: &IndexingState,
    emitter: Option<&dyn EventEmitter>,
) -> Result<IndexingResult, String> {
    index_folder_adopting(
        folder_path,
        space_id,
        options,
        rag,
        indexing_state,
        emitter,
        None,
    )
    .await
}

/// How a folder run groups its files for embedding and storing.
///
/// Files are read and chunked one after another, up to `read_ahead` files
/// ahead of the batch being embedded, so reading the next file overlaps with
/// embedding the current batch while memory stays bounded. A batch takes the
/// files that are ready, up to `max_files`, and no more once it has
/// `max_chunks` chunks (a file is never split): the chunks of small files go
/// to the model together, filling its batches, and the batch is stored with
/// one write and one keyword-index commit ([`RAGEngine::commit_files`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchLimits {
    pub max_files: usize,
    pub max_chunks: usize,
    pub read_ahead: usize,
}

impl Default for BatchLimits {
    /// 64 chunks fill the model's batches three times over (a batch holds
    /// about 21 chunks of 380 tokens, `embeddings::e5`); 16 files bound what
    /// one failed write costs; 4 files of text read ahead are a few
    /// megabytes at most.
    fn default() -> Self {
        Self {
            max_files: 16,
            max_chunks: 64,
            read_ahead: 4,
        }
    }
}

/// The files of the next batch: `first`, then the files `next` has ready,
/// until the batch has `limits.max_files` files or `limits.max_chunks`
/// chunks (`weight` is a file's chunks; a file is never split).
fn fill_batch<T>(
    first: T,
    mut next: impl FnMut() -> Option<T>,
    weight: impl Fn(&T) -> usize,
    limits: &BatchLimits,
) -> Vec<T> {
    let mut chunks = weight(&first);
    let mut batch = vec![first];
    while batch.len() < limits.max_files && chunks < limits.max_chunks {
        match next() {
            Some(item) => {
                chunks += weight(&item);
                batch.push(item);
            }
            None => break,
        }
    }
    batch
}

/// A file of a folder run, read and chunked (or not).
struct ReadFile {
    path: PathBuf,
    outcome: Result<crate::rag_engine::ChunkedFile, String>,
}

/// What a folder run has done so far.
struct Tally<'a> {
    files_processed: usize,
    total_chunks: usize,
    failures: Vec<FileFailure>,
    adopted: Option<crate::folder_sync::Adopted<'a>>,
}

impl Tally<'_> {
    /// `path` is done: indexed with `chunks`, or failed with a reason. Only
    /// a file whose chunks were stored counts as indexed.
    fn finish(&mut self, path: &Path, outcome: Result<usize, String>) {
        let file = path.to_string_lossy().to_string();
        // The stamp from before indexing: a change made meanwhile is seen
        // by the source's next sync.
        if let Some(adopted) = self.adopted.as_mut() {
            let key = crate::rag_engine::normalize_source_path(path);
            if let Some((_, stamp)) = adopted.on_disk.get(&key) {
                let failure = outcome.as_ref().err().map(|reason| FileFailure {
                    file: file.clone(),
                    reason: reason.clone(),
                });
                adopted.manifest.record(key, *stamp, failure);
            }
        }
        match outcome {
            Ok(chunks) => {
                self.files_processed += 1;
                self.total_chunks += chunks;
            }
            Err(reason) => {
                tracing::warn!(
                    file = %crate::log_safe::file(&file),
                    reason = %crate::log_safe::scrub(&reason, &file),
                    "Failed to index file"
                );
                self.failures.push(FileFailure { file, reason });
            }
        }
    }
}

/// [`index_folder`] for a Library folder source, given its folder manifests
/// (`adoption`). With `options.skip_indexed`, the files of the folder that
/// are already indexed under another source, unchanged and in the current
/// index format, are adopted instead of indexed again
/// ([`crate::folder_sync::adopt_indexed`]): they keep their chunks, become
/// this source's, and are counted in `files_unchanged`. The source's
/// manifest is then kept as the other files are indexed, so its first sync
/// starts from it. Without `skip_indexed` (indexing a source again, which
/// is forced) every file is indexed and no manifest is touched.
pub async fn index_folder_adopting(
    folder_path: &str,
    space_id: &str,
    options: &IndexingOptions,
    rag: &RwLock<RAGEngine>,
    indexing_state: &IndexingState,
    emitter: Option<&dyn EventEmitter>,
    adoption: Option<crate::folder_sync::Adoption<'_>>,
) -> Result<IndexingResult, String> {
    index_folder_batched(
        folder_path,
        space_id,
        options,
        rag,
        indexing_state,
        emitter,
        adoption,
        BatchLimits::default(),
    )
    .await
}

/// [`index_folder_adopting`] with its files grouped by `limits`.
///
/// Pausing holds the run before it reads its next file and before it
/// embeds its next batch. Cancelling it, or removing its source, stops it
/// before its next batch: the batch being embedded is stored, files read
/// but not embedded are dropped and not counted, and the result says it
/// stopped. A file counts as indexed only once its batch is stored.
#[allow(clippy::too_many_arguments)]
pub async fn index_folder_batched(
    folder_path: &str,
    space_id: &str,
    options: &IndexingOptions,
    rag: &RwLock<RAGEngine>,
    indexing_state: &IndexingState,
    emitter: Option<&dyn EventEmitter>,
    adoption: Option<crate::folder_sync::Adoption<'_>>,
    limits: BatchLimits,
) -> Result<IndexingResult, String> {
    // Without the embedding model every file would be parsed and then fail.
    if !rag.read().await.has_search_models() {
        return Err(SearchModelsMissing.to_string());
    }
    let start_time = Instant::now();
    // One spelling for every path that leaves this function (progress,
    // failures, metadata), however the folder was typed.
    let path = crate::rag_engine::canonical_path(Path::new(folder_path));

    if !path.exists() {
        return Err(format!("Path does not exist: {}", folder_path));
    }
    if !path.is_dir() {
        return Err(format!("Path is not a directory: {}", folder_path));
    }

    // Registered while it runs, so removing its source stops it and waits
    // for it (see `IndexingState::stop_source`), and cancelling stops it.
    let Some(run) = indexing_state.begin_run_at(space_id, &path.display().to_string()) else {
        return Err(format!(
            "{folder_path} was removed from the Library and is not indexed"
        ));
    };

    emit_progress(
        emitter,
        space_id,
        "Starting...",
        0,
        0,
        0.0,
        "Initializing indexing",
    );

    // Collect files to process
    let mut files_to_process = Vec::new();

    for entry in WalkDir::new(&path)
        .into_iter()
        .filter_entry(|e| !crate::folder_sync::is_hidden(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file_path = entry.path();
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("unknown")
            .to_lowercase();

        if is_selected_file_type(options, &extension)
            && !crate::folder_sync::is_temporary_file(file_path)
        {
            files_to_process.push(file_path.to_path_buf());
        }

        if run.stopped() {
            return Ok(IndexingResult {
                stopped: true,
                ..IndexingResult::empty(start_time.elapsed().as_millis() as u64)
            });
        }
    }

    let total_files = files_to_process.len();

    if total_files == 0 {
        return Ok(IndexingResult::empty(
            start_time.elapsed().as_millis() as u64
        ));
    }

    // Files already indexed under another source are adopted, not indexed
    // again. Adoption that cannot run (the index unreadable) adopts nothing:
    // every file is indexed, as without it.
    let adopted = match adoption.filter(|_| options.skip_indexed) {
        Some(adoption) => {
            match crate::folder_sync::adopt_indexed(
                &path,
                space_id,
                &files_to_process,
                adoption,
                rag,
            )
            .await
            {
                Ok(adopted) => Some(adopted),
                Err(error) => {
                    tracing::warn!(space_id, %error, "indexed files not adopted; indexing every file");
                    None
                }
            }
        }
        None => None,
    };
    let files_unchanged = adopted.as_ref().map_or(0, |a| a.keys.len());
    if files_unchanged > 0 {
        // Below 100: the run is not finished until the last event.
        emit_progress(
            emitter,
            space_id,
            "",
            files_unchanged,
            total_files,
            (files_unchanged as f32 / total_files as f32 * 100.0).min(99.9),
            &format!("{files_unchanged} of {total_files} already indexed"),
        );
    }
    let to_index: Vec<PathBuf> = files_to_process
        .into_iter()
        .filter(|file| {
            let key = crate::rag_engine::normalize_source_path(file);
            !adopted.as_ref().is_some_and(|a| a.keys.contains(&key))
        })
        .collect();

    let preparer = rag
        .read()
        .await
        .file_preparer()
        .map_err(|e| format!("{e:#}"))?;
    let mut tally = Tally {
        files_processed: 0,
        total_chunks: 0,
        failures: Vec::new(),
        adopted,
    };
    // Files done (indexed now, failed, or adopted), for the progress events
    // the reader sends while it reads a scan.
    let done = std::sync::atomic::AtomicUsize::new(files_unchanged);
    let indexing_started = Instant::now();
    let (tx, rx) = tokio::sync::mpsc::channel::<ReadFile>(limits.read_ahead.max(1));

    let reader = async {
        let tx = tx;
        // The first file is reported at once.
        let mut last_progress: Option<Instant> = None;
        for (index, file) in to_index.iter().enumerate() {
            if run.wait_while_paused().await {
                break;
            }
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let finished = done.load(std::sync::atomic::Ordering::Relaxed);
            if last_progress.is_none_or(|at| at.elapsed() > Duration::from_millis(100)) {
                emit_rated_progress(
                    emitter,
                    space_id,
                    &name,
                    (finished, total_files),
                    &format!(
                        "Reading file {} of {}",
                        files_unchanged + index + 1,
                        total_files
                    ),
                    rate_since(indexing_started, finished - files_unchanged),
                );
                last_progress = Some(Instant::now());
            }
            // While a scan is read, its pages move the bar within the file's share.
            let on_page = |page: usize, pages: usize| {
                let read = (page - 1) as f32 / pages.max(1) as f32;
                let finished = done.load(std::sync::atomic::Ordering::Relaxed);
                emit_scanned_page(
                    emitter,
                    space_id,
                    &name,
                    (finished, total_files),
                    (finished as f32 + read) / total_files as f32 * 100.0,
                    (page, pages),
                );
            };
            let outcome = read_folder_file(&preparer, rag, file, space_id, Some(&on_page)).await;
            let sent = tx
                .send(ReadFile {
                    path: file.clone(),
                    outcome,
                })
                .await;
            if sent.is_err() {
                // The batches stopped: nothing more is read.
                break;
            }
        }
    };

    let batches = async {
        let mut rx = rx;
        let mut stopped = false;
        while let Some(first) = rx.recv().await {
            // Only the files read already: the batch is not held for a file
            // still being read.
            let reads = fill_batch(
                first,
                || rx.try_recv().ok(),
                |read: &ReadFile| read.outcome.as_ref().map_or(0, |c| c.chunk_count()),
                &limits,
            );
            let mut batch: Vec<(PathBuf, crate::rag_engine::ChunkedFile)> = Vec::new();
            for read in reads {
                match read.outcome {
                    Err(reason) => {
                        tally.finish(&read.path, Err(reason));
                        done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(chunked) => batch.push((read.path, chunked)),
                }
            }
            if batch.is_empty() {
                continue;
            }
            if run.wait_while_paused().await {
                stopped = true;
                break;
            }
            let (paths, chunked): (Vec<PathBuf>, Vec<_>) = batch.into_iter().unzip();
            let embedder = preparer.clone();
            let embedded = tokio::task::spawn_blocking(move || embedder.embed_all(chunked)).await;
            let embedded = match embedded {
                Ok(results) => results,
                Err(e) => {
                    let reason = match e.try_into_panic() {
                        Ok(panic) => format!("Panic: {}", panic_message(panic.as_ref())),
                        Err(e) => format!("Indexing task failed: {e}"),
                    };
                    for path in &paths {
                        tally.finish(path, Err(format!("Failed to process file: {reason}")));
                    }
                    done.fetch_add(paths.len(), std::sync::atomic::Ordering::Relaxed);
                    continue;
                }
            };
            let mut ready = Vec::new();
            let mut ready_paths = Vec::new();
            for (path, result) in paths.into_iter().zip(embedded) {
                match result {
                    Ok(prepared) => {
                        ready.push(prepared);
                        ready_paths.push(path);
                    }
                    Err(e) => {
                        tally.finish(&path, Err(format!("Failed to process file: {e:#}")));
                        done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            if !ready.is_empty() {
                let committed = rag.write().await.commit_files(ready).await;
                for (path, result) in ready_paths.iter().zip(committed) {
                    tally.finish(
                        path,
                        result
                            .map(|ids| ids.len())
                            .map_err(|e| format!("Failed to process file: {e:#}")),
                    );
                }
                done.fetch_add(ready_paths.len(), std::sync::atomic::Ordering::Relaxed);
            }
            let finished = done.load(std::sync::atomic::Ordering::Relaxed);
            let current = ready_paths
                .last()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("");
            emit_rated_progress(
                emitter,
                space_id,
                current,
                (finished, total_files),
                &format!("Indexed {finished} of {total_files}"),
                rate_since(indexing_started, tally.files_processed),
            );
        }
        stopped || run.stopped()
    };

    let ((), stopped) = futures::join!(reader, batches);

    emit_progress(
        emitter,
        space_id,
        COMPLETED_FILE,
        tally.files_processed + files_unchanged,
        total_files,
        100.0,
        if stopped {
            "Indexing stopped"
        } else {
            "Indexing complete"
        },
    );

    if !tally.failures.is_empty() {
        tracing::warn!(
            failed = tally.failures.len(),
            processed = tally.files_processed,
            unchanged = files_unchanged,
            total = total_files,
            "Folder indexing finished with failures"
        );
    }
    if stopped {
        tracing::info!(
            processed = tally.files_processed,
            total = total_files,
            "Folder indexing stopped before its end"
        );
    }

    Ok(IndexingResult {
        files_processed: tally.files_processed,
        total_chunks: tally.total_chunks,
        failed_files: tally.failures.iter().map(|f| f.file.clone()).collect(),
        failures: tally.failures,
        files_unchanged,
        duration: start_time.elapsed().as_millis() as u64,
        stopped,
    })
}

/// The `current_file` of a run's last event.
pub const COMPLETED_FILE: &str = "Completed";

/// Reads and chunks one file of a folder run on a blocking thread (with the
/// metadata every folder file gets), passing OCR's progress through a scan
/// to `on_page`. A scan read, or refused, without the scan reader is
/// reported to the engine, as for a single file.
async fn read_folder_file(
    preparer: &crate::rag_engine::FilePreparer,
    rag: &RwLock<RAGEngine>,
    path: &Path,
    space_id: &str,
    on_page: Option<&ScannedPageFn<'_>>,
) -> Result<crate::rag_engine::ChunkedFile, String> {
    if !path.exists() {
        return Err(format!("File does not exist: {}", path.display()));
    }
    let metadata = folder_file_metadata(path, space_id);
    let reported = metadata.clone();
    let owned = path.to_path_buf();
    let reader = preparer.clone();
    let (pages_tx, mut pages_rx) = tokio::sync::mpsc::unbounded_channel::<(usize, usize)>();
    let task = tokio::task::spawn_blocking(move || {
        reader.chunk_with_progress(&owned, metadata, &|page, pages| {
            // The receiver is gone only when the caller stopped waiting.
            let _ = pages_tx.send((page, pages));
        })
    });
    tokio::pin!(task);
    let joined = loop {
        tokio::select! {
            joined = &mut task => break joined,
            Some((page, pages)) = pages_rx.recv() => {
                if let Some(on_page) = on_page {
                    on_page(page, pages);
                }
            }
        }
    };
    let chunked = joined.map_err(|e| match e.try_into_panic() {
        Ok(panic) => format!("Panic: {}", panic_message(panic.as_ref())),
        Err(e) => format!("Indexing task failed: {e}"),
    })?;
    match chunked {
        Ok(chunked) => {
            if chunked.scan_reader_missing() {
                rag.read().await.report_scan_reader_missing(path, reported);
            }
            if let Some(notice) = chunked.notice() {
                tracing::warn!(file = %crate::log_safe::file(&path), "Indexed with pages missing: {notice}");
            }
            Ok(chunked)
        }
        Err(e) => {
            if e.is::<crate::processing::ocr::ScanReaderMissing>() && queued_when_refused(&reported)
            {
                rag.read().await.report_scan_reader_missing(path, reported);
            }
            Err(format!("Failed to process file: {e:#}"))
        }
    }
}

/// Files a second since `started`, for `files` indexed.
fn rate_since(started: Instant, files: usize) -> f32 {
    let elapsed = started.elapsed().as_secs_f32();
    if elapsed > 0.0 {
        files as f32 / elapsed
    } else {
        0.0
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Every file extension (lowercase, without the dot) the ingest pipeline can
/// index. Shared with the app so allow-lists are never duplicated.
pub const SUPPORTED_FILE_EXTENSIONS: &[&str] = &[
    // Documents
    "txt", "md", "pdf", "html", "htm", "json", "docx", "pptx", "rst", "tex", // Tabular
    "csv", "tsv", "xlsx", "xls", "xlsm", "xlsb", "ods", // Code
    "rs", "py", "js", "ts", "jsx", "tsx", "java", "cpp", "c", "h", "hpp", "cs", "go", "rb", "php",
    "swift", "kt", "scala", "r", "sh", "bash", "zsh", "ps1", "bat", "cmd", // Web
    "css", "scss", "sass", "less", "vue", "svelte", // Config / data
    "toml", "yaml", "yml", "ini", "conf", "config", "env", "xml", "sql", "graphql", "proto",
    // Images (OCR)
    "png", "jpg", "jpeg", "bmp", "tiff", "tif",
];

/// Whether `extension` (lowercase, without the dot) can be indexed.
pub fn is_supported_file_type(extension: &str) -> bool {
    SUPPORTED_FILE_EXTENSIONS.contains(&extension)
}

/// Whether a file with `extension` should be indexed under `options`.
/// `options.file_types` is an optional restriction: an empty list means
/// "every supported type"; a non-empty list narrows the supported set.
fn is_selected_file_type(options: &IndexingOptions, extension: &str) -> bool {
    is_supported_file_type(extension)
        && (options.file_types.is_empty()
            || options
                .file_types
                .iter()
                .any(|t| t.trim_start_matches('.').eq_ignore_ascii_case(extension)))
}

/// Index one file of a folder source (the metadata every folder file gets),
/// reporting OCR's progress through a scan to `on_page` (page, pages).
pub(crate) async fn process_file_with_options(
    file_path: &Path,
    space_id: &str,
    rag: &RwLock<RAGEngine>,
    on_page: Option<&ScannedPageFn<'_>>,
) -> Result<usize, String> {
    if !file_path.exists() {
        return Err(format!("File does not exist: {}", file_path.display()));
    }

    let metadata = folder_file_metadata(file_path, space_id);
    let ids = index_file_with_progress(rag, file_path, metadata, on_page)
        .await
        .map_err(|e| format!("Failed to process file: {e}"))?;

    Ok(ids.len())
}

/// The metadata every file of a folder source is indexed with.
fn folder_file_metadata(file_path: &Path, space_id: &str) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    metadata.insert("space_id".to_string(), space_id.to_string());
    metadata.insert(
        "file_path".to_string(),
        file_path.to_string_lossy().to_string(),
    );
    metadata.insert("doc_type".to_string(), "document".to_string());
    metadata.insert(
        "title".to_string(),
        file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string(),
    );

    if let Some(extension) = file_path.extension() {
        let ext = extension.to_string_lossy().to_string();
        metadata.insert("file_type".to_string(), ext.clone());
        metadata.insert("file_extension".to_string(), ext.clone());

        if let Some(filename) = file_path.file_name() {
            metadata.insert(
                "filename".to_string(),
                filename.to_string_lossy().to_string(),
            );
        }
    }
    metadata
}

/// Called with (page being read, pages) while OCR reads a scanned PDF.
pub type ScannedPageFn<'a> = dyn Fn(usize, usize) + Sync + 'a;

/// Indexes one file into the shared engine. The file is parsed, chunked and embedded on
/// a blocking thread without the engine lock; only storing it takes the write lock, so
/// searches run while files are indexed.
pub async fn index_file(
    rag: &RwLock<RAGEngine>,
    path: &Path,
    metadata: HashMap<String, String>,
) -> Result<Vec<uuid::Uuid>, String> {
    index_file_with_progress(rag, path, metadata, None).await
}

/// [`index_file`], reporting OCR's progress through a scanned PDF to
/// `on_page` (page being read, pages). A scan read without the scan reader is
/// reported to the engine ([`RAGEngine::report_scan_reader_missing`]).
pub async fn index_file_with_progress(
    rag: &RwLock<RAGEngine>,
    path: &Path,
    metadata: HashMap<String, String>,
    on_page: Option<&ScannedPageFn<'_>>,
) -> Result<Vec<uuid::Uuid>, String> {
    index_file_with_notice(rag, path, metadata, on_page)
        .await
        .map(|indexed| indexed.ids)
}

/// A file indexed by [`index_file_with_notice`]: its chunks, and what the
/// user should know about how it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    pub ids: Vec<uuid::Uuid>,
    /// Pages of a scan OCR could not read ("pages 4, 9 could not be read"),
    /// when there were any.
    pub notice: Option<String>,
}

/// [`index_file_with_progress`], with what the user should know about how
/// the file was read. A panic while the file is parsed, chunked or embedded
/// is an error, not a panic of the caller.
pub async fn index_file_with_notice(
    rag: &RwLock<RAGEngine>,
    path: &Path,
    metadata: HashMap<String, String>,
    on_page: Option<&ScannedPageFn<'_>>,
) -> Result<IndexedFile, String> {
    let preparer = rag
        .read()
        .await
        .file_preparer()
        .map_err(|e| format!("{e:#}"))?;
    let owned = path.to_path_buf();
    let reported = metadata.clone();
    // Pages are reported from the blocking thread through a channel and
    // passed to `on_page` here, while the file is prepared.
    let (pages_tx, mut pages_rx) = tokio::sync::mpsc::unbounded_channel::<(usize, usize)>();
    let task = tokio::task::spawn_blocking(move || {
        preparer.prepare_with_progress(&owned, metadata, &|page, pages| {
            // The receiver is gone only when the caller stopped waiting.
            let _ = pages_tx.send((page, pages));
        })
    });
    tokio::pin!(task);
    let joined = loop {
        tokio::select! {
            joined = &mut task => break joined,
            Some((page, pages)) = pages_rx.recv() => {
                if let Some(on_page) = on_page {
                    on_page(page, pages);
                }
            }
        }
    };
    let prepared = joined.map_err(|e| match e.try_into_panic() {
        Ok(panic) => format!("Panic: {}", panic_message(panic.as_ref())),
        Err(e) => format!("Indexing task failed: {e}"),
    })?;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(e) => {
            if e.is::<crate::processing::ocr::ScanReaderMissing>() && queued_when_refused(&reported)
            {
                rag.read().await.report_scan_reader_missing(path, reported);
            }
            return Err(format!("{e:#}"));
        }
    };
    if prepared.scan_reader_missing() {
        rag.read().await.report_scan_reader_missing(path, reported);
    }
    let notice = prepared.notice().map(str::to_string);
    if let Some(notice) = &notice {
        tracing::warn!(file = %crate::log_safe::file(&path), "Indexed with pages missing: {notice}");
    }
    let ids = rag
        .write()
        .await
        .commit_file(prepared)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(IndexedFile { ids, notice })
}

/// Whether a file refused for want of the scan reader, indexed with
/// `metadata`, is queued to be read once the reader is installed. A Library
/// file is; a file added in a chat is not: the chat discards it, and the
/// chat's notice says why it was refused.
fn queued_when_refused(metadata: &HashMap<String, String>) -> bool {
    !metadata
        .get("space_id")
        .is_some_and(|owner| crate::rag_engine::is_chat_uploads(owner))
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = panic.downcast_ref::<&str>() {
        s.to_string()
    } else {
        "Unknown panic during file processing".to_string()
    }
}

fn emit_progress(
    emitter: Option<&dyn EventEmitter>,
    space_id: &str,
    current_file: &str,
    processed: usize,
    total: usize,
    percentage: f32,
    action: &str,
) {
    if let Some(e) = emitter {
        let progress = IndexingProgress {
            space_id: space_id.to_string(),
            current_file: current_file.to_string(),
            processed_files: processed,
            total_files: total,
            percentage,
            current_action: action.to_string(),
            eta_seconds: 0.0,
            speed: 0.0,
            scanned_page: None,
            scanned_pages: None,
        };
        e.emit(
            "indexing-progress",
            serde_json::to_value(&progress).unwrap_or_default(),
        );
    }
}

/// Progress of a folder run with its speed (`speed` files a second) and the
/// time left at that speed; the percentage stays below 100 until the run's
/// last event.
fn emit_rated_progress(
    emitter: Option<&dyn EventEmitter>,
    space_id: &str,
    current_file: &str,
    (processed, total): (usize, usize),
    action: &str,
    speed: f32,
) {
    if let Some(e) = emitter {
        let progress = rated_progress(space_id, current_file, (processed, total), action, speed);
        e.emit(
            "indexing-progress",
            serde_json::to_value(&progress).unwrap_or_default(),
        );
    }
}

fn rated_progress(
    space_id: &str,
    current_file: &str,
    (processed, total): (usize, usize),
    action: &str,
    speed: f32,
) -> IndexingProgress {
    let remaining = total.saturating_sub(processed) as f32;
    IndexingProgress {
        space_id: space_id.to_string(),
        current_file: current_file.to_string(),
        processed_files: processed,
        total_files: total,
        percentage: (processed as f32 / total.max(1) as f32 * 100.0).min(99.9),
        current_action: action.to_string(),
        eta_seconds: if speed > 0.0 { remaining / speed } else { 0.0 },
        speed,
        scanned_page: None,
        scanned_pages: None,
    }
}

/// Progress while OCR reads page `page` of a scanned PDF (`pages` pages):
/// "Reading scanned pages: 12 of 240". `files` is (processed, total) of the
/// run; `percentage` stays below 100 so the run does not read as finished.
fn emit_scanned_page(
    emitter: Option<&dyn EventEmitter>,
    space_id: &str,
    current_file: &str,
    files: (usize, usize),
    percentage: f32,
    (page, pages): (usize, usize),
) {
    if let Some(e) = emitter {
        let progress =
            scanned_page_progress(space_id, current_file, files, percentage, page, pages);
        e.emit(
            "indexing-progress",
            serde_json::to_value(&progress).unwrap_or_default(),
        );
    }
}

fn scanned_page_progress(
    space_id: &str,
    current_file: &str,
    (processed, total): (usize, usize),
    percentage: f32,
    page: usize,
    pages: usize,
) -> IndexingProgress {
    IndexingProgress {
        space_id: space_id.to_string(),
        current_file: current_file.to_string(),
        processed_files: processed,
        total_files: total,
        percentage: percentage.min(99.9),
        current_action: format!("Reading scanned pages: {page} of {pages}"),
        eta_seconds: 0.0,
        speed: 0.0,
        scanned_page: Some(page),
        scanned_pages: Some(pages),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spreadsheet_and_delimited_extensions_are_supported() {
        for ext in ["xlsx", "xls", "xlsm", "xlsb", "ods", "csv", "tsv"] {
            assert!(is_supported_file_type(ext), "{} should be supported", ext);
        }
        assert!(!is_supported_file_type("exe"));
    }

    #[test]
    fn refused_chat_uploads_are_not_queued_for_the_scan_reader() {
        let owned = |owner: &str| HashMap::from([("space_id".to_string(), owner.to_string())]);
        assert!(queued_when_refused(&owned("space-1")));
        assert!(queued_when_refused(&HashMap::new()));
        assert!(!queued_when_refused(&owned(
            crate::rag_engine::CHAT_UPLOADS_MAIN
        )));
        assert!(!queued_when_refused(&owned("chat-uploads:ws-1")));
    }

    /// The parser reads `.htm` as HTML; indexing takes it too.
    #[test]
    fn both_html_extensions_are_supported() {
        assert!(is_supported_file_type("html"));
        assert!(is_supported_file_type("htm"));
    }

    fn options(file_types: &[&str]) -> IndexingOptions {
        IndexingOptions {
            skip_indexed: false,
            watch_changes: false,
            process_subdirs: true,
            priority: "normal".to_string(),
            file_types: file_types.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn file_types_is_an_optional_restriction() {
        let all = options(&[]);
        assert!(is_selected_file_type(&all, "ods"));
        assert!(is_selected_file_type(&all, "tsv"));
        assert!(!is_selected_file_type(&all, "exe"));

        let narrowed = options(&["PDF", ".csv", "exe"]);
        assert!(is_selected_file_type(&narrowed, "pdf"));
        assert!(is_selected_file_type(&narrowed, "csv"));
        assert!(!is_selected_file_type(&narrowed, "xlsx"));
        // Requesting an unsupported type never makes it indexable.
        assert!(!is_selected_file_type(&narrowed, "exe"));
    }

    #[test]
    fn scanned_page_progress_names_the_page_and_never_reads_as_done() {
        let progress = scanned_page_progress("s1", "lease-scan.pdf", (2, 3), 100.0, 12, 240);
        assert_eq!(progress.current_action, "Reading scanned pages: 12 of 240");
        assert!(progress.percentage < 100.0);
        let json = serde_json::to_value(&progress).unwrap();
        assert_eq!(json["scanned_page"], 12);
        assert_eq!(json["scanned_pages"], 240);
        assert_eq!(json["current_file"], "lease-scan.pdf");
        assert_eq!(json["processed_files"], 2);
        // Other progress events carry no page fields.
        let plain = IndexingProgress {
            scanned_page: None,
            scanned_pages: None,
            ..progress
        };
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("scanned_page").is_none());
        assert!(json.get("scanned_pages").is_none());
    }

    #[test]
    fn indexing_result_failures_serialize_and_default_when_absent() {
        let result = IndexingResult {
            files_processed: 1,
            total_chunks: 4,
            failed_files: vec!["a.xlsx".to_string()],
            failures: vec![FileFailure {
                file: "a.xlsx".to_string(),
                reason: "Failed to process file: failed to open spreadsheet a.xlsx".to_string(),
            }],
            files_unchanged: 3,
            duration: 10,
            stopped: false,
        };
        let json = serde_json::to_value(&result).expect("serializable");
        assert_eq!(json["failures"][0]["file"], "a.xlsx");
        assert!(json["failures"][0]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("failed to open spreadsheet")));
        assert_eq!(json["files_unchanged"], 3);

        let legacy: IndexingResult = serde_json::from_str(
            r#"{"files_processed":0,"total_chunks":0,"failed_files":[],"duration":0}"#,
        )
        .expect("legacy payload deserializes");
        assert!(legacy.failures.is_empty());
        assert_eq!(legacy.files_unchanged, 0);
        assert!(!legacy.stopped);
    }

    #[test]
    fn poisoned_state_lock_does_not_panic() {
        let state = IndexingState::default();
        let run = state.begin_run("s1").expect("a run");
        std::thread::scope(|scope| {
            let _ = scope
                .spawn(|| {
                    let _guard = state.cancelled.lock().unwrap_or_else(|e| e.into_inner());
                    panic!("poison the lock");
                })
                .join();
        });
        assert_eq!(state.cancel(), 1);
        assert!(run.stopped());
    }

    /// Embeds like the word embedder, but a document batch containing `HOLD` waits
    /// until the test releases it: an index job stuck in embedding.
    struct GatedEmbedder {
        words: crate::statements::testing::WordEmbedder,
        entered: std::sync::Mutex<std::sync::mpsc::Sender<()>>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl crate::embeddings::EmbeddingModel for GatedEmbedder {
        fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            self.words.embed_query(text)
        }
        fn embed_document(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            if text.contains("HOLD") {
                let _ = self
                    .entered
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send(());
                let _ = self
                    .release
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .recv();
            }
            self.words.embed_document(text)
        }
        fn dimension(&self) -> usize {
            crate::statements::testing::DIM
        }
    }

    /// An engine holding a lease, whose embedder holds a document batch
    /// containing `HOLD` until released: the engine, the receiver told when
    /// the hold starts, and the sender that releases it.
    async fn gated_engine(
        dir: &Path,
    ) -> (
        Arc<RwLock<RAGEngine>>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.join("data");
        config.embedding.model_dir = dir.join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        config.search.min_score_threshold = 0.0;
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut engine = RAGEngine::new(config).await.expect("engine");
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(Arc::new(
                GatedEmbedder {
                    words: Default::default(),
                    entered: std::sync::Mutex::new(entered_tx),
                    release: std::sync::Mutex::new(release_rx),
                },
            )))
            .expect("models");
        engine
            .add_document(
                "The lease notice period is sixty days. Rent is reviewed every spring, and the landlord repairs the roof and the heating when they fail.",
                crate::types::DocumentFormat::TXT,
                HashMap::from([("title".to_string(), "lease".to_string())]),
                crate::types::Citation::default(),
            )
            .await
            .expect("indexed");
        (Arc::new(RwLock::new(engine)), entered_rx, release_tx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn searches_run_while_a_folder_is_indexed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (rag, entered_rx, release_tx) = gated_engine(dir.path()).await;

        let folder = dir.path().join("folder");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(
            folder.join("slow.txt"),
            "HOLD this file in embedding. It describes the parking rules of the building:              visitors park on the street, residents in the garage below the courtyard.",
        )
        .expect("file");
        let job = {
            let rag = rag.clone();
            let folder = folder.to_string_lossy().to_string();
            tokio::spawn(async move {
                index_folder(
                    &folder,
                    "space-1",
                    &options(&[]),
                    &rag,
                    &IndexingState::default(),
                    None,
                )
                .await
            })
        };
        // The job is embedding the file now.
        tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(60)))
            .await
            .expect("join")
            .expect("the index job reached embedding");

        let search = async { rag.read().await.search("lease notice period", 3).await };
        let found = tokio::time::timeout(Duration::from_secs(10), search)
            .await
            .expect("search waited for the index job")
            .expect("search");
        assert!(found.iter().any(|r| r.text.contains("sixty days")));

        release_tx.send(()).expect("release");
        let result = job.await.expect("join").expect("indexed");
        assert_eq!(result.files_processed, 1, "{:?}", result.failures);
        let after = rag
            .read()
            .await
            .search("HOLD file embedding", 3)
            .await
            .expect("search");
        assert!(after.iter().any(|r| r.text.contains("HOLD")));
    }

    /// A file added in a chat is read and embedded without the engine lock
    /// (as the upload command indexes it), so searches run meanwhile; it is
    /// stored under its chat's owner.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn searches_run_while_a_chat_upload_is_read() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (rag, entered_rx, release_tx) = gated_engine(dir.path()).await;
        let upload = dir.path().join("chat_uploads").join("u1").join("notes.txt");
        std::fs::create_dir_all(upload.parent().expect("folder")).expect("folder");
        std::fs::write(
            &upload,
            "HOLD this upload in embedding. It lists the rooms of the house: a kitchen facing the garden, two bedrooms upstairs and a study by the stairs.",
        )
        .expect("file");
        let owner = crate::rag_engine::chat_uploads_source(Some("ws-1")).expect("owner");
        let job = {
            let rag = rag.clone();
            let upload = upload.clone();
            let metadata = HashMap::from([("space_id".to_string(), owner.clone())]);
            tokio::spawn(async move { index_file_with_notice(&rag, &upload, metadata, None).await })
        };
        tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(60)))
            .await
            .expect("join")
            .expect("the upload reached embedding");

        let search = async { rag.read().await.search("lease notice period", 3).await };
        let found = tokio::time::timeout(Duration::from_secs(10), search)
            .await
            .expect("search waited for the upload")
            .expect("search");
        assert!(found.iter().any(|r| r.text.contains("sixty days")));

        release_tx.send(()).expect("release");
        let indexed = job.await.expect("join").expect("indexed");
        assert!(!indexed.ids.is_empty());
        assert_eq!(indexed.notice, None);
        let after = rag
            .read()
            .await
            .search("HOLD upload rooms kitchen", 3)
            .await
            .expect("search");
        assert!(
            after.iter().any(|r| r.text.contains("HOLD this upload")
                && r.metadata.get("space_id") == Some(&owner)),
            "{after:?}"
        );
    }

    #[tokio::test]
    async fn stopping_a_source_waits_for_its_runs_and_spares_the_others() {
        let state = IndexingState::default();
        let run = state.begin_run_at("papers", "C:/papers").expect("a run");
        let other = state.begin_run("notes").expect("another source's run");
        assert_eq!(
            state.running_paths(),
            [("papers".to_string(), "C:/papers".to_string())]
        );
        let stop = state.stop_source("papers");
        tokio::pin!(stop);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut stop)
                .await
                .is_err(),
            "the stop waits for the run"
        );
        assert!(state.should_stop("papers"));
        assert!(!state.should_stop("notes"), "other sources go on");
        assert!(!other.stopped(), "nothing else is cancelled");
        drop(run);
        stop.await;
        assert!(state.running_paths().is_empty());
        assert!(state.begin_run("papers").is_none(), "no run starts again");
        drop(other);
        state.restore_source("papers");
        assert!(state.begin_run("papers").is_some());
    }

    /// A folder whose source is removed while it is indexed stops before its
    /// next file, and the removal waits until it has.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_of_a_removed_source_stops_before_its_next_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.path().join("data");
        config.embedding.model_dir = dir.path().join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        config.search.min_score_threshold = 0.0;
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut engine = RAGEngine::new(config).await.expect("engine");
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(Arc::new(
                GatedEmbedder {
                    words: Default::default(),
                    entered: std::sync::Mutex::new(entered_tx),
                    release: std::sync::Mutex::new(release_rx),
                },
            )))
            .expect("models");
        let rag = Arc::new(RwLock::new(engine));
        let folder = dir.path().join("folder");
        std::fs::create_dir_all(&folder).expect("folder");
        // Either file is first; each is held in embedding until released.
        for name in ["one.txt", "two.txt"] {
            std::fs::write(
                folder.join(name),
                format!("HOLD {name} in embedding. It describes the parking rules of the building: visitors park on the street, residents in the garage below the courtyard."),
            )
            .expect("file");
        }
        let state = Arc::new(IndexingState::default());
        let job = {
            let (rag, state) = (rag.clone(), state.clone());
            let folder = folder.to_string_lossy().to_string();
            tokio::spawn(async move {
                // One file a batch: the run stops before its next file.
                index_folder_batched(
                    &folder,
                    "space-1",
                    &options(&[]),
                    &rag,
                    &state,
                    None,
                    None,
                    BatchLimits {
                        max_files: 1,
                        ..BatchLimits::default()
                    },
                )
                .await
            })
        };
        let entered = tokio::task::spawn_blocking(move || {
            entered_rx
                .recv_timeout(Duration::from_secs(60))
                .map(|()| entered_rx)
        })
        .await
        .expect("join")
        .expect("the index job reached embedding");

        let removal = {
            let state = state.clone();
            tokio::spawn(async move { state.stop_source("space-1").await })
        };
        // Removed while the first file is embedded: that file finishes, the
        // next one is not started.
        while !state.is_removed("space-1") {
            tokio::task::yield_now().await;
        }
        release_tx.send(()).expect("release");
        tokio::time::timeout(Duration::from_secs(60), removal)
            .await
            .expect("the removal ended with the run")
            .expect("join");
        let result = tokio::time::timeout(Duration::from_secs(60), job)
            .await
            .expect("the run stopped")
            .expect("join")
            .expect("indexed");
        assert_eq!(result.files_processed, 1, "{:?}", result.failures);
        assert!(
            entered.try_recv().is_err(),
            "the second file was never embedded"
        );
    }

    #[test]
    fn a_batch_takes_the_ready_files_up_to_its_limits() {
        let limits = BatchLimits {
            max_files: 3,
            max_chunks: 10,
            read_ahead: 4,
        };
        // Small files fill a batch up to its file limit.
        let mut ready = vec![2usize, 2, 2, 2].into_iter();
        let batch = fill_batch(1usize, || ready.next(), |c| *c, &limits);
        assert_eq!(batch, [1, 2, 2]);
        assert_eq!(ready.next(), Some(2), "the rest waits for the next batch");
        // A batch takes no more files once it has its chunks; a big file
        // is never split.
        let mut ready = vec![6usize, 3].into_iter();
        let batch = fill_batch(5usize, || ready.next(), |c| *c, &limits);
        assert_eq!(batch, [5, 6]);
        let batch = fill_batch(40usize, || None, |c| *c, &limits);
        assert_eq!(batch, [40]);
        // Only files that are ready: nothing waits for a file being read.
        let batch = fill_batch(1usize, || None, |c| *c, &limits);
        assert_eq!(batch, [1]);
    }

    #[test]
    fn rated_progress_estimates_the_time_left_and_never_reads_as_done() {
        let progress = rated_progress("s1", "a.pdf", (10, 40), "Indexed 10 of 40", 2.0);
        assert_eq!(progress.speed, 2.0);
        assert!((progress.eta_seconds - 15.0).abs() < 1e-6);
        assert!((progress.percentage - 25.0).abs() < 1e-6);
        let last = rated_progress("s1", "z.pdf", (40, 40), "Indexed 40 of 40", 2.0);
        assert!(last.percentage < 100.0, "only the run's last event is 100");
        assert_eq!(last.eta_seconds, 0.0);
        let unknown = rated_progress("s1", "a.pdf", (0, 40), "Reading", 0.0);
        assert_eq!(unknown.eta_seconds, 0.0);
    }

    #[tokio::test]
    async fn the_pause_switch_holds_waiters_until_resumed_and_reports_changes() {
        let switch = PauseSwitch::new(true);
        assert!(switch.is_paused());
        assert!(!switch.set(true), "already paused");
        let waiter = {
            let switch = switch.clone();
            tokio::spawn(async move { switch.wait_until_resumed().await })
        };
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "held while paused");
        assert!(switch.set(false));
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("released on resume")
            .expect("join");
        // The engine's state and the tray share one switch.
        let state = IndexingState::with_pause(switch.clone());
        state.pause();
        assert!(switch.is_paused());
        state.resume();
        assert!(!switch.is_paused());
    }

    #[test]
    fn cancelling_a_source_stops_its_runs_in_progress_only() {
        let state = IndexingState::default();
        let papers = state.begin_run_at("papers", "C:/papers").expect("run");
        let notes = state.begin_run("notes").expect("run");
        assert_eq!(state.cancel_source("papers"), 1);
        assert!(papers.stopped());
        assert!(state.should_stop("papers"));
        assert!(!notes.stopped(), "other sources go on");
        assert!(!state.is_removed("papers"), "cancelling is not removing");
        drop(papers);
        // A run started after the cancel is not cancelled.
        let again = state.begin_run("papers").expect("a new run");
        assert!(!again.stopped());
        assert!(!state.should_stop("papers"));
        // Cancelling a source with no run in progress cancels nothing.
        assert_eq!(state.cancel_source("calendar"), 0);
        // Cancel all stops every run in progress.
        assert_eq!(state.cancel(), 2);
        assert!(again.stopped() && notes.stopped());
    }

    #[tokio::test]
    async fn a_paused_run_waits_and_a_cancel_releases_it() {
        let state = IndexingState::with_pause(PauseSwitch::new(true));
        let run = state.begin_run("papers").expect("run");
        let wait = run.wait_while_paused();
        tokio::pin!(wait);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), &mut wait)
                .await
                .is_err(),
            "held while paused"
        );
        state.cancel_source("papers");
        let stopped = tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .expect("a cancelled run is not held");
        assert!(stopped);
    }

    async fn word_engine(dir: &Path) -> Arc<RwLock<RAGEngine>> {
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

    /// Files indexed in batches are each stored once with their own chunks,
    /// and indexing the folder again replaces them instead of adding copies.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_folder_indexed_in_batches_stores_each_file_once() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rag = word_engine(dir.path()).await;
        let folder = dir.path().join("folder");
        std::fs::create_dir_all(&folder).expect("folder");
        let topics = [
            "lease notice periods and rent reviews",
            "parking rules for visitors and residents",
            "boiler servicing and heating repairs",
            "laundry room booking and opening hours",
            "bicycle shelter quotes for the courtyard",
        ];
        for (i, topic) in topics.iter().enumerate() {
            std::fs::write(
                folder.join(format!("note-{i}.txt")),
                format!("This note is about {topic}. It records what the committee decided about {topic} at its last meeting."),
            )
            .expect("file");
        }
        let limits = BatchLimits {
            max_files: 2,
            max_chunks: 64,
            read_ahead: 4,
        };
        let state = IndexingState::default();
        let folder_text = folder.to_string_lossy().to_string();
        let first = index_folder_batched(
            &folder_text,
            "space-1",
            &options(&[]),
            &rag,
            &state,
            None,
            None,
            limits,
        )
        .await
        .expect("indexed");
        assert_eq!(first.files_processed, topics.len(), "{:?}", first.failures);
        assert!(!first.stopped);
        let sources = rag.read().await.document_sources().await.expect("sources");
        assert_eq!(sources.len(), topics.len());
        let chunks: usize = sources.iter().map(|s| s.chunks).sum();
        assert_eq!(chunks, first.total_chunks);

        let again = index_folder_batched(
            &folder_text,
            "space-1",
            &options(&[]),
            &rag,
            &state,
            None,
            None,
            limits,
        )
        .await
        .expect("indexed again");
        assert_eq!(again.files_processed, topics.len());
        let sources = rag.read().await.document_sources().await.expect("sources");
        assert_eq!(sources.len(), topics.len(), "no file is stored twice");
        let chunks: usize = sources.iter().map(|s| s.chunks).sum();
        assert_eq!(chunks, again.total_chunks, "the previous chunks are gone");
        let found = rag
            .read()
            .await
            .search("boiler servicing heating", 3)
            .await
            .expect("search");
        assert!(found.iter().any(|r| r.text.contains("boiler")));
    }

    /// A run cancelled while it embeds a batch stores that batch and nothing
    /// after it, says it stopped, and counts exactly the files it stored.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancelled_run_counts_only_the_files_it_stored() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (rag, entered_rx, release_tx) = gated_engine(dir.path()).await;
        let before = rag
            .read()
            .await
            .document_sources()
            .await
            .expect("sources")
            .len();
        let folder = dir.path().join("folder");
        std::fs::create_dir_all(&folder).expect("folder");
        // Whichever file is read first, its batch is held in embedding.
        for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
            std::fs::write(
                folder.join(name),
                format!("HOLD {name} in embedding. It describes the laundry room: two machines, booked on a sheet by the door."),
            )
            .expect("file");
        }
        let state = Arc::new(IndexingState::default());
        let job = {
            let (rag, state) = (rag.clone(), state.clone());
            let folder = folder.to_string_lossy().to_string();
            tokio::spawn(async move {
                index_folder_batched(
                    &folder,
                    "space-1",
                    &options(&[]),
                    &rag,
                    &state,
                    None,
                    None,
                    BatchLimits {
                        max_files: 1,
                        ..BatchLimits::default()
                    },
                )
                .await
            })
        };
        tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(60)))
            .await
            .expect("join")
            .expect("the run reached embedding");
        assert_eq!(state.cancel_source("space-1"), 1);
        release_tx.send(()).expect("release");
        let result = tokio::time::timeout(Duration::from_secs(60), job)
            .await
            .expect("the run stopped")
            .expect("join")
            .expect("indexed");
        assert!(result.stopped);
        assert_eq!(result.files_processed, 1, "{:?}", result.failures);
        assert!(result.failures.is_empty());
        let after = rag
            .read()
            .await
            .document_sources()
            .await
            .expect("sources")
            .len();
        assert_eq!(after, before + 1, "only the file counted was stored");
        // A run started after the cancel is not stopped by it.
        assert!(!state.should_stop("space-1"));
    }
}
