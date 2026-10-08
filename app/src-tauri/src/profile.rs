//! Which data folder this instance uses.
//!
//! Normally Shodh keeps everything in the platform app data folder. When the
//! `SHODH_DATA_DIR` environment variable names an absolute folder, this
//! instance is a separate profile (a demo or tester profile): its data,
//! WebView storage and credential-store entries all live apart from the
//! normal profile, and it runs beside a normal instance instead of handing
//! over to it. Every app data path goes through [`app_data_dir`]; the
//! variable is read once, by [`init`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sha2::{Digest, Sha256};
use tauri::webview::WebviewWindowBuilder;
use tauri::{Manager, Runtime};

/// Environment variable naming a separate profile's data folder.
pub const DATA_DIR_ENV: &str = "SHODH_DATA_DIR";

/// Credential-store service of the normal profile. Matches the Tauri bundle
/// identifier so the entries are easy to identify in the OS credential manager.
pub const BASE_KEYRING_SERVICE: &str = "com.shodh.rag-app";

/// The profile this instance runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// `None`: the normal profile in the platform app data folder.
    data_dir: Option<PathBuf>,
}

static NORMAL: Profile = Profile { data_dir: None };
static ACTIVE: OnceLock<Profile> = OnceLock::new();

impl Profile {
    /// The profile named by the value of [`DATA_DIR_ENV`]. Unset or empty is
    /// the normal profile. A relative path is an error: guessing what it is
    /// relative to could open a folder the user did not mean.
    pub fn from_env_value(value: Option<&OsStr>) -> Result<Self, String> {
        let Some(value) = value.filter(|v| !v.is_empty()) else {
            return Ok(NORMAL.clone());
        };
        let path = Path::new(value);
        if !path.is_absolute() {
            return Err(format!(
                "{DATA_DIR_ENV} must be an absolute folder path, got {}",
                path.display()
            ));
        }
        // Rebuilt from its components so `C:\demo\` and `C:\demo` are one profile.
        Ok(Self {
            data_dir: Some(path.components().collect()),
        })
    }

    /// The separate profile's data folder, or `None` for the normal profile.
    pub fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    /// Whether this is a separate profile rather than the normal one.
    pub fn is_separate(&self) -> bool {
        self.data_dir.is_some()
    }

    /// Only the normal profile is single-instance: a separate profile must be
    /// able to run beside it (the single-instance lock is per app, not per
    /// folder, so it would hand the launch over to the normal instance).
    pub fn uses_single_instance(&self) -> bool {
        !self.is_separate()
    }

    /// The credential-store service holding this profile's secrets. The
    /// normal profile keeps the original name so existing keys stay readable;
    /// a separate profile gets its own, so it never reads the normal keys.
    pub fn keyring_service(&self) -> String {
        match self.data_dir.as_deref() {
            None => BASE_KEYRING_SERVICE.to_string(),
            Some(_) => format!(
                "{BASE_KEYRING_SERVICE}.profile-{}",
                hex::encode(self.folder_id())
            ),
        }
    }

    /// WebView storage folder of a separate profile (`None`: the platform
    /// default of the normal profile).
    pub fn webview_data_dir(&self) -> Option<PathBuf> {
        self.data_dir.as_deref().map(|dir| dir.join("webview"))
    }

    /// A stable 16-byte id of the profile folder.
    fn folder_id(&self) -> [u8; 16] {
        let mut id = [0u8; 16];
        if let Some(dir) = self.data_dir.as_deref() {
            let digest = Sha256::digest(dir.as_os_str().as_encoded_bytes());
            id.copy_from_slice(&digest[..16]);
        }
        id
    }
}

/// Read [`DATA_DIR_ENV`] once and fix this instance's profile. Call before
/// anything resolves a path.
pub fn init() -> Result<&'static Profile, String> {
    let profile = Profile::from_env_value(std::env::var_os(DATA_DIR_ENV).as_deref())?;
    Ok(ACTIVE.get_or_init(|| profile))
}

/// This instance's profile.
pub fn active() -> &'static Profile {
    ACTIVE.get().unwrap_or(&NORMAL)
}

/// The folder all app data lives in: the separate profile's folder, else the
/// platform app data folder.
pub fn app_data_dir<R: Runtime, M: Manager<R>>(app: &M) -> tauri::Result<PathBuf> {
    match active().data_dir() {
        Some(dir) => Ok(dir.to_path_buf()),
        None => app.path().app_data_dir(),
    }
}

/// Keep a window's WebView storage inside a separate profile's folder
/// (Windows, Linux) or its own data store (macOS 14+). No change for the
/// normal profile.
pub fn webview_storage<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    let profile = active();
    match profile.webview_data_dir() {
        Some(dir) => builder
            .data_directory(dir)
            .data_store_identifier(profile.folder_id()),
        None => builder,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn absolute(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn unset_or_empty_is_the_normal_profile() {
        for value in [None, Some(OsString::new())] {
            let profile = Profile::from_env_value(value.as_deref()).unwrap();
            assert_eq!(profile.data_dir(), None);
            assert!(!profile.is_separate());
            assert_eq!(profile.webview_data_dir(), None);
        }
    }

    #[test]
    fn absolute_folder_is_a_separate_profile() {
        let dir = absolute("shodh-demo");
        let profile = Profile::from_env_value(Some(dir.as_os_str())).unwrap();
        assert_eq!(profile.data_dir(), Some(dir.as_path()));
        assert_eq!(profile.webview_data_dir(), Some(dir.join("webview")));
    }

    #[test]
    fn relative_folder_is_rejected() {
        let err = Profile::from_env_value(Some(OsStr::new("demo-data"))).unwrap_err();
        assert!(err.contains(DATA_DIR_ENV), "{err}");
    }

    #[test]
    fn trailing_separator_names_the_same_profile() {
        let dir = absolute("shodh-demo");
        let mut with_slash = dir.as_os_str().to_owned();
        with_slash.push(std::path::MAIN_SEPARATOR_STR);
        let a = Profile::from_env_value(Some(dir.as_os_str())).unwrap();
        let b = Profile::from_env_value(Some(&with_slash)).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.keyring_service(), b.keyring_service());
    }

    #[test]
    fn normal_profile_keeps_the_original_vault_service() {
        assert_eq!(NORMAL.keyring_service(), "com.shodh.rag-app");
    }

    #[test]
    fn separate_profiles_get_their_own_vault_service() {
        let a = Profile::from_env_value(Some(absolute("shodh-demo-a").as_os_str())).unwrap();
        let b = Profile::from_env_value(Some(absolute("shodh-demo-b").as_os_str())).unwrap();
        let service_a = a.keyring_service();
        assert!(service_a.starts_with("com.shodh.rag-app.profile-"));
        assert_ne!(service_a, BASE_KEYRING_SERVICE);
        assert_ne!(service_a, b.keyring_service());
        // Stable across runs: the same folder always finds its keys again.
        assert_eq!(service_a, a.clone().keyring_service());
    }

    #[test]
    fn only_the_normal_profile_is_single_instance() {
        assert!(NORMAL.uses_single_instance());
        let demo = Profile::from_env_value(Some(absolute("shodh-demo").as_os_str())).unwrap();
        assert!(!demo.uses_single_instance());
    }
}
