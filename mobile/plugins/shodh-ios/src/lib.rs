#![cfg(target_os = "ios")]

//! Shodh's own iOS glue, in Swift where iOS requires it (ios/Sources): the
//! file picker, the camera and what is opened with Shodh (the share sheet,
//! the Files app, a calendar link), the phone's calendar (EventKit),
//! notifications and staying connected for the time iOS allows, speech
//! recognition and text to speech, and the status bar and safe area.
//!
//! The same commands as the Android plugin (`plugins/shodh-android`), so the
//! app's Rust side calls either through [`Native::call`]; the window may only
//! listen to its events (what was shared, a calendar change, the insets).

use serde::de::DeserializeOwned;
use serde::Serialize;
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{Manager, Runtime};

// The Swift plugin's constructor (ios/Sources/ShodhPlugin.swift).
tauri::ios_plugin_binding!(init_plugin_shodh_ios);

/// The plugin's name, as the window's `addPluginListener` names it.
pub const NAME: &str = "shodh-ios";

/// The Swift plugin.
pub struct Native<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> Native<R> {
    /// Run the Swift command `command` with `payload`; its rejection is the
    /// error, in the plugin's own words.
    pub async fn call<T: DeserializeOwned>(
        &self,
        command: &str,
        payload: impl Serialize,
    ) -> Result<T, String> {
        self.0
            .run_mobile_plugin_async(command, payload)
            .await
            .map_err(|e| e.to_string())
    }
}

/// The plugin, managed as [`Native`].
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new(NAME)
        .setup(|app, api| {
            let handle = api.register_ios_plugin(init_plugin_shodh_ios)?;
            app.manage(Native(handle));
            Ok(())
        })
        .build()
}
