//! The Telegram connector in the app: its settings, token, polling and the commands of its
//! page in the Connectors view (see `shodh_rag::connectors::telegram` for the connector).
//!
//! - **Settings** (`connectors.telegram` in `app_settings.json`, user-only): the bot, whether
//!   it is on, the linked owner, the people and groups allowed to ask, the workspace they
//!   are answered from, and the daily cap of their answers.
//! - **Token** in the OS credential store (`telegram-bot-token:<bot id>`); never logged,
//!   never sent to the frontend, deleted on Disconnect.
//! - **Polling** only while the connector is on and Local-only mode is off; Local-only
//!   mode stops it, turning it off resumes it; closed on exit.
//! - **The owner** (linked with a one-time code shown as a link and a QR code) gets Shodh
//!   as in Ask from their private chat: the whole library, their memories, the model they
//!   chose, no tools. Their words go through learning like an Ask chat.
//! - **Others** (invited people, groups the owner allowed) get answers from one workspace,
//!   never memories, counted against Telegram's own daily cap.
//! - Every exchange is kept as an app conversation ([`super::conversations`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shodh_rag::audit::LOCAL_OWNER;
use shodh_rag::connectors::slack::answer::{
    retrieve_passages, AnswerPassage, AnswerScope, ThreadTurn,
};
use shodh_rag::connectors::slack::respond::AnswerQuota;
use shodh_rag::connectors::slack::sanitize::PrivateTerms;
use shodh_rag::connectors::telegram::access::Rules;
use shodh_rag::connectors::telegram::answer::{retrieve_library, OWNER_PASSAGES, OWNER_TURNS};
use shodh_rag::connectors::telegram::api::{TelegramApi, TelegramError, TokenBot};
use shodh_rag::connectors::telegram::link::{chat_link, deep_link, LinkCodes, LinkError, LinkKind};
use shodh_rag::connectors::telegram::poll::{self, OffsetStore, PollSink, PollState, PollTiming};
use shodh_rag::connectors::telegram::respond::{
    Asked, Audience, ChatRef, Dispatcher, Outcome, OutcomeSink, Remembered, Responder,
    TelegramBrain,
};
use shodh_rag::connectors::telegram::token::TelegramBotToken;
use shodh_rag::connectors::telegram::updates::{BotSelf, Chat, Person};
use shodh_rag::connectors::telegram::CancellationToken;
use shodh_rag::research::results::TextModel;
use shodh_rag::research::tables::calls::{next_utc_midnight, utc_day, SystemClock};
use shodh_rag::user_memory::learn::MemoryRequest;
use shodh_rag::user_memory::Actor;
use tauri::{AppHandle, Emitter, Manager, State};

use super::conversations::{self, key_part, Connector, Exchange, Said};
use super::slack::{app_private_terms, audit_change, workspace_ref, WorkspaceRef};
use super::slack_usage::SlackUsage;
use super::telegram_notices::{self, TelegramNotice, TelegramNoticeState};
use crate::app_settings::{broadcast, AppSettings, SettingsStore};
use crate::audit_commands::AuditState;
use crate::durable_file::{self, Loaded, ReadFailure};
use crate::llm_commands::LLMState;
use crate::memory_commands::{recall_for_run, MemoryState};
use crate::memory_learn::{LearnState, RequestHandled};
use crate::rag_commands::RagState;
use crate::workspace_commands::{answer_sources, WorkspaceState};

/// Emitted with the [`TelegramStatus`] whenever it changes.
pub const TELEGRAM_STATUS_EVENT: &str = "connectors-telegram-status";

/// Answers to others per UTC day when the user has not set a cap.
pub const DEFAULT_DAILY_ANSWER_CAP: usize = 20;
pub const MIN_DAILY_ANSWER_CAP: usize = 1;
pub const MAX_DAILY_ANSWER_CAP: usize = 1_000;

/// Credential-store user of the bot token (never a provider id).
const TOKEN_USER: &str = "telegram-bot-token";
/// The usage file of others' answers.
const USAGE_FILE: &str = "connectors/telegram_usage.json";
/// The polling checkpoint and each private chat's conversation number.
const STATE_FILE: &str = "connectors/telegram_state.json";
/// The longest wait for the polling to stop when the app exits.
const EXIT_WAIT: Duration = Duration::from_secs(3);

const LOCAL_ONLY: &str = "Local-only mode is on, so Shodh does not connect to Telegram. Turn it off in Settings → Privacy to use the Telegram connector.";

/// Said on the page: where Telegram messages go.
pub const TELEGRAM_PRIVACY_NOTE: &str = "Telegram bot chats are not end-to-end encrypted: messages to and from the bot pass through Telegram's servers and are stored there.";

// ── Settings ────────────────────────────────────────────────────────

/// Someone on Telegram the connector knows (the owner, an invited person).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramPerson {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub username: Option<String>,
    pub linked_at: DateTime<Utc>,
}

/// A group the bot was added to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramGroup {
    pub id: i64,
    pub title: String,
    /// Allowed by the owner; until then nobody there gets an answer.
    #[serde(default)]
    pub allowed: bool,
    /// Who added the bot, when Telegram said.
    #[serde(default)]
    pub added_by: Option<String>,
    pub seen_at: DateTime<Utc>,
}

/// The bot the connector runs as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramBotPrefs {
    pub bot_id: i64,
    /// Without the `@`.
    pub username: String,
    pub name: String,
    /// Privacy mode off: the bot reads every group message (it still answers only when
    /// mentioned).
    #[serde(default)]
    pub reads_all_group_messages: bool,
    pub connected_at: DateTime<Utc>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub owner: Option<TelegramPerson>,
    /// The workspace others are answered from; `None` until chosen.
    #[serde(default)]
    pub answer_workspace: Option<String>,
    #[serde(default)]
    pub people: Vec<TelegramPerson>,
    #[serde(default)]
    pub groups: Vec<TelegramGroup>,
}

/// The Telegram connector's settings. User-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TelegramPrefs {
    /// The most answers to others (model calls) per UTC day.
    pub daily_answer_cap: usize,
    pub bot: Option<TelegramBotPrefs>,
}

impl Default for TelegramPrefs {
    fn default() -> Self {
        Self {
            daily_answer_cap: DEFAULT_DAILY_ANSWER_CAP,
            bot: None,
        }
    }
}

/// Whether `cap` is a cap the page accepts.
pub fn check_daily_cap(cap: usize) -> Result<(), String> {
    if (MIN_DAILY_ANSWER_CAP..=MAX_DAILY_ANSWER_CAP).contains(&cap) {
        Ok(())
    } else {
        Err(format!(
            "The daily limit of Telegram answers is between {MIN_DAILY_ANSWER_CAP} and {MAX_DAILY_ANSWER_CAP}."
        ))
    }
}

impl TelegramPrefs {
    /// The cap to use: the stored one when in range, else the default.
    pub fn daily_cap(&self) -> usize {
        if check_daily_cap(self.daily_answer_cap).is_ok() {
            self.daily_answer_cap
        } else {
            DEFAULT_DAILY_ANSWER_CAP
        }
    }
}

