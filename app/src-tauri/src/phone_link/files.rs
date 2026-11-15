//! Files from a paired phone (Android's share sheet, Add in the Library, a
//! camera photo): received in parts that resume (`shodh_link::transfer`), then
//! added through the desktop's own upload path, so every upload rule applies
//! exactly as for a file dropped into a chat:
//!
//! - before a byte is sent ([`admit`]): the file name rule of chat uploads,
//!   files that may hold secrets ([`shodh_rag::indexing::secret_file_reason`]),
//!   file kinds Shodh cannot read, and the size limits of each kind
//!   ([`shodh_rag::processing::limits`]);
//! - when it is in: `upload_document_file`, the chat upload command itself,
//!   which reads legacy Office files in the guarded worker and says why a file
//!   was skipped. A file it did not add is removed again.
//!
//! A file goes to "From my phone" (a folder in Shodh's own data folder, shown
//! as a top-level folder of the phone's Library), searched in the main Ask, or
//! to a workspace the user chose, searched in that workspace's chats, as a
//! file added in one of its chats is. Activity records it as added from the
//! phone (the bridge records `phone_file_finish` with the phone's name).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use shodh_link::bridge::Caller;
use shodh_link::transfer::{Arrived, Offer, Part, Purpose, Staging};
use tauri::{AppHandle, Manager};

use crate::rag_commands::RagState;

/// "From my phone", in Shodh's data folder.
pub const INBOX_FOLDER: &str = "From my phone";

/// Parts of files on their way from phones.
const STAGING_FOLDER: &str = "phone_transfers";

/// The inbox in `data_dir`.
pub fn inbox_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(INBOX_FOLDER)
}

fn staging_in(data_dir: &Path) -> Staging {
    Staging::new(data_dir.join(STAGING_FOLDER))
}

/// Forget a transfer whose file was moved out of the staging folder.
pub(super) fn forget_transfer(data_dir: &Path, transfer_id: &str) {
    staging_in(data_dir).forget(transfer_id);
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, Value> {
    crate::profile::app_data_dir(app)
        .map_err(|e| failed(format!("The app data folder is unavailable: {e}")))
}

fn failed(message: impl Into<String>) -> Value {
    json!({ "code": "runtime_error", "message": message.into() })
}

fn refused(message: impl Into<String>) -> Value {
    shodh_link::bridge::refused(message)
}

fn invalid() -> Value {
    json!({ "code": "invalid_request", "message": "This file's details are not valid." })
}

/// The lowercase extension of `name`, without the dot.
fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "tiff", "tif", "gif", "webp"];

/// The upload rules a file from the phone meets before a byte of it is sent:
/// the same as for a file dropped into a chat. The reason is the one the
/// phone shows.
pub fn admit(offer: &Offer) -> Result<(), String> {
    let name = crate::document_upload_commands::upload_file_name(&offer.name)?;
    if let Some(reason) = shodh_rag::indexing::secret_file_reason(Path::new(name)) {
        return Err(shodh_rag::indexing::SecretFileKeptBack { reason }.to_string());
    }
    let ext = extension(name);
    if ext.is_empty() {
        return Err(
            "Shodh cannot read a file without an extension, so it was not sent.".to_string(),
        );
    }
    if !shodh_rag::indexing::is_supported_file_type(&ext) {
        return Err(format!(
            "Shodh cannot read .{ext} files, so this one was not sent."
        ));
    }
    if offer.purpose == Purpose::Photo && !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Err("Only a photo can be asked about this way.".to_string());
    }
    shodh_rag::processing::limits::check_file_size(&ext, offer.size).map_err(|e| e.to_string())
}

/// Where a file goes: the owner of chat uploads in the chosen workspace, or
/// of the main Ask for "From my phone". `None` for a workspace id that is not one.
pub fn destination(workspace_id: Option<&str>) -> Option<String> {
    shodh_rag::rag_engine::chat_uploads_source(workspace_id)
}

/// A name in `folder` that is free: `name`, else `name (2)`, `name (3)`…
pub fn free_name(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|e| e.to_str());
    (2..)
        .map(|n| match ext {
            Some(ext) => folder.join(format!("{stem} ({n}).{ext}")),
            None => folder.join(format!("{stem} ({n})")),
        })
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

/// Move an arrived file into `folder` under a free name.
pub fn place(arrived: &Arrived, folder: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let target = free_name(folder, &arrived.offer.name);
    std::fs::rename(&arrived.path, &target)?;
    Ok(target)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Value> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| failed(format!("Receiving the file stopped: {e}")))
}

