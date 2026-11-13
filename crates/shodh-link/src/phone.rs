//! What the phone keeps between starts: its own node key and the desktop it
//! paired with, both in the phone's secure storage ([`SecretVault`]: the
//! Android Keystore in Shodh's phone app), and the two ways it reaches that
//! desktop ([`pair_and_keep`] once, [`connect_to_kept`] after).
//!
//! The node key is the phone's identity: the desktop lets in only the node
//! ids it paired. Losing it silently would unpair the phone, so a key that
//! cannot be read is an error, never a reason to make a new one.

use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl, SecretKey};
use serde::{Deserialize, Serialize};

use crate::client::{offer_addr, ConnectError, LinkClient};
use crate::pairing::PairingOffer;

/// The vault entry of the phone's node key (32 bytes).
pub const NODE_KEY_ENTRY: &str = "node-key";

/// The vault entry of the paired desktop ([`PairedDesktop`], JSON).
pub const DESKTOP_ENTRY: &str = "desktop";

/// The phone's secure storage: small named secrets.
pub trait SecretVault: Send + Sync {
    /// The secret named `name`; `Ok(None)` only when none was ever stored (or
    /// it was removed). A vault that cannot be read is an `Err`.
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, String>;
    fn store(&self, name: &str, value: &[u8]) -> Result<(), String>;
    /// Removing what is not there is not an error.
    fn remove(&self, name: &str) -> Result<(), String>;
}

/// The phone's node key: the kept one, or a new one kept when none is. Any
/// failure to read it is an error, never a new key.
pub fn phone_key(vault: &dyn SecretVault) -> Result<SecretKey, String> {
    match vault.load(NODE_KEY_ENTRY)? {
        Some(bytes) => {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| "The phone's link key in secure storage is damaged.".to_string())?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        None => {
            let key = SecretKey::generate();
            vault.store(NODE_KEY_ENTRY, &key.to_bytes())?;
            Ok(key)
        }
    }
}

/// The desktop this phone paired with. One per phone (v1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDesktop {
    /// The desktop's node id: the only node this phone dials.
    pub node_id: String,
    /// The desktop's home relay when it paired, so the phone reaches it
    /// before address lookup answers.
    pub relay_url: Option<String>,
    /// Unix time in milliseconds.
    pub paired_at: u64,
}

impl PairedDesktop {
    /// The desktop of a scanned offer.
    pub fn from_offer(offer: &PairingOffer, paired_at: u64) -> Self {
        Self {
            node_id: offer.node_id.to_string(),
            relay_url: offer.relay_url.as_ref().map(|r| r.to_string()),
            paired_at,
        }
    }

    /// Where to dial it: its node id, and its relay when it had one.
    pub fn addr(&self) -> Result<EndpointAddr, String> {
        let damaged = || "The computer this phone paired with is not kept correctly. Pair again.";
        let id: EndpointId = self.node_id.parse().map_err(|_| damaged())?;
        let addr = EndpointAddr::new(id);
        Ok(match &self.relay_url {
            Some(relay) => addr.with_relay_url(relay.parse::<RelayUrl>().map_err(|_| damaged())?),
            None => addr,
        })
    }
}

/// The kept desktop; `None` when this phone has not paired.
pub fn kept_desktop(vault: &dyn SecretVault) -> Result<Option<PairedDesktop>, String> {
    match vault.load(DESKTOP_ENTRY)? {
        None => Ok(None),
        Some(json) => serde_json::from_slice(&json).map(Some).map_err(|_| {
            "The computer this phone paired with is not kept correctly. Pair again.".to_string()
        }),
    }
}

/// Keep `desktop` as the one this phone pairs with.
pub fn keep_desktop(vault: &dyn SecretVault, desktop: &PairedDesktop) -> Result<(), String> {
    let json = serde_json::to_vec(desktop).map_err(|e| e.to_string())?;
    vault.store(DESKTOP_ENTRY, &json)
}

/// Forget the paired desktop. The phone's own key stays: it is its identity.
pub fn forget_desktop(vault: &dyn SecretVault) -> Result<(), String> {
    vault.remove(DESKTOP_ENTRY)
}

/// Why pairing did not finish.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairError {
    /// The code's five minutes are over (checked on the phone first, so it
    /// says so without dialling; the desktop checks it again).
    #[error("This pairing code has expired. Make a new one on your computer.")]
    Expired,
    #[error(transparent)]
    Connect(#[from] ConnectError),
    /// Paired, but the phone could not keep the desktop: it would be lost at
    /// the next start.
    #[error("Paired, but this phone could not keep it: {0}. Pair again.")]
    NotKept(String),
}

