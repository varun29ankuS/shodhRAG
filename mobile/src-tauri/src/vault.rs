//! The phone's secure storage for the link ([`SecretVault`]): its node key
//! and the computer it paired with.
//!
//! Android's Keystore holds keys, not arbitrary secrets, so the values live in
//! the app's private SharedPreferences, encrypted with AES-GCM by a key that
//! the Android Keystore generates, keeps (in hardware where the phone has it)
//! and never exports. `android-native-keyring-store` does this (by the
//! keyring-rs maintainers), with the Android context that Tauri's runtime
//! (tao) sets up when the app starts. No Tauri plugin keeps secrets in the
//! Keystore without a biometric prompt on every read, which would stop the
//! phone reconnecting on its own.

use std::collections::HashMap;
use std::sync::Arc;

use android_native_keyring_store::Store;
use keyring_core::api::CredentialStoreApi;
use keyring_core::{Entry, Error};
use shodh_link::phone::SecretVault;

/// The keyring store: its SharedPreferences file and its Keystore key.
const STORE_NAME: &str = "shodh-link";

/// The service every entry is filed under.
const SERVICE: &str = "shodh";

pub struct KeystoreVault {
    store: Arc<Store>,
}

impl KeystoreVault {
    pub fn open() -> Result<Self, String> {
        let configuration = HashMap::from([("name", STORE_NAME)]);
        let store = Store::new_with_configuration(&configuration)
            .map_err(|e| format!("This phone's secure storage could not be opened: {e}"))?;
        Ok(Self { store })
    }

    fn entry(&self, name: &str) -> Result<Entry, String> {
        self.store
            .build(SERVICE, name, None)
            .map_err(|e| format!("This phone's secure storage refused {name}: {e}"))
    }
}

impl SecretVault for KeystoreVault {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        match self.entry(name)?.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(format!(
                "This phone's secure storage could not read {name}: {e}"
            )),
        }
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<(), String> {
        self.entry(name)?
            .set_secret(value)
            .map_err(|e| format!("This phone's secure storage could not keep {name}: {e}"))
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(format!(
                "This phone's secure storage could not remove {name}: {e}"
            )),
        }
    }
}
