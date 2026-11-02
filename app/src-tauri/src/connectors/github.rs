//! The GitHub connector in the app: sign-in, the user's list of repositories and what
//! Shodh may do on each, the page's commands, and the bridge the agent's GitHub tools
//! use (see `shodh_rag::connectors::github` for the connector itself).
//!
//! - **Sign-in**: GitHub's device flow for the Shodh GitHub App. The page shows a short
//!   code and opens github.com/login/device; Shodh polls until the user enters it (or
//!   cancels, or it expires). No client secret exists. Local-only mode refuses it.
//! - **Token** in the OS credential store (`github-user-token:<user id>`); never logged,
//!   never sent to the frontend, never written to a project. Sign out deletes it from
//!   this computer; GitHub's revoke endpoint needs the app's client secret, so the page
//!   links to GitHub's Authorized apps page to revoke it there too.
//! - **Settings** (`connectors.github` in `app_settings.json`, user-only): the account
//!   and the list (per repository: read; write; merge, off by default).
//! - **Tools**: [`AppGitHub`] gives the agent's tools the session afresh for each call
//!   (Local-only mode and sign-out apply at once), adds a repository after the user
//!   approved the agent's request, and tells the user about refused sign-ins, rejected
//!   pushes and rate limits ([`super::github_notices`]).
//! - **Activity**: the GitHub entries of the audit log.
//!
//! **Known limit (Windows):** commands Shodh runs are not sandboxed on Windows yet, so a
//! command the user approves runs as the user and can read this sign-in from the
//! credential store and change `app_settings.json`. The limits here bind Shodh's own
//! tools, not such a command; the page says so (`commands_can_read_sign_in`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shodh_rag::audit::{AuditEventType, AuditQuery};
use shodh_rag::connectors::github::api::{ApiError, GitHubApi};
use shodh_rag::connectors::github::device::{
    DeviceCode, DeviceError, DeviceFlow, Poll, APP_INSTALL_URL, AUTHORIZED_APPS_URL, MIN_INTERVAL,
};
use shodh_rag::connectors::github::git::HostGit;
use shodh_rag::connectors::github::limits::{grant_of, RepoGrant};
use shodh_rag::connectors::github::remote::RepoRef;
use shodh_rag::connectors::github::token::SignIn;
use shodh_rag::connectors::github::tools::{ActivityItem, GitHubHost, GitHubNotice, Session};
use shodh_rag::connectors::telegram::CancellationToken;
use tauri::{AppHandle, Emitter, Manager, State};

use super::github_notices::{self, GitHubNoticeState};
use super::slack::audit_change;
use crate::app_settings::{broadcast, AppSettings, SettingsStore};
use crate::audit_commands::AuditState;

/// Emitted with the [`GitHubStatus`] whenever it changes.
pub const GITHUB_STATUS_EVENT: &str = "connectors-github-status";

/// Credential-store user of the sign-in (never a provider id).
const TOKEN_USER: &str = "github-user-token";
/// How long before its expiry an expiring token is renewed.
const REFRESH_MARGIN: chrono::Duration = chrono::Duration::minutes(5);
/// The longest wait between two polls, whatever GitHub asks.
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(60);

const LOCAL_ONLY: &str = "Local-only mode is on, so Shodh does not connect to GitHub. Turn it off in Settings → Privacy to use the GitHub connector.";
const NOT_SIGNED_IN: &str =
    "Shodh is not signed in to GitHub. Ask the user to sign in under Connectors → GitHub.";
const SIGN_IN_AGAIN: &str = "GitHub no longer accepts Shodh's sign-in (it was revoked or expired). Ask the user to sign in again under Connectors → GitHub.";

/// What the permissions page says about the app, in plain words.
pub const GITHUB_PERMISSIONS_NOTE: &str = "The Shodh GitHub app can read and write code, pull requests and issues of the repositories you install it on, and read their basic details. It cannot change settings, secrets, webhooks, Actions workflows or who has access. Shodh reaches only the repositories on your list below.";

// ── Settings ────────────────────────────────────────────────────────

/// The signed-in GitHub account (public facts only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAccount {
    pub id: u64,
    pub login: String,
    #[serde(default)]
    pub name: Option<String>,
    pub signed_in_at: DateTime<Utc>,
}

/// The GitHub connector's settings. User-only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GitHubPrefs {
    pub account: Option<GitHubAccount>,
    /// The repositories Shodh may reach, and what it may do on each.
    pub repos: Vec<RepoGrant>,
}

