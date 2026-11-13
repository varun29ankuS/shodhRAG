//! Recall's index: its own library (vectors and keyword index, with its own search model
//! record) plus the catalog, in Recall's folder. A sync brings it in line with the
//! transcripts on disk and the user's choices: new and changed sessions are indexed once
//! they have been idle long enough, a session that grew re-embeds only the parts that
//! changed, and sessions that left the disk, or that the user excluded or removed, leave
//! the index.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;

use super::catalog::{Catalog, FileRecord, FileState, IndexedFile, TurnRow};
use super::chunks::{self, SessionLabel};
use super::discover::{self, Roots};
use super::entities;
use super::redact::Redactor;
use super::settings::RecallSettings;
use super::{read_transcript, ParseBudget, Tool, Transcript};
use crate::types::Citation;
use crate::RAGEngine;

/// Version of how sessions are indexed (chunking, context, metadata). Raised whenever it
/// changes, so every session is indexed again the new way.
pub const INDEX_VERSION: u32 = 2;
/// The vector and keyword index, in Recall's folder.
pub const INDEX_DIR: &str = "index";
/// The catalog, in Recall's folder.
pub const CATALOG_FILE: &str = "recall.db";
/// Fused candidates (vectors and keywords) per chunk a search asks for.
pub const SEARCH_CANDIDATE_MULTIPLIER: usize = 3;
/// Candidates the reranker reads at most. On a CI runner's CPU each turn costs about
/// a tenth of a second: 16 put the scorecard's 95th percentile at 2.02 s, over the
/// 2 s target; 10 leave room for a slower computer.
pub const RERANK_CANDIDATES: usize = 10;
/// `source_type` of every chunk Recall stores.
pub const SOURCE_TYPE: &str = "recall";

/// The source id of part `part` of a catalog file.
pub fn source_id(tool: Tool, file_id: i64, part: u32) -> String {
    format!("recall://{}/{file_id}/{part}", tool.id())
}

/// The catalog file and part a source id names.
pub fn parse_source(source: &str) -> Option<(i64, u32)> {
    let rest = source.strip_prefix("recall://")?;
    let mut parts = rest.split('/');
    let _tool = parts.next()?;
    let file: i64 = parts.next()?.parse().ok()?;
    let part: u32 = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((file, part))
}

/// Recall's index and catalog.
pub struct RecallIndex {
    dir: PathBuf,
    engine: Arc<RwLock<RAGEngine>>,
    catalog: Arc<Catalog>,
}

impl RecallIndex {
    /// Opens Recall's index in `dir` with the search models under `model_dir`. The index
    /// records which search model built it and always loads that one (never another, so
    /// vectors are never mixed); a new index is built with the standard model.
    pub async fn open(
        dir: &Path,
        model_dir: &Path,
        compute: crate::compute::provider::ComputePreference,
    ) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Recall's folder {} could not be made", dir.display()))?;
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.join(INDEX_DIR);
        config.embedding.model_dir = model_dir.to_path_buf();
        config.embedding.use_e5 = true;
        config.embedding.dimension = crate::embeddings::e5::E5Config::auto_detect(model_dir)
            .map(|c| c.dimension)
            .unwrap_or(crate::embeddings::e5::E5_DIMENSION);
        config.embedding.compute = compute;
        // The reranker reads `k * candidate_multiplier` candidates, each a whole turn:
        // three per chunk asked for keeps a search of five sessions under the 2 s target
        // on a CPU (the scorecard, crates/shodh-eval/src/recall.rs, measures it).
        config.search.candidate_multiplier = SEARCH_CANDIDATE_MULTIPLIER;
        config.search.rerank_candidates = RERANK_CANDIDATES;
        let engine = RAGEngine::new(config)
            .await
            .context("Recall's index could not be opened")?;
        let mut engine = engine;
        let catalog = Catalog::open(&dir.join(CATALOG_FILE))
            .context("Recall's catalog could not be opened")?;
        if catalog.was_reset() {
            // A catalog of another version was emptied: its chunks are no one's now.
            engine
                .clear_all_data()
                .await
                .context("Recall's old index could not be emptied")?;
        }
        Ok(Self::with_parts(
            dir,
            Arc::new(RwLock::new(engine)),
            Arc::new(catalog),
        ))
    }

    /// An index over an engine and catalog already open (tests, the eval).
    pub fn with_parts(dir: &Path, engine: Arc<RwLock<RAGEngine>>, catalog: Arc<Catalog>) -> Self {
        Self {
            dir: dir.to_path_buf(),
            engine,
            catalog,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn engine(&self) -> &Arc<RwLock<RAGEngine>> {
        &self.engine
    }

    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }
}

