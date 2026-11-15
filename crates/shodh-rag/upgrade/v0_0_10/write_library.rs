//! Writes a library with Shodh 0.0.10's own code, for the upgrade test of
//! 0.0.11 (`src/upgrade_tests.rs`).
//!
//! This file is not built with this version of the crate: nothing in Cargo's
//! target discovery reaches `upgrade/`. CI's "Upgrade from 0.0.10" job checks
//! out the `v0.0.10` tag, copies this file and `library.rs` there as the
//! example `write_upgrade_library` of that tree's shodh-rag, and runs it with
//! that tree's lockfile (lancedb 0.26, Lance 2.0, Tantivy 0.22):
//!
//!   cargo run --locked -p shodh-rag --example write_upgrade_library -- <fixture folder>
//!
//! It indexes the files of [`library::files`] the way the app does: a Library
//! folder source through 0.0.10's folder sync (which records each file's
//! chunker version in its manifest), the other files by path with their
//! library source. It adds a note without a file, builds 0.0.10's vector
//! index (`optimize`, an L2 IVF-PQ index from 1,000 rows), removes a file
//! (0.0.10 compacts after every removal), indexes one more file (rows the
//! index does not cover, recorded in the manifest), and saves memories in
//! the memory store the app uses. Then it reads everything back through
//! 0.0.10 (every column of every chunk, the index's statistics, searches,
//! memories) into `manifest.json`: what 0.0.10 saw, which the reader
//! compares with what 0.0.11 sees.
//!
//! Chunk and document ids are random (0.0.10 draws them), so two runs give
//! the same library under other ids; the manifest is written by the same run
//! as the library, so the comparison is exact either way.

#[path = "library.rs"]
mod library;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int64Array, RecordBatch, StringArray, UInt32Array,
};
use futures::TryStreamExt;
use lancedb::query::ExecutableQuery;
use sha2::{Digest, Sha256};

use shodh_rag::audit::AuditLog;
use shodh_rag::embeddings::EmbeddingModel;
use shodh_rag::folder_sync::{sync_folder, ManifestStore};
use shodh_rag::rag_engine::normalize_source_path;
use shodh_rag::statements::{
    DynamicsStore, EmbedderSource, Scope, StatementResult, StatementStore, SystemClock,
};
use shodh_rag::user_memory::{Actor, ListRequest, MemoryContent, MemoryService, Origin};
use shodh_rag::{Citation, DocumentFormat, RAGConfig, RAGEngine, SearchModels};

use library::{ChunkRow, Manifest, MemoryRow, SearchRecord, StoredFile};

/// The app version 0.0.10 wrote memories with.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

struct FixtureEmbedder;

impl EmbeddingModel for FixtureEmbedder {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(library::embed(text))
    }
    fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        Ok(library::embed(text))
    }
    fn dimension(&self) -> usize {
        library::DIMENSION
    }
}

struct FixtureEmbedderSource(Arc<dyn EmbeddingModel>);

#[async_trait::async_trait]
impl EmbedderSource for FixtureEmbedderSource {
    async fn embedder(&self) -> StatementResult<Arc<dyn EmbeddingModel>> {
        Ok(self.0.clone())
    }
}

fn config(data_dir: &Path) -> RAGConfig {
    let mut config = RAGConfig::default();
    config.data_dir = data_dir.to_path_buf();
    // No search models are installed; the fixture embedder is attached instead.
    config.embedding.model_dir = data_dir.join("models");
    config.embedding.use_e5 = false;
    config.embedding.dimension = library::DIMENSION;
    config
}

