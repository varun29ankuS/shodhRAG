//! One update, start to finish. The checks that cost nothing come first; the model is
//! called last, only for a real question in a chat Shodh answers.
//!
//! 1. The bot added to or removed from a group: the card lists the group as waiting for
//!    the owner (or forgets it). A group that became a supergroup keeps its place.
//! 2. Who and where ([`super::access::route`]): a link code, the owner's chat, an allowed
//!    person's or group's, a group not allowed yet (a short note, at most once in
//!    [`NOTE_EVERY`]), anyone else (a short refusal, at most once in [`NOTE_EVERY`] per
//!    person, so a stranger cannot flood the bot into Telegram's rate limits).
//! 3. Commands: `/new` starts a fresh conversation (private chats), `/start` and `/help`
//!    say what the bot does.
//! 4. **The owner**: what they ask to remember is saved first, as in Ask (a sensitive note
//!    waits for approval: "Open Shodh to approve it"); then the whole library and their
//!    memories, the chat so far, the model they chose, no tools ([`super::answer`]).
//! 5. **Others** (allowed people, allowed groups): the workspace chosen on the card, no
//!    memories, Telegram's own daily cap, the Slack answer path's rules.
//!    Before searching, for both: a message sent more than a day before it arrived gets a
//!    short note instead of an answer, once per chat for a backlog, and small talk is not
//!    searched (the owner gets a short model reply, others a fixed one)
//!    ([`crate::connectors::delivery`]). An answer to a message that waited more than ten
//!    minutes starts with when it was sent.
//! 6. "typing…" shows while Shodh works; the answer goes out in Telegram's HTML, in as
//!    many messages as it needs ([`super::format`]); in a group it replies to the
//!    question, and replies to it continue that thread.
//! 7. Every exchange is kept as the app's conversation of its chat; only the owner's own
//!    words reach learning (the app decides, `connectors::conversations`).
//!
//! **Stopping**: the connector's [`CancellationToken`] (switched off, disconnected, a new
//! token, Local-only mode, quit) stops an answer before the search, before the model and
//! before every message; nothing more is sent once it is cancelled.
//!
//! **Telegram refusing a message**: a 429 is waited out for its `retry_after` (a bounded
//! number of times, never longer than [`RetryPolicy::max_retry_after`]); a network error
//! or a 5xx gets one more try; HTML Telegram cannot read is sent again as plain text.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use tokio_util::sync::CancellationToken;

use super::access::{route, Route, Rules};
use super::answer::{
    owner_prompt, owner_query, owner_small_talk_prompt, OWNER_ANSWER_TOKENS, OWNER_PASSAGES,
    OWNER_SMALL_TALK_TOKENS, TELEGRAM,
};
use super::api::{BotApi, TelegramError};
use super::format::{answer_parts, note_part, owner_parts, Part};
use super::link::{LinkError, LinkKind};
use super::updates::{Chat, Message, Person, Update};
use crate::connectors::delivery::{
    is_small_talk, lateness, local_offset, stale_reply, with_late_line, Lateness, StaleNotes,
    SMALL_TALK_REPLY,
};
use crate::connectors::slack::answer::{
    answer_prompt_in, search_query, AnswerPassage, AnswerScope, ThreadTurn, MAX_ANSWER_TOKENS,
};
use crate::connectors::slack::respond::{
    at_cap_message, is_daily_free_quota_text, model_failure_message, reset_time, AnswerQuota,
    RetryPolicy,
};
use crate::connectors::slack::sanitize::{sanitize, PrivateTerms};
use crate::harness::model_fallback::AnsweredBy;
use crate::harness::provider_error::{classify, ProviderErrorKind};
use crate::research::results::TextModel;
use crate::research::tables::calls::{next_utc_midnight, utc_day, Clock};
use crate::user_memory::learn::{memory_request, MemoryRequest};

/// What a failed model call becomes in Telegram: the daily free model quota says when it
/// resets, in the computer's time (`offset_of`); anything else is the short reason Slack
/// also gives. Only the owner is told which provider and what would help: a guest or a
/// group learns nothing about the owner's setup.
pub fn model_failure_text(
    kind: ProviderErrorKind,
    error: &str,
    now: DateTime<Utc>,
    offset_of: fn(DateTime<Utc>) -> FixedOffset,
    audience: Audience,
) -> String {
    let now_ms = u64::try_from(now.timestamp_millis()).unwrap_or(0);
    if kind == ProviderErrorKind::QuotaExhausted && is_daily_free_quota_text(error) {
        let owner = audience == Audience::Owner;
        let quota = if owner {
            "Today's free OpenRouter quota"
        } else {
            "Today's free model quota"
        };
        let until = match reset_time(error, now_ms) {
            Some(reset) => format!(
                " until it resets at {}",
                reset.with_timezone(&offset_of(reset)).format("%H:%M")
            ),
            None => " until it resets".to_string(),
        };
        let help = if owner {
            " To answer before then, allow paid backups in Shodh (Settings → Model)."
        } else {
            ""
        };
        return format!("{quota} is used up, so I can't answer{until}.{help}");
    }
    model_failure_message(kind).to_string()
}

pub const NOT_LINKED: &str = "This is a private Shodh assistant. If its owner invited you, open the invite link they sent you.";
pub const GROUP_NOT_ALLOWED: &str =
    "Shodh answers in this group once its owner allows the group in the Shodh app.";
pub const NOT_SET_UP: &str = "Shodh isn't set up to answer here yet.";
pub const NOTHING_FOUND: &str =
    "I couldn't find anything about that in the documents Shodh answers from here.";
pub const SEARCH_FAILED: &str =
    "Shodh couldn't search its documents just now. Please try again later.";
pub const LIMIT_UNKNOWN: &str =
    "Shodh couldn't check today's answer limit, so it didn't answer. Please try again later.";
pub const EMPTY: &str =
    "Ask me a question in words, for example: “What does the report say about costs?”";
pub const FILES_ONLY: &str =
    "I can only read the text of a message, not photos or files. Ask your question in words.";
pub const NEW_CONVERSATION: &str = "Started a new conversation. What would you like to know?";
pub const OWNER_LINKED: &str = "Linked. This chat is your Shodh now: ask anything, and it answers from all your workspaces and memories, with sources. Send /new to start a fresh conversation. Shodh can't change anything from here; for that, open Shodh on your computer.";
pub const GUEST_LINKED: &str =
    "You're in. Ask a question and Shodh answers from the documents its owner shared with you.";
pub const ALREADY_LINKED: &str = "This Shodh is already linked to another Telegram account.";
pub const INVITE_FOR_OTHERS: &str =
    "That's an invite link for someone else. Your own chat with Shodh already works.";
pub const LINK_EXPIRED: &str =
    "That link has expired. Ask for a new one in Shodh (Connectors → Telegram).";
pub const LINK_UNKNOWN: &str =
    "That link doesn't work any more. Ask for a new one in Shodh (Connectors → Telegram).";
pub const OWNER_HELP: &str = "Ask anything: Shodh answers from all your workspaces and memories, with sources. Say “remember that …” to keep something. Send /new to start a fresh conversation. Shodh can't change files, tasks or settings from Telegram; open Shodh on your computer for that.";
pub const GUEST_HELP: &str =
    "Ask a question and Shodh answers from the documents its owner shared with you.";
pub const REMEMBERED: &str = "Remembered.";
pub const REMEMBER_FILED: &str =
    "I've kept that for your approval. Open Shodh to approve it, then I'll remember it.";
pub const ALREADY_REMEMBERED: &str = "I already remember that.";

/// How often the same person (or group) is sent a refusal at most.
pub const NOTE_EVERY: chrono::Duration = chrono::Duration::minutes(10);
/// "typing…" lasts about five seconds in Telegram; it is sent again this often.
pub const TYPING_EVERY: Duration = Duration::from_secs(4);
/// Message ids remembered for group threads (a reply to an older answer starts anew).
const THREAD_MEMORY: usize = 4_096;

/// What the owner asked to remember became.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remembered {
    Saved,
    /// Sensitive: waits for the owner's approval in the app.
    Filed,
    AlreadyKept,
}

/// Who an answer is for, decided by the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    Owner,
    Guest,
    Group,
}

/// The conversation an exchange belongs to, as the app keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRef {
    /// The chat key (letters, digits, `_`, `-`).
    pub key: String,
    /// What the sidebar groups it under ("Telegram · you").
    pub label: String,
}

/// A question as it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub author: String,
    /// Sent by the owner (their own words).
    pub owner: bool,
    pub text: String,
    pub at: DateTime<Utc>,
    /// The Telegram message id (unique in its chat).
    pub message_id: i64,
    /// The sender's Telegram user id.
    pub sender: i64,
}

/// What the connector needs from the app.
#[async_trait]
pub trait TelegramBrain: Send + Sync {
    /// The card's choices now.
    async fn rules(&self) -> Result<Rules, String>;
    /// Uses a link code.
    fn redeem(&self, code: &str) -> Result<LinkKind, LinkError>;
    /// Links `person` as the owner or as someone allowed.
    async fn link(&self, kind: LinkKind, person: &Person) -> Result<(), String>;
    /// The bot was added to (or removed from) group `chat` by `by`.
    async fn membership(&self, chat: &Chat, by: Option<&Person>, joined: bool);
    /// Group `from` is now supergroup `to`.
    async fn migrated(&self, from: i64, to: i64);
    /// The conversation of a private chat (until `/new`), or of a group thread starting
    /// at message `root`.
    async fn chat_ref(&self, audience: Audience, chat: &Chat, from: &Person, root: i64) -> ChatRef;
    /// `/new`: the private chat starts a new conversation.
    async fn new_conversation(&self, chat: i64);
    /// The workspace others are answered from (`None`: not chosen, or gone).
    async fn guest_scope(&self) -> Result<Option<AnswerScope>, String>;
    /// Passages for `query`: within `scope`, or the whole library with `None` (the owner).
    async fn retrieve(
        &self,
        scope: Option<&AnswerScope>,
        query: &str,
    ) -> Result<Vec<AnswerPassage>, String>;
    /// The owner's memories for `message`, as the app puts them before an Ask question.
    async fn memories(&self, message: &str) -> Option<String>;
    /// Saves what the owner asked to remember, as in Ask (`None`: it could not be filed).
    async fn remember(
        &self,
        request: &MemoryRequest,
        chat: &ChatRef,
        message_id: i64,
    ) -> Option<Remembered>;
    /// The earlier exchanges of the conversation, oldest first.
    async fn earlier(&self, chat: &ChatRef) -> Vec<ThreadTurn>;
    /// Keeps an exchange as the app's conversation of `chat`, answered for `audience`;
    /// `learn`: the owner's words may be learned from (the app learns only from the
    /// owner's, and gives the learner a group's answer, built on others' words, never).
    async fn keep(
        &self,
        chat: &ChatRef,
        audience: Audience,
        asked: &Asked,
        answer: &str,
        learn: bool,
    );
    /// [`Self::keep`], with the model that wrote the answer when the model fell back to
    /// another (the app's record says so; the chat is not told).
    #[allow(clippy::too_many_arguments)]
    async fn keep_answer(
        &self,
        chat: &ChatRef,
        audience: Audience,
        asked: &Asked,
        answer: &str,
        learn: bool,
        _answered_by: Option<AnsweredBy>,
    ) {
        self.keep(chat, audience, asked, answer, learn).await
    }
    /// The model the user chose (no tools).
    fn model(&self) -> Result<Arc<dyn TextModel>, String>;
    /// What never leaves this computer (home folder, the user's names).
    fn private_terms(&self) -> PrivateTerms;
}