/// Whether the bot should poll under `settings`.
pub fn should_run(settings: &AppSettings) -> bool {
    !settings.policy.local_only
        && settings
            .connectors
            .telegram
            .bot
            .as_ref()
            .is_some_and(|b| b.enabled)
}

/// The access rules of `bot`.
pub fn rules_of(bot: &TelegramBotPrefs) -> Rules {
    Rules {
        bot: Some(BotSelf {
            id: bot.bot_id,
            username: bot.username.clone(),
        }),
        owner: bot.owner.as_ref().map(|o| o.id),
        people: bot.people.iter().map(|p| p.id).collect(),
        groups: bot
            .groups
            .iter()
            .filter(|g| g.allowed)
            .map(|g| g.id)
            .collect(),
    }
}

/// The chat key of a conversation: a private chat's current conversation
/// (`u<id>-<n>`, a new `n` after `/new`), or a group thread (`g<id>-<root message>`).
pub fn chat_key(audience: Audience, chat: i64, epoch: u64, root: i64) -> String {
    match audience {
        Audience::Owner | Audience::Guest => format!("u{}-{epoch}", key_part(&chat.to_string())),
        Audience::Group => format!("g{}-{root}", key_part(&chat.unsigned_abs().to_string())),
    }
}

/// What the sidebar groups a conversation under.
pub fn chat_label(audience: Audience, from: &str, group: Option<&str>) -> String {
    match audience {
        Audience::Owner => "Telegram · you".to_string(),
        Audience::Guest => format!("Telegram · {from}"),
        Audience::Group => format!("Telegram · {}", group.unwrap_or("a group")),
    }
}

// ── Token ───────────────────────────────────────────────────────────

fn token_user(bot_id: i64) -> String {
    format!("{TOKEN_USER}:{bot_id}")
}

fn load_token(bot_id: i64) -> Result<Option<TelegramBotToken>, String> {
    match crate::api_key_store::load_secret(&token_user(bot_id))? {
        Some(raw) => TelegramBotToken::parse(&raw)
            .map(Some)
            .map_err(|e| format!("The saved bot token is not usable: {e}")),
        None => Ok(None),
    }
}

// ── State on disk ───────────────────────────────────────────────────

/// The polling checkpoint and the conversation number of each private chat.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct DiskState {
    offset: Option<i64>,
    /// By chat id: the conversation `/new` started last.
    epochs: HashMap<String, u64>,
}

/// [`DiskState`] in its file (`connectors/telegram_state.json`, atomic writes).
pub struct StateFile {
    path: PathBuf,
    lock: Mutex<()>,
}

impl StateFile {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(STATE_FILE),
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> Result<DiskState, String> {
        match durable_file::read_with_backup(
            &self.path,
            |text| serde_json::from_str::<DiskState>(text).map_err(|e| e.to_string()),
            DiskState::default,
        ) {
            Ok(Loaded { value, .. }) => Ok(value),
            // A damaged state starts empty: at worst Telegram sends the updates it still
            // keeps again, and private chats start a new conversation.
            Err(ReadFailure::Damaged { problem, .. }) => {
                tracing::warn!(target: "shodh::telegram", %problem, "the Telegram state was damaged; starting empty");
                Ok(DiskState::default())
            }
            Err(ReadFailure::Io { message, .. }) => Err(message),
        }
    }

    fn write(&self, state: &DiskState) -> Result<(), String> {
        let text = serde_json::to_string(state).map_err(|e| e.to_string())?;
        durable_file::write_atomic(&self.path, text.as_bytes()).map_err(|e| e.to_string())
    }

    fn change<T>(&self, f: impl FnOnce(&mut DiskState) -> T) -> Result<T, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.read()?;
        let out = f(&mut state);
        self.write(&state)?;
        Ok(out)
    }

    /// The conversation number of private chat `chat`.
    pub fn epoch(&self, chat: i64) -> u64 {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.read()
            .ok()
            .and_then(|s| s.epochs.get(&chat.to_string()).copied())
            .unwrap_or(0)
    }

    /// Starts a new conversation in private chat `chat`.
    pub fn next_epoch(&self, chat: i64) -> Result<u64, String> {
        self.change(|s| {
            let n = s.epochs.entry(chat.to_string()).or_insert(0);
            *n += 1;
            *n
        })
    }

    /// Forgets the checkpoint and the conversation numbers (another bot, Disconnect).
    pub fn clear(&self) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.write(&DiskState::default())
    }
}

impl OffsetStore for StateFile {
    fn load(&self) -> Result<Option<i64>, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.read()?.offset)
    }

    fn save(&self, next: i64) -> Result<(), String> {
        self.change(|s| s.offset = Some(next))
    }
}

// ── Running state ───────────────────────────────────────────────────

/// What is known about the connection now (not stored).
#[derive(Debug, Clone, Default)]
struct Live {
    poll: Option<PollState>,
    last_update: Option<DateTime<Utc>>,
    send_failed_at: Option<DateTime<Utc>>,
}

struct Running {
    stop: CancellationToken,
    task: tauri::async_runtime::JoinHandle<()>,
}

/// Managed state of the Telegram connector.
pub struct TelegramState {
    data_dir: PathBuf,
    cap: Arc<AtomicUsize>,
    usage: Arc<SlackUsage>,
    files: Arc<StateFile>,
    codes: Arc<LinkCodes>,
    running: Mutex<Option<Running>>,
    live: Arc<Mutex<Live>>,
    /// One change at a time (connect, switch, disconnect, follow the settings).
    op: tokio::sync::Mutex<()>,
    pub(super) notices: TelegramNoticeState,
}

impl TelegramState {
    pub fn new(data_dir: &Path) -> Self {
        let cap = SettingsStore::in_dir(data_dir)
            .load()
            .map(|s| s.connectors.telegram.daily_cap())
            .unwrap_or(DEFAULT_DAILY_ANSWER_CAP);
        let cap = Arc::new(AtomicUsize::new(cap));
        Self {
            data_dir: data_dir.to_path_buf(),
            usage: Arc::new(SlackUsage::in_file(
                data_dir,
                USAGE_FILE,
                "Telegram",
                cap.clone(),
            )),
            cap,
            files: Arc::new(StateFile::new(data_dir)),
            codes: Arc::new(LinkCodes::default()),
            running: Mutex::new(None),
            live: Arc::new(Mutex::new(Live::default())),
            op: tokio::sync::Mutex::new(()),
            notices: TelegramNoticeState::default(),
        }
    }

