#![cfg(target_os = "android")]

//! Shodh's own Android glue, in Kotlin where Android requires it
//! (android/src/main/java): the file picker, the camera and the share sheet,
//! the phone's calendar (CalendarContract), notifications and the "Stay
//! connected" service, UnifiedPush, speech recognition and text to speech,
//! and handing a verified update to Android's installer.
//!
//! The app's Rust side calls it through [`Native::call`]; the window may only
//! listen to its events (what was shared, a calendar change, a push).

use serde::de::DeserializeOwned;
use serde::Serialize;
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{Manager, Runtime};

/// The Kotlin plugin's package and class.
const PLUGIN_PACKAGE: &str = "com.shodhrag.android";
const PLUGIN_CLASS: &str = "ShodhPlugin";

/// The plugin's name, as the window's `addPluginListener` names it.
pub const NAME: &str = "shodh-android";

/// The Kotlin plugin.
pub struct Native<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> Native<R> {
    /// Run the Kotlin command `command` with `payload`; its rejection is the
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
            let handle = api.register_android_plugin(PLUGIN_PACKAGE, PLUGIN_CLASS)?;
            app.manage(Native(handle));
            Ok(())
        })
        .build()
}
