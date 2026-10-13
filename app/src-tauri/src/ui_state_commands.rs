//! UI state the app must not lose when it is killed: the Library's list of
//! sources and the first-run setup state.
//!
//! The frontend kept these in the WebView's localStorage only, which the
//! WebView writes to disk lazily: a force-quit (Task Manager, a crash, a
//! power cut) could lose them while the index they describe survived, so the
//! Library came back empty over a full index and setup started again. Here
//! each value is a JSON file in the app data folder, written to a temporary
//! file, flushed to disk and renamed over the old one, so a kill at any point
//! leaves either the previous value or the new one.
//!
//! Keys are a fixed list; values are opaque JSON the frontend validates when
//! it reads them back.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;

/// Folder under the app data folder holding one file per key.
const UI_STATE_DIR: &str = "ui_state";
/// Largest value stored, in bytes of JSON.
const MAX_VALUE_BYTES: usize = 2 * 1024 * 1024;

/// The keys that can be stored.
const KEYS: &[&str] = &["library_sources", "first_run"];

/// Serialises writes, so two saves of one key never interleave.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStateError {
    message: String,
}

impl UiStateError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// The file a key is stored in.
fn key_path(dir: &Path, key: &str) -> Result<PathBuf, UiStateError> {
    if !KEYS.contains(&key) {
        return Err(UiStateError::new(format!("Unknown UI state key: {key}")));
    }
    Ok(dir.join(UI_STATE_DIR).join(format!("{key}.json")))
}

/// The stored value of `key`; `None` when it was never stored.
pub(crate) fn read_value(dir: &Path, key: &str) -> Result<Option<Value>, UiStateError> {
    let path = key_path(dir, key)?;
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| UiStateError::new(format!("{} could not be read: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(UiStateError::new(format!(
            "{} could not be read: {e}",
            path.display()
        ))),
    }
}

/// Store `value` under `key` durably: a temporary file is written, flushed
/// to disk and renamed over the previous value.
pub(crate) fn write_value(dir: &Path, key: &str, value: &Value) -> Result<(), UiStateError> {
    let path = key_path(dir, key)?;
    let text = serde_json::to_string(value)
        .map_err(|e| UiStateError::new(format!("The value could not be encoded: {e}")))?;
    if text.len() > MAX_VALUE_BYTES {
        return Err(UiStateError::new(format!(
            "The value is too large to store ({} bytes; at most {MAX_VALUE_BYTES})",
            text.len()
        )));
    }
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let io = |e: std::io::Error| {
        UiStateError::new(format!("{} could not be saved: {e}", path.display()))
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io)?;
    }
    let tmp = path.with_extension("json.tmp");
    let written = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &path)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(io(e));
    }
    Ok(())
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, UiStateError> {
    crate::profile::app_data_dir(app)
        .map_err(|e| UiStateError::new(format!("The app data folder could not be found: {e}")))
}

/// The stored value of `key`, or `null` when it was never stored.
#[tauri::command]
pub async fn ui_state_get(app: AppHandle, key: String) -> Result<Option<Value>, UiStateError> {
    let dir = data_dir(&app)?;
    tokio::task::spawn_blocking(move || read_value(&dir, &key))
        .await
        .map_err(|e| UiStateError::new(format!("Reading the UI state failed: {e}")))?
}

/// Store `value` under `key` (durably, before this resolves).
#[tauri::command]
pub async fn ui_state_set(app: AppHandle, key: String, value: Value) -> Result<(), UiStateError> {
    let dir = data_dir(&app)?;
    tokio::task::spawn_blocking(move || write_value(&dir, &key, &value))
        .await
        .map_err(|e| UiStateError::new(format!("Saving the UI state failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_round_trip_and_replace_the_previous_one() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_value(dir.path(), "library_sources").unwrap(), None);
        let sources = json!([{"id": "s1", "path": "C:/papers", "name": "papers"}]);
        write_value(dir.path(), "library_sources", &sources).unwrap();
        assert_eq!(
            read_value(dir.path(), "library_sources").unwrap(),
            Some(sources)
        );
        write_value(dir.path(), "library_sources", &json!([])).unwrap();
        assert_eq!(
            read_value(dir.path(), "library_sources").unwrap(),
            Some(json!([]))
        );
        // No temporary file is left behind.
        let left: Vec<_> = fs::read_dir(dir.path().join(UI_STATE_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["library_sources.json"]);
    }

    #[test]
    fn only_known_keys_and_bounded_values_are_stored() {
        let dir = tempfile::tempdir().unwrap();
        assert!(write_value(dir.path(), "../escape", &json!(1)).is_err());
        assert!(read_value(dir.path(), "theme").is_err());
        let huge = json!("x".repeat(MAX_VALUE_BYTES + 1));
        assert!(write_value(dir.path(), "first_run", &huge).is_err());
        write_value(
            dir.path(),
            "first_run",
            &json!({"status": "completed", "step": "done"}),
        )
        .unwrap();
        assert_eq!(
            read_value(dir.path(), "first_run").unwrap(),
            Some(json!({"status": "completed", "step": "done"}))
        );
    }

    #[test]
    fn a_damaged_file_is_reported_not_taken_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(UI_STATE_DIR)).unwrap();
        fs::write(
            dir.path().join(UI_STATE_DIR).join("library_sources.json"),
            "{not json",
        )
        .unwrap();
        assert!(read_value(dir.path(), "library_sources").is_err());
    }
}
