//! A fresh engine over a corpus: a throwaway data directory, the search
//! models from an explicit folder, and the app's own folder indexer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use shodh_rag::config::{OcrConfig, OcrEngineChoice, RAGConfig};
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

/// Engine settings that differ from the app's defaults for one run, as
/// `key=value` pairs (`retrieval --search rrf_k=20,source_decay=0.7`), to
/// measure what a setting does before changing its default. Search keys:
/// `rrf_k`, `candidate_multiplier`, `min_score_threshold`, `score_weight`,
/// `source_decay`, `pool_source_cap`, `pool_open_share`, `reranking` (`on`/`off`); indexing key:
/// `chunk_max_tokens`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tweaks {
    pairs: Vec<(String, String)>,
}

impl Tweaks {
    /// Parse `key=value,key=value`; every key must be known and every value
    /// valid for it.
    pub fn parse(spec: &str) -> Result<Self> {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for item in spec.split(',').map(str::trim).filter(|i| !i.is_empty()) {
            let Some((key, value)) = item.split_once('=') else {
                bail!("'{item}' is not key=value");
            };
            let (key, value) = (key.trim().to_string(), value.trim().to_string());
            if pairs.iter().any(|(k, _)| *k == key) {
                bail!("{key} is given twice");
            }
            pairs.push((key, value));
        }
        let tweaks = Self { pairs };
        tweaks.apply(&mut RAGConfig::default())?;
        Ok(tweaks)
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Set every pair on `config` and validate the result.
    pub fn apply(&self, config: &mut RAGConfig) -> Result<()> {
        fn number<T: std::str::FromStr>(key: &str, value: &str) -> Result<T> {
            value
                .parse()
                .map_err(|_| anyhow::anyhow!("{key} must be a number, got '{value}'"))
        }
        for (key, value) in &self.pairs {
            match key.as_str() {
                "rrf_k" => config.search.rrf_k = number(key, value)?,
                "candidate_multiplier" => config.search.candidate_multiplier = number(key, value)?,
                "min_score_threshold" => config.search.min_score_threshold = number(key, value)?,
                "score_weight" => config.search.score_weight = number(key, value)?,
                "source_decay" => config.search.source_decay = number(key, value)?,
                "pool_source_cap" => config.search.pool_source_cap = number(key, value)?,
                "pool_open_share" => config.search.pool_open_share = number(key, value)?,
                "chunk_max_tokens" => config.chunking.max_tokens = number(key, value)?,
                "reranking" => {
                    config.features.enable_reranking = match value.as_str() {
                        "on" => true,
                        "off" => false,
                        other => bail!("reranking must be on or off, got '{other}'"),
                    }
                }
                other => bail!("unknown setting '{other}'"),
            }
        }
        config.validate().map_err(anyhow::Error::msg)
    }
}

/// Engine settings recorded in reports.
pub fn settings() -> BTreeMap<String, String> {
    settings_with(&Tweaks::default())
}

/// Engine settings recorded in reports, with `tweaks` applied.
pub fn settings_with(tweaks: &Tweaks) -> BTreeMap<String, String> {
    let mut config = RAGConfig::default();
    // `Tweaks::parse` validated the pairs; a failure here leaves the defaults
    // and is reported by the run itself.
    let _ = tweaks.apply(&mut config);
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
        (
            "candidate_multiplier",
            config.search.candidate_multiplier.to_string(),
        ),
        (
            "min_score_threshold",
            config.search.min_score_threshold.to_string(),
        ),
        ("score_weight", config.search.score_weight.to_string()),
        ("source_decay", config.search.source_decay.to_string()),
        ("pool_source_cap", config.search.pool_source_cap.to_string()),
        ("pool_open_share", config.search.pool_open_share.to_string()),
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
/// new temporary index, reading scans with the app's default OCR choice.
pub async fn index_corpus(corpus: &Path, models: &Path) -> Result<IndexedCorpus> {
    index_corpus_with(corpus, models, &OcrConfig::default(), &Tweaks::default()).await
}

/// [`index_corpus`] with the OCR engine and render resolutions of `ocr` and
/// the engine settings changed by `tweaks`. Choosing PP-OCRv6 fails when its
/// model is not in `models`, rather than quietly measuring another engine.
pub async fn index_corpus_with(
    corpus: &Path,
    models: &Path,
    ocr: &OcrConfig,
    tweaks: &Tweaks,
) -> Result<IndexedCorpus> {
    if ocr.engine == OcrEngineChoice::PpOcr
        && !shodh_rag::processing::ocr::is_installed(&shodh_rag::processing::ocr::model_dir(models))
    {
        bail!(
            "the OCR model is not in {} (install it with `shodh-eval models --models {}`)",
            models.display(),
            models.display()
        );
    }
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
    // The eval measures the CPU path, the one every machine has (CI's
    // runners have no graphics card).
    config.embedding.compute = shodh_rag::compute::provider::ComputePreference::Cpu;
    config.ocr = ocr.clone();
    tweaks.apply(&mut config)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tweaks_set_known_keys_and_reject_the_rest() {
        let tweaks =
            Tweaks::parse("rrf_k=20, source_decay=0.8,reranking=off,chunk_max_tokens=256").unwrap();
        let mut config = RAGConfig::default();
        tweaks.apply(&mut config).unwrap();
        assert_eq!(config.search.rrf_k, 20);
        assert!((config.search.source_decay - 0.8).abs() < 1e-6);
        assert!(!config.features.enable_reranking);
        assert_eq!(config.chunking.max_tokens, 256);
        let recorded = settings_with(&tweaks);
        assert_eq!(recorded["rrf_k"], "20");
        assert_eq!(recorded["reranking"], "false");
        assert!(Tweaks::parse("").unwrap().is_empty());
        for bad in [
            "rrf_k",
            "rrf_k=x",
            "unknown=1",
            "rrf_k=1,rrf_k=2",
            "source_decay=1.5",
            "reranking=maybe",
        ] {
            assert!(Tweaks::parse(bad).is_err(), "{bad}");
        }
    }
}
