//! Clipboard commands for image handling

use anyhow::Result;
use tauri::State;
use base64::{Engine as _, engine::general_purpose};

use crate::rag_commands::RagState;
use crate::image_upload_commands::{ImageProcessResult, process_image_from_base64};

/// Read image from clipboard and process it
#[tauri::command]
pub async fn paste_image_from_clipboard(
    state: State<'_, RagState>,
) -> Result<ImageProcessResult, String> {
    // This will be called from frontend when user presses paste button or Ctrl+V
    // The frontend will read the clipboard and pass the base64 data
    Err("This command should not be called directly. Use process_image_from_base64 instead.".to_string())
}