/// Whether `prefs` may hold `grant` (merge needs write).
pub fn check_grant(grant: &RepoGrant) -> Result<RepoRef, String> {
    let repo = RepoRef::new(&grant.owner, &grant.name).map_err(|e| e.to_string())?;
    if grant.merge && !grant.write {
        return Err("Merging needs Write on the repository too.".into());
    }
    Ok(repo)
}

/// Puts `grant` on the list (replacing the entry of the same repository).
pub fn upsert_grant(prefs: &mut GitHubPrefs, grant: RepoGrant) {
    match prefs.repos.iter_mut().find(|g| {
        g.owner.eq_ignore_ascii_case(&grant.owner) && g.name.eq_ignore_ascii_case(&grant.name)
    }) {
        Some(existing) => *existing = grant,
        None => {
            prefs.repos.push(grant);
            prefs
                .repos
                .sort_by_key(|g| format!("{}/{}", g.owner, g.name).to_ascii_lowercase());
        }
    }
}

/// What the agent's approved request does: adds `repo` (read), or adds write to it.
/// Never removes write and never turns merge on or off.
pub fn widen_grant(prefs: &mut GitHubPrefs, repo: &RepoRef, write: bool) {
    let current = grant_of(&prefs.repos, repo).cloned();
    let grant = match current {
        Some(g) => RepoGrant {
            write: g.write || write,
            ..g
        },
        None => RepoGrant {
            owner: repo.owner.clone(),
            name: repo.name.clone(),
            write,
            merge: false,
        },
    };
    upsert_grant(prefs, grant);
}

fn token_user(id: u64) -> String {
    format!("{TOKEN_USER}:{id}")
}

fn load_sign_in(id: u64) -> Result<Option<SignIn>, String> {
    match crate::api_key_store::load_secret(&token_user(id))? {
        Some(text) => SignIn::from_stored(&text)
            .map(Some)
            .map_err(|e| format!("The saved GitHub sign-in is not usable: {e}")),
        None => Ok(None),
    }
}

