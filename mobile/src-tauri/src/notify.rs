//! Notifications on the phone (Settings, Notifications), off by default:
//!
//! - **Off:** only while the app is open.
//! - **Stay connected:** a foreground service keeps the link open, and the
//!   link's own events (a step waiting, an answer finished) are told with a
//!   content-free notification (`shodh_link::notify`).
//! - **UnifiedPush:** the user's distributor app hands Shodh an endpoint, kept
//!   in secure storage and given to the computer over the link; the computer
//!   posts only a content-free word to it. The endpoint is never logged.
//!
//! The mode is kept sealed on the phone. Notifications are posted by the
//! Kotlin plugin's `Notifier`, reached through JNI from any thread, so they
//! work while the app's window is in the background.

use serde_json::{json, Value};
use shodh_link::notify::{Kind, Mode};
use shodh_link::phone::SecretVault;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_shodh_android::Native;

use crate::link::{blocking, Link};
use crate::store::Store;
use crate::vault::KeystoreVault;

const MODE_FILE: &str = "notifications.sealed";

/// The vault entry of the push endpoint (a secret).
const ENDPOINT_ENTRY: &str = "push-endpoint";

fn native(app: &AppHandle) -> State<'_, Native<tauri::Wry>> {
    app.state::<Native<tauri::Wry>>()
}

/// The mode as kept (Off when never set).
pub async fn mode(app: &AppHandle) -> Mode {
    let Ok(storage) = app.state::<Store>().get(app).await else {
        return Mode::Off;
    };
    blocking(move || {
        Ok(storage
            .sealed(MODE_FILE)
            .read::<Mode>()
            .ok()
            .flatten()
            .unwrap_or_default())
    })
    .await
    .unwrap_or_default()
}

/// Post a notification of `kind` through the plugin's Notifier (JNI).
pub fn post(kind: Kind) {
    if let Err(e) = post_jni(kind.word()) {
        tracing::warn!("A notification could not be shown: {e}");
    }
}

