//! The phone's own files: the offline queue, the copy of the agenda, the
//! calendar sync's state and the phone's settings, each sealed
//! (`shodh_link::sealed`) with a data key kept in the phone's secure storage
//! (the Android Keystore's, the iOS Keychain), in the app's private folder.
//! None of it leaves the phone: Android's backup is off for the app; on
//! iPhone the folder is kept out of iCloud and computer backups, and the key
//! never leaves the device (vault_ios.rs), so a copy would open nowhere else.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use shodh_link::sealed::{DataKey, SealedFile};
use tauri::{AppHandle, Manager};
use tokio::sync::OnceCell;

use crate::vault::Vault;

/// The folder and the key.
pub struct Storage {
    dir: PathBuf,
    cache: PathBuf,
    key: DataKey,
}

impl Storage {
    /// The sealed file `name` ("outbox.sealed").
    pub fn sealed(&self, name: &str) -> SealedFile {
        SealedFile::new(&self.dir, name, &self.key)
    }

    /// The app's cache folder (downloads, copies of files being sent).
    pub fn cache(&self) -> &Path {
        &self.cache
    }
}

/// Managed state: the storage, opened on first use.
#[derive(Default)]
pub struct Store {
    storage: OnceCell<Arc<Storage>>,
}

impl Store {
    /// The storage; a failure is not kept, so the next call tries again.
    pub async fn get(&self, app: &AppHandle) -> Result<Arc<Storage>, String> {
        self.storage
            .get_or_try_init(|| async {
                let base = app
                    .path()
                    .app_data_dir()
                    .map_err(|e| format!("This phone's storage for Shodh is unavailable: {e}"))?;
                let cache = app
                    .path()
                    .app_cache_dir()
                    .map_err(|e| format!("This phone's storage for Shodh is unavailable: {e}"))?;
                let key = crate::link::blocking(|| {
                    let vault = Vault::open()?;
                    DataKey::kept_or_new(&vault)
                })
                .await?;
                let dir = base.join("shodh");
                // iOS backs up an app's data unless a folder says otherwise.
                // Not kept out is not a reason to stop: the files are sealed
                // with a key no backup holds.
                #[cfg(target_os = "ios")]
                if let Err(e) = app
                    .state::<crate::native::Native<tauri::Wry>>()
                    .call::<serde_json::Value>(
                        "keepOnPhone",
                        serde_json::json!({ "folder": dir.display().to_string() }),
                    )
                    .await
                {
                    tracing::warn!("Shodh's folder was not kept out of this iPhone's backups: {e}");
                }
                Ok::<_, String>(Arc::new(Storage { dir, cache, key }))
            })
            .await
            .cloned()
    }
}
