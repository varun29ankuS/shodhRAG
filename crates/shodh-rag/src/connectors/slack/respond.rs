//! One question, start to finish: the checks that cost nothing come first, the model call
//! comes last and only when everything before it passed.
//!
//! 0. Who asked, and where ([`super::access`]): a channel not on the card's list, or a DM
//!    from someone outside the installing workspace (another organization, a guest) gets
//!    [`NOT_ALLOWED`], with no search and no model call. `remember: …` and `remember this
//!    thread` save a memory instead ([`super::remember`]); the asker must also be a full
//!    member of the installing workspace.
//! 1. No workspace selected (or it no longer exists): "isn't set up", no model call. The
//!    workspace's indexed Slack channels are narrowed to the one asked in (none in a DM).
//! 2. Empty question (only the mention, or only a file): a hint, no model call.
//! 3. A question about what the library holds ("what files do you have") is answered from
//!    the document list: exact, no model call.
//! 4. Today's cap reached: the limit message, no model call.
//! 5. A placeholder ("Searching your library…") is posted in the thread.
//! 6. Retrieval over the selected workspace (the query carries the thread's previous
//!    question), plus what people saved for this channel. Nothing found: the placeholder
//!    says so, no model call.
//! 7. One call is reserved against the cap of the UTC day it is made on (counted before it
//!    is made), then made, with the thread's earlier questions and answers as context.
//! 8. The placeholder is edited into the answer, or into a short, honest reason when the
//!    model failed. If Slack will not take the edit, the answer is posted as a new reply.
//!    An answer posted is kept for follow-ups in the same thread.
//!
//! **Stopping.** Switching the connector off, Disconnect, Local-only mode and new tokens
//! cancel the installation's [`CancellationToken`]. A question checks it before the
//! search, before the model call and before every post: a cancelled answer posts nothing
//! more, and gives its reservation back when no model call was made.
//!
//! **Slack refusing a post.** A rate limit (HTTP 429) is waited out for the `Retry-After`
//! Slack sends (<https://docs.slack.dev/apis/web-api/rate-limits>), a bounded number of
//! times; a network error or a 5xx gets one more try. That is Slack's protocol, not a retry
//! to hide flakiness: anything else fails at once and is reported.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use tokio_util::sync::CancellationToken;

use super::access::{Access, AccessGate, Refusal, NOT_ALLOWED};
use super::answer::{
    answer_prompt, is_library_question, library_listing_message, search_query, slack_answer,
    AnswerPassage, AnswerScope, LibraryListing, ThreadTurn, MAX_ANSWER_TOKENS,
};
use super::events::{Question, QuestionKind, ThreadKey};
use super::remember::{
    memory_text, parse_remember, summary_prompt, MemoryDraft, RememberCommand, SlackMemories,
    NOTHING_TO_SAVE, NOT_A_THREAD, NOT_SAVED, SAVED, SAVED_FOR_CHANNEL, SUMMARY_TOKENS,
    THREAD_UNREADABLE,
};
use super::sanitize::{sanitize, PrivateTerms};
use super::store::{SlackMemory, SlackMemoryKind};
use super::web::SlackApiError;
use crate::harness::provider_error::{classify, ProviderErrorKind};
use crate::research::results::TextModel;
use crate::research::tables::calls::{next_utc_midnight, utc_day, Clock};

pub const NOT_SET_UP: &str = "Shodh isn't set up to answer here yet.";
pub const PLACEHOLDER: &str = "Searching your library…";
pub const BUSY: &str = "I'm still answering the previous question in this thread.";
pub const OVERLOADED: &str =
    "Shodh is answering other questions right now. Please ask again in a few minutes.";
pub const EMPTY_QUESTION: &str =
    "Ask me a question after the mention, for example: “What does the report say about costs?”";
pub const FILES_ONLY: &str =
    "I can only read the text of a message, not its files. Ask your question in words.";
pub const NOTHING_FOUND: &str =
    "I couldn't find anything about that in the documents Shodh answers from here.";
pub const SEARCH_FAILED: &str =
    "Shodh couldn't search its documents just now. Please try again later.";
pub const LIMIT_UNKNOWN: &str =
    "Shodh couldn't check today's answer limit, so it didn't answer. Please try again later.";

/// The cap message, with the reset time in UTC.
pub fn at_cap_message(resets_at: DateTime<Utc>) -> String {
    format!(
        "Shodh has used today's answer limit; it resets at {} UTC.",
        resets_at.format("%H:%M")
    )
}

/// What a failed model call becomes in Slack: short and honest, no details (those go to
/// the app log).
pub fn model_failure_message(kind: ProviderErrorKind) -> &'static str {
    match kind {
        ProviderErrorKind::RateLimited => {
            "The model is busy right now (rate limited). Please try again in a minute."
        }
        ProviderErrorKind::QuotaExhausted => {
            "The model's quota for today is used up, so Shodh can't answer until it resets."
        }
        ProviderErrorKind::Auth => {
            "Shodh can't use its model right now (the API key was refused). The owner needs to fix it in Shodh."
        }
        ProviderErrorKind::Timeout | ProviderErrorKind::Network => {
            "Shodh couldn't reach its model. Please try again shortly."
        }
        ProviderErrorKind::ModelUnavailable => {
            "The model Shodh uses is unavailable right now. Please try again later."
        }
        ProviderErrorKind::ContextTooLong => {
            "That question needs more text than the model accepts. Try a narrower question."
        }
        ProviderErrorKind::ContentFiltered => "The model declined to answer that question.",
        _ => "Shodh couldn't answer that just now. Please try again later.",
    }
}

/// Posts and edits messages in Slack.
#[async_trait]
pub trait SlackPoster: Send + Sync {
    /// Posts `text` in `channel` (in the thread `thread_ts` when given); returns the
    /// message's `ts`.
    async fn post(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<String, SlackApiError>;
    /// Replaces the text of message `ts`.
    async fn update(&self, channel: &str, ts: &str, text: &str) -> Result<(), SlackApiError>;
}

/// Where answers come from. The app implements it over the selected workspace.
#[async_trait]
pub trait AnswerSource: Send + Sync {
    /// The scope of the selected workspace; `None` while none is selected or when the
    /// selected one no longer exists.
    async fn scope(&self) -> Result<Option<AnswerScope>, String>;
    /// Passages for `question` within `scope`
    /// ([`super::answer::retrieve_passages`]).
    async fn retrieve(
        &self,
        scope: &AnswerScope,
        question: &str,
    ) -> Result<Vec<AnswerPassage>, String>;
    /// The documents and Slack threads of `scope`
    /// ([`super::answer::library_listing`]).
    async fn listing(&self, scope: &AnswerScope) -> Result<LibraryListing, String>;
    /// The earlier questions and answers of `thread` (of this Slack workspace), oldest
    /// first.
    async fn thread_turns(&self, thread: &ThreadKey) -> Result<Vec<ThreadTurn>, String>;
    /// Keeps a question and its answer in `thread`, for follow-ups there.
    async fn record_turn(&self, thread: &ThreadKey, turn: ThreadTurn) -> Result<(), String>;
    /// The memories people other than the owner saved for `channel`.
    async fn channel_memories(&self, channel: &str) -> Result<Vec<SlackMemory>, String>;
    /// The model answers are written with (no tools).
    fn model(&self) -> Result<Arc<dyn TextModel>, String>;
    /// This computer's home folder and the user's names, never to appear in Slack.
    fn private_terms(&self) -> PrivateTerms;
}

/// The daily count of Slack answers (UTC days). The calls may block (a file); the
/// responder makes them off the async workers.
pub trait AnswerQuota: Send + Sync {
    /// The cap now.
    fn cap(&self) -> usize;
    /// Answers counted on `day`.
    fn used(&self, day: NaiveDate) -> Result<usize, String>;
    /// Counts one answer on `day` if that stays within the cap. `Ok(false)`: at the cap,
    /// nothing counted.
    fn try_reserve(&self, day: NaiveDate) -> Result<bool, String>;
    /// Gives back one answer reserved on `day` whose model call was never made.
    fn release(&self, day: NaiveDate) -> Result<(), String>;
}

/// How long Slack's refusals of a post are waited out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Tries after a rate limit (HTTP 429), each after Slack's `Retry-After`.
    pub rate_limit_retries: u32,
    /// The longest `Retry-After` waited for; a longer one fails the post at once.
    pub max_retry_after: Duration,
    /// The wait after a 429 without `Retry-After`.
    pub default_retry_after: Duration,
    /// Tries after a network error or a 5xx.
    pub transient_retries: u32,
    pub transient_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            rate_limit_retries: 3,
            max_retry_after: Duration::from_secs(30),
            default_retry_after: Duration::from_secs(1),
            transient_retries: 1,
            transient_delay: Duration::from_secs(2),
        }
    }
}

