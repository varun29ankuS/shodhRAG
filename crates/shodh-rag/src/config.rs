use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RAGConfig {
    pub data_dir: PathBuf,
    pub embedding: EmbeddingConfig,
    pub chunking: ChunkingConfig,
    pub search: SearchConfig,
    pub features: FeatureFlags,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlags {
    pub enable_reranking: bool,
    pub enable_knowledge_graph: bool,
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
        Ok(())
    }

    /// Load config from a JSON file, falling back to defaults for missing fields.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;
        let config: Self = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config: {}", e))?;
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
            },
            search: SearchConfig {
                default_k: 10,
                candidate_multiplier: 3,
                min_score_threshold: 0.1,
                hybrid_alpha: 0.7,
                rrf_k: 60,
                score_weight: 0.3,
            },
            features: FeatureFlags {
                enable_reranking: true,
                enable_knowledge_graph: false,
                enable_cross_encoder: true,
            },
        }
    }
}
