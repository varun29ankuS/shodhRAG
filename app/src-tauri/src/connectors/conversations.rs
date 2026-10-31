//! Connector conversations in the app: every exchange Shodh has in a connector (a question
//! asked in Slack or Telegram and Shodh's answer) is kept as an app conversation, so the
//! user sees it in the sidebar, finds it in search and history, and exports it.
//!
//! - **One conversation per chat**: a Slack thread (or a DM question outside a thread), a
//!   Telegram private chat (until `/new`), a Telegram group's reply thread. Its id is
//!   derived from the connector and the chat ([`conversation_id`]), so the connector finds
//!   it again and the UI can tell it apart ([`is_connector_id`]).
//! - **The same store as Ask** (`conversations.json`, [`ConversationStore`]), with a
//!   [`ConversationSource`] that marks it. The app shows it read-only; only the connector
//!   adds to it, and a late UI save never overwrites it or brings back a forgotten one
//!   ([`crate::conversation_commands::save_record`]).
//! - **Who said what**: each question carries its author's name, whether it was the
//!   owner (the user), and a link to the message when there is one, in the message's
//!   metadata (`connector`).
//! - **Memory** (decided): the owner's own words go through the app's normal learning
//!   path, exactly like an Ask chat ([`words_to_learn`]); other people's words never reach
//!   the user's memories automatically. They stay context in their conversation only.
//! - **Forgetting** a connector (Disconnect with forget, a forgotten Slack channel)
//!   removes its conversations ([`forget`]).

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::conversation_commands::{
    ConversationMessage, ConversationRecord, ConversationSource, ConversationStore,
};

/// Emitted with a [`ConversationChange`] whenever a connector conversation is added to,
/// created or removed, so open windows show it without a reload.
pub const CONNECTOR_CONVERSATION_EVENT: &str = "connector-conversation-changed";

/// Every connector conversation's id starts with this.
pub const ID_PREFIX: &str = "connector-";

/// Messages kept per connector conversation; the oldest go first. A Telegram chat that is
/// never reset with `/new` would otherwise grow without end.
pub const MAX_MESSAGES: usize = 400;

/// Characters of the first question used as a new conversation's title.
const TITLE_CHARS: usize = 60;

/// The connectors whose conversations the app keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connector {
    Slack,
    Telegram,
}

impl Connector {
    pub fn as_str(self) -> &'static str {
        match self {
            Connector::Slack => "slack",
            Connector::Telegram => "telegram",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Connector::Slack => "Slack",
            Connector::Telegram => "Telegram",
        }
    }
}

/// A question as it was asked in the connector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    /// The name people see (never an e-mail address or an id).
    pub author: String,
    /// The user themselves (the linked Telegram account, the Slack member picked as "me").
    pub owner: bool,
    pub text: String,
    pub at: DateTime<Utc>,
    /// The message in the connector, when it has a link.
    pub link: Option<String>,
}

/// One question and Shodh's answer to it, in one chat of a connector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    pub connector: Connector,
    /// The chat within the connector: only letters, digits, `_` and `-` (see
    /// [`conversation_id`]).
    pub chat: String,
    /// What the sidebar groups it under ("Slack · #deals", "Telegram · you").
    pub label: String,
    /// The chat in the connector, when it has a link.
    pub chat_link: Option<String>,
    /// Unique within the chat (a Slack message `ts`, a Telegram message id): the same
    /// exchange kept twice is kept once.
    pub turn_id: String,
    pub question: Said,
    /// Shodh's answer as it was sent.
    pub answer: String,
    pub answered_at: DateTime<Utc>,
    /// False when the question was handled without learning (a request to remember
    /// something, saved as asked): it never costs a learning call.
    pub learn: bool,
    /// Whether the answer may be given to learning as context. Not when it was written
    /// for a group or a channel: it is built on other people's words, which never reach
    /// the user's memories, not even through the answer.
    pub answer_as_context: bool,
    /// When the model fell back to another for this answer: "Answered by Gemma 4 31B —
    /// Nemotron Super is unavailable". Shown under the answer in the app only.
    pub answered_by: Option<String>,
}

/// What a window is told.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationChange {
    /// The conversation as it is now (added to or created).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationRecord>,
    /// Conversations that were removed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

