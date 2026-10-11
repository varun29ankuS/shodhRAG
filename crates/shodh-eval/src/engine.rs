//! A fresh engine over a corpus: a throwaway data directory, the search
//! models from an explicit folder, and the app's own folder indexer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use shodh_rag::config::RAGConfig;
use shodh_rag::indexing::{index_folder, IndexingOptions, IndexingState};
use shodh_rag::types::ComprehensiveResult;
use shodh_rag::RAGEngine;
use tokio::sync::RwLock;

use crate::corpus::{relative_key, root_key, supported_files};
use crate::metrics::RetrievedChunk;
use crate::report::IngestSummary;

/// The source id ("space") the corpus is indexed under.
pub const SPACE_ID: &str = "shodh-eval";

/// An indexed corpus. The index lives in a temporary directory removed when
/// this is dropped.
pub struct IndexedCorpus {
    pub rag: Arc<RwLock<RAGEngine>>,
    pub root: PathBuf,
    pub root_key: String,
    pub ingest: IngestSummary,
    data_dir: tempfile::TempDir,
}

impl IndexedCorpus {
    /// The temporary data directory (for components that keep state next
    /// to the index).
    pub fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }

    /// The document key of a result's source.
    pub fn key_of(&self, result: &ComprehensiveResult) -> Option<String> {
        relative_key(&self.root_key, &result_source(result))
    }

    pub fn chunk(&self, result: &ComprehensiveResult) -> RetrievedChunk {
        RetrievedChunk {
            key: self.key_of(result),
            pages: result_pages(&result.metadata),
            text: result.snippet.clone(),
        }
    }
}

/// The file a result came from, as the engine stored it.
pub fn result_source(result: &ComprehensiveResult) -> String {
    if result.citation.source.is_empty() {
        result
            .metadata
            .get("file_path")
            .or_else(|| result.metadata.get("source_file"))
            .cloned()
            .unwrap_or_default()
    } else {
        result.citation.source.clone()
    }
}

/// First and last page from the chunk metadata written at indexing time
/// (`page_start`/`page_end`, else `page`).
pub fn result_pages(metadata: &std::collections::HashMap<String, String>) -> Option<(u32, u32)> {
    let read = |key: &str| metadata.get(key).and_then(|v| v.trim().parse::<u32>().ok());
    match (read("page_start"), read("page_end")) {
        (Some(start), Some(end)) => Some((start.min(end), start.max(end))),
        (Some(one), None) | (None, Some(one)) => Some((one, one)),
        (None, None) => read("page").map(|p| (p, p)),
    }
}

/// Engine settings recorded in reports.
pub fn settings() -> BTreeMap<String, String> {
    let config = RAGConfig::default();
    [
        (
            "embedding",
            shodh_rag::embeddings::model_store::E5_DIR.to_string(),
        ),
        (
            "reranker",
            shodh_rag::embeddings::model_store::RERANKER_DIR.to_string(),
        ),
        ("chunk_max_tokens", config.chunking.max_tokens.to_string()),
        ("rrf_k", config.search.rrf_k.to_string()),
        ("reranking", config.features.enable_reranking.to_string()),
        // The app also refines table pages with the table model in the
        // background and fuses a citation-graph ranking into paper queries;
        // neither runs here.
        ("table_refinement", "off".to_string()),
        ("citation_graph", "off".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Index `corpus` (an absolute folder) with the models in `models` into a
/// new temporary index.
pub async fn index_corpus(corpus: &Path, models: &Path) -> Result<IndexedCorpus> {
    let data_dir = tempfile::Builder::new()
        .prefix("shodh-eval-index-")
        .tempdir()
        .context("creating a temporary index directory")?;
    let mut config = RAGConfig {
        data_dir: data_dir.path().to_path_buf(),
        ..RAGConfig::default()
    };
    config.embedding.model_dir = models.to_path_buf();
    config.embedding.use_e5 = true;
    config.embedding.dimension = 768;
    let engine = RAGEngine::new(config).await.context("opening the engine")?;
    if !engine.has_search_models() {
        bail!(
            "the search models are not in {} (install them with `shodh-eval models --models {}`)",
            models.display(),
            models.display()
        );
    }
    let rag = Arc::new(RwLock::new(engine));

    let files_seen = supported_files(corpus)?.len();
    if files_seen == 0 {
        bail!("{} has no files the indexer supports", corpus.display());
    }
    let options = IndexingOptions {
        skip_indexed: false,
        watch_changes: false,
        process_subdirs: true,
        priority: "normal".to_string(),
        file_types: Vec::new(),
    };
    let started = Instant::now();
    let folder = corpus.to_string_lossy().to_string();
    let result = index_folder(
        &folder,
        SPACE_ID,
        &options,
        &rag,
        &IndexingState::default(),
        None,
    )
    .await
    .map_err(anyhow::Error::msg)
    .with_context(|| format!("indexing {}", corpus.display()))?;
    let root_key = root_key(corpus);
    let failures = result
        .failures
        .iter()
        .map(|f| {
            let file = relative_key(&root_key, &f.file).unwrap_or_else(|| f.file.clone());
            format!("{file}: {}", f.reason)
        })
        .collect();
    let ingest = IngestSummary {
        files_seen,
        files_indexed: result.files_processed,
        failures,
        chunks: result.total_chunks,
        seconds: started.elapsed().as_secs_f64(),
    };
    Ok(IndexedCorpus {
        rag,
        root: corpus.to_path_buf(),
        root_key,
        ingest,
        data_dir,
    })
}
