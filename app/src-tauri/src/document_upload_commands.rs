//! Simple document upload commands with progress tracking
//! For drag-drop PDF/DOCX/XLSX into chat

use crate::rag_commands::RagState;
use serde::{Deserialize, Serialize};
use shodh_rag::comprehensive_system::Citation;
use shodh_rag::rag_engine::chat_uploads_source;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    pub success: bool,
    pub file_name: String,
    pub file_type: String,
    pub chunks_created: usize,
    pub file_size_mb: f64,
    pub processing_time_ms: u64,
    pub error: Option<String>,
}

/// Upload and index a document file (PDF, DOCX, XLSX, etc.) added in a
/// chat of `workspace_id` (`None`: the main Ask). It belongs to where it was
/// added ([`chat_uploads_source`]), not to a Library folder, so removing a
/// folder never removes it, and only that workspace's chats (or the main
/// Ask) search it (see `agent_session_commands::scope_for_workspace`).
#[tauri::command]
pub async fn upload_document_file(
    state: State<'_, RagState>,
    file_path: String,
    workspace_id: Option<String>,
) -> Result<UploadResult, String> {
    let workspace_id = workspace_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    let owner = chat_uploads_source(workspace_id.as_deref())
        .ok_or_else(|| "The chat's workspace id is not valid".to_string())?;
    let start_time = std::time::Instant::now();

    let path = Path::new(&file_path);

    // Get file metadata
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown")
        .to_string();

    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let file_size = std::fs::metadata(path)
        .map(|m| m.len() as f64 / 1_048_576.0) // Convert to MB
        .unwrap_or(0.0);

    // Determine file type
    let file_type = match extension.as_str() {
        "pdf" => "PDF",
        "docx" | "doc" => "Word Document",
        "xlsx" | "xls" => "Excel Spreadsheet",
        "pptx" | "ppt" => "PowerPoint",
        "txt" => "Text File",
        "md" => "Markdown",
        _ => "Document",
    };

    tracing::info!("📄 Uploading: {} ({:.2} MB)", file_name, file_size);

    // Prepare metadata
    let mut metadata = HashMap::new();
    metadata.insert("file_name".to_string(), file_name.clone());
    metadata.insert("file_type".to_string(), file_type.to_string());
    metadata.insert("file_size_mb".to_string(), format!("{:.2}", file_size));
    metadata.insert("uploaded_at".to_string(), chrono::Utc::now().to_rfc3339());

    metadata.insert("space_id".to_string(), owner);

    // Create citation
    let _citation = Citation {
        title: file_name.clone(),
        authors: vec![],
        source: file_path.clone(),
        year: chrono::Utc::now().format("%Y").to_string(),
        url: None,
        doi: None,
        page_numbers: None,
    };

    // Process the file
    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    match rag.add_document_from_file(path, metadata).await {
        Ok(chunk_ids) => {
            let processing_time = start_time.elapsed().as_millis() as u64;

            tracing::info!(
                "✅ Indexed {} chunks in {}ms",
                chunk_ids.len(),
                processing_time
            );

            Ok(UploadResult {
                success: true,
                file_name,
                file_type: file_type.to_string(),
                chunks_created: chunk_ids.len(),
                file_size_mb: file_size,
                processing_time_ms: processing_time,
                error: None,
            })
        }
        Err(e) => {
            let error_msg = e.to_string();
            tracing::info!("❌ Upload failed: {}", error_msg);

            Ok(UploadResult {
                success: false,
                file_name,
                file_type: file_type.to_string(),
                chunks_created: 0,
                file_size_mb: file_size,
                processing_time_ms: start_time.elapsed().as_millis() as u64,
                error: Some(error_msg),
            })
        }
    }
}

/// Folder under the app data folder that keeps the files dropped into a chat.
/// The index records a file's path as its source, so the file stays where it
/// was saved for as long as it is indexed: citations open it there.
const CHAT_UPLOADS_DIR: &str = "chat_uploads";

/// Longest file name kept (most file systems allow 255 bytes).
const MAX_UPLOAD_NAME_LEN: usize = 255;

