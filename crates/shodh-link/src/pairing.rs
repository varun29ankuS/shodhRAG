//! Pairing and the paired phones.
//!
//! The desktop shows a QR code ([`PairingOffer::to_qr`]): its node id, its
//! relay, a one-time secret and when the secret stops working. The phone dials
//! the node id (so only that desktop can answer: iroh authenticates both ends
//! by their keys) and sends the secret in [`crate::messages::Request::Pair`].
//! A secret works once, for [`PAIRING_TTL_MS`]; a new offer replaces the
//! previous one. The phone that redeemed it is identified by its own node id
//! as the connection authenticated it, never by anything it says, and from
//! then on only paired node ids are let in ([`DeviceRegistry`]).

use std::sync::Mutex;

use iroh::{EndpointId, RelayUrl};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// How long a pairing secret works: five minutes.
pub const PAIRING_TTL_MS: u64 = 5 * 60 * 1000;

/// The scheme of the QR string, with its format version.
pub const QR_PREFIX: &str = "shodh-pair:1:";

/// Bytes of a pairing secret.
pub const SECRET_BYTES: usize = 32;

/// Most phones paired with one desktop. The list is kept in the OS
/// credential store, whose entries are small (Windows: 2,560 bytes).
pub const MAX_DEVICES: usize = 5;

/// Longest phone name kept, in characters.
pub const MAX_DEVICE_NAME_CHARS: usize = 40;

/// What the QR code carries.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingOffer {
    pub node_id: EndpointId,
    /// The desktop's home relay, so the phone reaches it before address
    /// lookup answers. `None` while the desktop has no relay.
    pub relay_url: Option<RelayUrl>,
    pub secret: [u8; SECRET_BYTES],
    /// Unix time in milliseconds.
    pub expires_at: u64,
}

/// Never the secret: an offer printed in a log must not pair anyone.
impl std::fmt::Debug for PairingOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingOffer")
            .field("node_id", &self.node_id)
            .field("relay_url", &self.relay_url)
            .field("secret", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Why a QR string is not a Shodh pairing code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QrError {
    #[error("This is not a Shodh pairing code.")]
    NotShodh,
    #[error("This pairing code is from a newer Shodh. Update Shodh on your phone.")]
    NewerFormat,
    #[error("This pairing code is damaged. Make a new one on your computer.")]
    Damaged,
}

impl PairingOffer {
    /// `shodh-pair:1:<base32>`, where the base32 (RFC 4648, no padding) is the
    /// node id (32 bytes), the secret (32), the expiry (8, big-endian
    /// milliseconds) and the relay URL (UTF-8, the rest; empty for none).
    pub fn to_qr(&self) -> String {
        let mut bytes = Vec::with_capacity(72 + 64);
        bytes.extend_from_slice(self.node_id.as_bytes());
        bytes.extend_from_slice(&self.secret);
        bytes.extend_from_slice(&self.expires_at.to_be_bytes());
        if let Some(relay) = &self.relay_url {
            bytes.extend_from_slice(relay.to_string().as_bytes());
        }
        format!("{QR_PREFIX}{}", data_encoding::BASE32_NOPAD.encode(&bytes))
    }

    /// The offer in a scanned QR string.
    pub fn from_qr(text: &str) -> Result<Self, QrError> {
        let text = text.trim();
        let Some(rest) = text.strip_prefix("shodh-pair:") else {
            return Err(QrError::NotShodh);
        };
        let Some(body) = rest.strip_prefix("1:") else {
            return Err(QrError::NewerFormat);
        };
        let bytes = data_encoding::BASE32_NOPAD
            .decode(body.as_bytes())
            .map_err(|_| QrError::Damaged)?;
        if bytes.len() < 72 {
            return Err(QrError::Damaged);
        }
        let node: [u8; 32] = bytes[..32].try_into().map_err(|_| QrError::Damaged)?;
        let node_id = EndpointId::from_bytes(&node).map_err(|_| QrError::Damaged)?;
        let secret: [u8; SECRET_BYTES] = bytes[32..64].try_into().map_err(|_| QrError::Damaged)?;
        let expiry: [u8; 8] = bytes[64..72].try_into().map_err(|_| QrError::Damaged)?;
        let relay_url = match &bytes[72..] {
            [] => None,
            relay => {
                let text = std::str::from_utf8(relay).map_err(|_| QrError::Damaged)?;
                Some(text.parse::<RelayUrl>().map_err(|_| QrError::Damaged)?)
            }
        };
        Ok(Self {
            node_id,
            relay_url,
            secret,
            expires_at: u64::from_be_bytes(expiry),
        })
    }
}

/// The secret as the phone sends it ([`crate::messages::Request::Pair`]).
pub fn encode_secret(secret: &[u8; SECRET_BYTES]) -> String {
    data_encoding::BASE32_NOPAD.encode(secret)
}

/// The secret a phone sent; `None` when it is not one.
pub fn decode_secret(text: &str) -> Option<[u8; SECRET_BYTES]> {
    data_encoding::BASE32_NOPAD
        .decode(text.trim().as_bytes())
        .ok()?
        .try_into()
        .ok()
}

/// A new random pairing secret, from the operating system's random source.
pub fn new_secret() -> Result<[u8; SECRET_BYTES], String> {
    let mut secret = [0u8; SECRET_BYTES];
    getrandom::fill(&mut secret)
        .map_err(|e| format!("The computer's random number source failed: {e}"))?;
    Ok(secret)
}

/// Why a pairing secret was refused, in the words the phone shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PairingRefusal {
    #[error(
        "This pairing code expired. Make a new one in Shodh on your computer: Settings, Phone."
    )]
    Expired,
    #[error("This pairing code was already used. Make a new one in Shodh on your computer: Settings, Phone.")]
    Used,
    #[error("This pairing code is not the one your computer shows now. Scan the code in Settings, Phone.")]
    Unknown,
}

