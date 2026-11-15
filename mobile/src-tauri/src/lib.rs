//! Shodh on the phone: a remote for the user's own Shodh computer. The
//! computer does all the work (search, models, the agent); the phone pairs
//! with it once by QR code and then reaches it over the phone link
//! (`shodh-link`: iroh, end-to-end encrypted, no Shodh server).

#[cfg(not(target_os = "android"))]
compile_error!(
    "Shodh's phone app is built for Android: its link key is kept with the Android Keystore \
     (vault.rs). Build it with `npm run tauri android build`."
);

mod bars;
mod calendar;
mod files;
mod link;
mod lock;
mod logcat;
mod notify;
mod queue;
mod store;
#[cfg(debug_assertions)]
mod tour;
mod update;
mod vault;
mod voice;

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
        .plugin(tauri_plugin_shodh_android::init())
        .plugin(tauri_plugin_biometric::init())
        .manage(lock::LockGate::default())
        .manage(link::Link::new(link::device_name()))
        .manage(store::Store::default())
        .manage(queue::Queue::default())
        .manage(update::Updates::default())
        .setup(|app| {
            // CI's screenshot tour (tour.rs): a debug build only.
            #[cfg(debug_assertions)]
            tour::follow(app.handle().clone());
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                handle.state::<link::Link>().start(&handle).await;
                notify::resume(&handle).await;
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
            queue::outbox_list,
            queue::outbox_add,
            queue::outbox_mark,
            queue::outbox_remove,
            update::update_check,
            update::update_install,
            files::files_pick,
            files::photo_take,
            files::incoming_take,
            files::file_details,
            files::file_discard,
            files::file_send,
            files::photo_send,
            calendar::calendar_access,
            calendar::calendar_list,
            calendar::calendar_choice,
            calendar::calendar_choose,
            calendar::calendar_events,
            calendar::calendar_write,
            calendar::sealed_read,
            calendar::sealed_write,
            bars::system_bars,
            bars::system_insets,
            notify::notifications_mode,
            notify::notifications_set,
            notify::push_distributors,
            notify::push_sync,
            notify::briefs_schedule,
            lock::lock_status,
            lock::lock_set_pin,
            lock::lock_unlock,
            lock::lock_unlock_biometric,
            lock::lock_configure,
            lock::lock_hidden,
            lock::lock_shown,
            voice::voice_listen,
            voice::voice_stop_listening,
            voice::voice_speak,
            voice::voice_stop,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        tracing::error!("Shodh could not start: {e}");
    }
}
