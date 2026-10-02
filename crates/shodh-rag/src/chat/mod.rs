//! Shared pieces of the former chat pipeline that other modules still use:
//! the progress-event sink and corpus statistics for retrieval.
//!
//! Answers are produced by agent sessions (`crate::harness`).

/// Event sink for streaming tokens and progress events.
/// Tauri provides an implementation wrapping AppHandle.emit().
/// HTTP servers can provide SSE-based implementations.
pub trait EventEmitter: Send + Sync {
    fn emit(&self, event: &str, data: serde_json::Value);
}

/// No-op emitter for non-streaming contexts.
pub struct NoopEmitter;
impl EventEmitter for NoopEmitter {
    fn emit(&self, _event: &str, _data: serde_json::Value) {}
}

/// Build corpus statistics from the RAG engine for a given space.
pub async fn build_corpus_stats(
    rag: &crate::rag_engine::RAGEngine,
    space_id: Option<&str>,
) -> anyhow::Result<crate::rag::CorpusStats> {
    use crate::types::MetadataFilter;
    use std::collections::{HashMap, HashSet};

    let filter = space_id.map(|sid| MetadataFilter {
        space_id: Some(sid.to_string()),
        source_type: None,
        source_path: None,
        date_from: None,
        date_to: None,
        custom: None,
    });

    let chunks = match rag.list_documents(filter, 100_000).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("build_corpus_stats failed: {}", e);
            return Ok(crate::rag::CorpusStats {
                total_docs: 0,
                vocabulary: HashSet::new(),
                document_types: HashMap::new(),
                domain_terms: HashMap::new(),
                avg_doc_length: 0,
            });
        }
    };

    let mut seen_docs = HashSet::new();
    let mut document_types: HashMap<String, usize> = HashMap::new();
    let mut vocabulary = HashSet::new();
    let mut total_length: usize = 0;

    for chunk in &chunks {
        let doc_id = chunk.metadata.get("doc_id").cloned().unwrap_or_default();
        let is_new_doc = seen_docs.insert(doc_id);

        if is_new_doc {
            let ext = chunk
                .metadata
                .get("file_extension")
                .or_else(|| chunk.metadata.get("file_type"))
                .cloned()
                .unwrap_or_else(|| "unknown".to_string())
                .to_lowercase();
            *document_types.entry(ext).or_insert(0) += 1;
        }

        // Exclude space_metadata documents
        if chunk
            .metadata
            .get("doc_type")
            .map(|t| t == "space_metadata")
            .unwrap_or(false)
        {
            continue;
        }

        for word in chunk.snippet.split_whitespace() {
            let clean = word
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            if clean.len() > 2 {
                vocabulary.insert(clean);
            }
        }
        total_length += chunk.snippet.len();
    }

    let avg_doc_length = if chunks.is_empty() {
        0
    } else {
        total_length / chunks.len()
    };

    // Build domain term frequencies in a single pass over chunks.
    // Count how many chunks contain each term using per-chunk word sets.
    let total_docs_f = chunks.len().max(1) as f32;
    let mut term_doc_count: HashMap<String, usize> = HashMap::new();
    for chunk in &chunks {
        if chunk
            .metadata
            .get("doc_type")
            .map(|t| t == "space_metadata")
            .unwrap_or(false)
        {
            continue;
        }
        let chunk_words: HashSet<String> = chunk
            .snippet
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|w| w.len() > 2)
            .collect();
        for word in &chunk_words {
            *term_doc_count.entry(word.clone()).or_insert(0) += 1;
        }
    }
    let domain_terms: HashMap<String, f32> = term_doc_count
        .into_iter()
        .map(|(term, count)| (term, count as f32 / total_docs_f))
        .collect();

    tracing::debug!(
        "build_corpus_stats: {} chunks, {} unique docs, vocab={}, space_id={:?}",
        chunks.len(),
        seen_docs.len(),
        vocabulary.len(),
        space_id
    );

    Ok(crate::rag::CorpusStats {
        total_docs: seen_docs.len(),
        vocabulary,
        document_types,
        domain_terms,
        avg_doc_length,
    })
}
