//! What's new: the files that arrived in the user's watched folders since Shodh last
//! looked, kept in `shodh.db` ([`store`]) for the Library's What's new section and the
//! Inbox items that tell of them.
//!
//! Rows are written the moment something is found (a folder sync reports the files it
//! indexed for the first time); the engine job that found them then posts one Inbox item
//! for what it has not told of yet. A file is never listed twice (`(kind, source_ref)` is
//! unique), and rows are kept [`store::FEED_RETENTION_DAYS`] days.

pub mod store;
