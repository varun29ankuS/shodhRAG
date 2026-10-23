//! Socket Mode frames and which Slack events become questions.
//!
//! Frames (<https://docs.slack.dev/apis/events-api/using-socket-mode>): `hello` after
//! connecting, `disconnect` before Slack closes the connection, and envelopes
//! (`{"envelope_id", "type", "payload", "retry_attempt", …}`) that must each be
//! acknowledged with `{"envelope_id": …}`.
//!
//! Every event that is not a real question to the bot is dropped here, before anything
//! that costs a model call: retries of an event already seen, the bot's own messages and
//! every other bot's, edits, deletions, joins and every other message subtype (a message
//! with a file and a thread reply also sent to the channel are questions: their text is
//! read, the files are not), and messages that are neither a mention of the bot nor a DM
//! to it. Who may ask, and where, is decided later ([`super::access`]).

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Event ids remembered to drop Slack's retries of an event already seen.
pub const SEEN_EVENTS: usize = 1_024;

/// How long an event id is remembered. Slack retries an unacknowledged event a few
/// times within minutes (<https://docs.slack.dev/apis/events-api/using-socket-mode>);
/// an hour covers that with room, also across a restart of the connector.
pub const SEEN_TTL_MINUTES: i64 = 60;

/// Message subtypes that are still a person asking: a message with a file (its text is
/// read, the file is not) and a thread reply also sent to the channel.
pub const QUESTION_SUBTYPES: [&str; 2] = ["file_share", "thread_broadcast"];

/// Why Slack closes a connection (`disconnect` frame).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectReason {
    /// About ten seconds before a regular refresh (not always sent).
    Warning,
    /// A regular refresh: connect again at once.
    RefreshRequested,
    /// Socket Mode was turned off for the app: connecting again cannot work.
    LinkDisabled,
    Other(String),
}

impl DisconnectReason {
    fn parse(reason: &str) -> Self {
        match reason {
            "warning" => Self::Warning,
            "refresh_requested" => Self::RefreshRequested,
            "link_disabled" => Self::LinkDisabled,
            other => Self::Other(other.to_string()),
        }
    }
}

/// One envelope: a payload Slack waits to see acknowledged.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub envelope_id: String,
    /// `events_api`, `interactive`, `slash_commands`.
    pub kind: String,
    pub payload: Value,
    /// 0 for the first delivery; Slack retries an envelope it saw no ack for.
    pub retry_attempt: u32,
}

/// What Slack says in `hello`: the app the connection belongs to and how many
/// connections that app has open now, this one included
/// (<https://docs.slack.dev/apis/events-api/using-socket-mode>).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hello {
    pub app_id: Option<String>,
    pub num_connections: Option<u32>,
}

/// A text frame from Slack.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Hello(Hello),
    Disconnect(DisconnectReason),
    Envelope(Envelope),
    /// A frame type the connector does not use.
    Other(String),
}

/// Reads one text frame. A frame that is not JSON, or an envelope without an id, is an
/// error (logged and skipped by the caller).
pub fn parse_frame(text: &str) -> Result<Frame, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("not a JSON frame: {e}"))?;
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "hello" => Ok(Frame::Hello(Hello {
            app_id: value
                .pointer("/connection_info/app_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            num_connections: value
                .get("num_connections")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
        })),
        "disconnect" => Ok(Frame::Disconnect(DisconnectReason::parse(
            value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ))),
        _ => match value.get("envelope_id").and_then(Value::as_str) {
            Some(id) if !id.is_empty() => Ok(Frame::Envelope(Envelope {
                envelope_id: id.to_string(),
                kind: kind.to_string(),
                payload: value.get("payload").cloned().unwrap_or(Value::Null),
                retry_attempt: value
                    .get("retry_attempt")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0),
            })),
            _ if kind.is_empty() => Err("a frame without a type".to_string()),
            _ => Ok(Frame::Other(kind.to_string())),
        },
    }
}