/// How a question ended (for the log, the card and tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    NotAllowed(Refusal),
    NotSetUp,
    EmptyQuestion,
    FilesOnly,
    AtCap,
    NothingFound,
    Answered,
    /// A question about the library, answered from its document list.
    Listed,
    /// A memory was saved (`owner`: into the user's memories).
    Saved {
        owner: bool,
    },
    /// `remember:` with nothing after it, or "remember this thread" outside a thread.
    NothingToSave,
    /// The memory could not be saved.
    SaveFailed,
    ModelFailed(ProviderErrorKind),
    SearchFailed,
    LimitUnknown,
    /// Told the asker an answer in the thread is in flight.
    Busy,
    /// Told the asker too many questions are waiting.
    Overloaded,
    /// The connector was switched off, disconnected or given new tokens: nothing more
    /// was posted.
    Cancelled,
    /// Slack refused a post or an edit, after its retries; nothing more can be said there.
    SlackFailed(String),
}

impl Outcome {
    /// Something was said in Slack (or deliberately not), as opposed to Slack refusing it.
    pub fn reached_slack(&self) -> bool {
        !matches!(self, Outcome::SlackFailed(_) | Outcome::Cancelled)
    }
}

/// Why a post did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PostError {
    Cancelled,
    Slack(SlackApiError),
}

/// Answers questions with an [`AnswerSource`], within an [`AccessGate`], counted against an
/// [`AnswerQuota`], until its [`CancellationToken`] is cancelled.
pub struct Responder {
    pub source: Arc<dyn AnswerSource>,
    pub memories: Arc<dyn SlackMemories>,
    pub gate: Arc<dyn AccessGate>,
    pub quota: Arc<dyn AnswerQuota>,
    pub poster: Arc<dyn SlackPoster>,
    pub clock: Arc<dyn Clock>,
    pub cancel: CancellationToken,
    pub retry: RetryPolicy,
}

impl Responder {
    pub fn new(
        source: Arc<dyn AnswerSource>,
        memories: Arc<dyn SlackMemories>,
        gate: Arc<dyn AccessGate>,
        quota: Arc<dyn AnswerQuota>,
        poster: Arc<dyn SlackPoster>,
        clock: Arc<dyn Clock>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            source,
            memories,
            gate,
            quota,
            poster,
            clock,
            cancel,
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Cancelled when the connector stops answering.
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Waits `delay`; false when cancelled meanwhile.
    async fn pause(&self, delay: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(delay) => !self.cancelled(),
            _ = self.cancel.cancelled() => false,
        }
    }

    /// Makes `call` (a post or an edit), waiting out Slack's rate limits and one transient
    /// failure ([`RetryPolicy`]); never after cancellation.
    async fn retrying<T, F, Fut>(&self, what: &'static str, mut call: F) -> Result<T, PostError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, SlackApiError>>,
    {
        let mut rate_limited = 0;
        let mut transient = 0;
        loop {
            if self.cancelled() {
                return Err(PostError::Cancelled);
            }
            let error = match call().await {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };
            let delay = match &error {
                SlackApiError::RateLimited { retry_after }
                    if rate_limited < self.retry.rate_limit_retries =>
                {
                    let wait = retry_after.unwrap_or(self.retry.default_retry_after);
                    if wait > self.retry.max_retry_after {
                        return Err(PostError::Slack(error));
                    }
                    rate_limited += 1;
                    wait
                }
                e if e.is_transient() && transient < self.retry.transient_retries => {
                    transient += 1;
                    self.retry.transient_delay
                }
                _ => return Err(PostError::Slack(error)),
            };
            tracing::info!(target: "shodh::slack", what, error = %error, delay_ms = delay.as_millis() as u64, "Slack did not take the message yet; trying again");
            if !self.pause(delay).await {
                return Err(PostError::Cancelled);
            }
        }
    }

    async fn post(&self, question: &Question, text: &str) -> Result<String, PostError> {
        self.retrying("chat.postMessage", || {
            self.poster
                .post(&question.channel, question.reply_thread_ts.as_deref(), text)
        })
        .await
    }

    /// Posts a reply that needs no model call and reports `outcome` (or Slack's refusal).
    async fn reply(&self, question: &Question, text: &str, outcome: Outcome) -> Outcome {
        match self.post(question, text).await {
            Ok(_) => outcome,
            Err(PostError::Cancelled) => Outcome::Cancelled,
            Err(PostError::Slack(e)) => Outcome::SlackFailed(e.to_string()),
        }
    }

    /// Replaces the placeholder `ts` with `text`; when Slack will not take the edit, posts
    /// `text` as a new reply in the thread.
    async fn finish(&self, question: &Question, ts: &str, text: &str, outcome: Outcome) -> Outcome {
        let edited = self
            .retrying("chat.update", || {
                self.poster.update(&question.channel, ts, text)
            })
            .await;
        match edited {
            Ok(()) => outcome,
            Err(PostError::Cancelled) => Outcome::Cancelled,
            Err(PostError::Slack(e)) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the placeholder could not be edited; the answer is posted as a new reply");
                self.reply(question, text, outcome).await
            }
        }
    }

