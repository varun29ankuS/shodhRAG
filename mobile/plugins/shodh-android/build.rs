//! The Kotlin side (android/) is built into the app by Tauri's Android
//! project. Only event listening is open to the window (`addPluginListener`);
//! every other native call comes from the app's Rust side, which checks it
//! first.

const COMMANDS: &[&str] = &["register_listener", "remove_listener"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