/// How an update ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ignored,
    Linked(LinkKind),
    LinkRefused,
    Refused,
    GroupNotAllowed,
    Command,
    NotSetUp,
    Empty,
    AtCap,
    NothingFound,
    SearchFailed,
    LimitUnknown,
    Remembered(Remembered),
    Answered(Audience),
    ModelFailed(ProviderErrorKind),
    Cancelled,
    /// The owner was unlinked while their answer was written: not sent.
    Unlinked,
    /// Telegram refused a message after its retries.
    SendFailed(String),
    /// Sent more than a day before it arrived: noted (once per chat for a backlog), not
    /// answered.
    Stale,
    /// Small talk from someone other than the owner: a fixed reply, nothing searched.
    SmallTalk,
}

/// Group messages and the thread (the question message) each belongs to.
#[derive(Debug, Default)]
struct Threads {
    roots: HashMap<(i64, i64), i64>,
    order: VecDeque<(i64, i64)>,
}

impl Threads {
    fn root_of(&self, chat: i64, message: i64) -> Option<i64> {
        self.roots.get(&(chat, message)).copied()
    }

    fn remember(&mut self, chat: i64, message: i64, root: i64) {
        if self.roots.insert((chat, message), root).is_none() {
            self.order.push_back((chat, message));
        }
        while self.order.len() > THREAD_MEMORY {
            if let Some(old) = self.order.pop_front() {
                self.roots.remove(&old);
            }
        }
    }
}

/// Answers updates with a [`TelegramBrain`], sending with a [`BotApi`], counting others'
/// answers against an [`AnswerQuota`], until its [`CancellationToken`] is cancelled.
pub struct Responder {
    pub brain: Arc<dyn TelegramBrain>,
    pub bot: Arc<dyn BotApi>,
    pub quota: Arc<dyn AnswerQuota>,
    pub clock: Arc<dyn Clock>,
    pub cancel: CancellationToken,
    pub retry: RetryPolicy,
    pub typing_every: Duration,
    /// The time zone times are said in (the computer's; fixed in tests).
    pub offset_of: fn(DateTime<Utc>) -> FixedOffset,
    noted: Mutex<HashMap<i64, DateTime<Utc>>>,
    stale: StaleNotes,
    threads: Mutex<Threads>,
}

enum SendError {
    Cancelled,
    Telegram(TelegramError),
}

/// Stops the "typing…" loop when dropped.
struct Typing(CancellationToken);

impl Drop for Typing {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Responder {
    pub fn new(
        brain: Arc<dyn TelegramBrain>,
        bot: Arc<dyn BotApi>,
        quota: Arc<dyn AnswerQuota>,
        clock: Arc<dyn Clock>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            brain,
            bot,
            quota,
            clock,
            cancel,
            retry: RetryPolicy::default(),
            typing_every: TYPING_EVERY,
            offset_of: local_offset,
            noted: Mutex::new(HashMap::new()),
            stale: StaleNotes::default(),
            threads: Mutex::new(Threads::default()),
        }
    }

    /// Says times in the zone of `offset_of` instead of the computer's.
    pub fn with_offset_of(mut self, offset_of: fn(DateTime<Utc>) -> FixedOffset) -> Self {
        self.offset_of = offset_of;
        self
    }

    /// For a message sent more than a day ago: the note instead of an answer (sent once
    /// per chat for a backlog), as the outcome; `None` for any other message.
    async fn stale(&self, message: &Message, asked: &Asked) -> Option<Outcome> {
        let now = self.clock.now();
        if lateness(asked.at, now) != Lateness::Stale {
            return None;
        }
        if !self.stale.take(&message.chat.id.to_string(), now) {
            return Some(Outcome::Stale);
        }
        let note = stale_reply(asked.at, now, self.offset_of);
        Some(self.reply(message, &note, Outcome::Stale).await)
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    async fn pause(&self, delay: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(delay) => !self.cancelled(),
            _ = self.cancel.cancelled() => false,
        }
    }

    /// Sends one message, waiting out Telegram's rate limit and one transient failure.
    async fn send_one(
        &self,
        chat: i64,
        text: &str,
        html: bool,
        reply_to: Option<i64>,
    ) -> Result<i64, SendError> {
        let mut rate_limited = 0;
        let mut transient = 0;
        loop {
            if self.cancelled() {
                return Err(SendError::Cancelled);
            }
            let error = match self.bot.send(chat, text, html, reply_to).await {
                Ok(id) => return Ok(id),
                Err(error) => error,
            };
            let delay = match &error {
                TelegramError::RateLimited { retry_after }
                    if rate_limited < self.retry.rate_limit_retries =>
                {
                    if *retry_after > self.retry.max_retry_after {
                        return Err(SendError::Telegram(error));
                    }
                    rate_limited += 1;
                    *retry_after
                }
                e if e.is_transient() && transient < self.retry.transient_retries => {
                    transient += 1;
                    self.retry.transient_delay
                }
                _ => return Err(SendError::Telegram(error)),
            };
            tracing::info!(target: "shodh::telegram", error = %error, delay_ms = delay.as_millis() as u64, "Telegram did not take the message yet; trying again");
            if !self.pause(delay).await {
                return Err(SendError::Cancelled);
            }
        }
    }

