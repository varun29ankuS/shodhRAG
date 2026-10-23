//! The Slack connector in the app: its settings, tokens, connection and the commands of
//! its card in the Connectors view (see `shodh_rag::connectors::slack` for the connector
//! itself).
//!
//! - **Settings** (`connectors.slack` in `app_settings.json`, user-only): the installations
//!   (one per Slack workspace, by team id; the card shows one), whether each is on, the
//!   Shodh workspace it answers from, and the daily cap of answers.
//! - **Tokens** in the OS credential store (`slack-app-token:<team>`,
//!   `slack-bot-token:<team>`); never logged, never sent to the frontend, deleted on
//!   Disconnect.
//! - **Connection**: only while the connector is on and Local-only mode is off. Turning
//!   Local-only mode on stops it; turning it off again resumes it. Closed on exit.
//! - **Answers** come from the selected workspace only ([`AppAnswerSource`]); until one
//!   is selected, or after it was deleted or archived, the bot says it isn't set up.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::connectors::slack::answer::{retrieve_passages, AnswerPassage, AnswerScope};
use shodh_rag::connectors::slack::respond::{AnswerQuota, AnswerSource, Responder, SlackPoster};
use shodh_rag::connectors::slack::sanitize::PrivateTerms;
use shodh_rag::connectors::slack::service::AnswerService;
use shodh_rag::connectors::slack::setup::{
    check_scopes, create_app_url, missing_scopes_message, ScopeCheck, APP_TOKEN_SCOPE, BOT_EVENTS,
    REQUIRED_BOT_SCOPES,
};
use shodh_rag::connectors::slack::socket::{
    supervise, LinkState, SlackSocketOpener, StatusSink, Timing,
};
use shodh_rag::connectors::slack::tokens::{
    SlackAppToken, SlackBotToken, SlackCredentialStore, SlackCredentials,
};
use shodh_rag::connectors::slack::web::{BotPoster, SlackApiError, SlackWeb};
use shodh_rag::connectors::slack::SlackInstallation;
use shodh_rag::research::results::TextModel;
use shodh_rag::research::tables::calls::{next_utc_midnight, utc_day, SystemClock};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::watch;

use super::slack_usage::SlackUsage;
use crate::app_settings::{broadcast, AppSettings, SettingsStore};
use crate::audit_commands::AuditState;
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

const LOCAL_ONLY: &str =
    "Local-only mode is on, so Shodh does not connect to Slack. Turn it off in Settings → Privacy to use the Slack connector.";

// ── Settings ────────────────────────────────────────────────────────

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
}

/// The Slack connector's settings. User-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SlackPrefs {
    /// The most Slack answers (model calls) per UTC day.
    pub daily_answer_cap: usize,
    /// By team id. The card shows and manages the first.
    pub installations: Vec<SlackInstallationPrefs>,
}

