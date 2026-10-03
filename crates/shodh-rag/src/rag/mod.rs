//! RAG utilities: query decomposition, context optimization, citations, structured output
//! These are ported from the old comprehensive RAG system for backward compatibility.

pub mod citation_validator;
pub mod context_compressor;
pub mod context_optimizer;
pub mod conversation_summarizer;
pub mod eval;
pub mod form_exporter;
pub mod metadata;
pub mod query_decomposer;
pub mod structured_output;
pub mod system_context;

// Re-export commonly used types
pub use citation_validator::{CitationValidator, SourceDocument};
pub use context_compressor::{compress_chunk, compress_context};
pub use context_optimizer::{build_context_for_query, ContextQueryIntent, ContextTier};
pub use conversation_summarizer::{compress_history, format_compressed_history, CompressedHistory};
pub use eval::{evaluate, format_report, EvalMetrics, EvalQuery, EvalResult, QueryMetrics};
pub use form_exporter::{export_form_as_html, export_form_as_json_schema};
pub use metadata::{AccessLevel, MetadataFilter as RagMetadataFilter, SourceType};
pub use query_decomposer::{
    decompose_query, merge_results, DecomposedQuery, DecompositionStrategy, HasIdAndScore,
};
pub use structured_output::{
    parse_llm_response, ChartData, ChartType, Dataset, DiagramType, FieldType, FormField,
    StructuredOutput, SystemActionType, STRUCTURED_OUTPUT_INSTRUCTIONS,
};
pub use system_context::{build_prompt_prefix, build_system_context, QueryType};