/// Names Windows reserves for devices, with or without an extension.
const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The name a dropped file is kept under: exactly one plain path component.
/// A name with a separator, `..`, a drive or stream colon, a control
/// character or a device name is refused, so the file is always written
/// inside its own upload folder.
fn upload_file_name(file_name: &str) -> Result<&str, String> {
    let refused = || {
        Err(format!(
            "The file name \"{}\" cannot be used. Rename the file and drop it again.",
            file_name.escape_debug()
        ))
    };
    let name = file_name.trim();
    if name.is_empty() || name.len() > MAX_UPLOAD_NAME_LEN {
        return refused();
    }
    if name
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
    {
        return refused();
    }
    // Windows drops trailing dots and spaces: "report.pdf." would be
    // saved under another name than the one checked here.
    if name.trim_matches(|c| c == '.' || c == ' ').is_empty() || name.ends_with('.') {
        return refused();
    }
    if Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name) {
        return refused();
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if RESERVED_NAMES
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return refused();
    }
    Ok(name)
}

/// Write `bytes` as `<data_dir>/chat_uploads/<unique id>/<name>`, flushed to
/// disk. The file keeps its own name (the name citations show); the unique
/// folder keeps two drops of one name apart. Nothing is left behind when the
/// write fails.
fn store_upload(data_dir: &Path, name: &str, bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    let folder = data_dir
        .join(CHAT_UPLOADS_DIR)
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&folder)
        .map_err(|e| format!("The upload folder could not be created: {e}"))?;
    let path = folder.join(name);
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(e) = written {
        // Not indexed yet: nothing refers to the folder.
        let _ = fs::remove_dir_all(&folder);
        return Err(format!("The dropped file could not be saved: {e}"));
    }
    Ok(path)
}

/// Keep a file dropped into a chat in the app data folder and return its
/// path, which the index then records as the file's source.
#[tauri::command]
pub async fn save_temp_file(
    app: tauri::AppHandle,
    file_name: String,
    file_data: Vec<u8>,
) -> Result<String, String> {
    let name = upload_file_name(&file_name)?.to_string();
    let data_dir = crate::profile::app_data_dir(&app)
        .map_err(|e| format!("The app data folder is unavailable: {e}"))?;
    let path = tokio::task::spawn_blocking(move || store_upload(&data_dir, &name, &file_data))
        .await
        .map_err(|e| format!("Saving the dropped file failed: {e}"))??;
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| "The saved file's path is not valid text".to_string())
}

/// The upload folder holding `path` when `path` is exactly a file saved by
/// [`store_upload`]: `<data_dir>/chat_uploads/<unique id>/<name>`, compared
/// after resolving links and `..`. `Ok(None)` when it is no longer there;
/// an error for any other path, which is never removed.
fn upload_folder_of(data_dir: &Path, path: &Path) -> Result<Option<std::path::PathBuf>, String> {
    let not_an_upload = || Err("Only a file dropped into a chat can be discarded.".to_string());
    let uploads = match fs::canonicalize(data_dir.join(CHAT_UPLOADS_DIR)) {
        Ok(dir) => dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("The upload folder could not be read: {e}")),
    };
    let file = match fs::canonicalize(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("The dropped file could not be read: {e}")),
    };
    let Some(folder) = file.parent() else {
        return not_an_upload();
    };
    let unique = folder
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| uuid::Uuid::parse_str(n).is_ok());
    if folder.parent() != Some(uploads.as_path()) || !unique || !file.is_file() {
        return not_an_upload();
    }
    Ok(Some(folder.to_path_buf()))
}

/// Remove a file dropped into a chat whose indexing failed or was refused,
/// with its upload folder, so it does not stay in the app data folder
/// forever. Only a file under `chat_uploads/<unique id>/` is removed, and
/// never one the index still holds (a failed indexing may have stored part
/// of it, or the index cannot be asked right now): it is kept then.
/// Returns whether it was removed.
#[tauri::command]
pub async fn discard_upload(
    app: tauri::AppHandle,
    state: State<'_, RagState>,
    file_path: String,
) -> Result<bool, String> {
    let data_dir = crate::profile::app_data_dir(&app)
        .map_err(|e| format!("The app data folder is unavailable: {e}"))?;
    let requested = std::path::PathBuf::from(file_path.trim());
    let Some(folder) = upload_folder_of(&data_dir, &requested)? else {
        return Ok(false);
    };
    if !*state.rag_initialized.read().await {
        return Ok(false);
    }
    let canonical = folder.join(requested.file_name().unwrap_or_default());
    let indexed = {
        let Ok(rag) = tokio::time::timeout(DISCARD_INDEX_WAIT, state.rag.read()).await else {
            // Indexing holds the index: it cannot say the file is not in it.
            return Ok(false);
        };
        let mut indexed = false;
        for candidate in [
            requested.clone(),
            canonical,
            without_verbatim_prefix(&folder, &requested),
        ] {
            match rag.is_indexed_source(&candidate).await {
                Ok(false) => {}
                Ok(true) | Err(_) => {
                    indexed = true;
                    break;
                }
            }
        }
        indexed
    };
    if indexed {
        tracing::info!("A failed upload is kept: the index still holds it");
        return Ok(false);
    }
    tokio::task::spawn_blocking(move || fs::remove_dir_all(&folder))
        .await
        .map_err(|e| format!("Removing the dropped file failed: {e}"))?
        .map_err(|e| format!("The dropped file could not be removed: {e}"))?;
    Ok(true)
}

