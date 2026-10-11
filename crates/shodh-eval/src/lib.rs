//! Evaluation harness for shodh.
//!
//! Measures the real pipeline, not a re-implementation of it:
//! - retrieval: [`shodh_rag::indexing::index_folder`] into a fresh index, then
//!   [`shodh_rag::RAGEngine::search_comprehensive`] (what the agent's
//!   `search_documents` tool calls);
//! - answers: an omp agent session with the library's document tools and the
//!   grounding verifier, exactly as the app answers.
//!
//! Scoring is done by the pure functions in [`metrics`]; reports, baselines and
//! the regression gate live in [`report`].
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod answers;
pub mod corpus;
pub mod dataset;
pub mod engine;
pub mod generate;
pub mod metrics;
pub mod models;
pub mod pdf;
pub mod provider;
pub mod report;
pub mod retrieval;
pub mod synth;
pub mod text;
