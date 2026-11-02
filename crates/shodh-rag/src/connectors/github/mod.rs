//! The GitHub connector: Shodh itself does the remote GitHub work Code mode's sandboxed
//! commands cannot (they have no network and no credentials), under per-repository
//! permissions the user sets, so no command ever sees a token.
//!
//! **The app.** The Shodh **GitHub App** (client id [`device::CLIENT_ID`], public
//! install page <https://github.com/apps/shodh-rag>), with device flow on, expiring
//! tokens off, and repository permissions Contents read & write, Pull requests read &
//! write, Issues read & write, Metadata read; nothing else. Shodh ships no client secret
//! and no private key. Its user token can do what both the app and the user may, only on
//! repositories the app is installed on; Shodh's own list narrows that further.
//!
//! What GitHub's documentation says, and how it shapes this (checked 2026-11-02):
//! - The device flow needs only the client id. A refresh of an expiring token needs no
//!   client secret when the token came from the device flow. With expiring tokens off,
//!   GitHub issues no refresh token; [`device::DeviceFlow::refresh`] is used only if the
//!   app's tokens start to expire.
//! - A GitHub App's user token has no scopes (`scope` is always empty): its reach is the
//!   app's permissions, the user's own, and the installations.
//! - Revoking a token (`DELETE /applications/{client_id}/token`) needs the client
//!   secret, so signing out of Shodh deletes the token from this computer and points the
//!   user to GitHub's Authorized apps page to revoke it there too.
//!
//! Parts:
//! - [`token`]: the user token (and refresh token), never printed or serialised but to
//!   the credential store.
//! - [`device`]: sign-in with the device flow; refresh.
//! - [`api`]: the REST calls.
//! - [`remote`]: a project's remotes as `owner/repo`, forks, other hosts.
//! - [`limits`]: per-repository permissions and the hard limits (no force push, only
//!   `shodh/…` branches, no workflow changes, merge only when GitHub says ready).
//! - [`git`]: fetch and push done by Shodh, in a mirror it owns, with the sign-in given
//!   to that one git process only.
//! - [`tools`]: the agent's GitHub tools.

pub mod api;
pub mod device;
pub mod git;
pub mod limits;
pub mod remote;
pub mod token;
pub mod tools;

#[cfg(test)]
pub(crate) mod fake;
