//! The phone's lock: a PIN (or the phone's fingerprint) to open Shodh, on by
//! default, so a lost or borrowed phone cannot ask the user's computer
//! anything. A wrong PIN five times locks it for 30 seconds; the count and the
//! lock survive a kill of the app, and a clock set back keeps it locked rather
//! than reopening it.
//!
//! The PIN is kept only as a PBKDF2-HMAC-SHA256 hash with its own salt, in the
//! phone's secure storage ([`SecretVault`]).

use std::num::NonZeroU32;

use ring::pbkdf2;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use crate::phone::SecretVault;

/// The vault entry of the lock (JSON).
pub const LOCK_ENTRY: &str = "lock";

/// Wrong PINs in a row before the lock waits.
pub const MAX_TRIES: u32 = 5;

/// How long it waits then.
pub const WAIT_MS: u64 = 30_000;

/// The shortest and longest PIN.
pub const MIN_PIN: usize = 4;
pub const MAX_PIN: usize = 12;

const ITERATIONS: u32 = 120_000;

/// What the lock keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LockState {
    /// The lock is on (it is unless the user turned it off with the PIN).
    pub enabled: bool,
    /// The PIN's salt and hash, hex; none until the user sets one.
    pub salt: Option<String>,
    pub hash: Option<String>,
    /// The phone's fingerprint (or face) also opens it.
    pub biometric: bool,
    /// Wrong PINs in a row.
    pub failures: u32,
    /// Unix milliseconds until which no PIN is tried.
    pub waits_until: u64,
    /// The last time the lock was asked (a clock set back before it keeps it waiting).
    pub last_seen: u64,
}

impl Default for LockState {
    fn default() -> Self {
        Self {
            enabled: true,
            salt: None,
            hash: None,
            biometric: false,
            failures: 0,
            waits_until: 0,
            last_seen: 0,
        }
    }
}

/// What trying a PIN found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LockError {
    #[error("That is not your PIN. {left} more tries before Shodh waits 30 seconds.")]
    Wrong { left: u32 },
    #[error("Too many wrong PINs. Try again in {seconds} seconds.")]
    Waiting { seconds: u64 },
    #[error("A PIN is {MIN_PIN} to {MAX_PIN} digits.")]
    NotAPin,
    #[error("Set a PIN first.")]
    NoPin,
    #[error("{0}")]
    Storage(String),
}

fn hex(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(bytes)
}

fn hash_pin(pin: &str, salt: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(ITERATIONS).unwrap_or(NonZeroU32::MIN),
        salt,
        pin.as_bytes(),
        &mut out,
    );
    out
}

fn is_pin(pin: &str) -> bool {
    (MIN_PIN..=MAX_PIN).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
}

/// The phone's lock over its secure storage.
pub struct Lock<'a> {
    vault: &'a dyn SecretVault,
}

impl<'a> Lock<'a> {
    pub fn new(vault: &'a dyn SecretVault) -> Self {
        Self { vault }
    }

    /// The lock as kept (on, with no PIN, the first time).
    pub fn state(&self) -> Result<LockState, LockError> {
        match self.vault.load(LOCK_ENTRY).map_err(LockError::Storage)? {
            None => Ok(LockState::default()),
            Some(bytes) => serde_json::from_slice(&bytes)
                // A lock that cannot be read stays on, with nothing to open it but a new PIN
                // after starting over: never open.
                .map_err(|_| {
                    LockError::Storage("Shodh's lock on this phone could not be read.".into())
                }),
        }
    }

    fn save(&self, state: &LockState) -> Result<(), LockError> {
        let bytes = serde_json::to_vec(state).map_err(|e| LockError::Storage(e.to_string()))?;
        self.vault
            .store(LOCK_ENTRY, &bytes)
            .map_err(LockError::Storage)
    }

    /// Whether Shodh asks for the PIN (or fingerprint) on open.
    pub fn required(&self) -> Result<bool, LockError> {
        Ok(self.state()?.enabled)
    }

