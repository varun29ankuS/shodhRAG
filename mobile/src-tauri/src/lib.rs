//! Shodh on the phone: a remote for the user's own Shodh computer. The
//! computer does all the work (search, models, the agent); the phone pairs
//! with it once by QR code and then reaches it over the phone link
//! (`shodh-link`: iroh, end-to-end encrypted, no Shodh server).
//!
//! One app for Android and iPhone. What each phone does its own way is behind
//! one name here: its secure storage ([`vault`]: the Android Keystore, the iOS
//! Keychain), its log ([`log`]), and its native glue ([`native`]: Kotlin in
//! `plugins/shodh-android`, Swift in `plugins/shodh-ios`, the same commands).

#[cfg(not(any(target_os = "android", target_os = "ios")))]
compile_error!(
    "Shodh's phone app is built for Android and iPhone: its link key is kept with the Android \
     Keystore or the iOS Keychain (vault_android.rs, vault_ios.rs). Build it with \
     `npm run tauri android build` or `npm run tauri ios build`."
);

mod bars;
mod calendar;
mod files;
mod link;
mod lock;
mod log;
mod notify;
mod queue;
mod store;
// CI's screenshot tour is driven through Android's system properties.
#[cfg(all(debug_assertions, target_os = "android"))]
mod tour;
#[cfg(target_os = "android")]
mod update;
#[cfg(target_os = "ios")]
#[path = "update_ios.rs"]
mod update;
#[cfg(target_os = "android")]
#[path = "vault_android.rs"]
mod vault;
#[cfg(target_os = "ios")]
#[path = "vault_ios.rs"]
mod vault;
mod voice;

/// The phone's native glue: the same commands, in Kotlin or in Swift.
#[cfg(target_os = "android")]
pub(crate) use tauri_plugin_shodh_android as native;
#[cfg(target_os = "ios")]
pub(crate) use tauri_plugin_shodh_ios as native;

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
    log::init();
    tracing::info!("Shodh {} is starting", env!("CARGO_PKG_VERSION"));
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_barcode_scanner::init())
        .plugin(native::init())
        .plugin(tauri_plugin_biometric::init())
        .manage(lock::LockGate::default())
        .manage(link::Link::new(link::device_name()))
        .manage(store::Store::default())
        .manage(queue::Queue::default());
    // The one-tap update is Android's: the release it found, kept until it is
    // installed. An iPhone is updated by TestFlight or the App Store, so
    // there the app keeps nothing and has no install command.
    #[cfg(target_os = "android")]
    let builder = builder.manage(update::Updates::default());
    let built = builder
        .setup(|app| {
            // CI's screenshot tour (tour.rs): a debug build only.
            #[cfg(all(debug_assertions, target_os = "android"))]
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
            #[cfg(target_os = "android")]
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
        .build(tauri::generate_context!());
    match built {
        // iOS hands the app what was opened with it (a file from the share
        // sheet or Files, one of Shodh's calendar links) as an event of the
        // run loop; Android's share sheet reaches the Kotlin plugin instead.
        Ok(app) => app.run(|_handle, _event| {
            #[cfg(target_os = "ios")]
            if let tauri::RunEvent::Opened { urls } = _event {
                files::opened(_handle, urls);
            }
        }),
        Err(e) => tracing::error!("Shodh could not start: {e}"),
    }
}
