//! The phone app's build: Tauri's, and Shodh's release key compiled in.
//!
//! The one-tap update installs only an app signed with the key the desktop's
//! updater trusts (`plugins.updater.pubkey` in the desktop's tauri.conf.json),
//! read here from that file, never copied, so the two cannot drift.

use std::path::Path;

fn main() {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src-tauri/tauri.conf.json");
    println!("cargo:rerun-if-changed={}", config.display());
    let text = std::fs::read_to_string(&config)
        .unwrap_or_else(|e| panic!("the desktop's tauri.conf.json could not be read: {e}"));
    let json: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("the desktop's tauri.conf.json is not JSON: {e}"));
    let key = json["plugins"]["updater"]["pubkey"]
        .as_str()
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| panic!("the desktop's tauri.conf.json has no updater public key"));
    println!("cargo:rustc-env=SHODH_RELEASE_KEY={}", key.trim());
    tauri_build::build()
}
