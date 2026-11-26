//! The phone's status bar (and Android's navigation bar) follows the app's
//! theme: light icons on the dark theme, dark icons on the light one, changed
//! live with it. The app draws under them, so the page is told where they
//! are ([`system_insets`]; changes come as the plugin's `insets` event): on
//! iPhone, the status bar or Dynamic Island and the home indicator.

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::native::Native;

/// Set the system bars for `theme` (`light` or `dark`). `follows_phone` says
/// the app shows the phone's own theme rather than one the user pinned:
/// Android's bars are set the same either way; iOS draws its status bar for
/// the window's own light or dark, which is pinned only when the theme is.
#[tauri::command]
pub async fn system_bars(
    app: AppHandle,
    theme: String,
    follows_phone: Option<bool>,
) -> Result<(), String> {
    let dark = match theme.as_str() {
        "dark" => true,
        "light" => false,
        _ => return Err("The theme is light or dark".to_string()),
    };
    let _: serde_json::Value = app
        .state::<Native<tauri::Wry>>()
        .call(
            "setSystemBars",
            json!({ "dark": dark, "followsPhone": follows_phone.unwrap_or(false) }),
        )
        .await?;
    Ok(())
}

/// Where the status bar, the navigation bar and a camera cutout cover the
/// window, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Insets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl Insets {
    /// Each side is a finite size of zero or more.
    fn checked(self) -> Result<Self, String> {
        let sides = [self.top, self.right, self.bottom, self.left];
        if sides.iter().all(|side| side.is_finite() && *side >= 0.0) {
            Ok(self)
        } else {
            Err(format!("The system bars' insets are not sizes: {self:?}"))
        }
    }
}

/// The system bars' insets now.
#[tauri::command]
pub async fn system_insets(app: AppHandle) -> Result<Insets, String> {
    let insets: Insets = app
        .state::<Native<tauri::Wry>>()
        .call("systemInsets", json!({}))
        .await?;
    insets.checked()
}
