//! The few Telegram Bot API methods the connector calls
//! (<https://core.telegram.org/bots/api>): `getMe`, `getUpdates` (long polling),
//! `sendMessage` and `sendChatAction`.
//!
//! **The token is in every URL** (`https://api.telegram.org/bot<token>/<method>`). So:
//! the URL is built inside one function and never stored, every reqwest error is turned
//! into text with `without_url()`, any text that still carries the token is scrubbed
//! ([`scrub`]), and the client's `Debug` shows only the host.
//!
//! **Errors** come as JSON (`ok: false`, `error_code`, `description`, `parameters`):
//! - 429 carries `parameters.retry_after` (seconds) in the body, not in a header;
//! - `parameters.migrate_to_chat_id`: a group became a supergroup with a new chat id;
//! - 401: the token was revoked (or never valid);
//! - 409: a webhook is set, or another program is polling with this token. Both are the
//!   user's to fix, so the connector stops and says so instead of looping.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::token::TelegramBotToken;

/// Telegram's Bot API.
pub const TELEGRAM_API: &str = "https://api.telegram.org/";

/// Seconds a `getUpdates` call waits for an update before it returns empty.
pub const LONG_POLL_SECONDS: u32 = 30;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A long poll's own timeout: the poll's wait plus room for the answer.
const POLL_TIMEOUT: Duration = Duration::from_secs(LONG_POLL_SECONDS as u64 + 15);

/// What a 429 without `retry_after` waits.
pub const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(5);

/// The updates the connector asks for: messages (with a photo's caption), and the bot's
/// own membership in chats (added to or removed from a group).
pub const ALLOWED_UPDATES: [&str; 2] = ["message", "my_chat_member"];

/// Why a Telegram call failed. No variant ever holds a URL or the token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TelegramError {
    #[error("Telegram refused the bot token (it was revoked, or the bot was deleted)")]
    Unauthorized,
    #[error(
        "another program is reading this bot's messages (a webhook, or another copy of Shodh)"
    )]
    Conflict,
    #[error("Telegram is rate limiting the bot")]
    RateLimited { retry_after: Duration },
    #[error("the group moved to a new chat")]
    Migrated { to_chat: i64 },
    #[error("Telegram refused the request ({status}): {description}")]
    Api { status: u16, description: String },
    #[error("Telegram could not be reached: {0}")]
    Network(String),
    #[error("Telegram's answer could not be read: {0}")]
    BadResponse(String),
}

impl TelegramError {
    /// A failure on the way to Telegram or inside it, which one more try may get past.
    pub fn is_transient(&self) -> bool {
        match self {
            TelegramError::Network(_) => true,
            TelegramError::Api { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Telegram could not read the HTML of a message (sent again as plain text).
    pub fn is_bad_markup(&self) -> bool {
        matches!(self, TelegramError::Api { status: 400, description } if description.to_ascii_lowercase().contains("can't parse entities"))
    }
}

/// `text` with every copy of the token, and of its secret part, replaced.
pub fn scrub(text: &str, token: &TelegramBotToken) -> String {
    let secret = token.expose();
    let mut out = text.replace(secret, "[token]");
    if let Some((_, part)) = secret.split_once(':') {
        out = out.replace(part, "[token]");
    }
    out
}

/// Who the bot is (`getMe`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotInfo {
    pub id: i64,
    /// Without the `@`.
    pub username: String,
    pub name: String,
    /// Privacy mode is off: the bot receives every message of its groups.
    pub reads_all_group_messages: bool,
    pub can_join_groups: bool,
}

/// A client of the Bot API.
#[derive(Clone)]
pub struct TelegramApi {
    client: reqwest::Client,
    poll_client: reqwest::Client,
    base: String,
}

impl std::fmt::Debug for TelegramApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The base never holds a token (that is added per call).
        f.debug_struct("TelegramApi")
            .field("base", &self.base)
            .finish()
    }
}

