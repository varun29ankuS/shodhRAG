//! Signed in-app updates from GitHub Releases (tauri-plugin-updater).
//!
//! The release workflow (`.github/workflows/release.yml`) signs the update
//! bundles and uploads them with `latest.json`; the plugin reads that file
//! from the endpoint in `tauri.conf.json` (`plugins.updater`) and refuses a
//! bundle whose minisign signature does not verify against the public key
//! there. A build made without a public key has no updates and never
//! contacts the update server.
//!
//! Checks are quiet and ask before changing anything: a few minutes after
//! start and then once a day while the app runs, only while the user leaves
//! "Check for updates" on and Local-only mode is off. A found update is
//! announced to the window ([`UPDATE_AVAILABLE_EVENT`]); it is downloaded and
//! installed only when the user chooses to.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::app_settings::{broadcast, AppSettings, SettingsStore};
use crate::audit_commands::AuditState;

/// Emitted with an [`UpdateInfo`] when a check finds a newer version.
pub const UPDATE_AVAILABLE_EVENT: &str = "update-available";
/// Emitted with an [`UpdateProgress`] while an update downloads.
pub const UPDATE_PROGRESS_EVENT: &str = "update-progress";

/// Out of the way of startup and of the idle-memory measurement (60 s runs).
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5 * 60);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

const NOT_CONFIGURED: &str = "This build of Shodh was made without an update key, so it cannot verify updates. Download new versions from the Shodh releases page.";
const LOCAL_ONLY: &str = "Local-only mode is on, so Shodh does not contact the update server. The user can turn it off in Settings → Privacy.";

/// A newer version the update server offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    /// Release date (RFC 3339), when the server gives one.
    pub date: Option<String>,
    /// Release notes, when the server gives them.
    pub notes: Option<String>,
}

impl From<&Update> for UpdateInfo {
    fn from(update: &Update) -> Self {
        Self {
            version: update.version.clone(),
            current_version: update.current_version.clone(),
            date: update.date.and_then(|d| {
                chrono::DateTime::from_timestamp(d.unix_timestamp(), 0).map(|d| d.to_rfc3339())
            }),
            notes: update.body.clone().filter(|b| !b.trim().is_empty()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// What Settings shows about updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current_version: String,
    pub check_automatically: bool,
    /// Why this app may not check now (no update key, Local-only mode).
    pub blocked_reason: Option<&'static str>,
}

/// The update found by the last check, kept so Install installs exactly
/// what the user was shown.
#[derive(Default)]
pub struct UpdaterState {
    pending: tokio::sync::Mutex<Option<Update>>,
    installing: AtomicBool,
}

/// Why no check may run now, if none may. `configured`: the build has an
/// update public key.
pub fn block_reason(configured: bool, settings: &AppSettings) -> Option<&'static str> {
    if !configured {
        Some(NOT_CONFIGURED)
    } else if settings.policy.local_only {
        Some(LOCAL_ONLY)
    } else {
        None
    }
}

/// Whether the background check may contact the update server now.
pub fn automatic_check_allowed(configured: bool, settings: &AppSettings) -> bool {
    settings.updates.check_automatically && block_reason(configured, settings).is_none()
}

/// The build carries an update public key (`plugins.updater.pubkey`).
pub fn is_configured<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
}

fn settings_store<R: Runtime>(app: &AppHandle<R>) -> Result<SettingsStore, String> {
    crate::profile::app_data_dir(app)
        .map(|dir| SettingsStore::in_dir(&dir))
        .map_err(|e| format!("Failed to get app data directory: {e}"))
}

fn load_settings<R: Runtime>(app: &AppHandle<R>) -> Result<AppSettings, String> {
    settings_store(app)?.load().map_err(|e| e.to_string())
}

fn status<R: Runtime>(app: &AppHandle<R>, settings: &AppSettings) -> UpdateStatus {
    UpdateStatus {
        current_version: app.package_info().version.to_string(),
        check_automatically: settings.updates.check_automatically,
        blocked_reason: block_reason(is_configured(app), settings),
    }
}

/// Ask the update server for a newer version, keep it for Install and
/// announce it to the window.
async fn check<R: Runtime>(
    app: &AppHandle<R>,
    state: &UpdaterState,
) -> Result<Option<UpdateInfo>, String> {
    let update = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| format!("The updater could not start: {e}"))?
        .check()
        .await
        .map_err(|e| format!("Checking for updates failed: {e}"))?;
    let info = update.as_ref().map(UpdateInfo::from);
    *state.pending.lock().await = update;
    if let Some(info) = &info {
        tracing::info!(
            "Update available: {} → {}",
            info.current_version,
            info.version
        );
        if let Err(e) = app.emit(UPDATE_AVAILABLE_EVENT, info) {
            tracing::warn!("Failed to emit {UPDATE_AVAILABLE_EVENT}: {e}");
        }
    }
    Ok(info)
}

