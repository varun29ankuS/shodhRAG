use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridSource {
    Vector,
    TextSearch,
    Both,
}

/// Min-max scaling of one result list's scores to [0, 1]; a list whose scores are
/// all equal scales to 0.5, and a score that is not finite to 0.
fn min_max_scale(results: &[(String, f32)]) -> impl Fn(f32) -> f32 {
    let (min, max) = results
        .iter()
        .map(|(_, s)| *s)
        .filter(|s| s.is_finite())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), s| {
            (lo.min(s), hi.max(s))
        });
    let range = max - min;
    move |score| {
        if !score.is_finite() {
            0.0
        } else if range < 1e-9 {
            0.5
        } else {
            (score - min) / range
        }
    }
}

/// Score-aware RRF — standard RRF weighted by normalized original similarity scores.
/// Unlike plain RRF which discards quality signals, this modulates rank-based scores
/// by the original similarity/BM25 scores so high-confidence matches get a boost.
/// `score_weight` controls the blend: 0.0 = pure RRF, higher = more score influence.
/// Equal fused scores keep the order they were first seen in: vector rank, then
/// keyword rank.
pub fn score_aware_rrf(
    vector_results: Vec<(String, f32)>,
    fts_results: Vec<(String, f32)>,
    k: usize,
    top_k: usize,
    score_weight: f32,
) -> Vec<(String, f32, HybridSource)> {
    let mut merged: Vec<(String, f32, HybridSource)> =
        Vec::with_capacity(vector_results.len() + fts_results.len());
    let mut position: HashMap<String, usize> = HashMap::with_capacity(merged.capacity());

    for (results, list) in [
        (vector_results, HybridSource::Vector),
        (fts_results, HybridSource::TextSearch),
    ] {
        let scale = min_max_scale(&results);
        for (rank, (id, score)) in results.into_iter().enumerate() {
            let rrf = 1.0 / (k as f32 + rank as f32 + 1.0);
            let combined = rrf * (1.0 + score_weight * scale(score));
            match position.get(&id) {
                Some(&at) => {
                    merged[at].1 += combined;
                    merged[at].2 = HybridSource::Both;
                }
                None => {
                    position.insert(id.clone(), merged.len());
                    merged.push((id, combined, list));
                }
            }
        }
    }

    // Stable: the top of each list scales to 1 at rank 0, so ties are routine and
    // decide which candidates survive the cut to `top_k`.
    merged.sort_by(|a, b| b.1.total_cmp(&a.1));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(list: &[(&str, f32)]) -> Vec<(String, f32)> {
        list.iter().map(|(id, s)| (id.to_string(), *s)).collect()
    }

    #[test]
    fn equal_scores_keep_vector_then_keyword_order() {
        let vector = hits(&[("v1", 0.9), ("v2", 0.5)]);
        let keyword = hits(&[("k1", 12.0), ("k2", 3.0)]);
        for _ in 0..20 {
            let fused = score_aware_rrf(vector.clone(), keyword.clone(), 60, 3, 0.3);
            let ids: Vec<&str> = fused.iter().map(|(id, _, _)| id.as_str()).collect();
            assert_eq!(ids, ["v1", "k1", "v2"]);
            assert_eq!(fused[0].1, 1.0);
            assert_eq!(fused[0].1, fused[1].1);
        }
    }

    #[test]
    fn a_candidate_in_both_lists_sums_its_scores() {
        let fused = score_aware_rrf(
            hits(&[("a", 0.9), ("b", 0.1)]),
            hits(&[("b", 8.0), ("c", 1.0)]),
            60,
            10,
            0.3,
        );
        let ids: Vec<&str> = fused.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "c"]);
        assert_eq!(fused[0].2, HybridSource::Both);
        assert_eq!(fused[1].2, HybridSource::Vector);
        assert_eq!(fused[2].2, HybridSource::TextSearch);
        let expected_b = 1.0 / 62.0 + 1.3 / 61.0;
        let expected_a = 1.3 / 61.0;
        assert!((fused[1].1 - expected_a / expected_b).abs() < 1e-6);
    }

    #[test]
    fn a_score_that_is_not_a_number_does_not_take_the_top() {
        let fused = score_aware_rrf(
            hits(&[("nan", f32::NAN), ("a", 0.8), ("b", 0.2)]),
            Vec::new(),
            60,
            10,
            0.3,
        );
        assert!(fused.iter().all(|(_, s, _)| s.is_finite()));
        assert_eq!(fused[0].0, "a");
    }
}
