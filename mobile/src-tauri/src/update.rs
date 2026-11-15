//! The one-tap update ("Update Shodh on your phone"): the computer runs a
//! newer Shodh, so the phone fetches that version's Android app from Shodh's
//! GitHub release, verifies it (`shodh_link::update`: Shodh's release key and
//! the release's checksum and size) and only then hands it to Android's
//! installer, which also checks it is signed like the installed app.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};
use shodh_link::update::{self, ApkRelease, UpdateError, Version};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_shodh_android::Native;
use tokio::sync::Mutex;

use crate::link::{blocking, Link, Status};
use crate::store::Store;

/// Shodh's release key: the desktop updater's (build.rs).
const RELEASE_KEY: &str = env!("SHODH_RELEASE_KEY");

/// The largest signature file read.
const MAX_SIGNATURE_BYTES: i64 = 8 * 1024;

/// The release found by the last check, installed by [`update_install`].
#[derive(Default)]
pub struct Updates {
    found: Mutex<Option<ApkRelease>>,
}

/// What a check found, for the Update screen.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UpdateCheck {
    /// The matching app is published: install it.
    Ready { version: String, size: u64 },
    /// Nothing for the phone to do (another side must change, or no update).
    Unavailable { message: String },
}

fn error_words(e: UpdateError) -> UpdateCheck {
    UpdateCheck::Unavailable {
        message: e.to_string(),
    }
}

async fn fetch_text(app: &AppHandle, url: &str) -> Result<String, String> {
    let native = app.state::<Native<tauri::Wry>>();
    let reply: Value = native.call("fetchText", json!({ "url": url })).await?;
    reply
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "The answer could not be read.".to_string())
}

async fn download(
    app: &AppHandle,
    url: &str,
    path: &std::path::Path,
    max: i64,
) -> Result<(), String> {
    let native = app.state::<Native<tauri::Wry>>();
    let _: Value = native
        .call(
            "download",
            json!({ "url": url, "path": path.display().to_string(), "maxBytes": max }),
        )
        .await?;
    Ok(())
}

/// Which version the computer runs, from the link's state.
fn desktop_version(link: &Link) -> Option<String> {
    match link.status() {
        Status::Update { desktop, .. } => Some(desktop),
        _ => None,
    }
}

/// Look for the app matching the computer's version.
#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    link: State<'_, Link>,
    updates: State<'_, Updates>,
) -> Result<UpdateCheck, String> {
    let Some(desktop) = desktop_version(&link) else {
        return Ok(UpdateCheck::Unavailable {
            message: "Your phone and your computer run the same Shodh.".into(),
        });
    };
    let version: Version = match update::needed(env!("CARGO_PKG_VERSION"), &desktop) {
        Ok(version) => version,
        Err(e) => return Ok(error_words(e)),
    };
    let text = match fetch_text(&app, &update::release_api_url(version)).await {
        Ok(text) => text,
        Err(e) if e.contains("not found") => {
            return Ok(error_words(UpdateError::NotPublished(version.to_string())))
        }
        Err(e) => return Ok(error_words(UpdateError::Download(e))),
    };
    let release: Value = serde_json::from_str(&text)
        .map_err(|_| "GitHub's answer could not be read. Try again later.".to_string())?;
    match update::choose_apk(&release, version) {
        Ok(found) => {
            let check = UpdateCheck::Ready {
                version: found.version.clone(),
                size: found.size,
            };
            *updates.found.lock().await = Some(found);
            Ok(check)
        }
        Err(e) => Ok(error_words(e)),
    }
}

/// Download, verify and hand to Android's installer the app the last check
/// found. `false` when Android first needs the user to allow Shodh to
/// install updates (its settings page is open; tap Update again after).
#[tauri::command]
pub async fn update_install(
    app: AppHandle,
    store: State<'_, Store>,
    updates: State<'_, Updates>,
) -> Result<bool, String> {
    let found = updates
        .found
        .lock()
        .await
        .clone()
        .ok_or("Look for the update first.")?;
    let storage = store.get(&app).await?;
    let folder = storage.cache().join("update");
    let apk: PathBuf = folder.join(update::apk_name(
        Version::parse(&found.version).map_err(|e| e.to_string())?,
    ));
    let signature = folder.join("app.sig");
    let size = i64::try_from(found.size).map_err(|_| "The app is too large.".to_string())?;
    download(&app, &found.signature_url, &signature, MAX_SIGNATURE_BYTES)
        .await
        .map_err(|e| UpdateError::Download(e).to_string())?;
    download(&app, &found.apk_url, &apk, size)
        .await
        .map_err(|e| UpdateError::Download(e).to_string())?;
    let checked = {
        let (apk, signature, found) = (apk.clone(), signature.clone(), found.clone());
        blocking(move || {
            let key = update::release_key(RELEASE_KEY)?;
            let text = std::fs::read_to_string(&signature).map_err(|e| e.to_string())?;
            let file = std::fs::File::open(&apk).map_err(|e| e.to_string())?;
            update::verify_apk(std::io::BufReader::new(file), &text, &key, &found)
                .map_err(|e| e.to_string())
        })
        .await
    };
    if let Err(e) = checked {
        let _ = std::fs::remove_file(&apk);
        let _ = std::fs::remove_file(&signature);
        tracing::warn!("A downloaded update was refused: {e}");
        return Err(e);
    }
    tracing::info!(
        "Update {} verified; handing it to Android's installer",
        found.version
    );
    let native = app.state::<Native<tauri::Wry>>();
    let reply: Value = native
        .call("installApk", json!({ "path": apk.display().to_string() }))
        .await
        .map_err(|e| format!("Android did not install the update: {e}"))?;
    Ok(reply
        .get("started")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}
