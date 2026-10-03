pub mod chunker;
pub mod document_model;
pub mod lopdf_parser;
pub mod parser;
#[cfg(test)]
pub(crate) mod pdf_fixtures;
pub mod pdf_info;
pub mod pdf_layout;
pub mod structure_chunker;
pub mod tabular;
pub mod text_structure;

#[cfg(windows)]
pub mod windows_ocr;

pub use chunker::{ChunkResult, ContextualChunkResult, TextChunker};
pub use lopdf_parser::LoPdfParser;
pub use parser::{DocumentParser, ParsedDocument};
