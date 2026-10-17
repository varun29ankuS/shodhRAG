//! Database management commands for clearing and resetting data

use crate::rag_commands::RagState;
use shodh_rag::path_case::PathSpellings;
use shodh_rag::rag_engine::{normalize_source_path, on_disk_spelling, DocumentNames};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tauri::State;

use crate::audit_commands::AuditState;
use serde_json::json;
use shodh_rag::audit::payload::{source_change, ChangeOrigin};
use shodh_rag::audit::{AuditEventType, AuditRecord};

/// Clear all data from the database and reset to fresh state
async fn reset_database_inner(state: State<'_, RagState>) -> Result<String, String> {
    tracing::info!("=== Resetting database ===");

    // Step 1: Clear all spaces from memory and disk
    {
        let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;
        space_manager
            .clear_all_spaces()
            .map_err(|e| format!("Failed to clear spaces: {}", e))?;
    } // Drop space_manager lock before await

    // Step 2: Delete the spaces.json file from persistent storage
    let spaces_file = state.app_paths.data_dir.join("spaces.json");
    if spaces_file.exists() {
        fs::remove_file(&spaces_file)
            .map_err(|e| format!("Failed to delete spaces.json: {}", e))?;
        tracing::info!("Deleted spaces.json from {:?}", spaces_file);
    }

    // Step 3: Clear the RAG database
    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    // Clear all data using the public method
    if let Err(e) = rag.clear_all_data().await {
        tracing::info!("Failed to clear all data: {}", e);
        return Err(format!("Failed to clear all data: {}", e));
    }

    tracing::info!("All data cleared successfully");

    Ok("Database reset complete. Restart the application for a fresh start.".to_string())
}

/// Clear all documents from the database but keep spaces
async fn clear_all_documents_inner(state: State<'_, RagState>) -> Result<String, String> {
    tracing::info!("=== Clearing all documents ===");

    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    // Clear all data from RAG
    rag.clear_all_data()
        .await
        .map_err(|e| format!("Failed to clear data: {}", e))?;
    tracing::info!("Cleared all documents from RAG");
    drop(rag_guard);

    // Clear document associations from spaces
    let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;
    let spaces = space_manager
        .get_spaces()
        .map_err(|e| format!("Failed to get spaces: {}", e))?;

    for mut space in spaces {
        space.documents.clear();
        space.document_count = 0;
    }

    // Save the updated spaces
    space_manager
        .save_spaces()
        .map_err(|e| format!("Failed to save spaces: {}", e))?;

    Ok("All documents cleared from database".to_string())
}

/// Get database statistics
#[tauri::command]
pub async fn get_database_stats(state: State<'_, RagState>) -> Result<DatabaseStats, String> {
    let mut stats = DatabaseStats::default();

    // Get space count from state
    {
        let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;
        let spaces = space_manager
            .get_spaces()
            .map_err(|e| format!("Failed to get spaces: {}", e))?;
        stats.total_spaces = spaces.len();
        stats.total_documents_in_spaces = spaces.iter().map(|s| s.document_count).sum();
    } // Drop space_manager lock before await

    // Get database size from the actual data directory (not db_path which points elsewhere)
    let data_dir = &state.app_paths.data_dir;
    if data_dir.exists() {
        stats.database_size_mb = get_dir_size(data_dir) as f64 / (1024.0 * 1024.0);
    }

    // Get document count from RAG
    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // Get stats from RAG engine
    let rag_stats = rag.get_statistics().await.unwrap_or_default();
    let total_chunks: usize = rag_stats
        .get("total_chunks")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let total_docs = rag.count_documents().await.unwrap_or(0);

    stats.total_documents = total_docs;
    stats.total_vectors = total_chunks;

    Ok(stats)
}

/// Clean up orphaned documents (documents not associated with any space)
#[tauri::command]
pub async fn cleanup_orphaned_documents(state: State<'_, RagState>) -> Result<String, String> {
    tracing::info!("=== Cleaning up orphaned documents ===");

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // List all documents by metadata (not search)
    let all_docs = rag
        .list_documents(None, 100000)
        .await
        .map_err(|e| e.to_string())?;

    // Get all valid space IDs
    let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;
    let valid_space_ids: Vec<String> = space_manager
        .get_spaces()
        .map_err(|e| format!("Failed to get spaces: {}", e))?
        .iter()
        .map(|s| s.id.clone())
        .collect();

    let mut orphaned_count = 0;
    for doc in all_docs.iter() {
        // Check if document has a space_id that exists
        if let Some(space_id) = doc.metadata.get("space_id") {
            if !valid_space_ids.contains(space_id) {
                orphaned_count += 1;
                // Note: Need delete_document method in ComprehensiveRAG
                tracing::info!(
                    "Found orphaned document with invalid space_id: {}",
                    space_id
                );
            }
        } else {
            orphaned_count += 1;
            tracing::info!("Found orphaned document with no space_id");
        }
    }

    Ok(format!(
        "Found {} orphaned documents. Manual cleanup required for now.",
        orphaned_count
    ))
}

