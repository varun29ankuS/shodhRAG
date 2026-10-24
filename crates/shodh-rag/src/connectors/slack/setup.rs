//! Setting up the Slack app without an OAuth server: the manifest the user creates the
//! app from, and the check of what the installed bot was granted.
//!
//! Manifest fields: <https://docs.slack.dev/reference/app-manifest>. The create link
//! (`https://api.slack.com/apps?new_app=1&manifest_json=…`, URL-encoded):
//! <https://docs.slack.dev/app-manifests/configuring-apps-with-app-manifests>.

use serde_json::{json, Value};

/// The bot scopes answering in Slack needs, and nothing more:
/// - `app_mentions:read`: the `app_mention` event (a mention in a channel);
/// - `channels:read`, `groups:read`: list the public and private channels the bot is in
///   (`users.conversations`), so the user picks where it answers, and see which are
///   shared with other organizations;
/// - `chat:write`: post the answer and edit the placeholder into it;
/// - `im:history`: the `message.im` event (a DM to the bot);
/// - `im:read`, `im:write`: the bot's DM conversations;
/// - `users:read`: who sent a DM (`users.info`: their workspace, guest or not), and which
///   app the bot belongs to (`bots.info`), to catch tokens of two different apps.
pub const REQUIRED_BOT_SCOPES: [&str; 8] = [
    "app_mentions:read",
    "channels:read",
    "chat:write",
    "groups:read",
    "im:history",
    "im:read",
    "im:write",
    "users:read",
];

/// The events answering needs.
pub const BOT_EVENTS: [&str; 2] = ["app_mention", "message.im"];

/// The bot scopes indexing channels needs besides the answer scopes, checked apart: an
/// app without them still answers, and the card says what to add.
/// - `channels:history`, `groups:history`: `conversations.history` and
///   `conversations.replies` of the public and private channels the bot is in, and their
///   `message.channels` / `message.groups` events (new messages, edits, deletions).
///
/// `users:read` (already an answer scope) gives the names mentions are shown with and
/// the people of the "This is me" pick; `channels:read` / `groups:read` the archive,
/// deletion and leave events. No e-mail scope.
pub const INDEX_BOT_SCOPES: [&str; 2] = ["channels:history", "groups:history"];

/// The events indexing listens to. A private channel's archive has no Events API event
/// (`group_archive` is RTM-only): the sync sees it from `conversations.info`.
pub const INDEX_EVENTS: [&str; 7] = [
    "channel_archive",
    "channel_deleted",
    "channel_left",
    "group_deleted",
    "member_left_channel",
    "message.channels",
    "message.groups",
];

/// Every bot scope of the manifest.
pub fn manifest_scopes() -> Vec<&'static str> {
    let mut scopes: Vec<&'static str> = REQUIRED_BOT_SCOPES
        .iter()
        .chain(INDEX_BOT_SCOPES.iter())
        .copied()
        .collect();
    scopes.sort_unstable();
    scopes
}

/// Every event of the manifest.
pub fn manifest_events() -> Vec<&'static str> {
    let mut events: Vec<&'static str> = BOT_EVENTS
        .iter()
        .chain(INDEX_EVENTS.iter())
        .copied()
        .collect();
    events.sort_unstable();
    events
}

/// The index scopes the bot was not granted (`granted` empty: Slack did not say, none
/// listed).
pub fn missing_index_scopes(granted: &[String]) -> Vec<String> {
    if granted.is_empty() {
        return Vec::new();
    }
    INDEX_BOT_SCOPES
        .iter()
        .filter(|s| !granted.iter().any(|g| g == *s))
        .map(|s| s.to_string())
        .collect()
}

/// What the card says when indexing lacks scopes.
pub fn missing_index_scopes_message(missing: &[String]) -> String {
    format!(
        "To index channels, the Slack app needs these permissions: {}. In Slack, open the \
         app's settings, add them under OAuth & Permissions → Bot Token Scopes, add the \
         events under Event Subscriptions (or paste Shodh's manifest under App Manifest), \
         reinstall the app to the workspace, and connect again with the new bot token. \
         Answering keeps working meanwhile.",
        missing.join(", ")
    )
}

