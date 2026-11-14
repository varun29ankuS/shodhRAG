//! Shodh on the phone: a remote for the user's own Shodh computer. The
//! computer does all the work (search, models, the agent); the phone pairs
//! with it once by QR code and then reaches it over the phone link
//! (`shodh-link`: iroh, end-to-end encrypted, no Shodh server).

#[cfg(not(target_os = "android"))]
compile_error!(
    "Shodh's phone app is built for Android: its link key is kept with the Android Keystore \
     (vault.rs). Build it with `npm run tauri android build`."
);

mod link;
mod logcat;
mod vault;

use tauri::Manager;

/// Whether two strings are equal, in a constant.
const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

// The computer refuses a phone of another Shodh version, so the app and the
// link it is built with must be the same version.
const _: () = assert!(
    same(env!("CARGO_PKG_VERSION"), shodh_link::LINK_VERSION),
    "shodh-mobile and shodh-link must have the same version (scripts/release/versions.mjs)"
);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logcat::init();
    tracing::info!("Shodh {} is starting", env!("CARGO_PKG_VERSION"));
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_barcode_scanner::init())
        .manage(link::Link::new(link::device_name()))
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                handle.state::<link::Link>().start(&handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            link::link_status,
            link::link_pair,
            link::link_connect,
            link::link_invoke,
            link::link_subscribe,
            link::link_reset,
            link::app_ready,
            link::start_screen,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        tracing::error!("Shodh could not start: {e}");
    }
}
