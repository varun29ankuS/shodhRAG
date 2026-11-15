//! Files from the phone to the computer: what the user picked (Add in the
//! Library), photographed, or shared to Shodh from another app. Android's side
//! (the Kotlin plugin) copies each into the app's cache (`outgoing/`) at once;
//! from there the outbox keeps it until the computer has it, and it is sent
//! in parts that resume (`shodh_link::transfer`). The computer decides: it
//! applies every upload rule before a byte is sent.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shodh_link::outbox::QueuedFile;
use shodh_link::transfer::{self, Offer, Peer, Purpose};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_shodh_android::Native;

use crate::link::{blocking, Link};
use crate::store::Store;

/// A file's progress on its way (payload: [`Progress`]).
pub const PROGRESS_EVENT: &str = "transfer://progress";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    local_path: String,
    received: u64,
    size: u64,
}

/// A file Android copied into the app's cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picked {
    pub path: String,
    pub name: String,
}

/// The link, as the transfer's sender reaches the computer.
struct LinkPeer<'a>(&'a Link);

impl Peer for LinkPeer<'_> {
    fn call(
        &self,
        command: &str,
        args: Value,
    ) -> impl std::future::Future<Output = Result<Value, Value>> + Send {
        self.0.invoke(command, args)
    }
}

/// `path`, refused unless it is a copy in the app's own `outgoing/` folder.
fn own_copy(cache: &Path, path: &str) -> Result<PathBuf, String> {
    let outgoing = cache.join("outgoing");
    let path = PathBuf::from(path);
    let plain = path.is_absolute()
        && !path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        });
    if !plain || !path.starts_with(&outgoing) || !path.is_file() {
        return Err("Only a copy Shodh made on this phone can be sent.".to_string());
    }
    Ok(path)
}

/// Remove a copy and the folder it was made in.
fn remove_copy(path: &Path) {
    if let Some(folder) = path.parent() {
        let _ = std::fs::remove_dir_all(folder);
    }
}

fn native(app: &AppHandle) -> State<'_, Native<tauri::Wry>> {
    app.state::<Native<tauri::Wry>>()
}

/// Add in the Library: Android's file picker; the copies of the chosen files.
#[tauri::command]
pub async fn files_pick(app: AppHandle) -> Result<Vec<Picked>, String> {
    let reply: Value = native(&app).call("pickFiles", json!({})).await?;
    serde_json::from_value(reply["files"].clone()).map_err(|e| e.to_string())
}

/// The camera: the photo taken, or `None` when the user went back.
#[tauri::command]
pub async fn photo_take(app: AppHandle) -> Result<Option<Picked>, String> {
    let reply: Value = native(&app).call("takePhoto", json!({})).await?;
    if reply.get("path").is_none() {
        return Ok(None);
    }
    serde_json::from_value(reply)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// What was shared to Shodh from another app, once (`{ kind: "none" }` when nothing).
#[tauri::command]
pub async fn incoming_take(app: AppHandle) -> Result<Value, String> {
    native(&app).call("takeIncoming", json!({})).await
}

/// The details the outbox keeps of a copy: its size and SHA-256.
#[tauri::command]
pub async fn file_details(
    app: AppHandle,
    path: String,
    name: String,
    workspace_id: Option<String>,
    store: State<'_, Store>,
) -> Result<QueuedFile, String> {
    let storage = store.get(&app).await?;
    let path = own_copy(storage.cache(), &path)?;
    blocking(move || {
        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let sha256 =
            transfer::sha256_hex(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
        Ok(QueuedFile {
            local_path: path.display().to_string(),
            name,
            size,
            sha256,
            workspace_id,
        })
    })
    .await
}

/// The user chose not to send a copy.
#[tauri::command]
pub async fn file_discard(
    app: AppHandle,
    path: String,
    store: State<'_, Store>,
) -> Result<(), String> {
    let storage = store.get(&app).await?;
    let path = own_copy(storage.cache(), &path)?;
    remove_copy(&path);
    Ok(())
}

async fn send(
    app: &AppHandle,
    link: &Link,
    file: &QueuedFile,
    purpose: Purpose,
) -> Result<Value, Value> {
    let storage = app
        .state::<Store>()
        .get(app)
        .await
        .map_err(|message| json!({ "code": "runtime_error", "message": message }))?;
    let path = own_copy(storage.cache(), &file.local_path)
        .map_err(|message| json!({ "code": "refused", "message": message }))?;
    let offer = Offer {
        name: file.name.clone(),
        size: file.size,
        sha256: file.sha256.clone(),
        workspace_id: file.workspace_id.clone(),
        purpose,
    };
    let local = file.local_path.clone();
    let size = file.size;
    let emitter = app.clone();
    transfer::send_file(&LinkPeer(link), &path, &offer, move |received| {
        let _ = emitter.emit(
            PROGRESS_EVENT,
            Progress {
                local_path: local.clone(),
                received,
                size,
            },
        );
    })
    .await
}

/// Send a queued file to the library ("From my phone" or its workspace). The
/// computer's answer says whether it was added, or why it was skipped; the
/// copy goes once the computer has the file.
#[tauri::command]
pub async fn file_send(
    app: AppHandle,
    file: QueuedFile,
    link: State<'_, Link>,
) -> Result<Value, Value> {
    let answer = send(&app, &link, &file, Purpose::Library).await?;
    remove_copy(Path::new(&file.local_path));
    Ok(answer)
}

/// Send a photo to ask about: the computer reads it and keeps it apart from
/// the library. The copy stays on the phone for the answer to show.
#[tauri::command]
pub async fn photo_send(
    app: AppHandle,
    file: QueuedFile,
    link: State<'_, Link>,
) -> Result<Value, Value> {
    send(&app, &link, &file, Purpose::Photo).await
}