    /// Sends `parts` in order (as plain text when Telegram cannot read a part's HTML);
    /// returns the sent message ids.
    async fn send_parts(
        &self,
        chat: i64,
        parts: &[Part],
        reply_to: Option<i64>,
    ) -> Result<Vec<i64>, SendError> {
        let mut sent = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            let reply = if i == 0 { reply_to } else { None };
            let id = match self.send_one(chat, &part.html, true, reply).await {
                Ok(id) => id,
                Err(SendError::Telegram(e)) if e.is_bad_markup() => {
                    tracing::warn!(target: "shodh::telegram", "Telegram could not read an answer's HTML; sent as plain text");
                    self.send_one(chat, &part.plain, false, reply).await?
                }
                Err(e) => return Err(e),
            };
            sent.push(id);
        }
        Ok(sent)
    }

    /// Sends a short note and reports `outcome` (or why it could not be sent).
    async fn reply(&self, message: &Message, text: &str, outcome: Outcome) -> Outcome {
        let reply_to = message.chat.kind.is_group().then_some(message.message_id);
        let part = note_part(text, &self.brain.private_terms());
        match self.send_parts(message.chat.id, &[part], reply_to).await {
            Ok(_) => outcome,
            Err(SendError::Cancelled) => Outcome::Cancelled,
            Err(SendError::Telegram(e)) => Outcome::SendFailed(e.to_string()),
        }
    }

    /// Whether `key` (a person, a group) may be sent a note now, and marks it sent.
    fn may_note(&self, key: i64) -> bool {
        let now = self.clock.now();
        let mut noted = self.noted.lock().unwrap_or_else(|e| e.into_inner());
        noted.retain(|_, at| now - *at < NOTE_EVERY);
        if noted.contains_key(&key) {
            return false;
        }
        noted.insert(key, now);
        true
    }

    /// Shows "typing…" in `chat` until the returned guard is dropped.
    fn typing(&self, chat: i64) -> Typing {
        let stop = self.cancel.child_token();
        let guard = Typing(stop.clone());
        let bot = self.bot.clone();
        let every = self.typing_every;
        tokio::spawn(async move {
            loop {
                if let Err(e) = bot.typing(chat).await {
                    tracing::debug!(target: "shodh::telegram", error = %e, "typing indicator not shown");
                }
                tokio::select! {
                    _ = tokio::time::sleep(every) => {}
                    _ = stop.cancelled() => return,
                }
            }
        });
        guard
    }

    /// Handles `update` (see the module docs for the order of the steps).
    pub async fn respond(&self, update: &Update) -> Outcome {
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        match update {
            Update::Other { .. } => Outcome::Ignored,
            Update::Membership {
                chat, by, joined, ..
            } => {
                if chat.kind.is_group() {
                    self.brain.membership(chat, by.as_ref(), *joined).await;
                }
                Outcome::Ignored
            }
            Update::Message(message) => self.message(message).await,
        }
    }

    async fn message(&self, message: &Message) -> Outcome {
        if let Some(to) = message.migrate_to {
            self.brain.migrated(message.chat.id, to).await;
            return Outcome::Ignored;
        }
        let rules = match self.brain.rules().await {
            Ok(rules) => rules,
            Err(e) => {
                tracing::warn!(target: "shodh::telegram", error = %e, "the Telegram settings could not be read; message left unanswered");
                return Outcome::Ignored;
            }
        };
        match route(message, &rules) {
            Route::Ignore(why) => {
                tracing::debug!(target: "shodh::telegram", why, "a Telegram update needs no answer");
                Outcome::Ignored
            }
            Route::Refuse => {
                let from = message.from.as_ref().map(|p| p.id).unwrap_or_default();
                if !self.may_note(from) {
                    return Outcome::Refused;
                }
                self.reply(message, NOT_LINKED, Outcome::Refused).await
            }
            Route::GroupNotAllowed => {
                if !self.may_note(message.chat.id) {
                    return Outcome::GroupNotAllowed;
                }
                self.reply(message, GROUP_NOT_ALLOWED, Outcome::GroupNotAllowed)
                    .await
            }
            Route::Link { code } => self.link(message, &code, &rules).await,
            Route::Owner => self.private(message, Audience::Owner).await,
            Route::Guest => self.private(message, Audience::Guest).await,
            Route::Group { question } => self.answer(message, Audience::Group, &question).await,
        }
    }

    async fn link(&self, message: &Message, code: &str, rules: &Rules) -> Outcome {
        let Some(person) = message.from.clone() else {
            return Outcome::Ignored;
        };
        let known = rules.owner == Some(person.id) || rules.people.contains(&person.id);
        match self.brain.redeem(code) {
            Ok(LinkKind::Owner) => {
                if rules.owner.is_some_and(|owner| owner != person.id) {
                    tracing::warn!(target: "shodh::telegram", "an owner link was used by another account than the linked one; refused");
                    return self
                        .reply(message, ALREADY_LINKED, Outcome::LinkRefused)
                        .await;
                }
                if let Err(e) = self.brain.link(LinkKind::Owner, &person).await {
                    tracing::warn!(target: "shodh::telegram", error = %e, "the owner could not be linked");
                    return self
                        .reply(message, LINK_UNKNOWN, Outcome::LinkRefused)
                        .await;
                }
                self.reply(message, OWNER_LINKED, Outcome::Linked(LinkKind::Owner))
                    .await
            }
            Ok(LinkKind::Invite) => {
                if rules.owner == Some(person.id) {
                    return self
                        .reply(message, INVITE_FOR_OTHERS, Outcome::LinkRefused)
                        .await;
                }
                if let Err(e) = self.brain.link(LinkKind::Invite, &person).await {
                    tracing::warn!(target: "shodh::telegram", error = %e, "an invited person could not be added");
                    return self
                        .reply(message, LINK_UNKNOWN, Outcome::LinkRefused)
                        .await;
                }
                self.reply(message, GUEST_LINKED, Outcome::Linked(LinkKind::Invite))
                    .await
            }
            // Someone already linked sending an old link: their chat works anyway.
            Err(_) if known => {
                let help = if rules.owner == Some(person.id) {
                    OWNER_HELP
                } else {
                    GUEST_HELP
                };
                self.reply(message, help, Outcome::Command).await
            }
            Err(error) => {
                if !self.may_note(person.id) {
                    return Outcome::LinkRefused;
                }
                let text = match error {
                    LinkError::Expired => LINK_EXPIRED,
                    LinkError::Unknown => LINK_UNKNOWN,
                };
                self.reply(message, text, Outcome::LinkRefused).await
            }
        }
    }

    /// A private chat with the owner or an allowed person: its commands, then the answer.
    async fn private(&self, message: &Message, audience: Audience) -> Outcome {
        let help = if audience == Audience::Owner {
            OWNER_HELP
        } else {
            GUEST_HELP
        };
        let bot = match self.brain.rules().await.ok().and_then(|r| r.bot) {
            Some(bot) => bot,
            None => return Outcome::Ignored,
        };
        let mut text = message.text.clone();
        if let Some(cmd) = super::updates::command(&message.text, &bot) {
            match cmd.name.as_str() {
                "new" => {
                    self.brain.new_conversation(message.chat.id).await;
                    return self
                        .reply(message, NEW_CONVERSATION, Outcome::Command)
                        .await;
                }
                "ask" => text = cmd.rest,
                _ => return self.reply(message, help, Outcome::Command).await,
            }
        }
        self.answer(message, audience, &text).await
    }

    /// The thread a group message belongs to: the question it continues (a reply to one
    /// of Shodh's answers), else itself.
    fn root(&self, message: &Message) -> i64 {
        if !message.chat.kind.is_group() {
            return 0;
        }
        let threads = self.threads.lock().unwrap_or_else(|e| e.into_inner());
        message
            .reply_to
            .as_ref()
            .and_then(|r| threads.root_of(message.chat.id, r.message_id))
            .unwrap_or(message.message_id)
    }

    async fn answer(&self, message: &Message, audience: Audience, text: &str) -> Outcome {
        let Some(from) = message.from.clone() else {
            return Outcome::Ignored;
        };
        if text.trim().is_empty() {
            let note = if message.has_media { FILES_ONLY } else { EMPTY };
            return self.reply(message, note, Outcome::Empty).await;
        }
        let root = self.root(message);
        let chat = self
            .brain
            .chat_ref(audience, &message.chat, &from, root)
            .await;
        let rules_owner = self.brain.rules().await.ok().and_then(|r| r.owner);
        let asked = Asked {
            author: from.name.clone(),
            owner: rules_owner == Some(from.id),
            text: text.trim().to_string(),
            at: message.date,
            message_id: message.message_id,
            sender: from.id,
        };
        match audience {
            Audience::Owner => self.owner_answer(message, &chat, &asked).await,
            Audience::Guest | Audience::Group => {
                self.guest_answer(message, audience, &chat, &asked).await
            }
        }
    }

    async fn owner_answer(&self, message: &Message, chat: &ChatRef, asked: &Asked) -> Outcome {
        // "Remember that …": saved now, before the model, as in Ask.
        let request = memory_request(&asked.text);
        let handled = match &request {
            Some(request) => self
                .brain
                .remember(request, chat, message.message_id)
                .await
                .map(|r| (request, r)),
            None => None,
        };
        if let Some((request, remembered)) = &handled {
            if request.is_all_of_the_message() {
                let text = match remembered {
                    Remembered::Saved => REMEMBERED,
                    Remembered::Filed => REMEMBER_FILED,
                    Remembered::AlreadyKept => ALREADY_REMEMBERED,
                };
                let outcome = self
                    .reply(message, text, Outcome::Remembered(*remembered))
                    .await;
                if matches!(outcome, Outcome::Remembered(_)) && !self.cancelled() {
                    self.brain
                        .keep(chat, Audience::Owner, asked, text, false)
                        .await;
                }
                return outcome;
            }
        }
        // What they asked to remember was saved above; the rest is not answered a day late.
        if let Some(outcome) = self.stale(message, asked).await {
            return outcome;
        }
        if handled.is_none() && is_small_talk(&asked.text) {
            return self.owner_small_talk(message, chat, asked).await;
        }
        let note = handled.as_ref().map(|(request, remembered)| {
            let note = request.note.split_whitespace().collect::<Vec<_>>().join(" ");
            match remembered {
                Remembered::Saved => format!(
                    "Shodh has saved what they asked you to remember (“{note}”). Confirm it in one short sentence, then answer the rest of the message."
                ),
                Remembered::Filed => format!(
                    "What they asked you to remember (“{note}”) waits for their approval in Shodh. Say in one short sentence: open Shodh to approve it. Then answer the rest of the message."
                ),
                Remembered::AlreadyKept => format!(
                    "What they asked you to remember (“{note}”) is remembered already. Acknowledge it briefly, then answer the rest of the message."
                ),
            }
        });
        let _typing = self.typing(message.chat.id);
        let earlier = self.brain.earlier(chat).await;
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let passages = self
            .brain
            .retrieve(None, &owner_query(&asked.text, &earlier))
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(target: "shodh::telegram", error = %e, "the library search for the owner failed; answering without passages");
                Vec::new()
            });
        let passages: Vec<AnswerPassage> = passages.into_iter().take(OWNER_PASSAGES).collect();
        let memories = self.brain.memories(&asked.text).await;
        let private = self.brain.private_terms();
        let prompt = owner_prompt(
            &asked.text,
            &passages,
            memories.as_deref(),
            &earlier,
            note.as_deref(),
            &private,
        );
        // A message whose request to remember was handled is not learned from again.
        let learn = handled.is_none();
        self.complete_and_send(
            message,
            chat,
            asked,
            &prompt,
            OWNER_ANSWER_TOKENS,
            &passages,
            learn,
            Audience::Owner,
            None,
        )
        .await
    }

    /// Small talk with the owner: one short model call, with the chat so far, and no
    /// library search or memories.
    async fn owner_small_talk(&self, message: &Message, chat: &ChatRef, asked: &Asked) -> Outcome {
        let _typing = self.typing(message.chat.id);
        let earlier = self.brain.earlier(chat).await;
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let prompt = owner_small_talk_prompt(&asked.text, &earlier);
        self.complete_and_send(
            message,
            chat,
            asked,
            &prompt,
            OWNER_SMALL_TALK_TOKENS,
            &[],
            false,
            Audience::Owner,
            None,
        )
        .await
    }

    async fn guest_answer(
        &self,
        message: &Message,
        audience: Audience,
        chat: &ChatRef,
        asked: &Asked,
    ) -> Outcome {
        let scope = match self.brain.guest_scope().await {
            Ok(Some(scope)) => scope,
            Ok(None) => return self.reply(message, NOT_SET_UP, Outcome::NotSetUp).await,
            Err(e) => {
                tracing::warn!(target: "shodh::telegram", error = %e, "the workspace Telegram answers from could not be read");
                return self.reply(message, NOT_SET_UP, Outcome::NotSetUp).await;
            }
        };
        if let Some(outcome) = self.stale(message, asked).await {
            return outcome;
        }
        // No search and no model call (nor one of the day's answers) for a greeting.
        if is_small_talk(&asked.text) {
            return self
                .reply(message, SMALL_TALK_REPLY, Outcome::SmallTalk)
                .await;
        }
        let now = self.clock.now();
        match self.quota_used(utc_day(now)).await {
            Ok(used) if used >= self.quota.cap() => {
                return self
                    .reply(
                        message,
                        &at_cap_message(next_utc_midnight(now)),
                        Outcome::AtCap,
                    )
                    .await
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(target: "shodh::telegram", error = %e, "the Telegram answer count could not be read");
                return self
                    .reply(message, LIMIT_UNKNOWN, Outcome::LimitUnknown)
                    .await;
            }
        }
        let _typing = self.typing(message.chat.id);
        let earlier = self.brain.earlier(chat).await;
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let passages = match self
            .brain
            .retrieve(Some(&scope), &search_query(&asked.text, &earlier))
            .await
        {
            Ok(passages) => passages,
            Err(e) => {
                tracing::warn!(target: "shodh::telegram", error = %e, "search for a Telegram question failed");
                return self
                    .reply(message, SEARCH_FAILED, Outcome::SearchFailed)
                    .await;
            }
        };
        if passages.is_empty() {
            return self
                .reply(message, NOTHING_FOUND, Outcome::NothingFound)
                .await;
        }
        let prompt = answer_prompt_in(
            &TELEGRAM,
            &passages,
            &asked.text,
            &earlier,
            &self.brain.private_terms(),
        );
        // The call counts on the UTC day it is made.
        let day = utc_day(self.clock.now());
        match self.quota_reserve(day).await {
            Ok(true) => {}
            Ok(false) => {
                return self
                    .reply(
                        message,
                        &at_cap_message(next_utc_midnight(self.clock.now())),
                        Outcome::AtCap,
                    )
                    .await
            }
            Err(e) => {
                tracing::warn!(target: "shodh::telegram", error = %e, "a Telegram answer could not be counted");
                return self
                    .reply(message, LIMIT_UNKNOWN, Outcome::LimitUnknown)
                    .await;
            }
        }
        // The owner's words in a group are still theirs; anyone else's never reach learning
        // (the app checks `asked.owner`).
        self.complete_and_send(
            message,
            chat,
            asked,
            &prompt,
            MAX_ANSWER_TOKENS,
            &passages,
            true,
            audience,
            Some(day),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn complete_and_send(
        &self,
        message: &Message,
        chat: &ChatRef,
        asked: &Asked,
        prompt: &str,
        max_tokens: usize,
        passages: &[AnswerPassage],
        learn: bool,
        audience: Audience,
        reserved: Option<chrono::NaiveDate>,
    ) -> Outcome {
        let model = match self.brain.model() {
            Ok(model) => model,
            Err(e) => {
                tracing::warn!(target: "shodh::telegram", error = %e, "no model for a Telegram answer");
                if let Some(day) = reserved {
                    self.quota_release(day).await;
                }
                let kind = ProviderErrorKind::ModelUnavailable;
                return self
                    .reply(
                        message,
                        model_failure_message(kind),
                        Outcome::ModelFailed(kind),
                    )
                    .await;
            }
        };
        if self.cancelled() {
            if let Some(day) = reserved {
                self.quota_release(day).await;
            }
            return Outcome::Cancelled;
        }
        let completion = tokio::select! {
            completion = model.complete(prompt, max_tokens) => completion,
            // The call was made: it stays counted.
            _ = self.cancel.cancelled() => return Outcome::Cancelled,
        };
        let private = self.brain.private_terms();
        let text = match completion {
            // A message that waited while Shodh was off says when it was sent.
            Ok(text) if !text.trim().is_empty() => {
                with_late_line(&text, asked.at, self.clock.now(), self.offset_of)
            }
            Ok(_) => {
                let kind = ProviderErrorKind::EmptyResponse;
                return self
                    .reply(
                        message,
                        model_failure_message(kind),
                        Outcome::ModelFailed(kind),
                    )
                    .await;
            }
            Err(e) => {
                let now_ms = u64::try_from(self.clock.now().timestamp_millis()).unwrap_or(0);
                let kind = classify(&e, None, now_ms).kind;
                tracing::warn!(target: "shodh::telegram", kind = ?kind, error = %sanitize(&e, &private), "the model call for a Telegram answer failed");
                let text = model_failure_text(kind, &e, self.clock.now(), self.offset_of, audience);
                return self.reply(message, &text, Outcome::ModelFailed(kind)).await;
            }
        };
        // The owner's own chat is their data on their phone: nothing is redacted there.
        // Everyone else's answers are sanitized.
        let parts = match audience {
            Audience::Owner => owner_parts(&text, passages),
            Audience::Guest | Audience::Group => answer_parts(&text, passages, &private),
        };
        // An owner unlinked while the model wrote: their former account gets nothing.
        if audience == Audience::Owner {
            let owner = self.brain.rules().await.ok().and_then(|r| r.owner);
            if owner != Some(asked.sender) {
                tracing::info!(target: "shodh::telegram", "the owner was unlinked while an answer was written; not sent");
                return Outcome::Unlinked;
            }
        }
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let reply_to = message.chat.kind.is_group().then_some(message.message_id);
        match self.send_parts(message.chat.id, &parts, reply_to).await {
            Ok(sent) => {
                if message.chat.kind.is_group() {
                    let root = self.root(message);
                    let mut threads = self.threads.lock().unwrap_or_else(|e| e.into_inner());
                    threads.remember(message.chat.id, message.message_id, root);
                    for id in sent {
                        threads.remember(message.chat.id, id, root);
                    }
                }
                let kept = parts
                    .iter()
                    .map(|p| p.plain.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                // Stopped meanwhile (Disconnect with forget): nothing is kept after it.
                if self.cancelled() {
                    return Outcome::Cancelled;
                }
                self.brain
                    .keep_answer(chat, audience, asked, &kept, learn, model.answered_by())
                    .await;
                Outcome::Answered(audience)
            }
            Err(SendError::Cancelled) => Outcome::Cancelled,
            Err(SendError::Telegram(e)) => Outcome::SendFailed(e.to_string()),
        }
    }

    async fn quota_used(&self, day: chrono::NaiveDate) -> Result<usize, String> {
        let quota = self.quota.clone();
        tokio::task::spawn_blocking(move || quota.used(day))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn quota_reserve(&self, day: chrono::NaiveDate) -> Result<bool, String> {
        let quota = self.quota.clone();
        tokio::task::spawn_blocking(move || quota.try_reserve(day))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn quota_release(&self, day: chrono::NaiveDate) {
        let quota = self.quota.clone();
        let released = tokio::task::spawn_blocking(move || quota.release(day))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
        if let Err(e) = released {
            tracing::warn!(target: "shodh::telegram", error = %e, "an unused Telegram answer could not be given back");
        }
    }
}

/// Where outcomes go (the card, notifications).
pub trait OutcomeSink: Send + Sync {
    fn handled(&self, outcome: &Outcome);
}

/// Updates of people other than the owner taken from Telegram and not finished yet. When
/// they are all taken, polling waits (and confirms nothing more), so Telegram keeps the
/// rest: nothing is ever dropped.
pub const MAX_WAITING: usize = 64;
/// Answers to others worked on at once.
pub const MAX_RUNNING: usize = 2;

/// Counts an update from when it is taken until it is finished, a panic included (the
/// count goes down when this is dropped).
struct Pending {
    tasks: Arc<AtomicUsize>,
    idle: Arc<tokio::sync::Notify>,
}

impl Pending {
    fn new(tasks: Arc<AtomicUsize>, idle: Arc<tokio::sync::Notify>) -> Self {
        tasks.fetch_add(1, Ordering::SeqCst);
        Self { tasks, idle }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if self.tasks.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.idle.notify_waiters();
        }
    }
}

/// Hands updates to the [`Responder`]: one chat's messages are answered in order. The
/// owner's private chat has its own lane (never behind others' questions, never waiting
/// for room); others get at most [`MAX_RUNNING`] answers at once and [`MAX_WAITING`]
/// updates taken, after which polling waits.
pub struct Dispatcher {
    responder: Arc<Responder>,
    runtime: tokio::runtime::Handle,
    sink: Arc<dyn OutcomeSink>,
    chats: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
    room: Arc<tokio::sync::Semaphore>,
    running: Arc<tokio::sync::Semaphore>,
    owner_running: Arc<tokio::sync::Semaphore>,
    tasks: Arc<AtomicUsize>,
    idle: Arc<tokio::sync::Notify>,
}

/// Room taken for one update ([`Dispatcher::room`]); given back when it is finished.
pub struct Room {
    /// `None`: the owner's lane, which needs no room.
    _held: Option<tokio::sync::OwnedSemaphorePermit>,
    owner: bool,
}

impl Room {
    /// Room that holds nothing (a handler with no limit).
    pub fn unlimited() -> Self {
        Self {
            _held: None,
            owner: false,
        }
    }
}

impl Dispatcher {
    pub fn new(
        responder: Arc<Responder>,
        runtime: tokio::runtime::Handle,
        sink: Arc<dyn OutcomeSink>,
    ) -> Self {
        Self::with_room(responder, runtime, sink, MAX_WAITING)
    }

    /// A dispatcher with room for `waiting` updates of others (tests use a small one).
    pub fn with_room(
        responder: Arc<Responder>,
        runtime: tokio::runtime::Handle,
        sink: Arc<dyn OutcomeSink>,
        waiting: usize,
    ) -> Self {
        Self {
            responder,
            runtime,
            sink,
            chats: Mutex::new(HashMap::new()),
            room: Arc::new(tokio::sync::Semaphore::new(waiting.max(1))),
            running: Arc::new(tokio::sync::Semaphore::new(MAX_RUNNING)),
            owner_running: Arc::new(tokio::sync::Semaphore::new(1)),
            tasks: Arc::new(AtomicUsize::new(0)),
            idle: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn chat_of(update: &Update) -> i64 {
        match update {
            Update::Message(m) => m.chat.id,
            Update::Membership { chat, .. } => chat.id,
            Update::Other { .. } => 0,
        }
    }

    /// Whether `update` is a message in the owner's private chat.
    async fn is_owner(&self, update: &Update) -> bool {
        let Update::Message(message) = update else {
            return false;
        };
        if message.chat.kind != super::updates::ChatKind::Private {
            return false;
        }
        match self.responder.brain.rules().await {
            Ok(rules) => rules.owner == Some(message.chat.id),
            Err(_) => false,
        }
    }

    /// Room for `update`: at once for the owner's chat, else when one of [`MAX_WAITING`]
    /// places is free. `None` when `stop` came first: the update was not taken.
    pub async fn room(&self, update: &Update, stop: &CancellationToken) -> Option<Room> {
        if self.is_owner(update).await {
            return Some(Room {
                _held: None,
                owner: true,
            });
        }
        tokio::select! {
            permit = self.room.clone().acquire_owned() => permit.ok().map(|p| Room {
                _held: Some(p),
                owner: false,
            }),
            _ = stop.cancelled() => None,
        }
    }

    /// Starts handling `update` in the background, in the room taken for it.
    pub fn start(&self, update: Update, room: Room) {
        if matches!(update, Update::Other { .. }) {
            return;
        }
        let pending = Pending::new(self.tasks.clone(), self.idle.clone());
        let chat = Self::chat_of(&update);
        let lock = self
            .chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(chat)
            .or_default()
            .clone();
        let responder = self.responder.clone();
        let running = if room.owner {
            self.owner_running.clone()
        } else {
            self.running.clone()
        };
        let sink = self.sink.clone();
        self.runtime.spawn(async move {
            // Released when this ends, however it ends.
            let _room = room;
            let _pending = pending;
            let _in_order = lock.lock().await;
            let outcome = match running.acquire().await {
                Ok(_permit) => responder.respond(&update).await,
                Err(_) => Outcome::Cancelled,
            };
            sink.handled(&outcome);
        });
    }

    /// Updates taken and not finished now.
    pub fn pending(&self) -> usize {
        self.tasks.load(Ordering::SeqCst)
    }

    /// Waits until every update taken is finished (after the connector was stopped, so
    /// nothing is kept after a Disconnect forgets the conversations).
    pub async fn drained(&self) {
        loop {
            let notified = self.idle.notified();
            if self.pending() == 0 {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex as StdMutex;

    use chrono::TimeZone;
    use serde_json::Value;

    use super::*;
    use crate::connectors::slack::respond::testkit::{FixedClock, MemoryQuota, RecordingModel};
    use crate::connectors::telegram::updates::tests::{bot as bot_self, message};
    use crate::connectors::telegram::updates::ReplyTo;

    const OWNER: i64 = 42;
    const GUEST: i64 = 77;
    const GROUP: i64 = -100_200;

    /// A Telegram that records what was sent.
    #[derive(Default)]
    pub struct FakeBot {
        pub sent: StdMutex<Vec<(i64, String, bool, Option<i64>)>>,
        pub typing: StdMutex<usize>,
        /// Errors the next sends get, in order.
        pub failures: StdMutex<VecDeque<TelegramError>>,
        /// Cancelled right after the next message is sent (a Disconnect at that moment).
        pub cancel_after_send: StdMutex<Option<CancellationToken>>,
    }

    impl FakeBot {
        pub fn texts(&self) -> Vec<String> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.1.clone())
                .collect()
        }
    }

    #[async_trait]
    impl BotApi for FakeBot {
        async fn updates(
            &self,
            _offset: Option<i64>,
            _wait: u32,
        ) -> Result<Vec<Value>, TelegramError> {
            Ok(Vec::new())
        }
        async fn send(
            &self,
            chat: i64,
            text: &str,
            html: bool,
            reply_to: Option<i64>,
        ) -> Result<i64, TelegramError> {
            if let Some(error) = self.failures.lock().unwrap().pop_front() {
                return Err(error);
            }
            let id = {
                let mut sent = self.sent.lock().unwrap();
                sent.push((chat, text.to_string(), html, reply_to));
                1_000 + sent.len() as i64
            };
            if let Some(token) = self.cancel_after_send.lock().unwrap().take() {
                token.cancel();
            }
            Ok(id)
        }
        async fn typing(&self, _chat: i64) -> Result<(), TelegramError> {
            *self.typing.lock().unwrap() += 1;
            Ok(())
        }
        async fn webhook_set(&self) -> Result<bool, TelegramError> {
            Ok(false)
        }
    }

    /// The app, in memory.
    pub struct FakeBrain {
        pub rules: Arc<StdMutex<Rules>>,
        pub codes: StdMutex<HashMap<String, Result<LinkKind, LinkError>>>,
        pub linked: StdMutex<Vec<(LinkKind, i64)>>,
        pub scope: Option<AnswerScope>,
        pub passages: Vec<AnswerPassage>,
        pub retrieved_with: StdMutex<Vec<Option<AnswerScope>>>,
        pub memories_asked: StdMutex<usize>,
        pub remembered: StdMutex<Option<Remembered>>,
        pub kept: StdMutex<Vec<(ChatRef, Asked, String, bool, Audience)>>,
        pub model: Arc<RecordingModel>,
        /// Used instead of `model` when set.
        pub other_model: StdMutex<Option<Arc<dyn TextModel>>>,
        pub memberships: StdMutex<Vec<(i64, bool)>>,
        pub new_conversations: StdMutex<Vec<i64>>,
        /// What `keep_answer` was told about the model that answered.
        pub answered_by: StdMutex<Vec<Option<AnsweredBy>>>,
    }

    impl FakeBrain {
        pub fn new(model: RecordingModel) -> Self {
            Self {
                rules: Arc::new(StdMutex::new(Rules {
                    bot: Some(bot_self()),
                    owner: Some(OWNER),
                    people: vec![GUEST],
                    groups: vec![GROUP],
                })),
                codes: StdMutex::new(HashMap::new()),
                linked: StdMutex::new(Vec::new()),
                scope: Some(AnswerScope {
                    source_ids: vec!["space-a".into()],
                    ..AnswerScope::default()
                }),
                passages: vec![AnswerPassage {
                    n: 1,
                    title: "Office lease".into(),
                    page: Some("3".into()),
                    text: "The notice period is sixty days.".into(),
                }],
                retrieved_with: StdMutex::new(Vec::new()),
                memories_asked: StdMutex::new(0),
                remembered: StdMutex::new(Some(Remembered::Saved)),
                kept: StdMutex::new(Vec::new()),
                model: Arc::new(model),
                other_model: StdMutex::new(None),
                memberships: StdMutex::new(Vec::new()),
                new_conversations: StdMutex::new(Vec::new()),
                answered_by: StdMutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl TelegramBrain for FakeBrain {
        async fn rules(&self) -> Result<Rules, String> {
            Ok(self.rules.lock().unwrap().clone())
        }
        fn redeem(&self, code: &str) -> Result<LinkKind, LinkError> {
            self.codes
                .lock()
                .unwrap()
                .remove(code)
                .unwrap_or(Err(LinkError::Unknown))
        }
        async fn link(&self, kind: LinkKind, person: &Person) -> Result<(), String> {
            self.linked.lock().unwrap().push((kind, person.id));
            let mut rules = self.rules.lock().unwrap();
            match kind {
                LinkKind::Owner => rules.owner = Some(person.id),
                LinkKind::Invite => rules.people.push(person.id),
            }
            Ok(())
        }
        async fn membership(&self, chat: &Chat, _by: Option<&Person>, joined: bool) {
            self.memberships.lock().unwrap().push((chat.id, joined));
        }
        async fn migrated(&self, _from: i64, _to: i64) {}
        async fn chat_ref(
            &self,
            audience: Audience,
            chat: &Chat,
            _from: &Person,
            root: i64,
        ) -> ChatRef {
            ChatRef {
                key: format!("{}-{root}", chat.id),
                label: format!("{audience:?}"),
            }
        }
        async fn new_conversation(&self, chat: i64) {
            self.new_conversations.lock().unwrap().push(chat);
        }
        async fn guest_scope(&self) -> Result<Option<AnswerScope>, String> {
            Ok(self.scope.clone())
        }
        async fn retrieve(
            &self,
            scope: Option<&AnswerScope>,
            _query: &str,
        ) -> Result<Vec<AnswerPassage>, String> {
            self.retrieved_with.lock().unwrap().push(scope.cloned());
            Ok(self.passages.clone())
        }
        async fn memories(&self, _message: &str) -> Option<String> {
            *self.memories_asked.lock().unwrap() += 1;
            Some("<memories>\n- The owner lives in Pune\n</memories>".into())
        }
        async fn remember(
            &self,
            _request: &MemoryRequest,
            _chat: &ChatRef,
            _message_id: i64,
        ) -> Option<Remembered> {
            *self.remembered.lock().unwrap()
        }
        async fn earlier(&self, _chat: &ChatRef) -> Vec<ThreadTurn> {
            Vec::new()
        }
        async fn keep(
            &self,
            chat: &ChatRef,
            audience: Audience,
            asked: &Asked,
            answer: &str,
            learn: bool,
        ) {
            self.kept.lock().unwrap().push((
                chat.clone(),
                asked.clone(),
                answer.to_string(),
                learn,
                audience,
            ));
        }
        async fn keep_answer(
            &self,
            chat: &ChatRef,
            audience: Audience,
            asked: &Asked,
            answer: &str,
            learn: bool,
            answered_by: Option<AnsweredBy>,
        ) {
            self.answered_by.lock().unwrap().push(answered_by);
            self.keep(chat, audience, asked, answer, learn).await
        }
        fn model(&self) -> Result<Arc<dyn TextModel>, String> {
            if let Some(model) = self.other_model.lock().unwrap().clone() {
                return Ok(model);
            }
            Ok(self.model.clone())
        }
        fn private_terms(&self) -> PrivateTerms {
            PrivateTerms::default()
        }
    }

    pub struct Setup {
        pub bot: Arc<FakeBot>,
        pub brain: Arc<FakeBrain>,
        pub quota: Arc<MemoryQuota>,
        pub responder: Responder,
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 27, 9, 0, 0).unwrap()
    }

    pub fn setup(model: RecordingModel, cap: usize) -> Setup {
        let bot = Arc::new(FakeBot::default());
        let brain = Arc::new(FakeBrain::new(model));
        let quota = Arc::new(MemoryQuota::new(cap));
        let responder = Responder::new(
            brain.clone(),
            bot.clone(),
            quota.clone(),
            Arc::new(FixedClock(now())),
            CancellationToken::new(),
        )
        .with_retry(RetryPolicy {
            rate_limit_retries: 3,
            max_retry_after: Duration::from_secs(30),
            default_retry_after: Duration::from_millis(1),
            transient_retries: 1,
            transient_delay: Duration::from_millis(1),
        });
        Setup {
            bot,
            brain,
            quota,
            responder,
        }
    }

    fn update(m: Message) -> Update {
        Update::Message(Box::new(m))
    }

    #[tokio::test]
    async fn an_owner_code_links_once_and_never_from_another_account() {
        let s = setup(RecordingModel::answering("x"), 5);
        s.brain.rules.lock().unwrap().owner = None;
        s.brain
            .codes
            .lock()
            .unwrap()
            .insert("CODE1".into(), Ok(LinkKind::Owner));
        let outcome = s
            .responder
            .respond(&update(message(1, OWNER, OWNER, "/start CODE1")))
            .await;
        assert_eq!(outcome, Outcome::Linked(LinkKind::Owner));
        assert_eq!(
            *s.brain.linked.lock().unwrap(),
            vec![(LinkKind::Owner, OWNER)]
        );
        assert_eq!(s.bot.texts(), vec![OWNER_LINKED.to_string()]);
        // Used again: the owner's chat simply works.
        let again = s
            .responder
            .respond(&update(message(2, OWNER, OWNER, "/start CODE1")))
            .await;
        assert_eq!(again, Outcome::Command);
        // Another account with an owner code while one is linked: refused.
        s.brain
            .codes
            .lock()
            .unwrap()
            .insert("CODE2".into(), Ok(LinkKind::Owner));
        let other = s
            .responder
            .respond(&update(message(3, 9, 9, "/start CODE2")))
            .await;
        assert_eq!(other, Outcome::LinkRefused);
        assert_eq!(s.brain.linked.lock().unwrap().len(), 1);
        // An expired code says so.
        s.brain
            .codes
            .lock()
            .unwrap()
            .insert("OLD".into(), Err(LinkError::Expired));
        s.responder
            .respond(&update(message(4, 10, 10, "/start OLD")))
            .await;
        assert_eq!(s.bot.texts().last().unwrap(), LINK_EXPIRED);
        assert_eq!(s.brain.model.calls(), 0);
    }

    #[tokio::test]
    async fn an_invite_adds_a_guest_but_never_the_owner() {
        let s = setup(RecordingModel::answering("x"), 5);
        s.brain
            .codes
            .lock()
            .unwrap()
            .insert("INV".into(), Ok(LinkKind::Invite));
        assert_eq!(
            s.responder
                .respond(&update(message(1, OWNER, OWNER, "/start INV")))
                .await,
            Outcome::LinkRefused
        );
        s.brain
            .codes
            .lock()
            .unwrap()
            .insert("INV2".into(), Ok(LinkKind::Invite));
        assert_eq!(
            s.responder
                .respond(&update(message(2, 55, 55, "/start INV2")))
                .await,
            Outcome::Linked(LinkKind::Invite)
        );
        assert!(s.brain.rules.lock().unwrap().people.contains(&55));
        assert_eq!(s.brain.rules.lock().unwrap().owner, Some(OWNER));
    }

    #[tokio::test]
    async fn someone_unlinked_is_refused_once_in_a_while_with_no_model_call() {
        let s = setup(RecordingModel::answering("x"), 5);
        for i in 0..3 {
            assert_eq!(
                s.responder
                    .respond(&update(message(i, 9, 9, "what is the rent?")))
                    .await,
                Outcome::Refused
            );
        }
        assert_eq!(
            s.bot.texts(),
            vec![NOT_LINKED.to_string()],
            "one refusal, not a flood"
        );
        assert_eq!(s.brain.model.calls(), 0);
        assert!(s.brain.kept.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_owner_gets_the_whole_library_and_memories() {
        let s = setup(RecordingModel::answering("Sixty days [1]."), 5);
        let outcome = s
            .responder
            .respond(&update(message(
                1,
                OWNER,
                OWNER,
                "What is the notice period?",
            )))
            .await;
        assert_eq!(outcome, Outcome::Answered(Audience::Owner));
        assert_eq!(
            *s.brain.retrieved_with.lock().unwrap(),
            vec![None],
            "the whole library, not one workspace"
        );
        assert_eq!(*s.brain.memories_asked.lock().unwrap(), 1);
        let prompt = s.brain.model.prompts.lock().unwrap()[0].clone();
        assert!(prompt.contains("The owner lives in Pune"));
        assert!(prompt.contains("no tools"));
        let sent = s.bot.sent.lock().unwrap()[0].clone();
        assert!(sent.1.contains("Sixty days [1]."));
        assert!(sent.1.contains("<b>Sources</b>"));
        assert!(sent.2, "sent as HTML");
        assert_eq!(sent.3, None, "no reply in a private chat");
        // Kept as the owner's own words, which the app learns from.
        let kept = s.brain.kept.lock().unwrap()[0].clone();
        assert!(kept.1.owner);
        assert!(kept.3, "learnable");
        assert_eq!(
            s.quota.used(utc_day(now())).unwrap(),
            0,
            "the owner has no cap"
        );
    }

    #[tokio::test]
    async fn others_never_get_memories_and_count_against_the_cap() {
        let s = setup(RecordingModel::answering("Sixty days [1]."), 1);
        let outcome = s
            .responder
            .respond(&update(message(
                1,
                GUEST,
                GUEST,
                "What is the notice period?",
            )))
            .await;
        assert_eq!(outcome, Outcome::Answered(Audience::Guest));
        assert_eq!(*s.brain.memories_asked.lock().unwrap(), 0, "never memories");
        let prompt = s.brain.model.prompts.lock().unwrap()[0].clone();
        assert!(!prompt.contains("Pune"));
        assert!(prompt.starts_with("You answer one question asked in Telegram"));
        assert_eq!(
            s.brain.retrieved_with.lock().unwrap()[0],
            s.brain.scope,
            "the chosen workspace only"
        );
        let kept = s.brain.kept.lock().unwrap()[0].clone();
        assert!(!kept.1.owner, "someone else's words");
        assert_eq!(kept.1.author, "Ann Lee");
        // At the cap: no model call.
        let capped = s
            .responder
            .respond(&update(message(2, GUEST, GUEST, "And the deposit?")))
            .await;
        assert_eq!(capped, Outcome::AtCap);
        assert_eq!(s.brain.model.calls(), 1);
    }

    #[tokio::test]
    async fn in_a_group_only_a_mention_or_reply_is_answered_and_as_the_group() {
        let s = setup(RecordingModel::answering("Sixty days [1]."), 5);
        assert_eq!(
            s.responder
                .respond(&update(message(1, GROUP, OWNER, "what is the rent?")))
                .await,
            Outcome::Ignored
        );
        // The owner asking in a group gets the group's answer: no memories, the workspace.
        let asked = message(2, GROUP, OWNER, "/ask@shodh_notes_bot what is the rent?");
        let outcome = s.responder.respond(&update(asked.clone())).await;
        assert_eq!(outcome, Outcome::Answered(Audience::Group));
        assert_eq!(*s.brain.memories_asked.lock().unwrap(), 0);
        let sent = s.bot.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.3, Some(asked.message_id), "replies to the question");
        // A reply to Shodh's answer continues that thread.
        let answer_id = 1_001;
        let mut follow = message(3, GROUP, GUEST, "and the deposit?");
        follow.reply_to = Some(ReplyTo {
            message_id: answer_id,
            from: Some(bot_self().id),
        });
        s.responder.respond(&update(follow)).await;
        let kept = s.brain.kept.lock().unwrap().clone();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].0.key, kept[1].0.key, "one thread, one conversation");
        // A group not allowed: a note, once.
        s.responder
            .respond(&update(message(4, -9, GUEST, "@shodh_notes_bot hi")))
            .await;
        let mut mention = message(5, -9, GUEST, "@shodh_notes_bot hi");
        mention.mentions = vec![crate::connectors::telegram::updates::Mention::Username(
            "shodh_notes_bot".into(),
        )];
        assert_eq!(
            s.responder.respond(&update(mention.clone())).await,
            Outcome::GroupNotAllowed
        );
        assert_eq!(
            s.responder.respond(&update(mention)).await,
            Outcome::GroupNotAllowed
        );
        assert_eq!(
            s.bot
                .texts()
                .iter()
                .filter(|t| t.as_str() == GROUP_NOT_ALLOWED)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn a_request_to_remember_is_saved_without_a_model_call() {
        let s = setup(RecordingModel::answering("x"), 5);
        let outcome = s
            .responder
            .respond(&update(message(
                1,
                OWNER,
                OWNER,
                "Remember that my thesis is on vector search",
            )))
            .await;
        assert_eq!(outcome, Outcome::Remembered(Remembered::Saved));
        assert_eq!(s.bot.texts(), vec![REMEMBERED.to_string()]);
        assert_eq!(s.brain.model.calls(), 0);
        let kept = s.brain.kept.lock().unwrap()[0].clone();
        assert!(!kept.3, "a handled request is not learned again");
    }

    #[tokio::test]
    async fn a_sensitive_request_asks_to_open_shodh_to_approve() {
        let s = setup(RecordingModel::answering("x"), 5);
        *s.brain.remembered.lock().unwrap() = Some(Remembered::Filed);
        s.responder
            .respond(&update(message(
                1,
                OWNER,
                OWNER,
                "Remember that my thesis is on vector search",
            )))
            .await;
        assert_eq!(s.bot.texts(), vec![REMEMBER_FILED.to_string()]);
        assert!(REMEMBER_FILED.contains("Open Shodh to approve"));
    }

    #[tokio::test]
    async fn new_starts_a_fresh_conversation() {
        let s = setup(RecordingModel::answering("x"), 5);
        assert_eq!(
            s.responder
                .respond(&update(message(1, OWNER, OWNER, "/new")))
                .await,
            Outcome::Command
        );
        assert_eq!(*s.brain.new_conversations.lock().unwrap(), vec![OWNER]);
        assert_eq!(s.bot.texts(), vec![NEW_CONVERSATION.to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limited_message_waits_for_telegrams_retry_after() {
        let s = setup(RecordingModel::answering("Sixty days [1]."), 5);
        s.bot
            .failures
            .lock()
            .unwrap()
            .push_back(TelegramError::RateLimited {
                retry_after: Duration::from_secs(7),
            });
        let started = tokio::time::Instant::now();
        let outcome = s
            .responder
            .respond(&update(message(
                1,
                OWNER,
                OWNER,
                "What is the notice period?",
            )))
            .await;
        assert_eq!(outcome, Outcome::Answered(Audience::Owner));
        assert!(started.elapsed() >= Duration::from_secs(7));
        assert_eq!(s.bot.sent.lock().unwrap().len(), 1);
        // A wait longer than allowed fails at once.
        s.bot
            .failures
            .lock()
            .unwrap()
            .push_back(TelegramError::RateLimited {
                retry_after: Duration::from_secs(600),
            });
        let outcome = s
            .responder
            .respond(&update(message(2, OWNER, OWNER, "And the deposit?")))
            .await;
        assert!(matches!(outcome, Outcome::SendFailed(_)), "{outcome:?}");
    }

    #[tokio::test]
    async fn html_telegram_cannot_read_is_sent_as_plain_text() {
        let s = setup(RecordingModel::answering("Sixty days [1]."), 5);
        s.bot
            .failures
            .lock()
            .unwrap()
            .push_back(TelegramError::Api {
                status: 400,
                description: "Bad Request: can't parse entities: unclosed tag".into(),
            });
        s.responder
            .respond(&update(message(
                1,
                OWNER,
                OWNER,
                "What is the notice period?",
            )))
            .await;
        let sent = s.bot.sent.lock().unwrap()[0].clone();
        assert!(!sent.2, "plain");
        assert!(sent.1.contains("Sources\n[1] Office lease"));
    }

    #[tokio::test(start_paused = true)]
    async fn typing_shows_while_the_model_works_and_stops_after() {
        struct SlowModel;
        #[async_trait]
        impl TextModel for SlowModel {
            fn model_id(&self) -> String {
                "slow".into()
            }
            async fn complete(&self, _prompt: &str, _max: usize) -> Result<String, String> {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok("Done [1].".into())
            }
        }
        let bot = Arc::new(FakeBot::default());
        let brain = FakeBrain::new(RecordingModel::answering("unused"));
        struct SlowBrain(FakeBrain);
        #[async_trait]
        impl TelegramBrain for SlowBrain {
            async fn rules(&self) -> Result<Rules, String> {
                self.0.rules().await
            }
            fn redeem(&self, code: &str) -> Result<LinkKind, LinkError> {
                self.0.redeem(code)
            }
            async fn link(&self, kind: LinkKind, person: &Person) -> Result<(), String> {
                self.0.link(kind, person).await
            }
            async fn membership(&self, chat: &Chat, by: Option<&Person>, joined: bool) {
                self.0.membership(chat, by, joined).await
            }
            async fn migrated(&self, from: i64, to: i64) {
                self.0.migrated(from, to).await
            }
            async fn chat_ref(
                &self,
                a: Audience,
                chat: &Chat,
                from: &Person,
                root: i64,
            ) -> ChatRef {
                self.0.chat_ref(a, chat, from, root).await
            }
            async fn new_conversation(&self, chat: i64) {
                self.0.new_conversation(chat).await
            }
            async fn guest_scope(&self) -> Result<Option<AnswerScope>, String> {
                self.0.guest_scope().await
            }
            async fn retrieve(
                &self,
                scope: Option<&AnswerScope>,
                q: &str,
            ) -> Result<Vec<AnswerPassage>, String> {
                self.0.retrieve(scope, q).await
            }
            async fn memories(&self, m: &str) -> Option<String> {
                self.0.memories(m).await
            }
            async fn remember(
                &self,
                r: &MemoryRequest,
                c: &ChatRef,
                id: i64,
            ) -> Option<Remembered> {
                self.0.remember(r, c, id).await
            }
            async fn earlier(&self, c: &ChatRef) -> Vec<ThreadTurn> {
                self.0.earlier(c).await
            }
            async fn keep(&self, c: &ChatRef, au: Audience, a: &Asked, t: &str, l: bool) {
                self.0.keep(c, au, a, t, l).await
            }
            fn model(&self) -> Result<Arc<dyn TextModel>, String> {
                Ok(Arc::new(SlowModel))
            }
            fn private_terms(&self) -> PrivateTerms {
                PrivateTerms::default()
            }
        }
        let responder = Responder::new(
            Arc::new(SlowBrain(brain)),
            bot.clone(),
            Arc::new(MemoryQuota::new(5)),
            Arc::new(FixedClock(now())),
            CancellationToken::new(),
        );
        let outcome = responder
            .respond(&update(message(
                1,
                OWNER,
                OWNER,
                "What is the notice period?",
            )))
            .await;
        assert_eq!(outcome, Outcome::Answered(Audience::Owner));
        let shown = *bot.typing.lock().unwrap();
        // Shown at once and every 4 s during the 10 s answer: 3 times.
        assert_eq!(shown, 3, "typing shown {shown} times");
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert_eq!(
            *bot.typing.lock().unwrap(),
            shown,
            "and not after the answer"
        );
    }

    /// India: +05:30.
    fn ist(_: DateTime<Utc>) -> FixedOffset {
        FixedOffset::east_opt(5 * 3600 + 1800).unwrap()
    }

    #[tokio::test]
    async fn a_message_that_waited_while_shodh_was_off_says_when_it_was_sent() {
        let s = setup(RecordingModel::answering("Sixty days."), 5);
        let responder = s.responder.with_offset_of(ist);
        // Two hours before the clock's 09:00 UTC: 07:00 UTC, 12:30 in India.
        let mut late = message(1, OWNER, OWNER, "What is the notice period?");
        late.date = now() - chrono::Duration::hours(2);
        assert_eq!(
            responder.respond(&update(late)).await,
            Outcome::Answered(Audience::Owner)
        );
        let texts = s.bot.texts();
        assert!(
            texts[0].starts_with("(You sent this at 12:30, while Shodh was offline.)"),
            "{texts:?}"
        );
        assert!(texts[0].contains("Sixty days."));
        // A fresh message: answered as it is.
        let mut fresh = message(2, OWNER, OWNER, "And the deposit?");
        fresh.date = now() - chrono::Duration::minutes(3);
        responder.respond(&update(fresh)).await;
        let texts = s.bot.texts();
        assert!(!texts[1].contains("while Shodh was offline"), "{texts:?}");
        assert_eq!(s.brain.model.calls(), 2);
    }

    #[tokio::test]
    async fn a_backlog_of_day_old_messages_gets_one_note_and_no_model_call() {
        let s = setup(RecordingModel::answering("x"), 5);
        let responder = s.responder.with_offset_of(ist);
        // 30 hours before: Mon 26 Oct, 03:01 UTC = 08:31 in India.
        let sent = now() - chrono::Duration::hours(30);
        for i in 1..=3 {
            let mut m = message(i, OWNER, OWNER, "What is the rent?");
            m.date = sent + chrono::Duration::minutes(i);
            assert_eq!(responder.respond(&update(m)).await, Outcome::Stale);
        }
        assert_eq!(
            s.bot.texts(),
            vec![
                "I was offline when you sent this (Mon 08:31). Ask again if you still need it."
                    .to_string()
            ]
        );
        assert_eq!(s.brain.model.calls(), 0);
        assert!(s.brain.retrieved_with.lock().unwrap().is_empty());
        assert!(s.brain.kept.lock().unwrap().is_empty());
        // Someone else's chat gets its own note, and spends none of the day's answers.
        let mut guest = message(4, GUEST, GUEST, "What is the rent?");
        guest.date = sent;
        assert_eq!(responder.respond(&update(guest)).await, Outcome::Stale);
        assert_eq!(s.bot.texts().len(), 2);
        assert_eq!(s.brain.model.calls(), 0);
        assert_eq!(s.quota.used(utc_day(now())).unwrap(), 0);
    }

    #[tokio::test]
    async fn small_talk_is_answered_without_a_library_search() {
        let s = setup(RecordingModel::answering("All good here!"), 5);
        // The owner: one short model call, no search, no memories.
        assert_eq!(
            s.responder
                .respond(&update(message(1, OWNER, OWNER, "How's it going shodh")))
                .await,
            Outcome::Answered(Audience::Owner)
        );
        assert_eq!(s.brain.model.calls(), 1);
        assert!(s.brain.retrieved_with.lock().unwrap().is_empty());
        assert_eq!(*s.brain.memories_asked.lock().unwrap(), 0);
        assert!(s.brain.model.prompts.lock().unwrap()[0].contains("small talk"));
        // Someone else: a fixed reply; no search, no model call, no answer spent.
        assert_eq!(
            s.responder
                .respond(&update(message(2, GUEST, GUEST, "hi there!")))
                .await,
            Outcome::SmallTalk
        );
        assert!(s.bot.texts().last().unwrap().contains("Ask me a question"));
        assert_eq!(s.brain.model.calls(), 1);
        assert!(s.brain.retrieved_with.lock().unwrap().is_empty());
        assert_eq!(s.quota.used(utc_day(now())).unwrap(), 0);
        // A real question is searched as before, with a greeting in front or not.
        s.responder
            .respond(&update(message(3, OWNER, OWNER, "hi, what is the rent?")))
            .await;
        assert_eq!(s.brain.retrieved_with.lock().unwrap().len(), 1);
        assert_eq!(*s.brain.memories_asked.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_cancelled_responder_sends_nothing() {
        let s = setup(RecordingModel::answering("x"), 5);
        s.responder.cancel.cancel();
        assert_eq!(
            s.responder
                .respond(&update(message(1, OWNER, OWNER, "hi")))
                .await,
            Outcome::Cancelled
        );
        assert!(s.bot.texts().is_empty());
        assert_eq!(s.brain.model.calls(), 0);
    }

    #[tokio::test]
    async fn the_bot_added_to_a_group_is_told_to_the_app() {
        let s = setup(RecordingModel::answering("x"), 5);
        let added = Update::Membership {
            update_id: 1,
            chat: Chat {
                id: -5,
                kind: crate::connectors::telegram::updates::ChatKind::Group,
                title: Some("Deals".into()),
            },
            by: None,
            joined: true,
        };
        s.responder.respond(&added).await;
        assert_eq!(*s.brain.memberships.lock().unwrap(), vec![(-5, true)]);
        assert!(s.bot.texts().is_empty(), "nothing is said in the group");
    }

    /// A model that unlinks the owner while it writes (the user pressed Unlink).
    struct UnlinkingModel(Arc<StdMutex<Rules>>);

    #[async_trait]
    impl TextModel for UnlinkingModel {
        fn model_id(&self) -> String {
            "unlinking".into()
        }
        async fn complete(&self, _prompt: &str, _max: usize) -> Result<String, String> {
            self.0.lock().unwrap().owner = None;
            Ok("Sixty days [1].".into())
        }
    }

    #[tokio::test]
    async fn an_owner_unlinked_while_the_answer_is_written_gets_nothing() {
        let s = setup(RecordingModel::answering("unused"), 5);
        *s.brain.other_model.lock().unwrap() =
            Some(Arc::new(UnlinkingModel(s.brain.rules.clone())));
        let outcome = s
            .responder
            .respond(&update(message(
                1,
                OWNER,
                OWNER,
                "What is the notice period?",
            )))
            .await;
        assert_eq!(outcome, Outcome::Unlinked);
        assert!(s.bot.texts().is_empty(), "nothing sent to the former owner");
        assert!(s.brain.kept.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_answer_stopped_while_it_is_sent_is_not_kept() {
        let s = setup(RecordingModel::answering("Sixty days [1]."), 5);
        *s.bot.cancel_after_send.lock().unwrap() = Some(s.responder.cancel.clone());
        let outcome = s
            .responder
            .respond(&update(message(
                1,
                OWNER,
                OWNER,
                "What is the notice period?",
            )))
            .await;
        assert_eq!(outcome, Outcome::Cancelled);
        assert_eq!(s.bot.texts().len(), 1, "it was sent before the stop");
        assert!(
            s.brain.kept.lock().unwrap().is_empty(),
            "a Disconnect that forgets conversations is not undone by this answer"
        );
    }

    #[tokio::test]
    async fn the_owners_answer_keeps_their_paths_and_others_answers_never() {
        let reply = r"It is in C:\Users\Ann\Documents\lease.txt [1].";
        let s = setup(RecordingModel::answering(reply), 5);
        s.responder
            .respond(&update(message(1, OWNER, OWNER, "Where is the lease?")))
            .await;
        s.responder
            .respond(&update(message(2, GUEST, GUEST, "Where is the lease?")))
            .await;
        let texts = s.bot.texts();
        assert!(
            texts[0].contains(r"C:\Users\Ann\Documents\lease.txt"),
            "{}",
            texts[0]
        );
        assert!(!texts[1].contains(r"C:\Users"), "{}", texts[1]);
    }

    #[tokio::test]
    async fn a_group_answer_is_kept_as_the_groups() {
        let s = setup(RecordingModel::answering("Sixty days [1]."), 5);
        s.responder
            .respond(&update(message(
                1,
                GROUP,
                OWNER,
                "/ask@shodh_notes_bot rent?",
            )))
            .await;
        let kept = s.brain.kept.lock().unwrap()[0].clone();
        assert!(kept.1.owner, "the owner's own words");
        assert_eq!(
            kept.4,
            Audience::Group,
            "the app gives the learner no group answer"
        );
    }

    #[derive(Default)]
    struct Counted(StdMutex<Vec<Outcome>>);

    impl OutcomeSink for Counted {
        fn handled(&self, outcome: &Outcome) {
            self.0.lock().unwrap().push(outcome.clone());
        }
    }

    #[tokio::test]
    async fn a_backlog_is_answered_whole_never_dropped() {
        let s = setup(RecordingModel::answering("x"), 5);
        let sink = Arc::new(Counted::default());
        let dispatcher = Dispatcher::new(
            Arc::new(s.responder),
            tokio::runtime::Handle::current(),
            sink.clone(),
        );
        let stop = CancellationToken::new();
        // Thirty strangers wrote while the computer slept: more than 16 at once.
        for i in 0..30 {
            let u = update(message(i, 500 + i, 500 + i, "hello"));
            let room = dispatcher.room(&u, &stop).await.expect("room");
            dispatcher.start(u, room);
        }
        dispatcher.drained().await;
        let outcomes = sink.0.lock().unwrap().clone();
        assert_eq!(outcomes.len(), 30);
        assert!(outcomes.iter().all(|o| *o == Outcome::Refused));
        assert_eq!(s.bot.texts().len(), 30, "each one told, none lost");
        assert_eq!(dispatcher.pending(), 0);
    }

    #[tokio::test]
    async fn without_room_others_wait_and_the_owner_never_does() {
        let s = setup(RecordingModel::answering("x"), 5);
        let dispatcher = Dispatcher::with_room(
            Arc::new(s.responder),
            tokio::runtime::Handle::current(),
            Arc::new(Counted::default()),
            1,
        );
        let stop = CancellationToken::new();
        let first = update(message(1, 900, 900, "hello"));
        let held = dispatcher.room(&first, &stop).await.expect("room");
        // The owner's chat has its own lane: room at once.
        let owner = update(message(2, OWNER, OWNER, "hi"));
        assert!(dispatcher.room(&owner, &stop).await.is_some());
        // Anyone else waits until room is given back; a stop ends the wait with nothing
        // taken (the update stays with Telegram).
        let second = update(message(3, 901, 901, "hello"));
        let waiting = dispatcher.room(&second, &stop);
        stop.cancel();
        assert!(waiting.await.is_none());
        drop(held);
        let open = CancellationToken::new();
        assert!(
            dispatcher.room(&second, &open).await.is_some(),
            "room again once given back"
        );
    }

    #[tokio::test]
    async fn a_panicking_answer_gives_its_place_back() {
        let tasks = Arc::new(AtomicUsize::new(0));
        let idle = Arc::new(tokio::sync::Notify::new());
        let pending = Pending::new(tasks.clone(), idle.clone());
        assert_eq!(tasks.load(Ordering::SeqCst), 1);
        let crashed = tokio::spawn(async move {
            let _pending = pending;
            panic!("an answer crashed");
        })
        .await;
        assert!(crashed.is_err());
        assert_eq!(tasks.load(Ordering::SeqCst), 0, "the place is given back");
    }

    const NEMOTRON: &str = "nvidia/nemotron-3-super-120b-a12b:free";
    const GEMMA: &str = "google/gemma-4-31b-it:free";

    fn nemotron_then_gemma() -> crate::harness::model_fallback::Plan {
        use crate::harness::model_fallback::testkit::candidate;
        use crate::harness::model_fallback::{Origin, Plan};
        Plan {
            requested: candidate(NEMOTRON, "Nemotron Super", Origin::Requested),
            fallbacks: vec![candidate(GEMMA, "Gemma 4 31B", Origin::Automatic)],
        }
    }

    #[tokio::test]
    async fn telegram_answers_through_the_fallback_and_the_app_record_names_the_model() {
        use crate::harness::model_fallback::testkit::{
            fallback_model, unavailable, ScriptedModels,
        };
        let s = setup(RecordingModel::answering("unused"), 5);
        let models = ScriptedModels::new(vec![
            (NEMOTRON, Err(unavailable(NEMOTRON))),
            (GEMMA, Ok("Sixty days [1].")),
        ]);
        *s.brain.other_model.lock().unwrap() = Some(Arc::new(fallback_model(
            nemotron_then_gemma(),
            models.clone(),
            0,
        )));
        let outcome = s
            .responder
            .respond(&update(message(
                1,
                OWNER,
                OWNER,
                "What is the notice period?",
            )))
            .await;
        assert_eq!(outcome, Outcome::Answered(Audience::Owner));
        assert_eq!(*models.calls.lock().unwrap(), [NEMOTRON, GEMMA]);
        let texts = s.bot.texts();
        assert!(texts[0].contains("Sixty days"), "{texts:?}");
        assert!(
            !texts[0].contains("Gemma"),
            "no notice in the chat: {texts:?}"
        );
        let answered = s.brain.answered_by.lock().unwrap();
        assert_eq!(
            answered[0].as_ref().and_then(|by| by.note()).as_deref(),
            Some("Answered by Gemma 4 31B — Nemotron Super is unavailable"),
            "the app's record says which model answered"
        );
    }

    #[tokio::test]
    async fn the_daily_free_quota_says_when_it_resets_in_telegram() {
        use crate::harness::model_fallback::testkit::{
            daily_quota, fallback_model, ScriptedModels,
        };
        let s = setup(RecordingModel::answering("unused"), 5);
        // Midnight UTC: 05:30 in India.
        let reset = Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap();
        let reset_ms = u64::try_from(reset.timestamp_millis()).unwrap();
        let models = ScriptedModels::new(vec![
            (NEMOTRON, Err(daily_quota(NEMOTRON, reset_ms))),
            (GEMMA, Ok("must not answer")),
        ]);
        let now_ms = u64::try_from(now().timestamp_millis()).unwrap();
        *s.brain.other_model.lock().unwrap() = Some(Arc::new(fallback_model(
            nemotron_then_gemma(),
            models.clone(),
            now_ms,
        )));
        let responder = s.responder.with_offset_of(ist);
        let outcome = responder
            .respond(&update(message(
                1,
                OWNER,
                OWNER,
                "What is the notice period?",
            )))
            .await;
        assert_eq!(
            outcome,
            Outcome::ModelFailed(ProviderErrorKind::QuotaExhausted)
        );
        assert_eq!(
            *models.calls.lock().unwrap(),
            [NEMOTRON],
            "another free model shares the quota: not tried"
        );
        assert_eq!(
            s.bot.texts(),
            vec!["Today's free OpenRouter quota is used up, so I can't answer until it resets at 05:30. To answer before then, allow paid backups in Shodh (Settings → Model).".to_string()]
        );
    }

    #[test]
    fn a_guest_is_not_told_which_provider_the_owner_uses() {
        use crate::harness::model_fallback::testkit::daily_quota;
        let reset = Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap();
        let error =
            daily_quota(NEMOTRON, u64::try_from(reset.timestamp_millis()).unwrap()).to_string();
        for audience in [Audience::Guest, Audience::Group] {
            let text = model_failure_text(
                ProviderErrorKind::QuotaExhausted,
                &error,
                now(),
                ist,
                audience,
            );
            assert_eq!(
                text,
                "Today's free model quota is used up, so I can't answer until it resets at 05:30."
            );
        }
    }
}
