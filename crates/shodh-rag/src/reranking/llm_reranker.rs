//! LLM-based listwise reranker for merged search results.
//!
//! After multi-variant query expansion produces results from 2-3 different
//! search queries, `merge_expanded_results()` deduplicates them but the final
//! ordering uses scores that aren't comparable across runs. This module adds a
//! single LLM call that judges relevance to the original user question,
//! producing a globally-consistent ordering.
//!
//! Falls back to original merge order when the LLM is unavailable or produces
//! unparseable output.

use std::collections::HashSet;

use crate::llm::LLMManager;
use crate::types::SimpleSearchResult;

const MAX_RERANK_CANDIDATES: usize = 15;
const RERANK_SNIPPET_CHARS: usize = 300;
const RERANK_OUTPUT_TOKENS: usize = 256;

/// Rerank merged search results using a single listwise LLM call.
///
/// Sends numbered snippets to the LLM and asks it to return a JSON array of
/// indices ordered by relevance. Falls back to the original ordering on any
/// failure (LLM unavailable, generation error, unparseable output).
pub async fn llm_rerank(
    llm: &LLMManager,
    query: &str,
    results: Vec<SimpleSearchResult>,
) -> Vec<SimpleSearchResult> {
    if results.len() <= 1 {
        return results;
    }

    let candidate_count = results.len().min(MAX_RERANK_CANDIDATES);

    let snippets: String = results
        .iter()
        .take(candidate_count)
        .enumerate()
        .map(|(i, r)| {
            let truncated: String = r.text.chars().take(RERANK_SNIPPET_CHARS).collect();
            format!("[{}] {}", i + 1, truncated)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "You are a search relevance judge. Given a user query and numbered document snippets, \
         rank the snippets by relevance to the query.\n\n\
         Query: \"{}\"\n\n\
         Snippets:\n{}\n\n\
         Return ONLY a JSON array of snippet numbers ordered from most relevant to least relevant. \
         Include ALL {} snippet numbers. Example: [3, 1, 5, 2, 4]\n\
         Output ONLY the JSON array, nothing else.",
        query, snippets, candidate_count
    );

    // Cap reranking at 20 seconds — it's an optimization, not a requirement.
    // If the LLM is slow or unreachable, fall back to merge order immediately.
    let raw_output = match tokio::time::timeout(
        std::time::Duration::from_secs(20),
        llm.generate_custom(&prompt, RERANK_OUTPUT_TOKENS),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            tracing::warn!("LLM reranking call failed: {}, keeping merge order", e);
            return results;
        }
        Err(_) => {
            tracing::warn!("LLM reranking timed out after 20s, keeping merge order");
            return results;
        }
    };

    match parse_ranking(&raw_output, candidate_count) {
        Some(order) => {
            tracing::debug!(
                order = ?order,
                "LLM reranking parsed successfully"
            );
            apply_ranking(results, &order)
        }
        None => {
            tracing::warn!(
                output = %raw_output.chars().take(200).collect::<String>(),
                "Could not parse LLM reranking output, keeping merge order"
            );
            results
        }
    }
}

/// Parse the LLM output into a zero-indexed ranking vector.
///
/// Three-tier strategy:
/// 1. Direct JSON parse of the full output
/// 2. Find `[...]` substring and parse that
/// 3. Extract all integers from raw text, deduplicate
fn parse_ranking(output: &str, expected_count: usize) -> Option<Vec<usize>> {
    let trimmed = output
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    // Strategy 1: direct JSON parse
    if let Ok(indices) = serde_json::from_str::<Vec<usize>>(trimmed) {
        if validate_ranking(&indices, expected_count) {
            return Some(to_zero_indexed(indices));
        }
    }

    // Strategy 2: find the first JSON array in the output
    if let Some(start) = trimmed.find('[') {
        if let Some(end) = trimmed[start..].find(']') {
            let slice = &trimmed[start..=start + end];
            if let Ok(indices) = serde_json::from_str::<Vec<usize>>(slice) {
                if validate_ranking(&indices, expected_count) {
                    return Some(to_zero_indexed(indices));
                }
            }
        }
    }

    // Strategy 3: extract all integers
    let numbers: Vec<usize> = trimmed
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1 && n <= expected_count)
        .collect();

    // Accept if we got at least half the expected indices
    if numbers.len() >= (expected_count + 1) / 2 {
        let mut seen = HashSet::new();
        let deduped: Vec<usize> = numbers
            .into_iter()
            .filter(|n| seen.insert(*n))
            .map(|i| i.saturating_sub(1))
            .collect();
        if !deduped.is_empty() {
            return Some(deduped);
        }
    }

    None
}