struct Pending {
    secret: [u8; SECRET_BYTES],
    expires_at: u64,
    used: bool,
}

/// Never the secret.
impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("secret", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("used", &self.used)
            .finish()
    }
}

/// Equal in constant time: how long the comparison takes says nothing about
/// how much of a guess was right.
fn same_secret(a: &[u8; SECRET_BYTES], b: &[u8; SECRET_BYTES]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

/// The desktop's current pairing offer: one secret at a time.
#[derive(Debug, Default)]
pub struct PendingPairing {
    current: Mutex<Option<Pending>>,
}

impl PendingPairing {
    /// Start a pairing with `secret`, working until `now + PAIRING_TTL_MS`.
    /// The previous secret, used or not, stops working. Returns the expiry.
    pub fn issue(&self, secret: [u8; SECRET_BYTES], now: u64) -> u64 {
        let expires_at = now.saturating_add(PAIRING_TTL_MS);
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
            secret,
            expires_at,
            used: false,
        });
        expires_at
    }

    /// Use `secret` at `now`. It works once: the first correct attempt
    /// consumes it under the lock, so two phones racing with the same code
    /// cannot both pair.
    pub fn redeem(&self, secret: &[u8; SECRET_BYTES], now: u64) -> Result<(), PairingRefusal> {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pending) = current.as_mut() else {
            return Err(PairingRefusal::Unknown);
        };
        if !same_secret(&pending.secret, secret) {
            return Err(PairingRefusal::Unknown);
        }
        if pending.used {
            return Err(PairingRefusal::Used);
        }
        if now >= pending.expires_at {
            return Err(PairingRefusal::Expired);
        }
        pending.used = true;
        Ok(())
    }

    /// Whether a pairing could still be redeemed at `now` (the desktop keeps
    /// listening for phones while one can).
    pub fn open(&self, now: u64) -> bool {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|p| !p.used && now < p.expires_at)
    }

    /// Withdraw the current offer.
    pub fn cancel(&self) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// A paired phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    /// The phone's node id (hex), as its connection authenticated it.
    pub node_id: String,
    pub name: String,
    /// Unix time in milliseconds.
    pub paired_at: u64,
    pub last_seen: Option<u64>,
}

