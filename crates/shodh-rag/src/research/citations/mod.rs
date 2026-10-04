//! The citation graph of the user's papers.
//!
//! Each library PDF is scanned for its own identity (title, DOI or arXiv id) and its
//! bibliography, one parsed reference per entry ([`reference`], [`segment`],
//! [`identity`], [`scan`]). When the user's privacy policy allows the web, papers are
//! matched to OpenAlex works by identifier, or by a strict title match for references
//! without one ([`resolve`]); only bibliographic identifiers and reference titles are sent.
//! The graph is assembled deterministically ([`build`]) and stored as research-pack
//! statements: Paper nodes (library papers flagged `inLibrary`) with `cites`,
//! `authoredBy`, `publishedIn`, `usesMethod` and `evaluatedOn`, Author and Venue nodes, and
//! Method nodes with `proposedIn` ([`service`]). Methods and datasets come only from Result
//! statements and deterministic heading/caption rules; no entity model is involved.
//!
//! [`graph`] serves the snapshot: neighbours, citers, shared references, lineage paths,
//! filters, and personalized PageRank, which ranks library files as a third list for
//! document search ([`crate::search::graph_fusion`]). [`views`] shapes it for the pages,
//! the graph view and the agent tools.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

#[cfg(test)]
mod corpus_dump;
#[cfg(test)]
mod corpus_tests;
pub mod identity;
pub mod reference;
pub mod resolve;
pub mod scan;
pub mod segment;
pub mod text;

pub use resolve::{Resolver, ScholarlyTransport};
