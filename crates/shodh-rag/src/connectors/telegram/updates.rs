//! What an update from `getUpdates` says (<https://core.telegram.org/bots/api#update>):
//! a message, or the bot's own membership in a chat changing (added to a group, removed
//! from one). Anything else is ignored but still confirmed, so it is never fetched again.
//!
//! Whether a group message is meant for the bot ([`addressed`]): a mention of the bot
//! (`@name`, or a mention entity of the bot's user), a command meant for it
//! (`/ask@name …`), or a reply to one of its messages. In privacy mode (the default)
//! Telegram itself sends a bot only commands meant for it and replies to it
//! (<https://core.telegram.org/bots/features#privacy-mode>); plain mentions arrive only
//! when privacy mode is off. Shodh answers the same in both: only what is meant for it.

use chrono::{DateTime, Utc};
use serde_json::Value;

/// Longest message text read (Telegram's own limit is 4 096).
pub const MAX_TEXT_CHARS: usize = 4_096;

/// What kind of chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    Private,
    Group,
    Supergroup,
    Channel,
}

impl ChatKind {
    pub fn is_group(self) -> bool {
        matches!(self, ChatKind::Group | ChatKind::Supergroup)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chat {
    pub id: i64,
    pub kind: ChatKind,
    /// A group's title.
    pub title: Option<String>,
}

/// Someone on Telegram (the sender of a message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub id: i64,
    /// First and last name, as Telegram shows them.
    pub name: String,
    /// Without the `@`.
    pub username: Option<String>,
    pub is_bot: bool,
}

/// The message a message replies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyTo {
    pub message_id: i64,
    /// Its sender, when Telegram says.
    pub from: Option<i64>,
}

/// A mention in a message's entities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mention {
    /// `@username`, as written (without the `@`).
    Username(String),
    /// A mention of a user without a username, by id.
    User(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub update_id: i64,
    pub message_id: i64,
    pub chat: Chat,
    pub from: Option<Person>,
    /// The text, or a photo's or file's caption. Untrusted.
    pub text: String,
    /// The message carried media (a photo, a file, a voice note…).
    pub has_media: bool,
    pub mentions: Vec<Mention>,
    pub reply_to: Option<ReplyTo>,
    pub date: DateTime<Utc>,
    /// The group became a supergroup with this new chat id.
    pub migrate_to: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Message(Box<Message>),
    /// The bot was added to (`joined`) or removed from `chat`, by `by`.
    Membership {
        update_id: i64,
        chat: Chat,
        by: Option<Person>,
        joined: bool,
    },
    /// Something the connector does not use (an edit, a channel post…).
    Other {
        update_id: i64,
    },
}

impl Update {
    pub fn id(&self) -> i64 {
        match self {
            Update::Message(m) => m.update_id,
            Update::Membership { update_id, .. } | Update::Other { update_id } => *update_id,
        }
    }
}

