//! Library file browser commands.

use std::path::PathBuf;

use tauri::State;

use crate::agent_tools::{list_directory_in, DirEntry, IndexedRoots, SourceRoots};
use crate::rag_commands::RagState;

/// Entries one listing returns at most.
const MAX_ENTRIES: usize = 5_000;

/// Files and folders under `path`, which must be inside an indexed source
/// folder (checked after resolving symlinks). Paths are spelled as on disk;
/// folders come first; recursive listings do not follow symlinked folders.
#[tauri::command]
pub async fn list_directory(
    path: String,
    recursive: bool,
    rag: State<'_, RagState>,
) -> Result<Vec<DirEntry>, String> {
    let path = PathBuf::from(path.trim());
    if !path.is_absolute() {
        return Err("The folder path must be absolute".to_string());
    }
    let roots = IndexedRoots {
        rag: rag.rag.clone(),
    }
    .roots()
    .await
    .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || list_directory_in(&path, recursive, MAX_ENTRIES, &roots))
        .await
        .map_err(|e| format!("Listing was interrupted: {e}"))?
        .map(|listing| listing.entries)
        .map_err(|e| e.to_string())
}