/// Pair with the desktop of `offer` and keep it, so later starts reconnect
/// with [`connect_to_kept`]. `now_ms` is Unix time in milliseconds.
pub async fn pair_and_keep(
    endpoint: &Endpoint,
    vault: &dyn SecretVault,
    offer: &PairingOffer,
    desktop: impl Into<EndpointAddr>,
    device_name: &str,
    now_ms: u64,
) -> Result<LinkClient, PairError> {
    if offer.expires_at <= now_ms {
        return Err(PairError::Expired);
    }
    let client = LinkClient::pair(endpoint, desktop, offer, device_name).await?;
    if let Err(e) = keep_desktop(vault, &PairedDesktop::from_offer(offer, now_ms)) {
        client.close();
        return Err(PairError::NotKept(e));
    }
    Ok(client)
}

/// [`pair_and_keep`] with the desktop's address from the offer itself (its
/// node id and relay), as a scanned code gives it.
pub async fn pair_with_offer(
    endpoint: &Endpoint,
    vault: &dyn SecretVault,
    offer: &PairingOffer,
    device_name: &str,
    now_ms: u64,
) -> Result<LinkClient, PairError> {
    pair_and_keep(
        endpoint,
        vault,
        offer,
        offer_addr(offer),
        device_name,
        now_ms,
    )
    .await
}

/// Connect to the kept desktop at `desktop` as a paired phone.
pub async fn connect_to_kept(
    endpoint: &Endpoint,
    desktop: impl Into<EndpointAddr>,
    device_name: &str,
) -> Result<LinkClient, ConnectError> {
    LinkClient::connect(endpoint, desktop, device_name).await
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemoryVault(Mutex<HashMap<String, Vec<u8>>>);

    impl SecretVault for MemoryVault {
        fn load(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.lock().unwrap().get(name).cloned())
        }
        fn store(&self, name: &str, value: &[u8]) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(name.to_string(), value.to_vec());
            Ok(())
        }
        fn remove(&self, name: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(name);
            Ok(())
        }
    }

    /// A vault that cannot be read (a locked or broken Keystore).
    struct LockedVault;

    impl SecretVault for LockedVault {
        fn load(&self, _: &str) -> Result<Option<Vec<u8>>, String> {
            Err("locked".to_string())
        }
        fn store(&self, _: &str, _: &[u8]) -> Result<(), String> {
            panic!("nothing is written to a vault that cannot be read")
        }
        fn remove(&self, _: &str) -> Result<(), String> {
            panic!("nothing is removed from a vault that cannot be read")
        }
    }

    #[test]
    fn the_phone_key_is_kept_and_never_replaced_on_a_read_error() {
        let vault = MemoryVault::default();
        let first = phone_key(&vault).unwrap();
        let again = phone_key(&vault).unwrap();
        assert_eq!(first.public(), again.public(), "the same key every start");

        assert!(phone_key(&LockedVault).is_err());
        let damaged = MemoryVault::default();
        damaged.store(NODE_KEY_ENTRY, b"short").unwrap();
        assert!(phone_key(&damaged).is_err());
        assert_eq!(
            damaged.load(NODE_KEY_ENTRY).unwrap().as_deref(),
            Some(&b"short"[..]),
            "a damaged key is left for the user to see, not overwritten"
        );
    }

    #[test]
    fn the_paired_desktop_is_kept_and_forgotten() {
        let vault = MemoryVault::default();
        assert_eq!(kept_desktop(&vault).unwrap(), None);
        let offer = PairingOffer {
            node_id: SecretKey::generate().public(),
            relay_url: Some("https://relay.example.org./".parse().unwrap()),
            secret: [7; 32],
            expires_at: 5_000,
        };
        let desktop = PairedDesktop::from_offer(&offer, 1_000);
        keep_desktop(&vault, &desktop).unwrap();
        let kept = kept_desktop(&vault).unwrap().expect("kept");
        assert_eq!(kept, desktop);
        let addr = kept.addr().unwrap();
        assert_eq!(addr.id, offer.node_id);
        assert_eq!(addr, offer_addr(&offer));
        // The secret is never kept: it worked once.
        let raw = String::from_utf8(vault.load(DESKTOP_ENTRY).unwrap().unwrap()).unwrap();
        assert!(!raw.contains("secret"), "{raw}");

        forget_desktop(&vault).unwrap();
        assert_eq!(kept_desktop(&vault).unwrap(), None);
        assert!(vault.load(NODE_KEY_ENTRY).unwrap().is_none());
    }

    #[test]
    fn a_damaged_desktop_entry_says_pair_again() {
        let vault = MemoryVault::default();
        vault.store(DESKTOP_ENTRY, b"{not json").unwrap();
        assert!(kept_desktop(&vault).unwrap_err().contains("Pair again"));
        let bad_id = PairedDesktop {
            node_id: "not-a-node".into(),
            relay_url: None,
            paired_at: 0,
        };
        assert!(bad_id.addr().unwrap_err().contains("Pair again"));
        assert!(kept_desktop(&LockedVault).is_err());
    }
}
