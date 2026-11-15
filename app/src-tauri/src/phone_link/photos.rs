//! "Point the camera and ask": a photo of a printed page, a whiteboard or a
//! slide, sent from the phone (`shodh_link::transfer`, purpose photo), read on
//! this computer by the indexer's own OCR, and asked about. The answer cites
//! the photo's text as "Your photo" (the phone sends it as a passage of the
//! question) alongside the library.
//!
//! The photo is kept apart from the library, in `phone_photos`, which is not
//! a linked folder and is never indexed, for a day at most. Only "Keep in
//! library" (`phone_photo_keep`) moves it to "From my phone" and adds it
//! through the upload path ([`super::files::add_path`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shodh_link::bridge::Caller;
use shodh_link::transfer::Arrived;
use tauri::{AppHandle, Manager};

use super::files;
use crate::rag_commands::RagState;

/// Photos asked about, in Shodh's data folder.
pub const PHOTO_FOLDER: &str = "phone_photos";

/// How long a photo asked about is kept for "Keep in library".
pub const KEEP_MS: u64 = 24 * 60 * 60 * 1000;

/// The most characters of a photo's text sent back to the phone (and on to
/// the question as its passage).
pub const MAX_PHOTO_TEXT_CHARS: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhotoMeta {
    name: String,
    node_id: String,
    at_ms: u64,
}

fn photos_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(PHOTO_FOLDER)
}

fn is_photo_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn meta_path(dir: &Path, id: &str) -> PathBuf {
    photos_dir(dir).join(format!("{id}.json"))
}

fn image_path(dir: &Path, id: &str, name: &str) -> PathBuf {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpg")
        .to_ascii_lowercase();
    photos_dir(dir).join(format!("{id}.{ext}"))
}

/// Remove photos asked about more than [`KEEP_MS`] ago.
fn sweep(dir: &Path, now_ms: u64) {
    let Ok(entries) = std::fs::read_dir(photos_dir(dir)) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        let Some(meta) = read_meta(dir, id) else {
            continue;
        };
        if now_ms.saturating_sub(meta.at_ms) > KEEP_MS {
            let _ = std::fs::remove_file(image_path(dir, id, &meta.name));
            let _ = std::fs::remove_file(meta_path(dir, id));
        }
    }
}

fn read_meta(dir: &Path, id: &str) -> Option<PhotoMeta> {
    serde_json::from_slice(&std::fs::read(meta_path(dir, id)).ok()?).ok()
}

/// Keep an arrived photo apart from the library, under its transfer's id.
pub fn store(
    dir: &Path,
    node_id: &str,
    arrived: &Arrived,
    now_ms: u64,
) -> std::io::Result<PathBuf> {
    sweep(dir, now_ms);
    std::fs::create_dir_all(photos_dir(dir))?;
    let path = image_path(dir, &arrived.transfer_id, &arrived.offer.name);
    std::fs::rename(&arrived.path, &path)?;
    let meta = PhotoMeta {
        name: arrived.offer.name.clone(),
        node_id: node_id.to_string(),
        at_ms: now_ms,
    };
    let bytes = serde_json::to_vec(&meta).map_err(std::io::Error::other)?;
    crate::durable_file::write_atomic(&meta_path(dir, &arrived.transfer_id), &bytes)?;
    Ok(path)
}

/// Move photo `id` of `node_id` into "From my phone", under its own name,
/// for the library. Another phone's photo, or one no longer kept, is refused.
pub fn take_for_keeping(dir: &Path, node_id: &str, id: &str) -> Result<PathBuf, String> {
    let gone = || "This photo is no longer on your computer. Take it again.".to_string();
    if !is_photo_id(id) {
        return Err(gone());
    }
    let meta = read_meta(dir, id)
        .filter(|m| m.node_id == node_id)
        .ok_or_else(gone)?;
    let from = image_path(dir, id, &meta.name);
    let inbox = files::inbox_dir(dir);
    std::fs::create_dir_all(&inbox).map_err(|e| e.to_string())?;
    let target = files::free_name(&inbox, &meta.name);
    std::fs::rename(&from, &target).map_err(|_| gone())?;
    let _ = std::fs::remove_file(meta_path(dir, id));
    Ok(target)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Value> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| json!({ "code": "runtime_error", "message": format!("Reading the photo stopped: {e}") }))
}