#[derive(serde::Serialize, Default)]
pub struct DatabaseStats {
    pub total_spaces: usize,
    pub total_documents: usize,
    pub total_vectors: usize,
    pub total_documents_in_spaces: usize,
    pub database_size_mb: f64,
}

// Helper function to calculate directory size
fn get_dir_size(path: &Path) -> u64 {
    let mut size = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_dir() {
                    size += get_dir_size(&entry.path());
                } else {
                    size += metadata.len();
                }
            }
        }
    }
    size
}

/// List all distinct indexed source files from the vector store: each
/// document's id, title, identity (`source`) and the path to show for it
/// (`display_path`).
#[tauri::command]
pub async fn list_indexed_sources(
    state: State<'_, RagState>,
) -> Result<Vec<IndexedSourceInfo>, String> {
    let (docs, names) = {
        let rag_guard = state.rag.read().await;
        let docs = rag_guard
            .get_document_info()
            .await
            .map_err(|e| format!("Failed to list sources: {}", e))?;
        let names = rag_guard
            .document_names()
            .await
            .map_err(|e| format!("Failed to list sources: {}", e))?;
        (docs, names)
    };

    // Documents indexed before their display path was stored are resolved
    // from disk (each folder once per process), off the async runtime.
    let sources = tokio::task::spawn_blocking(move || {
        indexed_source_infos(docs, &names, PathSpellings::shared())
    })
    .await
    .map_err(|e| format!("Failed to list sources: {}", e))?;

    tracing::info!("Listed {} indexed sources", sources.len());
    Ok(sources)
}

/// The listed documents with the path each is shown under: the stored
/// display path when it spells the document's identity, else its spelling
/// on disk from `spellings` (its folder's spelling and the file name the
/// index keeps, `filename` or the title with the extension), else the
/// identity itself (a file that cannot be resolved, or an in-app record such
/// as `calendar://task/<id>`).
fn indexed_source_infos(
    docs: Vec<(String, String, String)>,
    names: &HashMap<String, DocumentNames>,
    spellings: &PathSpellings,
) -> Vec<IndexedSourceInfo> {
    docs.into_iter()
        .map(|(doc_id, title, source)| {
            let kept = names.get(&doc_id);
            let stored = kept
                .and_then(|n| n.display_path.as_ref())
                .filter(|d| normalize_source_path(Path::new(d.as_str())) == source)
                .cloned();
            let display_path = stored.unwrap_or_else(|| {
                if source.contains("://") {
                    return source.clone();
                }
                let extension = Path::new(&source)
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default();
                let titled = format!("{}{extension}", title.trim());
                let filename = kept.and_then(|n| n.filename.as_deref()).unwrap_or("");
                spellings
                    .spelling(&source, &[filename, titled.as_str()])
                    .unwrap_or_else(|| source.clone())
            });
            IndexedSourceInfo {
                doc_id,
                title,
                source,
                display_path,
            }
        })
        .collect()
}

#[derive(serde::Serialize)]
pub struct IndexedSourceInfo {
    pub doc_id: String,
    pub title: String,
    /// The file's identity in the index (lowercased on Windows).
    pub source: String,
    /// The path to show: the file system's spelling of `source`.
    pub display_path: String,
}

/// `path` with the letter case the file system gives it (Windows), for paths
/// saved in a lowercased spelling; any other difference (a short name
/// expanded, a junction followed) is never applied, so the path keeps its
/// identity. Returns `path` unchanged when it cannot be resolved, and on
/// other systems, whose paths keep their case.
#[tauri::command]
pub async fn resolve_path_case(path: String) -> Result<String, String> {
    if !cfg!(windows) || path.trim().is_empty() {
        return Ok(path);
    }
    tokio::task::spawn_blocking(move || on_disk_spelling(Path::new(&path)).unwrap_or(path))
        .await
        .map_err(|e| format!("Resolving the path was interrupted: {}", e))
}

/// Clear every document from the index (audited).
#[tauri::command]
pub async fn clear_all_documents(
    state: State<'_, RagState>,
    audit: State<'_, AuditState>,
) -> Result<String, String> {
    let result = clear_all_documents_inner(state).await;
    let outcome = match &result {
        Ok(_) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e}),
    };
    audit.record(AuditRecord::new(
        AuditEventType::SourceChange,
        source_change("clear_all", ChangeOrigin::Ui, None, None, outcome),
    ));
    result
}

