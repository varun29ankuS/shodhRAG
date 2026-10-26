//! The few Slack Web API methods the connector calls:
//! [`auth.test`](https://docs.slack.dev/reference/methods/auth.test),
//! [`bots.info`](https://docs.slack.dev/reference/methods/bots.info),
//! [`apps.connections.open`](https://docs.slack.dev/reference/methods/apps.connections.open),
//! [`users.info`](https://docs.slack.dev/reference/methods/users.info),
//! [`users.conversations`](https://docs.slack.dev/reference/methods/users.conversations),
//! [`chat.postMessage`](https://docs.slack.dev/reference/methods/chat.postMessage),
//! [`chat.update`](https://docs.slack.dev/reference/methods/chat.update), and for indexing
//! [`conversations.history`](https://docs.slack.dev/reference/methods/conversations.history),
//! [`conversations.replies`](https://docs.slack.dev/reference/methods/conversations.replies),
//! [`conversations.info`](https://docs.slack.dev/reference/methods/conversations.info),
//! [`chat.getPermalink`](https://docs.slack.dev/reference/methods/chat.getPermalink) and
//! [`users.list`](https://docs.slack.dev/reference/methods/users.list).
//!
//! Tokens go in the `Authorization` header only, never in a URL or a body, and no error
//! message carries one. Methods that read are sent form-encoded, methods that write as
//! JSON.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::access::{Member, MemberDirectory};
use super::content::HistoryPage;
use super::respond::SlackPoster;
use super::setup::parse_scopes;
use super::tokens::{SlackAppToken, SlackBotToken};

/// Slack's Web API.
pub const SLACK_API: &str = "https://slack.com/api/";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a Slack call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlackApiError {
    /// Slack answered `"ok": false` with this error code (`invalid_auth`, `missing_scope`,
    /// …).
    #[error("Slack refused the request ({code})")]
    Api {
        code: String,
        /// The scope Slack said was needed, with `missing_scope`.
        needed: Option<String>,
    },
    #[error("Slack is rate limiting requests")]
    RateLimited { retry_after: Option<Duration> },
    #[error("Slack answered with HTTP status {0}")]
    Status(u16),
    #[error("Slack could not be reached: {0}")]
    Network(String),
    #[error("Slack's answer could not be read: {0}")]
    BadResponse(String),
}

impl SlackApiError {
    /// The token is wrong, revoked or belongs to a removed app or workspace: trying again
    /// cannot help.
    pub fn is_auth(&self) -> bool {
        matches!(self, SlackApiError::Api { code, .. } if matches!(
            code.as_str(),
            "invalid_auth" | "not_authed" | "token_revoked" | "token_expired"
                | "account_inactive" | "no_permission" | "not_allowed_token_type"
                | "missing_scope" | "team_access_not_granted" | "ekm_access_denied"
        ))
    }

    /// The error code Slack sent, if it sent one.
    pub fn code(&self) -> Option<&str> {
        match self {
            SlackApiError::Api { code, .. } => Some(code),
            _ => None,
        }
    }

    /// A failure on the way to Slack or inside it (a network error, a 5xx), which one
    /// more try may get past; a refusal or a rate limit is not.
    pub fn is_transient(&self) -> bool {
        match self {
            SlackApiError::Network(_) => true,
            SlackApiError::Status(status) => *status >= 500,
            _ => false,
        }
    }
}

/// A channel the bot is a member of (`users.conversations`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotChannel {
    pub id: String,
    pub name: String,
    pub private: bool,
    /// Shared with another organization (Slack Connect: `is_ext_shared`, or waiting for
    /// the other side's approval: `is_pending_ext_shared`,
    /// <https://docs.slack.dev/reference/objects/conversation-object>).
    pub shared: bool,
}

/// Messages asked for per `conversations.history` / `conversations.replies` page: what
/// Slack recommends (at most 200), well within the 1 000 an internal app may ask for.
pub const HISTORY_PAGE: usize = 200;
/// `users.list` pages read at most (2 000 people).
pub const MAX_PEOPLE_PAGES: usize = 10;

/// A member of the workspace, for the "This is me" pick.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: String,
    pub name: String,
}

/// What `conversations.info` says about a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub name: String,
    pub archived: bool,
    pub shared: bool,
    pub private: bool,
}

