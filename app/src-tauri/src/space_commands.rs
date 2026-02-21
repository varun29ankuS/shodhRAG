//! Thin Tauri wrappers for space management commands.
//! Business logic lives in shodh_rag::space::SpaceManager.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;
use crate::rag_commands::RagState;
use shodh_rag::comprehensive_system::{Citation, DocumentFormat};
use uuid::Uuid;
use chrono::Utc;

/// Frontend-friendly Space type (camelCase, includes color).
/// Maps to/from shodh_rag::space::Space.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub color: Option<String>,
    pub document_count: usize,
    pub last_active: String,
    pub is_shared: bool,
    pub folder_path: Option<String>,
    pub watching_changes: bool,
}

impl From<shodh_rag::space::Space> for Space {
    fn from(s: shodh_rag::space::Space) -> Self {
        Space {
            id: s.id,
            name: s.name,
            emoji: s.emoji,
            color: None,
            document_count: s.document_count,
            last_active: s.last_active,
            is_shared: s.is_shared,
            folder_path: s.folder_path,
            watching_changes: s.watching_changes,
        }
    }
}

#[tauri::command]
pub async fn create_space(
    state: State<'_, RagState>,
    name: String,
    emoji: String,
    color: Option<String>,
) -> Result<Space, String> {
    let created_space = if let Ok(space_manager) = state.space_manager.lock() {
        space_manager.create_space(name.clone(), emoji.clone())
            .map_err(|e| format!("Failed to create space in manager: {}", e))?
    } else {
        return Err("Failed to access space manager".to_string());
    };

    let mut space: Space = created_space.into();
    space.color = color.clone();

    // Store space metadata in vector DB
    let mut rag_guard = state.rag.write().await;

    let mut metadata = HashMap::new();
    metadata.insert("doc_type".to_string(), "space_metadata".to_string());
    metadata.insert("space_id".to_string(), space.id.clone());
    metadata.insert("space_name".to_string(), space.name.clone());
    metadata.insert("space_emoji".to_string(), space.emoji.clone());
    if let Some(ref c) = color {
        metadata.insert("space_color".to_string(), c.clone());
    }
    metadata.insert("last_active".to_string(), space.last_active.clone());

    let citation = Citation {
        title: format!("Space: {}", space.name),
        authors: vec![],
        source: "Space Metadata".to_string(),
        year: "2024".to_string(),
        url: None,
        doi: None,
        page_numbers: None,
    };

    rag_guard.add_document(
        &format!("Space: {} {}", space.emoji, space.name),
        DocumentFormat::TXT,
        metadata,
        citation,
    ).await.map_err(|e| e.to_string())?;

    Ok(space)
}

#[tauri::command]
pub async fn get_spaces(state: State<'_, RagState>) -> Result<Vec<Space>, String> {
    if let Ok(space_manager) = state.space_manager.lock() {
        if let Ok(manager_spaces) = space_manager.get_spaces() {
            if !manager_spaces.is_empty() {
                return Ok(manager_spaces.into_iter().map(Space::from).collect());
            }
        }
    }

    // Fallback: list from RAG
    let rag_guard = state.rag.read().await;
    let results = rag_guard.list_documents(None, 10000)
        .await
        .map_err(|e| e.to_string())?;

    let mut spaces = Vec::new();
    for result in results {
        let metadata = &result.metadata;

        let doc_count = if let Ok(space_manager) = state.space_manager.lock() {
            if let Some(space_id) = metadata.get("space_id") {
                if let Ok(all) = space_manager.get_spaces() {
                    all.iter()
                        .find(|s| &s.id == space_id)
                        .map(|s| s.document_count)
                        .unwrap_or(0)
                } else { 0 }
            } else { 0 }
        } else { 0 };

        spaces.push(Space {
            id: metadata.get("space_id").unwrap_or(&String::new()).clone(),
            name: metadata.get("space_name").unwrap_or(&String::new()).clone(),
            emoji: metadata.get("space_emoji").unwrap_or(&String::new()).clone(),
            color: metadata.get("space_color").cloned(),
            document_count: doc_count,
            last_active: metadata.get("last_active").unwrap_or(&String::new()).clone(),
            is_shared: metadata.get("is_shared")
                .and_then(|v| v.parse().ok())
                .unwrap_or(false),
            folder_path: metadata.get("folder_path").cloned(),
            watching_changes: metadata.get("watching_changes")
                .and_then(|v| v.parse().ok())
                .unwrap_or(false),
        });
    }

    Ok(spaces)
}

#[tauri::command]
pub async fn add_document_to_space(
    state: State<'_, RagState>,
    space_id: String,
    title: String,
    content: String,
    file_path: Option<String>,
) -> Result<String, String> {
    let doc_id = Uuid::new_v4().to_string();

    {
        let mut rag_guard = state.rag.write().await;

        let mut metadata = HashMap::new();
        metadata.insert("space_id".to_string(), space_id.clone());
        metadata.insert("title".to_string(), title.clone());
        metadata.insert("created_at".to_string(), Utc::now().to_rfc3339());
        metadata.insert("doc_type".to_string(), "document".to_string());

        if let Some(path) = file_path {
            metadata.insert("file_path".to_string(), path);
        }

        let citation = Citation {
            title: title.clone(),
            authors: vec![],
            source: "User Document".to_string(),
            year: "2024".to_string(),
            url: None,
            doi: None,
            page_numbers: None,
        };

        rag_guard.add_document(&content, DocumentFormat::TXT, metadata, citation)
            .await.map_err(|e| e.to_string())?;
    }

    Ok(doc_id)
}

