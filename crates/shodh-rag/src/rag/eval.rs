//! Retrieval Evaluation Module
//!
//! Measures retrieval quality using standard IR metrics:
//! - Recall@K: fraction of relevant documents retrieved in top K
//! - Precision@K: fraction of top K results that are relevant
//! - MRR (Mean Reciprocal Rank): average 1/rank of first relevant result
//! - nDCG@K (Normalized Discounted Cumulative Gain): position-weighted relevance
//! - Hit Rate@K: fraction of queries with at least one relevant result in top K
//!
//! Designed for offline evaluation with labeled query-document pairs.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A single evaluation query with its expected relevant document IDs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalQuery {
    /// The query text
    pub query: String,
    /// IDs of documents that are relevant to this query.
    /// For graded relevance, use `graded_relevance` instead.
    pub relevant_ids: HashSet<String>,
    /// Optional graded relevance: doc_id → relevance score (0.0 to 1.0).
    /// If empty, binary relevance from `relevant_ids` is used.
    #[serde(default)]
    pub graded_relevance: HashMap<String, f32>,
}

/// A single retrieved result for evaluation.
#[derive(Debug, Clone)]
pub struct EvalResult {
    pub id: String,
    pub score: f32,
}

/// Aggregated metrics across an evaluation set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalMetrics {
    /// Number of queries evaluated
    pub num_queries: usize,
    /// Mean Reciprocal Rank
    pub mrr: f64,
    /// Recall at various K values
    pub recall_at: HashMap<usize, f64>,
    /// Precision at various K values
    pub precision_at: HashMap<usize, f64>,
    /// nDCG at various K values
    pub ndcg_at: HashMap<usize, f64>,
    /// Hit rate at various K values (fraction of queries with >=1 relevant in top K)
    pub hit_rate_at: HashMap<usize, f64>,
    /// Per-query breakdown (query text → per-query metrics)
    pub per_query: Vec<QueryMetrics>,
}

/// Metrics for a single query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryMetrics {
    pub query: String,
    pub reciprocal_rank: f64,
    pub recall_at_k: HashMap<usize, f64>,
    pub precision_at_k: HashMap<usize, f64>,
    pub ndcg_at_k: HashMap<usize, f64>,
    pub num_relevant: usize,
    pub num_retrieved_relevant: usize,
}

/// Evaluate retrieval quality across a set of queries.
///
/// `k_values` specifies which K values to compute metrics at (e.g., [1, 3, 5, 10]).
/// `results_fn` is called for each query and should return ranked results.
pub fn evaluate<F>(eval_set: &[EvalQuery], k_values: &[usize], mut results_fn: F) -> EvalMetrics
where
    F: FnMut(&str) -> Vec<EvalResult>,
{
    let mut per_query = Vec::with_capacity(eval_set.len());
    let mut mrr_sum = 0.0;
    let mut recall_sums: HashMap<usize, f64> = k_values.iter().map(|&k| (k, 0.0)).collect();
    let mut precision_sums: HashMap<usize, f64> = k_values.iter().map(|&k| (k, 0.0)).collect();
    let mut ndcg_sums: HashMap<usize, f64> = k_values.iter().map(|&k| (k, 0.0)).collect();
    let mut hit_sums: HashMap<usize, f64> = k_values.iter().map(|&k| (k, 0.0)).collect();

    for eval_query in eval_set {
        let results = results_fn(&eval_query.query);
        let qm = evaluate_single(eval_query, &results, k_values);

        mrr_sum += qm.reciprocal_rank;
        for &k in k_values {
            if let Some(&v) = qm.recall_at_k.get(&k) {
                *recall_sums.get_mut(&k).unwrap() += v;
            }
            if let Some(&v) = qm.precision_at_k.get(&k) {
                *precision_sums.get_mut(&k).unwrap() += v;
            }
            if let Some(&v) = qm.ndcg_at_k.get(&k) {
                *ndcg_sums.get_mut(&k).unwrap() += v;
            }
            // Hit: did we get at least one relevant in top K?
            if qm.recall_at_k.get(&k).copied().unwrap_or(0.0) > 0.0 {
                *hit_sums.get_mut(&k).unwrap() += 1.0;
            }
        }

        per_query.push(qm);
    }

    let n = eval_set.len().max(1) as f64;

    EvalMetrics {
        num_queries: eval_set.len(),
        mrr: mrr_sum / n,
        recall_at: recall_sums.into_iter().map(|(k, v)| (k, v / n)).collect(),
        precision_at: precision_sums
            .into_iter()
            .map(|(k, v)| (k, v / n))
            .collect(),
        ndcg_at: ndcg_sums.into_iter().map(|(k, v)| (k, v / n)).collect(),
        hit_rate_at: hit_sums.into_iter().map(|(k, v)| (k, v / n)).collect(),
        per_query,
    }
}