/// The scope the app-level token needs for Socket Mode.
pub const APP_TOKEN_SCOPE: &str = "connections:write";

/// The app's name in Slack (at most 35 characters).
pub const APP_NAME: &str = "Shodh";
/// The bot user's name (a-z, 0-9, `-`, `_`, `.`; at most 80 characters).
pub const BOT_DISPLAY_NAME: &str = "shodh";

/// The manifest of the Slack app: a bot user people can mention and DM, Socket Mode on (no
/// public URL), token rotation off (the connector does not refresh tokens).
pub fn manifest() -> Value {
    json!({
        "_metadata": { "major_version": 1, "minor_version": 1 },
        "display_information": {
            "name": APP_NAME,
            "description": "Answers questions from the documents of one Shodh workspace, with citations, and indexes the channels you choose.",
            "background_color": "#1f2933"
        },
        "features": {
            "app_home": {
                "home_tab_enabled": false,
                "messages_tab_enabled": true,
                "messages_tab_read_only_enabled": false
            },
            "bot_user": {
                "display_name": BOT_DISPLAY_NAME,
                "always_online": false
            }
        },
        "oauth_config": {
            "scopes": { "bot": manifest_scopes() }
        },
        "settings": {
            "event_subscriptions": { "bot_events": manifest_events() },
            "interactivity": { "is_enabled": false },
            "org_deploy_enabled": false,
            "socket_mode_enabled": true,
            "token_rotation_enabled": false
        }
    })
}

/// Percent-encodes everything but RFC 3986 unreserved characters.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The link that opens Slack's "create an app" dialog with [`manifest`] filled in.
pub fn create_app_url() -> String {
    format!(
        "https://api.slack.com/apps?new_app=1&manifest_json={}",
        percent_encode(&manifest().to_string())
    )
}

/// The scopes in an `x-oauth-scopes` response header (`"chat:write,im:read"`).
pub fn parse_scopes(header: &str) -> Vec<String> {
    let mut scopes: Vec<String> = header
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    scopes.sort();
    scopes.dedup();
    scopes
}

/// What is known about the bot's scopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeCheck {
    /// Every scope in [`REQUIRED_BOT_SCOPES`] was granted.
    Complete,
    /// These required scopes were not granted (in [`REQUIRED_BOT_SCOPES`] order).
    Missing(Vec<String>),
    /// Slack did not say which scopes were granted (no `x-oauth-scopes` header). Setup goes
    /// on; a scope that turns out missing is reported when Slack refuses a call with
    /// `missing_scope`.
    Unknown,
}

/// Compares the granted scopes (`None`: Slack did not list them) with
/// [`REQUIRED_BOT_SCOPES`].
pub fn check_scopes(granted: Option<&[String]>) -> ScopeCheck {
    let Some(granted) = granted else {
        return ScopeCheck::Unknown;
    };
    let missing: Vec<String> = REQUIRED_BOT_SCOPES
        .iter()
        .filter(|required| !granted.iter().any(|g| g == *required))
        .map(|s| s.to_string())
        .collect();
    if missing.is_empty() {
        ScopeCheck::Complete
    } else {
        ScopeCheck::Missing(missing)
    }
}