impl Default for SlackPrefs {
    fn default() -> Self {
        Self {
            daily_answer_cap: DEFAULT_DAILY_ANSWER_CAP,
            installations: Vec::new(),
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

    fn find_mut(&mut self, team_id: &str) -> Option<&mut SlackInstallationPrefs> {
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

// ── State ───────────────────────────────────────────────────────────

/// What is known about a connection now (not stored).
#[derive(Debug, Clone, Default)]
struct Live {
    link: Option<LinkState>,
    last_event: Option<DateTime<Utc>>,
    /// Slack refused a reply (a missing scope, a revoked bot token): shown on the card.
    problem: Option<String>,
}

struct Running {
    stop: watch::Sender<bool>,
    task: tauri::async_runtime::JoinHandle<()>,
}

/// Managed state of the Slack connector.
pub struct SlackState {
    data_dir: PathBuf,
    cap: Arc<AtomicUsize>,
    usage: Arc<SlackUsage>,
    running: Mutex<HashMap<String, Running>>,
    live: Arc<Mutex<HashMap<String, Live>>>,
    /// One change at a time (connect, switch, disconnect, follow the settings).
    op: tokio::sync::Mutex<()>,
}

impl SlackState {
    /// The connector of the app data directory `data_dir`. Reads the cap from the
    /// settings; connects nothing.
    pub fn new(data_dir: &Path) -> Self {
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
            live: Arc::new(Mutex::new(HashMap::new())),
            op: tokio::sync::Mutex::new(()),
        }
    }

    fn settings(&self) -> Result<AppSettings, String> {
        SettingsStore::in_dir(&self.data_dir)
            .load()
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

/// What the card shows. Never a token.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackStatus {
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

async fn workspace_ref(app: &AppHandle, id: &str) -> WorkspaceRef {
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
    let settings = state.settings()?;
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
    let answers_today = state.usage.used(utc_day(now)).unwrap_or_else(|e| {
        tracing::warn!(target: "shodh::slack", error = %e, "the Slack answer count could not be read");
        0
    });
    Ok(SlackStatus {
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
        create_app_url: create_app_url(),
        bot_scopes: REQUIRED_BOT_SCOPES.to_vec(),
        bot_events: BOT_EVENTS.to_vec(),
        app_token_scope: APP_TOKEN_SCOPE,
    })
}

/// Tells the card the status changed (computed in the background).
pub fn emit_status(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match status(&app).await {
            Ok(status) => {
                if let Err(e) = app.emit(SLACK_STATUS_EVENT, status) {
                    tracing::warn!(target: "shodh::slack", error = %e, "Slack status not emitted");
                }
            }
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "Slack status unavailable")
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

impl StatusSink for AppStatusSink {
    fn link(&self, state: LinkState) {
        match &state {
            LinkState::Error(message) => {
                tracing::warn!(target: "shodh::slack", %message, "the Slack connector stopped")
            }
            LinkState::Connected => tracing::info!(target: "shodh::slack", "connected to Slack"),
            _ => {}
        }
        {
            let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
            let entry = live.entry(self.team_id.clone()).or_default();
            if state == LinkState::Connected {
                entry.problem = None;
            }
            entry.link = Some(state);
        }
        emit_status(&self.app);
    }

    fn event_seen(&self, at: DateTime<Utc>) {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(self.team_id.clone())
            .or_default()
            .last_event = Some(at);
        emit_status(&self.app);
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

impl ObservedPoster {
    fn observe<T>(&self, result: Result<T, SlackApiError>) -> Result<T, SlackApiError> {
        if let Err(e) = &result {
            let problem = match e {
                SlackApiError::Api {
                    code,
                    needed: Some(scope),
                } if code == "missing_scope" => Some(missing_scopes_message(std::slice::from_ref(scope))),
                e if e.is_auth() => Some(format!(
                    "Slack refused the bot token ({}). Reinstall the Slack app and connect again with the new bot token.",
                    e.code().unwrap_or("refused")
                )),
                _ => None,
            };
            if let Some(problem) = problem {
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

// ── Answers ─────────────────────────────────────────────────────────

/// Answers from the Shodh workspace selected for one installation, read at every
/// question (a change on the card applies to the next question).
struct AppAnswerSource {
    app: AppHandle,
    team_id: String,
    data_dir: PathBuf,
}

#[async_trait]
impl AnswerSource for AppAnswerSource {
    async fn scope(&self) -> Result<Option<AnswerScope>, String> {
        let store = SettingsStore::in_dir(&self.data_dir);
        let settings = tokio::task::spawn_blocking(move || store.load())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let Some(id) = settings
            .connectors
            .slack
            .find(&self.team_id)
            .and_then(|i| i.answer_workspace.clone())
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

    fn model(&self) -> Result<Arc<dyn TextModel>, String> {
        let llm = self.app.state::<LLMState>();
        crate::memory_learn::configured_model(&self.data_dir, &llm)
    }

    fn private_terms(&self) -> PrivateTerms {
        let mut terms = PrivateTerms::of_this_computer();
        terms
            .home_dirs
            .push(self.data_dir.to_string_lossy().into_owned());
        terms
    }
}

// ── Running ─────────────────────────────────────────────────────────

/// Stops the connection of `team_id` (if any) and waits for it to close.
async fn stop(app: &AppHandle, team_id: &str) {
    let running = app
        .state::<SlackState>()
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(team_id);
    if let Some(running) = running {
        let _ = running.stop.send(true);
        if tokio::time::timeout(EXIT_WAIT, running.task).await.is_err() {
            tracing::warn!(target: "shodh::slack", "the Slack connection did not close in time");
        }
    }
    app.state::<SlackState>().set_link(team_id, LinkState::Off);
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
    let poster = ObservedPoster {
        inner: BotPoster {
            web: web.clone(),
            token: credentials.bot_token,
        },
        app: app.clone(),
        team_id: team_id.clone(),
        live: state.live.clone(),
    };
    let responder = Responder {
        source: Arc::new(AppAnswerSource {
            app: app.clone(),
            team_id: team_id.clone(),
            data_dir: state.data_dir.clone(),
        }),
        quota: state.usage.clone(),
        poster: Arc::new(poster),
        clock: Arc::new(SystemClock),
    };
    let service = Arc::new(AnswerService::new(
        prefs.installation.bot(),
        responder,
        runtime,
    ));
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
    let task =
        tauri::async_runtime::spawn(supervise(opener, service, sink, stop_rx, Timing::default()));
    state
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            team_id,
            Running {
                stop: stop_tx,
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
    let settings = match state.settings() {
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
    let wanted = app
        .state::<SlackState>()
        .settings()
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

/// After any settings change (Local-only mode switched): follow it.
pub fn settings_changed(app: &AppHandle, settings: &AppSettings) {
    let Some(state) = app.try_state::<SlackState>() else {
        return;
    };
    let any_running = !state
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty();
    let any_wanted = settings
        .connectors
        .slack
        .installations
        .iter()
        .any(|i| should_run(settings, i));
    if any_running || any_wanted {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { sync(&app).await });
    }
}

/// When the app exits: every connection is closed (a close frame, bounded wait).
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
        let _ = r.stop.send(true);
    }
    tauri::async_runtime::block_on(async move {
        for r in running {
            let _ = tokio::time::timeout(EXIT_WAIT, r.task).await;
        }
    });
}

// ── Commands ────────────────────────────────────────────────────────

fn audit_change(audit: &AuditState, action: &str, detail: serde_json::Value) {
    let mut payload = json!({ "action": action, "via": "ui" });
    if let (Some(map), serde_json::Value::Object(extra)) = (payload.as_object_mut(), detail) {
        map.extend(extra);
    }
    audit.record(AuditRecord::new(AuditEventType::SettingsChange, payload));
}

async fn save(
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
fn setup_error(which: &str, error: SlackApiError) -> String {
    match &error {
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
/// Slack (`auth.test`, its scopes, `apps.connections.open`) before anything is saved, then
/// kept in the OS credential store and the connector is switched on. User-only.
#[tauri::command]
pub async fn slack_connect(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    app_token: String,
    bot_token: String,
) -> Result<SlackStatus, String> {
    let _op = state.op.lock().await;
    let settings = state.settings()?;
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
        connected_at: Utc::now(),
    };
    let settings = save(&state, move |prefs| {
        let answer_workspace = prefs
            .find(&installation.team_id)
            .and_then(|i| i.answer_workspace.clone());
        prefs
            .installations
            .retain(|i| i.installation.team_id != installation.team_id);
        prefs.installations.insert(
            0,
            SlackInstallationPrefs {
                installation,
                enabled: true,
                answer_workspace,
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
    // New tokens: a running connection of this workspace starts again with them.
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
    let settings = state.settings()?;
    if enabled && settings.policy.local_only {
        return Err(LOCAL_ONLY.into());
    }
    let team_id = settings
        .connectors
        .slack
        .current()
        .map(|i| i.installation.team_id.clone())
        .ok_or_else(|| "Connect a Slack workspace first.".to_string())?;
    let team = team_id.clone();
    let settings = save(&state, move |prefs| {
        if let Some(i) = prefs.find_mut(&team) {
            i.enabled = enabled;
        }
        Ok(())
    })
    .await?;
    audit_change(&audit, "slack_set_enabled", json!({ "enabled": enabled }));
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
    let settings = state.settings()?;
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

/// Disconnects the Slack workspace: stops the connection, deletes both tokens from the
/// credential store and forgets the installation. User-only.
#[tauri::command]
pub async fn slack_disconnect(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
) -> Result<SlackStatus, String> {
    let _op = state.op.lock().await;
    let settings = state.settings()?;
    let Some(current) = settings.connectors.slack.current().cloned() else {
        return status(&app).await;
    };
    let team_id = current.installation.team_id.clone();
    stop(&app, &team_id).await;
    let remove_team = team_id.clone();
    tokio::task::spawn_blocking(move || KeyringCredentials.remove(&remove_team))
        .await
        .map_err(|e| e.to_string())??;
    let forget = team_id.clone();
    let settings = save(&state, move |prefs| {
        prefs
            .installations
            .retain(|i| i.installation.team_id != forget);
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
        json!({ "slackWorkspace": current.installation.team_name }),
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
                scopes: REQUIRED_BOT_SCOPES.iter().map(|s| s.to_string()).collect(),
                connected_at: Utc::now(),
            },
            enabled,
            answer_workspace: None,
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

    #[test]
    fn the_status_never_carries_a_token() {
        // The status type has no field a token could be in; its JSON is checked here so a
        // field added later that holds one fails this test.
        let status = SlackStatus {
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
            create_app_url: create_app_url(),
            bot_scopes: REQUIRED_BOT_SCOPES.to_vec(),
            bot_events: BOT_EVENTS.to_vec(),
            app_token_scope: APP_TOKEN_SCOPE,
        };
        let text = serde_json::to_string(&status).unwrap();
        assert!(!text.contains("xoxb-") && !text.contains("xapp-"));
        assert!(text.contains("\"teamName\":\"Acme\""));
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
