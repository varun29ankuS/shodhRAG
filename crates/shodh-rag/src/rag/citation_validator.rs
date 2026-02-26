//! Citation Validation Module
//!
//! Validates citations in LLM-generated answers to ensure they reference real files and line numbers.
//! Prevents hallucinated citations and improves trust in code search results.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static CITATION_WITH_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([a-zA-Z0-9_\-./\\]+\.[a-zA-Z]{1,5}):(\d+)").expect("citation with line regex is valid")
});

static CITATION_WITHOUT_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([a-zA-Z0-9_\-./\\]+\.[a-zA-Z]{1,5})\b").expect("citation without line regex is valid")
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub confidence: f32,
    pub total_citations: usize,
    pub valid_citations: usize,
    pub invalid_citations: Vec<InvalidCitation>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidCitation {
    pub citation_text: String,
    pub reason: String,
    pub file_path: Option<String>,
    pub line_number: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ExtractedCitation {
    pub full_text: String,
    pub file_path: String,
    pub line_number: Option<usize>,
    pub start_pos: usize,
    pub end_pos: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDocument {
    pub file_path: String,
    pub line_ranges: Vec<(u32, u32)>,
    pub content: String,
}

/// Citation validator for LLM answers
pub struct CitationValidator {
    /// Enable debug logging
    pub debug: bool,
}

impl CitationValidator {
    pub fn new() -> Self {
        Self { debug: false }
    }

    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    /// Extract citations from answer text
    ///
    /// Formats supported:
    /// - "filename.ext:line"
    /// - "path/to/filename.ext:line"
    /// - "filename.ext" (without line number)
    pub fn extract_citations(&self, answer: &str) -> Vec<ExtractedCitation> {
        let mut citations = Vec::new();

        // First, extract citations with line numbers
        for cap in CITATION_WITH_LINE_RE.captures_iter(answer) {
            if let (Some(file_match), Some(line_match)) = (cap.get(1), cap.get(2)) {
                let file_path = file_match.as_str().to_string();
                let line_number = line_match.as_str().parse::<usize>().ok();

                citations.push(ExtractedCitation {
                    full_text: cap.get(0).unwrap().as_str().to_string(),
                    file_path,
                    line_number,
                    start_pos: cap.get(0).unwrap().start(),
                    end_pos: cap.get(0).unwrap().end(),
                });
            }
        }

        // Then extract citations without line numbers (but skip if already captured)
        let mut seen_files = HashSet::new();
        for citation in &citations {
            seen_files.insert(citation.file_path.clone());
        }

        for cap in CITATION_WITHOUT_LINE_RE.captures_iter(answer) {
            if let Some(file_match) = cap.get(1) {
                let file_path = file_match.as_str().to_string();

                // Skip if we already have this file with a line number
                if seen_files.contains(&file_path) {
                    continue;
                }

                // Skip common false positives (version numbers, URLs, etc.)
                if self.is_false_positive(&file_path) {
                    continue;
                }

                citations.push(ExtractedCitation {
                    full_text: file_path.clone(),
                    file_path,
                    line_number: None,
                    start_pos: cap.get(0).unwrap().start(),
                    end_pos: cap.get(0).unwrap().end(),
                });

                seen_files.insert(file_match.as_str().to_string());
            }
        }

        if self.debug {
            tracing::debug!(count = citations.len(), "[CitationValidator] Extracted citations");
            for cit in &citations {
                tracing::debug!(citation = %cit.full_text, "  Citation found");
            }
        }

        citations
    }

    /// Check if a file path is likely a false positive
    fn is_false_positive(&self, path: &str) -> bool {
        // Version numbers (e.g., "1.2.3")
        if path.chars().all(|c| c.is_numeric() || c == '.') {
            return true;
        }

        // Common false positives
        let false_positives = [
            "e.g.", "i.e.", "etc.", "a.k.a.", "vs.", "localhost", "example.com", "test.txt",
        ];

        for fp in &false_positives {
            if path.to_lowercase().contains(fp) {
                return true;
            }
        }

        false
    }

    /// Validate citations against source documents
    pub fn validate(
        &self,
        answer: &str,
        source_documents: &[SourceDocument],
    ) -> ValidationResult {
        let citations = self.extract_citations(answer);

        if citations.is_empty() {
            return ValidationResult {
                confidence: 1.0, // No citations means nothing to validate
                total_citations: 0,
                valid_citations: 0,
                invalid_citations: Vec::new(),
                warnings: vec!["Answer contains no file citations".to_string()],
            };
        }

        // Build index of available files
        let mut available_files: HashSet<String> = HashSet::new();
        let mut file_line_ranges: HashMap<String, Vec<(u32, u32)>> = HashMap::new();

        for doc in source_documents {
            // Normalize path separators
            let normalized = doc.file_path.replace("\\", "/");
            available_files.insert(normalized.clone());

            // Track line ranges
            if !doc.line_ranges.is_empty() {
                file_line_ranges
                    .entry(normalized)
                    .or_insert_with(Vec::new)
                    .extend(doc.line_ranges.iter().cloned());
            }
        }

        // Validate each citation
        let mut valid_count = 0;
        let mut invalid_citations = Vec::new();
        let mut warnings = Vec::new();

        for citation in &citations {
            let normalized_path = citation.file_path.replace("\\", "/");

            // Check if file exists in source documents
            let file_exists = available_files.iter().any(|f| {
                f.ends_with(&normalized_path) || normalized_path.ends_with(f)
            });

            if !file_exists {
                invalid_citations.push(InvalidCitation {
                    citation_text: citation.full_text.clone(),
                    reason: format!("File '{}' not found in source documents", citation.file_path),
                    file_path: Some(citation.file_path.clone()),
                    line_number: citation.line_number,
                });
                warnings.push(format!(
                    "Citation '{}' references file not in retrieved documents",
                    citation.full_text
                ));
                continue;
            }

            // If line number is specified, validate it
            if let Some(line_num) = citation.line_number {
                let mut line_found = false;

                for (file, line_ranges) in &file_line_ranges {
                    if file.ends_with(&normalized_path) || normalized_path.ends_with(file) {
                        for (start, end) in line_ranges {
                            if line_num >= *start as usize && line_num <= *end as usize {
                                line_found = true;
                                break;
                            }
                        }
                    }

                    if line_found {
                        break;
                    }
                }

                if !line_found {
                    invalid_citations.push(InvalidCitation {
                        citation_text: citation.full_text.clone(),
                        reason: format!(
                            "Line {} not found in retrieved content for file '{}'",
                            line_num, citation.file_path
                        ),
                        file_path: Some(citation.file_path.clone()),
                        line_number: Some(line_num),
                    });
                    warnings.push(format!(
                        "Citation '{}' line number may be outside retrieved context",
                        citation.full_text
                    ));
                    continue;
                }
            }

            // Citation is valid
            valid_count += 1;
        }

        // Calculate confidence score
        let confidence = if citations.is_empty() {
            1.0
        } else {
            valid_count as f32 / citations.len() as f32
        };

        // Add overall warning if confidence is low
        if confidence < 0.7 {
            warnings.insert(
                0,
                format!(
                    "Low citation confidence ({:.0}%) - answer may contain inaccurate references",
                    confidence * 100.0
                ),
            );
        }

        if self.debug {
            tracing::debug!(valid = valid_count, total = citations.len(), confidence_pct = format_args!("{:.2}", confidence * 100.0), "[CitationValidator] Validation complete");
        }

        ValidationResult {
            confidence,
            total_citations: citations.len(),
            valid_citations: valid_count,
            invalid_citations,
            warnings,
        }
    }
}

impl Default for CitationValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_citations_with_line_numbers() {
        let validator = CitationValidator::new();
        let answer = "Check fast_attention_reranker.rs:42 for the implementation.";
        let citations = validator.extract_citations(answer);

        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].file_path, "fast_attention_reranker.rs");
        assert_eq!(citations[0].line_number, Some(42));
    }

    #[test]
    fn test_extract_citations_without_line_numbers() {
        let validator = CitationValidator::new();
        let answer = "The logic is in query_rewriter.rs and citation_validator.rs";
        let citations = validator.extract_citations(answer);

        assert_eq!(citations.len(), 2);
        assert!(citations.iter().any(|c| c.file_path == "query_rewriter.rs"));
        assert!(citations
            .iter()
            .any(|c| c.file_path == "citation_validator.rs"));
    }

    #[test]
    fn test_extract_citations_with_paths() {
        let validator = CitationValidator::new();
        let answer = "See src/rag/fast_attention_reranker.rs:324 for details";
        let citations = validator.extract_citations(answer);

        assert_eq!(citations.len(), 1);
        assert_eq!(
            citations[0].file_path,
            "src/rag/fast_attention_reranker.rs"
        );
        assert_eq!(citations[0].line_number, Some(324));
    }

    #[test]
    fn test_false_positive_filtering() {
        let validator = CitationValidator::new();
        let answer = "Version 1.2.3 was released e.g. last year";
        let citations = validator.extract_citations(answer);

        // Should not extract version numbers or "e.g."
        assert_eq!(citations.len(), 0);
    }

    #[test]
    fn test_validation_with_valid_citation() {
        let validator = CitationValidator::new();
        let answer = "Check query_rewriter.rs:100 for details";

        let sources = vec![SourceDocument {
            file_path: "src/rag/query_rewriter.rs".to_string(),
            line_ranges: vec![(1, 200)],
            content: String::new(),
        }];

        let result = validator.validate(answer, &sources);

        assert_eq!(result.total_citations, 1);
        assert_eq!(result.valid_citations, 1);
        assert!(result.confidence > 0.9);
    }

    #[test]
    fn test_validation_with_invalid_citation() {
        let validator = CitationValidator::new();
        let answer = "Check nonexistent.rs:100 for details";

        let sources = vec![SourceDocument {
            file_path: "src/rag/query_rewriter.rs".to_string(),
            line_ranges: vec![(1, 200)],
            content: String::new(),
        }];

        let result = validator.validate(answer, &sources);

        assert_eq!(result.total_citations, 1);
        assert_eq!(result.valid_citations, 0);
        assert!(result.confidence < 0.5);
        assert!(!result.invalid_citations.is_empty());
    }
}