fn post_jni(kind: &str) -> Result<(), String> {
    use jni::objects::{JClass, JObject, JValue};
    let context = ndk_context::android_context();
    // SAFETY: the VM and the application context are set up by Tauri's runtime
    // (tao) for the life of the process; neither is freed here.
    let vm = unsafe { jni::JavaVM::from_raw(context.vm().cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let app_context = unsafe { JObject::from_raw(context.context().cast()) };
    // The app's own class loader: a thread Rust started only sees the system's.
    let loader = env
        .call_method(
            &app_context,
            "getClassLoader",
            "()Ljava/lang/ClassLoader;",
            &[],
        )
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let name = env
        .new_string("com.shodhrag.android.Notifier")
        .map_err(|e| e.to_string())?;
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let class = JClass::from(class);
    let kind = env.new_string(kind).map_err(|e| e.to_string())?;
    env.call_static_method(
        &class,
        "post",
        "(Landroid/content/Context;Ljava/lang/String;)V",
        &[JValue::Object(&app_context), JValue::Object(&kind)],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// A desktop event the link received: told while "Stay connected" is chosen.
pub async fn on_event(app: &AppHandle, name: &str, payload: &Value) {
    let Some(kind) = shodh_link::notify::kind_of(name, payload) else {
        return;
    };
    if mode(app).await.notifies_from_link() {
        post(kind);
    }
}

/// Start or stop what the mode needs: the service, the push registration.
async fn apply(app: &AppHandle, mode: Mode, distributor: Option<String>) -> Result<(), String> {
    if mode.runs_service() {
        let _: Value = native(app).call("startConnection", json!({})).await?;
    } else {
        let _: Value = native(app).call("stopConnection", json!({})).await?;
    }
    if mode.registers_push() {
        let distributor = distributor.ok_or("Choose the push app to use.")?;
        let _: Value = native(app)
            .call("pushRegister", json!({ "distributor": distributor }))
            .await?;
    } else {
        let _: Value = native(app).call("pushUnregister", json!({})).await?;
        blocking(|| KeystoreVault::open()?.remove(ENDPOINT_ENTRY)).await?;
        // The computer forgets this phone's push address (when it can be reached now).
        let link = app.state::<Link>();
        if let Err(e) = link
            .invoke("phone_push_register", json!({ "endpoint": null }))
            .await
        {
            tracing::info!(
                "The computer was not told to forget the push address now: {}",
                e["message"]
            );
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn notifications_mode(app: AppHandle) -> Mode {
    mode(&app).await
}

/// Choose how the phone tells the user when the app is closed.
#[tauri::command]
pub async fn notifications_set(
    app: AppHandle,
    mode: Mode,
    distributor: Option<String>,
    store: State<'_, Store>,
) -> Result<Mode, String> {
    if mode != Mode::Off {
        let reply: Value = native(&app).call("notificationsAccess", json!({})).await?;
        if reply["granted"].as_bool() != Some(true) {
            return Err("Android does not let Shodh show notifications. Allow them in Android's settings for Shodh.".into());
        }
    }
    apply(&app, mode, distributor).await?;
    let storage = store.get(&app).await?;
    blocking(move || storage.sealed(MODE_FILE).write(&mode)).await?;
    tracing::info!("Notifications: {mode:?}");
    Ok(mode)
}

/// At start: the service runs again if the user chose Stay connected.
pub async fn resume(app: &AppHandle) {
    let mode = mode(app).await;
    if mode.runs_service() {
        if let Err(e) = native(app)
            .call::<Value>("startConnection", json!({}))
            .await
        {
            tracing::warn!("Shodh could not stay connected: {e}");
        }
    }
}

/// The push apps on this phone.
#[tauri::command]
pub async fn push_distributors(app: AppHandle) -> Result<Vec<String>, String> {
    let reply: Value = native(&app).call("pushDistributors", json!({})).await?;
    serde_json::from_value(reply["distributors"].clone()).map_err(|e| e.to_string())
}

/// Move a new endpoint from the distributor into secure storage, and give the
/// kept one to the computer. Whether the computer has it now.
#[tauri::command]
pub async fn push_sync(app: AppHandle, link: State<'_, Link>) -> Result<bool, String> {
    if !mode(&app).await.registers_push() {
        return Ok(false);
    }
    let reply: Value = native(&app).call("pushTakeEndpoint", json!({})).await?;
    if let Some(endpoint) = reply["endpoint"].as_str() {
        let endpoint = shodh_link::notify::Endpoint::parse(endpoint).map_err(str::to_string)?;
        let kept = endpoint.expose().to_string();
        blocking(move || KeystoreVault::open()?.store(ENDPOINT_ENTRY, kept.as_bytes())).await?;
    }
    let Some(kept) = blocking(|| KeystoreVault::open()?.load(ENDPOINT_ENTRY)).await? else {
        return Ok(false);
    };
    let endpoint =
        String::from_utf8(kept).map_err(|_| "The push address is damaged.".to_string())?;
    link.invoke("phone_push_register", json!({ "endpoint": endpoint }))
        .await
        .map(|_| true)
        .map_err(|e| {
            e["message"]
                .as_str()
                .unwrap_or("The computer did not keep it.")
                .to_string()
        })
}

/// Briefs before meetings: alarms at these instants (event id, Unix ms).
#[tauri::command]
pub async fn briefs_schedule(app: AppHandle, briefs: Vec<(String, i64)>) -> Result<(), String> {
    let list: Vec<Value> = briefs
        .into_iter()
        .filter(|(id, _)| !id.is_empty() && id.len() <= 100)
        .map(|(event_id, at_ms)| json!({ "eventId": event_id, "atMs": at_ms }))
        .collect();
    let _: Value = native(&app)
        .call("scheduleBriefs", json!({ "briefs": list }))
        .await?;
    Ok(())
}