/// How far a sync is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncProgress {
    /// Sessions indexed so far in this sync, of `total` to index.
    pub done: usize,
    pub total: usize,
    /// The session being indexed now, in a few words (its project and title).
    pub current: Option<String>,
}

/// What a sync may be told while it runs.
pub struct SyncControl<'a> {
    /// Stop now; what is done stays.
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    /// Indexing is paused: stop before the next part.
    pub paused: &'a (dyn Fn() -> bool + Sync),
    pub progress: &'a (dyn Fn(&SyncProgress) + Sync),
}

/// Why a sync stopped before it finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    Cancelled,
    Paused,
}

/// What a sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub indexed: usize,
    pub unchanged: usize,
    /// Changed sessions not idle long enough yet.
    pub waiting: usize,
    /// Sessions that left the disk, and left the index.
    pub removed: usize,
    /// Sessions not wanted (tool or project not chosen, removed by the user).
    pub excluded: usize,
    pub failed: usize,
    /// Parts embedded (a grown session embeds only what changed).
    pub parts_embedded: usize,
    pub stopped: Option<Stopped>,
}

/// Unix seconds now.
pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

impl RecallIndex {
    /// Brings the index in line with the transcripts under `roots` and `settings`.
    /// `now` is Unix seconds (a session changed less than `settings.idle_minutes` ago
    /// waits).
    pub async fn sync(
        &self,
        settings: &RecallSettings,
        roots: &Roots,
        now: i64,
        control: &SyncControl<'_>,
    ) -> Result<SyncReport> {
        let redactor = settings.redactor().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut report = SyncReport::default();
        let catalog = self.catalog.clone();

        // What is on disk, for the tools that are on.
        let known: HashMap<String, FileRecord> = catalog
            .files()?
            .into_iter()
            .map(|f| (f.path.clone(), f))
            .collect();
        let mut on_disk: HashSet<String> = HashSet::new();
        let mut records: Vec<FileRecord> = Vec::new();
        if settings.enabled {
            for tool in Tool::ALL {
                if !settings.tools.contains(&tool) {
                    continue;
                }
                let Some(root) = roots.of(tool) else { continue };
                let root = root.to_path_buf();
                let files = tokio::task::spawn_blocking(move || discover::transcripts(tool, &root))
                    .await
                    .context("listing transcripts")?;
                for file in files {
                    let path = file.path.display().to_string();
                    let known_project = known.get(&path).map(|r| {
                        (
                            r.project_key.as_str(),
                            r.project_dir.as_str(),
                            r.chains.as_slice(),
                        )
                    });
                    let found = discover::found_file(tool, &file, known_project, &redactor);
                    let mut record = catalog.upsert_found(&found)?;
                    // What is on disk now, to compare with what was indexed.
                    on_disk.insert(path);
                    record.size = file.size;
                    let disk_mtime = file.mtime;
                    records.push(FileRecord {
                        mtime: disk_mtime,
                        ..record
                    });
                }
            }
        }

        // Sessions that left the disk (or whose tool is off) leave the index.
        for (path, record) in &known {
            if on_disk.contains(path) {
                continue;
            }
            self.delete_parts(record).await?;
            if settings.enabled && settings.tools.contains(&record.tool) {
                catalog.remove_file(record.id)?;
                report.removed += 1;
            } else if record.state != FileState::Excluded {
                catalog.set_state(record.id, FileState::Excluded, None)?;
                report.excluded += 1;
            }
        }

        let removed: HashSet<(Tool, String)> = catalog.removed_sessions()?.into_iter().collect();
        let idle = i64::from(settings.idle_minutes) * 60;
        let mut work: Vec<FileRecord> = Vec::new();
        for record in records {
            let stored = known.get(&record.path);
            let wanted = settings.wants(record.tool, &record.chains)
                && !removed.contains(&(record.tool, record.session_id.clone()));
            if !wanted {
                let has_index =
                    stored.is_some_and(|s| s.parts > 0 || s.state == FileState::Indexed);
                if has_index {
                    self.delete_parts(stored.unwrap_or(&record)).await?;
                }
                if stored.map_or(true, |s| s.state != FileState::Excluded) {
                    catalog.set_state(record.id, FileState::Excluded, None)?;
                }
                report.excluded += 1;
                continue;
            }
            let up_to_date = stored.is_some_and(|s| {
                s.state == FileState::Indexed
                    && s.size == record.size
                    && s.mtime == record.mtime
                    && s.fingerprint.as_deref() == Some(index_fingerprint(&redactor).as_str())
            });
            let failed_unchanged = stored.is_some_and(|s| {
                s.state == FileState::Failed && s.size == record.size && s.mtime == record.mtime
            });
            if up_to_date || failed_unchanged {
                report.unchanged += 1;
                continue;
            }
            if now - record.mtime < idle {
                report.waiting += 1;
                continue;
            }
            work.push(record);
        }

        // Newest sessions first: they are the ones asked about.
        work.sort_by(|a, b| b.mtime.cmp(&a.mtime));
        let total = work.len();
        (control.progress)(&SyncProgress {
            done: 0,
            total,
            current: None,
        });
        for (done, record) in work.into_iter().enumerate() {
            if (control.cancelled)() {
                report.stopped = Some(Stopped::Cancelled);
                break;
            }
            if (control.paused)() {
                report.stopped = Some(Stopped::Paused);
                break;
            }
            (control.progress)(&SyncProgress {
                done,
                total,
                current: Some(discover::project_name(&record.project_dir)),
            });
            match self
                .index_file(&record, settings, &redactor, now, control)
                .await
            {
                Ok(FileOutcome::Indexed { parts }) => {
                    report.indexed += 1;
                    report.parts_embedded += parts;
                }
                Ok(FileOutcome::Excluded) => report.excluded += 1,
                Ok(FileOutcome::Stopped(why)) => {
                    report.stopped = Some(why);
                    break;
                }
                Err(e) if e.is::<crate::embeddings::SearchModelsMissing>() => return Err(e),
                Err(e) if is_model_refusal(&e) => return Err(e),
                Err(e) => {
                    tracing::warn!(target: "shodh::recall", file = record.id, error = %format!("{e:#}"), "a session could not be indexed");
                    // Whatever it had in the index goes: the catalog no longer maps it.
                    let stored_parts = known.get(&record.path).map_or(0, |s| s.parts);
                    self.delete_parts(&FileRecord {
                        parts: stored_parts.max(record.parts),
                        ..record.clone()
                    })
                    .await?;
                    catalog.record_failure(
                        record.id,
                        record.size,
                        record.mtime,
                        &plain_failure(&e),
                    )?;
                    report.failed += 1;
                }
            }
        }
        (control.progress)(&SyncProgress {
            done: report.indexed + report.failed,
            total,
            current: None,
        });
        self.maintain_store().await;
        Ok(report)
    }

