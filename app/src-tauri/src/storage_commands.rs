//! Storage maintenance for the document index.

use crate::rag_commands::RagState;
use tauri::State;

/// Optimize storage by triggering index creation if needed
#[tauri::command]
pub async fn optimize_storage(state: State<'_, RagState>) -> Result<String, String> {
    tracing::info!("Optimizing storage...");

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // Trigger index optimization
    rag.optimize()
        .await
        .map_err(|e| format!("Failed to optimize: {}", e))?;

    let stats = rag.get_statistics().await.unwrap_or_default();
    let total_chunks = stats.get("total_chunks").cloned().unwrap_or_default();

    Ok(format!("Storage optimized. Total chunks: {}", total_chunks))
}
