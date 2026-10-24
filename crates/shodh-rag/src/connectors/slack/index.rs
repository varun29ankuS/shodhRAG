//! Slack channels as an indexed source: the sync (Onyx's connector shape) and the live
//! updates from events.
//!
//! One [`SyncWorker`] per Slack workspace, while the connector runs. Each [`SyncWorker::step`]
//! does one unit of work, cheapest-to-wait-for first:
//!
//! 1. a channel deleted in Slack is forgotten;
//! 2. a thread marked by an event (a new message, a reply, an edit, a deletion) is fetched
//!    again and its document replaced;
//! 3. one backfill page: `conversations.history` from the fixed `latest` bound back to the
//!    channel's window, 200 messages, each threaded parent's replies fetched; the cursor
//!    of the next page is saved after the page (a checkpoint: a laptop that sleeps, or a
//!    rate-limit pause, resumes at that page);
//! 4. an incremental poll: what came after the newest message seen (every
//!    [`SyncTiming::poll_every`]), and whether the channel was archived or shared since;
//! 5. a slim pass (every [`SyncTiming::slim_every`]): the ids and latest replies of the
//!    window's threads; a thread gone from Slack is deleted, one with a reply Shodh has not
//!    seen is fetched again.
//!
//! Slack's history methods are Tier 3 for an internal app (50+ a minute,
//! <https://docs.slack.dev/apis/web-api/rate-limits>): calls are paced
//! ([`SyncTiming::pace`], one every 1.3 s per Slack workspace), and a 429 pauses the
//! channel for Slack's `Retry-After` (the pause is saved, the card shows it). No model is
//! called anywhere here: embeddings are computed on this computer.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;

use super::content::{
    derived_permalink, mentioned_users, permalink_base, thread_document, time_ts, ts_time,
    HistoryPage, SlackMessage, ThreadDocument, MAX_THREAD_MESSAGES,
};
use super::events::{BotIdentity, Envelope};
use super::setup::missing_scopes_message;
use super::store::{ts_value, ChannelState, Forgotten, IndexedChannel, SlackStore};
use super::tokens::SlackBotToken;
use super::web::{ChannelInfo, SlackApiError, SlackWeb};
use crate::research::tables::calls::Clock;
use crate::types::{Citation, DocumentFormat};
use crate::RAGEngine;

/// The history window when the user has not chosen one.
pub const DEFAULT_WINDOW_DAYS: u32 = 90;
pub const MIN_WINDOW_DAYS: u32 = 7;
pub const MAX_WINDOW_DAYS: u32 = 365;
/// Pages one incremental poll reads at most (the rest waits for the next poll).
pub const POLL_PAGES: usize = 10;
/// Pages one slim pass reads at most (10 000 messages).
pub const SLIM_PAGES: usize = 50;
/// Pages of one thread's replies read at most.
pub const REPLY_PAGES: usize = MAX_THREAD_MESSAGES / super::web::HISTORY_PAGE;
/// How long a looked-up name is used before Slack is asked again.
pub const NAME_TTL_HOURS: i64 = 24;

/// What a person reads when indexing waits for a missing scope.
pub const INDEX_SCOPES: [&str; 2] = ["channels:history", "groups:history"];

/// How often and how fast the sync works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncTiming {
    /// The least time between two history calls of one Slack workspace.
    pub pace: Duration,
    pub poll_every: chrono::Duration,
    pub slim_every: chrono::Duration,
    /// The pause after an error that may pass (a network error, a 5xx).
    pub error_backoff: chrono::Duration,
    /// The pause after a 429 without `Retry-After`.
    pub default_retry_after: chrono::Duration,
    /// The longest idle wait (events also wake the worker).
    pub idle_max: Duration,
}

impl Default for SyncTiming {
    fn default() -> Self {
        Self {
            pace: Duration::from_millis(1_300),
            poll_every: chrono::Duration::minutes(15),
            slim_every: chrono::Duration::hours(6),
            error_backoff: chrono::Duration::minutes(5),
            default_retry_after: chrono::Duration::seconds(30),
            idle_max: Duration::from_secs(300),
        }
    }
}

/// Whether `days` is a history window the card accepts.
pub fn check_window_days(days: u32) -> Result<(), String> {
    if (MIN_WINDOW_DAYS..=MAX_WINDOW_DAYS).contains(&days) {
        Ok(())
    } else {
        Err(format!(
            "The history window is between {MIN_WINDOW_DAYS} and {MAX_WINDOW_DAYS} days."
        ))
    }
}