/// Evaluate a single query against its results.
fn evaluate_single(
    eval_query: &EvalQuery,
    results: &[EvalResult],
    k_values: &[usize],
) -> QueryMetrics {
    let use_graded = !eval_query.graded_relevance.is_empty();
    let num_relevant = if use_graded {
        eval_query.graded_relevance.len()
    } else {
        eval_query.relevant_ids.len()
    };

    // Reciprocal rank: 1/position of first relevant result
    let reciprocal_rank = results
        .iter()
        .enumerate()
        .find(|(_, r)| is_relevant(r, eval_query))
        .map(|(idx, _)| 1.0 / (idx + 1) as f64)
        .unwrap_or(0.0);

    let mut recall_at_k = HashMap::new();
    let mut precision_at_k = HashMap::new();
    let mut ndcg_at_k = HashMap::new();
    let mut num_retrieved_relevant = 0;

    for &k in k_values {
        let top_k = &results[..results.len().min(k)];

        // Count relevant in top K
        let relevant_in_k = top_k.iter().filter(|r| is_relevant(r, eval_query)).count();

        // Recall@K
        let recall = if num_relevant > 0 {
            relevant_in_k as f64 / num_relevant as f64
        } else {
            0.0
        };
        recall_at_k.insert(k, recall);

        // Precision@K
        let precision = if k > 0 {
            relevant_in_k as f64 / top_k.len().max(1) as f64
        } else {
            0.0
        };
        precision_at_k.insert(k, precision);

        // nDCG@K
        let ndcg = compute_ndcg(top_k, eval_query, k);
        ndcg_at_k.insert(k, ndcg);

        if k == *k_values.iter().max().unwrap_or(&0) {
            num_retrieved_relevant = relevant_in_k;
        }
    }

    QueryMetrics {
        query: eval_query.query.clone(),
        reciprocal_rank,
        recall_at_k,
        precision_at_k,
        ndcg_at_k,
        num_relevant,
        num_retrieved_relevant,
    }
}

fn is_relevant(result: &EvalResult, eval_query: &EvalQuery) -> bool {
    if !eval_query.graded_relevance.is_empty() {
        eval_query
            .graded_relevance
            .get(&result.id)
            .copied()
            .unwrap_or(0.0)
            > 0.0
    } else {
        eval_query.relevant_ids.contains(&result.id)
    }
}

fn relevance_score(result: &EvalResult, eval_query: &EvalQuery) -> f64 {
    if !eval_query.graded_relevance.is_empty() {
        eval_query
            .graded_relevance
            .get(&result.id)
            .copied()
            .unwrap_or(0.0) as f64
    } else if eval_query.relevant_ids.contains(&result.id) {
        1.0
    } else {
        0.0
    }
}

/// Compute nDCG@K (Normalized Discounted Cumulative Gain).
fn compute_ndcg(results: &[EvalResult], eval_query: &EvalQuery, k: usize) -> f64 {
    let top_k = &results[..results.len().min(k)];

    // DCG: sum of relevance / log2(position + 1)
    let dcg: f64 = top_k
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let rel = relevance_score(r, eval_query);
            rel / (i as f64 + 2.0).log2() // log2(i+2) since i is 0-based
        })
        .sum();

    // Ideal DCG: sort all relevant docs by relevance descending
    let mut ideal_scores: Vec<f64> = if !eval_query.graded_relevance.is_empty() {
        eval_query
            .graded_relevance
            .values()
            .map(|&v| v as f64)
            .collect()
    } else {
        vec![1.0; eval_query.relevant_ids.len()]
    };
    ideal_scores.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    ideal_scores.truncate(k);

    let idcg: f64 = ideal_scores
        .iter()
        .enumerate()
        .map(|(i, &rel)| rel / (i as f64 + 2.0).log2())
        .sum();

    if idcg > 0.0 {
        dcg / idcg
    } else {
        0.0
    }
}