fn store_sign_in(id: u64, sign_in: &SignIn) -> Result<(), String> {
    crate::api_key_store::store_secret(&token_user(id), &sign_in.to_stored())
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ── Running state ───────────────────────────────────────────────────

/// A sign-in waiting for the user to enter the code.
struct Pending {
    user_code: String,
    verification_uri: String,
    expires_at: DateTime<Utc>,
    stop: CancellationToken,
}

#[derive(Debug, Clone, Default)]
struct Live {
    /// GitHub refused the saved sign-in: tools refuse until the user signs in again.
    refused: bool,
    /// Why the last sign-in did not finish.
    sign_in_error: Option<String>,
    rate_limited_until: Option<DateTime<Utc>>,
}

/// Managed state of the GitHub connector.
pub struct GitHubState {
    data_dir: PathBuf,
    pending: Mutex<Option<Pending>>,
    live: Mutex<Live>,
    /// One change at a time (sign in, sign out, the list).
    op: tokio::sync::Mutex<()>,
    pub(super) notices: GitHubNoticeState,
}

impl GitHubState {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            pending: Mutex::new(None),
            live: Mutex::new(Live::default()),
            op: tokio::sync::Mutex::new(()),
            notices: GitHubNoticeState::default(),
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

    fn change_live(&self, f: impl FnOnce(&mut Live)) {
        f(&mut self.live.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Stops a sign-in in progress.
    fn cancel_sign_in(&self) {
        if let Some(pending) = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            pending.stop.cancel();
        }
    }
}

/// Saves a change to the GitHub settings.
async fn save(
    state: &GitHubState,
    change: impl FnOnce(&mut GitHubPrefs) -> Result<(), String> + Send + 'static,
) -> Result<AppSettings, String> {
    let store = SettingsStore::in_dir(&state.data_dir);
    tokio::task::spawn_blocking(move || {
        store.update(|s| {
            change(&mut s.connectors.github).map_err(|reason| {
                crate::app_settings::SettingsError::InvalidValue {
                    key: "connectors.github".into(),
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

// ── Status ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub login: String,
    pub name: Option<String>,
    pub profile_url: String,
    pub signed_in_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInView {
    /// The code the user types on GitHub (shown big; not a secret on its own).
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoView {
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub url: String,
    pub write: bool,
    pub merge: bool,
}

/// What the page shows. Never a token.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubStatus {
    /// `not_signed_in`, `signing_in`, `signed_in`, `needs_sign_in`, `local_only`.
    pub state: &'static str,
    pub account: Option<AccountView>,
    pub sign_in: Option<SignInView>,
    pub sign_in_error: Option<String>,
    pub repos: Vec<RepoView>,
    pub rate_limited_until: Option<String>,
    pub install_url: &'static str,
    pub authorized_apps_url: &'static str,
    pub permissions_note: &'static str,
    /// Commands Shodh runs can read this sign-in (Windows: commands are not sandboxed yet).
    pub commands_can_read_sign_in: bool,
}

/// The page's state word.
pub fn state_word(
    local_only: bool,
    signed_in: bool,
    signing_in: bool,
    refused: bool,
) -> &'static str {
    if local_only {
        return "local_only";
    }
    if signing_in {
        return "signing_in";
    }
    match (signed_in, refused) {
        (false, _) => "not_signed_in",
        (true, true) => "needs_sign_in",
        (true, false) => "signed_in",
    }
}

fn repo_view(g: &RepoGrant) -> RepoView {
    RepoView {
        owner: g.owner.clone(),
        name: g.name.clone(),
        full_name: format!("{}/{}", g.owner, g.name),
        url: format!("https://github.com/{}/{}", g.owner, g.name),
        write: g.write,
        merge: g.merge && g.write,
    }
}

/// The status of `prefs` and the running state.
fn status_of(settings: &AppSettings, state: &GitHubState) -> GitHubStatus {
    let prefs = &settings.connectors.github;
    let live = state.live();
    let sign_in = state
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|p| SignInView {
            user_code: p.user_code.clone(),
            verification_uri: p.verification_uri.clone(),
            expires_at: rfc3339(p.expires_at),
        });
    GitHubStatus {
        state: state_word(
            settings.policy.local_only,
            prefs.account.is_some(),
            sign_in.is_some(),
            live.refused,
        ),
        account: prefs.account.as_ref().map(|a| AccountView {
            login: a.login.clone(),
            name: a.name.clone(),
            profile_url: format!("https://github.com/{}", a.login),
            signed_in_at: rfc3339(a.signed_in_at),
        }),
        sign_in,
        sign_in_error: live.sign_in_error.clone(),
        repos: prefs.repos.iter().map(repo_view).collect(),
        rate_limited_until: live
            .rate_limited_until
            .filter(|at| *at > Utc::now())
            .map(rfc3339),
        install_url: APP_INSTALL_URL,
        authorized_apps_url: AUTHORIZED_APPS_URL,
        permissions_note: GITHUB_PERMISSIONS_NOTE,
        commands_can_read_sign_in: cfg!(windows),
    }
}

pub async fn status(app: &AppHandle) -> Result<GitHubStatus, String> {
    let state = app.state::<GitHubState>();
    let settings = state.settings().await?;
    Ok(status_of(&settings, &state))
}

/// Tells the page the status changed.
pub fn emit_status(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match status(&app).await {
            Ok(status) => {
                if let Err(e) = app.emit(GITHUB_STATUS_EVENT, status) {
                    tracing::warn!(target: "shodh::github", error = %e, "GitHub status not emitted");
                }
            }
            Err(e) => {
                tracing::warn!(target: "shodh::github", error = %e, "GitHub status unavailable")
            }
        }
    });
}

/// Local-only mode switched on stops a sign-in in progress.
pub fn settings_changed(app: &AppHandle, settings: &AppSettings) {
    let Some(state) = app.try_state::<GitHubState>() else {
        return;
    };
    if settings.policy.local_only {
        state.cancel_sign_in();
    }
    emit_status(app);
}

/// When the app exits: a sign-in in progress stops polling.
pub fn shutdown(app: &AppHandle) {
    if let Some(state) = app.try_state::<GitHubState>() {
        state.cancel_sign_in();
    }
}

// ── Sign-in ─────────────────────────────────────────────────────────

/// What a refused sign-in tells the user.
fn device_error(error: &DeviceError) -> String {
    match error {
        DeviceError::Network(_) => format!("{error} Check the internet connection and try again."),
        other => other.to_string(),
    }
}

/// Polls until the user entered the code, then keeps the sign-in.
async fn poll_sign_in(app: AppHandle, flow: DeviceFlow, code: DeviceCode, stop: CancellationToken) {
    let mut interval = code.interval.clamp(MIN_INTERVAL, MAX_POLL_INTERVAL);
    let outcome = loop {
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(interval) => {}
        }
        if Utc::now() >= code.expires_at {
            break Err(DeviceError::Expired);
        }
        match flow.poll(&code, interval).await {
            Ok(Poll::Pending) => {}
            Ok(Poll::SlowDown(next)) => interval = next.clamp(MIN_INTERVAL, MAX_POLL_INTERVAL),
            Ok(Poll::Done(sign_in)) => break Ok(sign_in),
            // A dropped connection while waiting: the code is still good until it expires.
            Err(DeviceError::Network(e)) => {
                tracing::warn!(target: "shodh::github", error = %e, "a sign-in poll did not reach GitHub");
            }
            Err(e) => break Err(e),
        }
    };
    if stop.is_cancelled() {
        return;
    }
    let state = app.state::<GitHubState>();
    let _op = state.op.lock().await;
    if stop.is_cancelled() {
        return;
    }
    state
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let result = match outcome {
        Ok(sign_in) => finish_sign_in(&app, sign_in).await,
        Err(e) => Err(device_error(&e)),
    };
    match result {
        Ok(login) => {
            state.change_live(|l| {
                l.refused = false;
                l.sign_in_error = None;
            });
            github_notices::signed_in(&app);
            audit_change(
                &app.state::<AuditState>(),
                "github_sign_in",
                json!({ "login": login }),
            );
        }
        Err(e) => {
            tracing::warn!(target: "shodh::github", error = %e, "GitHub sign-in did not finish");
            state.change_live(|l| l.sign_in_error = Some(e));
        }
    }
    emit_status(&app);
}

/// Keeps a sign-in GitHub returned: who it is, the token in the credential store, the
/// account in the settings (another account starts with an empty list).
async fn finish_sign_in(app: &AppHandle, sign_in: SignIn) -> Result<String, String> {
    let api = GitHubApi::new().map_err(|e| e.to_string())?;
    let user = api.user(&sign_in.access).await.map_err(|e| e.to_string())?;
    let state = app.state::<GitHubState>();
    let previous = state.settings().await?.connectors.github.account;
    let id = user.id;
    let stored = sign_in.clone();
    tokio::task::spawn_blocking(move || store_sign_in(id, &stored))
        .await
        .map_err(|e| e.to_string())??;
    if let Some(old) = previous.filter(|a| a.id != id) {
        let old_id = old.id;
        if let Err(e) = tokio::task::spawn_blocking(move || {
            crate::api_key_store::remove_secret(&token_user(old_id))
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
        {
            tracing::warn!(target: "shodh::github", error = %e, "the earlier account's GitHub sign-in could not be deleted");
        }
    }
    let account = GitHubAccount {
        id,
        login: user.login.clone(),
        name: user.name.clone(),
        signed_in_at: Utc::now(),
    };
    let settings = save(&state, move |prefs| {
        if prefs.account.as_ref().is_some_and(|a| a.id != account.id) {
            prefs.repos.clear();
        }
        prefs.account = Some(account);
        Ok(())
    })
    .await?;
    broadcast(app, &settings);
    Ok(user.login)
}

#[tauri::command]
pub async fn github_status(app: AppHandle) -> Result<GitHubStatus, String> {
    status(&app).await
}

/// Starts signing in: asks GitHub for a code the page shows, then waits in the
/// background for the user to enter it. User-only.
#[tauri::command]
pub async fn github_sign_in_start(
    app: AppHandle,
    state: State<'_, GitHubState>,
) -> Result<GitHubStatus, String> {
    let _op = state.op.lock().await;
    let settings = state.settings().await?;
    if settings.policy.local_only {
        return Err(LOCAL_ONLY.into());
    }
    state.cancel_sign_in();
    let flow = DeviceFlow::new().map_err(|e| device_error(&e))?;
    let code = flow.start().await.map_err(|e| device_error(&e))?;
    let stop = CancellationToken::new();
    *state.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
        user_code: code.user_code.clone(),
        verification_uri: code.verification_uri.clone(),
        expires_at: code.expires_at,
        stop: stop.clone(),
    });
    state.change_live(|l| l.sign_in_error = None);
    tauri::async_runtime::spawn(poll_sign_in(app.clone(), flow, code, stop));
    Ok(status_of(&settings, &state))
}

/// Stops a sign-in in progress.
#[tauri::command]
pub async fn github_sign_in_cancel(
    app: AppHandle,
    state: State<'_, GitHubState>,
) -> Result<GitHubStatus, String> {
    state.cancel_sign_in();
    status(&app).await
}

/// Signs out: the sign-in is deleted from this computer's credential store and the
/// account forgotten; the list is kept for the next sign-in of the same account.
/// Revoking the app's access on GitHub is the user's step on GitHub (the page links to
/// it): GitHub's revoke endpoint needs the app's client secret. User-only.
#[tauri::command]
pub async fn github_sign_out(
    app: AppHandle,
    state: State<'_, GitHubState>,
    audit: State<'_, AuditState>,
) -> Result<GitHubStatus, String> {
    let _op = state.op.lock().await;
    state.cancel_sign_in();
    let settings = state.settings().await?;
    let Some(account) = settings.connectors.github.account.clone() else {
        return Ok(status_of(&settings, &state));
    };
    let id = account.id;
    tokio::task::spawn_blocking(move || crate::api_key_store::remove_secret(&token_user(id)))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
        .map_err(|e| {
            format!("The GitHub sign-in could not be deleted from this computer's credential store ({e}). Try again.")
        })?;
    let settings = save(&state, |prefs| {
        prefs.account = None;
        Ok(())
    })
    .await?;
    state.change_live(|l| *l = Live::default());
    github_notices::signed_out(&app);
    audit_change(&audit, "github_sign_out", json!({ "login": account.login }));
    broadcast(&app, &settings);
    Ok(status_of(&settings, &state))
}

/// A repository the app is installed on, for the picker.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableRepo {
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub private: bool,
    pub on_list: bool,
}

/// The session of the saved sign-in (renewed when it is about to expire).
async fn session_of(app: &AppHandle) -> Result<Session, String> {
    let state = app.state::<GitHubState>();
    let settings = state.settings().await?;
    if settings.policy.local_only {
        return Err(LOCAL_ONLY.into());
    }
    let Some(account) = settings.connectors.github.account.clone() else {
        return Err(NOT_SIGNED_IN.into());
    };
    if state.live().refused {
        return Err(SIGN_IN_AGAIN.into());
    }
    let id = account.id;
    let sign_in = tokio::task::spawn_blocking(move || load_sign_in(id))
        .await
        .map_err(|e| e.to_string())??
        .ok_or_else(|| NOT_SIGNED_IN.to_string())?;
    let sign_in = match sign_in.refresh.clone() {
        Some(refresh) if sign_in.needs_refresh(Utc::now(), REFRESH_MARGIN) => {
            let flow = DeviceFlow::new().map_err(|e| device_error(&e))?;
            match flow.refresh(&refresh).await {
                Ok(renewed) => {
                    let stored = renewed.clone();
                    tokio::task::spawn_blocking(move || store_sign_in(id, &stored))
                        .await
                        .map_err(|e| e.to_string())??;
                    renewed
                }
                Err(DeviceError::RefreshRefused) => {
                    refuse_sign_in(app);
                    return Err(SIGN_IN_AGAIN.into());
                }
                Err(e) => return Err(device_error(&e)),
            }
        }
        _ => sign_in,
    };
    Ok(Session {
        login: account.login,
        token: sign_in.access,
        grants: settings.connectors.github.repos.clone(),
        api: GitHubApi::new().map_err(|e| e.to_string())?,
        git: HostGit::new(&state.data_dir),
    })
}

/// GitHub refused the saved sign-in: tools stop, the user is told.
fn refuse_sign_in(app: &AppHandle) {
    let state = app.state::<GitHubState>();
    let first = !state.live().refused;
    state.change_live(|l| l.refused = true);
    if first {
        github_notices::tell(app, &GitHubNotice::SignInRefused);
    }
    emit_status(app);
}

/// The page's error for a GitHub call.
fn page_error(app: &AppHandle, error: ApiError) -> String {
    match &error {
        ApiError::Unauthorized => refuse_sign_in(app),
        ApiError::RateLimited { reset_at } => {
            let reset_at = *reset_at;
            app.state::<GitHubState>()
                .change_live(|l| l.rate_limited_until = reset_at);
            github_notices::tell(app, &GitHubNotice::RateLimited { reset_at });
        }
        _ => {}
    }
    error.to_string()
}

/// Every repository the app is installed on that the signed-in user can reach.
#[tauri::command]
pub async fn github_available_repos(app: AppHandle) -> Result<Vec<AvailableRepo>, String> {
    let session = session_of(&app).await?;
    let repos = session
        .api
        .installation_repos(&session.token)
        .await
        .map_err(|e| page_error(&app, e))?;
    Ok(repos
        .into_iter()
        .map(|r| AvailableRepo {
            on_list: grant_of(&session.grants, &r.repo).is_some(),
            owner: r.repo.owner.clone(),
            name: r.repo.name.clone(),
            full_name: r.repo.full_name(),
            private: r.private,
        })
        .collect())
}

/// Puts a repository on the list, or changes what Shodh may do on it. A repository new
/// to the list must be one the app is installed on. User-only.
#[tauri::command]
pub async fn github_set_repo(
    app: AppHandle,
    state: State<'_, GitHubState>,
    audit: State<'_, AuditState>,
    owner: String,
    name: String,
    write: bool,
    merge: bool,
) -> Result<GitHubStatus, String> {
    let grant = RepoGrant {
        owner: owner.trim().to_string(),
        name: name.trim().to_string(),
        write,
        merge,
    };
    let repo = check_grant(&grant)?;
    let _op = state.op.lock().await;
    let settings = state.settings().await?;
    let before = grant_of(&settings.connectors.github.repos, &repo).cloned();
    if before.is_none() {
        let session = session_of(&app).await?;
        // Only a repository of the user's own installations: a public repository answers
        // anyone, so its page loading proves nothing about the app.
        let installed = session
            .api
            .installation_repos(&session.token)
            .await
            .map_err(|e| page_error(&app, e))?;
        let info = installed
            .into_iter()
            .find(|r| r.repo.same(&repo))
            .ok_or_else(|| {
                format!(
                    "{} is not among the repositories you installed the Shodh GitHub app on. Install it there first ({APP_INSTALL_URL}).",
                    repo.full_name()
                )
            })?;
        // GitHub's own spelling of the name.
        let canonical = RepoGrant {
            owner: info.repo.owner,
            name: info.repo.name,
            ..grant.clone()
        };
        let settings = save(&state, move |prefs| {
            upsert_grant(prefs, canonical);
            Ok(())
        })
        .await?;
        audit_change(
            &audit,
            "github_add_repo",
            json!({ "repo": repo.full_name(), "write": write, "merge": merge }),
        );
        broadcast(&app, &settings);
        return Ok(status_of(&settings, &state));
    }
    let kept = before.clone();
    let settings = save(&state, move |prefs| {
        let current = kept.ok_or_else(|| "That repository is not on the list.".to_string())?;
        upsert_grant(
            prefs,
            RepoGrant {
                write,
                merge,
                ..current
            },
        );
        Ok(())
    })
    .await?;
    audit_change(
        &audit,
        "github_set_repo",
        json!({
            "repo": repo.full_name(),
            "write": write,
            "merge": merge,
            "before": before.map(|b| json!({ "write": b.write, "merge": b.merge })),
        }),
    );
    broadcast(&app, &settings);
    Ok(status_of(&settings, &state))
}

/// Takes a repository off the list: Shodh can no longer reach it. User-only.
#[tauri::command]
pub async fn github_remove_repo(
    app: AppHandle,
    state: State<'_, GitHubState>,
    audit: State<'_, AuditState>,
    owner: String,
    name: String,
) -> Result<GitHubStatus, String> {
    let repo = RepoRef::new(owner.trim(), name.trim()).map_err(|e| e.to_string())?;
    let _op = state.op.lock().await;
    let gone = repo.clone();
    let settings = save(&state, move |prefs| {
        prefs.repos.retain(|g| !g.is(&gone));
        Ok(())
    })
    .await?;
    audit_change(
        &audit,
        "github_remove_repo",
        json!({ "repo": repo.full_name() }),
    );
    broadcast(&app, &settings);
    Ok(status_of(&settings, &state))
}

// ── Activity ────────────────────────────────────────────────────────

/// What a settings change of the connector says, in words.
fn settings_words(action: &str, payload: &Value) -> Option<String> {
    let repo = payload
        .get("repo")
        .and_then(Value::as_str)
        .unwrap_or("a repository");
    let login = payload
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("GitHub");
    let level = |p: &Value| match (
        p.get("write").and_then(Value::as_bool).unwrap_or(false),
        p.get("merge").and_then(Value::as_bool).unwrap_or(false),
    ) {
        (true, true) => "read, write and merge",
        (true, false) => "read and write",
        _ => "read only",
    };
    Some(match action {
        "github_sign_in" => format!("Signed in as {login}"),
        "github_sign_out" => format!("Signed out of {login}"),
        "github_add_repo" => format!("Added {repo} ({})", level(payload)),
        "github_set_repo" => format!("Changed {repo} to {}", level(payload)),
        "github_remove_repo" => format!("Removed {repo}"),
        "github_grant" => format!("Approved Shodh's request for {repo} ({})", level(payload)),
        _ => return None,
    })
}

/// The GitHub entries of the audit log, newest first.
fn activity_of(rows: Vec<shodh_rag::audit::AuditRow>, limit: usize) -> Vec<ActivityItem> {
    rows.into_iter()
        .filter_map(|row| {
            let payload = &row.payload;
            if let Some(tool) = payload
                .get("tool")
                .and_then(Value::as_str)
                .filter(|t| t.starts_with("github_"))
            {
                if row.event_type != "tool_call" {
                    return None;
                }
                return Some(ActivityItem {
                    at: row.ts.clone(),
                    action: tool.to_string(),
                    summary: payload
                        .get("summary")
                        .and_then(Value::as_str)
                        .unwrap_or(tool)
                        .to_string(),
                    ok: payload.get("ok").and_then(Value::as_bool).unwrap_or(false),
                });
            }
            let action = payload.get("action").and_then(Value::as_str)?;
            Some(ActivityItem {
                at: row.ts.clone(),
                action: action.to_string(),
                summary: settings_words(action, payload)?,
                ok: true,
            })
        })
        .take(limit)
        .collect()
}

async fn activity(app: &AppHandle, limit: usize) -> Result<Vec<ActivityItem>, String> {
    let Some(log) = app.state::<AuditState>().log() else {
        return Ok(Vec::new());
    };
    tokio::task::spawn_blocking(move || {
        log.query(&AuditQuery {
            types: vec![AuditEventType::ToolCall, AuditEventType::SettingsChange],
            text: Some("github_".to_string()),
            limit: Some(500),
            ..AuditQuery::default()
        })
        .map(|rows| activity_of(rows, limit))
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The connector's latest actions.
#[tauri::command]
pub async fn github_activity(
    app: AppHandle,
    limit: Option<usize>,
) -> Result<Vec<ActivityItem>, String> {
    activity(&app, limit.unwrap_or(50).clamp(1, 200)).await
}

// ── The agent's bridge ──────────────────────────────────────────────

/// What the agent's GitHub tools use in the app.
pub struct AppGitHub {
    app: AppHandle,
}

impl AppGitHub {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl GitHubHost for AppGitHub {
    async fn session(&self) -> Result<Session, String> {
        session_of(&self.app).await
    }

    async fn grant(&self, repo: &RepoRef, write: bool) -> Result<(), String> {
        let state = self.app.state::<GitHubState>();
        let _op = state.op.lock().await;
        let wanted = repo.clone();
        let settings = save(&state, move |prefs| {
            widen_grant(prefs, &wanted, write);
            Ok(())
        })
        .await?;
        let mut payload = json!({ "action": "github_grant", "via": "approval", "repo": repo.full_name(), "write": write });
        if let Some(g) = grant_of(&settings.connectors.github.repos, repo) {
            payload["merge"] = json!(g.merge);
        }
        self.app
            .state::<AuditState>()
            .record(shodh_rag::audit::AuditRecord::new(
                AuditEventType::SettingsChange,
                payload,
            ));
        broadcast(&self.app, &settings);
        Ok(())
    }

    fn notify(&self, notice: GitHubNotice) {
        match &notice {
            GitHubNotice::SignInRefused => refuse_sign_in(&self.app),
            GitHubNotice::RateLimited { reset_at } => {
                let reset_at = *reset_at;
                self.app
                    .state::<GitHubState>()
                    .change_live(|l| l.rate_limited_until = reset_at);
                github_notices::tell(&self.app, &notice);
                emit_status(&self.app);
            }
            GitHubNotice::PushRejected { .. } => github_notices::tell(&self.app, &notice),
        }
    }

    async fn activity(&self, limit: usize) -> Vec<ActivityItem> {
        activity(&self.app, limit).await.unwrap_or_else(|e| {
            tracing::warn!(target: "shodh::github", error = %e, "GitHub activity unavailable");
            Vec::new()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(owner: &str, name: &str, write: bool, merge: bool) -> RepoGrant {
        RepoGrant {
            owner: owner.into(),
            name: name.into(),
            write,
            merge,
        }
    }

    #[test]
    fn the_state_word_follows_local_only_sign_in_and_refusal() {
        assert_eq!(state_word(true, true, false, false), "local_only");
        assert_eq!(state_word(false, false, true, false), "signing_in");
        assert_eq!(state_word(false, false, false, false), "not_signed_in");
        assert_eq!(state_word(false, true, false, true), "needs_sign_in");
        assert_eq!(state_word(false, true, false, false), "signed_in");
    }

    #[test]
    fn merge_needs_write() {
        assert!(check_grant(&grant("a", "b", false, true)).is_err());
        assert!(check_grant(&grant("a", "b", true, true)).is_ok());
        assert!(check_grant(&grant("a b", "c", false, false)).is_err());
    }

    #[test]
    fn an_approved_request_only_widens_and_never_touches_merge() {
        let mut prefs = GitHubPrefs::default();
        let repo = RepoRef::new("octo-org", "hello").unwrap();
        widen_grant(&mut prefs, &repo, false);
        assert_eq!(prefs.repos, vec![grant("octo-org", "hello", false, false)]);
        widen_grant(&mut prefs, &repo, true);
        assert_eq!(prefs.repos, vec![grant("octo-org", "hello", true, false)]);
        // A later read-only request keeps write.
        widen_grant(&mut prefs, &repo, false);
        assert!(prefs.repos[0].write);
        prefs.repos[0].merge = true;
        widen_grant(&mut prefs, &repo, true);
        assert!(prefs.repos[0].merge, "the user's merge choice is kept");
        let other = RepoRef::new("ann", "notes").unwrap();
        widen_grant(&mut prefs, &other, false);
        assert_eq!(prefs.repos.len(), 2);
        assert_eq!(prefs.repos[0].owner, "ann", "sorted");
    }

    #[test]
    fn the_settings_never_hold_a_token_and_old_files_still_read() {
        let prefs: GitHubPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs, GitHubPrefs::default());
        let prefs = GitHubPrefs {
            account: Some(GitHubAccount {
                id: 1,
                login: "ann".into(),
                name: None,
                signed_in_at: Utc::now(),
            }),
            repos: vec![grant("octo-org", "hello", true, false)],
        };
        let text = serde_json::to_string(&prefs).unwrap();
        assert!(
            !text.contains("ghu_") && !text.to_ascii_lowercase().contains("token"),
            "{text}"
        );
    }

    #[test]
    fn activity_is_read_from_the_audit_log() {
        let row = |event: &str, payload: Value| shodh_rag::audit::AuditRow {
            id: 1,
            ts: "2026-11-02T10:00:00Z".into(),
            principal: "local".into(),
            conversation_id: None,
            profile_id: None,
            run_id: None,
            event_type: event.into(),
            payload,
            prev_hash: String::new(),
            hash: String::new(),
        };
        let items = activity_of(
            vec![
                row(
                    "tool_call",
                    json!({"tool": "github_push", "ok": true, "summary": "Pushed to shodh/fix"}),
                ),
                row(
                    "approval",
                    json!({"tool": "github_push", "decision": "approved"}),
                ),
                row(
                    "tool_call",
                    json!({"tool": "web_search", "ok": true, "summary": "about github_"}),
                ),
                row(
                    "settings_change",
                    json!({"action": "github_add_repo", "repo": "octo-org/hello", "write": true}),
                ),
                row(
                    "settings_change",
                    json!({"action": "telegram_connect", "bot": "github_bot"}),
                ),
            ],
            10,
        );
        let summaries: Vec<&str> = items.iter().map(|i| i.summary.as_str()).collect();
        assert_eq!(
            summaries,
            vec![
                "Pushed to shodh/fix",
                "Added octo-org/hello (read and write)"
            ]
        );
    }

    #[test]
    fn the_status_never_carries_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let state = GitHubState::new(dir.path());
        let mut settings = AppSettings::default();
        settings.connectors.github.account = Some(GitHubAccount {
            id: 9,
            login: "ann".into(),
            name: Some("Ann".into()),
            signed_in_at: Utc::now(),
        });
        settings.connectors.github.repos = vec![grant("octo-org", "hello", false, true)];
        let status = status_of(&settings, &state);
        assert_eq!(status.state, "signed_in");
        assert!(!status.repos[0].merge, "merge without write is shown off");
        let text = serde_json::to_string(&status).unwrap();
        assert!(!text.contains("ghu_"), "{text}");
    }
}
