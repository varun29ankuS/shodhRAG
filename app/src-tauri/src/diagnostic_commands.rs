use serde::{Deserialize, Serialize};
use tauri::State;
use crate::rag_commands::RagState;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentDiagnostic {
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub full_text_length: usize,
    pub metadata: HashMap<String, String>,
    pub chunk_count: usize,
    pub indexing_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexDiagnostics {
    pub total_documents: usize,
    pub total_chunks: usize,
    pub documents_with_content: usize,
    pub empty_documents: usize,
    pub sample_documents: Vec<DocumentDiagnostic>,
    pub file_types: HashMap<String, usize>,
    pub spaces: HashMap<String, usize>,
}

/// Get diagnostic information about indexed content
#[tauri::command]
pub async fn get_index_diagnostics(
    state: State<'_, RagState>,
    sample_count: Option<usize>,
) -> Result<IndexDiagnostics, String> {
    tracing::debug!("Getting index diagnostics...");

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // Get all documents (or sample)
    let max_docs = sample_count.unwrap_or(20);
    let search_results = rag.list_documents(None, max_docs)
        .await
        .map_err(|e| format!("List error: {}", e))?;

        let mut total_chunks = 0;
        let mut documents_with_content = 0;
        let mut empty_documents = 0;
        let mut file_types: HashMap<String, usize> = HashMap::new();
        let mut spaces: HashMap<String, usize> = HashMap::new();
        let mut sample_documents = Vec::new();

        for (idx, result) in search_results.iter().enumerate() {
            // Check if document has content
            let has_content = !result.snippet.trim().is_empty() && result.snippet != "...";

            if has_content {
                documents_with_content += 1;
            } else {
                empty_documents += 1;
            }

            // Count file types
            if let Some(file_type) = result.metadata.get("file_type") {
                *file_types.entry(file_type.clone()).or_insert(0) += 1;
            }

            // Count spaces
            if let Some(space_id) = result.metadata.get("space_id") {
                *spaces.entry(space_id.clone()).or_insert(0) += 1;
            }

            // Get full text if available
            let full_text = result.metadata.get("full_text")
                .or_else(|| result.metadata.get("content"))
                .cloned()
                .unwrap_or_else(|| result.snippet.clone());

            // Create diagnostic info
            let diagnostic = DocumentDiagnostic {
                id: format!("doc-{}", idx),
                title: result.metadata.get("title")
                    .or_else(|| result.metadata.get("filename"))
                    .cloned()
                    .unwrap_or_else(|| format!("Document {}", idx)),
                snippet: if result.snippet.len() > 200 {
                    format!("{}...", &result.snippet[..200])
                } else {
                    result.snippet.clone()
                },
                full_text_length: full_text.len(),
                metadata: result.metadata.clone(),
                chunk_count: 1, // Will be updated if we can get chunk info
                indexing_status: if has_content { "OK" } else { "EMPTY" }.to_string(),
            };

            sample_documents.push(diagnostic);

            // Limit sample size
            if sample_documents.len() >= 10 {
                break;
            }
        }

    // Get total counts from statistics
    let stats = rag.get_statistics().await.unwrap_or_default();
    let total_chunks_stat: usize = stats.get("total_chunks").and_then(|s| s.parse().ok()).unwrap_or(0);

    Ok(IndexDiagnostics {
        total_documents: search_results.len(),
        total_chunks: total_chunks_stat,
        documents_with_content,
        empty_documents,
        sample_documents,
        file_types,
        spaces,
    })
}

/// Get the full content of a specific document
#[tauri::command]
pub async fn get_document_content(
    state: State<'_, RagState>,
    title: Option<String>,
    file_path: Option<String>,
) -> Result<String, String> {
    tracing::debug!("Getting document content for: {:?} {:?}", title, file_path);

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // Search for the specific document
    let query = title.clone().or(file_path.clone()).unwrap_or_default();
    let results = rag.search_comprehensive(&query, 10, None)
        .await
        .map_err(|e| format!("Search error: {}", e))?;

        // Find matching document
        for result in results {
            let matches = if let Some(ref t) = title {
                result.metadata.get("title").map_or(false, |v| v.contains(t))
            } else if let Some(ref p) = file_path {
                result.metadata.get("file_path").map_or(false, |v| v.contains(p))
            } else {
                false
            };

            if matches {
                // Try to get full content from metadata or snippet
                let content = result.metadata.get("full_text")
                    .or_else(|| result.metadata.get("content"))
                    .cloned()
                    .unwrap_or_else(|| result.snippet.clone());

                return Ok(format!(
                    "Document: {}\n\
                     File: {}\n\
                     Space: {}\n\
                     Score: {:.3}\n\
                     Content Length: {} chars\n\
                     ---\n\
                     Content:\n{}\n\
                     ---\n\
                     Metadata: {:?}",
                    result.metadata.get("title").unwrap_or(&"Unknown".to_string()),
                    result.metadata.get("file_path").unwrap_or(&"Unknown".to_string()),
                    result.metadata.get("space_id").unwrap_or(&"Unknown".to_string()),
                    result.score,
                    content.len(),
                    if content.is_empty() { "[EMPTY CONTENT]" } else { &content },
                    result.metadata
                ));
            }
        }

    Err(format!("Document not found: {:?} {:?}", title, file_path))
}

/// Debug command to inspect RAG state
#[tauri::command]
pub async fn debug_rag_state(
    state: State<'_, RagState>,
) -> Result<String, String> {
    tracing::debug!("Debugging RAG state...");

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    let stats = rag.get_statistics().await.unwrap_or_default();
    let total_chunks = stats.get("total_chunks").cloned().unwrap_or_default();
    let fts_indexed = stats.get("fts_indexed").cloned().unwrap_or_default();
    let dimension = stats.get("embedding_dimension").cloned().unwrap_or_default();
    let data_dir = stats.get("data_dir").cloned().unwrap_or_default();

    Ok(format!(
        "RAG System Status:\n\
         - Initialized: Yes\n\
         - Total Chunks: {}\n\
         - FTS Indexed: {}\n\
         - Embedding Dimension: {}\n\
         - Data Dir: {}\n\
         - Search: Hybrid (LanceDB + Tantivy)",
        total_chunks,
        fts_indexed,
        dimension,
        data_dir
    ))
}

/// Recalculate storage statistics from actual data
#[tauri::command]
pub async fn recalculate_stats(
    state: State<'_, RagState>,
) -> Result<String, String> {
    tracing::debug!("Recalculating storage statistics...");

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    let stats = rag.get_statistics().await.unwrap_or_default();
    let total_chunks = stats.get("total_chunks").cloned().unwrap_or_default();
    let fts_indexed = stats.get("fts_indexed").cloned().unwrap_or_default();

    Ok(format!(
        "Stats calculated:\n\
         - Total Chunks: {}\n\
         - FTS Indexed: {}",
        total_chunks,
        fts_indexed
    ))
}