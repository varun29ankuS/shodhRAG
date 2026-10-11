//! A third ranked list for document search: library files ranked by the citation graph.
//!
//! When a query names papers, methods, datasets or authors the graph knows, or asks about
//! citations ("what does everyone build on", "papers in my library that cite DeltaNet"),
//! a [`SourceRanker`] returns library files in graph order (personalized PageRank over the
//! citation graph, see `research::citations::graph`). For any other query it returns
//! `None` and search is unchanged.
//!
//! The list is fused twice, both times with reciprocal rank fusion:
//! 1. **with the vector and full-text lists** ([`boost_scores`]): each candidate chunk of a
//!    ranked file gains `1 / (k + rank)`, as a third RRF list would give it. This decides
//!    which candidates pass the score threshold, so a chunk the graph points at (a
//!    bibliography entry naming DeltaNet) is not dropped for matching only one list;
//! 2. **after the cross-encoder** ([`fuse_ranks`]), which replaces every score and would
//!    otherwise erase the graph's vote: the final order is the RRF of the reranked order
//!    and the graph order. The scores the reranker gave are kept as a set and reassigned
//!    in the new order, so their scale (thresholds, displayed relevance) is unchanged.

use std::collections::HashMap;

use crate::types::ComprehensiveResult;

/// Ranks library files for a query, best first, or `None` when the query is not about
/// anything in the citation graph.
pub trait SourceRanker: Send + Sync {
    /// File paths in graph order.
    fn rank_sources(&self, query: &str) -> Option<Vec<String>>;
}

/// Ranks (0-based) of files by their index spelling.
pub fn rank_map(files: &[String]) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for (rank, file) in files.iter().enumerate() {
        let key = crate::rag_engine::normalize_source_path(std::path::Path::new(file));
        out.entry(key).or_insert(rank);
    }
    out
}

fn source_of(result: &ComprehensiveResult) -> Option<&str> {
    result
        .metadata
        .get("source_file")
        .or_else(|| result.metadata.get("file_path"))
        .map(String::as_str)
}

/// Adds the graph list's RRF term to the score of each result from a ranked file and
/// re-sorts. Returns how many results were boosted.
pub fn boost_scores(
    results: &mut [ComprehensiveResult],
    ranks: &HashMap<String, usize>,
    k: usize,
) -> usize {
    let mut boosted = 0;
    for r in results.iter_mut() {
        let Some(rank) = source_of(r).and_then(|s| ranks.get(s)) else {
            continue;
        };
        r.score += 1.0 / (k as f32 + *rank as f32 + 1.0);
        r.metadata
            .insert("graph_rank".to_string(), (rank + 1).to_string());
        boosted += 1;
    }
    results.sort_by(|a, b| b.score.total_cmp(&a.score));
    boosted
}

/// Reorders results (already sorted by score) by the RRF of their current order and the
/// graph order, keeping the multiset of scores: the best fused result gets the highest
/// score, and so on. Results of unranked files take part with their current rank only.
pub fn fuse_ranks(
    results: &mut Vec<ComprehensiveResult>,
    ranks: &HashMap<String, usize>,
    k: usize,
) {
    if results.len() < 2 || ranks.is_empty() {
        return;
    }
    let mut scores: Vec<f32> = results.iter().map(|r| r.score).collect();
    scores.sort_by(|a, b| b.total_cmp(a));
    let mut fused: Vec<(f32, usize, ComprehensiveResult)> = std::mem::take(results)
        .into_iter()
        .enumerate()
        .map(|(position, r)| {
            let mut f = 1.0 / (k as f32 + position as f32 + 1.0);
            if let Some(rank) = source_of(&r).and_then(|s| ranks.get(s)) {
                f += 1.0 / (k as f32 + *rank as f32 + 1.0);
            }
            (f, position, r)
        })
        .collect();
    fused.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    *results = fused
        .into_iter()
        .zip(scores)
        .map(|((_, _, mut r), score)| {
            r.score = score;
            r
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(source: &str, score: f32, n: u8) -> ComprehensiveResult {
        let mut metadata = HashMap::new();
        metadata.insert("source_file".to_string(), source.to_string());
        ComprehensiveResult {
            id: uuid::Uuid::from_bytes([n; 16]),
            score,
            metadata,
            citation: Default::default(),
            snippet: String::new(),
            source_index: "hybrid".to_string(),
        }
    }

    fn order(results: &[ComprehensiveResult]) -> Vec<u8> {
        results.iter().map(|r| r.id.as_bytes()[0]).collect()
    }

    #[test]
    fn the_graph_list_lifts_a_candidate_over_the_threshold_like_a_third_rrf_list() {
        let ranks = rank_map(&["c:/p/cites-deltanet.pdf".to_string()]);
        let key = crate::rag_engine::normalize_source_path(std::path::Path::new(
            "c:/p/cites-deltanet.pdf",
        ));
        let mut results = vec![result("c:/p/other.pdf", 0.030, 1), result(&key, 0.016, 2)];
        assert_eq!(boost_scores(&mut results, &ranks, 60), 1);
        assert_eq!(order(&results), [2, 1]);
        assert!(results[0].score > 0.02, "above the default threshold");
        assert_eq!(
            results[0].metadata.get("graph_rank").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn after_reranking_the_order_is_fused_and_the_score_scale_kept() {
        let a = crate::rag_engine::normalize_source_path(std::path::Path::new("c:/p/a.pdf"));
        let ranks = rank_map(&["c:/p/a.pdf".to_string()]);
        // Reranker order: two unrelated chunks above the graph's file.
        let mut results = vec![
            result("c:/p/x.pdf", 0.91, 1),
            result("c:/p/y.pdf", 0.90, 2),
            result(&a, 0.89, 3),
        ];
        fuse_ranks(&mut results, &ranks, 60);
        assert_eq!(order(&results), [3, 1, 2]);
        let scores: Vec<f32> = results.iter().map(|r| r.score).collect();
        assert_eq!(scores, [0.91, 0.90, 0.89]);
        // Without a graph list nothing moves.
        let mut same = vec![result("c:/p/x.pdf", 0.5, 1), result("c:/p/y.pdf", 0.4, 2)];
        fuse_ranks(&mut same, &HashMap::new(), 60);
        assert_eq!(order(&same), [1, 2]);
    }
}