/// The name people see for user object `u`: the profile's display name, else the real
/// name, else the user name; never the e-mail address.
fn person_name(u: &Value) -> String {
    let text = |pointer: &str| {
        u.pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    text("/profile/display_name")
        .or_else(|| text("/profile/real_name"))
        .or_else(|| text("/real_name"))
        .or_else(|| text("/name"))
        .unwrap_or_else(|| "Someone".to_string())
}

/// Channels listed per `users.conversations` page (Slack recommends at most 200).
pub const CHANNELS_PER_PAGE: usize = 200;
/// Pages read at most: 2 000 channels, far more than a bot is invited to.
pub const MAX_CHANNEL_PAGES: usize = 10;

/// What `auth.test` says about the bot token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthInfo {
    pub team_id: String,
    pub team: String,
    /// The bot's user id and name.
    pub user_id: String,
    pub user: String,
    pub bot_id: Option<String>,
    /// From the `x-oauth-scopes` header; `None` when Slack did not send it.
    pub scopes: Option<Vec<String>>,
}

/// A client of Slack's Web API.
#[derive(Clone)]
pub struct SlackWeb {
    client: reqwest::Client,
    base: String,
}

impl std::fmt::Debug for SlackWeb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackWeb")
            .field("base", &self.base)
            .finish()
    }
}

impl SlackWeb {
    /// The client for Slack itself.
    pub fn new() -> Result<Self, SlackApiError> {
        Self::with_base(SLACK_API)
    }

    /// A client for the API at `base` (tests use a local server).
    pub fn with_base(base: &str) -> Result<Self, SlackApiError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("Shodh/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| SlackApiError::Network(e.to_string()))?;
        let mut base = base.to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        Ok(Self { client, base })
    }

    async fn call(
        &self,
        method: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<(Value, Option<String>), SlackApiError> {
        match body {
            Some(body) => {
                self.send(
                    method,
                    token,
                    "application/json; charset=utf-8",
                    body.to_string(),
                )
                .await
            }
            None => self.call_form(method, token, &[]).await,
        }
    }

    /// A call with form-encoded arguments (the reading methods do not take JSON).
    async fn call_form(
        &self,
        method: &str,
        token: &str,
        args: &[(&str, &str)],
    ) -> Result<(Value, Option<String>), SlackApiError> {
        // Built before the await: the serializer is not `Send`.
        let body = {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in args {
                form.append_pair(k, v);
            }
            form.finish()
        };
        self.send(method, token, "application/x-www-form-urlencoded", body)
            .await
    }

    async fn send(
        &self,
        method: &str,
        token: &str,
        content_type: &str,
        body: String,
    ) -> Result<(Value, Option<String>), SlackApiError> {
        let url = format!("{}{method}", self.base);
        let request = self
            .client
            .post(&url)
            .bearer_auth(token)
            .header("Content-Type", content_type)
            .body(body);
        let response = request
            .send()
            .await
            .map_err(|e| SlackApiError::Network(e.without_url().to_string()))?;
        let status = response.status();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        if status.as_u16() == 429 {
            let retry_after = header("retry-after")
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            return Err(SlackApiError::RateLimited { retry_after });
        }
        if !status.is_success() {
            return Err(SlackApiError::Status(status.as_u16()));
        }
        let scopes = header("x-oauth-scopes");
        let text = response
            .text()
            .await
            .map_err(|e| SlackApiError::Network(e.without_url().to_string()))?;
        let value: Value =
            serde_json::from_str(&text).map_err(|e| SlackApiError::BadResponse(e.to_string()))?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            let code = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown_error")
                .to_string();
            if code == "ratelimited" {
                return Err(SlackApiError::RateLimited { retry_after: None });
            }
            let needed = value
                .get("needed")
                .and_then(Value::as_str)
                .map(str::to_string);
            return Err(SlackApiError::Api { code, needed });
        }
        Ok((value, scopes))
    }

    /// Who the bot token belongs to, and its scopes.
    pub async fn auth_test(&self, token: &SlackBotToken) -> Result<AuthInfo, SlackApiError> {
        let (value, scopes) = self.call("auth.test", token.expose(), None).await?;
        let field = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let info = AuthInfo {
            team_id: field("team_id"),
            team: field("team"),
            user_id: field("user_id"),
            user: field("user"),
            bot_id: Some(field("bot_id")).filter(|b| !b.is_empty()),
            scopes: scopes.as_deref().map(parse_scopes),
        };
        if info.team_id.is_empty() || info.user_id.is_empty() {
            return Err(SlackApiError::BadResponse(
                "auth.test did not name the workspace and the bot".into(),
            ));
        }
        Ok(info)
    }

