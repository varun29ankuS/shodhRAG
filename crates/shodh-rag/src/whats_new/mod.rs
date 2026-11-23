//! What's new: the files that arrived in the user's watched folders since Shodh last
//! looked, and the papers the user's paper alerts found, kept in `shodh.db` ([`store`])
//! for the Library's What's new section and the Inbox items that tell of them.
//!
//! Rows are written the moment something is found (a folder sync reports the files it
//! indexed for the first time; an alert's check finds papers); the engine job that found
//! them then posts one Inbox item for what it has not told of yet. A file or paper is
//! never listed twice (`(kind, source_ref)` is unique), and rows are kept
//! [`store::FEED_RETENTION_DAYS`] days.
//!
//! - [`alerts`]: one check of a paper alert (arXiv, OpenAlex, Semantic Scholar).
//! - [`pace`]: spacing between requests to each API.
//! - [`library`]: the library's papers and the titles its file names spell.
//! - [`topics`]: topics suggested from the library.

pub mod alerts;
pub mod library;
pub mod pace;
pub mod store;
pub mod topics;
