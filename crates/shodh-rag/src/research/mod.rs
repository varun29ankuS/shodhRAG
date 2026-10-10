//! Research objects built on the statement store.
//!
//! - **Snippets** ([`snippets`]): a saved region of a PDF page — its image, the text inside
//!   it and its source (file, page, rectangle) — stored as a research-pack `Snippet`
//!   statement. The image bytes live in `shodh.db` (`snippet_images`, content-addressed),
//!   see [`db`].
//! - **Results** ([`results`]): `Result` statements (method, dataset, metric, value, setting,
//!   paper, cell page and box) read from the structured table blocks of papers, and the
//!   cross-paper comparison built from them.
//!
//! - **Figures and equations** ([`figures`], [`equations`], [`objects`]): the figures of a
//!   paper (caption, page and the region the drawing occupies) and its display equations
//!   (number, LaTeX from the paper's `.tex` source when it is in the library, else rebuilt
//!   from the text layer), derived from the PDF on demand and never stored.
//!
//! Snippets and results are document-derived statements: their provenance source is the file path, so the
//! memory layer (which only reads `conversation://` and settings sources) never sees them.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod citations;
#[cfg(test)]
mod corpus_dump;
pub mod db;
pub mod equations;
pub mod figures;
pub mod objects;
pub mod pdf_text;
pub mod results;
pub mod snippets;
pub mod vision;

use sha2::{Digest, Sha256};
use shodh_ontology::EntityRef;

use crate::statements::StatementError;

pub use db::{ImageInfo, ResearchDb, MAX_IMAGE_BYTES};
pub use pdf_text::{text_in_rect, PageRect, RegionText, TextBox};
pub use snippets::{
    NewSnippet, Snippet, SnippetKind, SnippetPatch, SnippetQuery, SnippetService, SnippetTable,
};

/// Errors of the research layer.
#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    /// The request is malformed (bad rectangle, page, image, filter).
    #[error("{0}")]
    Invalid(String),
    /// No snippet or result with this id.
    #[error("{0}")]
    NotFound(String),
    /// The statement store failed or rejected the statement.
    #[error(transparent)]
    Statement(#[from] StatementError),
    /// `shodh.db` failed.
    #[error("research database error: {0}")]
    Database(String),
    /// The PDF could not be read.
    #[error("{0}")]
    Pdf(String),
    /// A language model call failed or returned something unusable.
    #[error("{0}")]
    Model(String),
    /// A background task failed to complete.
    #[error("research task failed: {0}")]
    Task(String),
}

impl From<rusqlite::Error> for ResearchError {
    fn from(e: rusqlite::Error) -> Self {
        ResearchError::Database(e.to_string())
    }
}

impl From<crate::audit::AuditError> for ResearchError {
    fn from(e: crate::audit::AuditError) -> Self {
        ResearchError::Database(e.to_string())
    }
}

/// Result alias of the research layer.
pub type ResearchResult<T> = Result<T, ResearchError>;

/// The indexer's spelling of a file path (see [`crate::rag_engine::canonical_path`]), so
/// snippets and results of one file agree with its citations.
pub fn canonical_file(path: &str) -> String {
    crate::rag_engine::canonical_path(std::path::Path::new(path.trim()))
        .to_string_lossy()
        .to_string()
}

/// The key two spellings of one file share: the index's stored form (forward slashes,
/// lower-cased on Windows; see [`crate::rag_engine::normalize_source_path`]).
pub fn path_key(path: &str) -> String {
    crate::rag_engine::normalize_source_path(std::path::Path::new(path.trim()))
}

/// Entity id of a document: `doc:` and the first 16 bytes of the SHA-256 of its
/// [`path_key`], in hex. Paths contain spaces and separators; the hash is a stable, opaque
/// id that every spelling of the path maps to.
pub fn document_entity(path: &str) -> EntityRef {
    let digest = Sha256::digest(path_key(path).as_bytes());
    EntityRef::typed(format!("doc:{}", hex::encode(&digest[..16])), "Document")
}

/// The file name of a path, for display.
pub fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// Run blocking work (SQLite, PDF parsing) off the async runtime.
pub(crate) async fn blocking<T, F>(work: F) -> ResearchResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ResearchResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ResearchError::Task(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_ids_follow_the_indexer_spelling() {
        let a = document_entity("C:/Papers/./deep/a b.pdf");
        let b = document_entity("C:\\Papers\\deep\\a b.pdf");
        if cfg!(windows) {
            assert_eq!(a, b);
        }
        assert!(a.id.starts_with("doc:"));
        assert_eq!(a.id.len(), 4 + 32);
        assert_eq!(a.class.as_deref(), Some("Document"));
        assert_ne!(a, document_entity("C:/Papers/other.pdf"));
        assert_eq!(file_name("C:\\Papers\\a b.pdf"), "a b.pdf");
    }
}
