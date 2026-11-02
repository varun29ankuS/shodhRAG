//! The GitHub sign-in a device flow gives: a user access token (`ghu_…`) and, only when
//! the GitHub App issues expiring tokens, a refresh token (`ghr_…`) and their lifetimes.
//!
//! Neither token type implements `Display` or `Serialize`, and `Debug` prints
//! `[REDACTED]`: the only way to a secret is [`UserToken::expose`] /
//! [`RefreshToken::expose`], used where a request is sent to GitHub. The stored form
//! ([`StoredSignIn`]) is written to the OS credential store only.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Longest token accepted (GitHub's are 40 to a few hundred characters).
const MAX_TOKEN_CHARS: usize = 1_024;

/// Why a token GitHub returned (or the credential store held) was refused. Never holds
/// the token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("GitHub returned no access token")]
    Empty,
    #[error("the GitHub token has an unexpected form")]
    Malformed,
}

fn check(raw: &str) -> Result<String, TokenError> {
    let token = raw.trim();
    if token.is_empty() {
        return Err(TokenError::Empty);
    }
    if token.chars().count() > MAX_TOKEN_CHARS
        || !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(TokenError::Malformed);
    }
    Ok(token.to_string())
}

/// A user access token of the Shodh GitHub App.
#[derive(Clone, PartialEq, Eq)]
pub struct UserToken(String);

impl UserToken {
    /// Checks a token (only the characters GitHub uses; surrounding spaces ignored).
    pub fn parse(raw: &str) -> Result<Self, TokenError> {
        check(raw).map(Self)
    }

    /// The secret, for a request to GitHub only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for UserToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UserToken([REDACTED])")
    }
}

/// A refresh token (only when the app's tokens expire).
#[derive(Clone, PartialEq, Eq)]
pub struct RefreshToken(String);

impl RefreshToken {
    pub fn parse(raw: &str) -> Result<Self, TokenError> {
        check(raw).map(Self)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RefreshToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshToken([REDACTED])")
    }
}

/// What a sign-in (or a refresh) returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignIn {
    pub access: UserToken,
    /// When the access token stops working; `None`: it does not expire.
    pub expires_at: Option<DateTime<Utc>>,
    pub refresh: Option<RefreshToken>,
    pub refresh_expires_at: Option<DateTime<Utc>>,
}

impl SignIn {
    /// Whether the access token has expired (or will within `margin`) and can be renewed.
    pub fn needs_refresh(&self, now: DateTime<Utc>, margin: chrono::Duration) -> bool {
        self.refresh.is_some() && self.expires_at.is_some_and(|at| at - margin <= now)
    }

    /// The form kept in the credential store (JSON). Written there only.
    pub fn to_stored(&self) -> String {
        let stored = StoredSignIn {
            access: self.access.expose().to_string(),
            expires_at: self.expires_at,
            refresh: self.refresh.as_ref().map(|r| r.expose().to_string()),
            refresh_expires_at: self.refresh_expires_at,
        };
        serde_json::to_string(&stored).unwrap_or_default()
    }

    /// Reads the credential store's form back. The error never holds its text.
    pub fn from_stored(text: &str) -> Result<Self, TokenError> {
        let stored: StoredSignIn = serde_json::from_str(text).map_err(|_| TokenError::Malformed)?;
        Ok(Self {
            access: UserToken::parse(&stored.access)?,
            expires_at: stored.expires_at,
            refresh: stored
                .refresh
                .as_deref()
                .map(RefreshToken::parse)
                .transpose()?,
            refresh_expires_at: stored.refresh_expires_at,
        })
    }
}

/// The credential store's JSON. Private: nothing else serialises a token.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSignIn {
    access: String,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    refresh: Option<String>,
    #[serde(default)]
    refresh_expires_at: Option<DateTime<Utc>>,
}

/// `text` with every copy of `secret` replaced (for a message that might quote it).
pub fn scrub(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "[token]")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const TOKEN: &str = "ghu_16C7e42F292c6912E7710c838347Ae178B4a";
    pub const REFRESH: &str =
        "ghr_1B4a2e77838347a7E420ce178F2E7c6912E169246c34E1ccbF66C46812d16D5B1A9Dc86A1498";

    #[test]
    fn tokens_are_never_printed() {
        let token = UserToken::parse(TOKEN).unwrap();
        let refresh = RefreshToken::parse(REFRESH).unwrap();
        let sign_in = SignIn {
            access: token.clone(),
            expires_at: None,
            refresh: Some(refresh),
            refresh_expires_at: None,
        };
        let debug = format!("{token:?} {sign_in:?}");
        assert!(!debug.contains("ghu_16C7"), "{debug}");
        assert!(!debug.contains("ghr_1B4a"), "{debug}");
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn a_token_has_only_githubs_characters() {
        assert_eq!(
            UserToken::parse(&format!(" {TOKEN}\n")).unwrap().expose(),
            TOKEN
        );
        assert_eq!(UserToken::parse("  "), Err(TokenError::Empty));
        for bad in ["ghu_a b", "ghu_a\nb", "ghu_a:b", "ghu_a/b"] {
            assert_eq!(UserToken::parse(bad), Err(TokenError::Malformed), "{bad}");
        }
        assert_eq!(
            UserToken::parse(&"a".repeat(MAX_TOKEN_CHARS + 1)),
            Err(TokenError::Malformed)
        );
    }

    #[test]
    fn the_stored_form_round_trips_and_its_errors_never_quote_it() {
        let sign_in = SignIn {
            access: UserToken::parse(TOKEN).unwrap(),
            expires_at: Some(Utc::now()),
            refresh: Some(RefreshToken::parse(REFRESH).unwrap()),
            refresh_expires_at: None,
        };
        let stored = sign_in.to_stored();
        assert_eq!(SignIn::from_stored(&stored).unwrap(), sign_in);
        let damaged = stored.replace("access", "acess");
        let error = SignIn::from_stored(&damaged).unwrap_err().to_string();
        assert!(!error.contains("ghu_"), "{error}");
    }

    #[test]
    fn a_token_without_expiry_never_needs_refreshing() {
        let now = Utc::now();
        let mut sign_in = SignIn {
            access: UserToken::parse(TOKEN).unwrap(),
            expires_at: None,
            refresh: None,
            refresh_expires_at: None,
        };
        assert!(!sign_in.needs_refresh(now, chrono::Duration::minutes(5)));
        sign_in.expires_at = Some(now + chrono::Duration::minutes(2));
        assert!(
            !sign_in.needs_refresh(now, chrono::Duration::minutes(5)),
            "no refresh token: nothing to renew with"
        );
        sign_in.refresh = Some(RefreshToken::parse(REFRESH).unwrap());
        assert!(sign_in.needs_refresh(now, chrono::Duration::minutes(5)));
        assert!(!sign_in.needs_refresh(now, chrono::Duration::seconds(30)));
    }

    #[test]
    fn scrub_replaces_the_secret() {
        assert_eq!(scrub(&format!("a {TOKEN} b"), TOKEN), "a [token] b");
        assert_eq!(scrub("a", ""), "a");
    }
}
