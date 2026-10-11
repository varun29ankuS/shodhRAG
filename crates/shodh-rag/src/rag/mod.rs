//! Query decomposition: splits multi-part questions into sub-queries and merges
//! their search results.

pub mod query_decomposer;

pub use query_decomposer::{
    decompose_query, merge_results, DecomposedQuery, DecompositionStrategy, HasIdAndScore,
};