    /// Merges the chunk store's files and updates its vector index after a
    /// sync or removal, detached as after a Library indexing run: a write only
    /// marks the store changed (storage/maintenance.rs).
    async fn maintain_store(&self) {
        let maintenance = self.engine.read().await.store_maintenance();
        tokio::spawn(async move {
            if let Err(e) = maintenance.run_if_changed().await {
                tracing::warn!(error = %format!("{e:#}"), "Recall chunk store maintenance failed");
            }
        });
    }

    /// Removes every part of a file from the index.
    async fn delete_parts(&self, record: &FileRecord) -> Result<()> {
        if record.parts == 0 {
            return Ok(());
        }
        let mut engine = self.engine.write().await;
        for part in 0..record.parts {
            engine
                .delete_by_source(&source_id(record.tool, record.id, part))
                .await?;
        }
        Ok(())
    }

    /// Indexes one transcript: reads and redacts it, embeds the parts whose content
    /// changed, removes parts it no longer has, and records it in the catalog.
    async fn index_file(
        &self,
        record: &FileRecord,
        settings: &RecallSettings,
        redactor: &Redactor,
        now: i64,
        control: &SyncControl<'_>,
    ) -> Result<FileOutcome> {
        let path = PathBuf::from(&record.path);
        let tool = record.tool;
        let reader = redactor.clone();
        let transcript = tokio::task::spawn_blocking(move || {
            read_transcript(tool, &path, &reader, ParseBudget::default())
        })
        .await
        .context("reading the transcript")?
        .context("the transcript could not be read")?;

        // A session that worked in an excluded folder is not read, wherever it started.
        let mut chains = record.chains.clone();
        for chain in &transcript.info.cwd_chains {
            if !chains.contains(chain) {
                chains.push(chain.clone());
            }
        }
        if !settings.wants(record.tool, &chains) {
            let stored = FileRecord {
                parts: record
                    .parts
                    .max(self.catalog.part_hashes(record.id)?.len() as u32),
                ..record.clone()
            };
            self.delete_parts(&stored).await?;
            self.catalog
                .set_state(record.id, FileState::Excluded, None)?;
            self.catalog.set_chains(record.id, &chains)?;
            return Ok(FileOutcome::Excluded);
        }

        let label = label_of(record, &transcript);
        let parts = chunks::parts(&transcript, &label, redactor.fingerprint());
        let previous: BTreeMap<u32, String> =
            self.catalog.part_hashes(record.id)?.into_iter().collect();
        let previous_count = record.parts.max(previous.len() as u32);

        let mut embedded = 0usize;
        let mut changed_index = false;
        let mut stopped = None;
        let mut failure = None;
        for part in &parts {
            if previous.get(&part.number) == Some(&part.hash) && record.state == FileState::Indexed
            {
                continue;
            }
            if (control.cancelled)() {
                stopped = Some(Stopped::Cancelled);
                break;
            }
            if (control.paused)() {
                stopped = Some(Stopped::Paused);
                break;
            }
            match self.embed_part(record, &transcript, &label, part).await {
                Ok(true) => {
                    embedded += 1;
                    changed_index = true;
                }
                Ok(false) => changed_index = true,
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        if stopped.is_some() || failure.is_some() {
            // Some parts are new and the catalog still maps the old ones: a receipt could
            // open the wrong turn. The file leaves the index whole and is indexed again
            // from the start next time.
            if changed_index {
                let count = previous_count.max(parts.len() as u32);
                let mut engine = self.engine.write().await;
                for number in 0..count {
                    engine
                        .delete_by_source(&source_id(tool, record.id, number))
                        .await?;
                }
                drop(engine);
                self.catalog.set_state(record.id, FileState::Found, None)?;
            }
            return match failure {
                Some(e) => Err(e),
                None => Ok(FileOutcome::Stopped(stopped.unwrap_or(Stopped::Cancelled))),
            };
        }
        // Parts the transcript no longer has (it was rewritten shorter).
        if previous_count > parts.len() as u32 {
            let mut engine = self.engine.write().await;
            for number in parts.len() as u32..previous_count {
                engine
                    .delete_by_source(&source_id(tool, record.id, number))
                    .await?;
            }
        }

        let indexed = indexed_file(record, &transcript, &parts, redactor);
        self.catalog.record_indexed(record.id, &indexed, now)?;
        Ok(FileOutcome::Indexed { parts: embedded })
    }

    /// Embeds and stores one part of a transcript, replacing what it had; a part with
    /// nothing to index is removed. Returns whether anything was embedded.
    async fn embed_part(
        &self,
        record: &FileRecord,
        transcript: &Transcript,
        label: &SessionLabel,
        part: &chunks::Part,
    ) -> Result<bool> {
        let source = source_id(record.tool, record.id, part.number);
        if part.chunks.is_empty() {
            self.engine.write().await.delete_by_source(&source).await?;
            return Ok(false);
        }
        let preparer = self.engine.read().await.file_preparer()?;
        let metadata = part_metadata(record, transcript, part.number);
        let citation = Citation {
            title: label.title.clone(),
            source: source.clone(),
            ..Citation::default()
        };
        let given: Vec<_> = part.chunks.iter().map(|c| c.chunk.clone()).collect();
        let title = label.title.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            preparer.prepare_given_chunks(&source, &title, given, metadata, &citation)
        })
        .await
        .context("embedding a session")??;
        self.engine.write().await.commit_file(prepared).await?;
        Ok(true)
    }

