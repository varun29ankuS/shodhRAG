//! Voice (Kotlin, `Voice.kt`; Swift, `Voice.swift`): tap to talk with the
//! phone's own speech recognition (on the device when the phone has it), and
//! answers read aloud by its text to speech. The words read aloud are the
//! window's (mobile/src/lib/speech.ts: spoken citations from the citation
//! metadata).

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::native::Native;

/// The longest text read aloud at once.
const MAX_SPOKEN: usize = 20_000;

fn native(app: &AppHandle) -> tauri::State<'_, Native<tauri::Wry>> {
    app.state::<Native<tauri::Wry>>()
}

/// Listen once; what the user said.
#[tauri::command]
pub async fn voice_listen(app: AppHandle) -> Result<String, String> {
    let reply: Value = native(&app).call("voiceListen", json!({})).await?;
    Ok(reply["text"].as_str().unwrap_or_default().to_string())
}

#[tauri::command]
pub async fn voice_stop_listening(app: AppHandle) -> Result<(), String> {
    let _: Value = native(&app).call("voiceStopListening", json!({})).await?;
    Ok(())
}

/// Read `text` aloud; resolves when it was read.
#[tauri::command]
pub async fn voice_speak(app: AppHandle, text: String) -> Result<(), String> {
    let text: String = text.chars().take(MAX_SPOKEN).collect();
    let _: Value = native(&app)
        .call("voiceSpeak", json!({ "text": text }))
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn voice_stop(app: AppHandle) -> Result<(), String> {
    let _: Value = native(&app).call("voiceStop", json!({})).await?;
    Ok(())
}
