//! Answer Validation Module - Tauri Wrapper
//!
//! Thin wrapper around core library's CitationValidator for Tauri desktop app.

use serde::{Deserialize, Serialize};
use shodh_rag::rag::{CitationValidator, SourceDocument};
use crate::rag_commands::SearchResult;

// Re-export core library types with camelCase for frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationResult {
    pub confidence: f32,
    pub total_citations: usize,
    pub valid_citations: usize,
    pub invalid_citations: Vec<InvalidCitation>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidCitation {
    pub citation_text: String,
    pub reason: String,
    pub file_path: Option<String>,
    pub line_number: Option<usize>,
}

impl ValidationResult {
    pub fn empty() -> Self {
        Self {
            confidence: 0.0,
            total_citations: 0,
            valid_citations: 0,
            invalid_citations: vec![],
            warnings: vec![],
        }
    }
}

/// Validate citations in LLM answer against retrieved source documents
pub fn validate_citations(
    answer: &str,
    source_chunks: &[SearchResult],
) -> ValidationResult {
    // Convert SearchResult to SourceDocument format for core library
    let source_documents: Vec<SourceDocument> = source_chunks
        .iter()
        .map(|chunk| SourceDocument {
            file_path: chunk.source_file.clone(),
            line_ranges: chunk.line_range.map(|lr| vec![lr]).unwrap_or_default(),
            content: chunk.snippet.clone(),
        })
        .collect();

    // Use core library CitationValidator
    let validator = CitationValidator::new().with_debug(true);
    let result = validator.validate(answer, &source_documents);

    ValidationResult {
        confidence: result.confidence,
        total_citations: result.total_citations,
        valid_citations: result.valid_citations,
        invalid_citations: result.invalid_citations.into_iter().map(|ic| InvalidCitation {
            citation_text: ic.citation_text,
            reason: ic.reason,
            file_path: ic.file_path,
            line_number: ic.line_number,
        }).collect(),
        warnings: result.warnings,
    }
}
