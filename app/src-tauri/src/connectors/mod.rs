//! Connectors: Shodh working inside other tools (the Connectors view). One module per
//! connector; their settings live under `connectors` in `app_settings.json`.

pub mod conversations;
pub mod slack;
pub mod slack_index;
pub mod slack_notices;
mod slack_usage;
pub mod telegram;
pub mod telegram_notices;

use serde::{Deserialize, Serialize};

/// Every connector's settings. User-only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectorsPrefs {
    pub slack: slack::SlackPrefs,
    pub telegram: telegram::TelegramPrefs,
}