/// Reads a Slack workspace's channels (the bot's view).
#[async_trait]
pub trait SlackReader: Send + Sync {
    async fn history(
        &self,
        channel: &str,
        oldest: Option<&str>,
        latest: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<HistoryPage, SlackApiError>;
    async fn replies(
        &self,
        channel: &str,
        thread_ts: &str,
        cursor: Option<&str>,
    ) -> Result<HistoryPage, SlackApiError>;
    async fn channel_info(&self, channel: &str) -> Result<ChannelInfo, SlackApiError>;
    async fn permalink(&self, channel: &str, ts: &str) -> Result<String, SlackApiError>;
    /// The name people see for `user`.
    async fn user_name(&self, user: &str) -> Result<String, SlackApiError>;
}

/// Reads as the bot of one installation.
pub struct BotReader {
    pub web: SlackWeb,
    pub token: SlackBotToken,
}

#[async_trait]
impl SlackReader for BotReader {
    async fn history(
        &self,
        channel: &str,
        oldest: Option<&str>,
        latest: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<HistoryPage, SlackApiError> {
        self.web
            .history(&self.token, channel, oldest, latest, cursor)
            .await
    }

    async fn replies(
        &self,
        channel: &str,
        thread_ts: &str,
        cursor: Option<&str>,
    ) -> Result<HistoryPage, SlackApiError> {
        self.web
            .replies(&self.token, channel, thread_ts, cursor)
            .await
    }

    async fn channel_info(&self, channel: &str) -> Result<ChannelInfo, SlackApiError> {
        self.web.channel_info(&self.token, channel).await
    }

    async fn permalink(&self, channel: &str, ts: &str) -> Result<String, SlackApiError> {
        self.web.permalink(&self.token, channel, ts).await
    }

    async fn user_name(&self, user: &str) -> Result<String, SlackApiError> {
        Ok(self.web.user_info(&self.token, user).await?.name)
    }
}

/// Where thread documents go.
#[async_trait]
pub trait ThreadIndex: Send + Sync {
    /// Indexes `doc`; with `replace`, its earlier version (same source) is removed first.
    async fn put(&self, doc: &ThreadDocument, replace: bool) -> Result<(), String>;
    async fn delete_thread(&self, source: &str) -> Result<(), String>;
    /// Deletes every thread of a channel (its index space).
    async fn delete_space(&self, space_id: &str) -> Result<(), String>;
}

/// The document index (the same index Ask searches).
pub struct EngineIndex {
    pub rag: Arc<RwLock<RAGEngine>>,
}

/// The metadata a thread document is indexed with: its space, source, title, and the
/// permalink citations open.
pub fn thread_metadata(doc: &ThreadDocument) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    metadata.insert("space_id".to_string(), doc.space_id.clone());
    metadata.insert("file_path".to_string(), doc.source.clone());
    metadata.insert("title".to_string(), doc.title.clone());
    metadata.insert("source_type".to_string(), "slack".to_string());
    metadata.insert("slack_channel".to_string(), doc.channel_name.clone());
    if let Some(url) = &doc.permalink {
        metadata.insert("url".to_string(), url.clone());
    }
    metadata
}

#[async_trait]
impl ThreadIndex for EngineIndex {
    async fn put(&self, doc: &ThreadDocument, replace: bool) -> Result<(), String> {
        let rag = self.rag.clone();
        let doc = doc.clone();
        // Its own task: the removal and the new version are never split by a cancelled
        // caller.
        tokio::spawn(async move {
            let citation = Citation {
                title: doc.title.clone(),
                source: doc.source.clone(),
                url: doc.permalink.clone(),
                ..Citation::default()
            };
            let mut engine = rag.write().await;
            if replace {
                engine
                    .delete_by_source(&doc.source)
                    .await
                    .map_err(|e| format!("{e:#}"))?;
            }
            engine
                .add_document(
                    &doc.text,
                    DocumentFormat::TXT,
                    thread_metadata(&doc),
                    citation,
                )
                .await
                .map(|_| ())
                .map_err(|e| format!("{e:#}"))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn delete_thread(&self, source: &str) -> Result<(), String> {
        let rag = self.rag.clone();
        let source = source.to_string();
        tokio::spawn(async move {
            rag.write()
                .await
                .delete_by_source(&source)
                .await
                .map(|_| ())
                .map_err(|e| format!("{e:#}"))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn delete_space(&self, space_id: &str) -> Result<(), String> {
        let rag = self.rag.clone();
        let space = space_id.to_string();
        tokio::spawn(async move {
            rag.write()
                .await
                .delete_by_space_id(&space)
                .await
                .map(|_| ())
                .map_err(|e| format!("{e:#}"))
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

/// Hears what the sync did, for the card and for what lives outside the connector.
pub trait SyncObserver: Send + Sync {
    /// Something on the card changed.
    fn changed(&self);
    /// A channel was forgotten (deleted in Slack): the app removes it from its workspace
    /// and forgets its memories in the user's memories (`forgotten.memory_ids`).
    fn channel_forgotten(&self, channel: &IndexedChannel, forgotten: &Forgotten);
}

/// One store call off the async workers.
async fn blocking<T, F>(store: &Arc<SlackStore>, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&SlackStore) -> super::store::SlackStoreResult<T> + Send + 'static,
{
    let store = store.clone();
    tokio::task::spawn_blocking(move || f(&store).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Forgets everything of `channel`: its documents (its index space), sync state, threads,
/// memories and thread follow-ups. The caller holds the installation's sync gate, so no
/// sync step adds a thread back meanwhile.
pub async fn forget_channel(
    store: &Arc<SlackStore>,
    index: &dyn ThreadIndex,
    channel: &IndexedChannel,
) -> Result<Forgotten, String> {
    let (team, id) = (channel.team_id.clone(), channel.channel_id.clone());
    blocking(store, {
        let (team, id) = (team.clone(), id.clone());
        move |s| s.set_state(&team, &id, ChannelState::Forgetting, None)
    })
    .await?;
    index.delete_space(&channel.space_id()).await?;
    blocking(store, move |s| s.forget_channel(&team, &id)).await
}

/// One unit of the sync's work (for the log and tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Work {
    Forgotten {
        channel: String,
    },
    Thread {
        channel: String,
        thread_ts: String,
        kept: bool,
    },
    BackfillPage {
        channel: String,
        threads: usize,
        done: bool,
    },
    Poll {
        channel: String,
        threads: usize,
    },
    Slim {
        channel: String,
        deleted: usize,
        refetch: usize,
    },
    /// Slack's rate limit: paused until then.
    Paused {
        channel: String,
        until: DateTime<Utc>,
    },
    /// Stopped: archived, removed, shared since, or an error the user must fix.
    Stopped {
        channel: String,
        state: ChannelState,
    },
    /// A passing error; tried again later.
    Failed {
        channel: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Worked(Work),
    /// Nothing to do; the next work is due then (`None`: only an event brings work).
    Idle(Option<DateTime<Utc>>),
}

/// Spaces calls to Slack's history methods.
struct Pacer {
    every: Duration,
    next: tokio::sync::Mutex<Option<tokio::time::Instant>>,
}

impl Pacer {
    fn new(every: Duration) -> Self {
        Self {
            every,
            next: tokio::sync::Mutex::new(None),
        }
    }

    async fn wait(&self) {
        let mut next = self.next.lock().await;
        if let Some(at) = *next {
            tokio::time::sleep_until(at).await;
        }
        *next = Some(tokio::time::Instant::now() + self.every);
    }
}

/// Why a unit of work failed: Slack refused or failed, or this computer could not index.
enum StepError {
    Slack(SlackApiError),
    Local(String),
}

impl From<SlackApiError> for StepError {
    fn from(e: SlackApiError) -> Self {
        StepError::Slack(e)
    }
}

/// Why a step stopped short.
enum Halt {
    RateLimited(DateTime<Utc>),
    /// The channel is gone for the bot (`not_in_channel`, `channel_not_found`).
    Removed,
    /// The token or a scope was refused: every channel waits for the user.
    Refused(String),
    Passing(String),
}

/// The sync of one Slack workspace.
pub struct SyncWorker {
    team: String,
    bot_user: String,
    reader: Arc<dyn SlackReader>,
    index: Arc<dyn ThreadIndex>,
    store: Arc<SlackStore>,
    clock: Arc<dyn Clock>,
    observer: Arc<dyn SyncObserver>,
    gate: Arc<tokio::sync::Mutex<()>>,
    wake: Arc<Notify>,
    timing: SyncTiming,
    pacer: Pacer,
    names: Mutex<HashMap<String, (String, DateTime<Utc>)>>,
}

impl SyncWorker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        team: &str,
        bot_user: &str,
        reader: Arc<dyn SlackReader>,
        index: Arc<dyn ThreadIndex>,
        store: Arc<SlackStore>,
        clock: Arc<dyn Clock>,
        observer: Arc<dyn SyncObserver>,
        gate: Arc<tokio::sync::Mutex<()>>,
        wake: Arc<Notify>,
        timing: SyncTiming,
    ) -> Self {
        Self {
            team: team.to_string(),
            bot_user: bot_user.to_string(),
            reader,
            index,
            store,
            clock,
            observer,
            gate,
            wake,
            pacer: Pacer::new(timing.pace),
            timing,
            names: Mutex::new(HashMap::new()),
        }
    }

    /// Works until `cancel`: a step at a time, waiting when there is nothing to do until
    /// the next work is due or an event wakes it.
    pub async fn run(self, cancel: CancellationToken) {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let step = tokio::select! {
                step = self.step() => step,
                _ = cancel.cancelled() => return,
            };
            match step {
                Ok(Step::Worked(work)) => {
                    tracing::debug!(target: "shodh::slack", ?work, "Slack sync step");
                    self.observer.changed();
                }
                Ok(Step::Idle(due)) => {
                    let wait = due
                        .map(|at| {
                            (at - self.clock.now())
                                .to_std()
                                .unwrap_or(Duration::ZERO)
                                .min(self.timing.idle_max)
                        })
                        .unwrap_or(self.timing.idle_max);
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = self.wake.notified() => {}
                        _ = cancel.cancelled() => return,
                    }
                }
                Err(e) => {
                    // The store could not be read: wait, then try again.
                    tracing::warn!(target: "shodh::slack", error = %e, "the Slack sync could not read its state");
                    tokio::select! {
                        _ = tokio::time::sleep(self.timing.idle_max) => {}
                        _ = cancel.cancelled() => return,
                    }
                }
            }
        }
    }

    /// Does one unit of work, or says when the next is due.
    pub async fn step(&self) -> Result<Step, String> {
        let _turn = self.gate.lock().await;
        let team = self.team.clone();
        let channels = blocking(&self.store, move |s| s.channels(&team)).await?;
        let now = self.clock.now();
        if let Some(channel) = channels
            .iter()
            .find(|c| c.state == ChannelState::Forgetting)
        {
            let forgotten = forget_channel(&self.store, self.index.as_ref(), channel).await?;
            self.observer.channel_forgotten(channel, &forgotten);
            return Ok(Step::Worked(Work::Forgotten {
                channel: channel.channel_id.clone(),
            }));
        }
        let ready = |c: &&IndexedChannel| c.state == ChannelState::Active && !c.paused_at(now);
        let team = self.team.clone();
        let dirty = blocking(&self.store, move |s| s.dirty_threads(&team, 50)).await?;
        for (channel_id, thread_ts) in &dirty {
            if let Some(channel) = channels
                .iter()
                .filter(ready)
                .find(|c| &c.channel_id == channel_id)
            {
                return self
                    .refresh_thread(channel, thread_ts)
                    .await
                    .map(Step::Worked);
            }
        }
        if let Some(channel) = channels.iter().filter(ready).find(|c| !c.backfill_done) {
            return self.backfill_page(channel).await.map(Step::Worked);
        }
        let due = |last: Option<DateTime<Utc>>, every: chrono::Duration| {
            last.is_none_or(|at| now - at >= every)
        };
        if let Some(channel) = channels
            .iter()
            .filter(ready)
            .find(|c| due(c.last_poll_at, self.timing.poll_every))
        {
            return self.poll(channel).await.map(Step::Worked);
        }
        if let Some(channel) = channels
            .iter()
            .filter(ready)
            .find(|c| due(c.last_slim_at, self.timing.slim_every))
        {
            return self.slim(channel).await.map(Step::Worked);
        }
        let next = channels
            .iter()
            .filter(|c| c.state == ChannelState::Active)
            .flat_map(|c| {
                [
                    c.paused_until.filter(|until| *until > now),
                    c.last_poll_at.map(|at| at + self.timing.poll_every),
                    c.last_slim_at.map(|at| at + self.timing.slim_every),
                ]
            })
            .flatten()
            .min();
        Ok(Step::Idle(next))
    }

    // ── Slack calls ─────────────────────────────────────────────────

    fn halt(&self, error: StepError) -> Halt {
        let error = match error {
            StepError::Slack(error) => error,
            StepError::Local(e) => {
                return Halt::Passing(format!(
                    "Shodh could not index this channel ({e}); trying again shortly."
                ))
            }
        };
        let now = self.clock.now();
        match &error {
            SlackApiError::RateLimited { retry_after } => {
                let wait = retry_after
                    .and_then(|d| chrono::Duration::from_std(d).ok())
                    .unwrap_or(self.timing.default_retry_after);
                Halt::RateLimited(now + wait)
            }
            SlackApiError::Api { code, .. }
                if matches!(
                    code.as_str(),
                    "not_in_channel" | "channel_not_found" | "is_archived"
                ) =>
            {
                Halt::Removed
            }
            SlackApiError::Api {
                code,
                needed: Some(scope),
            } if code == "missing_scope" => {
                Halt::Refused(missing_scopes_message(std::slice::from_ref(scope)))
            }
            e if e.is_auth() => Halt::Refused(format!(
                "Slack refused the bot token ({}). Reinstall the Slack app and connect again with the new bot token.",
                e.code().unwrap_or("refused")
            )),
            e => Halt::Passing(format!(
                "Slack could not be read ({e}); trying again shortly."
            )),
        }
    }

    /// What a halted step leaves behind: the pause, the stop or the error, saved.
    async fn halted(&self, channel: &IndexedChannel, halt: Halt) -> Result<Work, String> {
        let (team, id) = (self.team.clone(), channel.channel_id.clone());
        match halt {
            Halt::RateLimited(until) => {
                // Slack limits each method per workspace: every channel waits.
                blocking(&self.store, move |s| {
                    s.pause_all(&team, until, "Slack's rate limit")
                })
                .await?;
                Ok(Work::Paused { channel: id, until })
            }
            Halt::Removed => {
                blocking(&self.store, move |s| {
                    s.set_state(
                        &team,
                        &id,
                        ChannelState::Removed,
                        Some("Shodh is no longer in this channel. Invite it again (/invite @shodh) and choose the channel again."),
                    )
                })
                .await?;
                Ok(Work::Stopped {
                    channel: channel.channel_id.clone(),
                    state: ChannelState::Removed,
                })
            }
            Halt::Refused(reason) => {
                blocking(&self.store, move |s| s.stop_all(&team, &reason)).await?;
                Ok(Work::Stopped {
                    channel: channel.channel_id.clone(),
                    state: ChannelState::Error,
                })
            }
            Halt::Passing(reason) => {
                let until = self.clock.now() + self.timing.error_backoff;
                let shown = reason.clone();
                blocking(&self.store, move |s| s.pause(&team, &id, until, &shown)).await?;
                Ok(Work::Failed {
                    channel: channel.channel_id.clone(),
                    reason,
                })
            }
        }
    }

    async fn history(
        &self,
        channel: &str,
        oldest: Option<&str>,
        latest: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<HistoryPage, SlackApiError> {
        self.pacer.wait().await;
        self.reader.history(channel, oldest, latest, cursor).await
    }

    /// Every message of thread `thread_ts` (bounded), its first message first.
    async fn thread_messages(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> Result<Vec<SlackMessage>, SlackApiError> {
        let mut messages = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..REPLY_PAGES.max(1) {
            self.pacer.wait().await;
            let page = self
                .reader
                .replies(channel, thread_ts, cursor.as_deref())
                .await?;
            messages.extend(page.messages);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        // Each page may repeat the thread's first message.
        let mut seen = std::collections::HashSet::new();
        messages.retain(|m| seen.insert(m.ts.clone()));
        Ok(messages)
    }

    /// The names of the people who wrote or are mentioned in `messages`.
    async fn names_for(&self, messages: &[SlackMessage]) -> HashMap<String, String> {
        let mut wanted: BTreeSet<String> = BTreeSet::new();
        for m in messages {
            if let Some(user) = &m.user {
                wanted.insert(user.clone());
            }
            wanted.extend(mentioned_users(&m.text));
        }
        let now = self.clock.now();
        let mut names = HashMap::new();
        let mut unknown = Vec::new();
        {
            let cache = self.names.lock().unwrap_or_else(|e| e.into_inner());
            for user in wanted {
                match cache.get(&user) {
                    Some((name, at)) if now - *at < chrono::Duration::hours(NAME_TTL_HOURS) => {
                        names.insert(user, name.clone());
                    }
                    _ => unknown.push(user),
                }
            }
        }
        for user in unknown {
            match self.reader.user_name(&user).await {
                Ok(name) => {
                    self.names
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(user.clone(), (name.clone(), now));
                    names.insert(user, name);
                }
                Err(e) => {
                    tracing::debug!(target: "shodh::slack", error = %e, "a Slack name could not be read");
                }
            }
        }
        names
    }

    /// The permalink of message `ts`, derived from the channel's base when it has one.
    async fn permalink(&self, channel: &IndexedChannel, ts: &str) -> Option<String> {
        let team = self.team.clone();
        let id = channel.channel_id.clone();
        let base = blocking(&self.store, move |s| s.channel(&team, &id))
            .await
            .ok()
            .flatten()
            .and_then(|c| c.permalink_base);
        if let Some(base) = base {
            return Some(derived_permalink(&base, ts));
        }
        match self.reader.permalink(&channel.channel_id, ts).await {
            Ok(link) => {
                if let Some(base) = permalink_base(&link, &channel.channel_id, ts) {
                    let (team, id) = (self.team.clone(), channel.channel_id.clone());
                    if let Err(e) = blocking(&self.store, move |s| {
                        s.set_permalink_base(&team, &id, &base)
                    })
                    .await
                    {
                        tracing::debug!(target: "shodh::slack", error = %e, "a permalink base could not be saved");
                    }
                }
                Some(link)
            }
            Err(e) => {
                tracing::debug!(target: "shodh::slack", error = %e, "no permalink for a Slack thread");
                None
            }
        }
    }

    // ── Units of work ───────────────────────────────────────────────

    /// Indexes the thread `parent` starts (its replies fetched when it has any); returns
    /// whether a document was kept.
    async fn index_thread(
        &self,
        channel: &IndexedChannel,
        thread_ts: &str,
        known: Option<Vec<SlackMessage>>,
        replace: bool,
    ) -> Result<bool, StepError> {
        let messages = match known {
            Some(messages) => messages,
            None => self.thread_messages(&channel.channel_id, thread_ts).await?,
        };
        let names = self.names_for(&messages).await;
        let permalink = self.permalink(channel, thread_ts).await;
        let doc = thread_document(
            &self.team,
            &channel.channel_id,
            &channel.name,
            thread_ts,
            &messages,
            &names,
            permalink,
            &self.bot_user,
        );
        let (team, id, ts) = (
            self.team.clone(),
            channel.channel_id.clone(),
            thread_ts.to_string(),
        );
        match doc {
            Some(doc) => {
                self.index
                    .put(&doc, replace)
                    .await
                    .map_err(StepError::Local)?;
                let now = self.clock.now();
                blocking(&self.store, move |s| {
                    s.record_thread(
                        &team,
                        &id,
                        &ts,
                        doc.latest_reply.as_deref(),
                        doc.reply_count,
                        &doc.message_ts,
                        now,
                    )
                })
                .await
                .map_err(StepError::Local)?;
                Ok(true)
            }
            None => {
                if replace {
                    self.index
                        .delete_thread(&super::ids::thread_source(&team, &id, &ts))
                        .await
                        .map_err(StepError::Local)?;
                }
                blocking(&self.store, move |s| s.remove_thread(&team, &id, &ts))
                    .await
                    .map_err(StepError::Local)?;
                Ok(false)
            }
        }
    }

    /// Fetches a thread an event marked and replaces its document (or deletes it when
    /// nothing of it is left).
    async fn refresh_thread(
        &self,
        channel: &IndexedChannel,
        thread_ts: &str,
    ) -> Result<Work, String> {
        let messages = match self.thread_messages(&channel.channel_id, thread_ts).await {
            Ok(messages) => Some(messages),
            Err(SlackApiError::Api { code, .. }) if code == "thread_not_found" => Some(Vec::new()),
            Err(e) => return self.halted(channel, self.halt(e.into())).await,
        };
        let (team, id, ts) = (
            self.team.clone(),
            channel.channel_id.clone(),
            thread_ts.to_string(),
        );
        let indexed = blocking(&self.store, move |s| s.is_indexed(&team, &id, &ts)).await?;
        let kept = match self
            .index_thread(channel, thread_ts, messages, indexed)
            .await
        {
            Ok(kept) => kept,
            Err(e) => return self.halted(channel, self.halt(e)).await,
        };
        let (team, id) = (self.team.clone(), channel.channel_id.clone());
        let now = self.clock.now();
        blocking(&self.store, move |s| s.touch_sync(&team, &id, now)).await?;
        Ok(Work::Thread {
            channel: channel.channel_id.clone(),
            thread_ts: thread_ts.to_string(),
            kept,
        })
    }

    /// Indexes the top-level messages of `page` (threads with replies fetched whole);
    /// returns how many threads were kept and the oldest and newest `ts` it held.
    async fn index_page(
        &self,
        channel: &IndexedChannel,
        page: &HistoryPage,
        known: &HashMap<String, Option<String>>,
    ) -> Result<(usize, Option<String>, Option<String>), StepError> {
        let mut kept = 0;
        let mut oldest: Option<String> = None;
        let mut newest: Option<String> = None;
        for message in &page.messages {
            if oldest
                .as_deref()
                .is_none_or(|o| ts_value(&message.ts) < ts_value(o))
            {
                oldest = Some(message.ts.clone());
            }
            if newest
                .as_deref()
                .is_none_or(|n| ts_value(&message.ts) > ts_value(n))
            {
                newest = Some(message.ts.clone());
            }
            if !message.top_level() {
                // A reply also sent to the channel: its thread is fetched again.
                let (team, id, thread) = (
                    self.team.clone(),
                    channel.channel_id.clone(),
                    message.thread().to_string(),
                );
                let now = self.clock.now();
                blocking(&self.store, move |s| s.mark_dirty(&team, &id, &thread, now))
                    .await
                    .map_err(StepError::Local)?;
                continue;
            }
            let replace = known.contains_key(&message.ts);
            let messages = (message.reply_count == 0).then(|| vec![message.clone()]);
            if self
                .index_thread(channel, &message.ts, messages, replace)
                .await?
            {
                kept += 1;
            }
        }
        Ok((kept, oldest, newest))
    }

    async fn backfill_page(&self, channel: &IndexedChannel) -> Result<Work, String> {
        let now = self.clock.now();
        let latest = channel
            .backfill_latest
            .clone()
            .unwrap_or_else(|| time_ts(now));
        let start = ts_time(&latest).unwrap_or(now)
            - chrono::Duration::days(i64::from(channel.window_days));
        let oldest = time_ts(start);
        let page = match self
            .history(
                &channel.channel_id,
                Some(&oldest),
                Some(&latest),
                channel.backfill_cursor.as_deref(),
            )
            .await
        {
            Ok(page) => page,
            Err(e) => return self.halted(channel, self.halt(e.into())).await,
        };
        let (team, id) = (self.team.clone(), channel.channel_id.clone());
        let known = blocking(&self.store, move |s| s.threads(&team, &id)).await?;
        let (threads, page_oldest, page_newest) =
            match self.index_page(channel, &page, &known).await {
                Ok(found) => found,
                Err(e) => return self.halted(channel, self.halt(e)).await,
            };
        let done = page.next_cursor.is_none();
        let (team, id) = (self.team.clone(), channel.channel_id.clone());
        let next = page.next_cursor.clone();
        let now = self.clock.now();
        blocking(&self.store, move |s| {
            s.save_backfill_page(
                &team,
                &id,
                &latest,
                next.as_deref(),
                page_oldest.as_deref(),
                page_newest.as_deref(),
                now,
            )?;
            if done {
                // The backfill read everything up to `latest`: the first poll and slim pass
                // are due one period from now.
                s.save_poll(&team, &id, Some(&latest), now)?;
                s.save_slim(&team, &id, now)?;
            }
            Ok(())
        })
        .await?;
        Ok(Work::BackfillPage {
            channel: channel.channel_id.clone(),
            threads,
            done,
        })
    }

    /// The channel as Slack has it now: archived or shared since it was chosen stops it.
    async fn check_channel(&self, channel: &IndexedChannel) -> Result<Option<Work>, String> {
        self.pacer.wait().await;
        let info = match self.reader.channel_info(&channel.channel_id).await {
            Ok(info) => info,
            Err(e) => return self.halted(channel, self.halt(e.into())).await.map(Some),
        };
        let (team, id) = (self.team.clone(), channel.channel_id.clone());
        if info.archived {
            blocking(&self.store, move |s| {
                s.set_state(
                    &team,
                    &id,
                    ChannelState::Archived,
                    Some("The channel was archived in Slack. What was indexed stays until you forget it."),
                )
            })
            .await?;
            return Ok(Some(Work::Stopped {
                channel: channel.channel_id.clone(),
                state: ChannelState::Archived,
            }));
        }
        if info.shared && !channel.shared {
            blocking(&self.store, move |s| {
                s.set_state(&team, &id, ChannelState::NowShared, Some(NOW_SHARED))
            })
            .await?;
            return Ok(Some(Work::Stopped {
                channel: channel.channel_id.clone(),
                state: ChannelState::NowShared,
            }));
        }
        Ok(None)
    }

    async fn poll(&self, channel: &IndexedChannel) -> Result<Work, String> {
        if let Some(stopped) = self.check_channel(channel).await? {
            return Ok(stopped);
        }
        let now = self.clock.now();
        let oldest = channel.newest_ts.clone().unwrap_or_else(|| {
            time_ts(now - chrono::Duration::days(i64::from(channel.window_days)))
        });
        let (team, id) = (self.team.clone(), channel.channel_id.clone());
        let known = blocking(&self.store, move |s| s.threads(&team, &id)).await?;
        let mut cursor: Option<String> = None;
        let mut threads = 0;
        let mut newest: Option<String> = None;
        for _ in 0..POLL_PAGES {
            let page = match self
                .history(&channel.channel_id, Some(&oldest), None, cursor.as_deref())
                .await
            {
                Ok(page) => page,
                Err(e) => return self.halted(channel, self.halt(e.into())).await,
            };
            let (kept, _, page_newest) = match self.index_page(channel, &page, &known).await {
                Ok(found) => found,
                Err(e) => return self.halted(channel, self.halt(e)).await,
            };
            threads += kept;
            if let Some(n) = page_newest {
                if newest
                    .as_deref()
                    .is_none_or(|old| ts_value(&n) > ts_value(old))
                {
                    newest = Some(n);
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        let (team, id) = (self.team.clone(), channel.channel_id.clone());
        let now = self.clock.now();
        blocking(&self.store, move |s| {
            s.save_poll(&team, &id, newest.as_deref(), now)
        })
        .await?;
        Ok(Work::Poll {
            channel: channel.channel_id.clone(),
            threads,
        })
    }

    async fn slim(&self, channel: &IndexedChannel) -> Result<Work, String> {
        let now = self.clock.now();
        let start = now - chrono::Duration::days(i64::from(channel.window_days));
        let oldest = time_ts(start);
        let mut listed: HashMap<String, Option<String>> = HashMap::new();
        let mut cursor: Option<String> = None;
        let mut complete = false;
        for _ in 0..SLIM_PAGES {
            let page = match self
                .history(&channel.channel_id, Some(&oldest), None, cursor.as_deref())
                .await
            {
                Ok(page) => page,
                Err(e) => return self.halted(channel, self.halt(e.into())).await,
            };
            for m in page.messages.iter().filter(|m| m.top_level()) {
                if m.indexed(&self.bot_user) || m.reply_count > 0 {
                    listed.insert(m.ts.clone(), m.latest_reply.clone());
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                complete = true;
                break;
            }
        }
        let (team, id) = (self.team.clone(), channel.channel_id.clone());
        let stored = blocking(&self.store, move |s| s.threads(&team, &id)).await?;
        let start_value = ts_value(&oldest);
        let mut deleted = 0;
        let mut refetch = Vec::new();
        for (ts, latest) in &stored {
            match listed.get(ts) {
                // Gone from Slack (only when the whole window was listed).
                None if complete && ts_value(ts) >= start_value => {
                    self.index
                        .delete_thread(&super::ids::thread_source(
                            &self.team,
                            &channel.channel_id,
                            ts,
                        ))
                        .await?;
                    let (team, id, ts) =
                        (self.team.clone(), channel.channel_id.clone(), ts.clone());
                    blocking(&self.store, move |s| s.remove_thread(&team, &id, &ts)).await?;
                    deleted += 1;
                }
                Some(listed_latest) if listed_latest.is_some() && listed_latest != latest => {
                    refetch.push(ts.clone());
                }
                _ => {}
            }
        }
        // Threads Slack has that Shodh never saw (posted while it was not running).
        for ts in listed.keys() {
            if !stored.contains_key(ts) {
                refetch.push(ts.clone());
            }
        }
        let count = refetch.len();
        let (team, id) = (self.team.clone(), channel.channel_id.clone());
        let now = self.clock.now();
        blocking(&self.store, move |s| {
            for ts in &refetch {
                s.mark_dirty(&team, &id, ts, now)?;
            }
            if complete {
                s.save_slim(&team, &id, now)?;
            }
            Ok(())
        })
        .await?;
        Ok(Work::Slim {
            channel: channel.channel_id.clone(),
            deleted,
            refetch: count,
        })
    }
}

/// Why a channel indexed while not shared stopped.
pub const NOW_SHARED: &str = "This channel is shared with another organization now. Shodh stopped indexing it; choose it again to confirm.";

// ── Events ──────────────────────────────────────────────────────────

/// What an event means for the indexed channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexEvent {
    /// A new message, a reply or an edit in thread `thread_ts`.
    Message {
        channel: String,
        thread_ts: String,
        ext_shared: bool,
    },
    /// Message `ts` was deleted (`thread_ts`: its thread, when the event says).
    Deleted {
        channel: String,
        ts: String,
        thread_ts: Option<String>,
    },
    Archived {
        channel: String,
    },
    ChannelDeleted {
        channel: String,
    },
    /// The bot left the channel or was removed from it.
    BotLeft {
        channel: String,
    },
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn from_bot(message: &Value, bot: &BotIdentity) -> bool {
    message.get("bot_id").is_some_and(|v| !v.is_null())
        || message.get("bot_profile").is_some_and(|v| !v.is_null())
        || field(message, "user") == Some(bot.user_id.as_str())
        || field(message, "subtype") == Some("bot_message")
}

/// What `envelope` means for indexing, if anything. Bots' messages (Shodh's own answers
/// and their edits included) never are: they would only fetch a thread again for nothing.
pub fn index_event(envelope: &Envelope, bot: &BotIdentity) -> Option<IndexEvent> {
    if envelope.kind != "events_api" {
        return None;
    }
    let payload = &envelope.payload;
    if field(payload, "type") != Some("event_callback") {
        return None;
    }
    let event = payload.get("event")?;
    let ext_shared = payload
        .get("is_ext_shared_channel")
        .and_then(Value::as_bool)
        == Some(true);
    let channel = || field(event, "channel").map(str::to_string);
    match field(event, "type")? {
        "message" => {
            if !matches!(
                field(event, "channel_type"),
                Some("channel") | Some("group")
            ) {
                return None;
            }
            match field(event, "subtype") {
                None | Some("file_share") | Some("thread_broadcast") | Some("me_message") => {
                    if from_bot(event, bot) {
                        return None;
                    }
                    let ts = field(event, "ts")?;
                    Some(IndexEvent::Message {
                        channel: channel()?,
                        thread_ts: field(event, "thread_ts").unwrap_or(ts).to_string(),
                        ext_shared,
                    })
                }
                Some("message_changed") => {
                    let message = event.get("message")?;
                    if from_bot(message, bot) {
                        return None;
                    }
                    let ts = field(message, "ts")?;
                    Some(IndexEvent::Message {
                        channel: channel()?,
                        thread_ts: field(message, "thread_ts").unwrap_or(ts).to_string(),
                        ext_shared,
                    })
                }
                Some("message_deleted") => {
                    let previous = event.get("previous_message");
                    if previous.is_some_and(|p| from_bot(p, bot)) {
                        return None;
                    }
                    Some(IndexEvent::Deleted {
                        channel: channel()?,
                        ts: field(event, "deleted_ts")?.to_string(),
                        thread_ts: previous
                            .and_then(|p| field(p, "thread_ts"))
                            .map(str::to_string),
                    })
                }
                _ => None,
            }
        }
        "channel_archive" | "group_archive" => Some(IndexEvent::Archived {
            channel: channel()?,
        }),
        "channel_deleted" | "group_deleted" => Some(IndexEvent::ChannelDeleted {
            channel: channel()?,
        }),
        "member_left_channel" if field(event, "user") == Some(bot.user_id.as_str()) => {
            Some(IndexEvent::BotLeft {
                channel: channel()?,
            })
        }
        "channel_left" | "group_left" => Some(IndexEvent::BotLeft {
            channel: channel()?,
        }),
        _ => None,
    }
}

/// Applies index events to the store and wakes the worker. Blocking (a database write).
pub struct IndexEvents {
    team: String,
    store: Arc<SlackStore>,
    clock: Arc<dyn Clock>,
    wake: Arc<Notify>,
}

impl IndexEvents {
    pub fn new(
        team: &str,
        store: Arc<SlackStore>,
        clock: Arc<dyn Clock>,
        wake: Arc<Notify>,
    ) -> Self {
        Self {
            team: team.to_string(),
            store,
            clock,
            wake,
        }
    }

    /// Applies `event`: only channels being indexed are touched. Returns whether anything
    /// changed.
    pub fn apply(&self, event: &IndexEvent) -> super::store::SlackStoreResult<bool> {
        let channel_id = match event {
            IndexEvent::Message { channel, .. }
            | IndexEvent::Deleted { channel, .. }
            | IndexEvent::Archived { channel }
            | IndexEvent::ChannelDeleted { channel }
            | IndexEvent::BotLeft { channel } => channel,
        };
        let Some(channel) = self.store.channel(&self.team, channel_id)? else {
            return Ok(false);
        };
        let now = self.clock.now();
        let changed = match event {
            IndexEvent::Message {
                thread_ts,
                ext_shared,
                ..
            } => {
                if channel.state != ChannelState::Active {
                    false
                } else if *ext_shared && !channel.shared {
                    self.store.set_state(
                        &self.team,
                        channel_id,
                        ChannelState::NowShared,
                        Some(NOW_SHARED),
                    )?;
                    true
                } else {
                    self.store
                        .mark_dirty(&self.team, channel_id, thread_ts, now)?;
                    true
                }
            }
            IndexEvent::Deleted { ts, thread_ts, .. } => {
                let thread = match thread_ts {
                    Some(thread) => Some(thread.clone()),
                    None => self.store.thread_of_message(&self.team, channel_id, ts)?,
                };
                match thread {
                    Some(thread) if channel.state == ChannelState::Active => {
                        self.store
                            .mark_dirty(&self.team, channel_id, &thread, now)?;
                        true
                    }
                    _ => false,
                }
            }
            IndexEvent::Archived { .. } => {
                self.store.set_state(
                    &self.team,
                    channel_id,
                    ChannelState::Archived,
                    Some("The channel was archived in Slack. What was indexed stays until you forget it."),
                )?;
                true
            }
            IndexEvent::ChannelDeleted { .. } => {
                self.store.set_state(
                    &self.team,
                    channel_id,
                    ChannelState::Forgetting,
                    Some(
                        "The channel was deleted in Slack; Shodh forgets what it indexed from it.",
                    ),
                )?;
                true
            }
            IndexEvent::BotLeft { .. } => {
                self.store.set_state(
                    &self.team,
                    channel_id,
                    ChannelState::Removed,
                    Some("Shodh is no longer in this channel. Invite it again (/invite @shodh) and choose the channel again."),
                )?;
                true
            }
        };
        if changed {
            self.wake.notify_one();
        }
        Ok(changed)
    }
}

/// What the card says about a channel's backfill: threads so far, about how many in all,
/// and about how long the rest takes at Slack's pace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackfillProgress {
    pub done: usize,
    /// `None` until enough of the window was read to say.
    pub estimated_total: Option<usize>,
    pub estimated_seconds_left: Option<u64>,
}

/// The progress of `channel`'s backfill at `now` (`pace`: the time between two calls).
pub fn backfill_progress(channel: &IndexedChannel, pace: Duration) -> BackfillProgress {
    let done = channel.threads;
    let latest = channel.backfill_latest.as_deref().map(ts_value);
    let oldest = channel.oldest_seen.as_deref().map(ts_value);
    let window = f64::from(channel.window_days) * 86_400.0;
    let fraction = match (latest, oldest) {
        (Some(latest), Some(oldest)) if latest > oldest && window > 0.0 => {
            ((latest - oldest) / window).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    if channel.backfill_done {
        return BackfillProgress {
            done,
            estimated_total: Some(done),
            estimated_seconds_left: Some(0),
        };
    }
    if fraction < 0.05 || done == 0 {
        return BackfillProgress {
            done,
            estimated_total: None,
            estimated_seconds_left: None,
        };
    }
    // `fraction` is at least 0.05, so the estimate is at most twenty times what was seen.
    let estimate = ((done as f64) / fraction).round();
    let total = if estimate.is_finite() && estimate >= 0.0 {
        (estimate as usize).max(done)
    } else {
        done
    };
    // Upper bound: one call per thread still to read.
    let left = (total.saturating_sub(done) as f64 * pace.as_secs_f64()).ceil();
    BackfillProgress {
        done,
        estimated_total: Some(total),
        estimated_seconds_left: Some(if left.is_finite() && left >= 0.0 {
            left as u64
        } else {
            0
        }),
    }
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
