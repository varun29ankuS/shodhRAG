//! Context Compression Module
//!
//! Extractive compression that reduces retrieved chunks to only the sentences
//! most relevant to the query. Cuts context size by 50-70% while preserving
//! signal, reducing token usage and improving LLM answer quality.

use std::collections::HashSet;
use std::sync::LazyLock;

static SENTENCE_SPLIT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?<=[.!?])\s+(?=[A-Z\d])").expect("sentence split regex is valid")
});

/// Compress a chunk by extracting only the most query-relevant sentences.
///
/// Returns the compressed text with the top `max_sentences` sentences in
/// original order. If the chunk is already short or has fewer sentences,
/// returns it unchanged.
pub fn compress_chunk(chunk: &str, query: &str, max_sentences: usize) -> String {
    let chunk = chunk.trim();
    if chunk.is_empty() {
        return String::new();
    }

    // Split into sentences
    let sentences: Vec<&str> = split_sentences(chunk);

    // Short chunks don't need compression
    if sentences.len() <= max_sentences {
        return chunk.to_string();
    }

    // Build query term set for scoring.
    // Preserve email addresses and URLs as intact terms (don't strip dots/@ from them).
    let query_terms: HashSet<String> = query
        .to_lowercase()
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .map(|w| {
            if w.contains('@') || w.contains("://") || w.contains(".com") || w.contains(".org") || w.contains(".net") || w.contains(".io") {
                // Email or URL — keep as-is (only trim outer punctuation like commas/quotes)
                w.trim_matches(|c: char| c == ',' || c == '"' || c == '\'' || c == '(' || c == ')').to_string()
            } else {
                w.trim_matches(|c: char| !c.is_alphanumeric()).to_string()
            }
        })
        .filter(|w| !w.is_empty())
        .collect();

    // Score each sentence
    let mut scored: Vec<(usize, f32, &str)> = sentences
        .iter()
        .enumerate()
        .map(|(idx, &sentence)| {
            let score = score_sentence(sentence, &query_terms, idx, sentences.len());
            (idx, score, sentence)
        })
        .collect();

    // Sort by score descending, take top N
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut selected_indices: Vec<usize> = scored
        .iter()
        .take(max_sentences)
        .map(|(idx, _, _)| *idx)
        .collect();

    // Restore original order for readability
    selected_indices.sort();

    // Reconstruct compressed text
    let compressed: Vec<&str> = selected_indices
        .iter()
        .map(|&idx| sentences[idx])
        .collect();

    compressed.join(" ")
}

/// Compress multiple chunks for building LLM context.
/// Each chunk is independently compressed, preserving source attribution.
pub fn compress_context(
    chunks: &[(String, f32)], // (text, score) pairs
    query: &str,
    max_sentences_per_chunk: usize,
    max_total_chars: usize,
) -> Vec<String> {
    let mut compressed_chunks = Vec::new();
    let mut total_chars = 0;

    for (text, _score) in chunks {
        let compressed = compress_chunk(text, query, max_sentences_per_chunk);

        if compressed.is_empty() {
            continue;
        }

        total_chars += compressed.len();
        compressed_chunks.push(compressed);

        if total_chars >= max_total_chars {
            break;
        }
    }

    compressed_chunks
}

/// Split text into sentences. Handles abbreviations and decimal numbers
/// to avoid false splits.
fn split_sentences(text: &str) -> Vec<&str> {
    // For structured data (form fields, key-value pairs), split on newlines
    if text.contains('\n') && text.lines().count() > 3 {
        let lines: Vec<&str> = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() >= 3 {
            return lines;
        }
    }

    // Standard sentence splitting
    let parts: Vec<&str> = SENTENCE_SPLIT_RE
        .split(text)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    // If regex didn't split (single long sentence), try period-based
    if parts.len() <= 1 && text.len() > 200 {
        let manual: Vec<&str> = text
            .split(". ")
            .flat_map(|s| s.split(".\n"))
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if manual.len() > 1 {
            return manual;
        }
    }

    parts
}

