//! The two Slack tokens the connector needs, checked before they are stored and never
//! printed.
//!
//! - The **app-level token** (`xapp-…`, scope `connections:write`) opens Socket Mode
//!   connections (`apps.connections.open`).
//! - The **bot token** (`xoxb-…`) reads who the bot is (`auth.test`) and posts answers
//!   (`chat.postMessage`, `chat.update`).
//!
//! Token kinds by prefix: <https://docs.slack.dev/authentication/tokens>. A user token
//! (`xoxp-`) would act as the person who installed the app, so it is refused with its own
//! message. Rotating tokens (`xoxe.`) expire within hours and need a refresh flow the
//! connector does not have, so they are refused too.
//!
//! The token types implement neither `Display` nor `Serialize`, and `Debug` prints
//! `[REDACTED]`: the only way to the secret is [`SlackAppToken::expose`] /
//! [`SlackBotToken::expose`], used where a request is sent to Slack.

use std::fmt;

/// Longest token accepted. Slack tokens are well under 200 characters; anything much longer
/// is a paste of something else.
const MAX_TOKEN_CHARS: usize = 512;

/// Why a pasted token was refused. The message never contains the token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("Paste the {0}.")]
    Empty(&'static str),
    #[error(
        "That is a user token (xoxp-). Shodh needs the bot token (xoxb-), from OAuth & Permissions."
    )]
    UserToken,
    #[error(
        "That is a rotating token (xoxe.). Turn token rotation off for the app and paste the {0}."
    )]
    Rotating(&'static str),
    #[error("That looks like the bot token. The app-level token starts with xapp-.")]
    BotTokenAsAppToken,
    #[error("That looks like the app-level token. The bot token starts with xoxb-.")]
    AppTokenAsBotToken,
    #[error("The {kind} starts with {prefix}. Check that the whole token was copied.")]
    WrongPrefix {
        kind: &'static str,
        prefix: &'static str,
    },
    #[error("The {0} contains spaces or characters a Slack token never has.")]
    Malformed(&'static str),
}

const APP_TOKEN: &str = "app-level token";
const BOT_TOKEN: &str = "bot token";

fn shape(raw: &str, kind: &'static str) -> Result<String, TokenError> {
    let token = raw.trim();
    if token.is_empty() {
        return Err(TokenError::Empty(kind));
    }
    if token.len() > MAX_TOKEN_CHARS
        || !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
    {
        return Err(TokenError::Malformed(kind));
    }
    if token.starts_with("xoxp-") {
        return Err(TokenError::UserToken);
    }
    if token.starts_with("xoxe.") || token.starts_with("xoxe-") {
        return Err(TokenError::Rotating(kind));
    }
    Ok(token.to_string())
}

/// The app-level token (`xapp-…`).
#[derive(Clone, PartialEq, Eq)]
pub struct SlackAppToken(String);

impl SlackAppToken {
    /// Checks a pasted app-level token (surrounding spaces are ignored).
    pub fn parse(raw: &str) -> Result<Self, TokenError> {
        let token = shape(raw, APP_TOKEN)?;
        if token.starts_with("xoxb-") {
            return Err(TokenError::BotTokenAsAppToken);
        }
        if !token.starts_with("xapp-") || token.len() <= "xapp-".len() {
            return Err(TokenError::WrongPrefix {
                kind: APP_TOKEN,
                prefix: "xapp-",
            });
        }
        Ok(Self(token))
    }

    /// The secret, for the request to Slack only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SlackAppToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SlackAppToken([REDACTED])")
    }
}

/// The bot token (`xoxb-…`).
#[derive(Clone, PartialEq, Eq)]
pub struct SlackBotToken(String);

impl SlackBotToken {
    /// Checks a pasted bot token (surrounding spaces are ignored).
    pub fn parse(raw: &str) -> Result<Self, TokenError> {
        let token = shape(raw, BOT_TOKEN)?;
        if token.starts_with("xapp-") {
            return Err(TokenError::AppTokenAsBotToken);
        }
        if !token.starts_with("xoxb-") || token.len() <= "xoxb-".len() {
            return Err(TokenError::WrongPrefix {
                kind: BOT_TOKEN,
                prefix: "xoxb-",
            });
        }
        Ok(Self(token))
    }

    /// The secret, for the request to Slack only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SlackBotToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SlackBotToken([REDACTED])")
    }
}

/// Both tokens of one installation. How they were obtained (pasted now, an "Add to Slack"
/// install later) is not this type's concern.
#[derive(Clone, PartialEq, Eq)]
pub struct SlackCredentials {
    pub app_token: SlackAppToken,
    pub bot_token: SlackBotToken,
}