/// Format evaluation metrics as a human-readable report.
pub fn format_report(metrics: &EvalMetrics) -> String {
    let mut report = String::new();

    report.push_str(&format!(
        "=== Retrieval Evaluation Report ({} queries) ===\n\n",
        metrics.num_queries
    ));

    report.push_str(&format!("MRR: {:.4}\n\n", metrics.mrr));

    // Sort K values for consistent output
    let mut k_values: Vec<usize> = metrics.recall_at.keys().copied().collect();
    k_values.sort();

    report.push_str("| K  | Recall | Precision | nDCG   | Hit Rate |\n");
    report.push_str("|----|--------|-----------|--------|----------|\n");
    for &k in &k_values {
        let recall = metrics.recall_at.get(&k).copied().unwrap_or(0.0);
        let precision = metrics.precision_at.get(&k).copied().unwrap_or(0.0);
        let ndcg = metrics.ndcg_at.get(&k).copied().unwrap_or(0.0);
        let hit_rate = metrics.hit_rate_at.get(&k).copied().unwrap_or(0.0);
        report.push_str(&format!(
            "| {:2} | {:.4} | {:.4}    | {:.4} | {:.4}   |\n",
            k, recall, precision, ndcg, hit_rate
        ));
    }

    // Per-query breakdown for failed queries (MRR = 0)
    let failed: Vec<&QueryMetrics> = metrics
        .per_query
        .iter()
        .filter(|q| q.reciprocal_rank == 0.0)
        .collect();

    if !failed.is_empty() {
        report.push_str(&format!(
            "\n--- Failed queries ({}/{}) ---\n",
            failed.len(),
            metrics.num_queries
        ));
        for q in &failed {
            report.push_str(&format!(
                "  - \"{}\" (expected {} relevant docs)\n",
                q.query, q.num_relevant
            ));
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_results(ids: &[&str]) -> Vec<EvalResult> {
        ids.iter()
            .enumerate()
            .map(|(i, &id)| EvalResult {
                id: id.to_string(),
                score: 1.0 - i as f32 * 0.1,
            })
            .collect()
    }

    #[test]
    fn test_perfect_retrieval() {
        let eval_set = vec![EvalQuery {
            query: "test query".to_string(),
            relevant_ids: HashSet::from(["a".to_string(), "b".to_string()]),
            graded_relevance: HashMap::new(),
        }];

        let metrics = evaluate(&eval_set, &[1, 3, 5], |_| {
            make_results(&["a", "b", "c", "d", "e"])
        });

        assert_eq!(metrics.mrr, 1.0); // First result is relevant
        assert_eq!(*metrics.recall_at.get(&1).unwrap(), 0.5); // 1 of 2 in top 1
        assert_eq!(*metrics.recall_at.get(&3).unwrap(), 1.0); // 2 of 2 in top 3
        assert_eq!(*metrics.precision_at.get(&1).unwrap(), 1.0); // 1/1 relevant
        assert!(*metrics.hit_rate_at.get(&1).unwrap() > 0.99);
    }

    #[test]
    fn test_no_relevant_found() {
        let eval_set = vec![EvalQuery {
            query: "missing query".to_string(),
            relevant_ids: HashSet::from(["x".to_string()]),
            graded_relevance: HashMap::new(),
        }];

        let metrics = evaluate(&eval_set, &[1, 5], |_| {
            make_results(&["a", "b", "c", "d", "e"])
        });

        assert_eq!(metrics.mrr, 0.0);
        assert_eq!(*metrics.recall_at.get(&5).unwrap(), 0.0);
        assert_eq!(*metrics.hit_rate_at.get(&5).unwrap(), 0.0);
    }

    #[test]
    fn test_mrr_first_relevant_at_position_3() {
        let eval_set = vec![EvalQuery {
            query: "test".to_string(),
            relevant_ids: HashSet::from(["c".to_string()]),
            graded_relevance: HashMap::new(),
        }];

        let metrics = evaluate(&eval_set, &[5], |_| {
            make_results(&["a", "b", "c", "d", "e"])
        });

        // c is at position 3 (0-indexed: 2), so MRR = 1/3
        assert!((metrics.mrr - 1.0 / 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_graded_relevance_ndcg() {
        let mut graded = HashMap::new();
        graded.insert("a".to_string(), 1.0);
        graded.insert("b".to_string(), 0.5);
        graded.insert("c".to_string(), 0.25);

        let eval_set = vec![EvalQuery {
            query: "graded test".to_string(),
            relevant_ids: HashSet::new(),
            graded_relevance: graded,
        }];

        // Perfect ordering: a(1.0), b(0.5), c(0.25)
        let metrics = evaluate(&eval_set, &[3], |_| make_results(&["a", "b", "c"]));

        // With ideal ordering, nDCG should be 1.0
        assert!((*metrics.ndcg_at.get(&3).unwrap() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_multiple_queries_averaged() {
        let eval_set = vec![
            EvalQuery {
                query: "q1".to_string(),
                relevant_ids: HashSet::from(["a".to_string()]),
                graded_relevance: HashMap::new(),
            },
            EvalQuery {
                query: "q2".to_string(),
                relevant_ids: HashSet::from(["x".to_string()]),
                graded_relevance: HashMap::new(),
            },
        ];

        let metrics = evaluate(&eval_set, &[3], |query| {
            if query == "q1" {
                make_results(&["a", "b", "c"]) // hit
            } else {
                make_results(&["a", "b", "c"]) // miss (x not in results)
            }
        });

        // MRR: (1.0 + 0.0) / 2 = 0.5
        assert!((metrics.mrr - 0.5).abs() < 1e-10);
        // Hit rate: 1/2 = 0.5
        assert!((*metrics.hit_rate_at.get(&3).unwrap() - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_format_report_no_panic() {
        let eval_set = vec![EvalQuery {
            query: "test".to_string(),
            relevant_ids: HashSet::from(["a".to_string()]),
            graded_relevance: HashMap::new(),
        }];

        let metrics = evaluate(&eval_set, &[1, 3, 5, 10], |_| {
            make_results(&["b", "a", "c"])
        });

        let report = format_report(&metrics);
        assert!(report.contains("MRR"));
        assert!(report.contains("Recall"));
    }
}
