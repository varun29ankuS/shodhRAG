use std::collections::HashMap;

use crate::storage::SearchHit;

/// Result from hybrid search combining vector and FTS results
#[derive(Debug, Clone)]
pub struct HybridResult {
    pub id: String,
    pub score: f32,
    pub source: HybridSource,
    pub hit: Option<SearchHit>,
}

#[derive(Debug, Clone, Copy)]
pub enum HybridSource {
    Vector,
    TextSearch,
    Both,
}

/// Reciprocal Rank Fusion — merges ranked lists without score normalization.
/// Formula: rrf_score(doc) = Σ 1/(k + rank_i) for each list containing doc
pub fn reciprocal_rank_fusion(
    vector_results: Vec<(String, f32)>,
    fts_results: Vec<(String, f32)>,
    k: usize,
    top_k: usize,
) -> Vec<(String, f32, HybridSource)> {
    let mut scores: HashMap<String, (f32, HybridSource)> = HashMap::new();

    for (rank, (id, _score)) in vector_results.iter().enumerate() {
        let rrf = 1.0 / (k as f32 + rank as f32 + 1.0);
        scores
            .entry(id.clone())
            .and_modify(|(s, src)| {
                *s += rrf;
                *src = HybridSource::Both;
            })
            .or_insert((rrf, HybridSource::Vector));
    }

    for (rank, (id, _score)) in fts_results.iter().enumerate() {
        let rrf = 1.0 / (k as f32 + rank as f32 + 1.0);
        scores
            .entry(id.clone())
            .and_modify(|(s, src)| {
                *s += rrf;
                *src = HybridSource::Both;
            })
            .or_insert((rrf, HybridSource::TextSearch));
    }

    let mut merged: Vec<(String, f32, HybridSource)> = scores
        .into_iter()
        .map(|(id, (score, source))| (id, score, source))
        .collect();

    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged.truncate(top_k);
    merged
}

/// Score-aware RRF — standard RRF weighted by normalized original similarity scores.
/// Unlike plain RRF which discards quality signals, this modulates rank-based scores
/// by the original similarity/BM25 scores so high-confidence matches get a boost.
/// `score_weight` controls the blend: 0.0 = pure RRF, higher = more score influence.
pub fn score_aware_rrf(
    vector_results: Vec<(String, f32)>,
    fts_results: Vec<(String, f32)>,
    k: usize,
    top_k: usize,
    score_weight: f32,
) -> Vec<(String, f32, HybridSource)> {
    let normalize = |results: &[(String, f32)]| -> HashMap<String, f32> {
        if results.is_empty() {
            return HashMap::new();
        }
        let max = results.iter().map(|(_, s)| *s).fold(f32::MIN, f32::max);
        let min = results.iter().map(|(_, s)| *s).fold(f32::MAX, f32::min);
        if (max - min).abs() < 1e-9 {
            // All scores identical — assign uniform normalized score
            return results.iter().map(|(id, _)| (id.clone(), 0.5)).collect();
        }
        let range = max - min;
        results
            .iter()
            .map(|(id, s)| (id.clone(), (s - min) / range))
            .collect()
    };

    let vec_norm = normalize(&vector_results);
    let fts_norm = normalize(&fts_results);

    let mut scores: HashMap<String, (f32, HybridSource)> = HashMap::new();

    for (rank, (id, _)) in vector_results.iter().enumerate() {
        let rrf = 1.0 / (k as f32 + rank as f32 + 1.0);
        let orig_score = vec_norm.get(id).copied().unwrap_or(0.0);
        let combined = rrf * (1.0 + score_weight * orig_score);
        scores
            .entry(id.clone())
            .and_modify(|(s, src)| {
                *s += combined;
                *src = HybridSource::Both;
            })
            .or_insert((combined, HybridSource::Vector));
    }

    for (rank, (id, _)) in fts_results.iter().enumerate() {
        let rrf = 1.0 / (k as f32 + rank as f32 + 1.0);
        let orig_score = fts_norm.get(id).copied().unwrap_or(0.0);
        let combined = rrf * (1.0 + score_weight * orig_score);
        scores
            .entry(id.clone())
            .and_modify(|(s, src)| {
                *s += combined;
                *src = HybridSource::Both;
            })
            .or_insert((combined, HybridSource::TextSearch));
    }

    let mut merged: Vec<(String, f32, HybridSource)> = scores
        .into_iter()
        .map(|(id, (score, source))| (id, score, source))
        .collect();

    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged.truncate(top_k);

    // Normalize scores to [0, 1] so downstream thresholds work correctly.
    // Raw RRF scores are in the 0.01-0.05 range which makes threshold filtering meaningless.
    if let Some(max_score) = merged.first().map(|(_, s, _)| *s) {
        if max_score > 0.0 {
            for item in &mut merged {
                item.1 /= max_score;
            }
        }
    }

    merged
}

/// Weighted combination — alpha-blends normalized vector and FTS scores
pub fn weighted_fusion(
    vector_results: Vec<(String, f32)>,
    fts_results: Vec<(String, f32)>,
    alpha: f32,
    top_k: usize,
) -> Vec<(String, f32, HybridSource)> {
    let normalize = |results: &[(String, f32)]| -> Vec<(String, f32)> {
        if results.is_empty() {
            return vec![];
        }
        let max = results.iter().map(|(_, s)| *s).fold(f32::MIN, f32::max);
        let min = results.iter().map(|(_, s)| *s).fold(f32::MAX, f32::min);
        let range = (max - min).max(1e-6);
        results
            .iter()
            .map(|(id, s)| (id.clone(), (s - min) / range))
            .collect()
    };

    let norm_vec = normalize(&vector_results);
    let norm_fts = normalize(&fts_results);

    let mut scores: HashMap<String, (f32, HybridSource)> = HashMap::new();

    for (id, score) in &norm_vec {
        scores.insert(id.clone(), (alpha * score, HybridSource::Vector));
    }

    for (id, score) in &norm_fts {
        scores
            .entry(id.clone())
            .and_modify(|(s, src)| {
                *s += (1.0 - alpha) * score;
                *src = HybridSource::Both;
            })
            .or_insert(((1.0 - alpha) * score, HybridSource::TextSearch));
    }

    let mut merged: Vec<(String, f32, HybridSource)> = scores
        .into_iter()
        .map(|(id, (score, source))| (id, score, source))
        .collect();

    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged.truncate(top_k);
    merged
}