fn client(timeout: Duration) -> Result<reqwest::Client, TelegramError> {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("Shodh/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| TelegramError::Network(e.without_url().to_string()))
}

impl TelegramApi {
    /// The client for Telegram itself.
    pub fn new() -> Result<Self, TelegramError> {
        Self::with_base(TELEGRAM_API)
    }

    /// A client for the API at `base` (tests use a local server).
    pub fn with_base(base: &str) -> Result<Self, TelegramError> {
        let mut base = base.to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        Ok(Self {
            client: client(REQUEST_TIMEOUT)?,
            poll_client: client(POLL_TIMEOUT)?,
            base,
        })
    }

    async fn call(
        &self,
        token: &TelegramBotToken,
        method: &str,
        body: Value,
        long: bool,
    ) -> Result<Value, TelegramError> {
        let client = if long {
            &self.poll_client
        } else {
            &self.client
        };
        // The only place the URL exists; it is never logged, kept or put in an error.
        let response = client
            .post(format!("{}bot{}/{method}", self.base, token.expose()))
            .json(&body)
            .send()
            .await
            .map_err(|e| TelegramError::Network(scrub(&e.without_url().to_string(), token)))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| TelegramError::Network(scrub(&e.without_url().to_string(), token)))?;
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(e) => {
                return Err(match status {
                    429 => TelegramError::RateLimited {
                        retry_after: DEFAULT_RETRY_AFTER,
                    },
                    401 => TelegramError::Unauthorized,
                    409 => TelegramError::Conflict,
                    200..=299 => TelegramError::BadResponse(e.to_string()),
                    _ => TelegramError::Api {
                        status,
                        description: format!("HTTP {status}"),
                    },
                })
            }
        };
        if value.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(error_of(&value, status, token))
    }

    /// Who the bot is.
    pub async fn get_me(&self, token: &TelegramBotToken) -> Result<BotInfo, TelegramError> {
        let me = self.call(token, "getMe", json!({}), false).await?;
        bot_info(&me)
    }
}

/// The error a failed call's JSON describes.
fn error_of(value: &Value, status: u16, token: &TelegramBotToken) -> TelegramError {
    let code = value
        .get("error_code")
        .and_then(Value::as_u64)
        .and_then(|c| u16::try_from(c).ok())
        .unwrap_or(status);
    let parameters = value.get("parameters");
    if let Some(to) = parameters
        .and_then(|p| p.get("migrate_to_chat_id"))
        .and_then(Value::as_i64)
    {
        return TelegramError::Migrated { to_chat: to };
    }
    match code {
        429 => TelegramError::RateLimited {
            retry_after: parameters
                .and_then(|p| p.get("retry_after"))
                .and_then(Value::as_u64)
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_RETRY_AFTER),
        },
        401 => TelegramError::Unauthorized,
        409 => TelegramError::Conflict,
        _ => TelegramError::Api {
            status: code,
            description: scrub(
                value
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("no description"),
                token,
            ),
        },
    }
}

fn bot_info(me: &Value) -> Result<BotInfo, TelegramError> {
    let id = me
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| TelegramError::BadResponse("getMe had no id".into()))?;
    let username = me
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if username.is_empty() || me.get("is_bot").and_then(Value::as_bool) != Some(true) {
        return Err(TelegramError::BadResponse(
            "that token does not belong to a bot".into(),
        ));
    }
    let flag = |key: &str| me.get(key).and_then(Value::as_bool).unwrap_or(false);
    Ok(BotInfo {
        id,
        name: me
            .get("first_name")
            .and_then(Value::as_str)
            .unwrap_or(&username)
            .to_string(),
        username,
        reads_all_group_messages: flag("can_read_all_group_messages"),
        can_join_groups: flag("can_join_groups"),
    })
}

/// What the connector does with the Bot API (a fake in tests).
#[async_trait]
pub trait BotApi: Send + Sync {
    /// Updates after `offset` (all unconfirmed ones with `None`), waiting up to `wait`
    /// seconds for one.
    async fn updates(&self, offset: Option<i64>, wait: u32) -> Result<Vec<Value>, TelegramError>;
    /// Sends `text` to `chat` (as HTML when `html`), as a reply to `reply_to` when given;
    /// returns the sent message's id.
    async fn send(
        &self,
        chat: i64,
        text: &str,
        html: bool,
        reply_to: Option<i64>,
    ) -> Result<i64, TelegramError>;
    /// Shows "typing…" in `chat` (Telegram shows it for about five seconds).
    async fn typing(&self, chat: i64) -> Result<(), TelegramError>;
}