    async fn settings(&self) -> Result<AppSettings, String> {
        let store = SettingsStore::in_dir(&self.data_dir);
        tokio::task::spawn_blocking(move || store.load())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    fn live(&self) -> Live {
        self.live.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn is_running(&self) -> bool {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Cancels every answer at once (before the change that stops polling is saved).
    fn cancel(&self) {
        if let Some(running) = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            running.stop.cancel();
        }
    }
}

/// Saves a change to the Telegram settings.
async fn save(
    state: &TelegramState,
    change: impl FnOnce(&mut TelegramPrefs) -> Result<(), String> + Send + 'static,
) -> Result<AppSettings, String> {
    let store = SettingsStore::in_dir(&state.data_dir);
    tokio::task::spawn_blocking(move || {
        store.update(|s| {
            change(&mut s.connectors.telegram).map_err(|reason| {
                crate::app_settings::SettingsError::InvalidValue {
                    key: "connectors.telegram".into(),
                    reason,
                }
            })
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map(|(settings, ())| settings)
    .map_err(|e| match e {
        crate::app_settings::SettingsError::InvalidValue { reason, .. } => reason,
        other => other.to_string(),
    })
}

/// A change to the bot's settings; `Err` when no bot is connected.
async fn save_bot(
    state: &TelegramState,
    change: impl FnOnce(&mut TelegramBotPrefs) -> Result<(), String> + Send + 'static,
) -> Result<AppSettings, String> {
    save(state, move |prefs| match prefs.bot.as_mut() {
        Some(bot) => change(bot),
        None => Err("Connect a Telegram bot first.".into()),
    })
    .await
}

// ── Status ──────────────────────────────────────────────────────────

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotView {
    pub username: String,
    pub name: String,
    /// `https://t.me/<bot>`.
    pub link: String,
    pub reads_all_group_messages: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonView {
    /// The Telegram user id, as text (an id for the page's buttons only).
    pub id: String,
    pub name: String,
    pub username: Option<String>,
    pub linked_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub id: String,
    pub title: String,
    pub allowed: bool,
    pub added_by: Option<String>,
    pub seen_at: String,
}

/// What the page shows. Never the token.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramStatus {
    /// `not_set_up`, `off`, `connecting`, `connected`, `reconnecting`, `error`,
    /// `local_only`.
    pub state: &'static str,
    pub bot: Option<BotView>,
    pub error: Option<String>,
    pub next_retry_at: Option<String>,
    pub retry_reason: Option<String>,
    pub last_update_at: Option<String>,
    pub owner: Option<PersonView>,
    pub people: Vec<PersonView>,
    pub groups: Vec<GroupView>,
    pub answer_workspace: Option<WorkspaceRef>,
    pub answers_today: usize,
    pub daily_cap: usize,
    pub min_daily_cap: usize,
    pub max_daily_cap: usize,
    pub resets_at: String,
    pub last_send_failed_at: Option<String>,
    pub privacy_note: &'static str,
}

/// The page's state word.
pub fn state_word(
    local_only: bool,
    bot: Option<&TelegramBotPrefs>,
    poll: Option<&PollState>,
) -> &'static str {
    let Some(bot) = bot else {
        return "not_set_up";
    };
    if local_only {
        return "local_only";
    }
    if !bot.enabled {
        return "off";
    }
    match poll {
        None | Some(PollState::Connecting) => "connecting",
        Some(PollState::Connected) => "connected",
        Some(PollState::Waiting { .. }) => "reconnecting",
        Some(PollState::Stopped(_)) => "error",
        Some(PollState::Off) => "off",
    }
}

fn person_view(p: &TelegramPerson) -> PersonView {
    PersonView {
        id: p.id.to_string(),
        name: p.name.clone(),
        username: p.username.clone(),
        linked_at: rfc3339(p.linked_at),
    }
}

/// The page's status now.
pub async fn status(app: &AppHandle) -> Result<TelegramStatus, String> {
    let state = app.state::<TelegramState>();
    let settings = state.settings().await?;
    let prefs = &settings.connectors.telegram;
    let bot = prefs.bot.as_ref();
    let live = state.live();
    let word = state_word(settings.policy.local_only, bot, live.poll.as_ref());
    let (next_retry_at, retry_reason) = match &live.poll {
        Some(PollState::Waiting {
            next_try_at,
            reason,
        }) if word == "reconnecting" => (Some(rfc3339(*next_try_at)), Some(reason.clone())),
        _ => (None, None),
    };
    let error = match (&live.poll, word) {
        (Some(PollState::Stopped(reason)), "error") => Some(reason.clone()),
        _ => None,
    };
    let answer_workspace = match bot.and_then(|b| b.answer_workspace.clone()) {
        Some(id) => Some(workspace_ref(app, &id).await),
        None => None,
    };
    let now = Utc::now();
    let usage = state.usage.clone();
    let today = utc_day(now);
    let answers_today = tokio::task::spawn_blocking(move || usage.used(today))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
        .unwrap_or_else(|e| {
            tracing::warn!(target: "shodh::telegram", error = %e, "the Telegram answer count could not be read");
            0
        });
    Ok(TelegramStatus {
        state: word,
        bot: bot.map(|b| BotView {
            username: b.username.clone(),
            name: b.name.clone(),
            link: chat_link(&b.username),
            reads_all_group_messages: b.reads_all_group_messages,
        }),
        error,
        next_retry_at,
        retry_reason,
        last_update_at: live.last_update.map(rfc3339),
        owner: bot.and_then(|b| b.owner.as_ref()).map(person_view),
        people: bot
            .map(|b| b.people.iter().map(person_view).collect())
            .unwrap_or_default(),
        groups: bot
            .map(|b| {
                b.groups
                    .iter()
                    .map(|g| GroupView {
                        id: g.id.to_string(),
                        title: g.title.clone(),
                        allowed: g.allowed,
                        added_by: g.added_by.clone(),
                        seen_at: rfc3339(g.seen_at),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        answer_workspace,
        answers_today,
        daily_cap: state.cap.load(Ordering::SeqCst),
        min_daily_cap: MIN_DAILY_ANSWER_CAP,
        max_daily_cap: MAX_DAILY_ANSWER_CAP,
        resets_at: rfc3339(next_utc_midnight(now)),
        last_send_failed_at: live.send_failed_at.map(rfc3339),
        privacy_note: TELEGRAM_PRIVACY_NOTE,
    })
}

/// Tells the page the status changed (and notifications what changed).
pub fn emit_status(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match status(&app).await {
            Ok(status) => {
                if let Some(bot) = &status.bot {
                    telegram_notices::observe_state(&app, &bot.username, status.state);
                }
                if let Err(e) = app.emit(TELEGRAM_STATUS_EVENT, status) {
                    tracing::warn!(target: "shodh::telegram", error = %e, "Telegram status not emitted");
                }
            }
            Err(e) => {
                tracing::warn!(target: "shodh::telegram", error = %e, "Telegram status unavailable")
            }
        }
    });
}

/// The bot's username for notices (empty when none is connected).
async fn bot_username(state: &TelegramState) -> String {
    state
        .settings()
        .await
        .ok()
        .and_then(|s| s.connectors.telegram.bot.map(|b| b.username))
        .unwrap_or_default()
}

struct AppPollSink {
    app: AppHandle,
    live: Arc<Mutex<Live>>,
}

impl PollSink for AppPollSink {
    fn state(&self, state: PollState) {
        match &state {
            PollState::Stopped(reason) => {
                tracing::warn!(target: "shodh::telegram", %reason, "the Telegram connector stopped")
            }
            PollState::Connected => {
                tracing::info!(target: "shodh::telegram", "connected to Telegram")
            }
            _ => {}
        }
        self.live.lock().unwrap_or_else(|e| e.into_inner()).poll = Some(state);
        emit_status(&self.app);
    }

    fn update_seen(&self, at: DateTime<Utc>) {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_update = Some(at);
        emit_status(&self.app);
    }
}

struct AppOutcomes {
    app: AppHandle,
    cap: Arc<AtomicUsize>,
    live: Arc<Mutex<Live>>,
}

impl OutcomeSink for AppOutcomes {
    fn handled(&self, outcome: &Outcome) {
        let app = self.app.clone();
        let notice = match outcome {
            Outcome::AtCap => app.try_state::<TelegramState>().and_then(|s| {
                s.notices
                    .watch
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .cap_reached(self.cap.load(Ordering::SeqCst), Utc::now())
            }),
            Outcome::ModelFailed(_) => Some(TelegramNotice::ModelFailed),
            _ => None,
        };
        if let Some(notice) = notice {
            tauri::async_runtime::spawn(async move {
                let state = app.state::<TelegramState>();
                let bot = bot_username(&state).await;
                telegram_notices::tell(&app, &bot, &notice);
            });
        }
        let changed = {
            let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
            match outcome {
                Outcome::SendFailed(e) => {
                    tracing::warn!(target: "shodh::telegram", error = %e, "an answer could not be sent to Telegram");
                    live.send_failed_at = Some(Utc::now());
                    true
                }
                Outcome::Answered(_) => live.send_failed_at.take().is_some(),
                _ => false,
            }
        };
        if changed {
            emit_status(&self.app);
        }
    }
}

// ── The app's side of answers ───────────────────────────────────────

/// Whether memories are given to the model (Settings → Memory), as for Ask.
fn inject_memories(data_dir: &Path) -> bool {
    match SettingsStore::in_dir(data_dir).load() {
        Ok(settings) => settings.memory.inject_memories,
        Err(e) => {
            tracing::warn!(target: "shodh::telegram", error = %e, "settings unreadable; memories not given to the model");
            false
        }
    }
}

/// The Actor of what Telegram does in memory.
fn telegram_actor(conversation: Option<String>) -> Actor {
    Actor {
        via: "telegram",
        principal: LOCAL_OWNER.to_string(),
        conversation_id: conversation,
        profile_id: None,
        run_id: None,
    }
}

struct AppBrain {
    app: AppHandle,
    data_dir: PathBuf,
    codes: Arc<LinkCodes>,
    files: Arc<StateFile>,
}

impl AppBrain {
    async fn bot(&self) -> Result<TelegramBotPrefs, String> {
        let store = SettingsStore::in_dir(&self.data_dir);
        let settings = tokio::task::spawn_blocking(move || store.load())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        settings
            .connectors
            .telegram
            .bot
            .ok_or_else(|| "no Telegram bot is connected".to_string())
    }

    async fn change(
        &self,
        change: impl FnOnce(&mut TelegramBotPrefs) -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        let state = self.app.state::<TelegramState>();
        let settings = save_bot(&state, change).await?;
        broadcast(&self.app, &settings);
        emit_status(&self.app);
        Ok(())
    }
}

#[async_trait]
impl TelegramBrain for AppBrain {
    async fn rules(&self) -> Result<Rules, String> {
        Ok(rules_of(&self.bot().await?))
    }

    fn redeem(&self, code: &str) -> Result<LinkKind, LinkError> {
        self.codes.redeem(code, Utc::now())
    }

    async fn link(&self, kind: LinkKind, person: &Person) -> Result<(), String> {
        let linked = TelegramPerson {
            id: person.id,
            name: person.name.clone(),
            username: person.username.clone(),
            linked_at: Utc::now(),
        };
        let kept = linked.clone();
        self.change(move |bot| {
            match kind {
                LinkKind::Owner => {
                    bot.people.retain(|p| p.id != kept.id);
                    bot.owner = Some(kept);
                }
                LinkKind::Invite => {
                    bot.people.retain(|p| p.id != kept.id);
                    bot.people.push(kept);
                }
            }
            Ok(())
        })
        .await?;
        if let Some(audit) = self.app.try_state::<AuditState>() {
            audit_change(
                &audit,
                match kind {
                    LinkKind::Owner => "telegram_owner_linked",
                    LinkKind::Invite => "telegram_person_joined",
                },
                json!({ "name": linked.name, "via": "telegram" }),
            );
        }
        if kind == LinkKind::Invite {
            let bot = self.bot().await.map(|b| b.username).unwrap_or_default();
            telegram_notices::tell(
                &self.app,
                &bot,
                &TelegramNotice::PersonJoined {
                    person: linked.id,
                    name: linked.name,
                },
            );
        }
        Ok(())
    }

    async fn membership(&self, chat: &Chat, by: Option<&Person>, joined: bool) {
        let id = chat.id;
        if joined {
            let group = TelegramGroup {
                id,
                title: chat.title.clone().unwrap_or_else(|| "A group".into()),
                allowed: false,
                added_by: by.map(|p| p.name.clone()),
                seen_at: Utc::now(),
            };
            let notice = TelegramNotice::GroupWaiting {
                group: id,
                title: group.title.clone(),
                by: group.added_by.clone(),
            };
            let mut new = false;
            let added = self
                .change(move |bot| {
                    if !bot.groups.iter().any(|g| g.id == id) {
                        bot.groups.push(group);
                    }
                    Ok(())
                })
                .await;
            if let Ok(bot) = self.bot().await {
                new = bot.groups.iter().any(|g| g.id == id && !g.allowed);
                if new {
                    telegram_notices::tell(&self.app, &bot.username, &notice);
                }
            }
            if let Err(e) = added {
                tracing::warn!(target: "shodh::telegram", error = %e, "a Telegram group could not be listed");
            }
            tracing::info!(target: "shodh::telegram", waiting = new, "the bot was added to a Telegram group");
        } else {
            if let Err(e) = self
                .change(move |bot| {
                    bot.groups.retain(|g| g.id != id);
                    Ok(())
                })
                .await
            {
                tracing::warn!(target: "shodh::telegram", error = %e, "a Telegram group could not be removed");
            }
            telegram_notices::group_decided(&self.app, id);
        }
    }

    async fn migrated(&self, from: i64, to: i64) {
        if let Err(e) = self
            .change(move |bot| {
                for group in bot.groups.iter_mut().filter(|g| g.id == from) {
                    group.id = to;
                }
                Ok(())
            })
            .await
        {
            tracing::warn!(target: "shodh::telegram", error = %e, "a Telegram group's new id could not be kept");
        }
    }

    async fn chat_ref(&self, audience: Audience, chat: &Chat, from: &Person, root: i64) -> ChatRef {
        let epoch = match audience {
            Audience::Group => 0,
            _ => {
                let files = self.files.clone();
                let id = chat.id;
                tokio::task::spawn_blocking(move || files.epoch(id))
                    .await
                    .unwrap_or(0)
            }
        };
        ChatRef {
            key: chat_key(audience, chat.id, epoch, root),
            label: chat_label(audience, &from.name, chat.title.as_deref()),
        }
    }

    async fn new_conversation(&self, chat: i64) {
        let files = self.files.clone();
        let next = tokio::task::spawn_blocking(move || files.next_epoch(chat))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
        if let Err(e) = next {
            tracing::warn!(target: "shodh::telegram", error = %e, "a new Telegram conversation could not be started");
        }
    }

    async fn guest_scope(&self) -> Result<Option<AnswerScope>, String> {
        let Some(id) = self.bot().await?.answer_workspace else {
            return Ok(None);
        };
        let detail = match self
            .app
            .state::<WorkspaceState>()
            .run(move |s| s.detail(&id))
            .await
        {
            Ok(detail) => detail,
            Err(e) if e.code == "not_found" => return Ok(None),
            Err(e) => return Err(e.message),
        };
        if detail.workspace.archived {
            return Ok(None);
        }
        let sources = answer_sources(&detail);
        // Indexed Slack channels are never answered from in Telegram.
        Ok(Some(
            AnswerScope {
                source_ids: sources.source_ids,
                files: sources.files,
                ..AnswerScope::default()
            }
            .for_channel(None),
        ))
    }

    async fn retrieve(
        &self,
        scope: Option<&AnswerScope>,
        query: &str,
    ) -> Result<Vec<AnswerPassage>, String> {
        let rag = self.app.state::<RagState>().rag.clone();
        match scope {
            Some(scope) => retrieve_passages(rag, scope, query).await,
            None => retrieve_library(rag, query, OWNER_PASSAGES).await,
        }
    }

    async fn memories(&self, message: &str) -> Option<String> {
        let dir = self.data_dir.clone();
        let inject = tokio::task::spawn_blocking(move || inject_memories(&dir))
            .await
            .unwrap_or(false);
        if !inject {
            return None;
        }
        let memory = self.app.state::<MemoryState>();
        recall_for_run(&memory, message, None, &telegram_actor(None))
            .await
            .map(|m| m.block)
    }

    async fn remember(
        &self,
        request: &MemoryRequest,
        chat: &ChatRef,
        message_id: i64,
    ) -> Option<Remembered> {
        let learn = self.app.try_state::<LearnState>()?;
        let conversation = conversations::conversation_id(Connector::Telegram, &chat.key);
        let run = format!("telegram-{}-{message_id}", chat.key);
        learn
            .file_memory_request(request, &conversation, &run, None)
            .await
            .map(|handled| match handled {
                RequestHandled::Saved => Remembered::Saved,
                RequestHandled::Filed => Remembered::Filed,
                RequestHandled::AlreadyKept => Remembered::AlreadyKept,
            })
    }

    async fn earlier(&self, chat: &ChatRef) -> Vec<ThreadTurn> {
        conversations::earlier(&self.app, Connector::Telegram, &chat.key, OWNER_TURNS)
            .await
            .into_iter()
            .map(|t| ThreadTurn {
                question: t.question,
                answer: t.answer,
            })
            .collect()
    }

    async fn keep(&self, chat: &ChatRef, asked: &Asked, answer: &str, learn: bool) {
        let link = self.bot().await.ok().map(|b| chat_link(&b.username));
        conversations::keep(
            &self.app,
            Exchange {
                connector: Connector::Telegram,
                chat: chat.key.clone(),
                label: chat.label.clone(),
                chat_link: link,
                turn_id: asked.message_id.to_string(),
                question: Said {
                    author: if asked.owner {
                        "You".to_string()
                    } else {
                        asked.author.clone()
                    },
                    owner: asked.owner,
                    text: asked.text.clone(),
                    at: asked.at,
                    link: None,
                },
                answer: answer.to_string(),
                answered_at: Utc::now(),
                learn,
            },
        )
        .await;
    }

    fn model(&self) -> Result<Arc<dyn TextModel>, String> {
        let llm = self.app.state::<LLMState>();
        crate::memory_learn::configured_model(&self.data_dir, &llm)
    }

    fn private_terms(&self) -> PrivateTerms {
        app_private_terms(&self.data_dir)
    }
}

// ── Running ─────────────────────────────────────────────────────────

/// Stops polling: answers are cancelled first, then the loop ends.
async fn stop(app: &AppHandle) {
    let state = app.state::<TelegramState>();
    let running = state
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(running) = running {
        running.stop.cancel();
        if tokio::time::timeout(EXIT_WAIT, running.task).await.is_err() {
            tracing::warn!(target: "shodh::telegram", "the Telegram polling did not stop in time");
        }
    }
    state.live.lock().unwrap_or_else(|e| e.into_inner()).poll = Some(PollState::Off);
}

/// Starts polling as `bot` (its token from the credential store).
async fn start(app: &AppHandle, bot: &TelegramBotPrefs) {
    let state = app.state::<TelegramState>();
    let set = |poll: PollState| {
        state.live.lock().unwrap_or_else(|e| e.into_inner()).poll = Some(poll);
    };
    let id = bot.bot_id;
    let token = match tokio::task::spawn_blocking(move || load_token(id))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
    {
        Ok(Some(token)) => token,
        Ok(None) => {
            set(PollState::Stopped(
                "The bot token is no longer saved on this computer. Connect again.".into(),
            ));
            return;
        }
        Err(e) => {
            set(PollState::Stopped(e));
            return;
        }
    };
    let api = match TelegramApi::new() {
        Ok(api) => api,
        Err(e) => {
            set(PollState::Stopped(e.to_string()));
            return;
        }
    };
    let runtime = match tokio::runtime::Handle::try_current() {
        Ok(runtime) => runtime,
        Err(e) => {
            set(PollState::Stopped(e.to_string()));
            return;
        }
    };
    let telegram: Arc<TokenBot> = Arc::new(TokenBot { api, token });
    let stop_token = CancellationToken::new();
    let responder = Responder::new(
        Arc::new(AppBrain {
            app: app.clone(),
            data_dir: state.data_dir.clone(),
            codes: state.codes.clone(),
            files: state.files.clone(),
        }),
        telegram.clone(),
        state.usage.clone(),
        Arc::new(SystemClock),
        stop_token.clone(),
    );
    let dispatcher = Arc::new(Dispatcher::new(
        Arc::new(responder),
        runtime,
        Arc::new(AppOutcomes {
            app: app.clone(),
            cap: state.cap.clone(),
            live: state.live.clone(),
        }),
    ));
    let task = tauri::async_runtime::spawn(poll::run(
        telegram,
        state.files.clone(),
        dispatcher,
        Arc::new(AppPollSink {
            app: app.clone(),
            live: state.live.clone(),
        }),
        stop_token.clone(),
        PollTiming::default(),
    ));
    *state.running.lock().unwrap_or_else(|e| e.into_inner()) = Some(Running {
        stop: stop_token,
        task,
    });
}

/// Brings polling in line with the settings.
async fn sync_locked(app: &AppHandle) {
    let state = app.state::<TelegramState>();
    let settings = match state.settings().await {
        Ok(settings) => settings,
        Err(e) => {
            tracing::warn!(target: "shodh::telegram", error = %e, "settings unreadable; the Telegram connector stays as it is");
            return;
        }
    };
    state
        .cap
        .store(settings.connectors.telegram.daily_cap(), Ordering::SeqCst);
    let wanted = should_run(&settings);
    if !wanted && state.is_running() {
        stop(app).await;
    }
    if wanted && !state.is_running() {
        if let Some(bot) = settings.connectors.telegram.bot.as_ref() {
            start(app, bot).await;
        }
    }
    emit_status(app);
}

async fn sync(app: &AppHandle) {
    let state = app.state::<TelegramState>();
    let _op = state.op.lock().await;
    sync_locked(app).await;
}

/// At startup: polls when the user left the connector on.
pub fn start_saved(app: &AppHandle) {
    let Some(state) = app.try_state::<TelegramState>() else {
        return;
    };
    let wanted = SettingsStore::in_dir(&state.data_dir)
        .load()
        .map(|s| should_run(&s))
        .unwrap_or(false);
    if wanted {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { sync(&app).await });
    }
}

/// After any settings change (Local-only mode switched): follow it. Answers that must
/// stop are cancelled here, at once.
pub fn settings_changed(app: &AppHandle, settings: &AppSettings) {
    let Some(state) = app.try_state::<TelegramState>() else {
        return;
    };
    let running = state.is_running();
    let wanted = should_run(settings);
    if running && !wanted {
        state.cancel();
    }
    if running != wanted {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { sync(&app).await });
    }
}

/// When the app exits: answers are cancelled and polling ends (bounded wait).
pub fn shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<TelegramState>() else {
        return;
    };
    let running = state
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(running) = running {
        running.stop.cancel();
        tauri::async_runtime::block_on(async move {
            let _ = tokio::time::timeout(EXIT_WAIT, running.task).await;
        });
    }
}

// ── Commands ────────────────────────────────────────────────────────

/// What a refused setup call tells the user (never the token).
fn setup_error(error: TelegramError) -> String {
    match error {
        TelegramError::Unauthorized => "Telegram refused that token. Copy it again from @BotFather (the message after /newbot, or /token).".into(),
        TelegramError::Network(_) => format!("{error}. Check the internet connection and try again."),
        TelegramError::BadResponse(reason) if reason.contains("not belong to a bot") => {
            "That token does not belong to a bot. Use the token @BotFather sent for your bot.".into()
        }
        other => other.to_string(),
    }
}

/// The page's status.
#[tauri::command]
pub async fn telegram_status(app: AppHandle) -> Result<TelegramStatus, String> {
    status(&app).await
}

/// Connects the bot whose token the user pasted: checked with Telegram (`getMe`) before
/// anything is saved, kept in the OS credential store, and the connector is switched on.
/// The same bot again keeps its owner, people, groups and workspace. User-only.
#[tauri::command]
pub async fn telegram_connect(
    app: AppHandle,
    state: State<'_, TelegramState>,
    audit: State<'_, AuditState>,
    token: String,
) -> Result<TelegramStatus, String> {
    let _op = state.op.lock().await;
    let settings = state.settings().await?;
    if settings.policy.local_only {
        return Err(LOCAL_ONLY.into());
    }
    let token = TelegramBotToken::parse(&token).map_err(|e| e.to_string())?;
    let api = TelegramApi::new().map_err(|e| e.to_string())?;
    let me = api.get_me(&token).await.map_err(setup_error)?;
    if let Some(other) = settings
        .connectors
        .telegram
        .bot
        .as_ref()
        .filter(|b| b.bot_id != me.id)
    {
        return Err(format!(
            "Shodh is connected to the bot @{}. Disconnect it first.",
            other.username
        ));
    }
    // New token: what the old one was doing stops now.
    state.cancel();
    let id = me.id;
    let stored = token.clone();
    tokio::task::spawn_blocking(move || {
        crate::api_key_store::store_secret(&token_user(id), stored.expose())
    })
    .await
    .map_err(|e| e.to_string())??;
    let fresh = TelegramBotPrefs {
        bot_id: me.id,
        username: me.username.clone(),
        name: me.name.clone(),
        reads_all_group_messages: me.reads_all_group_messages,
        connected_at: Utc::now(),
        enabled: true,
        owner: None,
        answer_workspace: None,
        people: Vec::new(),
        groups: Vec::new(),
    };
    let settings = save(&state, move |prefs| {
        let kept = prefs.bot.take().filter(|b| b.bot_id == fresh.bot_id);
        prefs.bot = Some(match kept {
            Some(old) => TelegramBotPrefs {
                owner: old.owner,
                answer_workspace: old.answer_workspace,
                people: old.people,
                groups: old.groups,
                ..fresh
            },
            None => fresh,
        });
        Ok(())
    })
    .await?;
    audit_change(&audit, "telegram_connect", json!({ "bot": me.username }));
    stop(&app).await;
    sync_locked(&app).await;
    broadcast(&app, &settings);
    status(&app).await
}

/// A QR code's modules, row by row (`1` dark, `0` light).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrMatrix {
    pub size: usize,
    pub rows: Vec<String>,
}

/// A link that links someone: the URL, its QR code, and when it stops working.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInvite {
    pub url: String,
    pub expires_at: String,
    pub qr: QrMatrix,
}

/// The QR code of `text`.
pub fn qr_matrix(text: &str) -> Result<QrMatrix, String> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .map_err(|e| format!("The QR code could not be made: {e}"))?;
    let size = code.width();
    let colors = code.to_colors();
    let rows = colors
        .chunks(size)
        .map(|row| {
            row.iter()
                .map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' })
                .collect()
        })
        .collect();
    Ok(QrMatrix { size, rows })
}

/// A one-time link (and its QR code) that links the owner's account (`kind: "owner"`) or
/// invites someone (`"invite"`). It works once, for ten minutes; a new one replaces the
/// previous of its kind. User-only.
#[tauri::command]
pub async fn telegram_link_code(
    state: State<'_, TelegramState>,
    kind: String,
) -> Result<LinkInvite, String> {
    let kind = match kind.as_str() {
        "owner" => LinkKind::Owner,
        "invite" => LinkKind::Invite,
        _ => return Err("Unknown kind of link.".into()),
    };
    let settings = state.settings().await?;
    let bot = settings
        .connectors
        .telegram
        .bot
        .ok_or_else(|| "Connect a Telegram bot first.".to_string())?;
    let issued = state.codes.issue(kind, Utc::now());
    let url = deep_link(&bot.username, &issued.code);
    Ok(LinkInvite {
        qr: qr_matrix(&url)?,
        url,
        expires_at: rfc3339(issued.expires_at),
    })
}

/// Unlinks the owner's Telegram account (it is refused until linked again). User-only.
#[tauri::command]
pub async fn telegram_unlink_owner(
    app: AppHandle,
    state: State<'_, TelegramState>,
    audit: State<'_, AuditState>,
) -> Result<TelegramStatus, String> {
    let _op = state.op.lock().await;
    let settings = save_bot(&state, |bot| {
        bot.owner = None;
        Ok(())
    })
    .await?;
    audit_change(&audit, "telegram_unlink_owner", json!({}));
    broadcast(&app, &settings);
    status(&app).await
}

/// Switches the connector on or off (the token stays saved). User-only.
#[tauri::command]
pub async fn telegram_set_enabled(
    app: AppHandle,
    state: State<'_, TelegramState>,
    audit: State<'_, AuditState>,
    enabled: bool,
) -> Result<TelegramStatus, String> {
    let _op = state.op.lock().await;
    let settings = state.settings().await?;
    if enabled && settings.policy.local_only {
        return Err(LOCAL_ONLY.into());
    }
    if !enabled {
        // Off means off now: answers stop before the switch is saved.
        state.cancel();
    }
    let settings = save_bot(&state, move |bot| {
        bot.enabled = enabled;
        Ok(())
    })
    .await?;
    audit_change(
        &audit,
        "telegram_set_enabled",
        json!({ "enabled": enabled }),
    );
    // Switching on again retries a connection that stopped on an error.
    stop(&app).await;
    sync_locked(&app).await;
    broadcast(&app, &settings);
    status(&app).await
}

/// Chooses the workspace people and groups are answered from (`None`: none; they are
/// told Shodh isn't set up). The owner's own chat always has the whole library.
/// User-only.
#[tauri::command]
pub async fn telegram_set_answer_workspace(
    app: AppHandle,
    state: State<'_, TelegramState>,
    workspaces: State<'_, WorkspaceState>,
    audit: State<'_, AuditState>,
    workspace_id: Option<String>,
) -> Result<TelegramStatus, String> {
    let _op = state.op.lock().await;
    let workspace_id = workspace_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    let mut name = None;
    if let Some(id) = &workspace_id {
        let lookup = id.clone();
        let detail = workspaces
            .run(move |s| s.detail(&lookup))
            .await
            .map_err(|e| e.message)?;
        if detail.workspace.archived {
            return Err("That workspace is archived. Restore it, or choose another.".into());
        }
        name = Some(detail.workspace.name);
    }
    let chosen = workspace_id.clone();
    let settings = save_bot(&state, move |bot| {
        bot.answer_workspace = chosen;
        Ok(())
    })
    .await?;
    audit_change(
        &audit,
        "telegram_set_answer_workspace",
        json!({ "workspaceId": workspace_id, "workspace": name }),
    );
    broadcast(&app, &settings);
    status(&app).await
}

fn parse_id(id: &str) -> Result<i64, String> {
    id.trim()
        .parse::<i64>()
        .map_err(|_| "That id is not a Telegram id.".to_string())
}

/// Allows a group that added the bot (`allowed`), or removes it from the list (it is
/// refused from then on, and listed again only if it adds the bot again). User-only.
#[tauri::command]
pub async fn telegram_set_group(
    app: AppHandle,
    state: State<'_, TelegramState>,
    audit: State<'_, AuditState>,
    group_id: String,
    allowed: bool,
) -> Result<TelegramStatus, String> {
    let _op = state.op.lock().await;
    let id = parse_id(&group_id)?;
    let mut title = None;
    let settings = state.settings().await?;
    if let Some(group) = settings
        .connectors
        .telegram
        .bot
        .as_ref()
        .and_then(|b| b.groups.iter().find(|g| g.id == id))
    {
        title = Some(group.title.clone());
    }
    let title = title.ok_or_else(|| "That group is not listed any more.".to_string())?;
    let settings = save_bot(&state, move |bot| {
        if allowed {
            for group in bot.groups.iter_mut().filter(|g| g.id == id) {
                group.allowed = true;
            }
        } else {
            bot.groups.retain(|g| g.id != id);
        }
        Ok(())
    })
    .await?;
    telegram_notices::group_decided(&app, id);
    audit_change(
        &audit,
        if allowed {
            "telegram_allow_group"
        } else {
            "telegram_remove_group"
        },
        json!({ "group": title }),
    );
    broadcast(&app, &settings);
    status(&app).await
}

/// Removes someone the user had invited: they are refused from then on. User-only.
#[tauri::command]
pub async fn telegram_remove_person(
    app: AppHandle,
    state: State<'_, TelegramState>,
    audit: State<'_, AuditState>,
    person_id: String,
) -> Result<TelegramStatus, String> {
    let _op = state.op.lock().await;
    let id = parse_id(&person_id)?;
    let settings = state.settings().await?;
    let name = settings
        .connectors
        .telegram
        .bot
        .as_ref()
        .and_then(|b| b.people.iter().find(|p| p.id == id))
        .map(|p| p.name.clone())
        .ok_or_else(|| "That person is not listed any more.".to_string())?;
    let settings = save_bot(&state, move |bot| {
        bot.people.retain(|p| p.id != id);
        Ok(())
    })
    .await?;
    audit_change(&audit, "telegram_remove_person", json!({ "name": name }));
    broadcast(&app, &settings);
    status(&app).await
}

/// Sets the daily cap of answers to others. User-only: the cap bounds what they spend.
#[tauri::command]
pub async fn telegram_set_daily_cap(
    app: AppHandle,
    state: State<'_, TelegramState>,
    audit: State<'_, AuditState>,
    daily_cap: usize,
) -> Result<TelegramStatus, String> {
    let _op = state.op.lock().await;
    check_daily_cap(daily_cap)?;
    let before = state.cap.load(Ordering::SeqCst);
    let settings = save(&state, move |prefs| {
        prefs.daily_answer_cap = daily_cap;
        Ok(())
    })
    .await?;
    state.cap.store(daily_cap, Ordering::SeqCst);
    if before != daily_cap {
        audit_change(
            &audit,
            "telegram_set_daily_cap",
            json!({ "old": before, "new": daily_cap }),
        );
    }
    broadcast(&app, &settings);
    status(&app).await
}

/// Disconnects the bot: cancels its answers, stops polling, switches the connector off
/// (saved first, so nothing restarts it), with `forget` removes every Telegram
/// conversation from the app, deletes the token from the credential store, and only then
/// forgets the bot. When the token cannot be deleted, the connector stays off and the page
/// says so. User-only.
#[tauri::command]
pub async fn telegram_disconnect(
    app: AppHandle,
    state: State<'_, TelegramState>,
    audit: State<'_, AuditState>,
    forget: bool,
) -> Result<TelegramStatus, String> {
    let _op = state.op.lock().await;
    let settings = state.settings().await?;
    let Some(bot) = settings.connectors.telegram.bot.clone() else {
        return status(&app).await;
    };
    state.cancel();
    let settings = save_bot(&state, |b| {
        b.enabled = false;
        Ok(())
    })
    .await?;
    stop(&app).await;
    state.codes.clear();
    broadcast(&app, &settings);
    let forgotten = if forget {
        Some(
            conversations::forget(&app, Connector::Telegram, None)
                .await
                .map_err(|e| {
                    format!("The Telegram connector is off, but its conversations could not be removed ({e}). Try Disconnect again.")
                })?,
        )
    } else {
        None
    };
    let id = bot.bot_id;
    let removed =
        tokio::task::spawn_blocking(move || crate::api_key_store::remove_secret(&token_user(id)))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
    if let Err(e) = removed {
        tracing::warn!(target: "shodh::telegram", error = %e, "the Telegram token could not be deleted");
        return Err(format!(
            "The Telegram connector is off, but its token could not be deleted from this computer's credential store ({e}). Try Disconnect again."
        ));
    }
    let files = state.files.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || files.clear())
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
    {
        tracing::warn!(target: "shodh::telegram", error = %e, "the Telegram checkpoint could not be cleared");
    }
    let settings = save(&state, |prefs| {
        prefs.bot = None;
        Ok(())
    })
    .await?;
    *state.live.lock().unwrap_or_else(|e| e.into_inner()) = Live::default();
    audit_change(
        &audit,
        "telegram_disconnect",
        json!({
            "bot": bot.username,
            "forgot": forget,
            "conversationsForgotten": forgotten,
        }),
    );
    broadcast(&app, &settings);
    status(&app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot() -> TelegramBotPrefs {
        TelegramBotPrefs {
            bot_id: 123_456_789,
            username: "shodh_notes_bot".into(),
            name: "Shodh".into(),
            reads_all_group_messages: false,
            connected_at: Utc::now(),
            enabled: true,
            owner: Some(TelegramPerson {
                id: 42,
                name: "Ann Lee".into(),
                username: None,
                linked_at: Utc::now(),
            }),
            answer_workspace: None,
            people: vec![TelegramPerson {
                id: 77,
                name: "Bob".into(),
                username: Some("bob".into()),
                linked_at: Utc::now(),
            }],
            groups: vec![
                TelegramGroup {
                    id: -100,
                    title: "Deals".into(),
                    allowed: true,
                    added_by: None,
                    seen_at: Utc::now(),
                },
                TelegramGroup {
                    id: -200,
                    title: "Waiting".into(),
                    allowed: false,
                    added_by: None,
                    seen_at: Utc::now(),
                },
            ],
        }
    }

    #[test]
    fn settings_written_before_telegram_load_with_it_off() {
        let old: AppSettings = serde_json::from_str(r#"{"seeded": true}"#).unwrap();
        assert_eq!(old.connectors.telegram, TelegramPrefs::default());
        assert_eq!(old.connectors.telegram.daily_cap(), 20);
        assert!(!should_run(&old));
        assert!(check_daily_cap(0).is_err() && check_daily_cap(1_001).is_err());
    }

    #[test]
    fn polling_runs_only_when_on_and_not_local_only() {
        let mut settings = AppSettings::default();
        settings.connectors.telegram.bot = Some(bot());
        assert!(should_run(&settings));
        settings.policy.local_only = true;
        assert!(!should_run(&settings));
        settings.policy.local_only = false;
        settings.connectors.telegram.bot.as_mut().unwrap().enabled = false;
        assert!(!should_run(&settings));
    }

    #[test]
    fn only_allowed_groups_are_answered() {
        let rules = rules_of(&bot());
        assert_eq!(rules.owner, Some(42));
        assert_eq!(rules.people, vec![77]);
        assert_eq!(rules.groups, vec![-100], "a waiting group is not allowed");
        assert_eq!(rules.bot.unwrap().username, "shodh_notes_bot");
    }

    #[test]
    fn chat_keys_and_labels() {
        assert_eq!(chat_key(Audience::Owner, 42, 0, 0), "u42-0");
        assert_eq!(chat_key(Audience::Owner, 42, 3, 0), "u42-3");
        assert_eq!(chat_key(Audience::Group, -1001234, 0, 55), "g1001234-55");
        assert_eq!(chat_label(Audience::Owner, "Ann", None), "Telegram · you");
        assert_eq!(chat_label(Audience::Guest, "Bob", None), "Telegram · Bob");
        assert_eq!(
            chat_label(Audience::Group, "Bob", Some("Deals")),
            "Telegram · Deals"
        );
    }

    #[test]
    fn the_card_state_follows_settings_and_polling() {
        let on = bot();
        assert_eq!(state_word(false, None, None), "not_set_up");
        assert_eq!(state_word(true, Some(&on), None), "local_only");
        assert_eq!(state_word(false, Some(&on), None), "connecting");
        assert_eq!(
            state_word(false, Some(&on), Some(&PollState::Connected)),
            "connected"
        );
        assert_eq!(
            state_word(false, Some(&on), Some(&PollState::Stopped("x".into()))),
            "error"
        );
        let mut off = bot();
        off.enabled = false;
        assert_eq!(
            state_word(false, Some(&off), Some(&PollState::Connected)),
            "off"
        );
    }

    #[test]
    fn the_state_file_keeps_the_checkpoint_and_new_conversations() {
        let dir = tempfile::tempdir().unwrap();
        let file = StateFile::new(dir.path());
        assert_eq!(file.load().unwrap(), None);
        file.save(41).unwrap();
        assert_eq!(
            StateFile::new(dir.path()).load().unwrap(),
            Some(41),
            "resumes"
        );
        assert_eq!(file.epoch(42), 0);
        assert_eq!(file.next_epoch(42).unwrap(), 1);
        assert_eq!(file.epoch(42), 1);
        assert_eq!(file.load().unwrap(), Some(41), "the checkpoint stays");
        file.clear().unwrap();
        assert_eq!(file.load().unwrap(), None);
        assert_eq!(file.epoch(42), 0);
    }

    #[test]
    fn the_status_and_settings_never_carry_the_token() {
        let mut settings = AppSettings::default();
        settings.connectors.telegram.bot = Some(bot());
        let text = serde_json::to_string(&settings).unwrap();
        assert!(!text.contains("AAH4"));
        assert!(!crate::api_key_store::is_known_provider(&token_user(123)));
        assert_eq!(token_user(123), "telegram-bot-token:123");
    }

    #[test]
    fn the_link_qr_code_is_square() {
        let qr = qr_matrix("https://t.me/shodh_notes_bot?start=abcdefghijklmnopqrstuvwx").unwrap();
        assert!(qr.size >= 21);
        assert_eq!(qr.rows.len(), qr.size);
        assert!(qr.rows.iter().all(|r| r.len() == qr.size));
        // A finder pattern's top row in the corner.
        assert!(qr.rows[0].starts_with("1111111"));
    }

    #[test]
    fn a_refused_token_never_echoes_it() {
        let text = setup_error(TelegramError::Unauthorized);
        assert!(text.contains("@BotFather"));
        let text = setup_error(TelegramError::Network("timed out".into()));
        assert!(text.contains("internet connection"), "{text}");
    }
}