#[tauri::command]
pub async fn search_in_space(
    state: State<'_, RagState>,
    space_id: String,
    query: String,
    max_results: usize,
) -> Result<Vec<SearchResult>, String> {
    let rag_guard = state.rag.read().await;

    let filter = shodh_rag::types::MetadataFilter {
        space_id: Some(space_id.clone()),
        ..Default::default()
    };
    let filtered_results = rag_guard.search_comprehensive(&query, max_results, Some(filter))
        .await
        .map_err(|e| e.to_string())?;

    Ok(filtered_results.into_iter().map(|r| SearchResult {
        id: r.id.to_string(),
        score: r.score,
        snippet: r.snippet,
        metadata: r.metadata,
    }).collect())
}

#[tauri::command]
pub async fn search_global(
    state: State<'_, RagState>,
    query: String,
    max_results: usize,
) -> Result<Vec<SearchResult>, String> {
    let rag_guard = state.rag.read().await;

    let all_results = rag_guard.search_comprehensive(&query, max_results * 2, None)
        .await
        .map_err(|e| e.to_string())?;

    let results: Vec<_> = all_results.into_iter()
        .filter(|r| {
            r.metadata.get("doc_type")
                .map(|dt| dt == "document")
                .unwrap_or(true)
        })
        .take(max_results)
        .collect();

    Ok(results.into_iter().map(|r| SearchResult {
        id: r.id.to_string(),
        score: r.score,
        snippet: r.snippet,
        metadata: r.metadata,
    }).collect())
}

#[tauri::command]
pub async fn remove_document(
    state: State<'_, RagState>,
    space_id: String,
    document_id: String,
) -> Result<(), String> {
    if let Ok(space_manager) = state.space_manager.lock() {
        let _ = space_manager.remove_document_from_space(&space_id, &document_id);
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_space_with_docs(
    state: State<'_, RagState>,
    space_id: String,
    delete_documents: bool,
) -> Result<(), String> {
    let mut rag_guard = state.rag.write().await;

    if delete_documents {
        match rag_guard.delete_by_space_id(&space_id).await {
            Ok(deleted) => {
                tracing::info!(space_id = %space_id, deleted = deleted, "Deleted space documents");
            }
            Err(e) => {
                tracing::error!(space_id = %space_id, error = %e, "Failed to delete space documents");
            }
        }
    }

    drop(rag_guard);

    if let Ok(space_manager) = state.space_manager.lock() {
        let _ = space_manager.delete_space(&space_id);
    }

    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_space_documents(
    state: State<'_, RagState>,
    space_id: String,
) -> Result<Vec<Document>, String> {
    let rag_guard = state.rag.read().await;

    let filter = shodh_rag::types::MetadataFilter {
        space_id: Some(space_id.clone()),
        ..Default::default()
    };
    let results = rag_guard.list_documents(Some(filter), 10000)
        .await
        .map_err(|e| format!("Failed to list documents: {}", e))?;

    let final_results: Vec<_> = results.into_iter()
        .filter(|r| {
            let is_document = r.metadata.get("doc_type")
                .map(|t| t == "document")
                .unwrap_or(true);
            let is_not_metadata = !r.metadata.get("doc_type")
                .map(|t| t == "space_metadata")
                .unwrap_or(false);
            is_document && is_not_metadata
        })
        .collect();

    Ok(final_results.into_iter().map(|r| {
        let title = r.metadata.get("title")
            .or_else(|| r.metadata.get("filename"))
            .or_else(|| r.metadata.get("file_name"))
            .cloned()
            .or_else(|| {
                r.metadata.get("file_path").map(|path| {
                    std::path::Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Untitled")
                        .to_string()
                })
            })
            .unwrap_or_else(|| "Untitled".to_string());

        Document {
            id: r.id.to_string(),
            title,
            content: r.snippet,
            metadata: r.metadata,
        }
    }).collect())
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub snippet: String,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct Document {
    pub id: String,
    pub title: String,
    pub content: String,
    pub metadata: HashMap<String, String>,
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_space_system_prompt(
    state: State<'_, RagState>,
    space_id: String,
    system_prompt: String,
) -> Result<(), String> {
    let space_manager = state.space_manager.lock()
        .map_err(|e| format!("Lock failed: {}", e))?;

    let prompt_value = system_prompt.trim().to_string();
    if prompt_value.is_empty() {
        space_manager.remove_space_metadata(&space_id, "system_prompt")
            .map_err(|e| format!("Failed to clear system prompt: {}", e))?;
    } else {
        space_manager.set_space_metadata(&space_id, "system_prompt", &prompt_value)
            .map_err(|e| format!("Failed to set system prompt: {}", e))?;
    }

    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_space_system_prompt(
    state: State<'_, RagState>,
    space_id: String,
) -> Result<String, String> {
    let space_manager = state.space_manager.lock()
        .map_err(|e| format!("Lock failed: {}", e))?;

    let prompt = space_manager.get_space_metadata(&space_id, "system_prompt")
        .unwrap_or_default();

    Ok(prompt)
}
