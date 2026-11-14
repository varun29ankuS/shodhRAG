//! The phone link: pairing, identity and the allowlisted bridge between
//! Shodh's phone app and the desktop.
//!
//! - [`messages`]: what the two sides say, framed as length-prefixed JSON
//!   with a 4 MiB cap, and [`LINK_VERSION`] (the phone and the desktop must
//!   be the same Shodh version).
//! - [`pairing`]: the QR code (`shodh-pair:1:<base32>`), the one-time secret
//!   (five minutes, single use) and the paired phones.
//! - [`allowlist`]: what a phone may call, and which approvals it may grant.
//! - [`bridge`]: the desktop's side, checked on the desktop.
//! - [`client`]: the phone's side.
//!
//! Transport: iroh (QUIC, end-to-end encrypted, dialled by public key),
//! direct where it can be and through n0's public relays otherwise. A relay
//! only forwards encrypted traffic.

pub mod allowlist;
pub mod bridge;
pub mod client;
pub mod messages;
pub mod pairing;

pub use bridge::ALPN;
/// The iroh the link is built on, so the apps use the very same one.
pub use iroh;
pub use messages::LINK_VERSION;

use iroh::endpoint::{presets, BindError};
use iroh::{Endpoint, SecretKey};

/// The desktop's endpoint: its own key, n0's default relays and address
/// lookup, accepting only the link's protocol.
pub async fn desktop_endpoint(secret_key: SecretKey) -> Result<Endpoint, BindError> {
    Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
}

/// A phone's endpoint: its own key and n0's default relays and address
/// lookup. It accepts no connections.
pub async fn phone_endpoint(secret_key: SecretKey) -> Result<Endpoint, BindError> {
    Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .bind()
        .await
}
