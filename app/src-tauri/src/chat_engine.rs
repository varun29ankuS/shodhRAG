//! Thin Tauri wrapper around shodh_rag::chat::ChatEngine.
//! All business logic lives in the backend library.

use std::sync::Arc;
use tokio::sync::RwLock as AsyncRwLock;

// Re-export backend types so existing callers don't break
pub use shodh_rag::chat::{
    AssistantResponse, Artifact, ArtifactType, ChatContext, Citation, ConversationMessage,
    EventEmitter, Intent, MessagePlatform, ResponseMetadata, SearchResult, UserMessage,
};
pub use shodh_rag::chat::engine::ChatEngine;

/// Tauri-specific EventEmitter that wraps AppHandle for streaming tokens.
///
/// Tauri events are broadcast app-wide, so concurrent or abandoned requests
/// would otherwise be indistinguishable on the frontend. When a request id is
/// set, it is attached as `requestId` to every object payload so listeners can
/// ignore events that belong to a different (e.g. cancelled) request.
pub struct TauriEventEmitter {
    app_handle: tauri::AppHandle,
    request_id: Option<String>,
}

impl TauriEventEmitter {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self { app_handle, request_id: None }
    }

    pub fn with_request_id(app_handle: tauri::AppHandle, request_id: Option<String>) -> Self {
        Self { app_handle, request_id }
    }
}

impl EventEmitter for TauriEventEmitter {
    fn emit(&self, event: &str, data: serde_json::Value) {
        use tauri::Emitter;
        let payload = match (&self.request_id, data) {
            (Some(id), serde_json::Value::Object(mut map)) => {
                map.insert("requestId".to_string(), serde_json::Value::String(id.clone()));
                serde_json::Value::Object(map)
            }
            (_, other) => other,
        };
        let _ = self.app_handle.emit(event, payload);
    }
}

/// Tauri-specific ToolLoopEmitter that forwards tool call events to the frontend.
pub struct TauriToolEmitter {
    app_handle: tauri::AppHandle,
}

impl TauriToolEmitter {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self { app_handle }
    }
}

impl shodh_rag::agent::tool_loop::ToolLoopEmitter for TauriToolEmitter {
    fn on_content_delta(&self, delta: &str) {
        use tauri::Emitter;
        let _ = self.app_handle.emit("chat_token", serde_json::json!({
            "delta": delta,
        }));
    }

    fn on_tool_start(&self, tool_name: &str, arguments: &str) {
        use tauri::Emitter;
        let _ = self.app_handle.emit("tool_call_start", serde_json::json!({
            "tool_name": tool_name,
            "arguments": arguments,
        }));
    }

    fn on_tool_complete(&self, invocation: &shodh_rag::agent::tool_loop::ToolInvocation) {
        use tauri::Emitter;
        let _ = self.app_handle.emit("tool_call_complete", serde_json::json!({
            "tool_name": invocation.tool_name,
            "result": invocation.result,
            "success": invocation.success,
            "duration_ms": invocation.duration_ms,
        }));
    }

    fn on_thinking(&self, message: &str) {
        use tauri::Emitter;
        let _ = self.app_handle.emit("agent_thinking", serde_json::json!({
            "message": message,
        }));
    }
}
