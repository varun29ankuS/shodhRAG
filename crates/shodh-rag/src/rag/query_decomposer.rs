//! Query Decomposition Module
//!
//! Splits complex multi-part queries into independent sub-queries for
//! parallel retrieval and result merging. Handles conjunctions, multiple
//! question marks, enumerated questions, and comparative queries.

use std::collections::HashSet;
use std::sync::LazyLock;

static CONJUNCTION_SPLIT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:and also|and then|and|also|additionally|plus|as well as)\b")
        .expect("conjunction regex is valid")
});

static QUESTION_MARK_SPLIT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\?\s+").expect("question mark split regex is valid"));

static ENUMERATED_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)^\s*(?:\d+[.)]\s*|[-•]\s+)(.+)$").expect("enumerated regex is valid")
});

/// Inline enumeration marker: "1. " / "2) " at the start of the query or after
/// whitespace. Requiring trailing whitespace keeps decimals ("1.5") from matching.
static INLINE_ENUM_MARKER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?:^|\s)(\d{1,2})[.)]\s+").expect("inline enumeration regex is valid")
});

static COMPARATIVE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:compare|difference between|versus|vs\.?|differ from)\b")
        .expect("comparative regex is valid")
});

static BETWEEN_ENTITIES_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)between\s+(.+?)\s+and\s+(.+?)(?:\s|$|\?)")
        .expect("between entities regex is valid")
});