    async fn quota_used(&self, day: NaiveDate) -> Result<usize, String> {
        let quota = self.quota.clone();
        tokio::task::spawn_blocking(move || quota.used(day))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn quota_reserve(&self, day: NaiveDate) -> Result<bool, String> {
        let quota = self.quota.clone();
        tokio::task::spawn_blocking(move || quota.try_reserve(day))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn quota_release(&self, day: NaiveDate) {
        let quota = self.quota.clone();
        let released = tokio::task::spawn_blocking(move || quota.release(day))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
        if let Err(e) = released {
            tracing::warn!(target: "shodh::slack", error = %e, "an unused Slack answer could not be given back");
        }
    }

    /// Answers `question` (see the module docs for the order of the steps).
    pub async fn respond(&self, question: &Question) -> Outcome {
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        if let Access::Refused(refusal) = self.gate.check(question).await {
            tracing::info!(target: "shodh::slack", ?refusal, "a Slack question from where Shodh does not answer");
            return self
                .reply(question, NOT_ALLOWED, Outcome::NotAllowed(refusal))
                .await;
        }
        if let Some(command) = parse_remember(&question.text) {
            return self.remember(question, command).await;
        }
        let scope = match self.source.scope().await {
            Ok(Some(scope)) => scope,
            Ok(None) => return self.reply(question, NOT_SET_UP, Outcome::NotSetUp).await,
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the selected workspace could not be read");
                return self.reply(question, NOT_SET_UP, Outcome::NotSetUp).await;
            }
        };
        // Indexed Slack channels of the workspace: only the one asked in (a DM: none).
        let scope = scope.for_channel(match question.kind {
            QuestionKind::Mention => Some(question.channel.as_str()),
            QuestionKind::Direct => None,
        });
        if question.text.trim().is_empty() {
            return if question.has_files {
                self.reply(question, FILES_ONLY, Outcome::FilesOnly).await
            } else {
                self.reply(question, EMPTY_QUESTION, Outcome::EmptyQuestion)
                    .await
            };
        }
        if is_library_question(&question.text) {
            // What the library holds is a list, not a search: exact, and no model call.
            return match self.source.listing(&scope).await {
                Ok(listing) => {
                    let text = library_listing_message(&listing, &self.source.private_terms());
                    self.reply(question, &text, Outcome::Listed).await
                }
                Err(e) => {
                    tracing::warn!(target: "shodh::slack", error = %e, "the documents of the workspace could not be listed");
                    self.reply(question, SEARCH_FAILED, Outcome::SearchFailed)
                        .await
                }
            };
        }
        let asked_at = self.clock.now();
        match self.quota_used(utc_day(asked_at)).await {
            Ok(used) if used >= self.quota.cap() => {
                return self
                    .reply(
                        question,
                        &at_cap_message(next_utc_midnight(asked_at)),
                        Outcome::AtCap,
                    )
                    .await
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the answer count could not be read");
                return self
                    .reply(question, LIMIT_UNKNOWN, Outcome::LimitUnknown)
                    .await;
            }
        }
        let placeholder = match self.post(question, PLACEHOLDER).await {
            Ok(ts) => ts,
            Err(PostError::Cancelled) => return Outcome::Cancelled,
            Err(PostError::Slack(e)) => return Outcome::SlackFailed(e.to_string()),
        };
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        // Earlier questions and answers of this thread only.
        let earlier = self
            .source
            .thread_turns(&question.thread)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(target: "shodh::slack", error = %e, "the earlier turns of a Slack thread could not be read");
                Vec::new()
            });
        let query = search_query(&question.text, &earlier);
        let mut passages = match self.source.retrieve(&scope, &query).await {
            Ok(passages) => passages,
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "search for a Slack question failed");
                return self
                    .finish(question, &placeholder, SEARCH_FAILED, Outcome::SearchFailed)
                    .await;
            }
        };
        // What people saved for this channel (or this DM), as numbered passages after the
        // documents'.
        let notes = self
            .source
            .channel_memories(&question.channel)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(target: "shodh::slack", error = %e, "the channel's Slack memories could not be read");
                Vec::new()
            });
        for note in notes {
            let n = u32::try_from(passages.len() + 1).unwrap_or(u32::MAX);
            passages.push(AnswerPassage {
                n,
                title: format!(
                    "Saved in this channel by {} on {}",
                    note.author_name,
                    note.saved_at.format("%Y-%m-%d")
                ),
                page: None,
                text: note.text,
            });
        }
        if passages.is_empty() {
            return self
                .finish(question, &placeholder, NOTHING_FOUND, Outcome::NothingFound)
                .await;
        }
        let model = match self.source.model() {
            Ok(model) => model,
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "no model for a Slack answer");
                let kind = ProviderErrorKind::ModelUnavailable;
                return self
                    .finish(
                        question,
                        &placeholder,
                        model_failure_message(kind),
                        Outcome::ModelFailed(kind),
                    )
                    .await;
            }
        };
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        // The call counts on the UTC day it is made, not the day the question arrived: a
        // question at 23:59:58 whose call is made at 00:00:03 counts on the new day.
        let now = self.clock.now();
        let day = utc_day(now);
        match self.quota_reserve(day).await {
            Ok(true) => {}
            Ok(false) => {
                return self
                    .finish(
                        question,
                        &placeholder,
                        &at_cap_message(next_utc_midnight(now)),
                        Outcome::AtCap,
                    )
                    .await
            }
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the answer could not be counted");
                return self
                    .finish(question, &placeholder, LIMIT_UNKNOWN, Outcome::LimitUnknown)
                    .await;
            }
        }
        if self.cancelled() {
            // Reserved, never called: the day's count gets it back.
            self.quota_release(day).await;
            return Outcome::Cancelled;
        }
        let private = self.source.private_terms();
        let prompt = answer_prompt(&passages, &question.text, &earlier, &private);
        let completion = tokio::select! {
            completion = model.complete(&prompt, MAX_ANSWER_TOKENS) => completion,
            // The call was made: it stays counted.
            _ = self.cancel.cancelled() => return Outcome::Cancelled,
        };
        match completion {
            Ok(text) if !text.trim().is_empty() => {
                let message = slack_answer(&text, &passages, &private);
                let outcome = self
                    .finish(question, &placeholder, &message, Outcome::Answered)
                    .await;
                if outcome == Outcome::Answered {
                    let turn = ThreadTurn {
                        question: question.text.trim().to_string(),
                        answer: message,
                    };
                    if let Err(e) = self.source.record_turn(&question.thread, turn).await {
                        tracing::warn!(target: "shodh::slack", error = %e, "a Slack thread's turn could not be kept");
                    }
                }
                outcome
            }
            Ok(_) => {
                let kind = ProviderErrorKind::EmptyResponse;
                tracing::warn!(target: "shodh::slack", "the model returned an empty answer");
                self.finish(
                    question,
                    &placeholder,
                    model_failure_message(kind),
                    Outcome::ModelFailed(kind),
                )
                .await
            }
            Err(e) => {
                let now_ms = u64::try_from(now.timestamp_millis()).unwrap_or(0);
                let kind = classify(&e, None, now_ms).kind;
                tracing::warn!(
                    target: "shodh::slack",
                    kind = ?kind,
                    error = %sanitize(&e, &private),
                    "the model call for a Slack answer failed"
                );
                self.finish(
                    question,
                    &placeholder,
                    model_failure_message(kind),
                    Outcome::ModelFailed(kind),
                )
                .await
            }
        }
    }

    /// Saves what `question` asked Shodh to remember (see [`super::remember`]): the
    /// owner's into the user's memories, anyone else's for the channel only. The asker
    /// must be a full member of the installing workspace.
    async fn remember(&self, question: &Question, command: RememberCommand) -> Outcome {
        let member = match self.gate.member(question).await {
            Ok(member) => member,
            Err(refusal) => {
                tracing::info!(target: "shodh::slack", ?refusal, "a Slack memory from someone who may not save one");
                return self
                    .reply(question, NOT_ALLOWED, Outcome::NotAllowed(refusal))
                    .await;
            }
        };
        let owner = self.memories.owner().await.unwrap_or_else(|e| {
            tracing::warn!(target: "shodh::slack", error = %e, "the owner's Slack account could not be read");
            None
        });
        let is_owner = owner.as_deref() == Some(question.user.as_str());
        let (text, kind) = match command {
            RememberCommand::Text(text) if text.trim().is_empty() => {
                return self
                    .reply(question, NOTHING_TO_SAVE, Outcome::NothingToSave)
                    .await
            }
            RememberCommand::Text(text) => (memory_text(&text), SlackMemoryKind::Text),
            RememberCommand::Thread => {
                if !question.in_thread {
                    return self
                        .reply(question, NOT_A_THREAD, Outcome::NothingToSave)
                        .await;
                }
                match self.summarize_thread(question).await {
                    Ok(summary) => (memory_text(&summary), SlackMemoryKind::Thread),
                    Err(outcome) => return outcome,
                }
            }
        };
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let draft = MemoryDraft {
            channel: question.channel.clone(),
            channel_name: self
                .memories
                .channel_name(&question.channel, question.kind == QuestionKind::Direct)
                .await,
            author_id: question.user.clone(),
            author_name: member.name,
            text,
            kind,
            message_ts: question.ts.clone(),
            permalink: self
                .memories
                .permalink(&question.channel, &question.ts)
                .await,
            at: self.clock.now(),
        };
        let saved = if is_owner {
            self.memories.save_owner(&draft).await
        } else {
            self.memories.save_channel(&draft).await
        };
        match saved {
            Ok(()) if is_owner => {
                self.reply(question, SAVED, Outcome::Saved { owner: true })
                    .await
            }
            Ok(()) => {
                self.reply(question, SAVED_FOR_CHANNEL, Outcome::Saved { owner: false })
                    .await
            }
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "a memory from Slack could not be saved");
                self.reply(question, NOT_SAVED, Outcome::SaveFailed).await
            }
        }
    }

    /// One model call (counted against the cap) that summarizes the question's thread;
    /// the outcome to end with when it cannot.
    async fn summarize_thread(&self, question: &Question) -> Result<String, Outcome> {
        let root = question.thread.thread.clone();
        let asked_at = self.clock.now();
        match self.quota_used(utc_day(asked_at)).await {
            Ok(used) if used >= self.quota.cap() => {
                return Err(self
                    .reply(
                        question,
                        &at_cap_message(next_utc_midnight(asked_at)),
                        Outcome::AtCap,
                    )
                    .await)
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the answer count could not be read");
                return Err(self
                    .reply(question, LIMIT_UNKNOWN, Outcome::LimitUnknown)
                    .await);
            }
        }
        let thread = match self.memories.thread_text(&question.channel, &root).await {
            Ok(thread) if !thread.trim().is_empty() => thread,
            Ok(_) => {
                return Err(self
                    .reply(question, THREAD_UNREADABLE, Outcome::SaveFailed)
                    .await)
            }
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "a Slack thread to remember could not be read");
                return Err(self
                    .reply(question, THREAD_UNREADABLE, Outcome::SaveFailed)
                    .await);
            }
        };
        let model = match self.source.model() {
            Ok(model) => model,
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "no model to summarize a Slack thread");
                let kind = ProviderErrorKind::ModelUnavailable;
                return Err(self
                    .reply(
                        question,
                        model_failure_message(kind),
                        Outcome::ModelFailed(kind),
                    )
                    .await);
            }
        };
        let now = self.clock.now();
        let day = utc_day(now);
        match self.quota_reserve(day).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(self
                    .reply(
                        question,
                        &at_cap_message(next_utc_midnight(now)),
                        Outcome::AtCap,
                    )
                    .await)
            }
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the summary could not be counted");
                return Err(self
                    .reply(question, LIMIT_UNKNOWN, Outcome::LimitUnknown)
                    .await);
            }
        }
        if self.cancelled() {
            self.quota_release(day).await;
            return Err(Outcome::Cancelled);
        }
        let completion = tokio::select! {
            completion = model.complete(&summary_prompt(&thread), SUMMARY_TOKENS) => completion,
            _ = self.cancel.cancelled() => return Err(Outcome::Cancelled),
        };
        match completion {
            Ok(summary) if !summary.trim().is_empty() => Ok(summary.trim().to_string()),
            Ok(_) => {
                let kind = ProviderErrorKind::EmptyResponse;
                Err(self
                    .reply(
                        question,
                        model_failure_message(kind),
                        Outcome::ModelFailed(kind),
                    )
                    .await)
            }
            Err(e) => {
                let now_ms = u64::try_from(now.timestamp_millis()).unwrap_or(0);
                let kind = classify(&e, None, now_ms).kind;
                tracing::warn!(target: "shodh::slack", kind = ?kind, "the model call for a Slack thread summary failed");
                Err(self
                    .reply(
                        question,
                        model_failure_message(kind),
                        Outcome::ModelFailed(kind),
                    )
                    .await)
            }
        }
    }

    /// Tells the asker an answer is already in flight in this thread (nothing where Shodh
    /// does not answer: the question in flight gets the refusal).
    pub async fn busy(&self, question: &Question) -> Outcome {
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        if let Access::Refused(refusal) = self.gate.check(question).await {
            return Outcome::NotAllowed(refusal);
        }
        self.reply(question, BUSY, Outcome::Busy).await
    }

    /// Tells the asker too many questions are waiting (nothing where Shodh does not
    /// answer).
    pub async fn overloaded(&self, question: &Question) -> Outcome {
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        if let Access::Refused(refusal) = self.gate.check(question).await {
            return Outcome::NotAllowed(refusal);
        }
        self.reply(question, OVERLOADED, Outcome::Overloaded).await
    }
}

