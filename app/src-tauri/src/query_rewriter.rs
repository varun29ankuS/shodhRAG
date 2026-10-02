//! Query Rewriting Module - Tauri Command Wrapper
//!
//! Thin wrapper around core library's QueryRewriter for Tauri desktop app.
//! Handles conversation context extraction and LLM integration.

use crate::llm_commands::LLMState;
use crate::rag_commands::RagState;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use shodh_rag::rag::{ConversationContext, QueryRewriter};
use tauri::State;

// Re-export core library types with camelCase for frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewrittenQuery {
    pub original_query: String,
    pub rewritten_query: String,
    pub explanation: String,
    pub used_context: bool,
    pub should_retrieve: bool,
    pub retrieval_reason: String,
}

impl RewrittenQuery {
    fn from_rewrite(original: &str, rewritten: &str) -> Self {
        Self {
            original_query: original.to_string(),
            rewritten_query: rewritten.to_string(),
            explanation: "Query processed for search optimization".to_string(),
            used_context: false,
            should_retrieve: true,
            retrieval_reason: "Document search".to_string(),
        }
    }
}

/// Rewrites a query using conversation context from ConversationManager
pub async fn rewrite_query_with_context(
    query: &str,
    rag_state: &RagState,
    llm_state: &LLMState,
) -> Result<RewrittenQuery> {
    // Get conversation manager
    let conversation_manager = rag_state.conversation_manager.read().await;

    if conversation_manager.is_none() {
        // No conversation context - but still check if retrieval is needed
        let rewriter = QueryRewriter::new();
        let (should_retrieve, retrieval_reason) = rewriter.should_retrieve_documents(query);

        return Ok(RewrittenQuery {
            original_query: query.to_string(),
            rewritten_query: query.to_string(),
            explanation: "No conversation context available".to_string(),
            used_context: false,
            should_retrieve,
            retrieval_reason,
        });
    }

    let manager = conversation_manager.as_ref().unwrap();

    // Get last conversation
    let last_conversation = manager.get_last_conversation().await?;

    if last_conversation.is_none() {
        // No active conversation - but still check if retrieval is needed
        let rewriter = QueryRewriter::new();
        let (should_retrieve, retrieval_reason) = rewriter.should_retrieve_documents(query);

        return Ok(RewrittenQuery {
            original_query: query.to_string(),
            rewritten_query: query.to_string(),
            explanation: "No active conversation".to_string(),
            used_context: false,
            should_retrieve,
            retrieval_reason,
        });
    }

    let conversation = last_conversation.unwrap();

    // Build ConversationContext from conversation
    let mut context = ConversationContext {
        topic: conversation.topic.clone(),
        recent_messages: conversation
            .messages
            .iter()
            .rev()
            .take(3)
            .map(|m| format!("{:?}: {}", m.role, m.content))
            .collect(),
        concepts_mentioned: conversation.context.concepts_mentioned.clone(),
        files_discussed: conversation.context.files_discussed.clone(),
        entities: Vec::new(), // Can be extended later
    };

    // Use core library QueryRewriter
    let rewriter = QueryRewriter::new().with_debug(true);

    // Get LLM manager
    let llm_manager = llm_state.manager.read().await;

    if let Some(ref manager) = *llm_manager {
        // Clone manager for async closure
        let manager_clone = manager.clone();

        // Call core library with LLM callback
        let result = rewriter
            .rewrite_with_context(query, &context, move |prompt, max_tokens| {
                let manager = manager_clone.clone();
                async move { manager.generate_custom(&prompt, max_tokens).await }
            })
            .await?;

        Ok(RewrittenQuery {
            original_query: result.original_query,
            rewritten_query: result.rewritten_query,
            explanation: result.explanation,
            used_context: result.used_context,
            should_retrieve: result.should_retrieve,
            retrieval_reason: result.retrieval_reason,
        })
    } else {
        // LLM not available - use rule-based fallback
        tracing::info!("[QueryRewriter] LLM not available, using rule-based fallback");
        let result = rewriter.rewrite_rule_based(query, &context);
        Ok(RewrittenQuery {
            original_query: result.original_query,
            rewritten_query: result.rewritten_query,
            explanation: result.explanation,
            used_context: result.used_context,
            should_retrieve: result.should_retrieve,
            retrieval_reason: result.retrieval_reason,
        })
    }
}

