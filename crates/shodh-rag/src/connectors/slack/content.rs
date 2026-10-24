//! Slack messages as Shodh indexes them: which are kept, how they read, and the document
//! one thread becomes.
//!
//! Kept: people's messages, messages with files (the file names and titles only; files are
//! never downloaded), thread replies also sent to the channel and `/me` messages. Left
//! out: every bot's messages and the Shodh bot's own, joins, leaves, topic, purpose and
//! name changes, and every other subtype. Mentions read as `@Display Name` (from
//! `users.info`; never an e-mail address), channel links as `#name`, links as their label.

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

use chrono::{DateTime, TimeZone, Utc};
use regex::{Captures, Regex};
use serde_json::Value;

use super::ids;

/// Message subtypes that are a person writing.
pub const INDEXED_SUBTYPES: [&str; 3] = ["file_share", "thread_broadcast", "me_message"];

/// Characters of a thread's first message in its title.
pub const TITLE_CHARS: usize = 60;
/// Messages of one thread indexed at most (Slack threads are rarely longer).
pub const MAX_THREAD_MESSAGES: usize = 1_000;

/// One message from `conversations.history` or `conversations.replies`, or an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackMessage {
    pub ts: String,
    /// The thread's first message (`None`: not in a thread).
    pub thread_ts: Option<String>,
    pub user: Option<String>,
    /// Sent by a bot or an app (`bot_id`, `bot_profile`, `bot_message`).
    pub bot: bool,
    pub subtype: Option<String>,
    pub text: String,
    /// Names or titles of the files attached.
    pub files: Vec<String>,
    pub reply_count: usize,
    pub latest_reply: Option<String>,
    pub hidden: bool,
}

fn text_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

impl SlackMessage {
    /// Reads one message object; `None` without a `ts`.
    pub fn from_value(value: &Value) -> Option<Self> {
        let ts = text_of(value, "ts")?.to_string();
        let subtype = text_of(value, "subtype").map(str::to_string);
        let files = value
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|f| text_of(f, "title").or_else(|| text_of(f, "name")))
            .map(str::to_string)
            .collect();
        Some(Self {
            thread_ts: text_of(value, "thread_ts").map(str::to_string),
            user: text_of(value, "user").map(str::to_string),
            bot: value.get("bot_id").is_some_and(|v| !v.is_null())
                || value.get("bot_profile").is_some_and(|v| !v.is_null())
                || subtype.as_deref() == Some("bot_message"),
            text: value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            files,
            reply_count: value
                .get("reply_count")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(0),
            latest_reply: text_of(value, "latest_reply").map(str::to_string),
            hidden: value.get("hidden").and_then(Value::as_bool) == Some(true),
            subtype,
            ts,
        })
    }

    /// Not a reply: a thread's first message or a message outside threads.
    pub fn top_level(&self) -> bool {
        self.thread_ts.as_deref().is_none_or(|t| t == self.ts)
    }

    /// The thread this message belongs to (its own `ts` when it starts one or is alone).
    pub fn thread(&self) -> &str {
        self.thread_ts.as_deref().unwrap_or(&self.ts)
    }

    /// Whether this message is a person's words that Shodh indexes.
    pub fn indexed(&self, bot_user: &str) -> bool {
        if self.bot || self.hidden {
            return false;
        }
        if self
            .subtype
            .as_deref()
            .is_some_and(|s| !INDEXED_SUBTYPES.contains(&s))
        {
            return false;
        }
        match self.user.as_deref() {
            Some(user) if user != bot_user => {}
            _ => return false,
        }
        !self.text.trim().is_empty() || !self.files.is_empty()
    }
}

/// A page of messages and the cursor of the next one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryPage {
    pub messages: Vec<SlackMessage>,
    pub next_cursor: Option<String>,
}

impl HistoryPage {
    /// Reads `conversations.history` / `conversations.replies`.
    pub fn from_value(value: &Value) -> Self {
        let messages = value
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(SlackMessage::from_value)
            .collect();
        let has_more = value.get("has_more").and_then(Value::as_bool) == Some(true);
        let next_cursor = value
            .pointer("/response_metadata/next_cursor")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .filter(|_| has_more);
        Self {
            messages,
            next_cursor,
        }
    }
}

