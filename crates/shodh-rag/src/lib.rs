// Allow unused variables for ported code with integration points
#![allow(unused_variables)]

pub mod chat;
pub mod config;
pub mod context;
pub mod embeddings;
pub mod graph;
pub mod indexing;
pub mod processing;
pub mod rag_engine;
pub mod reranking;
pub mod search;
pub mod space;
pub mod storage;
pub mod templates;
pub mod types;

// Ported modules from old shodh-rag
pub mod agent;
pub mod llm;
pub mod memory;
pub mod rag;
pub mod system;

// Re-export primary types for convenience
pub use config::RAGConfig;
pub use rag_engine::RAGEngine;
pub use types::{
    Citation, ComprehensiveResult, DocumentFormat, MetadataFilter, SimpleSearchResult,
};

// Re-export comprehensive_system types for backward compatibility
pub mod comprehensive_system {
    pub use crate::config::RAGConfig as ComprehensiveRAGConfig;
    pub use crate::rag_engine::RAGEngine as ComprehensiveRAG;
    pub use crate::types::{Citation, ComprehensiveResult, DocumentFormat, SimpleSearchResult};
}

// Re-export LLM types
pub use llm::{
    ApiProvider, DeviceType, GenerationConfig, LLMConfig, LLMManager, LLMMode, LocalModel,
    MemoryUsage, ModelManager, ProviderInfo, QuantizationType,
};

// Re-export common types
pub use anyhow::{Error, Result};
pub use uuid::Uuid;
