//! Calendar data and its search indexing, and memory-backed conversation
//! continuity.
//!
//! The earlier agent framework (executor, crews, tool loop, tool registry
//! and its tools) was replaced by the agent harness in `crate::harness`.

pub mod calendar;
pub mod calendar_indexer;
pub mod conversation_continuity;

pub use conversation_continuity::{Conversation, ConversationManager, Message, MessageRole};
