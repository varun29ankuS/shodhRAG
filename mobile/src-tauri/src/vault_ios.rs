//! The phone's secure storage for the link ([`SecretVault`]) on iPhone: its
//! node key and the computer it paired with, each a generic password in the
//! iOS Keychain.
//!
//! Every entry is kept as `AfterFirstUnlockThisDeviceOnly`: the app can read
//! it whenever the phone has been unlocked once since it started (so the
//! link reconnects on its own), and it never leaves this iPhone, in an
//! iCloud or computer backup or a transfer to a new one. A restored phone
//! therefore has no key and pairs again as a new phone, as on Android.

use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password_options, PasswordOptions,
};
use security_framework_sys::access_control::kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly;
use security_framework_sys::base::errSecItemNotFound;
use shodh_link::phone::SecretVault;

/// The Keychain service every entry is filed under.
const SERVICE: &str = "shodh-link";

#[link(name = "Security", kind = "framework")]
extern "C" {
    /// The attribute that says when an item may be read (Security's
    /// SecItem.h); `security-framework-sys` has its values but not the key.
    /// Apple's own name for it.
    #[allow(non_upper_case_globals)]
    static kSecAttrAccessible: CFStringRef;
}

pub struct Vault;

impl Vault {
    pub fn open() -> Result<Self, String> {
        Ok(Self)
    }
}

/// The words of a Keychain error: Apple's message, and its code.
fn words(error: security_framework::base::Error) -> String {
    match error.message() {
        Some(message) => format!("{message} ({})", error.code()),
        None => format!("Keychain error {}", error.code()),
    }
}

impl SecretVault for Vault {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        match get_generic_password(SERVICE, name) {
            Ok(secret) => Ok(Some(secret)),
            Err(e) if e.code() == errSecItemNotFound => Ok(None),
            Err(e) => Err(format!(
                "This iPhone's Keychain could not read {name}: {}",
                words(e)
            )),
        }
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<(), String> {
        let mut options = PasswordOptions::new_generic_password(SERVICE, name);
        // SAFETY: both are constants of the Security framework, valid for the
        // life of the process; `wrap_under_get_rule` retains them.
        let (key, this_device_only) = unsafe {
            (
                CFString::wrap_under_get_rule(kSecAttrAccessible),
                CFString::wrap_under_get_rule(kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly),
            )
        };
        // The one way to add an attribute the crate has no setter for.
        #[allow(deprecated)]
        options.query.push((key, this_device_only.into_CFType()));
        set_generic_password_options(value, options)
            .map_err(|e| format!("This iPhone's Keychain could not keep {name}: {}", words(e)))
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        match delete_generic_password(SERVICE, name) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == errSecItemNotFound => Ok(()),
            Err(e) => Err(format!(
                "This iPhone's Keychain could not remove {name}: {}",
                words(e)
            )),
        }
    }
}