/// `text` with every character other than ASCII letters, digits, `_` and `-` replaced by
/// `_` (a chat key part, an id).
pub fn key_part(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The id of the conversation of `chat` in `connector`.
pub fn conversation_id(connector: Connector, chat: &str) -> String {
    format!("{ID_PREFIX}{}-{}", connector.as_str(), key_part(chat))
}

/// Whether `id` is a connector conversation's (never one the UI makes: `conv-…`).
pub fn is_connector_id(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

/// The words of `exchange` that go to learning: the owner's own question only, and only
/// when it was not handled otherwise. Other people's words never do.
pub fn words_to_learn(exchange: &Exchange) -> Option<&str> {
    let text = exchange.question.text.trim();
    (exchange.question.owner && exchange.learn && !text.is_empty()).then_some(text)
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn title_of(question: &str) -> String {
    let flat = question.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return "Conversation".to_string();
    }
    if flat.chars().count() <= TITLE_CHARS {
        return flat;
    }
    let mut title: String = flat.chars().take(TITLE_CHARS - 1).collect();
    title.push('…');
    title
}

/// Adds `exchange` to its conversation (created on its first exchange) in `conversations`;
/// returns the conversation as it is now. The same exchange added again changes nothing.
pub fn add_exchange(
    conversations: &mut Vec<ConversationRecord>,
    exchange: &Exchange,
) -> ConversationRecord {
    let id = conversation_id(exchange.connector, &exchange.chat);
    let index = match conversations.iter().position(|c| c.id == id) {
        Some(index) => index,
        None => {
            let created = rfc3339(exchange.question.at);
            conversations.push(ConversationRecord {
                id: id.clone(),
                title: title_of(&exchange.question.text),
                messages: Vec::new(),
                created_at: created.clone(),
                updated_at: created,
                pinned: false,
                space_id: None,
                space_name: None,
                workspace_id: None,
                system_prompt: None,
                mode: None,
                focus_threads: None,
                source: None,
                seed: None,
                about_file: None,
            });
            conversations.len() - 1
        }
    };
    let record = &mut conversations[index];
    // The connector's record, whatever an older version or a UI save left in it.
    record.source = Some(ConversationSource {
        connector: exchange.connector.as_str().to_string(),
        chat: exchange.chat.clone(),
        label: exchange.label.clone(),
        link: exchange.chat_link.clone(),
    });
    let turn = key_part(&exchange.turn_id);
    let question_id = format!("{turn}-q");
    if record.messages.iter().any(|m| m.id == question_id) {
        return record.clone();
    }
    record.messages.push(ConversationMessage {
        id: question_id,
        role: "user".to_string(),
        content: exchange.question.text.trim().to_string(),
        timestamp: rfc3339(exchange.question.at),
        artifacts: None,
        search_results: None,
        metadata: Some(json!({
            "connector": {
                "connector": exchange.connector.as_str(),
                "author": exchange.question.author,
                "owner": exchange.question.owner,
                "link": exchange.question.link,
            }
        })),
        run: None,
        transcript: None,
    });
    record.messages.push(ConversationMessage {
        id: format!("{turn}-a"),
        role: "assistant".to_string(),
        content: exchange.answer.trim().to_string(),
        timestamp: rfc3339(exchange.answered_at),
        artifacts: None,
        search_results: None,
        metadata: Some(json!({
            "connector": {
                "connector": exchange.connector.as_str(),
                "author": "Shodh",
                "answeredBy": exchange.answered_by,
            }
        })),
        run: None,
        transcript: None,
    });
    let excess = record.messages.len().saturating_sub(MAX_MESSAGES);
    if excess > 0 {
        record.messages.drain(..excess);
    }
    record.updated_at = rfc3339(exchange.answered_at);
    record.clone()
}

/// Removes the conversations of `connector` whose chat starts with `chat_prefix` (all of
/// the connector's with `None`); returns their ids.
pub fn remove_conversations(
    conversations: &mut Vec<ConversationRecord>,
    connector: Connector,
    chat_prefix: Option<&str>,
) -> Vec<String> {
    let mut removed = Vec::new();
    conversations.retain(|c| {
        let matches = c.source.as_ref().is_some_and(|s| {
            s.connector == connector.as_str()
                && chat_prefix.is_none_or(|prefix| s.chat.starts_with(prefix))
        });
        if matches {
            removed.push(c.id.clone());
        }
        !matches
    });
    removed
}

/// One earlier question and answer of a conversation, oldest first: the context a
/// follow-up in the same chat is answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlierTurn {
    pub question: String,
    pub answer: String,
}

/// The last `max` exchanges of the conversation of `chat`, oldest first.
pub fn earlier_turns(
    conversations: &[ConversationRecord],
    connector: Connector,
    chat: &str,
    max: usize,
) -> Vec<EarlierTurn> {
    let id = conversation_id(connector, chat);
    let Some(record) = conversations.iter().find(|c| c.id == id) else {
        return Vec::new();
    };
    let mut turns = Vec::new();
    let mut question: Option<&str> = None;
    for message in &record.messages {
        match message.role.as_str() {
            "user" => question = Some(&message.content),
            "assistant" => {
                if let Some(q) = question.take() {
                    turns.push(EarlierTurn {
                        question: q.to_string(),
                        answer: message.content.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    let skip = turns.len().saturating_sub(max);
    turns.into_iter().skip(skip).collect()
}

fn store_of(app: &AppHandle) -> Result<ConversationStore, String> {
    let dir = crate::profile::app_data_dir(app).map_err(|e| e.to_string())?;
    Ok(ConversationStore::in_dir(&dir))
}

fn emit(app: &AppHandle, change: &ConversationChange) {
    if let Err(e) = app.emit(CONNECTOR_CONVERSATION_EVENT, change) {
        tracing::warn!(target: "shodh::connectors", error = %e, "a connector conversation change was not told to the windows");
    }
}

/// The answer learning may read as context for `exchange` (empty: none).
pub fn learning_context(exchange: &Exchange) -> &str {
    if exchange.answer_as_context {
        &exchange.answer
    } else {
        ""
    }
}

/// Keeps `exchange` in its conversation, tells the windows, and hands the owner's own words
/// to learning ([`words_to_learn`]). Failures are logged: the answer was given already.
pub async fn keep(app: &AppHandle, exchange: Exchange) {
    let store = match store_of(app) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(target: "shodh::connectors", error = %e, "a connector conversation could not be kept");
            return;
        }
    };
    let kept = exchange.clone();
    let saved = tokio::task::spawn_blocking(move || {
        store.update(|conversations| Ok(add_exchange(conversations, &kept)))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    match saved {
        Ok(record) => emit(
            app,
            &ConversationChange {
                conversation: Some(record),
                removed: Vec::new(),
            },
        ),
        Err(e) => {
            tracing::warn!(target: "shodh::connectors", error = %e, "a connector conversation could not be kept")
        }
    }
    if let Some(words) = words_to_learn(&exchange) {
        if let Some(learn) = app.try_state::<crate::memory_learn::LearnState>() {
            learn.record_connector_turn(
                &conversation_id(exchange.connector, &exchange.chat),
                &format!(
                    "{}-{}",
                    exchange.connector.as_str(),
                    key_part(&exchange.turn_id)
                ),
                words,
                learning_context(&exchange),
                exchange.question.at,
            );
        }
    }
}

/// The last `max` exchanges of the conversation of `chat` (none when it cannot be read).
pub async fn earlier(
    app: &AppHandle,
    connector: Connector,
    chat: &str,
    max: usize,
) -> Vec<EarlierTurn> {
    let store = match store_of(app) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(target: "shodh::connectors", error = %e, "a connector conversation could not be read");
            return Vec::new();
        }
    };
    let chat = chat.to_string();
    let read = tokio::task::spawn_blocking(move || {
        store
            .load()
            .map(|all| earlier_turns(&all, connector, &chat, max))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    read.unwrap_or_else(|e| {
        tracing::warn!(target: "shodh::connectors", error = %e, "a connector conversation could not be read");
        Vec::new()
    })
}

/// Removes the conversations of `connector` whose chat starts with `chat_prefix` (all with
/// `None`) and tells the windows; returns how many were removed.
pub async fn forget(
    app: &AppHandle,
    connector: Connector,
    chat_prefix: Option<String>,
) -> Result<usize, String> {
    let store = store_of(app)?;
    let removed = tokio::task::spawn_blocking(move || {
        store.update(|conversations| {
            Ok(remove_conversations(
                conversations,
                connector,
                chat_prefix.as_deref(),
            ))
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    let count = removed.len();
    if count > 0 {
        emit(
            app,
            &ConversationChange {
                conversation: None,
                removed,
            },
        );
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::conversation_commands::save_record;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 27, 9, minute, 0).unwrap()
    }

    fn exchange(chat: &str, turn: &str, owner: bool, text: &str) -> Exchange {
        Exchange {
            connector: Connector::Telegram,
            chat: chat.to_string(),
            label: "Telegram · you".to_string(),
            chat_link: Some("https://t.me/shodh_bot".to_string()),
            turn_id: turn.to_string(),
            question: Said {
                author: if owner { "You" } else { "Alex" }.to_string(),
                owner,
                text: text.to_string(),
                at: at(1),
                link: None,
            },
            answer: "The notice period is sixty days [1].".to_string(),
            answered_at: at(2),
            learn: true,
            answer_as_context: true,
            answered_by: None,
        }
    }

    #[test]
    fn exchanges_are_kept_in_one_conversation_per_chat() {
        let mut all = Vec::new();
        let first = add_exchange(
            &mut all,
            &exchange("u42-0", "7", true, "What is the notice period?"),
        );
        assert_eq!(first.id, "connector-telegram-u42-0");
        assert!(is_connector_id(&first.id));
        assert_eq!(first.title, "What is the notice period?");
        let source = first.source.clone().unwrap();
        assert_eq!(source.connector, "telegram");
        assert_eq!(source.label, "Telegram · you");
        assert_eq!(first.messages.len(), 2);
        assert_eq!(first.messages[0].role, "user");
        let meta = first.messages[0].metadata.clone().unwrap();
        assert_eq!(meta["connector"]["author"], "You");
        assert_eq!(meta["connector"]["owner"], true);
        assert_eq!(first.messages[1].role, "assistant");

        // The same exchange again (a retry): kept once.
        add_exchange(
            &mut all,
            &exchange("u42-0", "7", true, "What is the notice period?"),
        );
        let again = add_exchange(&mut all, &exchange("u42-0", "8", true, "And the deposit?"));
        assert_eq!(all.len(), 1);
        assert_eq!(again.messages.len(), 4);
        assert_eq!(again.title, "What is the notice period?", "the title stays");

        // Another chat is another conversation.
        add_exchange(&mut all, &exchange("u42-1", "9", true, "New topic"));
        assert_eq!(all.len(), 2);
        let turns = earlier_turns(&all, Connector::Telegram, "u42-0", 10);
        assert_eq!(
            turns
                .iter()
                .map(|t| t.question.as_str())
                .collect::<Vec<_>>(),
            ["What is the notice period?", "And the deposit?"]
        );
        assert_eq!(
            earlier_turns(&all, Connector::Telegram, "u42-0", 1).len(),
            1
        );
        assert!(earlier_turns(&all, Connector::Slack, "u42-0", 10).is_empty());
    }

    #[test]
    fn ids_are_safe_and_never_the_uis() {
        assert_eq!(
            conversation_id(Connector::Slack, "T1:C1:1700000000.000100"),
            "connector-slack-T1_C1_1700000000_000100"
        );
        assert!(!is_connector_id("conv-1761555000000-ab12cd"));
        assert_eq!(key_part("a/b\\c d"), "a_b_c_d");
    }

    #[test]
    fn only_the_owners_own_words_are_learned_from() {
        let owner = exchange("u42-0", "1", true, "I moved to Pune last month");
        assert_eq!(words_to_learn(&owner), Some("I moved to Pune last month"));
        let other = exchange("g7-t1", "2", false, "Remember that the budget is 10 lakh");
        assert_eq!(words_to_learn(&other), None, "other people's words never");
        let handled = Exchange {
            learn: false,
            ..owner.clone()
        };
        assert_eq!(
            words_to_learn(&handled),
            None,
            "a handled request is not learned again"
        );
        assert_eq!(learning_context(&owner), owner.answer);
        let in_a_group = Exchange {
            answer_as_context: false,
            ..owner.clone()
        };
        assert_eq!(
            words_to_learn(&in_a_group),
            Some("I moved to Pune last month"),
            "the owner's own words in a group are still theirs"
        );
        assert_eq!(
            learning_context(&in_a_group),
            "",
            "but a group's answer, built on others' words, never reaches learning"
        );
        let empty = exchange("u42-0", "3", true, "   ");
        assert_eq!(words_to_learn(&empty), None);
    }

    #[test]
    fn a_long_chat_keeps_its_newest_messages() {
        let mut all = Vec::new();
        for n in 0..(MAX_MESSAGES / 2 + 5) {
            add_exchange(
                &mut all,
                &exchange("u1-0", &n.to_string(), true, &format!("q{n}")),
            );
        }
        let record = &all[0];
        assert_eq!(record.messages.len(), MAX_MESSAGES);
        assert_eq!(
            record.messages.last().unwrap().id,
            format!("{}-a", MAX_MESSAGES / 2 + 4)
        );
        assert_eq!(
            record.messages[0].role, "user",
            "whole exchanges are dropped"
        );
    }

    #[test]
    fn a_ui_save_keeps_what_the_connector_wrote() {
        let mut all = Vec::new();
        let kept = add_exchange(&mut all, &exchange("u42-0", "1", true, "First"));
        // The UI holds this copy; the connector adds an exchange meanwhile.
        let stale = kept.clone();
        add_exchange(&mut all, &exchange("u42-0", "2", true, "Second"));
        // The UI renames and pins its stale copy (and an older UI drops the source).
        let mut ui = stale;
        ui.title = "Lease".to_string();
        ui.pinned = true;
        ui.source = None;
        ui.messages.clear();
        save_record(&mut all, ui);
        assert_eq!(all[0].messages.len(), 4, "the connector's messages stay");
        assert!(all[0].source.is_some(), "and its source");
        assert_eq!(all[0].title, "Lease");
        assert!(all[0].pinned);
    }

    #[test]
    fn a_forgotten_conversation_is_not_brought_back_by_a_late_save() {
        let mut all = Vec::new();
        let telegram = add_exchange(&mut all, &exchange("u42-0", "1", true, "First"));
        let mut slack = exchange("T1-C1-1_1", "2", false, "Hi");
        slack.connector = Connector::Slack;
        add_exchange(&mut all, &slack);
        let removed = remove_conversations(&mut all, Connector::Telegram, None);
        assert_eq!(removed, vec![telegram.id.clone()]);
        assert_eq!(all.len(), 1, "Slack's stay");
        save_record(&mut all, telegram);
        assert_eq!(all.len(), 1, "a stale UI copy does not come back");
        // Only one Slack channel's.
        let mut other = exchange("T1-C2-3_3", "3", false, "Hey");
        other.connector = Connector::Slack;
        add_exchange(&mut all, &other);
        let removed = remove_conversations(&mut all, Connector::Slack, Some("T1-C1-"));
        assert_eq!(removed.len(), 1);
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn an_ask_chat_never_becomes_a_connector_conversation_through_a_save() {
        let mut all: Vec<ConversationRecord> = Vec::new();
        let mut chat = add_exchange(&mut Vec::new(), &exchange("u1-0", "1", true, "Q"));
        chat.id = "conv-1-abc".to_string();
        // A new record that claims a source is not taken.
        save_record(&mut all, chat.clone());
        assert!(all.is_empty());
        let plain = ConversationRecord {
            source: None,
            ..chat.clone()
        };
        save_record(&mut all, plain);
        assert_eq!(all.len(), 1);
        save_record(&mut all, chat);
        assert!(
            all[0].source.is_none(),
            "the UI cannot mark a chat as a connector's"
        );
    }

    #[test]
    fn kept_conversations_survive_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::in_dir(dir.path());
        let ex = exchange("u42-0", "1", false, "Hello");
        store.update(|all| Ok(add_exchange(all, &ex))).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].source.as_ref().unwrap().chat, "u42-0");
        let meta = loaded[0].messages[0].metadata.clone().unwrap();
        assert_eq!(meta["connector"]["owner"], false);
        assert_eq!(meta["connector"]["author"], "Alex");
    }
}
