//! The phone's lock (`shodh_link::lock`), on by default: Shodh asks for the
//! PIN, or the phone's fingerprint, when it opens and when it comes back after
//! a minute away. While it is locked the app's Rust side refuses to call the
//! computer at all ([`LockGate::check`]), whatever the window asks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use shodh_link::lock::{Lock, LockError};
use tauri::{AppHandle, State};
use tauri_plugin_biometric::{AuthOptions, BiometricExt};

use crate::link::{blocking, now_ms};
use crate::vault::KeystoreVault;

/// Away this long, and Shodh asks again.
const AWAY: Duration = Duration::from_secs(60);

/// Whether the app is locked now.
pub struct LockGate {
    locked: AtomicBool,
    hidden_at: Mutex<Option<Instant>>,
}

impl Default for LockGate {
    /// Locked from the start: the lock opens only once it was asked.
    fn default() -> Self {
        Self {
            locked: AtomicBool::new(true),
            hidden_at: Mutex::new(None),
        }
    }
}

impl LockGate {
    pub fn locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }

    /// Refuse a call to the computer while locked.
    pub fn check(&self) -> Result<(), serde_json::Value> {
        if self.locked() {
            return Err(serde_json::json!({ "code": "locked", "message": "Unlock Shodh first." }));
        }
        Ok(())
    }

    fn open(&self) {
        self.locked.store(false, Ordering::SeqCst);
    }
}

fn words(e: LockError) -> String {
    e.to_string()
}

/// What the lock screen and Settings show.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockStatus {
    pub required: bool,
    pub has_pin: bool,
    pub biometric: bool,
    pub locked: bool,
}

async fn with_lock<T: Send + 'static>(
    work: impl FnOnce(&Lock<'_>) -> Result<T, LockError> + Send + 'static,
) -> Result<T, String> {
    blocking(move || {
        let vault = KeystoreVault::open()?;
        work(&Lock::new(&vault)).map_err(words)
    })
    .await
}

#[tauri::command]
pub async fn lock_status(gate: State<'_, LockGate>) -> Result<LockStatus, String> {
    let state = with_lock(|lock| lock.state()).await?;
    // A lock turned off opens at once.
    if !state.enabled {
        gate.open();
    }
    Ok(LockStatus {
        required: state.enabled,
        has_pin: state.hash.is_some(),
        biometric: state.biometric,
        locked: gate.locked(),
    })
}

/// Set the PIN: the first time (nothing to open yet), or when unlocked.
#[tauri::command]
pub async fn lock_set_pin(pin: String, gate: State<'_, LockGate>) -> Result<(), String> {
    let has_pin = with_lock(|lock| lock.state().map(|s| s.hash.is_some())).await?;
    if has_pin && gate.locked() {
        return Err("Unlock Shodh first.".into());
    }
    with_lock(move |lock| lock.set_pin(&pin)).await?;
    gate.open();
    Ok(())
}

#[tauri::command]
pub async fn lock_unlock(pin: String, gate: State<'_, LockGate>) -> Result<(), String> {
    with_lock(move |lock| lock.try_pin(&pin, now_ms())).await?;
    gate.open();
    tracing::info!("Shodh was unlocked");
    Ok(())
}

/// Unlock with the phone's fingerprint (or face), when the user turned it on.
#[tauri::command]
pub async fn lock_unlock_biometric(
    app: AppHandle,
    gate: State<'_, LockGate>,
) -> Result<(), String> {
    let state = with_lock(|lock| lock.state()).await?;
    if !state.biometric || state.hash.is_none() {
        return Err("Use your PIN.".into());
    }
    let handle = app.clone();
    blocking(move || {
        handle
            .biometric()
            .authenticate(
                "Unlock Shodh".to_string(),
                AuthOptions {
                    allow_device_credential: false,
                    title: Some("Unlock Shodh".to_string()),
                    cancel_title: Some("Use PIN".to_string()),
                    ..AuthOptions::default()
                },
            )
            .map_err(|e| format!("The fingerprint did not unlock Shodh: {e}"))
    })
    .await?;
    gate.open();
    Ok(())
}

/// Turn the lock on or off, or the fingerprint, with the PIN.
#[tauri::command]
pub async fn lock_configure(
    pin: String,
    enabled: Option<bool>,
    biometric: Option<bool>,
    gate: State<'_, LockGate>,
) -> Result<(), String> {
    if gate.locked() {
        return Err("Unlock Shodh first.".into());
    }
    with_lock(move |lock| {
        if let Some(on) = enabled {
            lock.set_enabled(on, &pin, now_ms())?;
        }
        if let Some(on) = biometric {
            lock.set_biometric(on, &pin, now_ms())?;
        }
        Ok(())
    })
    .await
}

/// The app went to the background.
#[tauri::command]
pub fn lock_hidden(gate: State<'_, LockGate>) {
    *gate.hidden_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

/// The app is back: locked again after a minute away (when the lock is on).
#[tauri::command]
pub async fn lock_shown(gate: State<'_, LockGate>) -> Result<bool, String> {
    let away = gate
        .hidden_at
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .is_some_and(|at| at.elapsed() >= AWAY);
    if away && with_lock(|lock| lock.required()).await? {
        gate.locked.store(true, Ordering::SeqCst);
    }
    Ok(gate.locked())
}
