//! Tauri implementation of `shodh_rag::chat::EventEmitter`: forwards
//! progress events (e.g. `indexing-progress`) from the core library to the
//! WebView.

use shodh_rag::chat::EventEmitter;
use tauri::Emitter;

pub struct TauriEventEmitter {
    app_handle: tauri::AppHandle,
}

impl TauriEventEmitter {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self { app_handle }
    }
}

impl EventEmitter for TauriEventEmitter {
    fn emit(&self, event: &str, data: serde_json::Value) {
        if let Err(e) = self.app_handle.emit(event, data) {
            tracing::debug!(event, error = %e, "emitting a progress event failed");
        }
    }
}