    /// The app a bot belongs to (`bots.info`, scope `users:read`): with the app id of
    /// the app-level token it tells whether both tokens are from the same Slack app.
    pub async fn bot_app_id(
        &self,
        token: &SlackBotToken,
        bot_id: &str,
    ) -> Result<String, SlackApiError> {
        let (value, _) = self
            .call_form("bots.info", token.expose(), &[("bot", bot_id)])
            .await?;
        value
            .pointer("/bot/app_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| SlackApiError::BadResponse("bots.info did not name the app".into()))
    }

    /// Who `user` is (`users.info`, scope `users:read`).
    pub async fn user_info(
        &self,
        token: &SlackBotToken,
        user: &str,
    ) -> Result<Member, SlackApiError> {
        let (value, _) = self
            .call_form("users.info", token.expose(), &[("user", user)])
            .await?;
        let Some(u) = value.get("user") else {
            return Err(SlackApiError::BadResponse(
                "users.info did not return a user".into(),
            ));
        };
        let flag = |key: &str| u.get(key).and_then(Value::as_bool).unwrap_or(false);
        let team_id = u
            .get("team_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if team_id.is_empty() {
            return Err(SlackApiError::BadResponse(
                "users.info did not name the user's workspace".into(),
            ));
        }
        Ok(Member {
            team_id,
            name: person_name(u),
            guest: flag("is_restricted") || flag("is_ultra_restricted"),
            stranger: flag("is_stranger"),
            bot: flag("is_bot") || flag("is_app_user"),
            deleted: flag("deleted"),
        })
    }

    /// The public and private channels the bot is a member of (`users.conversations`,
    /// scopes `channels:read`, `groups:read`), archived ones left out, by name.
    pub async fn bot_channels(
        &self,
        token: &SlackBotToken,
    ) -> Result<Vec<BotChannel>, SlackApiError> {
        let limit = CHANNELS_PER_PAGE.to_string();
        let mut channels = Vec::new();
        let mut cursor = String::new();
        for _ in 0..MAX_CHANNEL_PAGES {
            let (value, _) = {
                let mut args = vec![
                    ("types", "public_channel,private_channel"),
                    ("exclude_archived", "true"),
                    ("limit", limit.as_str()),
                ];
                if !cursor.is_empty() {
                    args.push(("cursor", cursor.as_str()));
                }
                self.call_form("users.conversations", token.expose(), &args)
                    .await?
            };
            for c in value
                .get("channels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let text = |key: &str| c.get(key).and_then(Value::as_str).unwrap_or_default();
                let flag = |key: &str| c.get(key).and_then(Value::as_bool).unwrap_or(false);
                if text("id").is_empty() || flag("is_archived") {
                    continue;
                }
                channels.push(BotChannel {
                    id: text("id").to_string(),
                    name: text("name").to_string(),
                    private: flag("is_private"),
                    shared: flag("is_ext_shared") || flag("is_pending_ext_shared"),
                });
            }
            cursor = value
                .pointer("/response_metadata/next_cursor")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if cursor.is_empty() {
                break;
            }
        }
        channels.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        channels.dedup_by(|a, b| a.id == b.id);
        Ok(channels)
    }

    /// One page of a channel's messages, newest first (`conversations.history`, scopes
    /// `channels:history` / `groups:history`), between `oldest` and `latest` (exclusive),
    /// from `cursor`.
    pub async fn history(
        &self,
        token: &SlackBotToken,
        channel: &str,
        oldest: Option<&str>,
        latest: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<HistoryPage, SlackApiError> {
        let limit = HISTORY_PAGE.to_string();
        let mut args = vec![("channel", channel), ("limit", limit.as_str())];
        if let Some(oldest) = oldest {
            args.push(("oldest", oldest));
        }
        if let Some(latest) = latest {
            args.push(("latest", latest));
        }
        if let Some(cursor) = cursor {
            args.push(("cursor", cursor));
        }
        let (value, _) = self
            .call_form("conversations.history", token.expose(), &args)
            .await?;
        Ok(HistoryPage::from_value(&value))
    }

    /// One page of a thread's messages, its first message first
    /// (`conversations.replies`).
    pub async fn replies(
        &self,
        token: &SlackBotToken,
        channel: &str,
        thread_ts: &str,
        cursor: Option<&str>,
    ) -> Result<HistoryPage, SlackApiError> {
        let limit = HISTORY_PAGE.to_string();
        let mut args = vec![
            ("channel", channel),
            ("ts", thread_ts),
            ("limit", limit.as_str()),
        ];
        if let Some(cursor) = cursor {
            args.push(("cursor", cursor));
        }
        let (value, _) = self
            .call_form("conversations.replies", token.expose(), &args)
            .await?;
        Ok(HistoryPage::from_value(&value))
    }

    /// What Slack says about a channel now (`conversations.info`).
    pub async fn channel_info(
        &self,
        token: &SlackBotToken,
        channel: &str,
    ) -> Result<ChannelInfo, SlackApiError> {
        let (value, _) = self
            .call_form(
                "conversations.info",
                token.expose(),
                &[("channel", channel)],
            )
            .await?;
        let Some(c) = value.get("channel") else {
            return Err(SlackApiError::BadResponse(
                "conversations.info did not return the channel".into(),
            ));
        };
        let flag = |key: &str| c.get(key).and_then(Value::as_bool).unwrap_or(false);
        Ok(ChannelInfo {
            name: c
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            archived: flag("is_archived"),
            shared: flag("is_ext_shared") || flag("is_pending_ext_shared"),
            private: flag("is_private"),
        })
    }

    /// The permalink of message `ts` (`chat.getPermalink`).
    pub async fn permalink(
        &self,
        token: &SlackBotToken,
        channel: &str,
        ts: &str,
    ) -> Result<String, SlackApiError> {
        let (value, _) = self
            .call_form(
                "chat.getPermalink",
                token.expose(),
                &[("channel", channel), ("message_ts", ts)],
            )
            .await?;
        value
            .get("permalink")
            .and_then(Value::as_str)
            .filter(|p| p.starts_with("https://"))
            .map(str::to_string)
            .ok_or_else(|| {
                SlackApiError::BadResponse("chat.getPermalink did not return a link".into())
            })
    }

    /// The full members of the bot's own workspace (`users.list`, scope `users:read`):
    /// no bots, guests, deactivated accounts or people of other organizations. For the
    /// card's "This is me" pick.
    pub async fn people(
        &self,
        token: &SlackBotToken,
        team_id: &str,
    ) -> Result<Vec<Person>, SlackApiError> {
        let limit = CHANNELS_PER_PAGE.to_string();
        let mut people = Vec::new();
        let mut cursor = String::new();
        for _ in 0..MAX_PEOPLE_PAGES {
            let (value, _) = {
                let mut args = vec![("limit", limit.as_str())];
                if !cursor.is_empty() {
                    args.push(("cursor", cursor.as_str()));
                }
                self.call_form("users.list", token.expose(), &args).await?
            };
            for u in value
                .get("members")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let flag = |key: &str| u.get(key).and_then(Value::as_bool).unwrap_or(false);
                let id = u.get("id").and_then(Value::as_str).unwrap_or_default();
                let team = u.get("team_id").and_then(Value::as_str).unwrap_or_default();
                if id.is_empty()
                    || team != team_id
                    || flag("deleted")
                    || flag("is_bot")
                    || flag("is_app_user")
                    || flag("is_restricted")
                    || flag("is_ultra_restricted")
                    || flag("is_stranger")
                    || id == "USLACKBOT"
                {
                    continue;
                }
                people.push(Person {
                    id: id.to_string(),
                    name: person_name(u),
                });
            }
            cursor = value
                .pointer("/response_metadata/next_cursor")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if cursor.is_empty() {
                break;
            }
        }
        people.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(people)
    }

    /// A WebSocket URL for a new Socket Mode connection.
    pub async fn open_connection(&self, token: &SlackAppToken) -> Result<String, SlackApiError> {
        let (value, _) = self
            .call("apps.connections.open", token.expose(), None)
            .await?;
        match value.get("url").and_then(Value::as_str) {
            Some(url) if url.starts_with("wss://") || url.starts_with("ws://") => {
                Ok(url.to_string())
            }
            _ => Err(SlackApiError::BadResponse(
                "apps.connections.open did not return a WebSocket URL".into(),
            )),
        }
    }

    /// Posts `text`; returns the message's `ts`.
    pub async fn post_message(
        &self,
        token: &SlackBotToken,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<String, SlackApiError> {
        let mut body = json!({
            "channel": channel,
            "text": text,
            "unfurl_links": false,
            "unfurl_media": false,
        });
        if let Some(thread_ts) = thread_ts {
            body["thread_ts"] = json!(thread_ts);
        }
        let (value, _) = self
            .call("chat.postMessage", token.expose(), Some(body))
            .await?;
        value
            .get("ts")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                SlackApiError::BadResponse("chat.postMessage did not return a ts".into())
            })
    }