fn mention_pattern() -> Option<&'static Regex> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN
        .get_or_init(|| Regex::new(r"<@([UW][A-Z0-9]+)(?:\|[^>]*)?>").ok())
        .as_ref()
}

/// The people `text` mentions (`<@U…>`), each once.
pub fn mentioned_users(text: &str) -> BTreeSet<String> {
    let Some(re) = mention_pattern() else {
        return BTreeSet::new();
    };
    re.captures_iter(text).map(|c| c[1].to_string()).collect()
}

/// `text` as people read it: mentions as `@name` (`names`, else "@someone"), channel
/// links as `#name`, `<!here>` as `@here`, links as their label (or the address), and
/// Slack's `&amp;`, `&lt;`, `&gt;` unescaped.
pub fn readable_text(text: &str, names: &HashMap<String, String>) -> String {
    static LINK: OnceLock<Option<Regex>> = OnceLock::new();
    let mut out = text.to_string();
    if let Some(re) = mention_pattern() {
        out = re
            .replace_all(&out, |c: &Captures<'_>| {
                format!(
                    "@{}",
                    names.get(&c[1]).map(String::as_str).unwrap_or("someone")
                )
            })
            .into_owned();
    }
    if let Some(re) = LINK
        .get_or_init(|| Regex::new(r"<([^<>|]+)(?:\|([^<>]*))?>").ok())
        .as_ref()
    {
        out = re
            .replace_all(&out, |c: &Captures<'_>| {
                let target = &c[1];
                let label = c.get(2).map(|m| m.as_str()).filter(|l| !l.is_empty());
                if let Some(rest) = target.strip_prefix('#') {
                    let name = label.unwrap_or(rest);
                    format!("#{name}")
                } else if let Some(special) = target.strip_prefix('!') {
                    let word = special.split('^').next().unwrap_or(special);
                    format!("@{word}")
                } else {
                    label.unwrap_or(target).to_string()
                }
            })
            .into_owned();
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// A Slack timestamp as a time.
pub fn ts_time(ts: &str) -> Option<DateTime<Utc>> {
    let seconds = ts.split('.').next()?.parse::<i64>().ok()?;
    Utc.timestamp_opt(seconds, 0).single()
}

/// The Slack timestamp of `time` (seconds, six zero decimals).
pub fn time_ts(time: DateTime<Utc>) -> String {
    format!("{}.000000", time.timestamp())
}

/// The permalink of message `ts` from a channel's permalink base
/// (`https://x.slack.com/archives/C1/`).
pub fn derived_permalink(base: &str, ts: &str) -> String {
    format!("{base}p{}", ts.replace('.', ""))
}

/// The base of `permalink` (the permalink of message `ts` of `channel`) when it has
/// Slack's `https://<host>/archives/<channel>/p<ts digits>` shape; then the permalinks of
/// the channel's other messages are derived from it ([`derived_permalink`]).
pub fn permalink_base(permalink: &str, channel: &str, ts: &str) -> Option<String> {
    let rest = permalink.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    if host.is_empty() || host.contains(['?', '#']) {
        return None;
    }
    let expected = format!("archives/{channel}/p{}", ts.replace('.', ""));
    (path == expected).then(|| format!("https://{host}/archives/{channel}/"))
}

/// One thread as a document of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDocument {
    /// `slack://<team>/<channel>/<thread_ts>`.
    pub source: String,
    pub space_id: String,
    pub title: String,
    pub text: String,
    pub permalink: Option<String>,
    pub channel_name: String,
    pub thread_ts: String,
    pub latest_reply: Option<String>,
    pub reply_count: usize,
    /// The `ts` of every message it holds.
    pub message_ts: Vec<String>,
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// What one message reads as in a document: its words, then its files.
fn message_body(message: &SlackMessage, names: &HashMap<String, String>) -> String {
    let mut body = readable_text(message.text.trim(), names);
    if !message.files.is_empty() {
        if !body.is_empty() {
            body.push(' ');
        }
        body.push_str(&format!("[files: {}]", message.files.join(", ")));
    }
    body
}

