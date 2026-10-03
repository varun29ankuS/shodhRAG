pub mod e5;
pub mod model_store;
pub mod tokenizer;

use anyhow::Result;

/// Stable code at the start of [`SearchModelsMissing`]'s message, so callers
/// that only see the error text (Tauri commands, agent tools) can recognise it.
pub const SEARCH_MODELS_MISSING_CODE: &str = "search_models_missing";

/// The engine was started without its embedding model (first run, or the
/// model files were removed). Search and indexing need it; install the
/// pinned models (see [`model_store`]) and attach them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("search_models_missing: the search models are not installed yet. Open Ask or Library and choose \"Set up search\".")]
pub struct SearchModelsMissing;

/// Unified embedding model trait
pub trait EmbeddingModel: Send + Sync {
    /// Embed a search query (with appropriate prefix for the model)
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed a document/passage (with appropriate prefix for the model)
    fn embed_document(&self, text: &str) -> Result<Vec<f32>>;

    /// Batch embed documents for ingestion
    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed_document(t)).collect()
    }

    /// Embedding vector dimension
    fn dimension(&self) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_models_message_starts_with_the_stable_code() {
        assert!(SearchModelsMissing
            .to_string()
            .starts_with(SEARCH_MODELS_MISSING_CODE));
    }
}