/// The acknowledgement of envelope `envelope_id`.
pub fn ack(envelope_id: &str) -> String {
    json!({ "envelope_id": envelope_id }).to_string()
}

/// The bot as Slack knows it (from `auth.test`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotIdentity {
    /// The bot's user id (`U…`), as mentioned in `<@U…>`.
    pub user_id: String,
    /// The bot id (`B…`), set on the bot's own messages.
    pub bot_id: Option<String>,
}

/// One answer at a time per thread: a channel and the thread's first message (`ts`), or a
/// DM conversation outside threads.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadKey {
    pub channel: String,
    pub thread: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionKind {
    /// `@shodh …` in a channel.
    Mention,
    /// A direct message to the bot.
    Direct,
}

/// A question to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The event id (dedupe key).
    pub event_id: String,
    pub kind: QuestionKind,
    pub channel: String,
    /// The thread the reply goes in (`None`: the DM itself).
    pub reply_thread_ts: Option<String>,
    pub thread: ThreadKey,
    /// The asker's user id (logged as an id only, never sent to the model).
    pub user: String,
    /// The Slack workspace the asker belongs to, when the event says (`user_team`, else
    /// `team`). Slack does not document it on every event, so [`super::access`] uses it
    /// only to refuse early, never as proof of membership.
    pub asker_team: Option<String>,
    /// Slack marks the event as from a channel shared with another organization
    /// (`is_ext_shared_channel` on the event wrapper,
    /// <https://docs.slack.dev/apis/events-api/>).
    pub ext_shared: bool,
    /// The message carried files (`file_share`); only its text is read.
    pub has_files: bool,
    /// The question with the bot's mention removed. Untrusted text.
    pub text: String,
}