/// Check that all indices are in [1, expected_count].
fn validate_ranking(indices: &[usize], expected_count: usize) -> bool {
    !indices.is_empty() && indices.iter().all(|&i| i >= 1 && i <= expected_count)
}

/// Convert 1-based indices to 0-based.
fn to_zero_indexed(indices: Vec<usize>) -> Vec<usize> {
    indices.into_iter().map(|i| i.saturating_sub(1)).collect()
}

/// Apply the ranking permutation to the results vector.
///
/// Indices in `order` that are out of bounds are skipped. Any results not
/// mentioned in `order` are appended at the end in their original relative
/// order (preserves the tail for results beyond MAX_RERANK_CANDIDATES).
fn apply_ranking(mut results: Vec<SimpleSearchResult>, order: &[usize]) -> Vec<SimpleSearchResult> {
    let mut reordered: Vec<SimpleSearchResult> = Vec::with_capacity(results.len());
    let mut used = HashSet::new();

    for &idx in order {
        if idx < results.len() && !used.contains(&idx) {
            used.insert(idx);
        }
    }

    // First pass: reordered items
    for &idx in order {
        if idx < results.len() {
            // Clone because we need the originals for the tail
            reordered.push(results[idx].clone());
        }
    }

    // Second pass: append any results not covered by the ranking
    for (i, result) in results.drain(..).enumerate() {
        if !used.contains(&i) {
            reordered.push(result);
        }
    }

    reordered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_clean_json() {
        let output = "[3, 1, 2]";
        let result = parse_ranking(output, 3).unwrap();
        assert_eq!(result, vec![2, 0, 1]); // zero-indexed
    }

    #[test]
    fn test_parse_json_with_fences() {
        let output = "```json\n[2, 1, 3]\n```";
        let result = parse_ranking(output, 3).unwrap();
        assert_eq!(result, vec![1, 0, 2]);
    }

    #[test]
    fn test_parse_json_with_surrounding_text() {
        let output = "Here is the ranking: [3, 1, 2] based on relevance.";
        let result = parse_ranking(output, 3).unwrap();
        assert_eq!(result, vec![2, 0, 1]);
    }

    #[test]
    fn test_parse_integer_extraction_fallback() {
        let output = "The order is: 3, then 1, then 2.";
        let result = parse_ranking(output, 3).unwrap();
        assert_eq!(result, vec![2, 0, 1]);
    }

    #[test]
    fn test_parse_garbage_returns_none() {
        let output = "I don't understand what you want.";
        assert!(parse_ranking(output, 5).is_none());
    }

    #[test]
    fn test_parse_partial_ranking() {
        // Only 3 out of 5 indices — still >= half
        let output = "[2, 4, 1]";
        let result = parse_ranking(output, 5).unwrap();
        assert_eq!(result, vec![1, 3, 0]);
    }

    #[test]
    fn test_apply_ranking_reorders() {
        let results = vec![
            make_result("a", 0.9),
            make_result("b", 0.8),
            make_result("c", 0.7),
        ];
        let order = vec![2, 0, 1]; // c, a, b
        let reordered = apply_ranking(results, &order);
        assert_eq!(reordered[0].title, "c");
        assert_eq!(reordered[1].title, "a");
        assert_eq!(reordered[2].title, "b");
    }

    #[test]
    fn test_apply_ranking_appends_unmentioned() {
        let results = vec![
            make_result("a", 0.9),
            make_result("b", 0.8),
            make_result("c", 0.7),
            make_result("d", 0.6),
        ];
        let order = vec![2, 0]; // only mentions c, a
        let reordered = apply_ranking(results, &order);
        assert_eq!(reordered.len(), 4);
        assert_eq!(reordered[0].title, "c");
        assert_eq!(reordered[1].title, "a");
        // b and d appended in original order
        assert_eq!(reordered[2].title, "b");
        assert_eq!(reordered[3].title, "d");
    }

    fn make_result(title: &str, score: f32) -> SimpleSearchResult {
        SimpleSearchResult {
            id: uuid::Uuid::new_v4(),
            score,
            text: format!("Content of {}", title),
            metadata: std::collections::HashMap::new(),
            title: title.to_string(),
            source: String::new(),
            heading: None,
            citation: None,
            doc_id: uuid::Uuid::nil(),
            chunk_id: 0,
        }
    }
}
