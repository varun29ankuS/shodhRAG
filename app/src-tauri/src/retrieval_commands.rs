//! Thin Tauri wrapper for retrieval decision commands.
//! Business logic (corpus stats, query analysis) lives in shodh_rag.

use serde::{Deserialize, Serialize};
use tauri::State;
use crate::rag_commands::RagState;
use crate::context_commands::ContextState;
use shodh_rag::rag::{QueryAnalyzer, QueryAnalysis};
use shodh_rag::chat::build_corpus_stats;
use std::collections::HashMap;

// Frontend-friendly types (camelCase serialization)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryAnalysisResult {
    pub intent: String,
    pub relevance: RelevanceScoreResult,
    pub requirements: QueryRequirementsResult,
    pub decision: RetrievalDecisionResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelevanceScoreResult {
    pub corpus_coverage: f32,
    pub domain_match: f32,
    pub term_frequency: f32,
    pub overall_confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequirementsResult {
    pub needs_filtering: bool,
    pub date_range: Option<String>,
    pub numeric_conditions: Vec<String>,
    pub entity_references: Vec<String>,
    pub document_type_hints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalDecisionResult {
    pub should_retrieve: bool,
    pub strategy: String,
    pub estimated_docs_needed: usize,
    pub confidence: f32,
    pub reasoning: String,
    pub fallback_plan: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusStatsResult {
    pub total_docs: usize,
    pub vocabulary_size: usize,
    pub document_types: HashMap<String, usize>,
    pub avg_doc_length: usize,
}

#[tauri::command]
pub async fn analyze_query(
    query: String,
    space_id: Option<String>,
    state: State<'_, RagState>,
    _context_state: State<'_, ContextState>,
) -> Result<QueryAnalysisResult, String> {
    let rag_guard = state.rag.read().await;
    let corpus_stats = build_corpus_stats(&rag_guard, space_id.as_deref())
        .await
        .map_err(|e| format!("Failed to build corpus stats: {}", e))?;

    let analyzer = QueryAnalyzer::new();
    let analysis = analyzer.analyze(&query, &corpus_stats);

    Ok(convert_analysis(analysis))
}

#[tauri::command]
pub async fn get_corpus_stats(
    space_id: Option<String>,
    state: State<'_, RagState>,
) -> Result<CorpusStatsResult, String> {
    let rag_guard = state.rag.read().await;
    let stats = build_corpus_stats(&rag_guard, space_id.as_deref())
        .await
        .map_err(|e| format!("Failed to build corpus stats: {}", e))?;

    Ok(CorpusStatsResult {
        total_docs: stats.total_docs,
        vocabulary_size: stats.vocabulary.len(),
        document_types: stats.document_types.clone(),
        avg_doc_length: stats.avg_doc_length,
    })
}

fn convert_analysis(analysis: QueryAnalysis) -> QueryAnalysisResult {
    QueryAnalysisResult {
        intent: format!("{:?}", analysis.intent),
        relevance: RelevanceScoreResult {
            corpus_coverage: analysis.relevance.corpus_coverage,
            domain_match: analysis.relevance.domain_match,
            term_frequency: analysis.relevance.term_frequency,
            overall_confidence: analysis.relevance.overall_confidence,
        },
        requirements: QueryRequirementsResult {
            needs_filtering: analysis.requirements.needs_filtering,
            date_range: analysis.requirements.date_range.map(|r| {
                format!(
                    "{} to {}",
                    r.start.unwrap_or_else(|| "?".to_string()),
                    r.end.unwrap_or_else(|| "?".to_string())
                )
            }),
            numeric_conditions: analysis
                .requirements
                .numeric_conditions
                .iter()
                .map(|c| format!("{} {:?} {}", c.field, c.operator, c.value))
                .collect(),
            entity_references: analysis.requirements.entity_references,
            document_type_hints: analysis.requirements.document_type_hints,
        },
        decision: RetrievalDecisionResult {
            should_retrieve: analysis.decision.should_retrieve,
            strategy: format!("{:?}", analysis.decision.strategy),
            estimated_docs_needed: analysis.decision.estimated_docs_needed,
            confidence: analysis.decision.confidence,
            reasoning: analysis.decision.reasoning,
            fallback_plan: analysis.decision.fallback_plan,
        },
    }
}