/// A photo is in: keep it apart, read its text with the indexer's OCR, and
/// tell the phone (`photoId` for "Keep in library", and the text it asks with).
pub async fn arrived(
    app: &AppHandle,
    caller: &Caller,
    dir: &Path,
    arrived: Arrived,
) -> Result<Value, Value> {
    let failed = |message: String| json!({ "code": "runtime_error", "message": message });
    let staging_dir = dir.to_path_buf();
    let node = caller.node_id.clone();
    let now = super::now_ms();
    let id = arrived.transfer_id.clone();
    let name = arrived.offer.name.clone();
    let (path, arrived) = blocking(move || {
        let stored = store(&staging_dir, &node, &arrived, now);
        (stored, arrived)
    })
    .await?;
    // The staged part was moved (or could not be): its record goes either way.
    files::forget_transfer(dir, &arrived.transfer_id);
    let path = path.map_err(|e| failed(format!("Your computer could not keep the photo: {e}")))?;
    let preparer = app
        .state::<RagState>()
        .rag
        .read()
        .await
        .file_preparer()
        .map_err(|e| failed(format!("{e:#}")))?;
    let read = blocking(move || preparer.read_text(&path)).await?;
    let text = read.map_err(|e| failed(format!("The photo could not be read: {e:#}")))?;
    let text: String = text.trim().chars().take(MAX_PHOTO_TEXT_CHARS).collect();
    Ok(json!({
        "photoId": id,
        "name": name,
        "text": text,
        "words": text.split_whitespace().count(),
    }))
}

/// `phone_photo_keep`: "Keep in library" for a photo asked about.
pub async fn keep(app: &AppHandle, caller: &Caller, raw: Value) -> Result<Value, Value> {
    let id = raw
        .get("photoId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let workspace_id = raw
        .get("workspaceId")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(ws) = workspace_id.as_deref() {
        if files::destination(Some(ws)).is_none() || !files::workspace_exists(app, ws).await {
            return Err(shodh_link::bridge::refused(
                "That workspace is not on your computer any more. Choose another.",
            ));
        }
    }
    let dir = crate::profile::app_data_dir(app)
        .map_err(|e| json!({ "code": "runtime_error", "message": format!("The app data folder is unavailable: {e}") }))?;
    let node = caller.node_id.clone();
    let taken = blocking(move || take_for_keeping(&dir, &node, &id)).await?;
    let path = taken.map_err(shodh_link::bridge::refused)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let added = files::add_path(app, &path, workspace_id).await?;
    Ok(files::added_answer(&name, &path, &added))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_link::transfer::{sha256_hex, Offer, Part, Purpose, Staging};

    fn arrive(dir: &Path, node: &str) -> Arrived {
        let staging = Staging::new(dir.join("phone_transfers"));
        let bytes = b"\xFF\xD8\xFF a whiteboard".to_vec();
        let offer = Offer {
            name: "whiteboard.jpg".into(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes.as_slice()).unwrap(),
            workspace_id: None,
            purpose: Purpose::Photo,
        };
        let begun = staging.begin(node, &offer, 0).unwrap();
        staging
            .chunk(
                node,
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
        staging.finish(node, &begun.transfer_id).unwrap()
    }

    #[test]
    fn a_photo_is_not_indexed_unless_kept() {
        let data = tempfile::tempdir().unwrap();
        let arrived = arrive(data.path(), "phone");
        let id = arrived.transfer_id.clone();
        let kept = store(data.path(), "phone", &arrived, 1_000).unwrap();
        // Asked about: kept apart from the library, never in "From my phone"
        // or a linked folder, and gone after a day.
        assert_eq!(
            kept,
            data.path().join(PHOTO_FOLDER).join(format!("{id}.jpg"))
        );
        assert!(!files::inbox_dir(data.path()).exists());
        // Another phone cannot keep it.
        assert!(take_for_keeping(data.path(), "another", &id).is_err());
        assert!(take_for_keeping(data.path(), "phone", "../../x").is_err());
        // "Keep in library": moved into "From my phone" under its own name, for the upload path.
        let library = take_for_keeping(data.path(), "phone", &id).unwrap();
        assert_eq!(
            library,
            files::inbox_dir(data.path()).join("whiteboard.jpg")
        );
        assert!(library.exists() && !kept.exists());
        assert!(
            take_for_keeping(data.path(), "phone", &id).is_err(),
            "kept once"
        );
    }

    #[test]
    fn a_photo_not_kept_is_removed_after_a_day() {
        let data = tempfile::tempdir().unwrap();
        let first = arrive(data.path(), "phone");
        let id = first.transfer_id.clone();
        let path = store(data.path(), "phone", &first, 0).unwrap();
        // The next photo, a day and a bit later, sweeps the first.
        let mut next = arrive(data.path(), "phone");
        next.transfer_id = "f".repeat(32);
        store(data.path(), "phone", &next, KEEP_MS + 1).unwrap();
        assert!(!path.exists());
        assert!(take_for_keeping(data.path(), "phone", &id).is_err());
    }
}