    /// Replaces the text of message `ts`.
    pub async fn update_message(
        &self,
        token: &SlackBotToken,
        channel: &str,
        ts: &str,
        text: &str,
    ) -> Result<(), SlackApiError> {
        let body = json!({ "channel": channel, "ts": ts, "text": text });
        self.call("chat.update", token.expose(), Some(body)).await?;
        Ok(())
    }
}

/// Posts as the bot of one installation.
pub struct BotPoster {
    pub web: SlackWeb,
    pub token: SlackBotToken,
}

#[async_trait]
impl SlackPoster for BotPoster {
    async fn post(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<String, SlackApiError> {
        self.web
            .post_message(&self.token, channel, thread_ts, text)
            .await
    }

    async fn update(&self, channel: &str, ts: &str, text: &str) -> Result<(), SlackApiError> {
        self.web
            .update_message(&self.token, channel, ts, text)
            .await
    }
}

/// Looks people up as the bot of one installation.
pub struct BotDirectory {
    pub web: SlackWeb,
    pub token: SlackBotToken,
}

#[async_trait]
impl MemberDirectory for BotDirectory {
    async fn member(&self, user: &str) -> Result<Member, SlackApiError> {
        self.web.user_info(&self.token, user).await
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A local HTTP server answering like Slack, so the client is tested without Slack.

    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One request the server received.
    #[derive(Debug, Clone)]
    pub struct Seen {
        pub path: String,
        pub head: String,
        pub body: String,
    }

    /// Serves `responses` (status line, extra headers, body) in order, one per request.
    pub async fn serve(
        responses: Vec<(&'static str, Vec<(&'static str, &'static str)>, String)>,
    ) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/api/", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            for (status, headers, body) in responses {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                // Read the head, then the body by its Content-Length.
                let (head, mut rest) = loop {
                    let n = socket.read(&mut chunk).await.unwrap();
                    buffer.extend_from_slice(&chunk[..n]);
                    if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buffer[..at]).to_string();
                        break (head, buffer[at + 4..].to_vec());
                    }
                };
                let length = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                while rest.len() < length {
                    let n = socket.read(&mut chunk).await.unwrap();
                    rest.extend_from_slice(&chunk[..n]);
                }
                let path = head
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .to_string();
                log.lock().unwrap().push(Seen {
                    path,
                    head: head.clone(),
                    body: String::from_utf8_lossy(&rest).to_string(),
                });
                let mut reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    reply.push_str(&format!("{k}: {v}\r\n"));
                }
                reply.push_str("\r\n");
                reply.push_str(&body);
                socket.write_all(reply.as_bytes()).await.unwrap();
                socket.shutdown().await.ok();
            }
        });
        (base, seen)
    }
}

