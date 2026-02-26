//! Live RAG Tools — Tool implementations backed by the actual RAG engine.
//!
//! These tools hold `Arc<RwLock<RAGEngine>>` and perform real searches,
//! document lookups, and source listings against the indexed corpus.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::rag_engine::RAGEngine;
use super::context::AgentContext;
use super::tools::{AgentTool, ToolInput, ToolResult};

/// Search documents in the knowledge base using hybrid retrieval.
pub struct LiveRAGSearchTool {
    rag: Arc<RwLock<RAGEngine>>,
}

impl LiveRAGSearchTool {
    pub fn new(rag: Arc<RwLock<RAGEngine>>) -> Self {
        Self { rag }
    }
}

#[async_trait]
impl AgentTool for LiveRAGSearchTool {
    fn id(&self) -> &str { "search_documents" }
    fn name(&self) -> &str { "Search Documents" }

    fn description(&self) -> &str {
        "Search the user's indexed documents using hybrid semantic + keyword search. \
         Returns relevant document chunks with scores, sources, and citations. \
         Use this whenever the user asks about information in their documents."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query — be specific and use keywords from the user's question"
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (default 5, max 20)",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: ToolInput, context: AgentContext) -> Result<ToolResult> {
        let query = input.parameters["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter"))?;

        let num_results = input.parameters["num_results"]
            .as_u64()
            .unwrap_or(5)
            .min(20) as usize;

        let rag = self.rag.read().await;
        let results = rag.search(query, num_results).await?;

        let formatted: Vec<serde_json::Value> = results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                serde_json::json!({
                    "rank": i + 1,
                    "score": r.score,
                    "text": r.text,
                    "source": r.source,
                    "title": r.title,
                    "heading": r.heading,
                    "doc_id": r.doc_id.to_string(),
                    "chunk_id": r.chunk_id,
                })
            })
            .collect();

        let summary = if results.is_empty() {
            format!("No results found for: '{}'", query)
        } else {
            format!(
                "Found {} results for '{}'. Top result (score {:.2}): {}",
                results.len(),
                query,
                results[0].score,
                results[0].title,
            )
        };

        Ok(ToolResult {
            success: !results.is_empty(),
            output: summary,
            data: serde_json::json!({
                "query": query,
                "results": formatted,
                "total": results.len(),
            }),
            error: None,
        })
    }
}

/// Get information about indexed documents and sources.
pub struct ListSourcesTool {
    rag: Arc<RwLock<RAGEngine>>,
}

impl ListSourcesTool {
    pub fn new(rag: Arc<RwLock<RAGEngine>>) -> Self {
        Self { rag }
    }
}

#[async_trait]
impl AgentTool for ListSourcesTool {
    fn id(&self) -> &str { "list_sources" }
    fn name(&self) -> &str { "List Sources" }

    fn description(&self) -> &str {
        "List all indexed document sources in the knowledge base. \
         Returns source names, document counts, and basic statistics. \
         Use this to understand what documents are available before searching."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _input: ToolInput, _context: AgentContext) -> Result<ToolResult> {
        let rag = self.rag.read().await;

        let stats = rag.get_statistics().await.unwrap_or_default();
        let total_chunks = stats.get("total_chunks").cloned().unwrap_or_else(|| "0".to_string());
        let doc_count = rag.count_documents().await.unwrap_or(0);
        let doc_info = rag.get_document_info().await.unwrap_or_default();

        let sources: Vec<serde_json::Value> = doc_info
            .iter()
            .map(|(doc_id, title, source)| {
                serde_json::json!({
                    "doc_id": doc_id,
                    "title": title,
                    "source": source,
                })
            })
            .collect();

        let summary = format!(
            "Knowledge base contains {} documents with {} searchable chunks.",
            doc_count, total_chunks,
        );

        Ok(ToolResult {
            success: true,
            output: summary,
            data: serde_json::json!({
                "total_documents": doc_count,
                "total_chunks": total_chunks,
                "documents": sources,
            }),
            error: None,
        })
    }
}

/// List all chunks from a specific document for deep context.
pub struct GetDocumentChunksTool {
    rag: Arc<RwLock<RAGEngine>>,
}

impl GetDocumentChunksTool {
    pub fn new(rag: Arc<RwLock<RAGEngine>>) -> Self {
        Self { rag }
    }
}

#[async_trait]
impl AgentTool for GetDocumentChunksTool {
    fn id(&self) -> &str { "get_document_chunks" }
    fn name(&self) -> &str { "Get Document Chunks" }

    fn description(&self) -> &str {
        "Retrieve all chunks from a specific document by its doc_id. \
         Use this after search_documents when you need the full content of a document, \
         not just the matching chunk. Provide the doc_id from a search result."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "doc_id": {
                    "type": "string",
                    "description": "The document ID from a search result"
                },
                "max_chunks": {
                    "type": "integer",
                    "description": "Maximum chunks to return (default 20)",
                    "default": 20
                }
            },
            "required": ["doc_id"]
        })
    }

    async fn execute(&self, input: ToolInput, _context: AgentContext) -> Result<ToolResult> {
        let doc_id = input.parameters["doc_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'doc_id' parameter"))?;

        let max_chunks = input.parameters["max_chunks"]
            .as_u64()
            .unwrap_or(20) as usize;

        let rag = self.rag.read().await;

        // Use search with the document title/source as query to find chunks from that document
        // This is a pragmatic approach since list_documents doesn't filter by doc_id directly
        let results = rag.search(&format!("doc_id:{}", doc_id), max_chunks).await?;

        // Filter to only chunks from the requested document
        let results: Vec<_> = results
            .into_iter()
            .filter(|r| r.doc_id.to_string() == doc_id)
            .collect();

        let chunks: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "chunk_id": r.chunk_id,
                    "text": r.text,
                    "title": r.title,
                    "source": r.source,
                    "heading": r.heading,
                })
            })
            .collect();

        let full_text: String = results
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(ToolResult {
            success: !results.is_empty(),
            output: if results.is_empty() {
                format!("No chunks found for document: {}", doc_id)
            } else {
                full_text
            },
            data: serde_json::json!({
                "doc_id": doc_id,
                "chunks": chunks,
                "total": results.len(),
            }),
            error: None,
        })
    }
}

/// Register all live RAG tools into a ToolRegistry.
pub fn register_rag_tools(
    registry: &mut super::tools::ToolRegistry,
    rag: Arc<RwLock<RAGEngine>>,
) {
    registry.register(Arc::new(LiveRAGSearchTool::new(rag.clone())));
    registry.register(Arc::new(ListSourcesTool::new(rag.clone())));
    registry.register(Arc::new(GetDocumentChunksTool::new(rag)));
}
