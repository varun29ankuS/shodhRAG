//! RAG utilities module - query rewriting, retrieval decisions, context optimization
//! These are ported from the old comprehensive RAG system for backward compatibility.

pub mod metadata;
pub mod query_rewriter;
pub mod retrieval_decision;
pub mod context_optimizer;
pub mod system_context;
pub mod structured_output;
pub mod citation_validator;
pub mod form_exporter;
pub mod conversation_summarizer;
pub mod query_decomposer;
pub mod context_compressor;
pub mod eval;
pub mod llm_router;

// Re-export commonly used types
pub use metadata::{MetadataFilter as RagMetadataFilter, AccessLevel, SourceType};
pub use query_rewriter::{QueryRewriter, ConversationContext};
pub use retrieval_decision::{
    QueryAnalyzer, CorpusStats, QueryAnalysis, QueryIntent, RetrievalStrategy,
    RetrievalDecision, RelevanceScore, QueryRequirements,
};
pub use context_optimizer::{build_context_for_query, ContextQueryIntent, ContextTier};
pub use system_context::{build_system_context, build_prompt_prefix, QueryType};
pub use structured_output::{parse_llm_response, FormField, FieldType, StructuredOutput, ChartType, ChartData, Dataset, DiagramType, SystemActionType, STRUCTURED_OUTPUT_INSTRUCTIONS};
pub use citation_validator::{CitationValidator, SourceDocument};
pub use form_exporter::{export_form_as_html, export_form_as_json_schema};
pub use conversation_summarizer::{compress_history, format_compressed_history, CompressedHistory};
pub use query_decomposer::{decompose_query, merge_results, DecomposedQuery, DecompositionStrategy, HasIdAndScore};
pub use context_compressor::{compress_chunk, compress_context};
pub use eval::{evaluate, format_report, EvalQuery, EvalResult, EvalMetrics, QueryMetrics};
pub use llm_router::{RouterOutput, RouterIntent, RouterTokenUsage};
