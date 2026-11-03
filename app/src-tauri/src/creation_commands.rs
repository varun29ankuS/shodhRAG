//! The Artifacts tab's commands for Creator mode's creations
//! ([`crate::creations`]).
//!
//! Every command names a creation by its id and a file by a path relative
//! to its folder: none takes an absolute path. Keep, Export and Delete are
//! the user's own decisions (no agent tool does them); the agent opens
//! creations with `open_artifact` and lists them with `list_artifacts`.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::agent_session_commands::AgentSessions;
use crate::agent_tools::export::EXPORTS_FOLDER;
use crate::creations::preview::{self, Grant, PREVIEWS};
use crate::creations::{
    creations_root, valid_id, Creation, CreationStore, FileChange, FileContent, FileEntry,
    MAX_TITLE_CHARS,
};

/// The event the Artifacts tab refreshes on.
pub const CREATIONS_CHANGED: &str = "creations-changed";

/// Longest request text a creation is named from.
const MAX_REQUEST_CHARS: usize = 2_000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreationError {
    message: String,
}

impl From<String> for CreationError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

impl From<&str> for CreationError {
    fn from(message: &str) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

type CreationResult<T> = Result<T, CreationError>;

fn data_dir(app: &AppHandle) -> CreationResult<std::path::PathBuf> {
    crate::profile::app_data_dir(app)
        .map_err(|e| CreationError::from(format!("Shodh's data folder is unavailable ({e})")))
}

fn documents(app: &AppHandle) -> CreationResult<std::path::PathBuf> {
    app.path()
        .document_dir()
        .map_err(|e| CreationError::from(format!("The Documents folder is unavailable ({e})")))
}

fn store(app: &AppHandle) -> CreationResult<CreationStore> {
    Ok(CreationStore::in_dir(&data_dir(app)?))
}

/// The resolved `Shodh Creations` folder.
pub(crate) fn root(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let documents = app
        .path()
        .document_dir()
        .map_err(|e| format!("The Documents folder is unavailable ({e})"))?;
    creations_root(&documents)
}

fn checked_id(id: &str) -> CreationResult<&str> {
    let id = id.trim();
    if valid_id(id) {
        Ok(id)
    } else {
        Err("That is not a creation id.".into())
    }
}

fn checked_conversation(conversation_id: &str) -> CreationResult<&str> {
    let id = conversation_id.trim();
    let ok = !id.is_empty()
        && id.len() <= 200
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'));
    if ok {
        Ok(id)
    } else {
        Err("That is not a conversation id.".into())
    }
}

/// Tell the Artifacts tab a creation of `conversation_id` changed.
pub(crate) fn changed(app: &AppHandle, conversation_id: &str, creation_id: &str) {
    if let Err(e) = app.emit(
        CREATIONS_CHANGED,
        serde_json::json!({ "conversationId": conversation_id, "creationId": creation_id }),
    ) {
        tracing::warn!(error = %e, "the creations-changed event could not be sent");
    }
}

/// The creation of `conversation_id`, made now (with a fresh folder named from
/// `request`) when it has none. Creator mode's first request calls this
/// before the answer starts.
#[tauri::command]
pub async fn creations_prepare(
    app: AppHandle,
    conversation_id: String,
    request: String,
) -> CreationResult<Creation> {
    let conversation_id = checked_conversation(&conversation_id)?.to_string();
    let request: String = request.chars().take(MAX_REQUEST_CHARS).collect();
    // No folder is made for an answer that could not run in it.
    if !crate::app_settings::code_mode_enabled(&data_dir(&app)?) {
        return Err(crate::agent_session_commands::CREATOR_NEEDS_CODE_MODE.into());
    }
    let store = store(&app)?;
    let documents = documents(&app)?;
    let conversation = conversation_id.clone();
    let creation = tauri::async_runtime::spawn_blocking(move || {
        let root = creations_root(&documents)?;
        store.prepare(
            &root,
            &conversation,
            &request,
            &chrono::Utc::now().to_rfc3339(),
        )
    })
    .await
    .map_err(|e| CreationError::from(format!("Preparing the creation did not finish ({e})")))??;
    changed(&app, &conversation_id, &creation.id);
    Ok(creation)
}

/// Every creation, the most recently changed first; with `text`, those whose
/// title, summary or choices contain it (any case).
#[tauri::command]
pub async fn creations_list(app: AppHandle, text: Option<String>) -> CreationResult<Vec<Creation>> {
    let store = store(&app)?;
    let all = tauri::async_runtime::spawn_blocking(move || store.list())
        .await
        .map_err(|e| CreationError::from(format!("Listing creations did not finish ({e})")))??;
    let needle = text
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty() && t.chars().count() <= MAX_TITLE_CHARS * 4);
    Ok(match needle {
        None => all,
        Some(needle) => all
            .into_iter()
            .filter(|c| {
                c.title.to_lowercase().contains(&needle)
                    || c.summary
                        .as_deref()
                        .is_some_and(|s| s.to_lowercase().contains(&needle))
                    || c.choices.iter().any(|x| x.to_lowercase().contains(&needle))
            })
            .collect(),
    })
}