/// What to do with an envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Not a question to the bot; nothing is sent back.
    Ignore(&'static str),
    Ask(Question),
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Removes every mention of the bot (`<@U123>`, `<@U123|shodh>`) and trims.
pub fn strip_bot_mention(text: &str, bot_user_id: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let opener = format!("<@{bot_user_id}");
    while let Some(at) = rest.find(&opener) {
        let after = &rest[at + opener.len()..];
        match after.chars().next() {
            Some('>') | Some('|') => match after.find('>') {
                Some(end) => {
                    out.push_str(&rest[..at]);
                    rest = &after[end + 1..];
                }
                None => break,
            },
            // `<@U1234>` must not swallow `<@U12345>`.
            _ => {
                out.push_str(&rest[..at + opener.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `envelope` is a question to the bot, and which.
pub fn decide(envelope: &Envelope, bot: &BotIdentity) -> Decision {
    if envelope.kind != "events_api" {
        return Decision::Ignore("not an Events API envelope");
    }
    let payload = &envelope.payload;
    if str_field(payload, "type") != Some("event_callback") {
        return Decision::Ignore("not an event callback");
    }
    let Some(event) = payload.get("event") else {
        return Decision::Ignore("no event");
    };
    if event.get("bot_id").is_some_and(|v| !v.is_null()) || event.get("bot_profile").is_some() {
        return Decision::Ignore("a bot's message");
    }
    let subtype = event.get("subtype").filter(|v| !v.is_null());
    if let Some(subtype) = subtype {
        if !subtype
            .as_str()
            .is_some_and(|s| QUESTION_SUBTYPES.contains(&s))
        {
            return Decision::Ignore("a message subtype (edit, deletion, join, …)");
        }
    }
    if event.get("hidden").and_then(Value::as_bool) == Some(true) {
        return Decision::Ignore("a hidden message");
    }
    let Some(user) = str_field(event, "user") else {
        return Decision::Ignore("no sender");
    };
    if user == bot.user_id {
        return Decision::Ignore("the bot's own message");
    }
    let (Some(channel), Some(ts)) = (str_field(event, "channel"), str_field(event, "ts")) else {
        return Decision::Ignore("no channel or timestamp");
    };
    let thread_ts = str_field(event, "thread_ts");
    let kind = match (str_field(event, "type"), str_field(event, "channel_type")) {
        (Some("app_mention"), _) => QuestionKind::Mention,
        (Some("message"), Some("im")) => QuestionKind::Direct,
        (Some("message"), _) => return Decision::Ignore("a channel message without a mention"),
        _ => return Decision::Ignore("an event the connector does not answer"),
    };
    let (reply_thread_ts, thread) = match kind {
        // In a channel the answer always goes in the question's thread.
        QuestionKind::Mention => {
            let root = thread_ts.unwrap_or(ts).to_string();
            (Some(root.clone()), root)
        }
        // In a DM: in the thread when the question was asked in one, else in the DM.
        QuestionKind::Direct => match thread_ts {
            Some(root) => (Some(root.to_string()), root.to_string()),
            None => (None, String::new()),
        },
    };
    let event_id = str_field(payload, "event_id")
        .unwrap_or(&envelope.envelope_id)
        .to_string();
    Decision::Ask(Question {
        event_id,
        kind,
        channel: channel.to_string(),
        reply_thread_ts,
        thread: ThreadKey {
            channel: channel.to_string(),
            thread,
        },
        user: user.to_string(),
        asker_team: str_field(event, "user_team")
            .or_else(|| str_field(event, "team"))
            .map(str::to_string),
        ext_shared: payload
            .get("is_ext_shared_channel")
            .and_then(Value::as_bool)
            == Some(true),
        has_files: subtype.and_then(Value::as_str) == Some("file_share")
            || event
                .get("files")
                .and_then(Value::as_array)
                .is_some_and(|f| !f.is_empty()),
        text: strip_bot_mention(str_field(event, "text").unwrap_or_default(), &bot.user_id),
    })
}

/// One remembered event id, as kept on disk (an id and a time, no message content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenEntry {
    pub id: String,
    pub at: DateTime<Utc>,
}

/// Event ids already handled: the most recent [`SEEN_EVENTS`], each for
/// [`SEEN_TTL_MINUTES`].
pub struct SeenEvents {
    ids: LruCache<String, DateTime<Utc>>,
    ttl: chrono::Duration,
}

impl SeenEvents {
    pub fn new(capacity: usize) -> Self {
        let capacity = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN);
        Self {
            ids: LruCache::new(capacity),
            ttl: chrono::Duration::minutes(SEEN_TTL_MINUTES),
        }
    }

    fn fresh(&self, at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        at <= now && now.signed_duration_since(at) < self.ttl
    }

    /// True the first time `event_id` is seen within the TTL (and remembers it at `now`).
    pub fn first_time(&mut self, event_id: &str, now: DateTime<Utc>) -> bool {
        // `peek` keeps the order; an id seen again is re-inserted as the newest.
        if let Some(at) = self.ids.peek(event_id).copied() {
            if self.fresh(at, now) {
                return false;
            }
        }
        self.ids.put(event_id.to_string(), now);
        true
    }

    /// The ids still within the TTL at `now`, oldest first (what is kept on disk).
    pub fn snapshot(&self, now: DateTime<Utc>) -> Vec<SeenEntry> {
        let mut entries: Vec<SeenEntry> = self
            .ids
            .iter()
            .filter(|(_, at)| self.fresh(**at, now))
            .map(|(id, at)| SeenEntry {
                id: id.clone(),
                at: *at,
            })
            .collect();
        entries.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
        entries
    }

    /// Remembers the `entries` kept on disk that are still within the TTL at `now` (the
    /// newest when there are more than the capacity).
    pub fn restore(&mut self, mut entries: Vec<SeenEntry>, now: DateTime<Utc>) {
        entries.sort_by_key(|e| e.at);
        for entry in entries {
            if self.fresh(entry.at, now) {
                self.ids.put(entry.id, entry.at);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

impl Default for SeenEvents {
    fn default() -> Self {
        Self::new(SEEN_EVENTS)
    }
}

/// Where the seen event ids are kept between runs of the connector (the app keeps a
/// small file). Called on a blocking thread, never on an async worker.
pub trait SeenStore: Send + Sync {
    fn load(&self) -> Result<Vec<SeenEntry>, String>;
    fn save(&self, entries: &[SeenEntry]) -> Result<(), String>;
}

/// Threads with an answer in flight.
#[derive(Clone, Default)]
pub struct InFlight(Arc<Mutex<HashSet<ThreadKey>>>);

impl InFlight {
    /// Claims `thread`, or `None` while another answer in it is in flight. The claim ends
    /// when the guard is dropped (also when the answering task fails or panics).
    pub fn try_begin(&self, thread: &ThreadKey) -> Option<InFlightGuard> {
        let mut threads = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !threads.insert(thread.clone()) {
            return None;
        }
        Some(InFlightGuard {
            threads: self.0.clone(),
            thread: thread.clone(),
        })
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An answer in flight in one thread.
pub struct InFlightGuard {
    threads: Arc<Mutex<HashSet<ThreadKey>>>,
    thread: ThreadKey,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.threads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.thread);
    }
}

/// What the router made of an envelope.
pub enum Routed {
    Ignore(&'static str),
    /// A question in a thread that already has an answer in flight.
    Busy(Question),
    Ask(Question, InFlightGuard),
}

impl std::fmt::Debug for Routed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Routed::Ignore(why) => f.debug_tuple("Ignore").field(why).finish(),
            Routed::Busy(q) => f.debug_tuple("Busy").field(&q.event_id).finish(),
            Routed::Ask(q, _) => f.debug_tuple("Ask").field(&q.event_id).finish(),
        }
    }
}

/// Filters, dedupes and claims threads for one installation's events, across its
/// connections and restarts of the connector: the app keeps one router per Slack
/// workspace, and the seen ids are also kept on disk ([`SeenStore`]).
pub struct EventRouter {
    bot: BotIdentity,
    seen: Mutex<SeenEvents>,
    /// Bumped with every new id, so the newest snapshot written wins.
    seen_generation: AtomicU64,
    in_flight: InFlight,
}

impl EventRouter {
    pub fn new(bot: BotIdentity) -> Self {
        Self {
            bot,
            seen: Mutex::new(SeenEvents::default()),
            seen_generation: AtomicU64::new(0),
            in_flight: InFlight::default(),
        }
    }

    /// A router that starts with the ids kept on disk (those still within the TTL).
    pub fn restored(bot: BotIdentity, entries: Vec<SeenEntry>, now: DateTime<Utc>) -> Self {
        let router = Self::new(bot);
        router
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .restore(entries, now);
        router
    }

    pub fn bot(&self) -> &BotIdentity {
        &self.bot
    }

    pub fn in_flight(&self) -> &InFlight {
        &self.in_flight
    }

    /// The seen ids to keep on disk now, and their generation: a snapshot replaces an
    /// older one, never a newer one.
    pub fn seen_snapshot(&self, now: DateTime<Utc>) -> (u64, Vec<SeenEntry>) {
        let seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        (
            self.seen_generation.load(Ordering::SeqCst),
            seen.snapshot(now),
        )
    }

    pub fn route(&self, envelope: &Envelope) -> Routed {
        self.route_at(envelope, Utc::now())
    }

    pub fn route_at(&self, envelope: &Envelope, now: DateTime<Utc>) -> Routed {
        let question = match decide(envelope, &self.bot) {
            Decision::Ignore(why) => return Routed::Ignore(why),
            Decision::Ask(question) => question,
        };
        let first = {
            let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            let first = seen.first_time(&question.event_id, now);
            if first {
                self.seen_generation.fetch_add(1, Ordering::SeqCst);
            }
            first
        };
        if !first {
            return Routed::Ignore("a retry of an event already handled");
        }
        match self.in_flight.try_begin(&question.thread) {
            Some(guard) => Routed::Ask(question, guard),
            None => Routed::Busy(question),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot() -> BotIdentity {
        BotIdentity {
            user_id: "UBOT".into(),
            bot_id: Some("BBOT".into()),
        }
    }

    fn envelope(id: &str, event_id: &str, event: Value) -> Envelope {
        Envelope {
            envelope_id: id.into(),
            kind: "events_api".into(),
            payload: json!({
                "type": "event_callback",
                "team_id": "T1",
                "event_id": event_id,
                "event": event,
            }),
            retry_attempt: 0,
        }
    }

    fn mention(text: &str) -> Value {
        json!({"type": "app_mention", "user": "U1", "text": text, "ts": "100.1", "channel": "C1"})
    }

    fn dm(text: &str) -> Value {
        json!({"type": "message", "channel_type": "im", "user": "U1", "text": text, "ts": "200.1", "channel": "D1"})
    }

    #[test]
    fn frames_are_read() {
        assert_eq!(
            parse_frame(
                r#"{"type":"hello","num_connections":2,"connection_info":{"app_id":"A1"}}"#
            )
            .unwrap(),
            Frame::Hello(Hello {
                app_id: Some("A1".into()),
                num_connections: Some(2)
            })
        );
        assert_eq!(
            parse_frame(r#"{"type":"hello"}"#).unwrap(),
            Frame::Hello(Hello::default())
        );
        assert_eq!(
            parse_frame(r#"{"type":"disconnect","reason":"refresh_requested","debug_info":{}}"#)
                .unwrap(),
            Frame::Disconnect(DisconnectReason::RefreshRequested)
        );
        assert_eq!(
            parse_frame(r#"{"type":"disconnect","reason":"link_disabled"}"#).unwrap(),
            Frame::Disconnect(DisconnectReason::LinkDisabled)
        );
        assert_eq!(
            parse_frame(r#"{"type":"disconnect","reason":"warning"}"#).unwrap(),
            Frame::Disconnect(DisconnectReason::Warning)
        );
        let Frame::Envelope(e) = parse_frame(
            r#"{"envelope_id":"E1","type":"events_api","accepts_response_payload":false,"retry_attempt":2,"retry_reason":"timeout","payload":{"type":"event_callback"}}"#,
        )
        .unwrap() else {
            panic!("an envelope")
        };
        assert_eq!(e.envelope_id, "E1");
        assert_eq!(e.retry_attempt, 2);
        assert_eq!(e.payload["type"], "event_callback");
        assert_eq!(
            parse_frame(r#"{"type":"slash_commands"}"#).unwrap(),
            Frame::Other("slash_commands".into())
        );
        assert!(parse_frame("not json").is_err());
        assert!(parse_frame("{}").is_err());
    }

    #[test]
    fn the_ack_names_the_envelope() {
        let ack: Value = serde_json::from_str(&ack("abc-123")).unwrap();
        assert_eq!(ack, json!({"envelope_id": "abc-123"}));
    }

    #[test]
    fn mentions_and_dms_are_questions() {
        let Decision::Ask(q) = decide(
            &envelope("E1", "Ev1", mention("<@UBOT> what is the notice period?")),
            &bot(),
        ) else {
            panic!("a question")
        };
        assert_eq!(q.kind, QuestionKind::Mention);
        assert_eq!(q.text, "what is the notice period?");
        assert_eq!(q.reply_thread_ts.as_deref(), Some("100.1"));
        assert_eq!(
            q.thread,
            ThreadKey {
                channel: "C1".into(),
                thread: "100.1".into()
            }
        );
        assert_eq!(q.event_id, "Ev1");

        let mut in_thread = mention("<@UBOT|shodh> and the rent?");
        in_thread["thread_ts"] = json!("90.5");
        let Decision::Ask(q) = decide(&envelope("E2", "Ev2", in_thread), &bot()) else {
            panic!("a question")
        };
        assert_eq!(q.text, "and the rent?");
        assert_eq!(q.reply_thread_ts.as_deref(), Some("90.5"));

        let Decision::Ask(q) = decide(&envelope("E3", "Ev3", dm("hello there")), &bot()) else {
            panic!("a question")
        };
        assert_eq!(q.kind, QuestionKind::Direct);
        assert_eq!(q.reply_thread_ts, None);
        assert_eq!(
            q.thread,
            ThreadKey {
                channel: "D1".into(),
                thread: String::new()
            }
        );
    }

    #[test]
    fn bots_the_bot_itself_subtypes_and_other_messages_are_ignored() {
        let cases: Vec<(&str, Value)> = vec![
            (
                "bot_id",
                json!({"type": "message", "channel_type": "im", "bot_id": "B9", "user": "U9", "text": "hi", "ts": "1.1", "channel": "D1"}),
            ),
            (
                "own bot",
                json!({"type": "message", "channel_type": "im", "bot_id": "BBOT", "text": "Searching…", "ts": "1.2", "channel": "D1"}),
            ),
            (
                "own user",
                json!({"type": "message", "channel_type": "im", "user": "UBOT", "text": "hi", "ts": "1.3", "channel": "D1"}),
            ),
            (
                "bot_profile",
                json!({"type": "app_mention", "user": "U9", "bot_profile": {"name": "x"}, "text": "<@UBOT> hi", "ts": "1.4", "channel": "C1"}),
            ),
            (
                "edit",
                json!({"type": "message", "channel_type": "im", "subtype": "message_changed", "message": {"user": "U1", "text": "x"}, "ts": "1.5", "channel": "D1"}),
            ),
            (
                "file comment",
                json!({"type": "message", "channel_type": "im", "subtype": "file_comment", "user": "U1", "text": "x", "ts": "1.55", "channel": "D1"}),
            ),
            (
                "channel broadcast without a mention",
                json!({"type": "message", "channel_type": "channel", "subtype": "thread_broadcast", "user": "U1", "text": "x", "ts": "1.56", "thread_ts": "1.0", "channel": "C1"}),
            ),
            (
                "delete",
                json!({"type": "message", "channel_type": "im", "subtype": "message_deleted", "ts": "1.6", "channel": "D1"}),
            ),
            (
                "join",
                json!({"type": "message", "channel_type": "channel", "subtype": "channel_join", "user": "U1", "ts": "1.7", "channel": "C1"}),
            ),
            (
                "bot_message",
                json!({"type": "message", "channel_type": "im", "subtype": "bot_message", "text": "x", "ts": "1.8", "channel": "D1"}),
            ),
            (
                "channel message",
                json!({"type": "message", "channel_type": "channel", "user": "U1", "text": "hi", "ts": "1.9", "channel": "C1"}),
            ),
            (
                "hidden",
                json!({"type": "message", "channel_type": "im", "hidden": true, "user": "U1", "ts": "2.0", "channel": "D1"}),
            ),
            (
                "other event",
                json!({"type": "reaction_added", "user": "U1", "ts": "2.1", "channel": "C1"}),
            ),
            (
                "no user",
                json!({"type": "app_mention", "text": "<@UBOT> hi", "ts": "2.2", "channel": "C1"}),
            ),
        ];
        for (name, event) in cases {
            assert!(
                matches!(
                    decide(&envelope("E", "Ev", event), &bot()),
                    Decision::Ignore(_)
                ),
                "{name} must be ignored"
            );
        }
        let mut interactive = envelope("E", "Ev", mention("<@UBOT> hi"));
        interactive.kind = "interactive".into();
        assert!(matches!(decide(&interactive, &bot()), Decision::Ignore(_)));
        let mut not_callback = envelope("E", "Ev", mention("<@UBOT> hi"));
        not_callback.payload["type"] = json!("url_verification");
        assert!(matches!(decide(&not_callback, &bot()), Decision::Ignore(_)));
    }

    #[test]
    fn a_message_with_a_file_and_a_thread_broadcast_are_questions() {
        let mut with_file = dm("what does our policy say about this?");
        with_file["subtype"] = json!("file_share");
        with_file["files"] = json!([{"id": "F1", "name": "screenshot.png"}]);
        let Decision::Ask(q) = decide(&envelope("E1", "Ev1", with_file), &bot()) else {
            panic!("a question")
        };
        assert_eq!(q.text, "what does our policy say about this?");
        assert!(q.has_files);

        let mut only_file = dm("");
        only_file["subtype"] = json!("file_share");
        let Decision::Ask(q) = decide(&envelope("E2", "Ev2", only_file), &bot()) else {
            panic!("a question, answered with a hint")
        };
        assert!(q.text.is_empty() && q.has_files);

        let mut broadcast = dm("<@UBOT> and the rent?");
        broadcast["subtype"] = json!("thread_broadcast");
        broadcast["thread_ts"] = json!("190.0");
        let Decision::Ask(q) = decide(&envelope("E3", "Ev3", broadcast), &bot()) else {
            panic!("a question")
        };
        assert_eq!(q.reply_thread_ts.as_deref(), Some("190.0"));
        assert!(!q.has_files);
    }

    #[test]
    fn the_askers_team_and_a_shared_channel_are_read() {
        let mut external = mention("<@UBOT> hi");
        external["user_team"] = json!("TCLIENT");
        external["team"] = json!("THOME");
        let mut env = envelope("E1", "Ev1", external);
        env.payload["is_ext_shared_channel"] = json!(true);
        let Decision::Ask(q) = decide(&env, &bot()) else {
            panic!("a question")
        };
        assert_eq!(q.asker_team.as_deref(), Some("TCLIENT"), "user_team first");
        assert!(q.ext_shared);

        let mut plain = dm("hi");
        plain["team"] = json!("THOME");
        let Decision::Ask(q) = decide(&envelope("E2", "Ev2", plain), &bot()) else {
            panic!("a question")
        };
        assert_eq!(q.asker_team.as_deref(), Some("THOME"));
        assert!(!q.ext_shared);
        let Decision::Ask(q) = decide(&envelope("E3", "Ev3", dm("hi")), &bot()) else {
            panic!("a question")
        };
        assert_eq!(q.asker_team, None);
    }

    #[test]
    fn only_the_bots_own_mention_is_removed() {
        assert_eq!(
            strip_bot_mention("<@UBOT> hi <@UBOT2> and <@UOTHER>", "UBOT"),
            "hi <@UBOT2> and <@UOTHER>"
        );
        assert_eq!(strip_bot_mention("<@UBOT|shodh>   ", "UBOT"), "");
        assert_eq!(
            strip_bot_mention("tell <@UBOT> about\nleases", "UBOT"),
            "tell about leases"
        );
        assert_eq!(strip_bot_mention("<@UBOT", "UBOT"), "<@UBOT");
    }

    #[test]
    fn retries_and_repeats_of_an_event_are_dropped() {
        let router = EventRouter::new(bot());
        let first = envelope("E1", "Ev1", mention("<@UBOT> q"));
        let Routed::Ask(_, guard) = router.route(&first) else {
            panic!("asked")
        };
        drop(guard);
        let mut retry = envelope("E2", "Ev1", mention("<@UBOT> q"));
        retry.retry_attempt = 1;
        assert!(matches!(router.route(&retry), Routed::Ignore(_)));
        assert!(matches!(router.route(&first), Routed::Ignore(_)));
        // Without an event id the envelope id is the key.
        let mut no_id = envelope("E9", "", mention("<@UBOT> q2"));
        no_id.payload.as_object_mut().unwrap().remove("event_id");
        assert!(matches!(router.route(&no_id), Routed::Ask(..)));
        assert!(matches!(router.route(&no_id), Routed::Ignore(_)));
    }

    fn at(minute: i64) -> DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(2026, 10, 23, 9, 0, 0).unwrap() + chrono::Duration::minutes(minute)
    }

    #[test]
    fn the_seen_list_is_bounded() {
        let mut seen = SeenEvents::new(3);
        for id in ["a", "b", "c", "d"] {
            assert!(seen.first_time(id, at(0)));
        }
        assert!(!seen.first_time("d", at(1)));
        assert!(seen.first_time("a", at(1)), "the oldest id was forgotten");
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn seen_ids_expire_after_the_ttl() {
        let mut seen = SeenEvents::new(10);
        assert!(seen.first_time("a", at(0)));
        assert!(!seen.first_time("a", at(SEEN_TTL_MINUTES - 1)));
        assert!(
            seen.first_time("a", at(SEEN_TTL_MINUTES)),
            "an id older than the TTL is new again"
        );
        assert!(seen.first_time("b", at(SEEN_TTL_MINUTES)));
        assert_eq!(
            seen.snapshot(at(2 * SEEN_TTL_MINUTES - 1)),
            vec![
                SeenEntry {
                    id: "a".into(),
                    at: at(SEEN_TTL_MINUTES)
                },
                SeenEntry {
                    id: "b".into(),
                    at: at(SEEN_TTL_MINUTES)
                }
            ]
        );
        assert!(
            seen.snapshot(at(2 * SEEN_TTL_MINUTES)).is_empty(),
            "expired ids are not kept"
        );
    }

    #[test]
    fn seen_ids_survive_a_restart_of_the_router() {
        let first = EventRouter::new(bot());
        let Routed::Ask(_, guard) =
            first.route_at(&envelope("E1", "Ev1", mention("<@UBOT> q")), at(0))
        else {
            panic!("asked")
        };
        drop(guard);
        let (generation, kept) = first.seen_snapshot(at(1));
        assert_eq!(generation, 1);
        assert_eq!(kept.len(), 1);
        // A retry delivered after the connector restarted costs nothing.
        let second = EventRouter::restored(bot(), kept.clone(), at(2));
        let mut retry = envelope("E2", "Ev1", mention("<@UBOT> q"));
        retry.retry_attempt = 1;
        assert!(matches!(second.route_at(&retry, at(2)), Routed::Ignore(_)));
        // Entries past the TTL, or from the future, are not restored.
        let stale = EventRouter::restored(bot(), kept, at(SEEN_TTL_MINUTES + 1));
        assert!(matches!(
            stale.route_at(&retry, at(SEEN_TTL_MINUTES + 1)),
            Routed::Ask(..)
        ));
        let future = EventRouter::restored(
            bot(),
            vec![SeenEntry {
                id: "Ev9".into(),
                at: at(30),
            }],
            at(0),
        );
        assert_eq!(future.seen_snapshot(at(0)).1.len(), 0);
    }

    #[test]
    fn one_answer_in_flight_per_thread() {
        let router = EventRouter::new(bot());
        let Routed::Ask(_, guard) = router.route(&envelope("E1", "Ev1", mention("<@UBOT> q1")))
        else {
            panic!("asked")
        };
        // Same thread (the mention's ts is the thread root): busy.
        let mut same_thread = mention("<@UBOT> q2");
        same_thread["ts"] = json!("100.9");
        same_thread["thread_ts"] = json!("100.1");
        assert!(matches!(
            router.route(&envelope("E2", "Ev2", same_thread)),
            Routed::Busy(_)
        ));
        // Another thread and a DM are answered at the same time.
        let mut other = mention("<@UBOT> q3");
        other["ts"] = json!("300.1");
        let Routed::Ask(_, other_guard) = router.route(&envelope("E3", "Ev3", other)) else {
            panic!("asked")
        };
        let Routed::Ask(_, dm_guard) = router.route(&envelope("E4", "Ev4", dm("q4"))) else {
            panic!("asked")
        };
        assert_eq!(router.in_flight().len(), 3);
        drop(guard);
        let mut again = mention("<@UBOT> q5");
        again["ts"] = json!("101.0");
        again["thread_ts"] = json!("100.1");
        assert!(
            matches!(router.route(&envelope("E5", "Ev5", again)), Routed::Ask(..)),
            "free once the answer finished"
        );
        drop(other_guard);
        drop(dm_guard);
    }
}
