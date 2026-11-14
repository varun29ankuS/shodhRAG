//! The phone's private files, sealed: the offline queue, the copy of the
//! agenda and tasks, the calendar sync's state and the phone's own settings.
//!
//! Each file is JSON sealed with ChaCha20-Poly1305 under one data key, which
//! is kept in the phone's secure storage ([`SecretVault`], the Android
//! Keystore in Shodh's phone app) and never written next to the files. The
//! file's name is bound in as associated data, so one sealed file can never
//! be passed off as another. Writes are atomic: a temporary file, flushed to
//! disk, then renamed over the old one, so a kill mid-write leaves the
//! previous version.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::phone::SecretVault;

/// The vault entry of the data key (32 bytes).
pub const DATA_KEY_ENTRY: &str = "data-key";

/// What a sealed file starts with: the format and its version.
const MAGIC: &[u8; 4] = b"SHS1";

/// The key the phone's files are sealed with.
pub struct DataKey {
    bytes: [u8; 32],
}

impl std::fmt::Debug for DataKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DataKey([REDACTED])")
    }
}

impl DataKey {
    /// The kept data key, or a new one kept when there is none. A key that
    /// cannot be read, or is damaged, is an error and never replaced: a new
    /// key would make every sealed file unreadable.
    pub fn kept_or_new(vault: &dyn SecretVault) -> Result<Self, String> {
        match vault.load(DATA_KEY_ENTRY)? {
            Some(bytes) => {
                let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
                    "The key of this phone's Shodh files in secure storage is damaged.".to_string()
                })?;
                Ok(Self { bytes })
            }
            None => {
                let mut bytes = [0u8; 32];
                getrandom::fill(&mut bytes)
                    .map_err(|e| format!("This phone's random number source failed: {e}"))?;
                vault.store(DATA_KEY_ENTRY, &bytes)?;
                Ok(Self { bytes })
            }
        }
    }

    /// A key from its bytes (tests, and a key read some other way).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    fn aead(&self) -> LessSafeKey {
        // A 32-byte key is always valid for ChaCha20-Poly1305.
        LessSafeKey::new(
            UnboundKey::new(&CHACHA20_POLY1305, &self.bytes)
                .unwrap_or_else(|_| unreachable!("a 32-byte ChaCha20-Poly1305 key")),
        )
    }
}

/// Why a sealed file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    #[error("Shodh's file {0} on this phone could not be read: {1}")]
    Io(String, String),
    /// Not sealed with this key, or changed since: never read as data.
    #[error("Shodh's file {0} on this phone is damaged or was not made by this phone.")]
    Damaged(String),
    #[error("Shodh's file {0} on this phone holds data this version cannot read: {1}")]
    Format(String, String),
}

/// One sealed file.
pub struct SealedFile {
    path: PathBuf,
    name: String,
    key: LessSafeKey,
}

