//! Shared piece of the former chat pipeline that other modules still use:
//! the progress-event sink.
//!
//! Answers are produced by agent sessions (`crate::harness`).

/// Event sink for streaming tokens and progress events.
/// Tauri provides an implementation wrapping AppHandle.emit().
/// HTTP servers can provide SSE-based implementations.
pub trait EventEmitter: Send + Sync {
    fn emit(&self, event: &str, data: serde_json::Value);
}

/// No-op emitter for non-streaming contexts.
pub struct NoopEmitter;
impl EventEmitter for NoopEmitter {
    fn emit(&self, _event: &str, _data: serde_json::Value) {}
}
