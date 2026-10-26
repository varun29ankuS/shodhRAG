//! The Slack connector in the app: its settings, tokens, connection and the commands of
//! its card in the Connectors view (see `shodh_rag::connectors::slack` for the connector
//! itself).
//!
//! - **Settings** (`connectors.slack` in `app_settings.json`, user-only): the installations
//!   (one per Slack workspace, by team id; the card shows one), whether each is on, the
//!   Shodh workspace it answers from, the Slack channels it answers in (none at first),
//!   and the daily cap of answers.
//! - **Tokens** in the OS credential store (`slack-app-token:<team>`,
//!   `slack-bot-token:<team>`); never logged, never sent to the frontend, deleted on
//!   Disconnect.
//! - **Connection**: only while the connector is on and Local-only mode is off. Turning
//!   Local-only mode on stops it; turning it off again resumes it. Closed on exit.
//! - **Indexing** chosen channels into Shodh workspaces and **memories** saved from Slack:
//!   [`super::slack_index`] (state in `shodh.db`).
//! - **Answers** come from the selected workspace only ([`AppAnswerSource`]), in the
//!   chosen channels and to DMs from members of the installing Slack workspace only
//!   ([`AppAccessRules`]). Until a workspace is selected, or after it was deleted or
//!   archived, the bot says it isn't set up. Switching off, Disconnect, Local-only mode and
//!   new tokens cancel every answer queued or running at once.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::connectors::slack::access::{
    AccessRules, AccessRulesSource, AllowedChannel, SlackAccessGate,
};
use shodh_rag::connectors::slack::answer::{
    library_listing, retrieve_passages, AnswerPassage, AnswerScope, LibraryListing, ThreadTurn,
};
use shodh_rag::connectors::slack::events::{
    EventRouter, Question, QuestionKind, SeenEntry, SeenStore, ThreadKey,
};
use shodh_rag::connectors::slack::index::SyncGate;
use shodh_rag::connectors::slack::respond::{
    AnswerQuota, AnswerSource, Outcome, Responder, SlackPoster,
};
use shodh_rag::connectors::slack::sanitize::PrivateTerms;
use shodh_rag::connectors::slack::service::{AnswerService, OutcomeSink, SeenPersister};
use shodh_rag::connectors::slack::setup::{
    check_scopes, create_app_url, manifest_events, manifest_scopes, missing_scopes_message,
    ScopeCheck, APP_TOKEN_SCOPE,
};
use shodh_rag::connectors::slack::socket::{
    supervise, LinkState, SlackSocketOpener, StatusSink, Timing,
};
use shodh_rag::connectors::slack::store::{SlackMemory, SlackStore};
use shodh_rag::connectors::slack::tokens::{
    SlackAppToken, SlackBotToken, SlackCredentialStore, SlackCredentials,
};
use shodh_rag::connectors::slack::web::{
    BotChannel, BotDirectory, BotPoster, SlackApiError, SlackWeb,
};
use shodh_rag::connectors::slack::{CancellationToken, SlackInstallation};
use shodh_rag::research::results::TextModel;
use shodh_rag::research::tables::calls::{next_utc_midnight, utc_day, SystemClock};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{watch, Notify};

use super::conversations::{self, key_part, Connector, Exchange, Said};
use super::slack_index::{self, AppMemories, Database, IndexStatus, SlackOwner};
use super::slack_notices::{self, SlackNoticeState};
use super::slack_usage::SlackUsage;
use crate::app_settings::{broadcast, AppSettings, SettingsStore};
use crate::audit_commands::AuditState;
use crate::durable_file::{self, Loaded, ReadFailure};
use crate::llm_commands::LLMState;
use crate::rag_commands::RagState;
use crate::workspace_commands::{answer_sources, WorkspaceState};

/// Emitted with the [`SlackStatus`] whenever it changes.
pub const SLACK_STATUS_EVENT: &str = "connectors-slack-status";

/// Slack answers per UTC day when the user has not set a cap.
pub const DEFAULT_DAILY_ANSWER_CAP: usize = 20;
pub const MIN_DAILY_ANSWER_CAP: usize = 1;
/// OpenRouter's daily request limit for a key with credits: more could not be spent.
pub const MAX_DAILY_ANSWER_CAP: usize = 1_000;

/// Credential-store users of an installation's tokens (never provider ids, so the
/// provider key commands cannot reach them).
const APP_TOKEN_USER: &str = "slack-app-token";
const BOT_TOKEN_USER: &str = "slack-bot-token";

/// The longest wait for a connection to close when the app exits.
const EXIT_WAIT: Duration = Duration::from_secs(3);

/// The card's status is sent at most this often while Slack events stream in.
const STATUS_EVERY: Duration = Duration::from_secs(1);

const LOCAL_ONLY: &str =
    "Local-only mode is on, so Shodh does not connect to Slack. Turn it off in Settings → Privacy to use the Slack connector.";

// ── Settings ────────────────────────────────────────────────────────

/// A Slack channel Shodh answers mentions in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerChannel {
    pub id: String,
    pub name: String,
    /// Shared with other organizations (Slack Connect) when the user chose it.
    #[serde(default)]
    pub shared: bool,
}

/// One Slack workspace the connector is installed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackInstallationPrefs {
    pub installation: SlackInstallation,
    /// The connector runs (connects to Slack) for this installation.
    #[serde(default)]
    pub enabled: bool,
    /// The Shodh workspace answers come from; `None` until the user selects one.
    #[serde(default)]
    pub answer_workspace: Option<String>,
    /// The channels mentions are answered in; empty (nowhere) until the user chooses.
    #[serde(default)]
    pub answer_channels: Vec<AnswerChannel>,
    /// The Slack member the user picked as themselves: their `remember:` goes into the
    /// user's memories. `None` until picked.
    #[serde(default)]
    pub owner: Option<SlackOwner>,
}

/// The Slack connector's settings. User-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SlackPrefs {
    /// The most Slack answers (model calls) per UTC day.
    pub daily_answer_cap: usize,
    /// By team id. The card shows and manages the first.
    pub installations: Vec<SlackInstallationPrefs>,
    /// How far back a newly indexed channel's first backfill reaches, in days.
    pub index_window_days: u32,
}

impl Default for SlackPrefs {
    fn default() -> Self {
        Self {
            daily_answer_cap: DEFAULT_DAILY_ANSWER_CAP,
            installations: Vec::new(),
            index_window_days: slack_index::default_window_days(),
        }
    }
}

/// Whether `cap` is a cap the card accepts.
pub fn check_daily_cap(cap: usize) -> Result<(), String> {
    if (MIN_DAILY_ANSWER_CAP..=MAX_DAILY_ANSWER_CAP).contains(&cap) {
        Ok(())
    } else {
        Err(format!(
            "The daily limit of Slack answers is between {MIN_DAILY_ANSWER_CAP} and {MAX_DAILY_ANSWER_CAP}."
        ))
    }
}

impl SlackPrefs {
    /// The cap to use: the stored one when in range (a hand-edited file may hold any
    /// number), else the default.
    pub fn daily_cap(&self) -> usize {
        match check_daily_cap(self.daily_answer_cap) {
            Ok(()) => self.daily_answer_cap,
            Err(e) => {
                tracing::warn!(target: "shodh::slack", cap = self.daily_answer_cap, error = %e, "the stored Slack answer cap is out of range; the default is used");
                DEFAULT_DAILY_ANSWER_CAP
            }
        }
    }

    pub fn find(&self, team_id: &str) -> Option<&SlackInstallationPrefs> {
        self.installations
            .iter()
            .find(|i| i.installation.team_id == team_id)
    }

    /// The history window to use: the stored one when in range, else the default.
    pub fn index_window(&self) -> u32 {
        match shodh_rag::connectors::slack::index::check_window_days(self.index_window_days) {
            Ok(()) => self.index_window_days,
            Err(_) => slack_index::default_window_days(),
        }
    }

    pub(super) fn find_mut(&mut self, team_id: &str) -> Option<&mut SlackInstallationPrefs> {
        self.installations
            .iter_mut()
            .find(|i| i.installation.team_id == team_id)
    }

    /// The installation the card manages.
    pub fn current(&self) -> Option<&SlackInstallationPrefs> {
        self.installations.first()
    }
}

/// Whether `installation` should be connected under `settings`.
pub fn should_run(settings: &AppSettings, installation: &SlackInstallationPrefs) -> bool {
    installation.enabled && !settings.policy.local_only
}

/// The answer rules of `installation`: its team, and the channels the user chose.
pub fn access_rules(installation: &SlackInstallationPrefs) -> AccessRules {
    AccessRules {
        team_id: installation.installation.team_id.clone(),
        channels: installation
            .answer_channels
            .iter()
            .map(|c| AllowedChannel {
                id: c.id.clone(),
                shared: c.shared,
            })
            .collect(),
    }
}

/// The channels `chosen` (ids) among the channels the bot is in, as kept in settings.
/// An id the bot is not in is refused: it cannot be chosen from the card.
pub fn choose_channels(
    chosen: &[String],
    available: &[BotChannel],
) -> Result<Vec<AnswerChannel>, String> {
    let mut channels = Vec::new();
    for id in chosen {
        let id = id.trim();
        if id.is_empty() || channels.iter().any(|c: &AnswerChannel| c.id == id) {
            continue;
        }
        let Some(channel) = available.iter().find(|c| c.id == id) else {
            return Err(
                "Shodh is not in one of those channels any more. Reload the list and choose again."
                    .into(),
            );
        };
        channels.push(AnswerChannel {
            id: channel.id.clone(),
            name: channel.name.clone(),
            shared: channel.shared,
        });
    }
    Ok(channels)
}