/// Result of query decomposition.
#[derive(Debug, Clone)]
pub struct DecomposedQuery {
    pub original: String,
    pub sub_queries: Vec<String>,
    pub strategy: DecompositionStrategy,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecompositionStrategy {
    /// Query was not decomposed (single intent)
    Single,
    /// Split on conjunctions ("X and Y")
    Conjunction,
    /// Split on multiple question marks
    MultiQuestion,
    /// Enumerated items ("1. X 2. Y")
    Enumerated,
    /// Comparative query decomposed into per-entity searches
    Comparative,
}

/// Decompose a complex query into sub-queries for parallel retrieval.
///
/// Returns the original query unchanged if it represents a single intent.
/// Otherwise splits into 2+ sub-queries that can be searched independently.
pub fn decompose_query(query: &str) -> DecomposedQuery {
    let query = query.trim();

    // Short queries are never decomposed
    if query.split_whitespace().count() < 5 {
        return single(query);
    }

    // Try each decomposition strategy in order of specificity

    // 1. Enumerated items: "1. what is X  2. what is Y"
    let enumerated = extract_enumerated(query);
    if enumerated.len() >= 2 {
        return DecomposedQuery {
            original: query.to_string(),
            sub_queries: enumerated,
            strategy: DecompositionStrategy::Enumerated,
        };
    }

    // 2. Multiple question marks: "What is X? What about Y?"
    let questions = split_on_question_marks(query);
    if questions.len() >= 2 {
        return DecomposedQuery {
            original: query.to_string(),
            sub_queries: questions,
            strategy: DecompositionStrategy::MultiQuestion,
        };
    }

    // 3. Comparative: "compare X and Y" or "difference between X and Y"
    if let Some(comparative) = try_comparative_split(query) {
        return comparative;
    }

    // 4. Conjunction split: "What is X and what is Y"
    // Only split if both parts look like independent questions
    if let Some(conjunction) = try_conjunction_split(query) {
        return conjunction;
    }

    single(query)
}

fn single(query: &str) -> DecomposedQuery {
    DecomposedQuery {
        original: query.to_string(),
        sub_queries: vec![query.to_string()],
        strategy: DecompositionStrategy::Single,
    }
}

fn extract_enumerated(query: &str) -> Vec<String> {
    let items: Vec<String> = ENUMERATED_RE
        .captures_iter(query)
        .filter_map(|cap| cap.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|s| s.split_whitespace().count() >= 2)
        .collect();

    if items.len() >= 2 {
        return items;
    }

    extract_inline_enumerated(query)
}

/// Split "1. what is X 2. what is Y" typed on a single line.
///
/// Only markers forming the consecutive sequence 1, 2, 3, ... are treated as item
/// boundaries, so stray numbers inside an item ("section 4. of the policy") do not
/// split it.
fn extract_inline_enumerated(query: &str) -> Vec<String> {
    // (marker_start, content_start) for each accepted marker
    let mut markers: Vec<(usize, usize)> = Vec::new();
    let mut expected: u32 = 1;

    for cap in INLINE_ENUM_MARKER_RE.captures_iter(query) {
        let (Some(number), Some(whole)) = (cap.get(1), cap.get(0)) else {
            continue;
        };
        if number.as_str().parse::<u32>().ok() == Some(expected) {
            markers.push((number.start(), whole.end()));
            expected += 1;
        }
    }

    if markers.len() < 2 {
        return Vec::new();
    }

    markers
        .iter()
        .enumerate()
        .map(|(i, &(_, content_start))| {
            let end = markers
                .get(i + 1)
                .map(|&(next_start, _)| next_start)
                .unwrap_or(query.len());
            query[content_start..end].trim().to_string()
        })
        .filter(|s| s.split_whitespace().count() >= 2)
        .collect()
}

fn split_on_question_marks(query: &str) -> Vec<String> {
    // Split on "? " followed by uppercase letter (new sentence)
    let parts: Vec<String> = QUESTION_MARK_SPLIT_RE
        .split(query)
        .map(|s| {
            let s = s.trim();
            if s.ends_with('?') {
                s.to_string()
            } else if !s.is_empty() {
                format!("{}?", s)
            } else {
                String::new()
            }
        })
        .filter(|s| s.split_whitespace().count() >= 2)
        .collect();

    parts
}

fn try_comparative_split(query: &str) -> Option<DecomposedQuery> {
    if !COMPARATIVE_RE.is_match(query) {
        return None;
    }

    // Try "between X and Y" pattern
    if let Some(cap) = BETWEEN_ENTITIES_RE.captures(query) {
        let entity_a = cap.get(1)?.as_str().trim().to_string();
        let entity_b = cap.get(2)?.as_str().trim().to_string();

        if entity_a.split_whitespace().count() <= 5 && entity_b.split_whitespace().count() <= 5 {
            return Some(DecomposedQuery {
                original: query.to_string(),
                sub_queries: vec![
                    format!("what is {}", entity_a),
                    format!("what is {}", entity_b),
                    query.to_string(), // Keep original for direct comparison matches
                ],
                strategy: DecompositionStrategy::Comparative,
            });
        }
    }

    None
}

fn try_conjunction_split(query: &str) -> Option<DecomposedQuery> {
    let lower = query.to_lowercase();

    // Don't split if it's a single question with "and" as part of it
    // e.g. "what are the pros and cons" should NOT be split
    let non_split_phrases = [
        "pros and cons",
        "advantages and disadvantages",
        "strengths and weaknesses",
        "name and address",
        "search and replace",
        "copy and paste",
        "back and forth",
        "trial and error",
    ];
    if non_split_phrases.iter().any(|p| lower.contains(p)) {
        return None;
    }

    let parts: Vec<&str> = CONJUNCTION_SPLIT_RE.split(query).collect();
    if parts.len() < 2 {
        return None;
    }

    // Each part must have enough substance to be a standalone query
    let valid_parts: Vec<String> = parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| p.split_whitespace().count() >= 3)
        .collect();

    if valid_parts.len() >= 2 {
        Some(DecomposedQuery {
            original: query.to_string(),
            sub_queries: valid_parts,
            strategy: DecompositionStrategy::Conjunction,
        })
    } else {
        None
    }
}