    /// Removes a session (every file of it) from the index; it is never indexed again.
    pub async fn remove_session(&self, tool: Tool, session_id: &str) -> Result<usize> {
        let files = self.catalog.session_files(tool, session_id)?;
        for file in &files {
            self.delete_parts(file).await?;
            self.catalog.set_state(file.id, FileState::Excluded, None)?;
        }
        self.catalog.add_removed(tool, session_id, now_secs())?;
        self.maintain_store().await;
        Ok(files.len())
    }

    /// Deletes the whole index (vectors, keyword index and catalog records); the
    /// transcripts are untouched.
    pub async fn clear(&self) -> Result<()> {
        self.engine.write().await.clear_all_data().await?;
        self.catalog.clear()?;
        Ok(())
    }
}

enum FileOutcome {
    Indexed {
        parts: usize,
    },
    /// It worked in a folder the user excluded.
    Excluded,
    Stopped(Stopped),
}

/// The model refused to embed (an int8 library on a processor that cannot run it
/// exactly): every session would fail the same way, so the sync stops with it.
fn is_model_refusal(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains(crate::rag_engine::INT8_NOT_EXACT_HERE)
}

fn plain_failure(e: &anyhow::Error) -> String {
    super::clip(&format!("{e:#}"), 300)
}

/// What each chunk of a session says about it.
fn label_of(record: &FileRecord, transcript: &Transcript) -> SessionLabel {
    let title = transcript
        .info
        .title
        .clone()
        .or_else(|| {
            transcript
                .turns
                .iter()
                .find(|t| t.role == super::Role::User)
                .map(|t| super::clip(t.text.lines().next().unwrap_or(""), 80))
        })
        .unwrap_or_default();
    SessionLabel {
        tool: record.tool.label().to_string(),
        project: discover::project_name(&record.project_dir),
        title,
        branch: transcript.info.branch.clone(),
        date: transcript
            .info
            .started
            .map(|t| t.format("%Y-%m-%d").to_string()),
    }
}