/// The required scopes the installation's bot was not granted when it connected (none
/// when Slack did not list them).
pub fn missing_scopes(installation: &SlackInstallation) -> Vec<String> {
    if installation.scopes.is_empty() {
        return Vec::new();
    }
    match check_scopes(Some(&installation.scopes)) {
        ScopeCheck::Missing(missing) => missing,
        ScopeCheck::Complete | ScopeCheck::Unknown => Vec::new(),
    }
}

// ── Tokens ──────────────────────────────────────────────────────────

/// The OS credential store, under this profile's service.
pub struct KeyringCredentials;

fn token_user(kind: &str, team_id: &str) -> String {
    format!("{kind}:{team_id}")
}

impl SlackCredentialStore for KeyringCredentials {
    fn load(&self, team_id: &str) -> Result<Option<SlackCredentials>, String> {
        let app = crate::api_key_store::load_secret(&token_user(APP_TOKEN_USER, team_id))?;
        let bot = crate::api_key_store::load_secret(&token_user(BOT_TOKEN_USER, team_id))?;
        let (Some(app), Some(bot)) = (app, bot) else {
            return Ok(None);
        };
        Ok(Some(SlackCredentials {
            app_token: SlackAppToken::parse(&app)
                .map_err(|e| format!("The saved app-level token is not usable: {e}"))?,
            bot_token: SlackBotToken::parse(&bot)
                .map_err(|e| format!("The saved bot token is not usable: {e}"))?,
        }))
    }

    fn store(&self, team_id: &str, credentials: &SlackCredentials) -> Result<(), String> {
        crate::api_key_store::store_secret(
            &token_user(APP_TOKEN_USER, team_id),
            credentials.app_token.expose(),
        )?;
        crate::api_key_store::store_secret(
            &token_user(BOT_TOKEN_USER, team_id),
            credentials.bot_token.expose(),
        )
    }

    fn remove(&self, team_id: &str) -> Result<(), String> {
        let app = crate::api_key_store::remove_secret(&token_user(APP_TOKEN_USER, team_id));
        let bot = crate::api_key_store::remove_secret(&token_user(BOT_TOKEN_USER, team_id));
        app.and(bot)
    }
}

// ── Names ───────────────────────────────────────────────────────────

/// The OS account's display name ("Ada Lovelace"), when the OS keeps one: never to appear
/// in Slack. Read once.
fn os_display_name() -> Option<String> {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    NAME.get_or_init(read_os_display_name).clone()
}

#[cfg(windows)]
fn read_os_display_name() -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
    let mut buffer = [0u16; 512];
    let mut size = u32::try_from(buffer.len()).unwrap_or(0);
    // SAFETY: the buffer holds `size` UTF-16 units; the call writes at most that many and
    // sets `size` to the length written (without the NUL), or fails and writes nothing
    // usable (an account without a display name fails with ERROR_NONE_MAPPED).
    let ok = unsafe { GetUserNameExW(NameDisplay, PWSTR(buffer.as_mut_ptr()), &mut size) };
    if ok.0 == 0 {
        return None;
    }
    let len = usize::try_from(size).ok()?.min(buffer.len());
    let name = String::from_utf16_lossy(&buffer[..len]);
    let name = name.trim_end_matches('\0').trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(not(windows))]
fn read_os_display_name() -> Option<String> {
    let user = std::env::var("USER").ok()?;
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd_display_name(&passwd, &user)
}

/// The display name of `user` in an `/etc/passwd` text (the first part of the GECOS
/// field).
#[cfg_attr(windows, allow(dead_code))]
fn passwd_display_name(passwd: &str, user: &str) -> Option<String> {
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.first() != Some(&user) {
            return None;
        }
        let name = fields.get(4)?.split(',').next()?.trim();
        (!name.is_empty()).then(|| name.to_string())
    })
}

// ── State ───────────────────────────────────────────────────────────

/// What is known about a connection now (not stored).
#[derive(Debug, Clone, Default)]
struct Live {
    link: Option<LinkState>,
    last_event: Option<DateTime<Utc>>,
    /// Slack refused a reply (a missing scope, a revoked bot token): shown on the card.
    problem: Option<String>,
    /// The app's open connections, from the last `hello`.
    connections: Option<u32>,
    /// The last answer could not be posted in Slack, after its retries.
    post_failed_at: Option<DateTime<Utc>>,
    /// Channels chosen while not shared that are shared with other organizations now.
    now_shared: BTreeSet<String>,
}

struct Running {
    stop: watch::Sender<bool>,
    /// Cancels every answer of this connection, queued or running.
    answers: CancellationToken,
    task: tauri::async_runtime::JoinHandle<()>,
}

/// One installation's event router and the file its seen event ids are kept in: they
/// outlive restarts of the connection, so a retry delivered across one costs nothing.
struct Routing {
    router: Arc<EventRouter>,
    seen: Arc<SeenPersister>,
}

/// Sends the card's status at most once per [`STATUS_EVERY`]: the first request is sent
/// at once, the ones during the wait are sent together after it.
#[derive(Default)]
pub(super) struct StatusPacer {
    running: AtomicBool,
    again: AtomicBool,
}

impl StatusPacer {
    /// True when the caller should start sending (no round is running).
    pub(super) fn request(&self) -> bool {
        self.again.store(true, Ordering::SeqCst);
        !self.running.swap(true, Ordering::SeqCst)
    }

    /// Called before each send: takes the pending request.
    pub(super) fn take(&self) -> bool {
        self.again.swap(false, Ordering::SeqCst)
    }

    /// Called after a round's wait: true when another request came meanwhile and this
    /// sender goes on.
    pub(super) fn finish_round(&self) -> bool {
        if self.again.load(Ordering::SeqCst) {
            return true;
        }
        self.running.store(false, Ordering::SeqCst);
        // A request between the check and the store found `running` true and left it to
        // this sender: take it back.
        self.again.load(Ordering::SeqCst) && !self.running.swap(true, Ordering::SeqCst)
    }
}

/// Managed state of the Slack connector.
pub struct SlackState {
    data_dir: PathBuf,
    cap: Arc<AtomicUsize>,
    usage: Arc<SlackUsage>,
    running: Mutex<HashMap<String, Running>>,
    routing: Mutex<HashMap<String, Routing>>,
    live: Arc<Mutex<HashMap<String, Live>>>,
    pacer: Arc<StatusPacer>,
    /// One change at a time (connect, switch, disconnect, follow the settings).
    op: tokio::sync::Mutex<()>,
    /// `shodh.db`, where indexing and memories from Slack are kept (opened on first use).
    database: Database,
    store: tokio::sync::OnceCell<Arc<SlackStore>>,
    /// Per Slack workspace: held by each sync step and by forgetting, so a step never
    /// adds a thread back to a channel being forgotten.
    gates: Mutex<HashMap<String, Arc<SyncGate>>>,
    /// Per Slack workspace: wakes its sync (an event, a channel chosen).
    wakes: Mutex<HashMap<String, Arc<Notify>>>,
    /// What notifications and the Inbox were told ([`slack_notices`]).
    pub(super) notices: SlackNoticeState,
}

impl SlackState {
    /// The connector of the app data directory `data_dir`, keeping its indexing state in
    /// `database`. Reads the cap from the settings; connects nothing.
    pub fn new(data_dir: &Path, database: Database) -> Self {
        let cap = SettingsStore::in_dir(data_dir)
            .load()
            .map(|s| s.connectors.slack.daily_cap())
            .unwrap_or(DEFAULT_DAILY_ANSWER_CAP);
        let cap = Arc::new(AtomicUsize::new(cap));
        Self {
            data_dir: data_dir.to_path_buf(),
            usage: Arc::new(SlackUsage::new(data_dir, cap.clone())),
            cap,
            running: Mutex::new(HashMap::new()),
            routing: Mutex::new(HashMap::new()),
            live: Arc::new(Mutex::new(HashMap::new())),
            pacer: Arc::new(StatusPacer::default()),
            op: tokio::sync::Mutex::new(()),
            database,
            store: tokio::sync::OnceCell::new(),
            gates: Mutex::new(HashMap::new()),
            wakes: Mutex::new(HashMap::new()),
            notices: SlackNoticeState::default(),
        }
    }

    /// The Slack tables of `shodh.db`, opened on first use (tried again on the next call
    /// when it fails).
    pub(super) async fn store(&self) -> Result<Arc<SlackStore>, String> {
        self.store
            .get_or_try_init(|| slack_index::open_store(&self.database))
            .await
            .cloned()
    }

    /// The sync gate of Slack workspace `team_id`.
    pub(super) fn gate(&self, team_id: &str) -> Arc<SyncGate> {
        self.gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(team_id.to_string())
            .or_default()
            .clone()
    }

    /// What wakes the sync of Slack workspace `team_id`.
    pub(super) fn wake(&self, team_id: &str) -> Arc<Notify> {
        self.wakes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(team_id.to_string())
            .or_default()
            .clone()
    }