impl fmt::Debug for SlackCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlackCredentials")
            .field("app_token", &self.app_token)
            .field("bot_token", &self.bot_token)
            .finish()
    }
}

/// Where an installation's credentials are kept, by Slack team id. The app keeps them in
/// the OS credential store; tests use memory.
pub trait SlackCredentialStore: Send + Sync {
    fn load(&self, team_id: &str) -> Result<Option<SlackCredentials>, String>;
    fn store(&self, team_id: &str, credentials: &SlackCredentials) -> Result<(), String>;
    /// Removing credentials that do not exist is not an error.
    fn remove(&self, team_id: &str) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "xapp-test-app-token-0123456789abcdef";
    const BOT: &str = "xoxb-test-bot-token-AbCdEfGhIjKl";

    #[test]
    fn valid_tokens_are_accepted_and_trimmed() {
        let app = SlackAppToken::parse(&format!("  {APP}\n")).unwrap();
        assert_eq!(app.expose(), APP);
        let bot = SlackBotToken::parse(&format!("\t{BOT} ")).unwrap();
        assert_eq!(bot.expose(), BOT);
    }

    #[test]
    fn user_rotating_and_swapped_tokens_are_refused_with_their_own_reason() {
        assert_eq!(
            SlackBotToken::parse("xoxp-123-456-789-abc"),
            Err(TokenError::UserToken)
        );
        assert_eq!(
            SlackAppToken::parse("xoxp-123-456-789-abc"),
            Err(TokenError::UserToken)
        );
        assert_eq!(
            SlackBotToken::parse("xoxe.xoxb-1-abc"),
            Err(TokenError::Rotating("bot token"))
        );
        assert_eq!(
            SlackAppToken::parse("xoxe.xapp-1-abc"),
            Err(TokenError::Rotating("app-level token"))
        );
        assert_eq!(
            SlackAppToken::parse(BOT),
            Err(TokenError::BotTokenAsAppToken)
        );
        assert_eq!(
            SlackBotToken::parse(APP),
            Err(TokenError::AppTokenAsBotToken)
        );
        assert!(matches!(
            SlackBotToken::parse("xoxc-123"),
            Err(TokenError::WrongPrefix { .. })
        ));
        assert!(matches!(
            SlackBotToken::parse("xoxb-"),
            Err(TokenError::WrongPrefix { .. })
        ));
        assert_eq!(
            SlackAppToken::parse("   "),
            Err(TokenError::Empty("app-level token"))
        );
        assert_eq!(
            SlackBotToken::parse("xoxb-12 34"),
            Err(TokenError::Malformed("bot token"))
        );
        assert_eq!(
            SlackBotToken::parse(&format!("xoxb-{}", "a".repeat(600))),
            Err(TokenError::Malformed("bot token"))
        );
    }

    #[test]
    fn errors_never_contain_the_token() {
        for raw in [
            "xoxp-secret-user-token-value",
            "xoxe.xoxb-secret-rotating",
            "xoxb-secret bot",
        ] {
            let message = SlackBotToken::parse(raw).unwrap_err().to_string();
            assert!(!message.contains("secret"), "{message}");
        }
    }

    #[test]
    fn debug_output_is_redacted() {
        let credentials = SlackCredentials {
            app_token: SlackAppToken::parse(APP).unwrap(),
            bot_token: SlackBotToken::parse(BOT).unwrap(),
        };
        let printed = format!("{credentials:?} {:?}", credentials.bot_token);
        assert!(printed.contains("[REDACTED]"));
        assert!(!printed.contains("xapp-"), "{printed}");
        assert!(!printed.contains("xoxb-"), "{printed}");
        assert!(!printed.contains("AbCdEf"), "{printed}");
    }

    /// The token types must never be serializable: a settings struct or a status sent to
    /// the frontend that held one would not compile. Checked through the trait bound.
    #[test]
    fn tokens_are_not_serializable() {
        trait NotSerialize {
            const SERIALIZABLE: bool = false;
        }
        impl<T: ?Sized> NotSerialize for T {}
        struct Probe<T: ?Sized>(std::marker::PhantomData<T>);
        #[allow(dead_code)]
        impl<T: ?Sized + serde::Serialize> Probe<T> {
            const SERIALIZABLE: bool = true;
        }
        assert!(!Probe::<SlackAppToken>::SERIALIZABLE);
        assert!(!Probe::<SlackBotToken>::SERIALIZABLE);
        assert!(!Probe::<SlackCredentials>::SERIALIZABLE);
        // The probe does tell a serializable type apart.
        assert!(Probe::<String>::SERIALIZABLE);
    }
}
