//! The Swift side (ios/) is built into the app as a static library by this
//! build script (tauri-plugin compiles the Swift package and links it). Only
//! event listening is open to the window (`addPluginListener`); every other
//! native call comes from the app's Rust side, which checks it first.

const COMMANDS: &[&str] = &["register_listener", "remove_listener"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();
}