pub(super) async fn workspace_exists(app: &AppHandle, id: &str) -> bool {
    let id = id.to_string();
    app.state::<crate::workspace_commands::WorkspaceState>()
        .run(move |s| s.get(&id))
        .await
        .is_ok()
}

/// `phone_file_begin`: the upload rules, then where the transfer starts.
pub async fn begin(app: &AppHandle, caller: &Caller, raw: Value) -> Result<Value, Value> {
    let offer: Offer = serde_json::from_value(raw).map_err(|_| invalid())?;
    admit(&offer).map_err(refused)?;
    if let Some(id) = offer.workspace_id.as_deref() {
        if destination(Some(id)).is_none() || !workspace_exists(app, id).await {
            return Err(refused(
                "That workspace is not on your computer any more. Choose another.",
            ));
        }
    }
    let dir = data_dir(app)?;
    let node = caller.node_id.clone();
    let now = super::now_ms();
    let begun = blocking(move || staging_in(&dir).begin(&node, &offer, now)).await?;
    begun
        .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
        .map_err(|e| e.to_value())
}

/// `phone_file_chunk`: one part, at the offset the computer has.
pub async fn chunk(app: &AppHandle, caller: &Caller, raw: Value) -> Result<Value, Value> {
    let part: Part = serde_json::from_value(raw).map_err(|_| invalid())?;
    let dir = data_dir(app)?;
    let node = caller.node_id.clone();
    let done = blocking(move || staging_in(&dir).chunk(&node, &part)).await?;
    done.map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
        .map_err(|e| e.to_value())
}

/// What the phone is told about a file it sent.
pub(super) fn added_answer(
    name: &str,
    path: &Path,
    result: &crate::document_upload_commands::UploadResult,
) -> Value {
    json!({
        "name": name,
        "path": path.display().to_string(),
        "added": result.success,
        "reason": result.error,
        "notice": result.notice,
    })
}

/// Remove `path` unless the index holds it (a failed indexing may have stored
/// part of it, or the index cannot say now: it is kept then).
async fn discard_unindexed(app: &AppHandle, path: &Path) {
    let state = app.state::<RagState>();
    if !*state.rag_initialized.read().await {
        return;
    }
    let indexed = match state.rag.read().await.is_indexed_source(path).await {
        Ok(indexed) => indexed,
        Err(_) => true,
    };
    if !indexed {
        if let Err(e) = std::fs::remove_file(path) {
            tracing::warn!(target: "shodh::link", error = %e, "a file from a phone that was not added could not be removed");
        }
    }
}

/// `phone_file_finish`: the file is complete and checked; add it through the
/// upload path, or (a photo) keep it for the question about it.
pub async fn finish(app: &AppHandle, caller: &Caller, raw: Value) -> Result<Value, Value> {
    let id = raw
        .get("transferId")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?
        .to_string();
    let dir = data_dir(app)?;
    let node = caller.node_id.clone();
    let staged = dir.clone();
    let arrived = blocking(move || staging_in(&staged).finish(&node, &id))
        .await?
        .map_err(|e| e.to_value())?;
    // The rules again: the same file is judged the same way as when it began.
    admit(&arrived.offer).map_err(|reason| {
        staging_in(&dir).forget(&arrived.transfer_id);
        refused(reason)
    })?;
    match arrived.offer.purpose {
        Purpose::Library => add_to_library(app, &dir, arrived).await,
        Purpose::Photo => super::photos::arrived(app, caller, &dir, arrived).await,
    }
}

async fn add_to_library(app: &AppHandle, dir: &Path, arrived: Arrived) -> Result<Value, Value> {
    let inbox = inbox_dir(dir);
    let staging_dir = dir.to_path_buf();
    let (arrived, placed) = blocking(move || {
        let placed = place(&arrived, &inbox);
        staging_in(&staging_dir).forget(&arrived.transfer_id);
        (arrived, placed)
    })
    .await?;
    let path = placed.map_err(|e| failed(format!("Your computer could not keep the file: {e}")))?;
    let added = add_path(app, &path, arrived.offer.workspace_id.clone()).await?;
    Ok(added_answer(&arrived.offer.name, &path, &added))
}