/// The Bot API with the bot's token.
pub struct TokenBot {
    pub api: TelegramApi,
    pub token: TelegramBotToken,
}

impl std::fmt::Debug for TokenBot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenBot")
            .field("api", &self.api)
            .field("token", &self.token)
            .finish()
    }
}

#[async_trait]
impl BotApi for TokenBot {
    async fn updates(&self, offset: Option<i64>, wait: u32) -> Result<Vec<Value>, TelegramError> {
        let mut body = json!({ "timeout": wait, "allowed_updates": ALLOWED_UPDATES });
        if let Some(offset) = offset {
            body["offset"] = json!(offset);
        }
        let result = self.api.call(&self.token, "getUpdates", body, true).await?;
        match result {
            Value::Array(updates) => Ok(updates),
            _ => Err(TelegramError::BadResponse("getUpdates gave no list".into())),
        }
    }

    async fn send(
        &self,
        chat: i64,
        text: &str,
        html: bool,
        reply_to: Option<i64>,
    ) -> Result<i64, TelegramError> {
        let mut body = json!({
            "chat_id": chat,
            "text": text,
            "link_preview_options": { "is_disabled": true },
        });
        if html {
            body["parse_mode"] = json!("HTML");
        }
        if let Some(message) = reply_to {
            body["reply_parameters"] =
                json!({ "message_id": message, "allow_sending_without_reply": true });
        }
        let sent = self
            .api
            .call(&self.token, "sendMessage", body, false)
            .await?;
        sent.get("message_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| TelegramError::BadResponse("sendMessage gave no message id".into()))
    }

    async fn typing(&self, chat: i64) -> Result<(), TelegramError> {
        self.api
            .call(
                &self.token,
                "sendChatAction",
                json!({ "chat_id": chat, "action": "typing" }),
                false,
            )
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::slack::web::fake::serve;
    use crate::connectors::telegram::token::tests::TOKEN;

    fn token() -> TelegramBotToken {
        TelegramBotToken::parse(TOKEN).unwrap()
    }

    fn bot(base: &str) -> TokenBot {
        TokenBot {
            api: TelegramApi::with_base(base).unwrap(),
            token: token(),
        }
    }

    #[tokio::test]
    async fn get_me_reads_the_bot() {
        let (base, seen) = serve(vec![(
            "200 OK",
            vec![],
            json!({"ok": true, "result": {"id": 123456789, "is_bot": true, "first_name": "Shodh",
                "username": "shodh_notes_bot", "can_join_groups": true,
                "can_read_all_group_messages": false}})
            .to_string(),
        )])
        .await;
        let api = TelegramApi::with_base(&base).unwrap();
        let me = api.get_me(&token()).await.unwrap();
        assert_eq!(me.username, "shodh_notes_bot");
        assert_eq!(me.name, "Shodh");
        assert!(!me.reads_all_group_messages);
        let request = seen.lock().unwrap()[0].clone();
        assert_eq!(request.path, format!("/api/bot{TOKEN}/getMe"));
        // The client's Debug never shows a token.
        assert!(!format!("{:?}", bot(&base)).contains("AAH4"));
    }

    #[tokio::test]
    async fn a_user_account_token_is_refused() {
        let (base, _) = serve(vec![(
            "200 OK",
            vec![],
            json!({"ok": true, "result": {"id": 5, "is_bot": false, "first_name": "Ann"}})
                .to_string(),
        )])
        .await;
        let error = TelegramApi::with_base(&base)
            .unwrap()
            .get_me(&token())
            .await
            .unwrap_err();
        assert!(matches!(error, TelegramError::BadResponse(_)));
    }

    #[tokio::test]
    async fn telegrams_errors_are_read_from_the_body() {
        let (base, _) = serve(vec![
            (
                "429 Too Many Requests",
                vec![],
                json!({"ok": false, "error_code": 429, "description": "Too Many Requests: retry after 17",
                    "parameters": {"retry_after": 17}})
                .to_string(),
            ),
            (
                "409 Conflict",
                vec![],
                json!({"ok": false, "error_code": 409,
                    "description": "Conflict: terminated by other getUpdates request"})
                .to_string(),
            ),
            (
                "401 Unauthorized",
                vec![],
                json!({"ok": false, "error_code": 401, "description": "Unauthorized"}).to_string(),
            ),
            (
                "400 Bad Request",
                vec![],
                json!({"ok": false, "error_code": 400, "description": "Bad Request: group chat was upgraded to a supergroup chat",
                    "parameters": {"migrate_to_chat_id": -1001234567890_i64}})
                .to_string(),
            ),
            (
                "400 Bad Request",
                vec![],
                json!({"ok": false, "error_code": 400,
                    "description": "Bad Request: can't parse entities: Unsupported start tag"})
                .to_string(),
            ),
            ("502 Bad Gateway", vec![], "<html>bad gateway</html>".to_string()),
        ])
        .await;
        let bot = bot(&base);
        assert_eq!(
            bot.updates(None, 0).await.unwrap_err(),
            TelegramError::RateLimited {
                retry_after: Duration::from_secs(17)
            }
        );
        assert_eq!(
            bot.updates(None, 0).await.unwrap_err(),
            TelegramError::Conflict
        );
        assert_eq!(
            bot.typing(1).await.unwrap_err(),
            TelegramError::Unauthorized
        );
        assert_eq!(
            bot.send(-42, "hi", false, None).await.unwrap_err(),
            TelegramError::Migrated {
                to_chat: -1001234567890
            }
        );
        let markup = bot.send(1, "<b>hi", true, None).await.unwrap_err();
        assert!(markup.is_bad_markup());
        let gateway = bot.send(1, "hi", false, None).await.unwrap_err();
        assert!(gateway.is_transient(), "{gateway:?}");
    }

    #[tokio::test]
    async fn sends_ask_for_html_replies_without_previews() {
        let (base, seen) = serve(vec![
            (
                "200 OK",
                vec![],
                json!({"ok": true, "result": {"message_id": 77}}).to_string(),
            ),
            (
                "200 OK",
                vec![],
                json!({"ok": true, "result": [{"update_id": 5}]}).to_string(),
            ),
        ])
        .await;
        let bot = bot(&base);
        assert_eq!(
            bot.send(-100, "<b>Hi</b>", true, Some(9)).await.unwrap(),
            77
        );
        let body: Value = serde_json::from_str(&seen.lock().unwrap()[0].body).unwrap();
        assert_eq!(body["parse_mode"], "HTML");
        assert_eq!(body["reply_parameters"]["message_id"], 9);
        assert_eq!(body["link_preview_options"]["is_disabled"], true);
        assert_eq!(bot.updates(Some(5), 30).await.unwrap().len(), 1);
        let body: Value = serde_json::from_str(&seen.lock().unwrap()[1].body).unwrap();
        assert_eq!(body["offset"], 5);
        assert_eq!(body["timeout"], 30);
        assert_eq!(
            body["allowed_updates"],
            json!(["message", "my_chat_member"])
        );
    }

    #[tokio::test]
    async fn a_network_error_never_carries_the_url_or_the_token() {
        // A port nothing listens on: reqwest's own error names the whole URL.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let base = format!("http://127.0.0.1:{port}/");
        let raw = reqwest::Client::new()
            .post(format!("{base}bot{TOKEN}/getMe"))
            .send()
            .await
            .unwrap_err()
            .to_string();
        assert!(
            raw.contains("AAH4"),
            "this test needs reqwest's error to name the URL: {raw}"
        );
        let error = bot(&base).typing(1).await.unwrap_err();
        assert!(matches!(error, TelegramError::Network(_)));
        let text = format!("{error} {error:?}");
        assert!(!text.contains("AAH4"), "{text}");
        assert!(!text.contains("123456789:"), "{text}");
        assert!(!text.contains("/bot"), "{text}");
    }

    #[test]
    fn scrub_removes_the_token_anywhere() {
        let token = token();
        let secret = TOKEN.split_once(':').unwrap().1;
        assert_eq!(
            scrub(&format!("x {TOKEN} y {secret}"), &token),
            "x [token] y [token]"
        );
    }
}