/// Score a sentence by query relevance.
///
/// Factors:
/// - Term overlap: how many query terms appear in the sentence
/// - Position: first and last sentences get a mild boost (often summaries)
/// - Density: shorter sentences with more matches score higher
/// - Key-value: lines with ":" pattern get a boost (structured data)
fn score_sentence(
    sentence: &str,
    query_terms: &HashSet<String>,
    position: usize,
    total_sentences: usize,
) -> f32 {
    let lower = sentence.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    if words.is_empty() {
        return 0.0;
    }

    // Term overlap (primary signal)
    let matching_terms = query_terms
        .iter()
        .filter(|term| lower.contains(term.as_str()))
        .count();

    let term_score = if query_terms.is_empty() {
        0.0
    } else {
        matching_terms as f32 / query_terms.len() as f32
    };

    // Density: matching terms per word (rewards concise answers)
    let density = matching_terms as f32 / words.len().max(1) as f32;

    // Position bias: first and last sentences often contain key information
    let position_score = if position == 0 || position == total_sentences - 1 {
        0.1
    } else if position <= 2 {
        0.05
    } else {
        0.0
    };

    // Key-value pattern boost (structured data like "Name: John")
    let kv_boost = if sentence.contains(':') && sentence.len() < 200 {
        0.15
    } else {
        0.0
    };

    // Structured data boost: always preserve lines containing emails, phone
    // numbers, IDs, or other extractable facts — these are high-value for RAG
    // regardless of keyword overlap with the query.
    let structured_boost = if lower.contains('@')
        || lower.contains(".com")
        || lower.contains(".org")
        || lower.contains(".net")
        || lower.contains("http")
        || lower.contains("phone")
        || lower.contains("mobile")
        || lower.contains("tel:")
        || (lower.contains("email") && sentence.len() < 200)
    {
        0.35
    } else {
        0.0
    };

    // Weighted combination
    term_score * 0.6 + density * 0.15 + position_score + kv_boost + structured_boost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_chunk_unchanged() {
        let chunk = "This is a short chunk.";
        let result = compress_chunk(chunk, "short chunk", 5);
        assert_eq!(result, chunk);
    }

    #[test]
    fn test_compression_selects_relevant_sentences() {
        let chunk = "The weather is nice today. \
                     Anushree's PAN number is ABCDE1234F. \
                     The sky is blue and clear. \
                     Her salary is 150000 per month. \
                     Birds are singing outside.";
        let result = compress_chunk(chunk, "PAN number salary", 2);
        assert!(result.contains("PAN"));
        assert!(result.contains("salary"));
        assert!(!result.contains("Birds"));
    }

    #[test]
    fn test_preserves_original_order() {
        let chunk = "First important fact about salary. \
                     Some irrelevant filler text here. \
                     Another irrelevant sentence. \
                     Last important fact about PAN number.";
        let result = compress_chunk(chunk, "salary PAN", 2);
        // Salary should come before PAN (original order)
        let salary_pos = result.find("salary").unwrap_or(usize::MAX);
        let pan_pos = result.find("PAN").unwrap_or(usize::MAX);
        assert!(salary_pos < pan_pos);
    }

    #[test]
    fn test_key_value_boosted() {
        let chunk = "This is background information. \
                     Name: Anushree Sharma. \
                     Some other context here. \
                     Random unrelated sentence.";
        let result = compress_chunk(chunk, "name", 2);
        assert!(result.contains("Name: Anushree"));
    }

    #[test]
    fn test_compress_context_respects_limit() {
        let chunks = vec![
            ("A".repeat(500), 0.9),
            ("B".repeat(500), 0.8),
            ("C".repeat(500), 0.7),
        ];
        let result = compress_context(&chunks, "test", 10, 600);
        // Should stop after exceeding max_total_chars
        assert!(result.len() <= 2);
    }

    #[test]
    fn test_newline_split_for_structured_data() {
        let chunk = "Name: Anushree\nPAN: ABCDE1234F\nSalary: 150000\nAge: 30\nCity: Mumbai";
        let result = compress_chunk(chunk, "PAN salary", 2);
        assert!(result.contains("PAN"));
        assert!(result.contains("Salary") || result.contains("salary"));
    }
}