fn part_metadata(
    record: &FileRecord,
    transcript: &Transcript,
    part: u32,
) -> std::collections::HashMap<String, String> {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "space_id".to_string(),
        discover::space_id(record.tool, &record.project_key),
    );
    metadata.insert("source_type".to_string(), SOURCE_TYPE.to_string());
    // A turn is a unit: search returns the turn that matched, never joined with others.
    metadata.insert("unit_kind".to_string(), "turn".to_string());
    metadata.insert("recall_tool".to_string(), record.tool.id().to_string());
    metadata.insert("recall_file".to_string(), record.id.to_string());
    metadata.insert("recall_part".to_string(), part.to_string());
    if let Some(id) = &transcript.info.session_id {
        metadata.insert("recall_session".to_string(), id.clone());
    }
    metadata
}

/// What the catalog records of an indexed transcript.
fn indexed_file(
    record: &FileRecord,
    transcript: &Transcript,
    parts: &[chunks::Part],
    redactor: &Redactor,
) -> IndexedFile {
    let mut turns: Vec<TurnRow> = Vec::with_capacity(transcript.turns.len());
    let mut spans: HashMap<u32, (u32, u32)> = HashMap::new();
    for part in parts {
        for (ordinal, chunk) in part.chunks.iter().enumerate() {
            let ordinal = ordinal as u32;
            spans
                .entry(chunk.turn)
                .and_modify(|span| span.1 = ordinal)
                .or_insert((ordinal, ordinal));
        }
    }
    let mut found_entities = Vec::new();
    for turn in &transcript.turns {
        turns.push(TurnRow {
            file_id: record.id,
            turn: turn.index,
            part: turn.index / chunks::PART_TURNS as u32,
            chunks: spans.get(&turn.index).copied(),
            role: turn.role,
            tool_name: turn.tool_name.clone(),
            at: turn.at.map(|t| t.timestamp()),
            line: turn.line,
            offset: turn.offset,
            kept_back: turn.kept_back.map(str::to_string),
        });
        if turn.kept_back.is_none() {
            // The indexed text only: a long output's middle is not looked up either.
            for piece in chunks::indexed_pieces(turn) {
                for entity in entities::extract(&piece) {
                    found_entities.push((entity, turn.index));
                }
            }
        }
    }
    let info = &transcript.info;
    for extra in info.branch.iter().chain(info.pull_requests.iter()) {
        for entity in entities::extract(extra) {
            found_entities.push((entity, 0));
        }
        if let Some(branch) = &info.branch {
            if extra == branch {
                found_entities.push((
                    entities::Entity {
                        kind: entities::EntityKind::Branch,
                        value: branch.clone(),
                    },
                    0,
                ));
            }
        }
    }
    if let Some(model) = &info.model {
        found_entities.push((
            entities::Entity {
                kind: entities::EntityKind::Model,
                value: model.clone(),
            },
            0,
        ));
    }
    found_entities.sort();
    found_entities.dedup();
    IndexedFile {
        format: Some(transcript.format.id().to_string()),
        session_id: info
            .session_id
            .clone()
            .unwrap_or_else(|| record.session_id.clone()),
        thread: info.thread.clone().or_else(|| record.thread.clone()),
        title: info.title.clone(),
        branch: info.branch.clone(),
        model: info.model.clone(),
        started_at: info.started.map(|t| t.timestamp()),
        ended_at: info.ended.map(|t| t.timestamp()),
        size: record.size,
        mtime: record.mtime,
        parts: parts.iter().map(|p| (p.number, p.hash.clone())).collect(),
        turns,
        entities: found_entities,
        errors: transcript.errors.clone(),
        error_count: transcript.errors.len() as u32,
        redacted: transcript.redacted as u32,
        fingerprint: index_fingerprint(redactor),
        chains: {
            let mut chains = record.chains.clone();
            for chain in &info.cwd_chains {
                if !chains.contains(chain) {
                    chains.push(chain.clone());
                }
            }
            chains
        },
    }
}

/// What a file is recorded as indexed under: the redaction rules and patterns, and
/// [`INDEX_VERSION`]. A file recorded under another is indexed again.
pub(crate) fn index_fingerprint(redactor: &Redactor) -> String {
    format!("{}-v{INDEX_VERSION}", redactor.fingerprint())
}
