//! The bot token @BotFather gives (`123456789:AAE…`), checked before it is stored and never
//! printed.
//!
//! Its shape: the bot's numeric id, a colon, then 35 characters of `A-Z a-z 0-9 _ -`
//! (<https://core.telegram.org/bots/api#authorizing-your-bot>). The type implements neither
//! `Display` nor `Serialize`, and `Debug` prints `[REDACTED]`: the only way to the secret
//! is [`TelegramBotToken::expose`], used where a request is sent to Telegram.

use std::fmt;

/// Characters after the colon.
pub const SECRET_CHARS: usize = 35;

/// Why a pasted token was refused. The message never contains the token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("Paste the bot token @BotFather sent you.")]
    Empty,
    #[error(
        "That doesn't look like a bot token. It looks like 123456789:AAH… : digits, a colon, then 35 letters, digits, - or _. Check that the whole token was copied."
    )]
    Malformed,
}

/// The bot token.
#[derive(Clone, PartialEq, Eq)]
pub struct TelegramBotToken(String);

impl TelegramBotToken {
    /// Checks a pasted token (surrounding spaces are ignored).
    pub fn parse(raw: &str) -> Result<Self, TokenError> {
        let token = raw.trim();
        if token.is_empty() {
            return Err(TokenError::Empty);
        }
        let Some((id, secret)) = token.split_once(':') else {
            return Err(TokenError::Malformed);
        };
        let id_ok = (1..=20).contains(&id.len())
            && id.chars().all(|c| c.is_ascii_digit())
            && id.parse::<i64>().is_ok_and(|n| n > 0);
        let secret_ok = secret.chars().count() == SECRET_CHARS
            && secret
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !id_ok || !secret_ok {
            return Err(TokenError::Malformed);
        }
        Ok(Self(token.to_string()))
    }

    /// The bot's user id (the part before the colon; public, not secret).
    pub fn bot_id(&self) -> i64 {
        self.0
            .split_once(':')
            .and_then(|(id, _)| id.parse().ok())
            .unwrap_or_default()
    }

    /// The secret, for the request to Telegram only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for TelegramBotToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TelegramBotToken([REDACTED])")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const TOKEN: &str = "123456789:AAH4f9Zk2_x-QwErTyUiOpAsDfGhJkLzXcV";

    #[test]
    fn a_bot_token_has_its_shape() {
        assert_eq!(TOKEN.split_once(':').unwrap().1.len(), SECRET_CHARS);
        let token = TelegramBotToken::parse(&format!("  {TOKEN}\n")).unwrap();
        assert_eq!(token.expose(), TOKEN);
        assert_eq!(token.bot_id(), 123_456_789);
        assert_eq!(TelegramBotToken::parse(" "), Err(TokenError::Empty));
        for bad in [
            "123456789",
            "123456789:short",
            "abc:AAH4f9Zk2_x-QwErTyUiOpAsDfGhJkLzXcV",
            "123456789:AAH4f9Zk2_x-QwErTyUiOpAsDfGhJkLzXc!",
            "123456789:AAH4f9Zk2_x-QwErTyUiOpAsDfGhJkLzXcVV",
            "0:AAH4f9Zk2_x-QwErTyUiOpAsDfGhJkLzXcV",
            "xoxb-1-2-3",
        ] {
            assert_eq!(
                TelegramBotToken::parse(bad),
                Err(TokenError::Malformed),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_token_is_never_printed() {
        let token = TelegramBotToken::parse(TOKEN).unwrap();
        let debug = format!("{token:?}");
        assert!(!debug.contains("AAH4"), "{debug}");
        assert!(debug.contains("REDACTED"));
        // Refusals never echo what was pasted.
        let error = TelegramBotToken::parse("123456789:AAH4f9Zk2_x-QwErTyUiOpAsDfGhJkLz")
            .unwrap_err()
            .to_string();
        assert!(!error.contains("AAH4f9"), "{error}");
    }
}
