//! Android's status and navigation bars follow the app's theme: light icons
//! on the dark theme, dark icons on the light one, changed live with it.

use serde_json::json;
use tauri::{AppHandle, Manager};
use tauri_plugin_shodh_android::Native;

/// Set the system bars for `theme` (`light` or `dark`).
#[tauri::command]
pub async fn system_bars(app: AppHandle, theme: String) -> Result<(), String> {
    let dark = match theme.as_str() {
        "dark" => true,
        "light" => false,
        _ => return Err("The theme is light or dark".to_string()),
    };
    let _: serde_json::Value = app
        .state::<Native<tauri::Wry>>()
        .call("setSystemBars", json!({ "dark": dark }))
        .await?;
    Ok(())
}