/// Reset the index and spaces (audited).
#[tauri::command]
pub async fn reset_database(
    state: State<'_, RagState>,
    audit: State<'_, AuditState>,
) -> Result<String, String> {
    let result = reset_database_inner(state).await;
    let outcome = match &result {
        Ok(_) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e}),
    };
    audit.record(AuditRecord::new(
        AuditEventType::SourceChange,
        source_change("reset", ChangeOrigin::Ui, None, None, outcome),
    ));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: &str, source: &str) -> (String, String, String) {
        (id.to_string(), "Title".to_string(), source.to_string())
    }

    fn named(display_path: Option<&str>, filename: Option<&str>) -> DocumentNames {
        DocumentNames {
            display_path: display_path.map(str::to_string),
            filename: filename.map(str::to_string),
        }
    }

    /// A stored display path is used only when it spells the document's
    /// identity; records and files that cannot be resolved show the identity.
    #[test]
    fn listed_sources_show_a_stored_spelling_only_of_the_same_file() {
        let names = HashMap::from([
            (
                "other".to_string(),
                named(Some("D:/Elsewhere/Other.md"), None),
            ),
            (
                "task".to_string(),
                named(Some("D:/Elsewhere/Task.md"), None),
            ),
        ]);
        let infos = indexed_source_infos(
            vec![
                doc("other", "/no/such/folder/a.md"),
                doc("task", "calendar://task/1"),
            ],
            &names,
            &PathSpellings::new(),
        );
        assert_eq!(infos[0].source, "/no/such/folder/a.md");
        assert_eq!(infos[0].display_path, "/no/such/folder/a.md");
        assert_eq!(infos[1].display_path, "calendar://task/1");
        let json = serde_json::to_value(&infos[1]).unwrap();
        assert_eq!(json["display_path"], "calendar://task/1");
        assert_eq!(json["doc_id"], "task");
    }

    /// On Windows a document listed under its lowercase identity is shown with
    /// the case its folders and name have on disk: stored, or resolved for
    /// documents indexed before it was stored.
    #[cfg(windows)]
    #[test]
    fn listed_sources_show_the_case_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = shodh_rag::rag_engine::canonical_path(&fs::canonicalize(dir.path()).unwrap());
        let file = root.join("Research").join("Papers").join("SPANN - X.pdf");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"%PDF-1.4").unwrap();
        let spelled = file.display().to_string();
        let identity = normalize_source_path(&file);
        let spellings = PathSpellings::new();

        // No matching name kept: the file itself is resolved.
        let resolved =
            indexed_source_infos(vec![doc("old", &identity)], &HashMap::new(), &spellings);
        assert_eq!(resolved[0].source, identity);
        assert_eq!(resolved[0].display_path, spelled);

        // A stored spelling is shown as stored, without reading the disk.
        let names = HashMap::from([(
            "new".to_string(),
            named(Some(spelled.to_uppercase().as_str()), None),
        )]);
        let kept = indexed_source_infos(vec![doc("new", &identity)], &names, &spellings);
        assert_eq!(kept[0].display_path, spelled.to_uppercase());

        let gone = format!("{identity}.gone");
        let missing = indexed_source_infos(vec![doc("gone", &gone)], &HashMap::new(), &spellings);
        assert_eq!(missing[0].display_path, gone);
    }

    /// Many documents of one folder, indexed before display paths were
    /// stored, ask the file system once: for the folder. Their names come
    /// from the index (`filename`, else the title with the extension).
    #[cfg(windows)]
    #[test]
    fn listing_a_folder_of_legacy_documents_resolves_the_folder_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let spellings = PathSpellings::with_canonicalize(move |path| {
            counted.fetch_add(1, Ordering::SeqCst);
            if path.display().to_string().to_lowercase() == "c:\\research\\papers" {
                Ok(std::path::PathBuf::from("C:\\Research\\papers"))
            } else {
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            }
        });
        let mut docs = Vec::new();
        let mut names = HashMap::new();
        for i in 0..20 {
            let id = format!("d{i}");
            docs.push((
                id.clone(),
                format!("Paper {i}"),
                format!("c:/research/papers/paper {i}.pdf"),
            ));
            if i % 2 == 0 {
                names.insert(id, named(None, Some(format!("Paper {i}.pdf").as_str())));
            }
        }
        let infos = indexed_source_infos(docs, &names, &spellings);
        for (i, info) in infos.iter().enumerate() {
            assert_eq!(
                info.display_path,
                format!("C:\\Research\\papers\\Paper {i}.pdf")
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
