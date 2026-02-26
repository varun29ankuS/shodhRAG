pub mod e5;
pub mod tokenizer;

use anyhow::Result;

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