/// Add the file at `path` (in "From my phone") to the library through the
/// chat upload command, to `workspace_id` or the main Ask; removed again
/// when it was not added.
pub async fn add_path(
    app: &AppHandle,
    path: &Path,
    workspace_id: Option<String>,
) -> Result<crate::document_upload_commands::UploadResult, Value> {
    let result = crate::document_upload_commands::upload_document_file(
        app.state(),
        app.state(),
        path.display().to_string(),
        workspace_id,
    )
    .await
    .map_err(refused)?;
    if !result.success {
        discard_unindexed(app, path).await;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_link::transfer::{sha256_hex, Purpose};

    fn offer(name: &str, size: u64) -> Offer {
        Offer {
            name: name.into(),
            size,
            sha256: "0".repeat(64),
            workspace_id: None,
            purpose: Purpose::Library,
        }
    }

    #[test]
    fn a_secret_file_from_the_phone_is_blocked_like_any_upload() {
        for name in [
            ".env",
            "prod.env",
            "id_rsa",
            "server.pem",
            "credentials.json",
            ".npmrc",
        ] {
            let refused = admit(&offer(name, 10)).unwrap_err();
            let reason = shodh_rag::indexing::secret_file_reason(Path::new(name)).unwrap();
            assert_eq!(
                refused,
                shodh_rag::indexing::SecretFileKeptBack { reason }.to_string(),
                "{name} is kept back with the upload's own words"
            );
        }
        // A name a chat upload refuses is refused the same way.
        for name in ["../a.pdf", "a/b.pdf", "CON.txt", "report.pdf.", ""] {
            assert!(admit(&offer(name, 10)).is_err(), "{name}");
        }
        assert_eq!(admit(&offer("self-rag.pdf", 10)), Ok(()));
    }

    #[test]
    fn a_file_over_the_size_limit_is_refused_with_the_reason() {
        use shodh_rag::processing::limits::{MAX_TABLE_BYTES, MAX_TEXT_BYTES, MIB};
        let text = admit(&offer("notes.md", MAX_TEXT_BYTES + 1)).unwrap_err();
        assert!(
            text.contains("too large to index (limit 50 MB for text files)"),
            "{text}"
        );
        let table = admit(&offer("sales.xlsx", MAX_TABLE_BYTES + MIB)).unwrap_err();
        assert!(
            table.contains("too large to index (limit 100 MB for spreadsheets)"),
            "{table}"
        );
        assert_eq!(admit(&offer("notes.md", MAX_TEXT_BYTES)), Ok(()));
        // Kinds Shodh cannot read never start.
        assert!(admit(&offer("song.mp3", 10)).unwrap_err().contains(".mp3"));
        assert!(admit(&offer("README", 10))
            .unwrap_err()
            .contains("without an extension"));
        // A photo to ask about must be an image.
        let mut photo = offer("scan.pdf", 10);
        photo.purpose = Purpose::Photo;
        assert!(admit(&photo).is_err());
        photo.name = "board.jpg".into();
        assert_eq!(admit(&photo), Ok(()));
    }

    #[test]
    fn a_file_from_the_phone_lands_in_the_chosen_workspace() {
        // The inbox by default: the main Ask's uploads; a workspace's otherwise.
        assert_eq!(
            destination(None),
            shodh_rag::rag_engine::chat_uploads_source(None)
        );
        assert_eq!(
            destination(Some("w1")),
            shodh_rag::rag_engine::chat_uploads_source(Some("w1"))
        );
        assert_ne!(destination(None), destination(Some("w1")));
        assert_eq!(destination(Some("../etc")), None, "not a workspace id");

        // The file is kept in "From my phone" under its own name, never over another.
        let data = tempfile::tempdir().unwrap();
        let staging = staging_in(data.path());
        let bytes = b"%PDF-1.7 a scan".to_vec();
        let mut sent = offer("scan.pdf", bytes.len() as u64);
        sent.sha256 = sha256_hex(bytes.as_slice()).unwrap();
        sent.workspace_id = Some("w1".into());
        let arrive = || {
            let begun = staging.begin("phone", &sent, 0).unwrap();
            staging
                .chunk(
                    "phone",
                    &Part {
                        transfer_id: begun.transfer_id.clone(),
                        offset: 0,
                        data: base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            &bytes,
                        ),
                    },
                )
                .unwrap();
            let arrived = staging.finish("phone", &begun.transfer_id).unwrap();
            let placed = place(&arrived, &inbox_dir(data.path())).unwrap();
            staging.forget(&arrived.transfer_id);
            placed
        };
        let first = arrive();
        let second = arrive();
        assert_eq!(first, data.path().join("From my phone").join("scan.pdf"));
        assert_eq!(
            second,
            data.path().join("From my phone").join("scan (2).pdf")
        );
        assert_eq!(std::fs::read(&first).unwrap(), bytes);
    }
}