/// Check a few minutes after start and then once a day, while allowed.
pub fn spawn_background_checks(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            let allowed = match load_settings(&app) {
                Ok(settings) => automatic_check_allowed(is_configured(&app), &settings),
                Err(e) => {
                    tracing::warn!("Update check skipped: settings unavailable: {e}");
                    false
                }
            };
            if allowed {
                let state = app.state::<UpdaterState>();
                match check(&app, &state).await {
                    Ok(Some(_)) => {}
                    Ok(None) => tracing::debug!("No update available"),
                    // Offline or the server is unreachable: try again tomorrow.
                    Err(e) => tracing::info!("{e}"),
                }
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

// ── Commands ─────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_update_status(app: AppHandle) -> Result<UpdateStatus, String> {
    let settings = load_settings(&app)?;
    Ok(status(&app, &settings))
}

/// Turn the automatic check on or off. User-only: no agent tool calls this,
/// and the agent's settings tools refuse `check_for_updates`.
#[tauri::command]
pub async fn set_update_checks(
    app: AppHandle,
    enabled: bool,
    audit: State<'_, AuditState>,
) -> Result<UpdateStatus, String> {
    let (settings, before) = settings_store(&app)?
        .update(|s| {
            Ok(std::mem::replace(
                &mut s.updates.check_automatically,
                enabled,
            ))
        })
        .map_err(|e| e.to_string())?;
    if before != enabled {
        audit.record(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({"action": "check_for_updates", "old": before, "new": enabled, "via": "ui"}),
        ));
    }
    broadcast(&app, &settings);
    Ok(status(&app, &settings))
}

/// Check now (Settings → Privacy, Check now). Allowed with the automatic
/// check off, never in Local-only mode or without an update key.
#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    state: State<'_, UpdaterState>,
) -> Result<Option<UpdateInfo>, String> {
    let settings = load_settings(&app)?;
    if let Some(reason) = block_reason(is_configured(&app), &settings) {
        return Err(reason.to_string());
    }
    check(&app, &state).await
}

/// Download the update the last check found, verify its signature, stop the
/// agent runtimes and install it, then restart into the new version. On
/// Windows the installer replaces the running app and starts it again.
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    state: State<'_, UpdaterState>,
    audit: State<'_, AuditState>,
) -> Result<(), String> {
    let settings = load_settings(&app)?;
    if let Some(reason) = block_reason(is_configured(&app), &settings) {
        return Err(reason.to_string());
    }
    if state.installing.swap(true, Ordering::SeqCst) {
        return Err("An update is already being installed".to_string());
    }
    let result = download_and_install(&app, &state, &audit).await;
    state.installing.store(false, Ordering::SeqCst);
    result?;
    app.restart()
}

async fn download_and_install(
    app: &AppHandle,
    state: &UpdaterState,
    audit: &AuditState,
) -> Result<(), String> {
    let Some(update) = state.pending.lock().await.clone() else {
        return Err("No update is waiting to be installed. Check for updates first.".to_string());
    };
    let mut downloaded: u64 = 0;
    let mut last_percent: Option<u64> = None;
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                // One event per whole percent (or per MiB without a length).
                let mark = match total {
                    Some(total) if total > 0 => downloaded * 100 / total,
                    _ => downloaded >> 20,
                };
                if last_percent != Some(mark) {
                    last_percent = Some(mark);
                    let _ = app.emit(UPDATE_PROGRESS_EVENT, UpdateProgress { downloaded, total });
                }
            },
            || {},
        )
        .await
        .map_err(|e| format!("Downloading the update failed: {e}"))?;

    audit.record(AuditRecord::new(
        AuditEventType::SettingsChange,
        json!({
            "action": "app_update_install",
            "old": update.current_version,
            "new": update.version,
            "via": "ui",
        }),
    ));
    // The installer replaces files the agent runtimes may hold open, and on
    // Windows it ends this process without the normal exit path: the window
    // writes its waiting saves first (bounded wait).
    crate::shutdown::prepare_exit(app).await;
    app.state::<crate::agent_session_commands::AgentSessions>()
        .shutdown_all()
        .await;
    let installed = match tokio::task::spawn_blocking(move || update.install(bytes)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("Installing the update failed: {e}")),
        Err(e) => Err(format!("Installing the update failed: {e}")),
    };
    if installed.is_err() {
        crate::shutdown::cancel_exit(app);
    }
    installed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_need_a_key_and_respect_local_only_and_the_setting() {
        let mut settings = AppSettings::default();
        assert!(automatic_check_allowed(true, &settings));
        assert_eq!(block_reason(true, &settings), None);

        assert!(!automatic_check_allowed(false, &settings));
        assert_eq!(block_reason(false, &settings), Some(NOT_CONFIGURED));

        settings.policy.local_only = true;
        assert!(!automatic_check_allowed(true, &settings));
        assert_eq!(block_reason(true, &settings), Some(LOCAL_ONLY));

        settings.policy.local_only = false;
        settings.updates.check_automatically = false;
        assert!(!automatic_check_allowed(true, &settings));
        // A manual check is still allowed.
        assert_eq!(block_reason(true, &settings), None);
    }

    #[test]
    fn release_config_points_at_github_releases_over_https() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let updater = &conf["plugins"]["updater"];
        assert!(
            updater["pubkey"].is_string(),
            "plugin init needs the pubkey field"
        );
        let endpoints = updater["endpoints"].as_array().unwrap();
        assert_eq!(
            endpoints,
            &[json!(
                "https://github.com/varun29ankuS/shodhRAG/releases/latest/download/latest.json"
            )]
        );
        assert_eq!(conf["version"], json!(env!("CARGO_PKG_VERSION")));
    }
}