/// A phone name fit to store and show: control characters dropped, spaces
/// collapsed, at most [`MAX_DEVICE_NAME_CHARS`] characters, "Phone" when
/// nothing is left.
pub fn clean_device_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_DEVICE_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "Phone".to_string()
    } else {
        cleaned
    }
}

/// Where the paired phones are kept (the desktop's OS credential store; an
/// in-memory store in tests).
pub trait DeviceStore: Send + Sync {
    fn load(&self) -> Result<Vec<PairedDevice>, String>;
    fn save(&self, devices: &[PairedDevice]) -> Result<(), String>;
}

/// Why a phone could not be added.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddDeviceError {
    #[error("{MAX_DEVICES} phones are paired already. Remove one in Shodh on your computer: Settings, Phone.")]
    Full,
    #[error("Your computer could not save this phone: {0}")]
    Store(String),
}

/// The paired phones: the allowlist of node ids the desktop lets in.
/// Removing one tells every live connection of it to close
/// ([`DeviceRegistry::revocations`]).
pub struct DeviceRegistry {
    store: Box<dyn DeviceStore>,
    devices: Mutex<Vec<PairedDevice>>,
    revoked: broadcast::Sender<String>,
}

impl DeviceRegistry {
    /// The registry over `store`, loaded once.
    pub fn open(store: Box<dyn DeviceStore>) -> Result<Self, String> {
        let devices = store.load()?;
        let (revoked, _) = broadcast::channel(16);
        Ok(Self {
            store,
            devices: Mutex::new(devices),
            revoked,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<PairedDevice>> {
        self.devices.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn list(&self) -> Vec<PairedDevice> {
        self.lock().clone()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// The paired phone with node id `node_id` (hex).
    pub fn get(&self, node_id: &str) -> Option<PairedDevice> {
        self.lock().iter().find(|d| d.node_id == node_id).cloned()
    }

    /// Whether a phone could be added now (checked before a pairing secret is
    /// used, so a full list does not waste it).
    pub fn has_room_for(&self, node_id: &str) -> bool {
        let devices = self.lock();
        devices.len() < MAX_DEVICES || devices.iter().any(|d| d.node_id == node_id)
    }

    /// Add (or rename, when it is paired already) the phone `node_id`.
    pub fn add(&self, node_id: &str, name: &str, now: u64) -> Result<PairedDevice, AddDeviceError> {
        let mut devices = self.lock();
        let mut next = devices.clone();
        let device = PairedDevice {
            node_id: node_id.to_string(),
            name: clean_device_name(name),
            paired_at: now,
            last_seen: Some(now),
        };
        match next.iter_mut().find(|d| d.node_id == node_id) {
            Some(existing) => *existing = device.clone(),
            None if next.len() >= MAX_DEVICES => return Err(AddDeviceError::Full),
            None => next.push(device.clone()),
        }
        self.store.save(&next).map_err(AddDeviceError::Store)?;
        *devices = next;
        Ok(device)
    }

    /// Remove the phone `node_id`. Its live connections are told to close,
    /// and it is refused from now on. Returns whether it was paired.
    pub fn remove(&self, node_id: &str) -> Result<bool, String> {
        let mut devices = self.lock();
        let next: Vec<PairedDevice> = devices
            .iter()
            .filter(|d| d.node_id != node_id)
            .cloned()
            .collect();
        if next.len() == devices.len() {
            return Ok(false);
        }
        self.store.save(&next)?;
        *devices = next;
        drop(devices);
        // No receiver (no live connection) is not an error.
        let _ = self.revoked.send(node_id.to_string());
        Ok(true)
    }

    /// Record that `node_id` connected at `now`. A failure to save is only
    /// logged: it never refuses a paired phone.
    pub fn touch(&self, node_id: &str, now: u64) {
        let mut devices = self.lock();
        let mut next = devices.clone();
        let Some(device) = next.iter_mut().find(|d| d.node_id == node_id) else {
            return;
        };
        device.last_seen = Some(now);
        match self.store.save(&next) {
            Ok(()) => *devices = next,
            Err(e) => {
                tracing::warn!(target: "shodh::link", error = %e, "the phone's last connection time was not saved")
            }
        }
    }

    /// Node ids of phones removed from now on.
    pub fn revocations(&self) -> broadcast::Receiver<String> {
        self.revoked.subscribe()
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// An in-memory device store; `fail` makes every save fail.
    #[derive(Default)]
    pub struct MemoryStore {
        pub saved: Mutex<Vec<PairedDevice>>,
        pub fail: bool,
    }

    impl DeviceStore for MemoryStore {
        fn load(&self) -> Result<Vec<PairedDevice>, String> {
            Ok(self.saved.lock().unwrap().clone())
        }
        fn save(&self, devices: &[PairedDevice]) -> Result<(), String> {
            if self.fail {
                return Err("the store refused".into());
            }
            *self.saved.lock().unwrap() = devices.to_vec();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::MemoryStore;
    use super::*;

    fn secret(byte: u8) -> [u8; SECRET_BYTES] {
        // Built at run time, so no secret-shaped literal is in the source.
        let mut s = new_secret().unwrap();
        s[0] = byte;
        s
    }

    fn node() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    #[test]
    fn the_qr_string_round_trips() {
        let relay: RelayUrl = "https://relay.example.org./".parse().unwrap();
        for relay_url in [None, Some(relay)] {
            let offer = PairingOffer {
                node_id: node(),
                relay_url,
                secret: secret(1),
                expires_at: 1_763_000_000_000,
            };
            let qr = offer.to_qr();
            assert!(qr.starts_with("shodh-pair:1:"), "{qr}");
            assert!(qr[QR_PREFIX.len()..]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
            assert_eq!(PairingOffer::from_qr(&qr).unwrap(), offer);
        }
        assert_eq!(
            PairingOffer::from_qr("https://example.org"),
            Err(QrError::NotShodh)
        );
        assert_eq!(
            PairingOffer::from_qr("shodh-pair:2:AAAA"),
            Err(QrError::NewerFormat)
        );
        assert_eq!(
            PairingOffer::from_qr("shodh-pair:1:AAAA"),
            Err(QrError::Damaged)
        );
        assert_eq!(
            PairingOffer::from_qr("shodh-pair:1:not base32!"),
            Err(QrError::Damaged)
        );
    }

    #[test]
    fn a_secret_round_trips_as_the_phone_sends_it() {
        let s = secret(9);
        assert_eq!(decode_secret(&encode_secret(&s)), Some(s));
        assert_eq!(decode_secret("AAAA"), None);
        assert_ne!(new_secret().unwrap(), new_secret().unwrap());
    }

    #[test]
    fn an_expired_pairing_secret_is_refused() {
        let pending = PendingPairing::default();
        let s = secret(1);
        let expires = pending.issue(s, 1_000);
        assert_eq!(expires, 1_000 + PAIRING_TTL_MS);
        assert!(pending.open(expires - 1));
        assert!(!pending.open(expires));
        assert_eq!(pending.redeem(&s, expires), Err(PairingRefusal::Expired));
        assert_eq!(
            pending.redeem(&s, expires + 60_000),
            Err(PairingRefusal::Expired)
        );
        // A secret refused for expiry is not consumed, but it stays expired.
        assert_eq!(pending.redeem(&s, expires), Err(PairingRefusal::Expired));
    }

    #[test]
    fn a_used_pairing_secret_is_refused() {
        let pending = PendingPairing::default();
        let s = secret(2);
        pending.issue(s, 1_000);
        assert_eq!(pending.redeem(&s, 2_000), Ok(()));
        assert!(!pending.open(2_000), "a used offer is closed");
        // The same QR code scanned again (or replayed) is refused.
        assert_eq!(pending.redeem(&s, 2_001), Err(PairingRefusal::Used));
    }

    #[test]
    fn an_unknown_pairing_secret_is_refused() {
        let pending = PendingPairing::default();
        let s = secret(3);
        assert_eq!(pending.redeem(&s, 0), Err(PairingRefusal::Unknown));
        pending.issue(s, 1_000);
        let mut guess = s;
        guess[SECRET_BYTES - 1] ^= 1;
        assert_eq!(pending.redeem(&guess, 1_001), Err(PairingRefusal::Unknown));
        // A wrong guess does not use the real secret up.
        assert_eq!(pending.redeem(&s, 1_002), Ok(()));
        // A new offer replaces the old one: the old code stops working.
        let old = secret(4);
        pending.issue(old, 2_000);
        let new = secret(5);
        pending.issue(new, 2_001);
        assert_eq!(pending.redeem(&old, 2_002), Err(PairingRefusal::Unknown));
        pending.cancel();
        assert_eq!(pending.redeem(&new, 2_003), Err(PairingRefusal::Unknown));
    }

    #[test]
    fn device_names_are_cleaned() {
        assert_eq!(clean_device_name("  Pixel\t8\n "), "Pixel 8");
        assert_eq!(clean_device_name("a\u{0}b\u{1b}[31m"), "ab[31m");
        assert_eq!(clean_device_name(""), "Phone");
        assert_eq!(clean_device_name("\u{7}\u{7}"), "Phone");
        assert_eq!(
            clean_device_name(&"é".repeat(100)).chars().count(),
            MAX_DEVICE_NAME_CHARS
        );
    }

    #[test]
    fn the_registry_adds_renames_and_removes() {
        let registry = DeviceRegistry::open(Box::<MemoryStore>::default()).unwrap();
        let mut revoked = registry.revocations();
        let a = node().to_string();
        registry.add(&a, "Pixel 8", 10).unwrap();
        registry.add(&a, "Pixel 8 Pro", 20).unwrap();
        assert_eq!(registry.list().len(), 1, "pairing again renames");
        assert_eq!(registry.get(&a).unwrap().name, "Pixel 8 Pro");
        registry.touch(&a, 30);
        assert_eq!(registry.get(&a).unwrap().last_seen, Some(30));
        assert!(registry.remove(&a).unwrap());
        assert!(registry.get(&a).is_none());
        assert_eq!(revoked.try_recv().unwrap(), a, "live connections are told");
        assert!(!registry.remove(&a).unwrap());
    }

    #[test]
    fn the_registry_holds_at_most_max_devices() {
        let registry = DeviceRegistry::open(Box::<MemoryStore>::default()).unwrap();
        let ids: Vec<String> = (0..MAX_DEVICES).map(|_| node().to_string()).collect();
        for id in &ids {
            registry.add(id, "Phone", 1).unwrap();
        }
        let extra = node().to_string();
        assert!(!registry.has_room_for(&extra));
        assert!(
            registry.has_room_for(&ids[0]),
            "a paired phone can pair again"
        );
        assert_eq!(registry.add(&extra, "Phone", 2), Err(AddDeviceError::Full));
    }

    #[test]
    fn a_failed_save_changes_nothing() {
        let registry = DeviceRegistry::open(Box::new(MemoryStore {
            fail: true,
            ..MemoryStore::default()
        }))
        .unwrap();
        let a = node().to_string();
        assert!(matches!(
            registry.add(&a, "Pixel", 1),
            Err(AddDeviceError::Store(_))
        ));
        assert!(registry.get(&a).is_none(), "not let in when not saved");
    }
}