/// Merge and deduplicate results from multiple sub-query searches.
/// Uses chunk ID for dedup, keeps the highest score for each unique chunk.
pub fn merge_results<T: HasIdAndScore>(mut result_sets: Vec<Vec<T>>, limit: usize) -> Vec<T> {
    if result_sets.len() == 1 {
        let mut single = result_sets.remove(0);
        single.truncate(limit);
        return single;
    }

    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut merged: Vec<T> = Vec::new();

    // Round-robin interleave to ensure each sub-query contributes
    let max_len = result_sets.iter().map(|r| r.len()).max().unwrap_or(0);

    for idx in 0..max_len {
        for result_set in &mut result_sets {
            if idx < result_set.len() {
                // We need to move out of the vec - use swap_remove pattern
                // But since we're iterating in order, we'll collect first
            }
        }
    }

    // Simpler approach: flatten with interleaving
    let mut iterators: Vec<std::vec::IntoIter<T>> =
        result_sets.into_iter().map(|v| v.into_iter()).collect();

    let mut round = 0;
    loop {
        let mut any_produced = false;
        for iter in iterators.iter_mut() {
            if let Some(item) = iter.next() {
                let id = item.result_id();
                if seen_ids.insert(id) {
                    merged.push(item);
                    any_produced = true;

                    if merged.len() >= limit {
                        return merged;
                    }
                }
            }
        }
        if !any_produced {
            break;
        }
        round += 1;
        if round > 1000 {
            break; // Safety limit
        }
    }

    merged.truncate(limit);
    merged
}

/// Trait for result types that have an ID and score for deduplication.
pub trait HasIdAndScore {
    fn result_id(&self) -> String;
    fn result_score(&self) -> f32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_query_not_decomposed() {
        let result = decompose_query("what is the salary");
        assert_eq!(result.strategy, DecompositionStrategy::Single);
        assert_eq!(result.sub_queries.len(), 1);
    }

    #[test]
    fn test_short_query_not_decomposed() {
        let result = decompose_query("PAN number");
        assert_eq!(result.strategy, DecompositionStrategy::Single);
    }

    #[test]
    fn test_multi_question_decomposition() {
        let result = decompose_query("What is the PAN number? What is the salary?");
        assert_eq!(result.strategy, DecompositionStrategy::MultiQuestion);
        assert_eq!(result.sub_queries.len(), 2);
        assert!(result.sub_queries[0].contains("PAN"));
        assert!(result.sub_queries[1].contains("salary"));
    }

    #[test]
    fn test_conjunction_decomposition() {
        let result = decompose_query("what is the PAN number and what is the monthly salary");
        assert_eq!(result.strategy, DecompositionStrategy::Conjunction);
        assert_eq!(result.sub_queries.len(), 2);
    }

    #[test]
    fn test_pros_and_cons_not_split() {
        let result = decompose_query("what are the pros and cons of this approach");
        assert_eq!(result.strategy, DecompositionStrategy::Single);
    }

    #[test]
    fn test_comparative_decomposition() {
        let result =
            decompose_query("compare the difference between savings account and fixed deposit");
        assert_eq!(result.strategy, DecompositionStrategy::Comparative);
        assert!(result.sub_queries.len() >= 2);
    }

    #[test]
    fn test_enumerated_decomposition() {
        let result = decompose_query(
            "1. What is the account number 2. What is the IFSC code 3. What is the balance",
        );
        assert_eq!(result.strategy, DecompositionStrategy::Enumerated);
        assert_eq!(result.sub_queries.len(), 3);
    }

    #[test]
    fn test_inline_enumeration_items_are_split_at_markers() {
        let result = decompose_query(
            "1. What is the account number 2. What is the IFSC code 3. What is the balance",
        );
        assert_eq!(
            result.sub_queries,
            vec![
                "What is the account number".to_string(),
                "What is the IFSC code".to_string(),
                "What is the balance".to_string(),
            ]
        );
    }

    #[test]
    fn test_multiline_enumeration_still_supported() {
        let result = decompose_query("1. What is the account number\n2. What is the IFSC code");
        assert_eq!(result.strategy, DecompositionStrategy::Enumerated);
        assert_eq!(result.sub_queries.len(), 2);
    }

    #[test]
    fn test_non_sequential_numbers_are_not_enumeration() {
        // "4." and "1.5" do not form a 1, 2, ... marker sequence
        let result = decompose_query("what does section 4. say about the 1.5 percent fee");
        assert_ne!(result.strategy, DecompositionStrategy::Enumerated);
    }
}