impl SealedFile {
    /// The file `name` in `dir`, sealed with `key`. `name` is a plain file
    /// name ("outbox.sealed").
    pub fn new(dir: &Path, name: &str, key: &DataKey) -> Self {
        Self {
            path: dir.join(name),
            name: name.to_string(),
            key: key.aead(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file's value; `Ok(None)` when it was never written.
    pub fn read<T: DeserializeOwned>(&self) -> Result<Option<T>, SealError> {
        let sealed = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SealError::Io(self.name.clone(), e.to_string())),
        };
        let plain = self.open(sealed)?;
        serde_json::from_slice(&plain)
            .map(Some)
            .map_err(|e| SealError::Format(self.name.clone(), e.to_string()))
    }

    /// Seal `value` and replace the file with it, atomically.
    pub fn write<T: Serialize>(&self, value: &T) -> Result<(), String> {
        let plain = serde_json::to_vec(value)
            .map_err(|e| format!("Shodh's file {} could not be written: {e}", self.name))?;
        let sealed = self.seal(plain)?;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)
                .map_err(|e| format!("Shodh's folder on this phone could not be made: {e}"))?;
        }
        let temporary = self.path.with_extension("partial");
        let written = (|| {
            let mut file = fs::File::create(&temporary)?;
            file.write_all(&sealed)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        })();
        written.map_err(|e| {
            let _ = fs::remove_file(&temporary);
            format!("Shodh's file {} could not be saved: {e}", self.name)
        })
    }

    /// Remove the file; removing what is not there is not an error.
    pub fn remove(&self) -> Result<(), String> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!(
                "Shodh's file {} could not be removed: {e}",
                self.name
            )),
        }
    }

    fn seal(&self, mut plain: Vec<u8>) -> Result<Vec<u8>, String> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce)
            .map_err(|e| format!("This phone's random number source failed: {e}"))?;
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(self.name.as_bytes()),
                &mut plain,
            )
            .map_err(|_| format!("Shodh's file {} could not be sealed", self.name))?;
        let mut sealed = Vec::with_capacity(MAGIC.len() + NONCE_LEN + plain.len());
        sealed.extend_from_slice(MAGIC);
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&plain);
        Ok(sealed)
    }

    fn open(&self, sealed: Vec<u8>) -> Result<Vec<u8>, SealError> {
        let damaged = || SealError::Damaged(self.name.clone());
        if sealed.len() < MAGIC.len() + NONCE_LEN || &sealed[..MAGIC.len()] != MAGIC {
            return Err(damaged());
        }
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&sealed[MAGIC.len()..MAGIC.len() + NONCE_LEN]);
        let mut body = sealed[MAGIC.len() + NONCE_LEN..].to_vec();
        let plain = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(self.name.as_bytes()),
                &mut body,
            )
            .map_err(|_| damaged())?;
        Ok(plain.to_vec())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A vault in memory, for the phone-side tests.
    #[derive(Default)]
    pub(crate) struct MemoryVault {
        pub(crate) entries: Mutex<HashMap<String, Vec<u8>>>,
        pub(crate) broken: bool,
    }

    impl SecretVault for MemoryVault {
        fn load(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
            if self.broken {
                return Err("the Keystore is locked".into());
            }
            Ok(self.entries.lock().unwrap().get(name).cloned())
        }
        fn store(&self, name: &str, value: &[u8]) -> Result<(), String> {
            self.entries
                .lock()
                .unwrap()
                .insert(name.to_string(), value.to_vec());
            Ok(())
        }
        fn remove(&self, name: &str) -> Result<(), String> {
            self.entries.lock().unwrap().remove(name);
            Ok(())
        }
    }

    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "shodh-link-{name}-{}",
            data_encoding::HEXLOWER.encode(&random)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_sealed_file_round_trips_and_holds_no_plain_text() {
        let dir = temp_dir("sealed");
        let key = DataKey::from_bytes([7; 32]);
        let file = SealedFile::new(&dir, "agenda.sealed", &key);
        assert_eq!(file.read::<Vec<String>>().unwrap(), None);
        let agenda = vec!["Meet Asha about Self-RAG".to_string()];
        file.write(&agenda).unwrap();
        assert_eq!(file.read::<Vec<String>>().unwrap(), Some(agenda));
        let raw = fs::read(file.path()).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("Self-RAG"));
        assert!(
            !dir.join("agenda.partial").exists(),
            "no temporary file is left"
        );
    }

    #[test]
    fn a_file_sealed_with_another_key_or_name_is_damaged() {
        let dir = temp_dir("sealed-keys");
        let file = SealedFile::new(&dir, "outbox.sealed", &DataKey::from_bytes([1; 32]));
        file.write(&42u32).unwrap();
        let stranger = SealedFile::new(&dir, "outbox.sealed", &DataKey::from_bytes([2; 32]));
        assert!(matches!(stranger.read::<u32>(), Err(SealError::Damaged(_))));
        // The same bytes under another name are refused too.
        fs::copy(dir.join("outbox.sealed"), dir.join("agenda.sealed")).unwrap();
        let renamed = SealedFile::new(&dir, "agenda.sealed", &DataKey::from_bytes([1; 32]));
        assert!(matches!(renamed.read::<u32>(), Err(SealError::Damaged(_))));
        // A changed byte is refused.
        let mut raw = fs::read(dir.join("outbox.sealed")).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        fs::write(dir.join("outbox.sealed"), raw).unwrap();
        assert!(matches!(file.read::<u32>(), Err(SealError::Damaged(_))));
    }

    #[test]
    fn the_data_key_is_kept_and_never_replaced_on_a_read_error() {
        let vault = MemoryVault::default();
        let first = DataKey::kept_or_new(&vault).unwrap();
        let again = DataKey::kept_or_new(&vault).unwrap();
        assert_eq!(first.bytes, again.bytes);
        let broken = MemoryVault {
            broken: true,
            ..MemoryVault::default()
        };
        assert!(DataKey::kept_or_new(&broken).is_err());
        assert!(broken.entries.lock().unwrap().is_empty(), "no new key");
        let damaged = MemoryVault::default();
        damaged.store(DATA_KEY_ENTRY, &[1, 2, 3]).unwrap();
        assert!(DataKey::kept_or_new(&damaged).is_err());
        assert_eq!(
            damaged.load(DATA_KEY_ENTRY).unwrap(),
            Some(vec![1, 2, 3]),
            "the damaged key is left for the user to see"
        );
        assert_eq!(format!("{first:?}"), "DataKey([REDACTED])");
    }
}