#[tokio::main]
async fn main() -> Result<()> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: write_upgrade_library <fixture folder>")?;
    if out.exists() {
        bail!(
            "{} exists: the fixture is written into a new folder",
            out.display()
        );
    }
    std::fs::create_dir_all(&out)?;
    // The absolute path (not canonicalize: on Windows that gives a `\\?\`
    // path, which no user's library holds): sources are stored as absolute paths.
    let out = std::path::absolute(&out)?;
    let files_dir = out.join(library::FILES_DIR);
    let data_dir = out.join(library::DATA_DIR);
    let files = library::files();
    for f in &files {
        let path = files_dir.join(&f.path);
        std::fs::create_dir_all(path.parent().expect("a file has a folder"))?;
        std::fs::write(&path, &f.text)?;
    }

    let embedder: Arc<dyn EmbeddingModel> = Arc::new(FixtureEmbedder);
    let mut engine = RAGEngine::new(config(&data_dir)).await?;
    engine.attach_search_models(SearchModels::from_embedder(embedder.clone()))?;

    // The Library folder source, synced by 0.0.10's folder sync, which keeps
    // its manifest (each file's stamp and chunker version) in the data folder.
    let rag = tokio::sync::RwLock::new(engine);
    let report = sync_folder(
        &files_dir.join(library::FOLDER).to_string_lossy(),
        library::FOLDER_SOURCE,
        &ManifestStore::in_dir(data_dir.join(library::FOLDER_MANIFESTS)),
        &rag,
        || false,
    )
    .await
    .map_err(|e| anyhow::anyhow!("syncing the folder source: {e}"))?;
    let in_folder = files.iter().filter(|f| f.in_folder()).count();
    if report.indexed != in_folder {
        bail!(
            "the folder sync indexed {} of {in_folder} files",
            report.indexed
        );
    }
    let mut engine = rag.into_inner();

    // The other files one by one, as the Library's single files are.
    for f in files.iter().filter(|f| !f.late && !f.in_folder()) {
        index_file(&mut engine, &files_dir, f).await?;
    }
    let note_metadata = HashMap::from([
        ("source".to_string(), library::NOTE_SOURCE.to_string()),
        ("title".to_string(), "Expedition note".to_string()),
    ]);
    engine
        .add_document(
            library::NOTE_TEXT,
            DocumentFormat::TXT,
            note_metadata,
            Citation::default(),
        )
        .await
        .context("adding the note")?;

    // 0.0.10's vector index: an L2 IVF-PQ index, built from 1,000 rows.
    let rows = engine.get_statistics().await?["total_chunks"].parse::<usize>()?;
    if rows < 1_000 {
        bail!("{rows} chunks: 0.0.10 builds its vector index from 1,000");
    }
    engine
        .optimize()
        .await
        .context("building 0.0.10's vector index")?;

    // A removal (0.0.10 compacts the table and updates its index after each),
    // then rows its index does not cover.
    for f in files.iter().filter(|f| f.removed) {
        let path = files_dir.join(&f.path);
        let removed = engine
            .delete_by_source(&path.to_string_lossy())
            .await
            .with_context(|| format!("removing {}", f.path))?;
        if removed == 0 {
            bail!("removing {} removed no chunks", f.path);
        }
    }
    for f in files.iter().filter(|f| f.late) {
        index_file(&mut engine, &files_dir, f).await?;
    }

    // Memories, in the memory store the app opens (lance_data and shodh.db
    // in the data folder), with the audit log beside them.
    let database = data_dir.join("shodh.db");
    let lance_dir = data_dir.join("lance_data");
    let audit = Arc::new(
        AuditLog::open(&database, None)
            .map_err(|e| anyhow::anyhow!("opening the audit log: {e}"))?,
    );
    let ontology = Arc::new(
        shodh_ontology::Ontology::builtin_with_packs(&["research"])
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    );
    let store = StatementStore::open(
        &lance_dir,
        library::DIMENSION,
        ontology,
        Arc::new(DynamicsStore::open(&database, None).map_err(|e| anyhow::anyhow!("{e}"))?),
        Arc::new(FixtureEmbedderSource(embedder.clone())),
        Arc::new(SystemClock),
    )
    .await
    .map_err(|e| anyhow::anyhow!("opening the memory store: {e}"))?;
    let memory = MemoryService::new(Arc::new(store), Some(audit), APP_VERSION);
    let mut tokens = BTreeMap::new();
    for (token, text, workspace) in library::memories() {
        let scope = match workspace {
            Some(id) => Scope::Workspace(id.to_string()),
            None => Scope::Global,
        };
        let outcome = memory
            .remember(
                MemoryContent::Note {
                    text: text.to_string(),
                },
                scope,
                &Origin::user_interface(APP_VERSION),
                &Actor::ui(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("remembering {token}: {e}"))?;
        let id = outcome
            .created_id()
            .with_context(|| format!("remembering {token} added no memory"))?
            .to_string();
        tokens.insert(id, token);
    }

    // What 0.0.10 reads back.
    let listed = memory
        .list(&ListRequest::default())
        .await
        .map_err(|e| anyhow::anyhow!("listing memories: {e}"))?;
    let mut memories = Vec::new();
    for record in listed {
        let token = tokens
            .get(&record.id)
            .with_context(|| format!("an unexpected memory {}", record.id))?;
        memories.push(MemoryRow {
            id: record.id.clone(),
            token: token.to_string(),
            class: record.class.clone(),
            text: record.text.clone(),
            scope: match &record.scope {
                Scope::Global => "global".to_string(),
                Scope::Workspace(id) => id.clone(),
            },
            source: record.source.clone(),
            created_at: record.created_at.to_rfc3339(),
        });
    }
    if memories.len() != tokens.len() {
        bail!("{} memories saved, {} listed", tokens.len(), memories.len());
    }
    memories.sort_by(|a, b| a.id.cmp(&b.id));

    let mut searches = Vec::new();
    let mut queries: Vec<String> = files
        .iter()
        .filter(|f| f.kind() != library::Kind::Secret)
        .map(|f| f.token.clone())
        .collect();
    queries.push(library::NOTE_TOKEN.to_string());
    for query in queries {
        let results = engine.search(&query, 5).await?;
        searches.push(SearchRecord {
            query,
            chunk_ids: results.iter().map(|r| r.id.to_string()).collect(),
            sources: results.into_iter().map(|r| r.source).collect(),
        });
    }
    let keyword_documents = engine.get_statistics().await?["fts_indexed"].parse::<usize>()?;
    drop(engine);

    let stored_files = files
        .iter()
        .map(|f| StoredFile {
            path: f.path.clone(),
            source: normalize_source_path(&files_dir.join(&f.path)),
            token: f.token.clone(),
            removed: f.removed,
        })
        .collect::<Vec<_>>();

    let db = lancedb::connect(&lance_dir.to_string_lossy())
        .execute()
        .await?;
    let documents = db.open_table("documents").execute().await?;
    let chunks = read_chunks(&documents).await?;
    let configs = documents.list_indices().await?;
    let indices = configs
        .iter()
        .map(|i| {
            (
                i.name.clone(),
                format!("{:?}", i.index_type),
                i.columns.clone(),
            )
        })
        .collect::<Vec<_>>();
    let vector_index = configs
        .iter()
        .find(|i| i.columns.iter().any(|c| c == "vector"))
        .with_context(|| format!("0.0.10 built no vector index: {indices:?}"))?;
    let stats = documents
        .index_stats(&vector_index.name)
        .await?
        .context("0.0.10's vector index has no statistics")?;
    let vector_index_rows = (stats.num_indexed_rows, stats.num_unindexed_rows);
    let statement_rows = db
        .open_table("statements")
        .execute()
        .await?
        .count_rows(None)
        .await?;

    for f in &stored_files {
        let present = chunks.values().any(|c| c.source == f.source);
        if present == f.removed {
            bail!(
                "{}: chunks present {present}, removed {}",
                f.path,
                f.removed
            );
        }
    }

    let manifest = Manifest {
        written_by: APP_VERSION.to_string(),
        fixture_dir: out.to_string_lossy().to_string(),
        files: stored_files,
        chunks,
        indices,
        vector_index_rows,
        keyword_documents,
        searches,
        memories,
        statement_rows,
    };
    std::fs::write(
        out.join(library::MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    println!(
        "Wrote a {} library in {}: {} chunks, {} keyword documents, {} memories, indices {:?}",
        manifest.written_by,
        out.display(),
        manifest.chunks.len(),
        manifest.keyword_documents,
        manifest.memories.len(),
        manifest.indices
    );
    Ok(())
}

/// Indexes `f` by its path, with its library source, as the app's folder
/// sources do.
async fn index_file(
    engine: &mut RAGEngine,
    files_dir: &Path,
    f: &library::LibraryFile,
) -> Result<()> {
    let path = files_dir.join(&f.path);
    let metadata = HashMap::from([("space_id".to_string(), f.space.to_string())]);
    let ids = engine
        .add_document_from_file(&path, metadata)
        .await
        .with_context(|| format!("indexing {}", f.path))?;
    if ids.is_empty() {
        bail!("{} gave no chunks", f.path);
    }
    Ok(())
}

fn strings<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .with_context(|| format!("no string column {name}"))
}

/// Every row of the documents table, every column.
async fn read_chunks(table: &lancedb::Table) -> Result<BTreeMap<String, ChunkRow>> {
    let batches: Vec<RecordBatch> = table.query().execute().await?.try_collect().await?;
    let mut rows = BTreeMap::new();
    for batch in &batches {
        let id = strings(batch, "id")?;
        let doc_id = strings(batch, "doc_id")?;
        let text = strings(batch, "text")?;
        let title = strings(batch, "title")?;
        let source = strings(batch, "source")?;
        let heading = strings(batch, "heading")?;
        let space_id = strings(batch, "space_id")?;
        let metadata_json = strings(batch, "metadata_json")?;
        let citation_json = strings(batch, "citation_json")?;
        let chunk_index = batch
            .column_by_name("chunk_index")
            .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
            .context("no chunk_index column")?;
        let created_at = batch
            .column_by_name("created_at")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .context("no created_at column")?;
        let vectors = batch
            .column_by_name("vector")
            .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
            .context("no vector column")?;
        for i in 0..batch.num_rows() {
            let vector = vectors.value(i);
            let vector = vector
                .as_any()
                .downcast_ref::<Float32Array>()
                .context("vector items are not f32")?;
            let row = ChunkRow {
                id: id.value(i).to_string(),
                doc_id: doc_id.value(i).to_string(),
                chunk_index: chunk_index.value(i),
                text: text.value(i).to_string(),
                title: title.value(i).to_string(),
                source: source.value(i).to_string(),
                heading: heading.value(i).to_string(),
                vector_sha256: library::vector_digest(vector.values(), |b| {
                    Sha256::digest(b).to_vec()
                }),
                space_id: space_id.value(i).to_string(),
                metadata_json: metadata_json.value(i).to_string(),
                citation_json: citation_json.value(i).to_string(),
                created_at: created_at.value(i),
            };
            if rows.insert(row.id.clone(), row).is_some() {
                bail!("chunk id {} is stored twice", id.value(i));
            }
        }
    }
    Ok(rows)
}