    /// Set (or change) the PIN; the lock is on from now.
    pub fn set_pin(&self, pin: &str) -> Result<(), LockError> {
        if !is_pin(pin) {
            return Err(LockError::NotAPin);
        }
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|e| LockError::Storage(e.to_string()))?;
        let mut state = self.state()?;
        state.enabled = true;
        state.salt = Some(hex(&salt));
        state.hash = Some(hex(&hash_pin(pin, &salt)));
        state.failures = 0;
        state.waits_until = 0;
        self.save(&state)
    }

    /// Try `pin` at `now_ms`. A right PIN opens it and clears the count.
    pub fn try_pin(&self, pin: &str, now_ms: u64) -> Result<(), LockError> {
        let mut state = self.state()?;
        let (Some(salt), Some(hash)) = (state.salt.clone(), state.hash.clone()) else {
            return Err(LockError::NoPin);
        };
        // The clock was set back: wait from now, whatever was counted.
        if now_ms < state.last_seen {
            state.waits_until = now_ms.saturating_add(WAIT_MS);
        }
        state.last_seen = state.last_seen.max(now_ms);
        if now_ms < state.waits_until {
            let seconds = (state.waits_until - now_ms).div_ceil(1000);
            self.save(&state)?;
            return Err(LockError::Waiting { seconds });
        }
        let salt = data_encoding::HEXLOWER
            .decode(salt.as_bytes())
            .map_err(|_| LockError::Storage("Shodh's lock on this phone is damaged.".into()))?;
        let expected = data_encoding::HEXLOWER
            .decode(hash.as_bytes())
            .map_err(|_| LockError::Storage("Shodh's lock on this phone is damaged.".into()))?;
        let computed = hash_pin(pin, &salt);
        let right = is_pin(pin) && bool::from(computed[..].ct_eq(&expected[..]));
        if right {
            state.failures = 0;
            state.waits_until = 0;
            self.save(&state)?;
            return Ok(());
        }
        state.failures += 1;
        if state.failures >= MAX_TRIES {
            state.failures = 0;
            state.waits_until = now_ms.saturating_add(WAIT_MS);
            self.save(&state)?;
            return Err(LockError::Waiting {
                seconds: WAIT_MS / 1000,
            });
        }
        let left = MAX_TRIES - state.failures;
        self.save(&state)?;
        Err(LockError::Wrong { left })
    }

    /// Turn the lock off (the right PIN first) or on.
    pub fn set_enabled(&self, enabled: bool, pin: &str, now_ms: u64) -> Result<(), LockError> {
        self.try_pin(pin, now_ms)?;
        let mut state = self.state()?;
        state.enabled = enabled;
        self.save(&state)
    }

    /// Let the phone's fingerprint open it too (the right PIN first).
    pub fn set_biometric(&self, on: bool, pin: &str, now_ms: u64) -> Result<(), LockError> {
        self.try_pin(pin, now_ms)?;
        let mut state = self.state()?;
        state.biometric = on;
        self.save(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sealed::tests::MemoryVault;

    #[test]
    fn the_lock_is_required_on_open_by_default() {
        let vault = MemoryVault::default();
        let lock = Lock::new(&vault);
        assert!(lock.required().unwrap(), "on before any PIN is set");
        assert_eq!(
            lock.try_pin("1234", 1),
            Err(LockError::NoPin),
            "nothing opens it without a PIN"
        );
        lock.set_pin("482913").unwrap();
        assert!(lock.required().unwrap());
        assert_eq!(lock.try_pin("482913", 2), Ok(()));
        // Kept as a salted hash, never the PIN.
        let kept = String::from_utf8(vault.load(LOCK_ENTRY).unwrap().unwrap()).unwrap();
        assert!(!kept.contains("482913"));
        // Off only with the PIN.
        assert!(lock.set_enabled(false, "000000", 3).is_err());
        assert!(lock.required().unwrap());
        lock.set_enabled(false, "482913", 4).unwrap();
        assert!(!lock.required().unwrap());
        for bad in ["123", "12345678901234", "12a4", ""] {
            assert_eq!(lock.set_pin(bad), Err(LockError::NotAPin), "{bad}");
        }
    }

    #[test]
    fn a_wrong_pin_five_times_locks_for_30_seconds() {
        let vault = MemoryVault::default();
        Lock::new(&vault).set_pin("482913").unwrap();
        let t = 1_000_000;
        for n in 1..MAX_TRIES {
            assert_eq!(
                Lock::new(&vault).try_pin("000000", t + u64::from(n)),
                Err(LockError::Wrong {
                    left: MAX_TRIES - n
                })
            );
        }
        assert_eq!(
            Lock::new(&vault).try_pin("000000", t + 5),
            Err(LockError::Waiting { seconds: 30 })
        );
        // The app is killed and opened again: still waiting, even for the right PIN.
        let again = Lock::new(&vault);
        assert_eq!(
            again.try_pin("482913", t + 5 + 10_000),
            Err(LockError::Waiting { seconds: 20 })
        );
        // After 30 seconds the right PIN opens it.
        assert_eq!(again.try_pin("482913", t + 5 + WAIT_MS), Ok(()));
    }

    #[test]
    fn a_clock_set_back_keeps_it_waiting() {
        let vault = MemoryVault::default();
        let lock = Lock::new(&vault);
        lock.set_pin("482913").unwrap();
        let now = 200_000_000;
        assert_eq!(lock.try_pin("482913", now), Ok(()));
        // The phone's clock goes back a day: no PIN is tried for 30 seconds.
        let back = now - 86_400_000;
        assert_eq!(
            lock.try_pin("482913", back),
            Err(LockError::Waiting { seconds: 30 })
        );
        assert_eq!(lock.try_pin("482913", now + 1), Ok(()));
    }

    #[test]
    fn a_damaged_lock_never_opens() {
        let vault = MemoryVault::default();
        vault.store(LOCK_ENTRY, b"{not json").unwrap();
        let lock = Lock::new(&vault);
        assert!(lock.state().is_err());
        assert!(lock.try_pin("482913", 1).is_err());
    }
}
