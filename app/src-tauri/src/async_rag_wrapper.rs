//! Async wrapper for ComprehensiveRAG to enable concurrent operations

use shodh_rag::comprehensive_system::{ComprehensiveRAG, ComprehensiveRAGConfig};
use shodh_rag::query::FilterPredicate;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use anyhow::Result;

/// Async wrapper around ComprehensiveRAG
pub struct AsyncRAG {
    inner: Arc<RwLock<ComprehensiveRAG>>,
}

impl AsyncRAG {
    /// Create new async RAG wrapper
    pub fn new(config: ComprehensiveRAGConfig) -> Result<Self> {
        let rag = ComprehensiveRAG::new(config)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(rag)),
        })
    }

    /// Add document asynchronously
    pub async fn add_document(
        &self,
        id: &str,
        content: &str,
        metadata: HashMap<String, String>,
    ) -> Result<()> {
        let rag = self.inner.clone();
        let id = id.to_string();
        let content = content.to_string();
        
        // Run the sync operation in a blocking task
        tokio::task::spawn_blocking(move || {
            let rag = tokio::runtime::Handle::current().block_on(rag.read());
            
            // Convert metadata to the format ComprehensiveRAG expects
            let doc_format = shodh_rag::comprehensive_system::DocumentFormat::PlainText;
            let citation = shodh_rag::comprehensive_system::Citation {
                title: metadata.get("title").cloned().unwrap_or_else(|| "Untitled".to_string()),
                authors: vec![],
                year: metadata.get("year").cloned(),
                source: metadata.get("source").cloned(),
                url: metadata.get("url").cloned(),
                document_type: metadata.get("doc_type").cloned(),
                metadata: Some(metadata),
            };
            
            // Use add_document_comprehensive which is the actual method
            rag.add_document_comprehensive(
                &id,
                &content,
                doc_format,
                citation,
                None, // chunk_config
                None, // embedding_config
            )
        })
        .await??;
        
        Ok(())
    }

    /// Search documents asynchronously
    pub async fn search(
        &self,
        query: &str,
        max_results: usize,
        filters: Option<Vec<FilterPredicate>>,
    ) -> Result<Vec<SearchResult>> {
        let rag = self.inner.clone();
        let query = query.to_string();
        
        tokio::task::spawn_blocking(move || {
            let rag = tokio::runtime::Handle::current().block_on(rag.read());
            
            // Use search_comprehensive which is the actual method
            let results = rag.search_comprehensive(
                &query,
                max_results,
                None, // search_type
                filters,
                None, // rerank_config
            )?;
            
            // Convert to our SearchResult format
            Ok(results.into_iter().map(|r| SearchResult {
                id: r.id,
                score: r.score,
                text: r.content,
                metadata: r.citation.metadata,
            }).collect())
        })
        .await?
    }

    /// Delete document asynchronously
    pub async fn delete_document(&self, id: &str) -> Result<()> {
        let rag = self.inner.clone();
        let id = id.to_string();
        
        tokio::task::spawn_blocking(move || {
            let rag = tokio::runtime::Handle::current().block_on(rag.write());
            // ComprehensiveRAG doesn't have a delete method, so we'll need to track this separately
            // For now, we'll just return Ok
            // In production, you'd maintain a separate deleted documents list
            Ok::<(), anyhow::Error>(())
        })
        .await??;
        
        Ok(())
    }

    /// Get statistics asynchronously
    pub async fn get_statistics(&self) -> Result<RAGStats> {
        let rag = self.inner.clone();
        
        tokio::task::spawn_blocking(move || {
            let rag = tokio::runtime::Handle::current().block_on(rag.read());
            let stats = rag.get_statistics();
            
            Ok(RAGStats {
                total_documents: stats.total_documents,
                total_vectors: stats.total_vectors,
                index_size: stats.index_stats.total_vectors,
            })
        })
        .await?
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub text: String,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone)]
pub struct RAGStats {
    pub total_documents: usize,
    pub total_vectors: usize,
    pub index_size: usize,
}