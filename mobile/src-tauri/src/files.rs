//! Files from the phone to the computer: what the user picked (Add in the
//! Library), photographed, or shared to Shodh from another app. The phone's
//! side (the Kotlin or the Swift plugin) copies each into the app's cache
//! (`outgoing/`) at once; from there the outbox keeps it until the computer
//! has it, and it is sent in parts that resume (`shodh_link::transfer`). The
//! computer decides: it applies every upload rule before a byte is sent.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shodh_link::outbox::QueuedFile;
use shodh_link::transfer::{self, Offer, Peer, Purpose};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::link::{blocking, Link};
use crate::native::Native;
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

/// A file the phone's side copied into the app's cache.
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

/// What the plugin is told when it is about to copy files. Android's side
/// finds the app's cache itself (its context's `cacheDir`, the folder
/// [`own_copy`] checks).
#[cfg(target_os = "android")]
async fn copies_go(_app: &AppHandle) -> Result<Value, String> {
    Ok(json!({}))
}

/// What the plugin is told when it is about to copy files. iOS has no one
/// cache folder an app and Tauri agree on by themselves, so the Swift side
/// is given the folder [`own_copy`] checks, and makes every copy there.
#[cfg(target_os = "ios")]
async fn copies_go(app: &AppHandle) -> Result<Value, String> {
    let storage = app.state::<Store>().get(app).await?;
    let folder = storage.cache().join("outgoing");
    Ok(json!({ "folder": folder.display().to_string() }))
}

/// Add in the Library: the phone's file picker; the copies of the chosen files.
#[tauri::command]
pub async fn files_pick(app: AppHandle) -> Result<Vec<Picked>, String> {
    let reply: Value = native(&app)
        .call("pickFiles", copies_go(&app).await?)
        .await?;
    serde_json::from_value(reply["files"].clone()).map_err(|e| e.to_string())
}

/// The camera: the photo taken, or `None` when the user went back.
#[tauri::command]
pub async fn photo_take(app: AppHandle) -> Result<Option<Picked>, String> {
    let reply: Value = native(&app)
        .call("takePhoto", copies_go(&app).await?)
        .await?;
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

/// The most links iOS hands over at once that are looked at (a share of
/// more files than the Send sheet takes is cut to its limit by the plugin).
#[cfg(target_os = "ios")]
const MAX_OPENED: usize = 64;

/// iOS opened the app with something: files from the share sheet or the
/// Files app (each a `file://` link to iOS's copy in the app's Inbox), or
/// "Open in Shodh" on one of Shodh's events in the phone's calendar
/// (`shodh://event/<id>`). The Swift side copies the files into `outgoing/`
/// and keeps what arrived until the window takes it ([`incoming_take`]), as
/// Android's share activity does.
#[cfg(target_os = "ios")]
pub fn opened(app: &AppHandle, urls: Vec<tauri::Url>) {
    let urls: Vec<String> = urls
        .into_iter()
        .take(MAX_OPENED)
        .map(String::from)
        .collect();
    if urls.is_empty() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let delivered = async {
            let mut payload = copies_go(&app).await?;
            payload["urls"] = json!(urls);
            native(&app).call::<Value>("deliverOpened", payload).await
        };
        if let Err(e) = delivered.await {
            tracing::warn!("What was opened with Shodh could not be read: {e}");
        }
    });
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