    /// The settings, read off the async workers.
    pub(super) async fn settings(&self) -> Result<AppSettings, String> {
        let store = SettingsStore::in_dir(&self.data_dir);
        tokio::task::spawn_blocking(move || store.load())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    fn live_of(&self, team_id: &str) -> Live {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(team_id)
            .cloned()
            .unwrap_or_default()
    }

    fn set_link(&self, team_id: &str, link: LinkState) {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(team_id.to_string())
            .or_default()
            .link = Some(link);
    }

    fn is_running(&self, team_id: &str) -> bool {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(team_id)
    }

    /// Cancels every answer of `team_id`, queued or running, at once (before the settings
    /// change that stops the connection is even saved).
    fn cancel_answers(&self, team_id: &str) {
        if let Some(running) = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(team_id)
        {
            running.answers.cancel();
        }
    }
}

// ── Seen events on disk ─────────────────────────────────────────────

/// The seen event ids of one installation, next to the settings
/// (`connectors/slack_seen_<team>.json`, written atomically with a backup).
struct FileSeenStore {
    path: PathBuf,
}

impl FileSeenStore {
    fn new(data_dir: &Path, team_id: &str) -> Self {
        let safe: String = team_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        Self {
            path: data_dir
                .join("connectors")
                .join(format!("slack_seen_{safe}.json")),
        }
    }
}

impl SeenStore for FileSeenStore {
    fn load(&self) -> Result<Vec<SeenEntry>, String> {
        match durable_file::read_with_backup(
            &self.path,
            |text| serde_json::from_str::<Vec<SeenEntry>>(text).map_err(|e| e.to_string()),
            Vec::new,
        ) {
            Ok(Loaded { value, .. }) => Ok(value),
            // A damaged list starts empty: at worst one retry is answered twice.
            Err(ReadFailure::Damaged { problem, .. }) => {
                tracing::warn!(target: "shodh::slack", %problem, "the seen Slack events were damaged; starting empty");
                Ok(Vec::new())
            }
            Err(ReadFailure::Io { message, .. }) => Err(message),
        }
    }

    fn save(&self, entries: &[SeenEntry]) -> Result<(), String> {
        let text = serde_json::to_string(entries).map_err(|e| e.to_string())?;
        durable_file::write_atomic(&self.path, text.as_bytes()).map_err(|e| e.to_string())
    }
}

// ── Status ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationView {
    pub team_name: String,
    pub bot_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRef {
    pub id: String,
    pub name: String,
    /// False when the workspace was deleted or archived: the bot then says it isn't set
    /// up.
    pub available: bool,
}

/// A channel Shodh answers in, as the card shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelView {
    pub id: String,
    pub name: String,
    /// Shared with other organizations (Slack Connect).
    pub shared: bool,
    /// Chosen while not shared, shared now: refused until chosen again.
    pub now_shared: bool,
}

/// A channel the bot is in, offered on the card.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelChoice {
    pub id: String,
    pub name: String,
    pub private: bool,
    pub shared: bool,
    pub selected: bool,
    /// The Shodh workspace the channel is indexed into, if it is.
    pub indexed_into: Option<String>,
}

/// What the card shows. Never a token.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackStatus {
    /// The Slack workspace's id, for the app only (never sent to the card).
    #[serde(skip)]
    pub team_id: Option<String>,
    /// `not_set_up`, `off`, `connecting`, `connected`, `reconnecting`, `error`,
    /// `local_only`.
    pub state: &'static str,
    pub installation: Option<InstallationView>,
    pub error: Option<String>,
    pub next_retry_at: Option<String>,
    pub retry_reason: Option<String>,
    pub last_event_at: Option<String>,
    pub answers_today: usize,
    pub daily_cap: usize,
    pub min_daily_cap: usize,
    pub max_daily_cap: usize,
    pub resets_at: String,
    pub answer_workspace: Option<WorkspaceRef>,
    pub answer_channels: Vec<ChannelView>,
    /// Another connection (another Shodh) uses the same Slack app.
    pub other_connections: bool,
    /// When the last answer could not be posted in Slack.
    pub last_post_failed_at: Option<String>,
    /// Scopes the bot lacks (an app created from an earlier manifest).
    pub missing_scopes: Vec<String>,
    /// Channels indexed into Shodh workspaces, memories from Slack, the owner.
    pub index: IndexStatus,
    pub create_app_url: String,
    pub bot_scopes: Vec<&'static str>,
    pub bot_events: Vec<&'static str>,
    pub app_token_scope: &'static str,
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The card's state word for an installation.
pub fn state_word(
    local_only: bool,
    installation: Option<&SlackInstallationPrefs>,
    link: Option<&LinkState>,
) -> &'static str {
    let Some(installation) = installation else {
        return "not_set_up";
    };
    if local_only {
        return "local_only";
    }
    if !installation.enabled {
        return "off";
    }
    match link {
        None | Some(LinkState::Connecting) => "connecting",
        Some(LinkState::Connected) => "connected",
        Some(LinkState::Reconnecting { .. }) => "reconnecting",
        Some(LinkState::Error(_)) => "error",
        Some(LinkState::Off) => "off",
    }
}

pub(super) async fn workspace_ref(app: &AppHandle, id: &str) -> WorkspaceRef {
    let lookup = id.to_string();
    let detail = app
        .state::<WorkspaceState>()
        .run(move |s| s.detail(&lookup))
        .await;
    match detail {
        Ok(detail) => WorkspaceRef {
            id: id.to_string(),
            name: detail.workspace.name,
            available: !detail.workspace.archived,
        },
        Err(_) => WorkspaceRef {
            id: id.to_string(),
            name: String::new(),
            available: false,
        },
    }
}

