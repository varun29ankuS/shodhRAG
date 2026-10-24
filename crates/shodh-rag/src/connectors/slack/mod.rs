//! The Slack connector: Shodh answers questions in Slack, indexes the channels the user
//! chooses, and saves memories people ask it to keep.
//!
//! People in one Slack workspace mention the bot in a channel (or DM it) and get an answer,
//! with citations, from the documents of the one Shodh workspace the user selected. The
//! connection is Slack's Socket Mode, opened from this computer: no public URL, no server,
//! and the tokens stay in the OS credential store.
//!
//! - [`tokens`]: the app-level and bot tokens, checked and never printed.
//! - [`setup`]: the app manifest and the scope check.
//! - [`web`]: the Slack Web API calls.
//! - [`events`]: Socket Mode frames; which events are questions; dedupe; one answer per
//!   thread.
//! - [`access`]: who may ask, and where (the channels chosen on the card; DMs from
//!   members of the installing workspace only). Deny by default.
//! - [`socket`]: the connection: ack first, reconnect, back off, stop.
//! - [`answer`]: retrieval over the selected workspace only, the prompt, the Slack text.
//! - [`sanitize`]: no path, home folder, user name or id leaves for Slack.
//! - [`respond`]: one question start to finish, cheapest checks first.
//! - [`service`]: envelopes to answers (and to the index's live updates).
//! - [`ids`], [`content`], [`index`], [`store`]: channels as an indexed source (one thread
//!   one document), the sync and its checkpoints in `shodh.db`.
//! - [`remember`]: `@Shodh remember: …` saved directly, by who said it.
//!
//! Plan and decisions: `docs/superpowers/plans/2026-10-23-slack-connector.md`.

pub mod access;
pub mod answer;
pub mod content;
pub mod events;
pub mod ids;
pub mod index;
pub mod remember;
pub mod respond;
pub mod sanitize;
pub mod service;
pub mod setup;
pub mod socket;
pub mod store;
pub mod tokens;
pub mod web;

#[cfg(test)]
mod isolation_tests;

/// Cancels an installation's answers (queued, waiting and running) when the connector is
/// switched off, disconnected, given new tokens or paused by Local-only mode.
pub use tokio_util::sync::CancellationToken;

/// One Slack workspace the connector is installed in: what Shodh may show about it (no
/// secrets). The credentials are kept apart, by `team_id`
/// ([`tokens::SlackCredentialStore`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackInstallation {
    pub team_id: String,
    pub team_name: String,
    /// The bot's user id (`U…`) and name.
    pub bot_user_id: String,
    pub bot_name: String,
    #[serde(default)]
    pub bot_id: Option<String>,
    /// The bot scopes Slack reported at connection time; empty when it did not say.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// The Slack app the bot belongs to (`bots.info`), checked against every Socket Mode
    /// connection's `hello`; `None` when it could not be read.
    #[serde(default)]
    pub app_id: Option<String>,
    pub connected_at: chrono::DateTime<chrono::Utc>,
}

impl SlackInstallation {
    pub fn bot(&self) -> events::BotIdentity {
        events::BotIdentity {
            user_id: self.bot_user_id.clone(),
            bot_id: self.bot_id.clone(),
        }
    }
}
