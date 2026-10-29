use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RAGConfig {
    pub data_dir: PathBuf,
    pub embedding: EmbeddingConfig,
    pub chunking: ChunkingConfig,
    pub search: SearchConfig,
    pub features: FeatureFlags,
    /// OCR of scanned PDFs and images.
    #[serde(default)]
    pub ocr: OcrConfig,
}

/// Which OCR engine reads scanned PDFs and images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngineChoice {
    /// PP-OCRv6 (the scan reader) when its model is installed, on every
    /// platform; without it the system OCR on Windows, and no OCR elsewhere.
    /// Scans with JBIG2 or JPEG 2000 images go to the system OCR on Windows.
    /// On the scanned evaluation set PP-OCRv6 found far more passages
    /// (passage_hit@10 1.0 against 0.67) but ranked the right page first
    /// slightly less often (page_hit@1 0.88 against 0.92) and is slower per
    /// page; it is the default by decision (see `crates/shodh-eval/README.md`).
    #[default]
    Auto,
    /// The system OCR only (Windows.Media.Ocr; none on other platforms).
    System,
    /// PP-OCRv6 only: scanned pages stay unread while its model is missing
    /// (JBIG2 and JPEG 2000 scans still go to the system OCR on Windows).
    PpOcr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrConfig {
    #[serde(default)]
    pub engine: OcrEngineChoice,
    /// Resolution PDF pages are rendered at for OCR. The system OCR keeps its
    /// historical rendering (the page size in device-independent pixels, 96 dpi)
    /// unless `system_render_dpi` is set.
    #[serde(default = "default_render_dpi")]
    pub render_dpi: u32,
    /// Resolution PDF pages are rendered at for the system OCR; `None`: 96 dpi.
    #[serde(default)]
    pub system_render_dpi: Option<u32>,
}

fn default_render_dpi() -> u32 {
    crate::processing::ocr::raster::RENDER_DPI
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            engine: OcrEngineChoice::default(),
            render_dpi: default_render_dpi(),
            system_render_dpi: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub model_dir: PathBuf,
    pub dimension: usize,
    pub use_e5: bool,
    pub cache_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkingConfig {
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub min_chunk_size: usize,
    /// Token budget for structure-aware chunks (PDF, LaTeX, Markdown),
    /// counted with the embedder's tokenizer, context prefix excluded.
    #[serde(default = "default_max_chunk_tokens")]
    pub max_tokens: usize,
}

fn default_max_chunk_tokens() -> usize {
    crate::processing::structure_chunker::DEFAULT_CHUNK_TOKENS
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    pub default_k: usize,
    pub candidate_multiplier: usize,
    pub min_score_threshold: f32,
    pub hybrid_alpha: f32,
    pub rrf_k: usize,
    /// Weight for original similarity scores in RRF fusion (0.0 = pure RRF, higher = more score influence)
    pub score_weight: f32,
    /// Diminishing returns per source file after reranking: a result's score is
    /// multiplied by this once for every result of the same file ranked above it
    /// (1.0 = no diversity, 0.0 = one result per file first). In 0..=1.
    #[serde(default = "default_source_decay")]
    pub source_decay: f32,
    /// Most candidates one source file may hold in the pool the reranker
    /// reads (the fused vector and keyword candidates, cut to
    /// `k * candidate_multiplier`). The files that fill it are left out of
    /// one more vector search, and the pool is filled from the next-best
    /// candidates of other files first, then with the rest. Without it a few
    /// long files (surveys, theses) took every candidate of a broad question.
    /// 0 = no limit, the default: on the eval's 33-file paper library every
    /// limit tried (2 to 4, with and without an open head) raised multi-paper
    /// recall but lowered `depth_recall@20`, the questions that need several
    /// chunks of one paper (`crates/shodh-eval`, README).
    #[serde(default = "default_pool_source_cap")]
    pub pool_source_cap: usize,
    /// Share of the pool (0..=1) filled in score order before
    /// `pool_source_cap` applies, so the best candidates of one file (what a
    /// question about that file needs) are never displaced.
    #[serde(default = "default_pool_open_share")]
    pub pool_open_share: f32,
}

fn default_pool_open_share() -> f32 {
    0.0
}

fn default_pool_source_cap() -> usize {
    0
}

fn default_source_decay() -> f32 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlags {
    pub enable_reranking: bool,
    pub enable_cross_encoder: bool,
}

impl RAGConfig {
    /// Validate config values, returning errors for clearly broken configurations.
    pub fn validate(&self) -> Result<(), String> {
        if self.embedding.dimension == 0 {
            return Err("embedding.dimension must be > 0".into());
        }
        if self.chunking.chunk_size < 50 {
            return Err("chunking.chunk_size must be >= 50".into());
        }
        if self.chunking.chunk_overlap >= self.chunking.chunk_size {
            return Err("chunking.chunk_overlap must be < chunk_size".into());
        }
        if self.search.default_k == 0 {
            return Err("search.default_k must be > 0".into());
        }
        if self.search.candidate_multiplier == 0 {
            return Err("search.candidate_multiplier must be > 0".into());
        }
        if !(0.0..=1.0).contains(&self.search.min_score_threshold) {
            return Err("search.min_score_threshold must be in [0.0, 1.0]".into());
        }
        if !(0.0..=1.0).contains(&self.search.pool_open_share) {
            return Err("search.pool_open_share must be in [0.0, 1.0]".into());
        }
        if !(0.0..=1.0).contains(&self.search.source_decay) {
            return Err("search.source_decay must be in [0.0, 1.0]".into());
        }
        let dpi_ok = |dpi: u32| (72..=600).contains(&dpi);
        if !dpi_ok(self.ocr.render_dpi) || self.ocr.system_render_dpi.is_some_and(|d| !dpi_ok(d)) {
            return Err("ocr render resolutions must be between 72 and 600 dpi".into());
        }
        Ok(())
    }

    /// Load config from a JSON file, falling back to defaults for missing fields.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;
        let config: Self =
            serde_json::from_str(&content).map_err(|e| format!("Failed to parse config: {}", e))?;
        config.validate()?;
        Ok(config)
    }
}

impl Default for RAGConfig {
    fn default() -> Self {
        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("shodh-rag");

        let model_dir = if Path::new("models").exists() {
            PathBuf::from("models")
        } else if let Ok(env_path) = std::env::var("MODEL_PATH") {
            PathBuf::from(env_path)
        } else {
            data_dir.join("models")
        };

        let e5_available = model_dir.join("multilingual-e5-base").exists();
        let dimension = if e5_available { 768 } else { 384 };

        Self {
            data_dir,
            embedding: EmbeddingConfig {
                model_dir,
                dimension,
                use_e5: e5_available,
                cache_size: 1000,
            },
            chunking: ChunkingConfig {
                chunk_size: 1750,
                chunk_overlap: 200,
                min_chunk_size: 100,
                max_tokens: default_max_chunk_tokens(),
            },
            search: SearchConfig {
                default_k: 10,
                candidate_multiplier: 5,
                min_score_threshold: 0.02,
                hybrid_alpha: 0.7,
                rrf_k: 60,
                score_weight: 0.3,
                source_decay: default_source_decay(),
                pool_source_cap: default_pool_source_cap(),
                pool_open_share: default_pool_open_share(),
            },
            features: FeatureFlags {
                enable_reranking: true,
                enable_cross_encoder: true,
            },
            ocr: OcrConfig::default(),
        }
    }
}