#[cfg(test)]
pub(crate) mod testkit {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// A Slack that records what was posted and edited.
    #[derive(Default)]
    pub struct RecordingSlack {
        pub messages: Mutex<Vec<(String, Option<String>, String)>>,
        pub edits: Mutex<HashMap<String, String>>,
    }

    impl RecordingSlack {
        /// Every message as it reads now (edits applied), in order.
        pub fn texts(&self) -> Vec<String> {
            let edits = self.edits.lock().unwrap();
            self.messages
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, (_, _, text))| edits.get(&i.to_string()).cloned().unwrap_or(text.clone()))
                .collect()
        }
    }

    #[async_trait]
    impl SlackPoster for RecordingSlack {
        async fn post(
            &self,
            channel: &str,
            thread_ts: Option<&str>,
            text: &str,
        ) -> Result<String, SlackApiError> {
            let mut messages = self.messages.lock().unwrap();
            messages.push((channel.into(), thread_ts.map(str::to_string), text.into()));
            Ok((messages.len() - 1).to_string())
        }
        async fn update(&self, _channel: &str, ts: &str, text: &str) -> Result<(), SlackApiError> {
            self.edits.lock().unwrap().insert(ts.into(), text.into());
            Ok(())
        }
    }

    /// A model that records its prompts and answers with `reply`.
    pub struct RecordingModel {
        pub prompts: Mutex<Vec<String>>,
        pub reply: Result<String, String>,
    }

    impl RecordingModel {
        pub fn answering(reply: &str) -> Self {
            Self {
                prompts: Mutex::new(Vec::new()),
                reply: Ok(reply.into()),
            }
        }
        pub fn failing(error: &str) -> Self {
            Self {
                prompts: Mutex::new(Vec::new()),
                reply: Err(error.into()),
            }
        }
        pub fn calls(&self) -> usize {
            self.prompts.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl TextModel for RecordingModel {
        fn model_id(&self) -> String {
            "test-model".into()
        }
        async fn complete(&self, prompt: &str, _max_tokens: usize) -> Result<String, String> {
            self.prompts.lock().unwrap().push(prompt.into());
            self.reply.clone()
        }
    }

    /// A quota in memory.
    pub struct MemoryQuota {
        pub cap: usize,
        pub used: Mutex<HashMap<NaiveDate, usize>>,
    }

    impl MemoryQuota {
        pub fn new(cap: usize) -> Self {
            Self {
                cap,
                used: Mutex::new(HashMap::new()),
            }
        }
    }

    impl AnswerQuota for MemoryQuota {
        fn cap(&self) -> usize {
            self.cap
        }
        fn used(&self, day: NaiveDate) -> Result<usize, String> {
            Ok(*self.used.lock().unwrap().get(&day).unwrap_or(&0))
        }
        fn try_reserve(&self, day: NaiveDate) -> Result<bool, String> {
            let mut used = self.used.lock().unwrap();
            let n = used.entry(day).or_insert(0);
            if *n >= self.cap {
                return Ok(false);
            }
            *n += 1;
            Ok(true)
        }
        fn release(&self, day: NaiveDate) -> Result<(), String> {
            let mut used = self.used.lock().unwrap();
            if let Some(n) = used.get_mut(&day) {
                *n = n.saturating_sub(1);
            }
            Ok(())
        }
    }

    pub struct FixedClock(pub DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// A clock that reads the given times in turn, then stays at the last.
    pub struct StepClock(pub Mutex<std::collections::VecDeque<DateTime<Utc>>>);

    impl StepClock {
        pub fn new(times: &[DateTime<Utc>]) -> Self {
            Self(Mutex::new(times.iter().copied().collect()))
        }
    }

    impl Clock for StepClock {
        fn now(&self) -> DateTime<Utc> {
            let mut times = self.0.lock().unwrap();
            if times.len() > 1 {
                times.pop_front().unwrap()
            } else {
                *times.front().unwrap()
            }
        }
    }

    /// The gate most tests use: mentions in `C1`, DMs from `U1` of `T1`.
    pub fn open_gate() -> Arc<dyn AccessGate> {
        Arc::new(crate::connectors::slack::access::testkit::open_gate())
    }

    /// Memories from Slack, recorded: the owner's and the channels'.
    pub struct RecordingMemories {
        pub owner: Option<String>,
        pub thread: Result<String, String>,
        pub owners: Mutex<Vec<MemoryDraft>>,
        pub channels: Mutex<Vec<MemoryDraft>>,
    }

    impl RecordingMemories {
        pub fn owned_by(owner: Option<&str>) -> Self {
            Self {
                owner: owner.map(str::to_string),
                thread: Ok("Ann: the budget is ten.\nBob: agreed, due Friday.".into()),
                owners: Mutex::new(Vec::new()),
                channels: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl SlackMemories for RecordingMemories {
        async fn owner(&self) -> Result<Option<String>, String> {
            Ok(self.owner.clone())
        }
        async fn channel_name(&self, channel: &str, direct: bool) -> String {
            if direct {
                "a direct message".into()
            } else {
                format!("#{}", channel.to_lowercase())
            }
        }
        async fn permalink(&self, channel: &str, ts: &str) -> Option<String> {
            Some(format!(
                "https://acme.slack.com/archives/{channel}/p{}",
                ts.replace('.', "")
            ))
        }
        async fn thread_text(&self, _channel: &str, _thread_ts: &str) -> Result<String, String> {
            self.thread.clone()
        }
        async fn save_owner(&self, draft: &MemoryDraft) -> Result<(), String> {
            self.owners.lock().unwrap().push(draft.clone());
            Ok(())
        }
        async fn save_channel(&self, draft: &MemoryDraft) -> Result<(), String> {
            self.channels.lock().unwrap().push(draft.clone());
            Ok(())
        }
    }

    /// Memories of a Slack with no owner picked.
    pub fn memories() -> Arc<RecordingMemories> {
        Arc::new(RecordingMemories::owned_by(None))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use chrono::TimeZone;

    use super::testkit::*;
    use super::*;
    use crate::connectors::slack::events::{QuestionKind, ThreadKey};

    /// A source with fixed passages (or none selected).
    struct FixedSource {
        scope: Option<AnswerScope>,
        passages: Vec<AnswerPassage>,
        model: Arc<RecordingModel>,
        searches: Mutex<usize>,
        scope_reads: Mutex<usize>,
        /// Cancelled during the search (the connector was switched off meanwhile).
        cancel_on_search: Option<CancellationToken>,
        listing: LibraryListing,
        listed: Mutex<Vec<AnswerScope>>,
        searched: Mutex<Vec<(AnswerScope, String)>>,
        turns: Mutex<std::collections::HashMap<ThreadKey, Vec<ThreadTurn>>>,
        notes: std::collections::HashMap<String, Vec<SlackMemory>>,
    }

    #[async_trait]
    impl AnswerSource for FixedSource {
        async fn scope(&self) -> Result<Option<AnswerScope>, String> {
            *self.scope_reads.lock().unwrap() += 1;
            Ok(self.scope.clone())
        }
        async fn retrieve(
            &self,
            scope: &AnswerScope,
            q: &str,
        ) -> Result<Vec<AnswerPassage>, String> {
            *self.searches.lock().unwrap() += 1;
            self.searched
                .lock()
                .unwrap()
                .push((scope.clone(), q.to_string()));
            if let Some(cancel) = &self.cancel_on_search {
                cancel.cancel();
            }
            Ok(self.passages.clone())
        }
        async fn listing(&self, scope: &AnswerScope) -> Result<LibraryListing, String> {
            self.listed.lock().unwrap().push(scope.clone());
            Ok(self.listing.clone())
        }
        async fn thread_turns(&self, thread: &ThreadKey) -> Result<Vec<ThreadTurn>, String> {
            Ok(self
                .turns
                .lock()
                .unwrap()
                .get(thread)
                .cloned()
                .unwrap_or_default())
        }
        async fn record_turn(&self, thread: &ThreadKey, turn: ThreadTurn) -> Result<(), String> {
            self.turns
                .lock()
                .unwrap()
                .entry(thread.clone())
                .or_default()
                .push(turn);
            Ok(())
        }
        async fn channel_memories(&self, channel: &str) -> Result<Vec<SlackMemory>, String> {
            Ok(self.notes.get(channel).cloned().unwrap_or_default())
        }
        fn model(&self) -> Result<Arc<dyn TextModel>, String> {
            Ok(self.model.clone())
        }
        fn private_terms(&self) -> PrivateTerms {
            PrivateTerms::default()
        }
    }

    fn question(text: &str) -> Question {
        Question {
            event_id: "Ev1".into(),
            kind: QuestionKind::Mention,
            channel: "C1".into(),
            reply_thread_ts: Some("100.1".into()),
            thread: ThreadKey {
                channel: "C1".into(),
                thread: "100.1".into(),
            },
            user: "U1".into(),
            ts: "100.1".into(),
            in_thread: false,
            asker_team: None,
            ext_shared: false,
            has_files: false,
            text: text.into(),
        }
    }

    fn passage() -> AnswerPassage {
        AnswerPassage {
            n: 1,
            title: "Lease".into(),
            page: Some("3".into()),
            text: "The notice period is sixty days.".into(),
        }
    }

    struct Setup {
        responder: Responder,
        slack: Arc<RecordingSlack>,
        model: Arc<RecordingModel>,
        quota: Arc<MemoryQuota>,
        source: Arc<FixedSource>,
    }

    fn noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 23, 18, 30, 0).unwrap()
    }

    fn setup_with(
        scope: Option<AnswerScope>,
        passages: Vec<AnswerPassage>,
        model: RecordingModel,
        cap: usize,
        cancel: CancellationToken,
        cancel_on_search: bool,
    ) -> Setup {
        let slack = Arc::new(RecordingSlack::default());
        let model = Arc::new(model);
        let quota = Arc::new(MemoryQuota::new(cap));
        let source = Arc::new(FixedSource {
            scope,
            passages,
            model: model.clone(),
            searches: Mutex::new(0),
            scope_reads: Mutex::new(0),
            cancel_on_search: cancel_on_search.then(|| cancel.clone()),
            listing: LibraryListing::default(),
            listed: Mutex::new(Vec::new()),
            searched: Mutex::new(Vec::new()),
            turns: Mutex::new(std::collections::HashMap::new()),
            notes: std::collections::HashMap::new(),
        });
        Setup {
            responder: Responder::new(
                source.clone(),
                memories(),
                open_gate(),
                quota.clone(),
                slack.clone(),
                Arc::new(FixedClock(noon())),
                cancel,
            ),
            slack,
            model,
            quota,
            source,
        }
    }

    fn setup(
        scope: Option<AnswerScope>,
        passages: Vec<AnswerPassage>,
        model: RecordingModel,
        cap: usize,
    ) -> Setup {
        setup_with(scope, passages, model, cap, CancellationToken::new(), false)
    }

    fn scope() -> Option<AnswerScope> {
        Some(AnswerScope {
            source_ids: vec!["space-a".into()],
            files: vec![],
            ..AnswerScope::default()
        })
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    #[tokio::test]
    async fn without_a_selected_workspace_nothing_is_searched_or_spent() {
        let s = setup(None, vec![passage()], RecordingModel::answering("x [1]"), 5);
        assert_eq!(
            s.responder.respond(&question("notice period?")).await,
            Outcome::NotSetUp
        );
        assert_eq!(s.slack.texts(), vec![NOT_SET_UP.to_string()]);
        assert_eq!(s.model.calls(), 0);
        assert_eq!(*s.source.searches.lock().unwrap(), 0);
        assert!(s.quota.used.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_question_from_where_shodh_does_not_answer_costs_nothing() {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("x [1]"),
            5,
        );
        let mut elsewhere = question("notice period?");
        elsewhere.channel = "C9".into();
        assert_eq!(
            s.responder.respond(&elsewhere).await,
            Outcome::NotAllowed(Refusal::ChannelNotAllowed)
        );
        let mut external_dm = question("notice period?");
        external_dm.kind = QuestionKind::Direct;
        external_dm.channel = "D1".into();
        external_dm.user = "UEXT".into();
        assert_eq!(
            s.responder.respond(&external_dm).await,
            Outcome::NotAllowed(Refusal::AskerUnknown)
        );
        assert_eq!(
            s.slack.texts(),
            vec![NOT_ALLOWED.to_string(), NOT_ALLOWED.to_string()]
        );
        assert_eq!(*s.source.scope_reads.lock().unwrap(), 0, "decided first");
        assert_eq!(*s.source.searches.lock().unwrap(), 0);
        assert_eq!(s.model.calls(), 0);
        assert!(s.quota.used.lock().unwrap().is_empty());
        // Busy and overloaded replies say nothing there either.
        assert_eq!(
            s.responder.busy(&elsewhere).await,
            Outcome::NotAllowed(Refusal::ChannelNotAllowed)
        );
        assert_eq!(
            s.responder.overloaded(&elsewhere).await,
            Outcome::NotAllowed(Refusal::ChannelNotAllowed)
        );
        assert_eq!(s.slack.texts().len(), 2);
    }

    #[tokio::test]
    async fn an_answer_replaces_the_placeholder_in_the_thread_and_is_counted() {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("It is **sixty days** [1]."),
            5,
        );
        assert_eq!(
            s.responder.respond(&question("notice period?")).await,
            Outcome::Answered
        );
        let messages = s.slack.messages.lock().unwrap().clone();
        assert_eq!(messages.len(), 1, "one message: the placeholder, edited");
        assert_eq!(
            messages[0].1.as_deref(),
            Some("100.1"),
            "in the question's thread"
        );
        assert_eq!(messages[0].2, PLACEHOLDER);
        let text = s.slack.texts()[0].clone();
        assert!(text.starts_with("It is *sixty days* [1]."), "{text}");
        assert!(text.contains("[1] Lease, p. 3"), "{text}");
        assert_eq!(s.model.calls(), 1);
        assert_eq!(s.quota.used(day(23)).unwrap(), 1);
    }

    #[tokio::test]
    async fn at_the_cap_no_model_call_is_made() {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("x [1]"),
            1,
        );
        assert_eq!(
            s.responder.respond(&question("one?")).await,
            Outcome::Answered
        );
        assert_eq!(s.responder.respond(&question("two?")).await, Outcome::AtCap);
        assert_eq!(s.model.calls(), 1);
        let texts = s.slack.texts();
        assert_eq!(
            texts[1],
            "Shodh has used today's answer limit; it resets at 00:00 UTC."
        );
        assert_eq!(
            *s.source.searches.lock().unwrap(),
            1,
            "no search at the cap either"
        );
    }

    #[tokio::test]
    async fn the_cap_is_rechecked_when_the_call_is_reserved() {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("x [1]"),
            1,
        );
        // Another answer took the last call while this one was searching.
        struct TakeLast(Arc<MemoryQuota>, NaiveDate);
        #[async_trait]
        impl AnswerSource for TakeLast {
            async fn scope(&self) -> Result<Option<AnswerScope>, String> {
                Ok(Some(AnswerScope::default()))
            }
            async fn retrieve(
                &self,
                _s: &AnswerScope,
                _q: &str,
            ) -> Result<Vec<AnswerPassage>, String> {
                assert!(self.0.try_reserve(self.1).unwrap());
                Ok(vec![passage()])
            }
            async fn listing(&self, _s: &AnswerScope) -> Result<LibraryListing, String> {
                Ok(LibraryListing::default())
            }
            async fn thread_turns(&self, _t: &ThreadKey) -> Result<Vec<ThreadTurn>, String> {
                Ok(Vec::new())
            }
            async fn record_turn(&self, _t: &ThreadKey, _turn: ThreadTurn) -> Result<(), String> {
                Ok(())
            }
            async fn channel_memories(&self, _c: &str) -> Result<Vec<SlackMemory>, String> {
                Ok(Vec::new())
            }
            fn model(&self) -> Result<Arc<dyn TextModel>, String> {
                Ok(Arc::new(RecordingModel::failing("never called")))
            }
            fn private_terms(&self) -> PrivateTerms {
                PrivateTerms::default()
            }
        }
        let responder = Responder {
            source: Arc::new(TakeLast(s.quota.clone(), day(23))),
            ..s.responder
        };
        assert_eq!(responder.respond(&question("q?")).await, Outcome::AtCap);
        assert_eq!(
            s.slack.texts(),
            vec!["Shodh has used today's answer limit; it resets at 00:00 UTC.".to_string()]
        );
        assert_eq!(
            s.quota.used(day(23)).unwrap(),
            1,
            "the refused call is not counted"
        );
    }

    /// A question that arrives just before 00:00 UTC and whose call is made just after
    /// counts on the new day, against the new day's cap.
    #[tokio::test]
    async fn the_call_counts_on_the_utc_day_it_is_made() {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("Sixty days [1]."),
            1,
        );
        // The old day's cap is used up; the new day's is not.
        assert!(s.quota.try_reserve(day(23)).unwrap());
        let responder = Responder {
            clock: Arc::new(StepClock::new(&[
                Utc.with_ymd_and_hms(2026, 10, 23, 23, 59, 58).unwrap(),
                Utc.with_ymd_and_hms(2026, 10, 24, 0, 0, 3).unwrap(),
            ])),
            ..s.responder
        };
        // Asked at 23:59:58 with the old day at its cap: refused on the old day.
        assert_eq!(responder.respond(&question("q?")).await, Outcome::AtCap);
        assert_eq!(s.model.calls(), 0);

        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("Sixty days [1]."),
            1,
        );
        let responder = Responder {
            clock: Arc::new(StepClock::new(&[
                Utc.with_ymd_and_hms(2026, 10, 23, 23, 59, 58).unwrap(),
                Utc.with_ymd_and_hms(2026, 10, 24, 0, 0, 3).unwrap(),
            ])),
            ..s.responder
        };
        assert_eq!(responder.respond(&question("q?")).await, Outcome::Answered);
        assert_eq!(s.quota.used(day(23)).unwrap(), 0, "not the old day");
        assert_eq!(s.quota.used(day(24)).unwrap(), 1, "the day of the call");
    }

    #[tokio::test]
    async fn nothing_found_and_empty_questions_cost_nothing() {
        let s = setup(scope(), vec![], RecordingModel::answering("x"), 5);
        assert_eq!(
            s.responder.respond(&question("unknown?")).await,
            Outcome::NothingFound
        );
        assert_eq!(
            s.responder.respond(&question("   ")).await,
            Outcome::EmptyQuestion
        );
        let mut only_a_file = question("");
        only_a_file.has_files = true;
        assert_eq!(s.responder.respond(&only_a_file).await, Outcome::FilesOnly);
        assert_eq!(s.model.calls(), 0);
        assert!(s.quota.used.lock().unwrap().is_empty());
        assert_eq!(
            s.slack.texts(),
            vec![
                NOTHING_FOUND.to_string(),
                EMPTY_QUESTION.to_string(),
                FILES_ONLY.to_string()
            ]
        );
    }

    #[tokio::test]
    async fn model_failures_become_short_honest_replies() {
        for (error, expected) in [
            (
                "HTTP 429 Too Many Requests: rate limit exceeded",
                ProviderErrorKind::RateLimited,
            ),
            (
                "HTTP 401 Unauthorized: invalid api key",
                ProviderErrorKind::Auth,
            ),
            (
                "HTTP 402: insufficient credits",
                ProviderErrorKind::QuotaExhausted,
            ),
        ] {
            let s = setup(scope(), vec![passage()], RecordingModel::failing(error), 5);
            let outcome = s.responder.respond(&question("q?")).await;
            let Outcome::ModelFailed(kind) = outcome else {
                panic!("{error}: {outcome:?}")
            };
            assert_eq!(kind, expected, "{error}");
            let text = s.slack.texts()[0].clone();
            assert_eq!(text, model_failure_message(kind));
            assert!(!text.contains("HTTP"), "no details in Slack");
        }
    }

    #[test]
    fn every_failure_kind_has_a_reply() {
        for kind in [
            ProviderErrorKind::RateLimited,
            ProviderErrorKind::QuotaExhausted,
            ProviderErrorKind::ModelUnavailable,
            ProviderErrorKind::Auth,
            ProviderErrorKind::Timeout,
            ProviderErrorKind::Network,
            ProviderErrorKind::InvalidRequest,
            ProviderErrorKind::ContextTooLong,
            ProviderErrorKind::ContentFiltered,
            ProviderErrorKind::EmptyResponse,
            ProviderErrorKind::Other,
        ] {
            assert!(!model_failure_message(kind).is_empty());
        }
    }

    // ── The library, follow-ups, Slack threads and notes ──────────

    /// A setup whose source is changed by `change` before use.
    fn setup_custom(change: impl FnOnce(&mut FixedSource)) -> Setup {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("Sixty days [1]."),
            5,
        );
        let mut source = FixedSource {
            scope: s.source.scope.clone(),
            passages: s.source.passages.clone(),
            model: s.model.clone(),
            searches: Mutex::new(0),
            scope_reads: Mutex::new(0),
            cancel_on_search: None,
            listing: LibraryListing::default(),
            listed: Mutex::new(Vec::new()),
            searched: Mutex::new(Vec::new()),
            turns: Mutex::new(std::collections::HashMap::new()),
            notes: std::collections::HashMap::new(),
        };
        change(&mut source);
        let source = Arc::new(source);
        Setup {
            responder: Responder {
                source: source.clone(),
                ..s.responder
            },
            source,
            ..s
        }
    }

    #[tokio::test]
    async fn a_question_about_the_library_is_answered_from_its_list_with_no_model_call() {
        let s = setup_custom(|source| {
            source.listing = LibraryListing {
                titles: vec!["Lease".into(), "Loan".into()],
                slack_threads: 0,
            };
        });
        assert_eq!(
            s.responder
                .respond(&question("What files do you have?"))
                .await,
            Outcome::Listed
        );
        assert_eq!(
            s.slack.texts(),
            vec!["Shodh answers from here with 2 documents:\n• Lease\n• Loan".to_string()]
        );
        assert_eq!(s.model.calls(), 0);
        assert_eq!(*s.source.searches.lock().unwrap(), 0, "not a search");
        assert!(s.quota.used.lock().unwrap().is_empty(), "no answer spent");
        // A question about a topic is a normal question.
        assert_eq!(
            s.responder
                .respond(&question("list the files about rent"))
                .await,
            Outcome::Answered
        );
        assert_eq!(s.model.calls(), 1);
    }

    fn in_thread(text: &str, root: &str) -> Question {
        let mut q = question(text);
        q.reply_thread_ts = Some(root.into());
        q.thread.thread = root.into();
        q.ts = "300.1".into();
        q.in_thread = true;
        q
    }

    #[tokio::test]
    async fn a_follow_up_sees_its_own_thread_and_never_another() {
        let thread = |root: &str| ThreadKey {
            channel: "C1".into(),
            thread: root.into(),
        };
        let s = setup_custom(|source| {
            let mut turns = std::collections::HashMap::new();
            turns.insert(
                thread("100.1"),
                vec![ThreadTurn {
                    question: "What is the notice period?".into(),
                    answer: "Sixty days [1].".into(),
                }],
            );
            turns.insert(
                thread("200.1"),
                vec![ThreadTurn {
                    question: "What about the walrus clause?".into(),
                    answer: "The walrus clause says ninety.".into(),
                }],
            );
            source.turns = Mutex::new(turns);
        });
        assert_eq!(
            s.responder.respond(&in_thread("shorter", "100.1")).await,
            Outcome::Answered
        );
        let prompt = s.model.prompts.lock().unwrap()[0].clone();
        assert!(prompt.contains("Earlier in this Slack thread"), "{prompt}");
        assert!(prompt.contains("What is the notice period?"));
        assert!(
            !prompt.to_lowercase().contains("walrus"),
            "another thread: {prompt}"
        );
        assert_eq!(
            s.source.searched.lock().unwrap()[0].1,
            "What is the notice period? shorter",
            "the search keeps the thread's topic"
        );
        let kept = s
            .source
            .turns
            .lock()
            .unwrap()
            .get(&thread("100.1"))
            .cloned()
            .unwrap();
        assert_eq!(kept.len(), 2, "the answer is kept for the next follow-up");
        assert_eq!(kept[1].question, "shorter");
        assert!(kept[1].answer.starts_with("Sixty days [1]."));
        assert_eq!(
            s.source
                .turns
                .lock()
                .unwrap()
                .get(&thread("200.1"))
                .unwrap()
                .len(),
            1,
            "the other thread is untouched"
        );
        // A new thread starts with no earlier turns.
        s.responder
            .respond(&in_thread("and the rent?", "400.1"))
            .await;
        let prompt = s.model.prompts.lock().unwrap()[1].clone();
        assert!(!prompt.contains("Earlier in this Slack thread"), "{prompt}");
        assert_eq!(s.source.searched.lock().unwrap()[1].1, "and the rent?");
    }

    #[tokio::test]
    async fn a_slack_answer_searches_only_the_slack_channel_it_was_asked_in() {
        let s = setup_custom(|source| {
            source.scope = Some(AnswerScope {
                source_ids: vec![
                    "space-a".into(),
                    crate::connectors::slack::ids::space_id("T1", "C1"),
                    crate::connectors::slack::ids::space_id("T1", "C2"),
                ],
                files: vec![],
                ..AnswerScope::default()
            });
        });
        s.responder.respond(&question("notice period?")).await;
        let mut dm = question("notice period?");
        dm.kind = QuestionKind::Direct;
        dm.channel = "D1".into();
        dm.thread.channel = "D1".into();
        s.responder.respond(&dm).await;
        let searched = s.source.searched.lock().unwrap().clone();
        assert_eq!(
            searched[0].0.source_ids,
            vec![
                "space-a".to_string(),
                crate::connectors::slack::ids::space_id("T1", "C1")
            ],
            "the channel asked in, never the other one"
        );
        assert_eq!(
            searched[1].0.source_ids,
            vec!["space-a".to_string()],
            "a DM gets no Slack channel"
        );
    }

    fn note(channel: &str, text: &str) -> SlackMemory {
        SlackMemory {
            id: format!("n-{channel}"),
            team_id: "T1".into(),
            channel_id: channel.into(),
            channel_name: format!("#{channel}"),
            author_id: "U2".into(),
            author_name: "Bob".into(),
            text: text.into(),
            kind: SlackMemoryKind::Text,
            message_ts: "1.1".into(),
            permalink: None,
            saved_at: noon(),
            owner: false,
            memory_id: None,
        }
    }

    #[tokio::test]
    async fn what_people_saved_for_a_channel_is_used_there_only() {
        let s = setup_custom(|source| {
            source.passages = vec![];
            source
                .notes
                .insert("C1".into(), vec![note("C1", "Standup moved to 10:30.")]);
            source
                .notes
                .insert("D1".into(), vec![note("D1", "The quokka code is 7.")]);
        });
        // No document matched, but the channel's note answers.
        assert_eq!(
            s.responder.respond(&question("when is standup?")).await,
            Outcome::Answered
        );
        let prompt = s.model.prompts.lock().unwrap()[0].clone();
        assert!(
            prompt.contains("[1] Saved in this channel by Bob on 2026-10-23"),
            "{prompt}"
        );
        assert!(prompt.contains("Standup moved to 10:30."));
        assert!(!prompt.contains("quokka"), "another conversation's note");
    }

    // ── Remember ───────────────────────────────────────────────────

    fn remembering(owner: Option<&str>) -> (Setup, Arc<RecordingMemories>) {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("Budget is ten, due Friday."),
            5,
        );
        let memories = Arc::new(RecordingMemories::owned_by(owner));
        let responder = Responder {
            memories: memories.clone(),
            ..s.responder
        };
        (Setup { responder, ..s }, memories)
    }

    #[tokio::test]
    async fn the_owner_saves_into_their_memories_at_once_with_provenance() {
        let (s, memories) = remembering(Some("U1"));
        assert_eq!(
            s.responder
                .respond(&question("remember: the budget review is on Friday"))
                .await,
            Outcome::Saved { owner: true }
        );
        assert_eq!(s.slack.texts(), vec![SAVED.to_string()]);
        let saved = memories.owners.lock().unwrap().clone();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].text, "the budget review is on Friday");
        assert_eq!(saved[0].author_name, "Ann");
        assert_eq!(saved[0].author_id, "U1");
        assert_eq!(saved[0].channel_name, "#c1");
        assert_eq!(saved[0].message_ts, "100.1");
        assert_eq!(
            saved[0].permalink.as_deref(),
            Some("https://acme.slack.com/archives/C1/p1001")
        );
        assert_eq!(saved[0].kind, SlackMemoryKind::Text);
        assert!(memories.channels.lock().unwrap().is_empty());
        assert_eq!(s.model.calls(), 0, "explicit text needs no model");
        assert!(s.quota.used.lock().unwrap().is_empty());
        assert_eq!(*s.source.searches.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn anyone_else_saves_for_the_channel_only() {
        let (s, memories) = remembering(Some("UOWNER"));
        assert_eq!(
            s.responder
                .respond(&question("remember: standup is at 10"))
                .await,
            Outcome::Saved { owner: false }
        );
        assert_eq!(s.slack.texts(), vec![SAVED_FOR_CHANNEL.to_string()]);
        assert!(
            memories.owners.lock().unwrap().is_empty(),
            "never the user's memories"
        );
        assert_eq!(memories.channels.lock().unwrap()[0].channel, "C1");
        // With no owner picked, nobody is the owner.
        let (s, memories) = remembering(None);
        assert_eq!(
            s.responder.respond(&question("remember: x")).await,
            Outcome::Saved { owner: false }
        );
        assert_eq!(memories.channels.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn people_from_elsewhere_and_other_channels_cannot_save() {
        let (s, memories) = remembering(Some("U1"));
        // Not a member Slack can vouch for (another organization in an allowed channel).
        let mut external = question("remember: the client owes us nothing");
        external.user = "UEXT".into();
        assert_eq!(
            s.responder.respond(&external).await,
            Outcome::NotAllowed(Refusal::AskerUnknown)
        );
        let mut other_team = question("remember: x");
        other_team.asker_team = Some("TCLIENT".into());
        assert_eq!(
            s.responder.respond(&other_team).await,
            Outcome::NotAllowed(Refusal::OtherTeam)
        );
        // A channel not on the list.
        let mut elsewhere = question("remember: x");
        elsewhere.channel = "C9".into();
        assert_eq!(
            s.responder.respond(&elsewhere).await,
            Outcome::NotAllowed(Refusal::ChannelNotAllowed)
        );
        assert_eq!(s.slack.texts(), vec![NOT_ALLOWED.to_string(); 3]);
        assert!(memories.owners.lock().unwrap().is_empty());
        assert!(memories.channels.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn remembering_a_thread_costs_one_counted_call_and_needs_a_thread() {
        let (s, memories) = remembering(Some("U1"));
        assert_eq!(
            s.responder.respond(&question("remember this thread")).await,
            Outcome::NothingToSave
        );
        assert_eq!(
            s.responder.respond(&question("remember:")).await,
            Outcome::NothingToSave
        );
        assert_eq!(s.model.calls(), 0);
        assert_eq!(
            s.responder
                .respond(&in_thread("remember this thread", "100.1"))
                .await,
            Outcome::Saved { owner: true }
        );
        assert_eq!(s.model.calls(), 1);
        assert_eq!(s.quota.used(day(23)).unwrap(), 1, "counted against the cap");
        let prompt = s.model.prompts.lock().unwrap()[0].clone();
        assert!(prompt.contains("Ann: the budget is ten."), "{prompt}");
        assert!(prompt.contains("untrusted text"));
        let saved = memories.owners.lock().unwrap()[0].clone();
        assert_eq!(saved.text, "Budget is ten, due Friday.");
        assert_eq!(saved.kind, SlackMemoryKind::Thread);
        assert_eq!(saved.message_ts, "300.1");
        assert_eq!(
            s.slack.texts(),
            vec![
                NOT_A_THREAD.to_string(),
                NOTHING_TO_SAVE.to_string(),
                SAVED.to_string()
            ]
        );
    }

    #[tokio::test]
    async fn at_the_cap_a_thread_is_not_summarized() {
        let s = setup(scope(), vec![passage()], RecordingModel::answering("x"), 1);
        assert!(s.quota.try_reserve(day(23)).unwrap());
        let memories = Arc::new(RecordingMemories::owned_by(Some("U1")));
        let responder = Responder {
            memories: memories.clone(),
            ..s.responder
        };
        assert_eq!(
            responder
                .respond(&in_thread("remember this thread", "100.1"))
                .await,
            Outcome::AtCap
        );
        assert_eq!(s.model.calls(), 0);
        assert!(memories.owners.lock().unwrap().is_empty());
    }

    // ── Cancellation ───────────────────────────────────────────────

    #[tokio::test]
    async fn a_cancelled_answer_posts_nothing() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let s = setup_with(
            scope(),
            vec![passage()],
            RecordingModel::answering("x [1]"),
            5,
            cancel,
            false,
        );
        assert_eq!(
            s.responder.respond(&question("q?")).await,
            Outcome::Cancelled
        );
        assert_eq!(s.responder.busy(&question("q?")).await, Outcome::Cancelled);
        assert!(s.slack.texts().is_empty());
        assert_eq!(*s.source.searches.lock().unwrap(), 0);
        assert_eq!(s.model.calls(), 0);
    }

    #[tokio::test]
    async fn switched_off_during_the_search_no_call_is_made_or_counted() {
        let s = setup_with(
            scope(),
            vec![passage()],
            RecordingModel::answering("x [1]"),
            5,
            CancellationToken::new(),
            true,
        );
        assert_eq!(
            s.responder.respond(&question("q?")).await,
            Outcome::Cancelled
        );
        assert_eq!(s.model.calls(), 0);
        assert_eq!(s.quota.used(day(23)).unwrap(), 0);
        assert_eq!(
            s.slack.texts(),
            vec![PLACEHOLDER.to_string()],
            "nothing after the placeholder"
        );
    }

    /// A quota that cancels the connector as it reserves: the reservation must come back.
    struct CancellingQuota(MemoryQuota, CancellationToken);

    impl AnswerQuota for CancellingQuota {
        fn cap(&self) -> usize {
            self.0.cap()
        }
        fn used(&self, day: NaiveDate) -> Result<usize, String> {
            self.0.used(day)
        }
        fn try_reserve(&self, day: NaiveDate) -> Result<bool, String> {
            let reserved = self.0.try_reserve(day);
            self.1.cancel();
            reserved
        }
        fn release(&self, day: NaiveDate) -> Result<(), String> {
            self.0.release(day)
        }
    }

    #[tokio::test]
    async fn a_reservation_without_a_call_is_given_back() {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("x [1]"),
            5,
        );
        let cancel = s.responder.cancel.clone();
        let quota = Arc::new(CancellingQuota(MemoryQuota::new(5), cancel));
        let responder = Responder {
            quota: quota.clone(),
            ..s.responder
        };
        assert_eq!(responder.respond(&question("q?")).await, Outcome::Cancelled);
        assert_eq!(s.model.calls(), 0);
        assert_eq!(quota.used(day(23)).unwrap(), 0, "given back");
        assert_eq!(s.slack.texts(), vec![PLACEHOLDER.to_string()]);
    }

    /// A model that switches the connector off while it answers.
    struct CancellingModel(CancellationToken, Mutex<usize>);

    #[async_trait]
    impl TextModel for CancellingModel {
        fn model_id(&self) -> String {
            "test-model".into()
        }
        async fn complete(&self, _prompt: &str, _max_tokens: usize) -> Result<String, String> {
            *self.1.lock().unwrap() += 1;
            self.0.cancel();
            Ok("Sixty days [1].".into())
        }
    }

    #[tokio::test]
    async fn switched_off_during_the_model_call_the_answer_is_not_posted() {
        let s = setup(scope(), vec![passage()], RecordingModel::answering("x"), 5);
        let model = Arc::new(CancellingModel(s.responder.cancel.clone(), Mutex::new(0)));
        struct WithModel(Arc<CancellingModel>);
        #[async_trait]
        impl AnswerSource for WithModel {
            async fn scope(&self) -> Result<Option<AnswerScope>, String> {
                Ok(Some(AnswerScope::default()))
            }
            async fn retrieve(
                &self,
                _s: &AnswerScope,
                _q: &str,
            ) -> Result<Vec<AnswerPassage>, String> {
                Ok(vec![passage()])
            }
            async fn listing(&self, _s: &AnswerScope) -> Result<LibraryListing, String> {
                Ok(LibraryListing::default())
            }
            async fn thread_turns(&self, _t: &ThreadKey) -> Result<Vec<ThreadTurn>, String> {
                Ok(Vec::new())
            }
            async fn record_turn(&self, _t: &ThreadKey, _turn: ThreadTurn) -> Result<(), String> {
                Ok(())
            }
            async fn channel_memories(&self, _c: &str) -> Result<Vec<SlackMemory>, String> {
                Ok(Vec::new())
            }
            fn model(&self) -> Result<Arc<dyn TextModel>, String> {
                Ok(self.0.clone())
            }
            fn private_terms(&self) -> PrivateTerms {
                PrivateTerms::default()
            }
        }
        let responder = Responder {
            source: Arc::new(WithModel(model.clone())),
            ..s.responder
        };
        assert_eq!(responder.respond(&question("q?")).await, Outcome::Cancelled);
        assert_eq!(*model.1.lock().unwrap(), 1);
        assert_eq!(s.quota.used(day(23)).unwrap(), 1, "the call was made");
        assert_eq!(s.slack.texts(), vec![PLACEHOLDER.to_string()]);
    }

    // ── Slack refusing a post ──────────────────────────────────────

    /// A Slack whose posts and edits follow scripts (`Some(error)`: refused, `None`: taken),
    /// then are taken; it records like [`RecordingSlack`].
    #[derive(Default)]
    struct FlakySlack {
        inner: RecordingSlack,
        post_script: Mutex<VecDeque<Option<SlackApiError>>>,
        update_script: Mutex<VecDeque<Option<SlackApiError>>>,
        posts: Mutex<usize>,
        updates: Mutex<usize>,
    }

    #[async_trait]
    impl SlackPoster for FlakySlack {
        async fn post(
            &self,
            channel: &str,
            thread_ts: Option<&str>,
            text: &str,
        ) -> Result<String, SlackApiError> {
            *self.posts.lock().unwrap() += 1;
            if let Some(Some(e)) = self.post_script.lock().unwrap().pop_front() {
                return Err(e);
            }
            self.inner.post(channel, thread_ts, text).await
        }
        async fn update(&self, channel: &str, ts: &str, text: &str) -> Result<(), SlackApiError> {
            *self.updates.lock().unwrap() += 1;
            if let Some(Some(e)) = self.update_script.lock().unwrap().pop_front() {
                return Err(e);
            }
            self.inner.update(channel, ts, text).await
        }
    }

    fn flaky(
        posts: Vec<Option<SlackApiError>>,
        updates: Vec<Option<SlackApiError>>,
    ) -> (Responder, Arc<FlakySlack>) {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("Sixty days [1]."),
            5,
        );
        let slack = Arc::new(FlakySlack {
            post_script: Mutex::new(posts.into()),
            update_script: Mutex::new(updates.into()),
            ..FlakySlack::default()
        });
        (
            Responder {
                poster: slack.clone(),
                ..s.responder
            },
            slack,
        )
    }

    fn limited(secs: u64) -> Option<SlackApiError> {
        Some(SlackApiError::RateLimited {
            retry_after: Some(Duration::from_secs(secs)),
        })
    }

    fn refused() -> Option<SlackApiError> {
        Some(SlackApiError::Api {
            code: "channel_not_found".into(),
            needed: None,
        })
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limits_are_waited_out_for_slacks_retry_after() {
        let (responder, slack) = flaky(vec![limited(2)], vec![limited(3), limited(3)]);
        let started = tokio::time::Instant::now();
        assert_eq!(responder.respond(&question("q?")).await, Outcome::Answered);
        assert!(
            started.elapsed() >= Duration::from_secs(8),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(*slack.posts.lock().unwrap(), 2);
        assert_eq!(*slack.updates.lock().unwrap(), 3);
        let texts = slack.inner.texts();
        assert_eq!(
            texts.len(),
            1,
            "the placeholder became the answer: {texts:?}"
        );
        assert!(texts[0].starts_with("Sixty days [1]."));
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limits_are_tried_a_bounded_number_of_times() {
        let tries = RetryPolicy::default().rate_limit_retries as usize + 1;
        let (responder, slack) = flaky(vec![limited(1); tries + 2], vec![]);
        let outcome = responder.respond(&question("q?")).await;
        assert!(matches!(outcome, Outcome::SlackFailed(_)), "{outcome:?}");
        assert_eq!(*slack.posts.lock().unwrap(), tries, "bounded");
        assert_eq!(*slack.updates.lock().unwrap(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn an_edit_slack_keeps_refusing_becomes_a_new_reply() {
        let tries = RetryPolicy::default().rate_limit_retries as usize + 1;
        let (responder, slack) = flaky(vec![], vec![limited(1); tries]);
        assert_eq!(responder.respond(&question("q?")).await, Outcome::Answered);
        assert_eq!(*slack.updates.lock().unwrap(), tries, "bounded");
        let texts = slack.inner.texts();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert_eq!(texts[0], PLACEHOLDER);
        assert!(
            texts[1].starts_with("Sixty days [1]."),
            "the answer as a new reply"
        );
        let messages = slack.inner.messages.lock().unwrap().clone();
        assert_eq!(messages[1].1.as_deref(), Some("100.1"), "in the thread");
    }

    #[tokio::test(start_paused = true)]
    async fn a_network_error_gets_one_more_try_then_a_new_reply() {
        let (responder, slack) = flaky(
            vec![],
            vec![
                Some(SlackApiError::Network("reset".into())),
                Some(SlackApiError::Status(502)),
            ],
        );
        assert_eq!(responder.respond(&question("q?")).await, Outcome::Answered);
        assert_eq!(*slack.updates.lock().unwrap(), 2, "one more try, no more");
        assert_eq!(slack.inner.texts().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_slack_refuses_everywhere_is_reported() {
        // The placeholder goes up after one 500; the edit and the new reply are refused.
        let (responder, slack) = flaky(
            vec![Some(SlackApiError::Status(500)), None, refused()],
            vec![refused()],
        );
        let outcome = responder.respond(&question("q?")).await;
        assert!(matches!(outcome, Outcome::SlackFailed(_)), "{outcome:?}");
        assert!(!outcome.reached_slack());
        assert_eq!(*slack.posts.lock().unwrap(), 3);
        assert_eq!(
            *slack.updates.lock().unwrap(),
            1,
            "a refusal is not retried"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_retry_after_beyond_the_limit_is_not_waited_for() {
        let (responder, slack) = flaky(vec![], vec![limited(120)]);
        let started = tokio::time::Instant::now();
        assert_eq!(responder.respond(&question("q?")).await, Outcome::Answered);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(*slack.updates.lock().unwrap(), 1);
        assert_eq!(slack.inner.texts().len(), 2, "posted as a new reply");
    }

    #[tokio::test(start_paused = true)]
    async fn switching_off_during_a_retry_wait_stops_at_once() {
        let (responder, slack) = flaky(vec![], vec![limited(20)]);
        let cancel = responder.cancel.clone();
        let waiting = slack.clone();
        let asked = question("q?");
        let (outcome, _) = tokio::join!(responder.respond(&asked), async {
            // Switched off once the edit was refused and the 20 s wait began.
            while *waiting.updates.lock().unwrap() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            cancel.cancel();
        });
        assert_eq!(outcome, Outcome::Cancelled);
        assert_eq!(*slack.updates.lock().unwrap(), 1, "no try after the switch");
        assert_eq!(slack.inner.texts(), vec![PLACEHOLDER.to_string()]);
    }
}