/// The card's status now.
pub async fn status(app: &AppHandle) -> Result<SlackStatus, String> {
    let state = app.state::<SlackState>();
    let settings = state.settings().await?;
    let prefs = &settings.connectors.slack;
    let current = prefs.current();
    let live = current
        .map(|i| state.live_of(&i.installation.team_id))
        .unwrap_or_default();
    let word = state_word(settings.policy.local_only, current, live.link.as_ref());
    let (next_retry_at, retry_reason) = match &live.link {
        Some(LinkState::Reconnecting {
            next_try_at,
            reason,
        }) if word == "reconnecting" => (Some(rfc3339(*next_try_at)), Some(reason.clone())),
        _ => (None, None),
    };
    let error = match (&live.link, word) {
        (Some(LinkState::Error(message)), "error") => Some(message.clone()),
        (_, "connected") | (_, "reconnecting") | (_, "connecting") => live.problem.clone(),
        _ => None,
    };
    let answer_workspace = match current.and_then(|i| i.answer_workspace.clone()) {
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
            tracing::warn!(target: "shodh::slack", error = %e, "the Slack answer count could not be read");
            0
        });
    let running = word == "connected" || word == "reconnecting" || word == "connecting";
    let index = slack_index::index_status(app, current, prefs.index_window()).await;
    Ok(SlackStatus {
        team_id: current.map(|i| i.installation.team_id.clone()),
        state: word,
        installation: current.map(|i| InstallationView {
            team_name: i.installation.team_name.clone(),
            bot_name: i.installation.bot_name.clone(),
        }),
        error,
        next_retry_at,
        retry_reason,
        last_event_at: live.last_event.map(rfc3339),
        answers_today,
        daily_cap: state.cap.load(Ordering::SeqCst),
        min_daily_cap: MIN_DAILY_ANSWER_CAP,
        max_daily_cap: MAX_DAILY_ANSWER_CAP,
        resets_at: rfc3339(next_utc_midnight(now)),
        answer_workspace,
        answer_channels: current
            .map(|i| {
                i.answer_channels
                    .iter()
                    .map(|c| ChannelView {
                        id: c.id.clone(),
                        name: c.name.clone(),
                        shared: c.shared,
                        now_shared: live.now_shared.contains(&c.id),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        other_connections: running && live.connections.is_some_and(|n| n > 1),
        last_post_failed_at: live.post_failed_at.map(rfc3339),
        missing_scopes: current
            .map(|i| missing_scopes(&i.installation))
            .unwrap_or_default(),
        index,
        create_app_url: create_app_url(),
        bot_scopes: manifest_scopes(),
        bot_events: manifest_events(),
        app_token_scope: APP_TOKEN_SCOPE,
    })
}

/// Tells the card the status changed (computed in the background, at most once per
/// [`STATUS_EVERY`]).
pub fn emit_status(app: &AppHandle) {
    let Some(state) = app.try_state::<SlackState>() else {
        return;
    };
    let pacer = state.pacer.clone();
    if !pacer.request() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            if pacer.take() {
                match status(&app).await {
                    Ok(status) => {
                        slack_notices::observe_status(&app, &status);
                        if let Err(e) = app.emit(SLACK_STATUS_EVENT, status) {
                            tracing::warn!(target: "shodh::slack", error = %e, "Slack status not emitted");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(target: "shodh::slack", error = %e, "Slack status unavailable")
                    }
                }
            }
            tokio::time::sleep(STATUS_EVERY).await;
            if !pacer.finish_round() {
                return;
            }
        }
    });
}

/// Connection states and event times of one installation, kept for the card.
struct AppStatusSink {
    app: AppHandle,
    team_id: String,
    live: Arc<Mutex<HashMap<String, Live>>>,
}

impl AppStatusSink {
    fn update(&self, change: impl FnOnce(&mut Live)) {
        {
            let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
            change(live.entry(self.team_id.clone()).or_default());
        }
        emit_status(&self.app);
    }
}

impl StatusSink for AppStatusSink {
    fn link(&self, state: LinkState) {
        match &state {
            LinkState::Error(message) => {
                tracing::warn!(target: "shodh::slack", %message, "the Slack connector stopped")
            }
            LinkState::Connected => tracing::info!(target: "shodh::slack", "connected to Slack"),
            _ => {}
        }
        self.update(|entry| {
            if state == LinkState::Connected {
                entry.problem = None;
            }
            entry.link = Some(state);
        });
    }

    fn event_seen(&self, at: DateTime<Utc>) {
        self.update(|entry| entry.last_event = Some(at));
    }

    fn connections(&self, count: u32) {
        if count > 1 {
            tracing::warn!(target: "shodh::slack", count, "another connection uses this Slack app; Slack splits the events between them");
        }
        self.update(|entry| entry.connections = Some(count));
    }
}

/// Posts as the bot and reports Slack's refusals that the user must fix (a missing
/// scope, a revoked bot token) on the card.
struct ObservedPoster {
    inner: BotPoster,
    app: AppHandle,
    team_id: String,
    live: Arc<Mutex<HashMap<String, Live>>>,
}

/// The card's words for a refusal the user must fix, if it is one.
fn refusal_problem(e: &SlackApiError) -> Option<String> {
    match e {
        SlackApiError::Api {
            code,
            needed: Some(scope),
        } if code == "missing_scope" => Some(missing_scopes_message(std::slice::from_ref(scope))),
        e if e.is_auth() => Some(format!(
            "Slack refused the bot token ({}). Reinstall the Slack app and connect again with the new bot token.",
            e.code().unwrap_or("refused")
        )),
        _ => None,
    }
}

impl ObservedPoster {
    fn observe<T>(&self, result: Result<T, SlackApiError>) -> Result<T, SlackApiError> {
        if let Err(e) = &result {
            if let Some(problem) = refusal_problem(e) {
                self.live
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(self.team_id.clone())
                    .or_default()
                    .problem = Some(problem);
                emit_status(&self.app);
            }
        }
        result
    }
}

#[async_trait]
impl SlackPoster for ObservedPoster {
    async fn post(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<String, SlackApiError> {
        let result = self.inner.post(channel, thread_ts, text).await;
        self.observe(result)
    }

    async fn update(&self, channel: &str, ts: &str, text: &str) -> Result<(), SlackApiError> {
        let result = self.inner.update(channel, ts, text).await;
        self.observe(result)
    }
}

/// Shows on the card when the last answer could not be posted in Slack, and tells the
/// user when the model failed an answer or the day's answers ran out
/// ([`slack_notices`]).
struct AppOutcomes {
    app: AppHandle,
    team_id: String,
    team_name: String,
    cap: Arc<AtomicUsize>,
    live: Arc<Mutex<HashMap<String, Live>>>,
}

impl OutcomeSink for AppOutcomes {
    fn handled(&self, outcome: &Outcome) {
        match outcome {
            Outcome::AtCap => slack_notices::cap_reached(
                &self.app,
                &self.team_id,
                &self.team_name,
                self.cap.load(Ordering::SeqCst),
            ),
            Outcome::ModelFailed(_) => slack_notices::tell(
                &self.app,
                &self.team_id,
                &self.team_name,
                &slack_notices::SlackNotice::ModelFailed,
            ),
            _ => {}
        }
        let changed = {
            let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
            let entry = live.entry(self.team_id.clone()).or_default();
            match outcome {
                Outcome::SlackFailed(_) => {
                    entry.post_failed_at = Some(Utc::now());
                    true
                }
                Outcome::Cancelled => false,
                _ => entry.post_failed_at.take().is_some(),
            }
        };
        if changed {
            emit_status(&self.app);
        }
    }
}

// ── Conversations in the app ────────────────────────────────────────

/// The chat key of a Slack thread in the app's conversations: one conversation per thread
/// (a DM question outside a thread is its own, [`Question::turns_key`]).
pub fn slack_chat(team_id: &str, thread: &ThreadKey) -> String {
    format!(
        "{}{}",
        slack_channel_prefix(team_id, &thread.channel),
        key_part(&thread.thread)
    )
}

/// What the chat keys of every thread of `channel` start with.
pub fn slack_channel_prefix(team_id: &str, channel: &str) -> String {
    format!("{}{}-", slack_team_prefix(team_id), key_part(channel))
}

/// What the chat keys of every thread of Slack workspace `team_id` start with.
pub fn slack_team_prefix(team_id: &str) -> String {
    format!("{}-", key_part(team_id))
}

/// Slack's escaped text (`&amp;`, `&lt;`, `&gt;`) as the app shows it.
pub fn unescape_slack(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// When a Slack message was sent, from its `ts` (seconds since the epoch, as text).
fn ts_time(ts: &str) -> Option<DateTime<Utc>> {
    let seconds = ts.split('.').next()?.parse::<i64>().ok()?;
    DateTime::from_timestamp(seconds, 0)
}

/// The app's copy of an answered Slack question: in its thread's conversation, grouped
/// under its channel (or the DM), with the asker's name and whether it was the user.
pub fn slack_exchange(
    team_id: &str,
    installation: Option<&SlackInstallationPrefs>,
    question: &Question,
    asker: Option<&str>,
    turn: &ThreadTurn,
    now: DateTime<Utc>,
) -> Exchange {
    let owner = installation
        .and_then(|i| i.owner.as_ref())
        .is_some_and(|o| o.user_id == question.user);
    let asker = asker.map(str::trim).filter(|a| !a.is_empty());
    let label = match question.kind {
        QuestionKind::Mention => {
            let name = installation
                .and_then(|i| i.answer_channels.iter().find(|c| c.id == question.channel))
                .map(|c| format!("#{}", c.name))
                .unwrap_or_else(|| "a channel".to_string());
            format!("Slack · {name}")
        }
        QuestionKind::Direct if owner => "Slack · you".to_string(),
        QuestionKind::Direct => format!("Slack · {}", asker.unwrap_or("a direct message")),
    };
    Exchange {
        connector: Connector::Slack,
        chat: slack_chat(team_id, &question.turns_key()),
        label,
        chat_link: None,
        turn_id: question.ts.clone(),
        question: Said {
            author: if owner {
                "You".to_string()
            } else {
                asker.unwrap_or("Someone in Slack").to_string()
            },
            owner,
            text: turn.question.clone(),
            at: ts_time(&question.ts).unwrap_or(now),
            link: None,
        },
        answer: unescape_slack(&turn.answer),
        answered_at: now,
        learn: true,
    }
}

// ── Answers ─────────────────────────────────────────────────────────

/// Reads the installation's settings off the async workers.
pub(super) async fn installation_prefs(
    data_dir: &Path,
    team_id: &str,
) -> Result<Option<SlackInstallationPrefs>, String> {
    let store = SettingsStore::in_dir(data_dir);
    let settings = tokio::task::spawn_blocking(move || store.load())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(settings.connectors.slack.find(team_id).cloned())
}

/// Where one installation answers: read at every question (a change on the card applies
/// to the next one).
struct AppAccessRules {
    app: AppHandle,
    team_id: String,
    data_dir: PathBuf,
    live: Arc<Mutex<HashMap<String, Live>>>,
}

#[async_trait]
impl AccessRulesSource for AppAccessRules {
    async fn rules(&self) -> Result<AccessRules, String> {
        installation_prefs(&self.data_dir, &self.team_id)
            .await?
            .map(|i| access_rules(&i))
            .ok_or_else(|| "the Slack installation is gone".to_string())
    }

    fn channel_now_shared(&self, channel: &str) {
        tracing::warn!(target: "shodh::slack", "a channel chosen for answers is shared with another organization now; not answered there until chosen again");
        let inserted = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(self.team_id.clone())
            .or_default()
            .now_shared
            .insert(channel.to_string());
        if inserted {
            emit_status(&self.app);
        }
    }
}

/// Answers from the Shodh workspace selected for one installation, read at every
/// question (a change on the card applies to the next question).
struct AppAnswerSource {
    app: AppHandle,
    team_id: String,
    data_dir: PathBuf,
}

impl AppAnswerSource {
    async fn store(&self) -> Result<Arc<SlackStore>, String> {
        self.app.state::<SlackState>().store().await
    }
}

#[async_trait]
impl AnswerSource for AppAnswerSource {
    async fn scope(&self) -> Result<Option<AnswerScope>, String> {
        let Some(id) = installation_prefs(&self.data_dir, &self.team_id)
            .await?
            .and_then(|i| i.answer_workspace)
        else {
            return Ok(None);
        };
        let lookup = id.clone();
        let detail = match self
            .app
            .state::<WorkspaceState>()
            .run(move |s| s.detail(&lookup))
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
        Ok(Some(AnswerScope {
            source_ids: sources.source_ids,
            files: sources.files,
            slack_team: self.team_id.clone(),
            ..AnswerScope::default()
        }))
    }

    async fn retrieve(
        &self,
        scope: &AnswerScope,
        question: &str,
    ) -> Result<Vec<AnswerPassage>, String> {
        let rag = self.app.state::<RagState>().rag.clone();
        retrieve_passages(rag, scope, question).await
    }

    async fn listing(&self, scope: &AnswerScope) -> Result<LibraryListing, String> {
        let rag = self.app.state::<RagState>().rag.clone();
        library_listing(rag, scope).await
    }

    async fn thread_turns(&self, thread: &ThreadKey) -> Result<Vec<ThreadTurn>, String> {
        let store = self.store().await?;
        let (team, channel, key) = (
            self.team_id.clone(),
            thread.channel.clone(),
            thread.thread.clone(),
        );
        tokio::task::spawn_blocking(move || {
            store
                .thread_turns(&team, &channel, &key)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn record_turn(&self, thread: &ThreadKey, turn: ThreadTurn) -> Result<(), String> {
        let store = self.store().await?;
        let (team, channel, key) = (
            self.team_id.clone(),
            thread.channel.clone(),
            thread.thread.clone(),
        );
        tokio::task::spawn_blocking(move || {
            store
                .add_turn(&team, &channel, &key, &turn, Utc::now())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn keep_exchange(&self, question: &Question, asker: Option<&str>, turn: &ThreadTurn) {
        let installation = installation_prefs(&self.data_dir, &self.team_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(target: "shodh::slack", error = %e, "the Slack settings could not be read for the app's copy of an answer");
                None
            });
        let exchange = slack_exchange(
            &self.team_id,
            installation.as_ref(),
            question,
            asker,
            turn,
            Utc::now(),
        );
        conversations::keep(&self.app, exchange).await;
    }

    async fn channel_memories(&self, channel: &str) -> Result<Vec<SlackMemory>, String> {
        let store = self.store().await?;
        let (team, channel) = (self.team_id.clone(), channel.to_string());
        tokio::task::spawn_blocking(move || {
            store
                .channel_memories(&team, &channel)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    fn model(&self) -> Result<Arc<dyn TextModel>, String> {
        let llm = self.app.state::<LLMState>();
        crate::memory_learn::configured_model(&self.data_dir, &llm)
    }

    fn private_terms(&self) -> PrivateTerms {
        app_private_terms(&self.data_dir)
    }
}

/// What never leaves this computer in a connector's answer: the home folder, the user's
/// names (the OS account's and its display name) and the app data folder.
pub(super) fn app_private_terms(data_dir: &Path) -> PrivateTerms {
    let mut terms = PrivateTerms::of_this_computer();
    if let Some(name) = os_display_name() {
        terms.add_name(&name);
    }
    terms
        .home_dirs
        .push(data_dir.to_string_lossy().into_owned());
    terms
}

// ── Running ─────────────────────────────────────────────────────────

/// Stops the connection of `team_id` (if any): its answers are cancelled first, then the
/// connection closes.
async fn stop(app: &AppHandle, team_id: &str) {
    let running = app
        .state::<SlackState>()
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(team_id);
    if let Some(running) = running {
        running.answers.cancel();
        let _ = running.stop.send(true);
        if tokio::time::timeout(EXIT_WAIT, running.task).await.is_err() {
            tracing::warn!(target: "shodh::slack", "the Slack connection did not close in time");
        }
    }
    app.state::<SlackState>().set_link(team_id, LinkState::Off);
}

/// The router of `installation`, kept across restarts of its connection (a new one when
/// the bot changed), with the event ids seen before.
async fn routing(
    state: &SlackState,
    installation: &SlackInstallation,
) -> (Arc<EventRouter>, Arc<SeenPersister>) {
    let bot = installation.bot();
    if let Some(r) = state
        .routing
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&installation.team_id)
        .filter(|r| *r.router.bot() == bot)
    {
        return (r.router.clone(), r.seen.clone());
    }
    let seen = Arc::new(SeenPersister::new(Arc::new(FileSeenStore::new(
        &state.data_dir,
        &installation.team_id,
    ))));
    let loader = seen.clone();
    let entries = tokio::task::spawn_blocking(move || loader.load())
        .await
        .unwrap_or_default();
    let router = Arc::new(EventRouter::restored(bot, entries, Utc::now()));
    state
        .routing
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            installation.team_id.clone(),
            Routing {
                router: router.clone(),
                seen: seen.clone(),
            },
        );
    (router, seen)
}

/// Starts the connection of `prefs` (its tokens from the credential store).
async fn start(app: &AppHandle, prefs: &SlackInstallationPrefs) {
    let state = app.state::<SlackState>();
    let team_id = prefs.installation.team_id.clone();
    let lookup = team_id.clone();
    let credentials = tokio::task::spawn_blocking(move || KeyringCredentials.load(&lookup))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    let credentials = match credentials {
        Ok(Some(credentials)) => credentials,
        Ok(None) => {
            state.set_link(
                &team_id,
                LinkState::Error(
                    "The Slack tokens are no longer saved on this computer. Connect again.".into(),
                ),
            );
            return;
        }
        Err(e) => {
            state.set_link(&team_id, LinkState::Error(e));
            return;
        }
    };
    let web = match SlackWeb::new() {
        Ok(web) => web,
        Err(e) => {
            state.set_link(&team_id, LinkState::Error(e.to_string()));
            return;
        }
    };
    let runtime = match tokio::runtime::Handle::try_current() {
        Ok(runtime) => runtime,
        Err(e) => {
            state.set_link(&team_id, LinkState::Error(e.to_string()));
            return;
        }
    };
    let answers = CancellationToken::new();
    let poster = ObservedPoster {
        inner: BotPoster {
            web: web.clone(),
            token: credentials.bot_token.clone(),
        },
        app: app.clone(),
        team_id: team_id.clone(),
        live: state.live.clone(),
    };
    let memories = AppMemories {
        app: app.clone(),
        team_id: team_id.clone(),
        bot_user_id: prefs.installation.bot_user_id.clone(),
        data_dir: state.data_dir.clone(),
        web: web.clone(),
        token: credentials.bot_token.clone(),
    };
    let index_events =
        slack_index::start_sync(app, prefs, &web, &credentials.bot_token, answers.clone()).await;
    let gate = SlackAccessGate::new(
        Arc::new(AppAccessRules {
            app: app.clone(),
            team_id: team_id.clone(),
            data_dir: state.data_dir.clone(),
            live: state.live.clone(),
        }),
        Arc::new(BotDirectory {
            web: web.clone(),
            token: credentials.bot_token,
        }),
        Arc::new(SystemClock),
    );
    let responder = Responder::new(
        Arc::new(AppAnswerSource {
            app: app.clone(),
            team_id: team_id.clone(),
            data_dir: state.data_dir.clone(),
        }),
        Arc::new(memories),
        Arc::new(gate),
        state.usage.clone(),
        Arc::new(poster),
        Arc::new(SystemClock),
        answers.clone(),
    );
    let (router, seen) = routing(&state, &prefs.installation).await;
    let mut service = AnswerService::new(
        router,
        responder,
        runtime,
        Arc::new(AppOutcomes {
            app: app.clone(),
            team_id: team_id.clone(),
            team_name: prefs.installation.team_name.clone(),
            cap: state.cap.clone(),
            live: state.live.clone(),
        }),
    )
    .with_seen_store(seen);
    if let Some(events) = index_events {
        service = service.with_index_events(events);
    }
    let service = Arc::new(service);
    let opener = Arc::new(SlackSocketOpener {
        web,
        app_token: credentials.app_token,
    });
    let sink = Arc::new(AppStatusSink {
        app: app.clone(),
        team_id: team_id.clone(),
        live: state.live.clone(),
    });
    let (stop_tx, stop_rx) = watch::channel(false);
    let task = tauri::async_runtime::spawn(supervise(
        opener,
        service,
        sink,
        stop_rx,
        Timing::default(),
        prefs.installation.app_id.clone(),
    ));
    state
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            team_id,
            Running {
                stop: stop_tx,
                answers,
                task,
            },
        );
}

/// Brings the connections in line with the settings: on for installations that are
/// switched on (outside Local-only mode), off for the rest. A connection that stopped by
/// itself (a refused token) is not restarted here; switching the connector on again does.
async fn sync(app: &AppHandle) {
    let state = app.state::<SlackState>();
    let _op = state.op.lock().await;
    sync_locked(app).await;
}

async fn sync_locked(app: &AppHandle) {
    let state = app.state::<SlackState>();
    let settings = match state.settings().await {
        Ok(settings) => settings,
        Err(e) => {
            tracing::warn!(target: "shodh::slack", error = %e, "settings unreadable; the Slack connector stays as it is");
            return;
        }
    };
    let prefs = &settings.connectors.slack;
    state.cap.store(prefs.daily_cap(), Ordering::SeqCst);
    let running: Vec<String> = state
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect();
    for team_id in running {
        if !prefs
            .find(&team_id)
            .is_some_and(|i| should_run(&settings, i))
        {
            stop(app, &team_id).await;
        }
    }
    for installation in &prefs.installations {
        if should_run(&settings, installation)
            && !state.is_running(&installation.installation.team_id)
        {
            start(app, installation).await;
        }
    }
    emit_status(app);
}

/// At startup: connects the installations the user left on (nothing when the connector
/// is off).
pub fn start_saved(app: &AppHandle) {
    let Some(state) = app.try_state::<SlackState>() else {
        return;
    };
    let wanted = SettingsStore::in_dir(&state.data_dir)
        .load()
        .map(|s| {
            s.connectors
                .slack
                .installations
                .iter()
                .any(|i| should_run(&s, i))
        })
        .unwrap_or(false);
    if wanted {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { sync(&app).await });
    }
}

/// After any settings change (Local-only mode switched): follow it. Answers of an
/// installation that must stop are cancelled here, at once; the connection closes after.
pub fn settings_changed(app: &AppHandle, settings: &AppSettings) {
    let Some(state) = app.try_state::<SlackState>() else {
        return;
    };
    let running: Vec<String> = state
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect();
    for team_id in &running {
        if !settings
            .connectors
            .slack
            .find(team_id)
            .is_some_and(|i| should_run(settings, i))
        {
            state.cancel_answers(team_id);
        }
    }
    let any_wanted = settings
        .connectors
        .slack
        .installations
        .iter()
        .any(|i| should_run(settings, i));
    if !running.is_empty() || any_wanted {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { sync(&app).await });
    }
}

/// When the app exits: every answer is cancelled and every connection closed (a close
/// frame, bounded wait).
pub fn shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<SlackState>() else {
        return;
    };
    let running: Vec<Running> = state
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain()
        .map(|(_, r)| r)
        .collect();
    if running.is_empty() {
        return;
    }
    for r in &running {
        r.answers.cancel();
        let _ = r.stop.send(true);
    }
    tauri::async_runtime::block_on(async move {
        for r in running {
            let _ = tokio::time::timeout(EXIT_WAIT, r.task).await;
        }
    });
}

// ── Exposure ────────────────────────────────────────────────────────

/// The Slack workspaces (by name) whose questions are answered from Shodh workspace
/// `workspace_id`.
pub fn slack_teams_answering_from(settings: &AppSettings, workspace_id: &str) -> Vec<String> {
    settings
        .connectors
        .slack
        .installations
        .iter()
        .filter(|i| i.answer_workspace.as_deref() == Some(workspace_id))
        .map(|i| i.installation.team_name.clone())
        .collect()
}

/// What adding sources to a workspace Slack answers from means, for an approval prompt;
/// `None` when no Slack workspace answers from it.
pub fn slack_exposure_note(teams: &[String]) -> Option<String> {
    if teams.is_empty() {
        return None;
    }
    Some(format!(
        "Slack answers come from this workspace: people in {} will be able to get answers from what is added here.",
        teams.join(", ")
    ))
}

/// [`slack_exposure_note`] for `workspace_id`, with the settings of `data_dir` read off
/// the async workers (none when they cannot be read).
pub async fn slack_exposure_of(data_dir: &Path, workspace_id: &str) -> Option<String> {
    let store = SettingsStore::in_dir(data_dir);
    let settings = tokio::task::spawn_blocking(move || store.load())
        .await
        .ok()?
        .ok()?;
    slack_exposure_note(&slack_teams_answering_from(&settings, workspace_id))
}

// ── Commands ────────────────────────────────────────────────────────

pub(super) fn audit_change(audit: &AuditState, action: &str, detail: serde_json::Value) {
    let mut payload = json!({ "action": action, "via": "ui" });
    if let (Some(map), serde_json::Value::Object(extra)) = (payload.as_object_mut(), detail) {
        map.extend(extra);
    }
    audit.record(AuditRecord::new(AuditEventType::SettingsChange, payload));
}

pub(super) async fn save(
    state: &SlackState,
    change: impl FnOnce(&mut SlackPrefs) -> Result<(), String> + Send + 'static,
) -> Result<AppSettings, String> {
    let store = SettingsStore::in_dir(&state.data_dir);
    tokio::task::spawn_blocking(move || {
        store.update(|s| {
            change(&mut s.connectors.slack).map_err(|reason| {
                crate::app_settings::SettingsError::InvalidValue {
                    key: "connectors.slack".into(),
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

/// What a refused setup call tells the user.
pub(super) fn setup_error(which: &str, error: SlackApiError) -> String {
    match &error {
        SlackApiError::Api {
            code,
            needed: Some(scope),
        } if code == "missing_scope" => missing_scopes_message(std::slice::from_ref(scope)),
        e if e.is_auth() => format!(
            "Slack refused the {which} ({}). Check that the whole token was copied, and that the app is installed to the workspace.",
            e.code().unwrap_or("refused")
        ),
        SlackApiError::Network(_) => format!("{error}. Check the internet connection and try again."),
        _ => error.to_string(),
    }
}

/// The Slack card's status.
#[tauri::command]
pub async fn slack_status(app: AppHandle) -> Result<SlackStatus, String> {
    status(&app).await
}

/// Connects a Slack workspace with the tokens the user pasted: both are checked with
/// Slack (`auth.test`, its scopes, `bots.info`, `apps.connections.open`) before anything is
/// saved, then kept in the OS credential store and the connector is switched on. That both
/// belong to the same Slack app is checked on the connection's `hello`. User-only.
#[tauri::command]
pub async fn slack_connect(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    app_token: String,
    bot_token: String,
) -> Result<SlackStatus, String> {
    let _op = state.op.lock().await;
    let settings = state.settings().await?;
    if settings.policy.local_only {
        return Err(LOCAL_ONLY.into());
    }
    let app_token = SlackAppToken::parse(&app_token).map_err(|e| e.to_string())?;
    let bot_token = SlackBotToken::parse(&bot_token).map_err(|e| e.to_string())?;
    let web = SlackWeb::new().map_err(|e| e.to_string())?;
    let info = web
        .auth_test(&bot_token)
        .await
        .map_err(|e| setup_error("bot token", e))?;
    if let ScopeCheck::Missing(missing) = check_scopes(info.scopes.as_deref()) {
        return Err(missing_scopes_message(&missing));
    }
    let bot_app = match &info.bot_id {
        Some(bot_id) => Some(
            web.bot_app_id(&bot_token, bot_id)
                .await
                .map_err(|e| setup_error("bot token", e))?,
        ),
        None => None,
    };
    // Proves the app-level token works. Which app it belongs to is checked against the
    // bot's app on the connection's `hello`, the only place Slack names it: the `app_id`
    // in this URL is an opaque value, not the app id.
    web.open_connection(&app_token)
        .await
        .map_err(|e| setup_error("app-level token", e))?;
    if let Some(other) = settings
        .connectors
        .slack
        .installations
        .iter()
        .find(|i| i.installation.team_id != info.team_id)
    {
        return Err(format!(
            "Shodh is connected to the Slack workspace {}. Disconnect it first.",
            other.installation.team_name
        ));
    }
    let team_id = info.team_id.clone();
    // New tokens: answers of the old ones stop now, before anything else changes.
    state.cancel_answers(&team_id);
    let credentials = SlackCredentials {
        app_token,
        bot_token,
    };
    let store_team = team_id.clone();
    tokio::task::spawn_blocking(move || KeyringCredentials.store(&store_team, &credentials))
        .await
        .map_err(|e| e.to_string())??;
    let installation = SlackInstallation {
        team_id: team_id.clone(),
        team_name: info.team.clone(),
        bot_user_id: info.user_id.clone(),
        bot_name: info.user.clone(),
        bot_id: info.bot_id.clone(),
        scopes: info.scopes.clone().unwrap_or_default(),
        app_id: bot_app,
        connected_at: Utc::now(),
    };
    let settings = save(&state, move |prefs| {
        let kept = prefs.find(&installation.team_id).cloned();
        prefs
            .installations
            .retain(|i| i.installation.team_id != installation.team_id);
        prefs.installations.insert(
            0,
            SlackInstallationPrefs {
                installation,
                enabled: true,
                answer_workspace: kept.as_ref().and_then(|k| k.answer_workspace.clone()),
                owner: kept.as_ref().and_then(|k| k.owner.clone()),
                answer_channels: kept.map(|k| k.answer_channels).unwrap_or_default(),
            },
        );
        Ok(())
    })
    .await?;
    audit_change(
        &audit,
        "slack_connect",
        json!({ "slackWorkspace": info.team, "bot": info.user }),
    );
    // Channels that stopped on a refused token or a missing scope try again with these.
    retry_stopped_channels(&state, &team_id).await;
    // A running connection of this workspace starts again with the new tokens.
    stop(&app, &team_id).await;
    state
        .live
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&team_id);
    sync_locked(&app).await;
    broadcast(&app, &settings);
    status(&app).await
}

/// Switches the connector on or off (the tokens stay saved). User-only.
#[tauri::command]
pub async fn slack_set_enabled(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    enabled: bool,
) -> Result<SlackStatus, String> {
    let _op = state.op.lock().await;
    let settings = state.settings().await?;
    if enabled && settings.policy.local_only {
        return Err(LOCAL_ONLY.into());
    }
    let team_id = settings
        .connectors
        .slack
        .current()
        .map(|i| i.installation.team_id.clone())
        .ok_or_else(|| "Connect a Slack workspace first.".to_string())?;
    if !enabled {
        // Off means off now: queued and running answers stop before the switch is saved.
        state.cancel_answers(&team_id);
    }
    let team = team_id.clone();
    let settings = save(&state, move |prefs| {
        if let Some(i) = prefs.find_mut(&team) {
            i.enabled = enabled;
        }
        Ok(())
    })
    .await?;
    audit_change(&audit, "slack_set_enabled", json!({ "enabled": enabled }));
    if enabled {
        retry_stopped_channels(&state, &team_id).await;
    }
    // Switching on again retries a connection that stopped on an error.
    stop(&app, &team_id).await;
    state
        .live
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&team_id);
    sync_locked(&app).await;
    broadcast(&app, &settings);
    status(&app).await
}

/// Selects the Shodh workspace Slack answers come from (`None`: none, the bot then says
/// it isn't set up). User-only.
#[tauri::command]
pub async fn slack_set_answer_workspace(
    app: AppHandle,
    state: State<'_, SlackState>,
    workspaces: State<'_, WorkspaceState>,
    audit: State<'_, AuditState>,
    workspace_id: Option<String>,
) -> Result<SlackStatus, String> {
    let _op = state.op.lock().await;
    let workspace_id = workspace_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    let mut workspace_name = None;
    if let Some(id) = &workspace_id {
        let lookup = id.clone();
        let detail = workspaces
            .run(move |s| s.detail(&lookup))
            .await
            .map_err(|e| e.message)?;
        if detail.workspace.archived {
            return Err("That workspace is archived. Restore it, or choose another.".into());
        }
        workspace_name = Some(detail.workspace.name);
    }
    let settings = state.settings().await?;
    let team_id = settings
        .connectors
        .slack
        .current()
        .map(|i| i.installation.team_id.clone())
        .ok_or_else(|| "Connect a Slack workspace first.".to_string())?;
    let chosen = workspace_id.clone();
    let settings = save(&state, move |prefs| {
        if let Some(i) = prefs.find_mut(&team_id) {
            i.answer_workspace = chosen;
        }
        Ok(())
    })
    .await?;
    audit_change(
        &audit,
        "slack_set_answer_workspace",
        json!({ "workspaceId": workspace_id, "workspace": workspace_name }),
    );
    broadcast(&app, &settings);
    status(&app).await
}

/// Lets the indexed channels of `team_id` that stopped on an error try again (new
/// tokens, or the connector switched on again).
async fn retry_stopped_channels(state: &SlackState, team_id: &str) {
    let Ok(store) = state.store().await else {
        return;
    };
    let team = team_id.to_string();
    let retried = tokio::task::spawn_blocking(move || store.retry_stopped(&team))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()));
    if let Err(e) = retried {
        tracing::warn!(target: "shodh::slack", error = %e, "stopped Slack channels could not be retried");
    }
}

/// The bot token of the installation the card manages, for a call the user asked for.
pub(super) async fn current_bot_token(
    state: &SlackState,
) -> Result<(AppSettings, SlackInstallationPrefs, SlackBotToken), String> {
    let settings = state.settings().await?;
    if settings.policy.local_only {
        return Err(LOCAL_ONLY.into());
    }
    let current = settings
        .connectors
        .slack
        .current()
        .cloned()
        .ok_or_else(|| "Connect a Slack workspace first.".to_string())?;
    let lookup = current.installation.team_id.clone();
    let credentials = tokio::task::spawn_blocking(move || KeyringCredentials.load(&lookup))
        .await
        .map_err(|e| e.to_string())??
        .ok_or_else(|| {
            "The Slack tokens are no longer saved on this computer. Connect again.".to_string()
        })?;
    Ok((settings, current, credentials.bot_token))
}

/// The channels the bot is in, from Slack, with the ones Shodh answers in marked.
/// User-only (the card's channel list).
#[tauri::command]
pub async fn slack_list_channels(
    state: State<'_, SlackState>,
) -> Result<Vec<ChannelChoice>, String> {
    let (_, current, token) = current_bot_token(&state).await?;
    let web = SlackWeb::new().map_err(|e| e.to_string())?;
    let channels = web
        .bot_channels(&token)
        .await
        .map_err(|e| setup_error("bot token", e))?;
    let indexed: HashMap<String, String> = match state.store().await {
        Ok(store) => {
            let team = current.installation.team_id.clone();
            tokio::task::spawn_blocking(move || store.channels(&team))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|c| (c.channel_id, c.workspace_id))
                .collect()
        }
        Err(_) => HashMap::new(),
    };
    Ok(channels
        .into_iter()
        .map(|c| ChannelChoice {
            selected: current.answer_channels.iter().any(|a| a.id == c.id),
            indexed_into: indexed.get(&c.id).cloned(),
            id: c.id,
            name: c.name,
            private: c.private,
            shared: c.shared,
        })
        .collect())
}

/// Chooses the channels Shodh answers mentions in (empty: none). Each must be a channel
/// the bot is in now; whether it is shared with other organizations is read from Slack
/// at this moment. User-only.
#[tauri::command]
pub async fn slack_set_answer_channels(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    channel_ids: Vec<String>,
) -> Result<SlackStatus, String> {
    let _op = state.op.lock().await;
    let (_, current, token) = current_bot_token(&state).await?;
    let available = if channel_ids.iter().all(|id| id.trim().is_empty()) {
        Vec::new()
    } else {
        SlackWeb::new()
            .map_err(|e| e.to_string())?
            .bot_channels(&token)
            .await
            .map_err(|e| setup_error("bot token", e))?
    };
    let chosen = choose_channels(&channel_ids, &available)?;
    let team_id = current.installation.team_id.clone();
    let team = team_id.clone();
    let names: Vec<String> = chosen.iter().map(|c| c.name.clone()).collect();
    let shared: Vec<String> = chosen
        .iter()
        .filter(|c| c.shared)
        .map(|c| c.name.clone())
        .collect();
    let ids: BTreeSet<String> = chosen.iter().map(|c| c.id.clone()).collect();
    let settings = save(&state, move |prefs| {
        if let Some(i) = prefs.find_mut(&team) {
            i.answer_channels = chosen;
        }
        Ok(())
    })
    .await?;
    // A channel chosen again with its sharing read now is no longer flagged.
    if let Some(live) = state
        .live
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(&team_id)
    {
        live.now_shared.retain(|c| !ids.contains(c));
    }
    audit_change(
        &audit,
        "slack_set_answer_channels",
        json!({ "channels": names, "sharedWithOtherOrganizations": shared }),
    );
    broadcast(&app, &settings);
    status(&app).await
}

/// Sets the daily cap of Slack answers. User-only: the cap bounds what Slack may spend.
#[tauri::command]
pub async fn slack_set_daily_cap(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    daily_cap: usize,
) -> Result<SlackStatus, String> {
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
            "slack_set_daily_cap",
            json!({ "old": before, "new": daily_cap }),
        );
    }
    broadcast(&app, &settings);
    status(&app).await
}

/// Disconnects the Slack workspace: cancels its answers, stops the connection, switches
/// the connector off (saved first, so nothing reconnects it), with `forget` forgets what
/// was indexed from Slack and every memory saved from it, deletes both tokens from the
/// credential store, and only then forgets the installation. When the tokens cannot be
/// deleted, the connector stays off and the card says so. User-only.
#[tauri::command]
pub async fn slack_disconnect(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    forget: bool,
) -> Result<SlackStatus, String> {
    let _op = state.op.lock().await;
    let settings = state.settings().await?;
    let Some(current) = settings.connectors.slack.current().cloned() else {
        return status(&app).await;
    };
    let team_id = current.installation.team_id.clone();
    state.cancel_answers(&team_id);
    let off = team_id.clone();
    let settings = save(&state, move |prefs| {
        if let Some(i) = prefs.find_mut(&off) {
            i.enabled = false;
        }
        Ok(())
    })
    .await?;
    stop(&app, &team_id).await;
    broadcast(&app, &settings);
    let forgotten = if forget {
        Some(slack_index::forget_team(&app, &team_id).await.map_err(|e| {
            format!("The Slack connector is off, but what was indexed from Slack could not be forgotten ({e}). Try Disconnect again.")
        })?)
    } else {
        None
    };
    let remove_team = team_id.clone();
    let removed = tokio::task::spawn_blocking(move || KeyringCredentials.remove(&remove_team))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    if let Err(e) = removed {
        tracing::warn!(target: "shodh::slack", error = %e, "the Slack tokens could not be deleted");
        return Err(format!(
            "The Slack connector is off, but its tokens could not be deleted from this computer's credential store ({e}). Try Disconnect again."
        ));
    }
    let gone = team_id.clone();
    let settings = save(&state, move |prefs| {
        prefs
            .installations
            .retain(|i| i.installation.team_id != gone);
        Ok(())
    })
    .await?;
    state
        .live
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&team_id);
    audit_change(
        &audit,
        "slack_disconnect",
        json!({
            "slackWorkspace": current.installation.team_name,
            "forgot": forget,
            "threadsForgotten": forgotten.as_ref().map(|f| f.threads),
            "memoriesForgotten": forgotten.as_ref().map(|f| f.memories),
        }),
    );
    broadcast(&app, &settings);
    status(&app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installation(enabled: bool) -> SlackInstallationPrefs {
        SlackInstallationPrefs {
            installation: SlackInstallation {
                team_id: "T1".into(),
                team_name: "Acme".into(),
                bot_user_id: "UBOT".into(),
                bot_name: "shodh".into(),
                bot_id: Some("BBOT".into()),
                scopes: shodh_rag::connectors::slack::setup::REQUIRED_BOT_SCOPES
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                app_id: Some("A1".into()),
                connected_at: Utc::now(),
            },
            enabled,
            answer_workspace: None,
            answer_channels: Vec::new(),
            owner: None,
        }
    }

    #[test]
    fn the_default_cap_is_twenty_and_out_of_range_caps_fall_back() {
        let prefs = SlackPrefs::default();
        assert_eq!(prefs.daily_cap(), 20);
        assert!(check_daily_cap(0).is_err());
        assert!(check_daily_cap(MAX_DAILY_ANSWER_CAP + 1).is_err());
        assert!(check_daily_cap(1).is_ok());
        let odd = SlackPrefs {
            daily_answer_cap: 0,
            ..SlackPrefs::default()
        };
        assert_eq!(odd.daily_cap(), DEFAULT_DAILY_ANSWER_CAP);
        // Settings written before the connector existed load with it off.
        let old: AppSettings = serde_json::from_str(r#"{"seeded": true}"#).unwrap();
        assert_eq!(old.connectors.slack, SlackPrefs::default());
        // Settings written by part A load with the default history window.
        let part_a: SlackPrefs =
            serde_json::from_value(json!({"dailyAnswerCap": 20, "installations": []})).unwrap();
        assert_eq!(part_a.index_window(), 90);
        let odd_window = SlackPrefs {
            index_window_days: 3,
            ..SlackPrefs::default()
        };
        assert_eq!(odd_window.index_window(), 90, "out of range falls back");
    }

    #[test]
    fn an_installation_saved_before_channels_existed_answers_in_none() {
        let saved = json!({
            "installation": {
                "teamId": "T1", "teamName": "Acme", "botUserId": "UBOT", "botName": "shodh",
                "connectedAt": "2026-10-23T09:00:00Z"
            },
            "enabled": true,
            "answerWorkspace": "ws-1"
        });
        let prefs: SlackInstallationPrefs = serde_json::from_value(saved).unwrap();
        assert!(prefs.answer_channels.is_empty());
        assert_eq!(prefs.owner, None, "nobody is the owner until picked");
        assert_eq!(prefs.installation.app_id, None);
        let rules = access_rules(&prefs);
        assert_eq!(rules.team_id, "T1");
        assert!(rules.channels.is_empty(), "deny by default");
    }

    #[test]
    fn the_connection_runs_only_when_on_and_not_local_only() {
        let mut settings = AppSettings::default();
        assert!(should_run(&settings, &installation(true)));
        assert!(!should_run(&settings, &installation(false)));
        settings.policy.local_only = true;
        assert!(!should_run(&settings, &installation(true)));
    }

    #[test]
    fn the_card_state_follows_settings_and_the_link() {
        let on = installation(true);
        assert_eq!(state_word(false, None, None), "not_set_up");
        assert_eq!(state_word(true, Some(&on), None), "local_only");
        assert_eq!(state_word(false, Some(&installation(false)), None), "off");
        assert_eq!(state_word(false, Some(&on), None), "connecting");
        assert_eq!(
            state_word(false, Some(&on), Some(&LinkState::Connected)),
            "connected"
        );
        assert_eq!(
            state_word(
                false,
                Some(&on),
                Some(&LinkState::Reconnecting {
                    next_try_at: Utc::now(),
                    reason: "x".into()
                })
            ),
            "reconnecting"
        );
        assert_eq!(
            state_word(false, Some(&on), Some(&LinkState::Error("x".into()))),
            "error"
        );
    }

    fn bot_channel(id: &str, name: &str, shared: bool) -> BotChannel {
        BotChannel {
            id: id.into(),
            name: name.into(),
            private: false,
            shared,
        }
    }

    #[test]
    fn only_channels_the_bot_is_in_can_be_chosen_with_their_sharing() {
        let available = [
            bot_channel("C1", "research", false),
            bot_channel("C2", "client-a", true),
        ];
        let chosen = choose_channels(
            &["C2".into(), " C1 ".into(), "C2".into(), "".into()],
            &available,
        )
        .unwrap();
        assert_eq!(
            chosen,
            vec![
                AnswerChannel {
                    id: "C2".into(),
                    name: "client-a".into(),
                    shared: true
                },
                AnswerChannel {
                    id: "C1".into(),
                    name: "research".into(),
                    shared: false
                },
            ]
        );
        assert!(choose_channels(&["C9".into()], &available).is_err());
        assert!(choose_channels(&[], &available).unwrap().is_empty());
        let mut prefs = installation(true);
        prefs.answer_channels = chosen;
        let rules = access_rules(&prefs);
        assert_eq!(
            rules.channels,
            vec![
                AllowedChannel {
                    id: "C2".into(),
                    shared: true
                },
                AllowedChannel {
                    id: "C1".into(),
                    shared: false
                }
            ]
        );
    }

    #[test]
    fn scopes_missing_since_the_manifest_grew_are_listed() {
        let mut prefs = installation(true);
        assert!(missing_scopes(&prefs.installation).is_empty());
        prefs.installation.scopes = [
            "app_mentions:read",
            "chat:write",
            "im:history",
            "im:read",
            "im:write",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            missing_scopes(&prefs.installation),
            vec!["channels:read", "groups:read", "users:read"]
        );
        prefs.installation.scopes.clear();
        assert!(
            missing_scopes(&prefs.installation).is_empty(),
            "unknown is not missing"
        );
    }

    #[test]
    fn growing_the_workspace_slack_answers_from_is_flagged() {
        let mut settings = AppSettings::default();
        let mut prefs = installation(true);
        prefs.answer_workspace = Some("ws-client-a".into());
        settings.connectors.slack.installations.push(prefs);
        assert_eq!(
            slack_teams_answering_from(&settings, "ws-client-a"),
            vec!["Acme".to_string()]
        );
        assert!(slack_teams_answering_from(&settings, "ws-other").is_empty());
        let note = slack_exposure_note(&["Acme".into()]).unwrap();
        assert!(note.contains("people in Acme"), "{note}");
        assert_eq!(slack_exposure_note(&[]), None);
    }

    #[test]
    fn the_status_pacer_sends_once_then_once_more_for_what_came_meanwhile() {
        let pacer = StatusPacer::default();
        assert!(pacer.request(), "the first request starts a sender");
        assert!(pacer.take());
        assert!(!pacer.request(), "a sender is running");
        assert!(!pacer.request());
        assert!(pacer.finish_round(), "requests came during the wait");
        assert!(pacer.take());
        assert!(!pacer.take(), "taken once");
        assert!(!pacer.finish_round(), "nothing more: the sender stops");
        assert!(pacer.request(), "the next request starts a sender again");
    }

    #[test]
    fn the_display_name_is_read_from_passwd() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nqm:x:1000:1000:Quill Marrow,,,:/home/qm:/bin/bash\nnobody:x:65534:65534::/:/usr/sbin/nologin\n";
        assert_eq!(
            passwd_display_name(passwd, "qm").as_deref(),
            Some("Quill Marrow")
        );
        assert_eq!(passwd_display_name(passwd, "nobody"), None);
        assert_eq!(passwd_display_name(passwd, "absent"), None);
    }

    #[test]
    fn seen_events_survive_in_their_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSeenStore::new(dir.path(), "T1/../x");
        assert!(
            store.path.ends_with("slack_seen_T1x.json"),
            "{:?}",
            store.path
        );
        assert!(store.load().unwrap().is_empty());
        let entries = vec![SeenEntry {
            id: "Ev1".into(),
            at: Utc::now(),
        }];
        store.save(&entries).unwrap();
        assert_eq!(
            FileSeenStore::new(dir.path(), "T1/../x").load().unwrap(),
            entries
        );
        std::fs::write(&store.path, "{damaged").unwrap();
        assert!(store.load().unwrap().is_empty(), "damaged starts empty");
    }

    #[test]
    fn the_status_never_carries_a_token() {
        // The status type has no field a token could be in; its JSON is checked here so a
        // field added later that holds one fails this test.
        let status = SlackStatus {
            team_id: Some("T1".into()),
            state: "connected",
            installation: Some(InstallationView {
                team_name: "Acme".into(),
                bot_name: "shodh".into(),
            }),
            error: None,
            next_retry_at: None,
            retry_reason: None,
            last_event_at: None,
            answers_today: 1,
            daily_cap: 20,
            min_daily_cap: 1,
            max_daily_cap: 1_000,
            resets_at: "2026-10-24T00:00:00Z".into(),
            answer_workspace: None,
            answer_channels: vec![ChannelView {
                id: "C1".into(),
                name: "research".into(),
                shared: false,
                now_shared: false,
            }],
            other_connections: false,
            last_post_failed_at: None,
            missing_scopes: Vec::new(),
            index: IndexStatus::default(),
            create_app_url: create_app_url(),
            bot_scopes: manifest_scopes(),
            bot_events: manifest_events(),
            app_token_scope: APP_TOKEN_SCOPE,
        };
        let text = serde_json::to_string(&status).unwrap();
        assert!(!text.contains("xoxb-") && !text.contains("xapp-"));
        assert!(text.contains("\"teamName\":\"Acme\""));
        assert!(text.contains("\"answerChannels\":[{\"id\":\"C1\""));
        // The team id is the app's, never the card's.
        assert!(!text.contains("teamId") && !text.contains("\"T1\""));
        // The settings hold no token either.
        let mut settings = AppSettings::default();
        settings
            .connectors
            .slack
            .installations
            .push(installation(true));
        let text = serde_json::to_string(&settings).unwrap();
        assert!(!text.contains("xoxb-") && !text.contains("xapp-"));
    }

    fn question(kind: QuestionKind, user: &str, thread: Option<&str>) -> Question {
        Question {
            event_id: "Ev1".into(),
            kind,
            channel: "C1".into(),
            reply_thread_ts: thread.map(str::to_string),
            thread: ThreadKey {
                channel: "C1".into(),
                thread: thread.unwrap_or("").into(),
            },
            user: user.into(),
            ts: "1761555600.000200".into(),
            in_thread: thread.is_some(),
            asker_team: Some("T1".into()),
            ext_shared: false,
            has_files: false,
            text: "What is the notice period?".into(),
        }
    }

    #[test]
    fn an_answered_question_becomes_the_apps_conversation_of_its_thread() {
        let mut prefs = installation(true);
        prefs.answer_channels = vec![AnswerChannel {
            id: "C1".into(),
            name: "deals".into(),
            shared: false,
        }];
        prefs.owner = Some(SlackOwner {
            user_id: "UME".into(),
            name: "Me".into(),
        });
        let turn = ThreadTurn {
            question: "What is the notice period?".into(),
            answer: "Sixty days &amp; no more &lt;b&gt; [1]".into(),
        };
        let now = Utc::now();
        let mention = question(QuestionKind::Mention, "U2", Some("1761555500.000100"));
        let ex = slack_exchange("T1", Some(&prefs), &mention, Some("Alex"), &turn, now);
        assert_eq!(ex.chat, "T1-C1-1761555500_000100");
        assert!(ex.chat.starts_with(&slack_channel_prefix("T1", "C1")));
        assert!(ex.chat.starts_with(&slack_team_prefix("T1")));
        assert_eq!(ex.label, "Slack · #deals");
        assert_eq!(ex.question.author, "Alex");
        assert!(!ex.question.owner);
        assert_eq!(
            conversations::words_to_learn(&ex),
            None,
            "someone else's question never reaches learning"
        );
        assert_eq!(ex.answer, "Sixty days & no more <b> [1]");
        assert_eq!(ex.question.at.timestamp(), 1_761_555_600);

        // The user's own question (the member picked as "me") is learned from like Ask.
        let mine = question(QuestionKind::Direct, "UME", None);
        let ex = slack_exchange("T1", Some(&prefs), &mine, Some("Me"), &turn, now);
        assert!(ex.question.owner);
        assert_eq!(ex.label, "Slack · you");
        assert_eq!(ex.question.author, "You");
        assert_eq!(
            ex.chat, "T1-C1-1761555600_000200",
            "a DM question outside a thread is its own conversation"
        );
        assert_eq!(
            conversations::words_to_learn(&ex),
            Some("What is the notice period?")
        );

        // Nobody picked: nobody is the owner.
        let ex = slack_exchange("T1", None, &mine, None, &turn, now);
        assert!(!ex.question.owner);
        assert_eq!(ex.label, "Slack · a direct message");
        assert_eq!(ex.question.author, "Someone in Slack");
    }

    #[test]
    fn token_entries_are_not_provider_keys() {
        for kind in [APP_TOKEN_USER, BOT_TOKEN_USER] {
            assert!(!crate::api_key_store::is_known_provider(&token_user(
                kind, "T1"
            )));
        }
        assert_eq!(token_user(BOT_TOKEN_USER, "T1"), "slack-bot-token:T1");
    }
}