#[cfg(test)]
mod tests {
    use super::fake::serve;
    use super::*;

    const BOT: &str = "xoxb-1-2-secretbot";
    const APP: &str = "xapp-1-A1-2-secretapp";

    #[tokio::test]
    async fn auth_test_reads_the_workspace_bot_and_scopes() {
        let (base, seen) = serve(vec![(
            "200 OK",
            vec![("x-oauth-scopes", "chat:write,app_mentions:read,im:history")],
            json!({"ok": true, "url": "https://acme.slack.com/", "team": "Acme", "user": "shodh",
                   "team_id": "T1", "user_id": "UBOT", "bot_id": "BBOT"})
            .to_string(),
        )])
        .await;
        let web = SlackWeb::with_base(&base).unwrap();
        let info = web
            .auth_test(&SlackBotToken::parse(BOT).unwrap())
            .await
            .unwrap();
        assert_eq!(info.team_id, "T1");
        assert_eq!(info.team, "Acme");
        assert_eq!(info.user_id, "UBOT");
        assert_eq!(info.user, "shodh");
        assert_eq!(info.bot_id.as_deref(), Some("BBOT"));
        assert_eq!(
            info.scopes.unwrap(),
            vec!["app_mentions:read", "chat:write", "im:history"]
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].path, "/api/auth.test");
        assert!(seen[0]
            .head
            .to_lowercase()
            .contains(&format!("authorization: bearer {}", BOT.to_lowercase())));
        assert!(!seen[0].path.contains("secret") && !seen[0].body.contains("secret"));
    }

    #[tokio::test]
    async fn a_missing_scope_header_is_unknown_not_empty() {
        let (base, _) = serve(vec![(
            "200 OK",
            vec![],
            json!({"ok": true, "team": "Acme", "user": "shodh", "team_id": "T1", "user_id": "UBOT"})
                .to_string(),
        )])
        .await;
        let info = SlackWeb::with_base(&base)
            .unwrap()
            .auth_test(&SlackBotToken::parse(BOT).unwrap())
            .await
            .unwrap();
        assert_eq!(info.scopes, None);
        assert_eq!(info.bot_id, None);
    }

    #[tokio::test]
    async fn slack_errors_are_typed_and_never_carry_the_token() {
        let (base, _) = serve(vec![
            (
                "200 OK",
                vec![],
                json!({"ok": false, "error": "invalid_auth"}).to_string(),
            ),
            (
                "429 Too Many Requests",
                vec![("Retry-After", "30")],
                String::new(),
            ),
            (
                "200 OK",
                vec![],
                json!({"ok": false, "error": "missing_scope", "needed": "im:write"}).to_string(),
            ),
            ("500 Internal Server Error", vec![], String::new()),
        ])
        .await;
        let web = SlackWeb::with_base(&base).unwrap();
        let app = SlackAppToken::parse(APP).unwrap();
        let bot = SlackBotToken::parse(BOT).unwrap();
        let invalid = web.open_connection(&app).await.unwrap_err();
        assert!(invalid.is_auth());
        assert_eq!(invalid.code(), Some("invalid_auth"));
        assert_eq!(
            web.open_connection(&app).await.unwrap_err(),
            SlackApiError::RateLimited {
                retry_after: Some(Duration::from_secs(30))
            }
        );
        let missing = web.post_message(&bot, "C1", None, "hi").await.unwrap_err();
        assert_eq!(
            missing,
            SlackApiError::Api {
                code: "missing_scope".into(),
                needed: Some("im:write".into())
            }
        );
        let status = web
            .update_message(&bot, "C1", "1.1", "hi")
            .await
            .unwrap_err();
        assert_eq!(status, SlackApiError::Status(500));
        assert!(!status.is_auth());
        for e in [invalid, missing, status] {
            assert!(!format!("{e} {e:?}").contains("secret"));
        }
    }

    #[test]
    fn transient_failures_are_told_apart() {
        assert!(SlackApiError::Network("reset".into()).is_transient());
        assert!(SlackApiError::Status(503).is_transient());
        assert!(!SlackApiError::Status(404).is_transient());
        assert!(!SlackApiError::RateLimited { retry_after: None }.is_transient());
        assert!(!SlackApiError::Api {
            code: "channel_not_found".into(),
            needed: None
        }
        .is_transient());
    }

    #[tokio::test]
    async fn people_bots_and_channels_are_read_form_encoded() {
        let (base, seen) = serve(vec![
            (
                "200 OK",
                vec![],
                json!({"ok": true, "bot": {"id": "B1", "app_id": "A1", "user_id": "UBOT"}})
                    .to_string(),
            ),
            (
                "200 OK",
                vec![],
                json!({"ok": true, "user": {"id": "U2", "team_id": "T1", "is_restricted": true,
                       "is_bot": false, "deleted": false}})
                .to_string(),
            ),
            (
                "200 OK",
                vec![],
                json!({"ok": true, "channels": [
                    {"id": "C2", "name": "zeta", "is_private": true, "is_ext_shared": true},
                    {"id": "C1", "name": "alpha", "is_private": false},
                    {"id": "C9", "name": "old", "is_archived": true}
                ], "response_metadata": {"next_cursor": "dXNlcjpVMDYxTkZUVDI="}})
                .to_string(),
            ),
            (
                "200 OK",
                vec![],
                json!({"ok": true, "channels": [
                    {"id": "C3", "name": "client-b", "is_pending_ext_shared": true}
                ], "response_metadata": {"next_cursor": ""}})
                .to_string(),
            ),
        ])
        .await;
        let web = SlackWeb::with_base(&base).unwrap();
        let bot = SlackBotToken::parse(BOT).unwrap();
        assert_eq!(web.bot_app_id(&bot, "B1").await.unwrap(), "A1");
        let member = web.user_info(&bot, "U2").await.unwrap();
        assert_eq!(member.team_id, "T1");
        assert!(member.guest && !member.stranger && !member.bot);
        let channels = web.bot_channels(&bot).await.unwrap();
        assert_eq!(
            channels,
            vec![
                BotChannel {
                    id: "C1".into(),
                    name: "alpha".into(),
                    private: false,
                    shared: false
                },
                BotChannel {
                    id: "C3".into(),
                    name: "client-b".into(),
                    private: false,
                    shared: true
                },
                BotChannel {
                    id: "C2".into(),
                    name: "zeta".into(),
                    private: true,
                    shared: true
                },
            ]
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].path, "/api/bots.info");
        assert_eq!(seen[0].body, "bot=B1");
        assert!(seen[0]
            .head
            .to_lowercase()
            .contains("content-type: application/x-www-form-urlencoded"));
        assert_eq!(seen[1].body, "user=U2");
        assert_eq!(seen[2].path, "/api/users.conversations");
        assert!(seen[2]
            .body
            .contains("types=public_channel%2Cprivate_channel"));
        assert!(!seen[2].body.contains("cursor"));
        assert!(seen[3].body.contains("cursor=dXNlcjpVMDYxTkZUVDI%3D"));
        for s in seen.iter() {
            assert!(!s.body.contains("secret") && !s.path.contains("secret"));
        }
    }

    #[tokio::test]
    async fn history_threads_permalinks_and_people_are_read() {
        let (base, seen) = serve(vec![
            (
                "200 OK",
                vec![],
                json!({"ok": true, "messages": [
                    {"ts": "20.0", "user": "U1", "text": "hi", "reply_count": 1, "latest_reply": "21.0", "thread_ts": "20.0"},
                    {"ts": "19.0", "user": "U2", "text": "yo"}
                ], "has_more": true, "response_metadata": {"next_cursor": "bmV4dA=="}})
                .to_string(),
            ),
            (
                "200 OK",
                vec![],
                json!({"ok": true, "messages": [
                    {"ts": "20.0", "user": "U1", "text": "hi", "thread_ts": "20.0", "reply_count": 1},
                    {"ts": "21.0", "user": "U2", "text": "reply", "thread_ts": "20.0"}
                ], "has_more": false, "response_metadata": {"next_cursor": ""}})
                .to_string(),
            ),
            (
                "200 OK",
                vec![],
                json!({"ok": true, "channel": {"id": "C1", "name": "deals", "is_archived": true,
                       "is_private": true, "is_ext_shared": false}})
                .to_string(),
            ),
            (
                "200 OK",
                vec![],
                json!({"ok": true, "channel": "C1",
                       "permalink": "https://acme.slack.com/archives/C1/p200"})
                .to_string(),
            ),
            (
                "200 OK",
                vec![],
                json!({"ok": true, "members": [
                    {"id": "U2", "team_id": "T1", "profile": {"display_name": "", "real_name": "Bob Ray", "email": "bob@example.com"}},
                    {"id": "U1", "team_id": "T1", "profile": {"display_name": "ann"}},
                    {"id": "U3", "team_id": "T1", "is_restricted": true, "profile": {"display_name": "guest"}},
                    {"id": "U4", "team_id": "TCLIENT", "profile": {"display_name": "client"}},
                    {"id": "UB", "team_id": "T1", "is_bot": true, "profile": {"display_name": "bot"}},
                    {"id": "USLACKBOT", "team_id": "T1", "profile": {"display_name": "Slackbot"}}
                ], "response_metadata": {"next_cursor": ""}})
                .to_string(),
            ),
            (
                "429 Too Many Requests",
                vec![("Retry-After", "42")],
                json!({"ok": false, "error": "ratelimited"}).to_string(),
            ),
        ])
        .await;
        let web = SlackWeb::with_base(&base).unwrap();
        let bot = SlackBotToken::parse(BOT).unwrap();
        let page = web
            .history(&bot, "C1", Some("10.0"), Some("30.0"), None)
            .await
            .unwrap();
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[0].reply_count, 1);
        assert_eq!(page.next_cursor.as_deref(), Some("bmV4dA=="));
        let thread = web
            .replies(&bot, "C1", "20.0", Some("bmV4dA=="))
            .await
            .unwrap();
        assert_eq!(thread.messages.len(), 2);
        assert_eq!(thread.next_cursor, None, "no more pages");
        let info = web.channel_info(&bot, "C1").await.unwrap();
        assert_eq!(
            info,
            ChannelInfo {
                name: "deals".into(),
                archived: true,
                shared: false,
                private: true
            }
        );
        assert_eq!(
            web.permalink(&bot, "C1", "20.0").await.unwrap(),
            "https://acme.slack.com/archives/C1/p200"
        );
        let people = web.people(&bot, "T1").await.unwrap();
        assert_eq!(
            people,
            vec![
                Person {
                    id: "U1".into(),
                    name: "ann".into()
                },
                Person {
                    id: "U2".into(),
                    name: "Bob Ray".into()
                },
            ],
            "full members of the workspace only, by name, never an e-mail"
        );
        assert_eq!(
            web.history(&bot, "C1", None, None, None).await,
            Err(SlackApiError::RateLimited {
                retry_after: Some(Duration::from_secs(42))
            })
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].path, "/api/conversations.history");
        assert!(seen[0].body.contains("channel=C1"));
        assert!(seen[0].body.contains("limit=200"));
        assert!(seen[0].body.contains("oldest=10.0") && seen[0].body.contains("latest=30.0"));
        assert!(!seen[0].body.contains("cursor"));
        assert_eq!(seen[1].path, "/api/conversations.replies");
        assert!(seen[1].body.contains("ts=20.0") && seen[1].body.contains("cursor=bmV4dA%3D%3D"));
        assert_eq!(seen[2].path, "/api/conversations.info");
        assert_eq!(seen[3].path, "/api/chat.getPermalink");
        assert!(seen[3].body.contains("message_ts=20.0"));
        assert_eq!(seen[4].path, "/api/users.list");
    }

    #[tokio::test]
    async fn messages_are_posted_in_threads_and_edited() {
        let (base, seen) = serve(vec![
            (
                "200 OK",
                vec![],
                json!({"ok": true, "url": "wss://wss.slack.com/link/?ticket=1"}).to_string(),
            ),
            (
                "200 OK",
                vec![],
                json!({"ok": true, "ts": "123.456", "channel": "C1"}).to_string(),
            ),
            ("200 OK", vec![], json!({"ok": true}).to_string()),
        ])
        .await;
        let web = SlackWeb::with_base(&base).unwrap();
        let app = SlackAppToken::parse(APP).unwrap();
        let bot = SlackBotToken::parse(BOT).unwrap();
        assert_eq!(
            web.open_connection(&app).await.unwrap(),
            "wss://wss.slack.com/link/?ticket=1"
        );
        let poster = BotPoster {
            web: web.clone(),
            token: bot,
        };
        let ts = poster
            .post("C1", Some("100.1"), "Searching…")
            .await
            .unwrap();
        assert_eq!(ts, "123.456");
        poster.update("C1", &ts, "The answer").await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].path, "/api/apps.connections.open");
        assert!(seen[0]
            .head
            .to_lowercase()
            .contains(&format!("authorization: bearer {}", APP.to_lowercase())));
        let post: Value = serde_json::from_str(&seen[1].body).unwrap();
        assert_eq!(post["channel"], "C1");
        assert_eq!(post["thread_ts"], "100.1");
        assert_eq!(post["unfurl_links"], false);
        let update: Value = serde_json::from_str(&seen[2].body).unwrap();
        assert_eq!(
            update,
            json!({"channel": "C1", "ts": "123.456", "text": "The answer"})
        );
    }
}