/// Tauri command for query rewriting with search
#[tauri::command]
pub async fn search_with_query_rewriting(
    query: String,
    space_id: Option<String>,
    max_results: usize,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<SearchWithRewritingResult, String> {
    tracing::info!("\n=== Search with Query Rewriting ===");
    tracing::info!("Original query: {}", query);

    // Step 1: Rewrite query using core library
    let rewritten = rewrite_query_with_context(&query, &rag_state, &llm_state)
        .await
        .map_err(|e| format!("Query rewriting failed: {}", e))?;

    tracing::info!("Rewritten query: {}", rewritten.rewritten_query);
    tracing::info!("Used context: {}", rewritten.used_context);
    tracing::info!("Should retrieve: {}", rewritten.should_retrieve);
    tracing::info!("Retrieval reason: {}", rewritten.retrieval_reason);

    // Step 2: Skip search if retrieval not needed
    if !rewritten.should_retrieve {
        tracing::info!("Skipping retrieval - {}", rewritten.retrieval_reason);
        return Ok(SearchWithRewritingResult {
            query_rewriting: rewritten,
            results: vec![],
            total_results: 0,
        });
    }

    // Step 3: Analyze query to determine search strategy (web vs local)
    use shodh_rag::rag::retrieval_decision::{CorpusStats, QueryAnalyzer, RetrievalStrategy};

    let search_query = if rewritten.used_context {
        &rewritten.rewritten_query
    } else {
        &query
    };

    let analyzer = QueryAnalyzer::new();
    let corpus_stats = CorpusStats::default();
    let analysis = analyzer.analyze(search_query, &corpus_stats);

    tracing::info!("🧠 Query Strategy Analysis:");
    tracing::info!("  Intent: {:?}", analysis.intent);
    tracing::info!("  Strategy: {:?}", analysis.decision.strategy);
    tracing::info!("  Reasoning: {}", analysis.decision.reasoning);

    let rag_guard = rag_state.rag.read().await;

    // Execute search based on strategy
    let mut filtered_results = {
        // All strategies use local comprehensive search in the new API
        tracing::info!("📚 Executing local document search...");
        let filter = None;

        let results = rag_guard
            .search_comprehensive(search_query, max_results, filter)
            .await
            .map_err(|e| format!("Search failed: {}", e))?;

        // Filter by space_id if provided
        if let Some(ref sid) = space_id {
            results
                .into_iter()
                .filter(|r| {
                    r.metadata
                        .get("space_id")
                        .map(|s| s == sid)
                        .unwrap_or(false)
                })
                .collect()
        } else {
            results
        }
    };

    tracing::info!("Found {} results before re-ranking", filtered_results.len());

    // Re-rank results to boost documents with query terms in actual content vs metadata
    boost_content_over_metadata(&mut filtered_results, search_query);

    tracing::info!(
        "Found {} results after content boosting",
        filtered_results.len()
    );

    // Convert to frontend format
    let search_results: Vec<crate::rag_commands::SearchResult> = filtered_results
        .into_iter()
        .map(|r| {
            let source_file = r
                .metadata
                .get("file_path")
                .or_else(|| r.metadata.get("source"))
                .cloned()
                .unwrap_or_else(|| r.citation.source.clone());

            let page_number = r
                .metadata
                .get("page_number")
                .or_else(|| r.metadata.get("page"))
                .and_then(|p| p.parse::<u32>().ok());

            let line_range = r
                .metadata
                .get("line_start")
                .and_then(|start| start.parse::<u32>().ok())
                .and_then(|start| {
                    r.metadata
                        .get("line_end")
                        .and_then(|end| end.parse::<u32>().ok())
                        .map(|end| (start, end))
                });

            let full_text = r
                .metadata
                .get("full_text")
                .or_else(|| r.metadata.get("content"))
                .cloned()
                .unwrap_or_else(|| r.snippet.clone());

            let snippet_pos = full_text.find(&r.snippet).unwrap_or(0);
            let context_start = snippet_pos.saturating_sub(200);
            let context_end = (snippet_pos + r.snippet.len() + 200).min(full_text.len());
            let surrounding_context = full_text[context_start..context_end].to_string();

            crate::rag_commands::SearchResult {
                id: r.id.to_string(),
                score: r.score,
                snippet: r.snippet,
                citation: r.citation,
                metadata: r.metadata,
                source_file,
                page_number,
                line_range,
                surrounding_context,
            }
        })
        .collect();

    let total = search_results.len();

    Ok(SearchWithRewritingResult {
        query_rewriting: rewritten,
        results: search_results,
        total_results: total,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchWithRewritingResult {
    pub query_rewriting: RewrittenQuery,
    pub results: Vec<crate::rag_commands::SearchResult>,
    pub total_results: usize,
}

/// Re-rank search results to boost documents with query terms in actual content
/// over documents that only have query terms in metadata (file paths, etc.)
fn boost_content_over_metadata(
    results: &mut Vec<shodh_rag::comprehensive_system::ComprehensiveResult>,
    query: &str,
) {
    tracing::info!("\n=== Content Boosting ===");
    tracing::info!("Query: {}", query);

    // Extract query terms (simple tokenization)
    let query_terms: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .filter(|term| term.len() > 2) // Skip short words like "me", "of"
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Calculate boost scores for each result
    let mut scored_results: Vec<(f32, shodh_rag::comprehensive_system::ComprehensiveResult)> =
        Vec::new();

    for (idx, result) in results.drain(..).enumerate() {
        let mut boost_score = result.score; // Start with original score

        // Check if query terms appear in actual content (snippet)
        let snippet_lower = result.snippet.to_lowercase();
        let content_matches: usize = query_terms
            .iter()
            .filter(|term| snippet_lower.contains(term.as_str()))
            .count();

        // Check if query terms appear only in file_path metadata
        let file_path = result
            .metadata
            .get("file_path")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        let path_only_matches: usize = query_terms
            .iter()
            .filter(|term| {
                !snippet_lower.contains(term.as_str()) && file_path.contains(term.as_str())
            })
            .count();

        // Boost calculation:
        // - Content matches: +0.5 per term (strong boost)
        // - Path-only matches: +0.05 per term (weak boost)
        let content_boost = content_matches as f32 * 0.5;
        let path_boost = path_only_matches as f32 * 0.05;

        boost_score += content_boost + path_boost;

        // Debug logging for first few results or results with content matches
        if idx < 3 || content_matches > 0 {
            let file_name = result
                .metadata
                .get("file_name")
                .map(|s| s.as_str())
                .or_else(|| {
                    result
                        .metadata
                        .get("file_path")
                        .and_then(|p| std::path::Path::new(p).file_name().and_then(|n| n.to_str()))
                })
                .unwrap_or("unknown");

            tracing::info!("  File: {}", file_name);
            tracing::info!("    Original score: {:.3}", result.score);
            tracing::info!(
                "    Content matches: {} (boost: +{:.3})",
                content_matches,
                content_boost
            );
            tracing::info!(
                "    Path-only matches: {} (boost: +{:.3})",
                path_only_matches,
                path_boost
            );
            tracing::info!("    Final score: {:.3}", boost_score);
        }

        scored_results.push((boost_score, result));
    }

    // Sort by boosted score (descending)
    scored_results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // Extract results back
    *results = scored_results
        .into_iter()
        .map(|(_, result)| result)
        .collect();

    tracing::info!("=== Content Boosting Complete ===\n");
}
