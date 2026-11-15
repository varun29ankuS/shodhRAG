//! What the phone is told outside the app, and how little: a notification
//! carries only its kind ("Shodh needs your approval", "Your answer is
//! ready"), never a question, an answer, a title or a name. The details come
//! over the link when the app opens.
//!
//! The same kinds are used when the phone's own link sees an event ("Stay
//! connected", [`kind_of`]) and when the computer posts to the phone's
//! UnifiedPush endpoint ([`payload`]): the push body is the kind's word and
//! nothing else, so a push server never holds any of the user's content.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bridge::{AGENT_EVENT, CALENDAR_CHANGED};

/// What a notification says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// A step waits for the user's approval.
    Approval,
    /// An answer the user asked for is ready.
    Answer,
    /// A reminder of the user's tasks.
    Reminder,
    /// A brief before a meeting is ready.
    Brief,
}

impl Kind {
    /// The kind's word: the whole push body.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Approval => "approval",
            Kind::Answer => "answer",
            Kind::Reminder => "reminder",
            Kind::Brief => "brief",
        }
    }

    /// The kind of a push body; anything else is not a Shodh push.
    pub fn from_word(word: &str) -> Option<Kind> {
        match word.trim() {
            "approval" => Some(Kind::Approval),
            "answer" => Some(Kind::Answer),
            "reminder" => Some(Kind::Reminder),
            "brief" => Some(Kind::Brief),
            _ => None,
        }
    }

    /// What the phone shows: fixed words, no content.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Approval => "Shodh needs your approval",
            Kind::Answer => "Your answer is ready",
            Kind::Reminder => "You have a reminder in Shodh",
            Kind::Brief => "Your meeting brief is ready",
        }
    }
}

/// How the phone tells the user when the app is closed (Settings, Notifications).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Only while the app is open (the default).
    #[default]
    Off,
    /// "Stay connected": a foreground service keeps the link open.
    Stay,
    /// UnifiedPush, through the user's distributor app.
    Push,
}

impl Mode {
    /// Whether the foreground service runs: only for "Stay connected".
    pub fn runs_service(self) -> bool {
        self == Mode::Stay
    }

    /// Whether the phone's own link posts notifications from the events it sees.
    pub fn notifies_from_link(self) -> bool {
        self == Mode::Stay
    }

    /// Whether the computer keeps a push address for this phone.
    pub fn registers_push(self) -> bool {
        self == Mode::Push
    }
}

/// The push body of `kind`: its word, as bytes.
pub fn payload(kind: Kind) -> Vec<u8> {
    kind.word().as_bytes().to_vec()
}

/// The notification a desktop event calls for, if any: a step waiting for
/// approval, or an answer that finished. Everything else is quiet.
pub fn kind_of(name: &str, payload: &Value) -> Option<Kind> {
    if name == CALENDAR_CHANGED {
        return None;
    }
    if name != AGENT_EVENT {
        return None;
    }
    let event = payload.get("event")?;
    match event.get("type")?.as_str()? {
        "approval_requested" => Some(Kind::Approval),
        "run_finished" if event.get("status").and_then(Value::as_str) == Some("completed") => {
            Some(Kind::Answer)
        }
        _ => None,
    }
}

/// A UnifiedPush endpoint, which is a secret (anyone holding it can wake the
/// phone): never printed. Only `https://` endpoints are kept.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Endpoint(String);

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Endpoint([REDACTED])")
    }
}

/// The longest endpoint kept.
pub const MAX_ENDPOINT: usize = 2048;

impl Endpoint {
    /// An endpoint the phone registered; refused unless it is a plain `https://` URL.
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        let raw = raw.trim();
        let rest = raw
            .strip_prefix("https://")
            .ok_or("A push address must start with https://.")?;
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if raw.len() > MAX_ENDPOINT
            || host.is_empty()
            || host.contains('@')
            || raw.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("This push address is not valid.");
        }
        Ok(Self(raw.to_string()))
    }

    /// The URL, to post to. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_push_payload_has_no_content() {
        // An approval of a step full of the user's content: the push says only "approval".
        let approval = json!({
            "sessionId": "s1",
            "event": { "type": "approval_requested", "stepId": "x", "tool": "create_note", "label": "Save a note about Asha's diagnosis" }
        });
        let kind = kind_of(AGENT_EVENT, &approval).unwrap();
        assert_eq!(payload(kind), b"approval");
        let finished = json!({ "sessionId": "s1", "event": { "type": "run_finished", "status": "completed", "error": null } });
        assert_eq!(payload(kind_of(AGENT_EVENT, &finished).unwrap()), b"answer");
        // Every body is one of four fixed words, and reads back as its kind.
        for kind in [Kind::Approval, Kind::Answer, Kind::Reminder, Kind::Brief] {
            let body = String::from_utf8(payload(kind)).unwrap();
            assert!(body.chars().all(|c| c.is_ascii_lowercase()) && body.len() <= 8);
            assert_eq!(Kind::from_word(&body), Some(kind));
            assert!(!kind.title().contains('{'));
        }
        // Text deltas, other events, and stopped answers stay quiet.
        let delta =
            json!({ "sessionId": "s1", "event": { "type": "text_delta", "delta": "secret" } });
        assert_eq!(kind_of(AGENT_EVENT, &delta), None);
        let stopped =
            json!({ "sessionId": "s1", "event": { "type": "run_finished", "status": "aborted" } });
        assert_eq!(kind_of(AGENT_EVENT, &stopped), None);
        assert_eq!(kind_of(CALENDAR_CHANGED, &json!(null)), None);
        assert_eq!(Kind::from_word("Your answer: 42"), None);
    }

    #[test]
    fn the_foreground_service_runs_only_when_stay_connected_is_chosen() {
        assert_eq!(Mode::default(), Mode::Off, "off by default");
        assert!(Mode::Stay.runs_service() && Mode::Stay.notifies_from_link());
        for mode in [Mode::Off, Mode::Push] {
            assert!(
                !mode.runs_service() && !mode.notifies_from_link(),
                "{mode:?}"
            );
        }
        assert!(
            Mode::Push.registers_push()
                && !Mode::Stay.registers_push()
                && !Mode::Off.registers_push()
        );
        assert_eq!(
            serde_json::from_str::<Mode>("\"stay\"").unwrap(),
            Mode::Stay
        );
    }

    #[test]
    fn an_endpoint_is_https_and_never_printed() {
        let token = format!("{}{}", "up", "a1b2c3d4e5f6");
        let url = format!("https://ntfy.sh/{token}?up=1");
        let endpoint = Endpoint::parse(&url).unwrap();
        assert_eq!(endpoint.expose(), url);
        assert!(!format!("{endpoint:?}").contains(&token));
        for bad in [
            "http://ntfy.sh/x",
            "https://",
            "https://user@host/x",
            "https://host/a b",
            "ftp://x",
            "",
        ] {
            assert!(Endpoint::parse(bad).is_err(), "{bad}");
        }
        assert!(Endpoint::parse(&format!("https://h/{}", "a".repeat(MAX_ENDPOINT))).is_err());
    }
}
