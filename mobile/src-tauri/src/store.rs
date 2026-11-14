//! The phone's own files: the offline queue, the copy of the agenda, the
//! calendar sync's state and the phone's settings, each sealed
//! (`shodh_link::sealed`) with a data key kept in the Android Keystore-backed
//! storage, in the app's private folder. Android's backup is off for the app,
//! so none of it leaves the phone.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use shodh_link::sealed::{DataKey, SealedFile};
use tauri::{AppHandle, Manager};
use tokio::sync::OnceCell;

use crate::vault::KeystoreVault;

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

    /// The app's private folder for Shodh's files.
    pub fn dir(&self) -> &Path {
        &self.dir
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
                    let vault = KeystoreVault::open()?;
                    DataKey::kept_or_new(&vault)
                })
                .await?;
                Ok::<_, String>(Arc::new(Storage {
                    dir: base.join("shodh"),
                    cache,
                    key,
                }))
            })
            .await
            .cloned()
    }
}