/// Longest wait for the index before a discard gives up (and keeps the file).
const DISCARD_INDEX_WAIT: std::time::Duration = std::time::Duration::from_secs(4);

/// `folder/<name of requested>` without Windows' `\\?\` prefix, the
/// spelling the indexer may have recorded.
fn without_verbatim_prefix(folder: &Path, requested: &Path) -> std::path::PathBuf {
    let joined = folder.join(requested.file_name().unwrap_or_default());
    let text = joined.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => std::path::PathBuf::from(rest),
        None => joined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_file_name_is_one_plain_component() {
        for ok in [
            "paper.pdf",
            "My notes (v2).docx",
            "résumé.pdf",
            "a..b.pdf",
            ".hidden.md",
            "console.txt",
        ] {
            assert_eq!(upload_file_name(ok), Ok(ok), "{ok}");
        }
        assert_eq!(upload_file_name("  paper.pdf "), Ok("paper.pdf"));
        for refused in [
            "",
            "   ",
            ".",
            "..",
            "...",
            "../evil.pdf",
            "..\\evil.pdf",
            "a/b.pdf",
            "a\\b.pdf",
            "/etc/passwd",
            "C:\\Windows\\win.ini",
            "C:evil.pdf",
            "paper.pdf:stream",
            "line\nbreak.pdf",
            "nul\0.pdf",
            "CON",
            "con.pdf",
            "Lpt1.txt",
            "NUL .pdf",
            "report.pdf.",
            "report.pdf. .",
        ] {
            assert!(upload_file_name(refused).is_err(), "{refused:?}");
        }
        assert!(upload_file_name(&"a".repeat(MAX_UPLOAD_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn only_a_saved_upload_can_be_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let upload = store_upload(dir.path(), "paper.pdf", b"x").unwrap();
        let folder = upload_folder_of(dir.path(), &upload).unwrap().unwrap();
        assert_eq!(
            fs::canonicalize(upload.parent().unwrap()).unwrap(),
            folder,
            "its own unique folder"
        );

        // Anything else is refused, never removed.
        let settings = dir.path().join("app_settings.json");
        fs::write(&settings, "{}").unwrap();
        assert!(upload_folder_of(dir.path(), &settings).is_err());
        let loose = dir.path().join(CHAT_UPLOADS_DIR).join("loose.pdf");
        fs::write(&loose, "x").unwrap();
        assert!(
            upload_folder_of(dir.path(), &loose).is_err(),
            "not in a unique folder"
        );
        let odd = dir.path().join(CHAT_UPLOADS_DIR).join("not-a-uuid");
        fs::create_dir_all(&odd).unwrap();
        fs::write(odd.join("a.pdf"), "x").unwrap();
        assert!(upload_folder_of(dir.path(), &odd.join("a.pdf")).is_err());
        let escape = upload
            .parent()
            .unwrap()
            .join("..")
            .join("..")
            .join("app_settings.json");
        assert!(
            upload_folder_of(dir.path(), &escape).is_err(),
            "resolved before the check"
        );
        assert!(
            upload_folder_of(dir.path(), upload.parent().unwrap()).is_err(),
            "a folder"
        );
        // Gone already: nothing to do.
        assert_eq!(
            upload_folder_of(dir.path(), &upload.with_file_name("gone.pdf")).unwrap(),
            None
        );
        assert!(settings.exists() && loose.exists());
    }

    #[test]
    fn uploads_stay_in_their_own_folder_under_the_app_data() {
        let dir = tempfile::tempdir().unwrap();
        let uploads = dir.path().join(CHAT_UPLOADS_DIR);
        let first = store_upload(dir.path(), "paper.pdf", b"one").unwrap();
        let second = store_upload(dir.path(), "paper.pdf", b"two").unwrap();
        assert_ne!(first, second, "two drops of one name are both kept");
        for (path, bytes) in [(&first, b"one"), (&second, b"two")] {
            assert_eq!(path.file_name().unwrap(), "paper.pdf");
            assert_eq!(path.parent().unwrap().parent().unwrap(), uploads);
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}
