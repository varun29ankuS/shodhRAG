//! Shared helpers for integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Utc};
use shodh_ontology::{
    Extractor, ExtractorKind, Ontology, Provenance, RawValue, Statement, TextSpan, ValidStatement,
};

pub fn core() -> Ontology {
    Ontology::builtin().unwrap_or_else(|e| panic!("{e}"))
}

pub fn with_research() -> Ontology {
    Ontology::builtin_with_packs(&["research"]).unwrap_or_else(|e| panic!("{e}"))
}

pub fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap_or_else(|e| panic!("{text}: {e}"))
        .with_timezone(&Utc)
}

pub fn provenance(extracted_at: &str) -> Provenance {
    Provenance {
        source: "docs/invoices/inv-1043.pdf".to_owned(),
        generation: 3,
        page: Some(1),
        span: Some(TextSpan { start: 120, end: 180 }),
        extractor: Extractor {
            kind: ExtractorKind::Rule,
            version: "invoice-rules/1.0.0".to_owned(),
        },
        confidence: 0.98,
        extracted_at: at(extracted_at),
    }
}

pub fn statement(id: &str, class: &str, properties: Vec<(&str, RawValue)>) -> Statement {
    Statement {
        id: id.to_owned(),
        class: class.to_owned(),
        subject: None,
        properties: properties
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        ontology_version: semver::Version::new(1, 0, 0),
        valid_from: None,
        provenance: Some(provenance("2026-10-03T09:00:00Z")),
    }
}

pub fn valid(ontology: &Ontology, statement: &Statement) -> ValidStatement {
    ontology
        .validate(statement)
        .unwrap_or_else(|v| panic!("expected valid statement, got {v:?}"))
}

/// 1-based line of the first line in `text` that equals `needle` after trimming.
pub fn line_of(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|l| l.trim() == needle)
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("`{needle}` not found"))
}
