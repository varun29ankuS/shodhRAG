//! Storage maintenance for the document index.

use crate::rag_commands::RagState;
use tauri::State;

/// Optimize storage by triggering index creation if needed
#[tauri::command]
pub async fn optimize_storage(state: State<'_, RagState>) -> Result<String, String> {
    tracing::info!("Optimizing storage...");

    // The pass runs without the engine lock: searches go on meanwhile, and a
    // purge waiting for the pass holds no engine lock either.
    let maintenance = state.rag.read().await.store_maintenance();
    maintenance
        .run()
        .await
        .map_err(|e| format!("Failed to optimize: {}", e))?;

    let stats = state
        .rag
        .read()
        .await
        .get_statistics()
        .await
        .unwrap_or_default();
    let total_chunks = stats.get("total_chunks").cloned().unwrap_or_default();

    Ok(format!("Storage optimized. Total chunks: {}", total_chunks))
}