fn chat(value: &Value) -> Option<Chat> {
    let id = value.get("id")?.as_i64()?;
    let kind = match value.get("type")?.as_str()? {
        "private" => ChatKind::Private,
        "group" => ChatKind::Group,
        "supergroup" => ChatKind::Supergroup,
        "channel" => ChatKind::Channel,
        _ => return None,
    };
    Some(Chat {
        id,
        kind,
        title: value
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn person(value: &Value) -> Option<Person> {
    let id = value.get("id")?.as_i64()?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let name = match (text("first_name"), text("last_name")) {
        (Some(first), Some(last)) => format!("{first} {last}"),
        (Some(first), None) => first.to_string(),
        (None, Some(last)) => last.to_string(),
        (None, None) => text("username").unwrap_or("Someone").to_string(),
    };
    Some(Person {
        id,
        name,
        username: text("username").map(str::to_string),
        is_bot: value
            .get("is_bot")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// The part of `text` an entity covers: Telegram counts offsets in UTF-16 code units.
fn utf16_slice(text: &str, offset: usize, length: usize) -> Option<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let end = offset.checked_add(length)?;
    units.get(offset..end).map(String::from_utf16_lossy)
}

fn mentions(text: &str, entities: Option<&Value>) -> Vec<Mention> {
    let mut out = Vec::new();
    for entity in entities.and_then(Value::as_array).into_iter().flatten() {
        let kind = entity
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "mention" => {
                let offset = entity.get("offset").and_then(Value::as_u64);
                let length = entity.get("length").and_then(Value::as_u64);
                if let (Some(offset), Some(length)) = (offset, length) {
                    if let Some(name) = utf16_slice(text, offset as usize, length as usize) {
                        out.push(Mention::Username(name.trim_start_matches('@').to_string()));
                    }
                }
            }
            "text_mention" => {
                if let Some(id) = entity.pointer("/user/id").and_then(Value::as_i64) {
                    out.push(Mention::User(id));
                }
            }
            _ => {}
        }
    }
    out
}

/// The update `value` says, `None` when it has no `update_id`.
pub fn parse_update(value: &Value) -> Option<Update> {
    let update_id = value.get("update_id")?.as_i64()?;
    if let Some(member) = value.get("my_chat_member") {
        let Some(chat) = member.get("chat").and_then(chat) else {
            return Some(Update::Other { update_id });
        };
        let status = member
            .pointer("/new_chat_member/status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let joined = matches!(status, "member" | "administrator" | "restricted");
        return Some(Update::Membership {
            update_id,
            chat,
            by: member.get("from").and_then(person),
            joined,
        });
    }
    let Some(message) = value.get("message") else {
        return Some(Update::Other { update_id });
    };
    let (Some(message_id), Some(chat)) = (
        message.get("message_id").and_then(Value::as_i64),
        message.get("chat").and_then(chat),
    ) else {
        return Some(Update::Other { update_id });
    };
    let (text, entities) = match message.get("text").and_then(Value::as_str) {
        Some(text) => (text, message.get("entities")),
        None => (
            message
                .get("caption")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            message.get("caption_entities"),
        ),
    };
    let has_media = [
        "photo",
        "document",
        "voice",
        "audio",
        "video",
        "video_note",
        "sticker",
        "animation",
    ]
    .iter()
    .any(|key| message.get(key).is_some());
    let text: String = text.chars().take(MAX_TEXT_CHARS).collect();
    Some(Update::Message(Box::new(Message {
        update_id,
        message_id,
        mentions: mentions(&text, entities),
        chat,
        from: message.get("from").and_then(person),
        text,
        has_media,
        reply_to: message.get("reply_to_message").and_then(|r| {
            Some(ReplyTo {
                message_id: r.get("message_id")?.as_i64()?,
                from: r.pointer("/from/id").and_then(Value::as_i64),
            })
        }),
        date: message
            .get("date")
            .and_then(Value::as_i64)
            .and_then(|s| DateTime::from_timestamp(s, 0))
            .unwrap_or_else(Utc::now),
        migrate_to: message.get("migrate_to_chat_id").and_then(Value::as_i64),
    })))
}

/// The bot, as messages name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotSelf {
    pub id: i64,
    /// Without the `@`.
    pub username: String,
}

/// A command at the start of a message (`/new`, `/start CODE`, `/ask@bot question`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Lower case, without the `/` or the `@bot` part.
    pub name: String,
    /// Whether it named this bot (`/ask@this_bot`).
    pub for_this_bot: bool,
    /// What follows the command.
    pub rest: String,
}

/// The command `text` starts with, `None` when it does not start with one, or names
/// another bot.
pub fn command(text: &str, bot: &BotSelf) -> Option<Command> {
    let text = text.trim_start();
    let body = text.strip_prefix('/')?;
    let (word, rest) = match body.find(char::is_whitespace) {
        Some(at) => (&body[..at], body[at..].trim()),
        None => (body, ""),
    };
    let (name, target) = match word.split_once('@') {
        Some((name, target)) => (name, Some(target)),
        None => (word, None),
    };
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    if target.is_some_and(|t| !t.eq_ignore_ascii_case(&bot.username)) {
        return None;
    }
    Some(Command {
        name: name.to_ascii_lowercase(),
        for_this_bot: target.is_some(),
        rest: rest.to_string(),
    })
}

/// `text` with every `@bot` mention removed (case-insensitive), spaces tidied.
pub fn without_mention(text: &str, bot: &BotSelf) -> String {
    let needle = format!("@{}", bot.username.to_ascii_lowercase());
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if lower[i..].starts_with(&needle) {
            let end = i + needle.len();
            let next = text[end..].chars().next();
            if !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                i = end;
                continue;
            }
        }
        let Some(c) = text[i..].chars().next() else {
            break;
        };
        out.push(c);
        i += c.len_utf8();
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// In a group: the question when `message` is meant for the bot (a mention of it, a
/// command meant for it, a reply to one of its messages), `None` otherwise.
pub fn addressed(message: &Message, bot: &BotSelf) -> Option<String> {
    if let Some(cmd) = command(&message.text, bot) {
        if cmd.name == "ask" && (cmd.for_this_bot || reply_to_bot(message, bot)) {
            return Some(without_mention(&cmd.rest, bot));
        }
        return None;
    }
    let mentioned = message.mentions.iter().any(|m| match m {
        Mention::Username(name) => name.eq_ignore_ascii_case(&bot.username),
        Mention::User(id) => *id == bot.id,
    });
    (mentioned || reply_to_bot(message, bot)).then(|| without_mention(&message.text, bot))
}

/// Whether `message` replies to one of the bot's messages.
pub fn reply_to_bot(message: &Message, bot: &BotSelf) -> bool {
    message
        .reply_to
        .as_ref()
        .is_some_and(|r| r.from == Some(bot.id))
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    pub fn bot() -> BotSelf {
        BotSelf {
            id: 123_456_789,
            username: "shodh_notes_bot".into(),
        }
    }

    /// A message update in `chat` (positive: private with that user; negative: a group),
    /// sent at 2026-10-27 09:00 UTC (the responder tests' clock: answered on time).
    pub fn message_value(update_id: i64, chat: i64, from: i64, text: &str) -> Value {
        let chat_value = if chat > 0 {
            json!({"id": chat, "type": "private", "first_name": "Ann"})
        } else {
            json!({"id": chat, "type": "supergroup", "title": "Deals"})
        };
        json!({
            "update_id": update_id,
            "message": {
                "message_id": update_id * 10,
                "date": 1_793_091_600,
                "chat": chat_value,
                "from": {"id": from, "is_bot": false, "first_name": "Ann", "last_name": "Lee", "username": "annlee"},
                "text": text
            }
        })
    }

    pub fn message(update_id: i64, chat: i64, from: i64, text: &str) -> Message {
        match parse_update(&message_value(update_id, chat, from, text)) {
            Some(Update::Message(m)) => *m,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_message_is_read_with_its_sender_and_chat() {
        let m = message(5, 42, 42, "What is the notice period?");
        assert_eq!(m.update_id, 5);
        assert_eq!(m.message_id, 50);
        assert_eq!(m.chat.kind, ChatKind::Private);
        let from = m.from.unwrap();
        assert_eq!(from.name, "Ann Lee");
        assert_eq!(from.username.as_deref(), Some("annlee"));
        assert_eq!(m.date.timestamp(), 1_793_091_600);
        assert_eq!(parse_update(&json!({"message": {}})), None);
        assert_eq!(
            parse_update(&json!({"update_id": 9, "edited_message": {}})),
            Some(Update::Other { update_id: 9 })
        );
    }

    #[test]
    fn the_bot_added_to_a_group_or_removed_is_a_membership_change() {
        let added = json!({"update_id": 7, "my_chat_member": {
            "chat": {"id": -100, "type": "group", "title": "Deals"},
            "from": {"id": 42, "is_bot": false, "first_name": "Ann"},
            "new_chat_member": {"status": "member", "user": {"id": 123456789, "is_bot": true}}
        }});
        match parse_update(&added).unwrap() {
            Update::Membership {
                chat, by, joined, ..
            } => {
                assert!(joined);
                assert_eq!(chat.title.as_deref(), Some("Deals"));
                assert_eq!(by.unwrap().id, 42);
            }
            other => panic!("{other:?}"),
        }
        let mut removed = added.clone();
        removed["my_chat_member"]["new_chat_member"]["status"] = json!("kicked");
        assert!(matches!(
            parse_update(&removed).unwrap(),
            Update::Membership { joined: false, .. }
        ));
    }

    #[test]
    fn mentions_are_found_by_utf16_offsets() {
        // An emoji is two UTF-16 units: the entity's offset counts them so.
        let text = "😀 @Shodh_Notes_Bot what is the rent?";
        let mut value = message_value(1, -100, 42, text);
        value["message"]["entities"] = json!([{"type": "mention", "offset": 3, "length": 16}]);
        let Update::Message(m) = parse_update(&value).unwrap() else {
            panic!()
        };
        assert_eq!(
            m.mentions,
            vec![Mention::Username("Shodh_Notes_Bot".into())]
        );
        assert_eq!(
            addressed(&m, &bot()).as_deref(),
            Some("😀 what is the rent?")
        );
    }

    #[test]
    fn in_a_group_only_what_is_meant_for_the_bot_is_a_question() {
        let bot = bot();
        let plain = message(1, -100, 42, "what is the rent?");
        assert_eq!(addressed(&plain, &bot), None, "not meant for the bot");

        let ask = message(2, -100, 42, "/ask@shodh_notes_bot what is the rent?");
        assert_eq!(addressed(&ask, &bot).as_deref(), Some("what is the rent?"));
        let other_bot = message(3, -100, 42, "/ask@other_bot what is the rent?");
        assert_eq!(addressed(&other_bot, &bot), None);

        let mut reply = message(4, -100, 42, "and the deposit?");
        reply.reply_to = Some(ReplyTo {
            message_id: 99,
            from: Some(bot.id),
        });
        assert_eq!(addressed(&reply, &bot).as_deref(), Some("and the deposit?"));
        reply.reply_to = Some(ReplyTo {
            message_id: 99,
            from: Some(7),
        });
        assert_eq!(addressed(&reply, &bot), None, "a reply to someone else");

        let mut by_id = message(5, -100, 42, "Shodh, what is the rent?");
        by_id.mentions = vec![Mention::User(bot.id)];
        assert!(addressed(&by_id, &bot).is_some());
        // A mention of a name that only starts with the bot's is not the bot.
        assert_eq!(
            without_mention("@shodh_notes_bot2 hi @SHODH_NOTES_BOT", &bot),
            "@shodh_notes_bot2 hi"
        );
    }

    #[test]
    fn commands_are_read_with_their_target() {
        let bot = bot();
        assert_eq!(
            command("/start abc123_-x", &bot),
            Some(Command {
                name: "start".into(),
                for_this_bot: false,
                rest: "abc123_-x".into()
            })
        );
        assert_eq!(command("/New@Shodh_Notes_Bot", &bot).unwrap().name, "new");
        assert!(command("/new@other_bot", &bot).is_none());
        assert!(command("hello /new", &bot).is_none());
        assert!(command("/ hello", &bot).is_none());
    }
}