/// "Slack is missing these permissions: im:history, im:write. …" for the card.
pub fn missing_scopes_message(missing: &[String]) -> String {
    format!(
        "The Slack app is missing these permissions: {}. Add them under OAuth & Permissions → Bot \
         Token Scopes (or create the app again from Shodh's manifest), reinstall the app to the \
         workspace, and paste the new bot token.",
        missing.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_asks_for_socket_mode_and_exactly_the_answer_and_index_scopes() {
        let m = manifest();
        assert_eq!(m["settings"]["socket_mode_enabled"], true);
        assert_eq!(m["settings"]["token_rotation_enabled"], false);
        assert_eq!(
            m["settings"]["event_subscriptions"]["bot_events"],
            json!([
                "app_mention",
                "channel_archive",
                "channel_deleted",
                "channel_left",
                "group_deleted",
                "member_left_channel",
                "message.channels",
                "message.groups",
                "message.im"
            ])
        );
        assert_eq!(
            m["oauth_config"]["scopes"]["bot"],
            json!([
                "app_mentions:read",
                "channels:history",
                "channels:read",
                "chat:write",
                "groups:history",
                "groups:read",
                "im:history",
                "im:read",
                "im:write",
                "users:read"
            ])
        );
        // No e-mail addresses, no group DMs, no files: none is asked for.
        let scopes = m["oauth_config"]["scopes"]["bot"].to_string();
        for absent in ["users:read.email", "mpim:", "files:", "search:"] {
            assert!(!scopes.contains(absent), "{absent}");
        }
        assert!(m["oauth_config"]["scopes"].get("user").is_none());
        assert!(m["settings"].get("event_subscriptions").unwrap()["request_url"].is_null());
        assert_eq!(m["features"]["app_home"]["messages_tab_enabled"], true);
        let name = m["display_information"]["name"].as_str().unwrap();
        assert!(name.chars().count() <= 35);
        let description = m["display_information"]["description"].as_str().unwrap();
        assert!(description.chars().count() <= 140);
        let bot = m["features"]["bot_user"]["display_name"].as_str().unwrap();
        assert!(bot
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c)));
    }

    #[test]
    fn the_create_link_carries_the_whole_manifest_encoded() {
        let url = create_app_url();
        let prefix = "https://api.slack.com/apps?new_app=1&manifest_json=";
        assert!(url.starts_with(prefix));
        let encoded = &url[prefix.len()..];
        assert!(!encoded.contains(['{', '"', ' ', '#', '&', ':', ',']));
        let decoded = url::form_urlencoded::parse(format!("m={encoded}").as_bytes())
            .next()
            .unwrap()
            .1
            .into_owned();
        assert_eq!(serde_json::from_str::<Value>(&decoded).unwrap(), manifest());
    }

    #[test]
    fn missing_scopes_are_listed_by_name() {
        let granted = parse_scopes(" chat:write,IM:READ ,app_mentions:read,,chat:write");
        assert_eq!(granted, vec!["app_mentions:read", "chat:write", "im:read"]);
        assert_eq!(
            check_scopes(Some(&granted)),
            ScopeCheck::Missing(vec![
                "channels:read".into(),
                "groups:read".into(),
                "im:history".into(),
                "im:write".into(),
                "users:read".into()
            ])
        );
        // An app created from the earlier manifest lacks the three scopes added since.
        let earlier = parse_scopes("app_mentions:read,chat:write,im:history,im:read,im:write");
        assert_eq!(
            check_scopes(Some(&earlier)),
            ScopeCheck::Missing(vec![
                "channels:read".into(),
                "groups:read".into(),
                "users:read".into()
            ])
        );
        let mut all: Vec<String> = REQUIRED_BOT_SCOPES.iter().map(|s| s.to_string()).collect();
        all.push("channels:read".into());
        assert_eq!(check_scopes(Some(&all)), ScopeCheck::Complete);
        assert_eq!(
            check_scopes(Some(&[])),
            ScopeCheck::Missing(REQUIRED_BOT_SCOPES.iter().map(|s| s.to_string()).collect())
        );
        assert_eq!(check_scopes(None), ScopeCheck::Unknown);
        let message = missing_scopes_message(&["im:history".into(), "im:write".into()]);
        assert!(message.contains("im:history, im:write"));
    }

    #[test]
    fn index_scopes_are_checked_apart_from_the_answer_scopes() {
        // A 0.0.8 app: every answer scope, no history. It still answers.
        let answering: Vec<String> = REQUIRED_BOT_SCOPES.iter().map(|s| s.to_string()).collect();
        assert_eq!(check_scopes(Some(&answering)), ScopeCheck::Complete);
        assert_eq!(
            missing_index_scopes(&answering),
            vec!["channels:history".to_string(), "groups:history".to_string()]
        );
        let mut all = answering.clone();
        all.extend(INDEX_BOT_SCOPES.iter().map(|s| s.to_string()));
        assert!(missing_index_scopes(&all).is_empty());
        assert!(
            missing_index_scopes(&[]).is_empty(),
            "unknown is not missing"
        );
        let message = missing_index_scopes_message(&missing_index_scopes(&answering));
        assert!(message.contains("channels:history, groups:history"));
        assert!(message.contains("Answering keeps working"));
    }
}