/// One creation.
#[tauri::command]
pub async fn creation_get(app: AppHandle, id: String) -> CreationResult<Creation> {
    let id = checked_id(&id)?.to_string();
    let store = store(&app)?;
    tauri::async_runtime::spawn_blocking(move || store.get(&id))
        .await
        .map_err(|e| CreationError::from(format!("Reading the creation did not finish ({e})")))??
        .ok_or_else(|| "That creation no longer exists.".into())
}

/// The creation of `conversation_id`, if it has one.
#[tauri::command]
pub async fn creation_for_conversation(
    app: AppHandle,
    conversation_id: String,
) -> CreationResult<Option<Creation>> {
    let conversation_id = checked_conversation(&conversation_id)?.to_string();
    let store = store(&app)?;
    Ok(
        tauri::async_runtime::spawn_blocking(move || store.for_conversation(&conversation_id))
            .await
            .map_err(|e| {
                CreationError::from(format!("Reading the creation did not finish ({e})"))
            })??,
    )
}

/// The files of a version, or of the folder as it is now (`version` absent).
#[tauri::command]
pub async fn creation_files(
    app: AppHandle,
    id: String,
    version: Option<u32>,
) -> CreationResult<Vec<FileEntry>> {
    let id = checked_id(&id)?.to_string();
    let store = store(&app)?;
    let root = root(&app)?;
    Ok(
        tauri::async_runtime::spawn_blocking(move || store.files(&root, &id, version))
            .await
            .map_err(|e| {
                CreationError::from(format!("Listing the files did not finish ({e})"))
            })??,
    )
}

/// One file of a version, or of the folder now, as text when it is text.
#[tauri::command]
pub async fn creation_file(
    app: AppHandle,
    id: String,
    version: Option<u32>,
    path: String,
) -> CreationResult<FileContent> {
    let id = checked_id(&id)?.to_string();
    let store = store(&app)?;
    let root = root(&app)?;
    Ok(
        tauri::async_runtime::spawn_blocking(move || store.file(&root, &id, version, &path))
            .await
            .map_err(|e| CreationError::from(format!("Reading the file did not finish ({e})")))??,
    )
}

/// How version `to` differs from version `from` (0: from nothing).
#[tauri::command]
pub async fn creation_diff(
    app: AppHandle,
    id: String,
    from: u32,
    to: u32,
) -> CreationResult<Vec<FileChange>> {
    let id = checked_id(&id)?.to_string();
    let store = store(&app)?;
    Ok(
        tauri::async_runtime::spawn_blocking(move || store.diff(&id, from, to))
            .await
            .map_err(|e| CreationError::from(format!("Comparing did not finish ({e})")))??,
    )
}

/// A preview of a version, or of the folder now: the URL its sandboxed frame
/// loads ([`preview`]). `network` lets this preview reach the internet; only
/// the user's switch sets it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub url: String,
    pub network: bool,
    /// The page the preview opens.
    pub page: &'static str,
}

#[tauri::command]
pub async fn creation_preview(
    app: AppHandle,
    id: String,
    version: Option<u32>,
    network: bool,
) -> CreationResult<PreviewInfo> {
    let id = checked_id(&id)?.to_string();
    let data_dir = data_dir(&app)?;
    let root = root(&app)?;
    let store = CreationStore::in_dir(&data_dir);
    let creation = {
        let id = id.clone();
        tauri::async_runtime::spawn_blocking(move || store.get(&id))
            .await
            .map_err(|e| {
                CreationError::from(format!("Reading the creation did not finish ({e})"))
            })??
            .ok_or_else(|| CreationError::from("That creation no longer exists."))?
    };
    if let Some(number) = version {
        if !creation.versions.iter().any(|v| v.number == number) {
            return Err(format!("{} has no version {number}.", creation.title).into());
        }
    }
    let (_, url) = PREVIEWS.grant(Grant {
        creation_id: id,
        version,
        network,
        data_dir,
        root,
    });
    Ok(PreviewInfo {
        url,
        network,
        page: "index.html",
    })
}

/// Keep: the creation stays, marked kept.
#[tauri::command]
pub async fn creation_keep(app: AppHandle, id: String) -> CreationResult<Creation> {
    let id = checked_id(&id)?.to_string();
    let store = store(&app)?;
    let creation = tauri::async_runtime::spawn_blocking(move || {
        store.keep(&id, &chrono::Utc::now().to_rfc3339())
    })
    .await
    .map_err(|e| CreationError::from(format!("Keeping the creation did not finish ({e})")))??;
    changed(&app, &creation.conversation_id, &creation.id);
    Ok(creation)
}

