//! One-time codes that link a Telegram account to Shodh.
//!
//! The card shows a code as a link (`https://t.me/<bot>?start=<code>`, Telegram's deep
//! link: up to 64 characters of `A-Z a-z 0-9 _ -`,
//! <https://core.telegram.org/bots/features#deep-linking>) and as a QR code. Opening it
//! sends `/start <code>` to the bot from the person's private chat, and that links their
//! Telegram user id: as the owner (an owner code), or as someone allowed to ask (an invite
//! code). A code:
//! - is random (24 characters from the OS's random source), so it cannot be guessed;
//! - says what it is for, so an invite can never make someone the owner;
//! - works once, and only for [`CODE_TTL`] after it was made;
//! - replaces the previous code of its kind (only the newest one shown works).
//!
//! Codes live in memory only: a restart forgets them, and the card makes a new one.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use rand::distributions::Alphanumeric;
use rand::Rng;

/// How long a code works.
pub const CODE_TTL: Duration = Duration::minutes(10);
/// Characters of a code.
pub const CODE_CHARS: usize = 24;

/// What a code links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// The user's own account: their private chat is full Shodh.
    Owner,
    /// Someone the user invites: they may ask questions of the chosen workspace.
    Invite,
}

/// Why a code did not link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// No such code (never made, used already, or replaced by a newer one).
    Unknown,
    Expired,
}

/// A code shown on the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCode {
    pub code: String,
    pub kind: LinkKind,
    pub expires_at: DateTime<Utc>,
}

/// The codes waiting to be used.
#[derive(Debug, Default)]
pub struct LinkCodes {
    codes: Mutex<HashMap<String, (LinkKind, DateTime<Utc>)>>,
}

/// Whether `code` has a code's shape (a deep link's `start` parameter).
pub fn is_code_shaped(code: &str) -> bool {
    (1..=64).contains(&code.len())
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl LinkCodes {
    /// A new code of `kind`, valid from `now` for [`CODE_TTL`]; the previous code of that
    /// kind stops working.
    pub fn issue(&self, kind: LinkKind, now: DateTime<Utc>) -> IssuedCode {
        let code: String = rand::rngs::OsRng
            .sample_iter(&Alphanumeric)
            .take(CODE_CHARS)
            .map(char::from)
            .collect();
        let expires_at = now + CODE_TTL;
        let mut codes = self.codes.lock().unwrap_or_else(|e| e.into_inner());
        codes.retain(|_, (k, expires)| *k != kind && *expires > now);
        codes.insert(code.clone(), (kind, expires_at));
        IssuedCode {
            code,
            kind,
            expires_at,
        }
    }

    /// Uses `code` at `now`: what it links, once. Any use ends it, an expired one too.
    pub fn redeem(&self, code: &str, now: DateTime<Utc>) -> Result<LinkKind, LinkError> {
        let mut codes = self.codes.lock().unwrap_or_else(|e| e.into_inner());
        match codes.remove(code.trim()) {
            Some((kind, expires)) if now < expires => Ok(kind),
            Some(_) => Err(LinkError::Expired),
            None => Err(LinkError::Unknown),
        }
    }

    /// Ends every code (the bot was disconnected or got a new token).
    pub fn clear(&self) {
        self.codes.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// The deep link that sends `/start <code>` to the bot `@username`.
pub fn deep_link(username: &str, code: &str) -> String {
    format!("https://t.me/{username}?start={code}")
}

/// The link that opens a private chat with the bot.
pub fn chat_link(username: &str) -> String {
    format!("https://t.me/{username}")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 27, 9, minute, 0).unwrap()
    }

    #[test]
    fn a_code_links_once() {
        let codes = LinkCodes::default();
        let issued = codes.issue(LinkKind::Owner, at(0));
        assert_eq!(issued.code.len(), CODE_CHARS);
        assert!(is_code_shaped(&issued.code));
        assert_eq!(issued.expires_at, at(10));
        assert_eq!(codes.redeem(&issued.code, at(1)), Ok(LinkKind::Owner));
        assert_eq!(
            codes.redeem(&issued.code, at(1)),
            Err(LinkError::Unknown),
            "single use"
        );
    }

    #[test]
    fn a_code_expires_after_ten_minutes() {
        let codes = LinkCodes::default();
        let issued = codes.issue(LinkKind::Owner, at(0));
        assert_eq!(codes.redeem(&issued.code, at(10)), Err(LinkError::Expired));
        assert_eq!(codes.redeem(&issued.code, at(11)), Err(LinkError::Unknown));
    }

    #[test]
    fn an_invite_never_links_the_owner_and_a_new_code_replaces_the_old() {
        let codes = LinkCodes::default();
        let owner = codes.issue(LinkKind::Owner, at(0));
        let invite = codes.issue(LinkKind::Invite, at(0));
        assert_ne!(owner.code, invite.code);
        assert_eq!(codes.redeem(&invite.code, at(1)), Ok(LinkKind::Invite));
        let newer = codes.issue(LinkKind::Owner, at(2));
        assert_eq!(codes.redeem(&owner.code, at(3)), Err(LinkError::Unknown));
        assert_eq!(codes.redeem(&newer.code, at(3)), Ok(LinkKind::Owner));
        let gone = codes.issue(LinkKind::Invite, at(4));
        codes.clear();
        assert_eq!(codes.redeem(&gone.code, at(4)), Err(LinkError::Unknown));
    }

    #[test]
    fn links_open_the_bot() {
        assert_eq!(
            deep_link("shodh_notes_bot", "abc"),
            "https://t.me/shodh_notes_bot?start=abc"
        );
        assert!(!is_code_shaped("a b"));
        assert!(!is_code_shaped(&"a".repeat(65)));
    }
}