/// The document of the thread whose messages are `messages` (its first message first):
/// `None` when none of them is indexed ([`SlackMessage::indexed`]).
#[allow(clippy::too_many_arguments)]
pub fn thread_document(
    team: &str,
    channel: &str,
    channel_name: &str,
    thread_ts: &str,
    messages: &[SlackMessage],
    names: &HashMap<String, String>,
    permalink: Option<String>,
    bot_user: &str,
) -> Option<ThreadDocument> {
    let kept: Vec<&SlackMessage> = messages
        .iter()
        .filter(|m| m.indexed(bot_user))
        .take(MAX_THREAD_MESSAGES)
        .collect();
    let first = kept.first()?;
    let name_of = |user: &Option<String>| -> String {
        user.as_ref()
            .and_then(|u| names.get(u))
            .cloned()
            .unwrap_or_else(|| "Someone".to_string())
    };
    let mut lines = Vec::with_capacity(kept.len() + 1);
    let started = ts_time(thread_ts)
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    lines.push(format!(
        "Slack thread in #{channel_name}, started {started} by {}.",
        name_of(&first.user)
    ));
    for message in &kept {
        let when = ts_time(&message.ts)
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default();
        lines.push(format!(
            "{} ({when}): {}",
            name_of(&message.user),
            message_body(message, names)
        ));
    }
    let opening = clip(&message_body(first, names), TITLE_CHARS);
    let title = if opening.is_empty() {
        format!("#{channel_name}")
    } else {
        format!("#{channel_name}: {opening}")
    };
    let parent = messages.iter().find(|m| m.ts == thread_ts);
    Some(ThreadDocument {
        source: ids::thread_source(team, channel, thread_ts),
        space_id: ids::space_id(team, channel),
        title,
        text: lines.join("\n"),
        permalink,
        channel_name: channel_name.to_string(),
        thread_ts: thread_ts.to_string(),
        latest_reply: parent.and_then(|p| p.latest_reply.clone()).or_else(|| {
            messages
                .iter()
                .rev()
                .find(|m| m.ts != thread_ts)
                .map(|m| m.ts.clone())
        }),
        reply_count: parent
            .map(|p| p.reply_count)
            .filter(|n| *n > 0)
            .unwrap_or_else(|| messages.iter().filter(|m| m.ts != thread_ts).count()),
        message_ts: messages.iter().map(|m| m.ts.clone()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn message(value: Value) -> SlackMessage {
        SlackMessage::from_value(&value).unwrap()
    }

    #[test]
    fn people_are_indexed_bots_and_noise_are_not() {
        let person = message(json!({"ts": "1.1", "user": "U1", "text": "hello"}));
        assert!(person.indexed("UBOT"));
        for noise in [
            json!({"ts": "1.2", "user": "UBOT", "text": "Saved."}),
            json!({"ts": "1.3", "bot_id": "B1", "user": "U9", "text": "deploy done"}),
            json!({"ts": "1.4", "subtype": "bot_message", "text": "ci"}),
            json!({"ts": "1.5", "subtype": "channel_join", "user": "U1", "text": "<@U1> has joined"}),
            json!({"ts": "1.6", "subtype": "channel_leave", "user": "U1", "text": "left"}),
            json!({"ts": "1.7", "subtype": "channel_topic", "user": "U1", "text": "topic"}),
            json!({"ts": "1.8", "user": "U1", "text": "  "}),
            json!({"ts": "1.9", "user": "U1", "text": "x", "hidden": true}),
            json!({"ts": "2.0", "text": "no sender"}),
        ] {
            assert!(!message(noise.clone()).indexed("UBOT"), "{noise}");
        }
        let file = message(
            json!({"ts": "2.1", "user": "U1", "subtype": "file_share", "text": "",
            "files": [{"name": "plan.pdf", "title": "Q3 plan"}, {"name": "x.png"}]}),
        );
        assert!(file.indexed("UBOT"));
        assert_eq!(file.files, vec!["Q3 plan".to_string(), "x.png".to_string()]);
        assert!(message(json!({"ts": "2.2", "user": "U1", "subtype": "thread_broadcast", "text": "also", "thread_ts": "1.1"})).indexed("UBOT"));
        assert!(message(
            json!({"ts": "2.3", "user": "U1", "subtype": "me_message", "text": "waves"})
        )
        .indexed("UBOT"));
        assert!(SlackMessage::from_value(&json!({"user": "U1"})).is_none());
    }

    #[test]
    fn mentions_links_and_escapes_read_naturally() {
        let names: HashMap<String, String> = [("U1".to_string(), "Ann Lee".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            readable_text(
                "<@U1> and <@U2|bob> see <#C1|general>, <https://x.org|the site>, <https://y.org> <!here> &lt;3 &amp; co",
                &names
            ),
            "@Ann Lee and @someone see #general, the site, https://y.org @here <3 & co"
        );
        assert_eq!(
            mentioned_users("<@U1> <@W2|x> <@U1> <#C1>"),
            ["U1".to_string(), "W2".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn permalinks_are_derived_only_from_the_shape_slack_gives() {
        let link = "https://acme.slack.com/archives/C1/p1700000000000100";
        let base = permalink_base(link, "C1", "1700000000.000100").unwrap();
        assert_eq!(base, "https://acme.slack.com/archives/C1/");
        assert_eq!(
            derived_permalink(&base, "1700000500.000200"),
            "https://acme.slack.com/archives/C1/p1700000500000200"
        );
        assert_eq!(permalink_base(link, "C2", "1700000000.000100"), None);
        assert_eq!(
            permalink_base(&format!("{link}?thread_ts=1"), "C1", "1700000000.000100"),
            None
        );
        assert_eq!(
            permalink_base(
                "http://acme.slack.com/archives/C1/p1700000000000100",
                "C1",
                "1700000000.000100"
            ),
            None
        );
    }

    #[test]
    fn a_thread_becomes_one_document() {
        let names: HashMap<String, String> = [
            ("U1".to_string(), "Ann".to_string()),
            ("U2".to_string(), "Bob".to_string()),
        ]
        .into_iter()
        .collect();
        let messages = vec![
            message(
                json!({"ts": "1700000000.000100", "thread_ts": "1700000000.000100", "user": "U1",
                "text": "Can <@U2> send the lease by Friday?", "reply_count": 2, "latest_reply": "1700000200.000100"}),
            ),
            message(
                json!({"ts": "1700000100.000100", "thread_ts": "1700000000.000100", "user": "UBOT", "text": "Searching…"}),
            ),
            message(
                json!({"ts": "1700000200.000100", "thread_ts": "1700000000.000100", "user": "U2",
                "text": "Yes, attached", "subtype": "file_share", "files": [{"name": "lease.pdf"}]}),
            ),
        ];
        let doc = thread_document(
            "T1",
            "C1",
            "deals",
            "1700000000.000100",
            &messages,
            &names,
            Some("https://acme.slack.com/archives/C1/p1700000000000100".into()),
            "UBOT",
        )
        .unwrap();
        assert_eq!(doc.source, "slack://T1/C1/1700000000.000100");
        assert_eq!(doc.space_id, "slack:T1:C1");
        assert_eq!(doc.title, "#deals: Can @Bob send the lease by Friday?");
        assert!(
            doc.text
                .starts_with("Slack thread in #deals, started 2023-11-14 by Ann."),
            "{}",
            doc.text
        );
        assert!(
            doc.text
                .contains("Ann (2023-11-14 22:13 UTC): Can @Bob send the lease by Friday?"),
            "{}",
            doc.text
        );
        assert!(doc
            .text
            .contains("Bob (2023-11-14 22:16 UTC): Yes, attached [files: lease.pdf]"));
        assert!(
            !doc.text.contains("Searching"),
            "the bot's own message is left out"
        );
        assert_eq!(doc.reply_count, 2);
        assert_eq!(doc.latest_reply.as_deref(), Some("1700000200.000100"));
        assert_eq!(doc.message_ts.len(), 3, "every message maps to the thread");
        let only_bots = vec![message(json!({"ts": "1.1", "bot_id": "B1", "text": "x"}))];
        assert!(
            thread_document("T1", "C1", "deals", "1.1", &only_bots, &names, None, "UBOT").is_none()
        );
    }

    #[test]
    fn timestamps_convert() {
        let t = ts_time("1700000000.000100").unwrap();
        assert_eq!(t.timestamp(), 1_700_000_000);
        assert_eq!(time_ts(t), "1700000000.000000");
        assert!(ts_time("x").is_none());
    }
}