/// Export a version (the latest when absent) as a zip file in
/// `Documents/Shodh exports`. Returns the file's path.
#[tauri::command]
pub async fn creation_export(
    app: AppHandle,
    id: String,
    version: Option<u32>,
) -> CreationResult<String> {
    let id = checked_id(&id)?.to_string();
    let store = store(&app)?;
    let folder = documents(&app)?.join(EXPORTS_FOLDER);
    let path = tauri::async_runtime::spawn_blocking(move || store.export(&id, version, &folder))
        .await
        .map_err(|e| CreationError::from(format!("The export did not finish ({e})")))??;
    Ok(path.display().to_string())
}

/// Delete it: the creation folder, its saved versions and its record. Only
/// while no answer of its conversation runs; the conversation's idle session
/// (working in that folder) is stopped first. The UI asks the user to confirm.
#[tauri::command]
pub async fn creation_delete(
    app: AppHandle,
    id: String,
    sessions: State<'_, AgentSessions>,
) -> CreationResult<()> {
    let id = checked_id(&id)?.to_string();
    let store = store(&app)?;
    let creation = {
        let store = store.clone();
        let id = id.clone();
        tauri::async_runtime::spawn_blocking(move || store.get(&id))
            .await
            .map_err(|e| {
                CreationError::from(format!("Reading the creation did not finish ({e})"))
            })??
            .ok_or_else(|| CreationError::from("That creation no longer exists."))?
    };
    if sessions.answering(&creation.conversation_id) {
        return Err("Stop the answer first: it is still building in this folder.".into());
    }
    sessions.close_conversation(&creation.conversation_id).await;
    let root = root(&app)?;
    PREVIEWS.revoke(&id);
    let removed = tauri::async_runtime::spawn_blocking(move || store.delete(&root, &id))
        .await
        .map_err(|e| CreationError::from(format!("Deleting did not finish ({e})")))??;
    changed(&app, &removed.conversation_id, &removed.id);
    Ok(())
}

/// After an answer of a Creator conversation: save its folder as a version
/// (off the async workers) and tell the Artifacts tab.
pub(crate) async fn snapshot_after_run(app: AppHandle, conversation_id: String, run_id: String) {
    let (store, root) = match (store(&app), root(&app)) {
        (Ok(store), Ok(root)) => (store, root),
        (Err(e), _) => {
            tracing::warn!(error = %e.message, "no version saved: the data folder is unavailable");
            return;
        }
        (_, Err(e)) => {
            tracing::warn!(error = %e, "no version saved: the creations folder is unavailable");
            return;
        }
    };
    let conversation = conversation_id.clone();
    let saved = tauri::async_runtime::spawn_blocking(move || {
        let snapshot = store.snapshot(
            &root,
            &conversation,
            Some(&run_id),
            &chrono::Utc::now().to_rfc3339(),
        );
        (snapshot, store.for_conversation(&conversation))
    })
    .await;
    match saved {
        Ok((Ok(_), Ok(Some(creation)))) => changed(&app, &conversation_id, &creation.id),
        Ok((Ok(_), _)) => {}
        Ok((Err(e), _)) => tracing::warn!(error = %e, "a creation's version could not be saved"),
        Err(e) => tracing::warn!(error = %e, "saving a creation's version did not finish"),
    }
}

/// The agent's `finish_creation` (its summary and choices) is kept with the
/// creation, for the finish card and the Artifacts tab.
pub(crate) async fn record_finish(
    app: AppHandle,
    conversation_id: String,
    summary: String,
    choices: Vec<String>,
) {
    let Ok(store) = store(&app) else {
        return;
    };
    let conversation = conversation_id.clone();
    let saved = tauri::async_runtime::spawn_blocking(move || {
        store.record_finish(
            &conversation,
            &summary,
            choices,
            &chrono::Utc::now().to_rfc3339(),
        )?;
        store.for_conversation(&conversation)
    })
    .await;
    match saved {
        Ok(Ok(Some(creation))) => changed(&app, &conversation_id, &creation.id),
        Ok(Ok(None)) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "a creation's finish could not be saved"),
        Err(e) => tracing::warn!(error = %e, "saving a creation's finish did not finish"),
    }
}

/// The scheme handler `lib.rs` registers for [`preview::SCHEME`].
pub fn preview_protocol<R: tauri::Runtime>(
    context: tauri::UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let webview = context.webview_label().to_string();
    // File reads happen off the webview's thread.
    tauri::async_runtime::spawn_blocking(move || {
        responder.respond(preview::response(&webview, &request));
    });
}
