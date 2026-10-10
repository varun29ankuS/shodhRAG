//! Settings → Model → Connect: the three ways to connect a model provider.
//!
//! - **Sign in with a subscription** (Claude Pro/Max, ChatGPT Plus/Pro,
//!   GitHub Copilot, Google Gemini): the agent runtime's own sign-in
//!   (`omp auth-broker login`, see `harness::omp_auth`), in Shodh's isolated
//!   accounts directory. The app opens the provider's page in the system
//!   browser and reports progress as `connect-sign-in` events; the status
//!   (connected, expired, signed out) comes from omp's `token` command.
//! - **Paste an API key**: the provider is detected from the key's prefix
//!   (or chosen when ambiguous), the key is checked with one cheap request to
//!   that provider and saved in the OS credential store.
//! - **Run locally**: LM Studio and Ollama are detected on this computer.
//!
//! No key, token or account identity is ever logged or audited.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;

use serde::Serialize;
use shodh_rag::audit::{payload as audit_payload, AuditEventType, AuditRecord};
use shodh_rag::harness::catalog_fetch::{fetch_lmstudio, fetch_ollama, verify_key, KeyCheckError};
use shodh_rag::harness::model_catalog::{
    detect_key_provider, is_plausible_key, KeyDetection, ProviderId,
};
use shodh_rag::harness::omp_auth::{self, LineSplitter, LoginParser, LoginStep, SignInState};
use shodh_rag::harness::sidecar::{resolve_binary_path, verify_binary, OmpLayout};
use shodh_rag::harness::HarnessError;
use shodh_rag::llm::{SubscriptionService, LM_STUDIO_DEFAULT_BASE_URL};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex as AsyncMutex};

use crate::api_key_store;
use crate::app_settings::SettingsStore;
use crate::audit_commands::AuditState;
use crate::llm_commands::LLMState;

/// Emitted while a sign-in runs ([`SignInEvent`]).
pub const SIGN_IN_EVENT: &str = "connect-sign-in";

/// One line, next to the Claude button.
pub const CLAUDE_TERMS_NOTE: &str = "Anthropic's terms limit using Pro/Max plans outside Anthropic's apps; your account may be restricted. An API key is the supported way.";

/// Why Ollama models are shown but cannot be chosen.
pub const OLLAMA_NOTE: &str =
    "Ollama models don't work through the assistant yet (a known issue in its runtime).";

/// Typed failure of a connect command, for the UI.
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct ConnectError {
    /// `ambiguous` (choose the provider), `rejected` (the key was refused),
    /// `unreachable`, `invalid`, `runtime_missing`, `busy` or `failed`.
    pub code: &'static str,
    pub message: String,
    /// For `ambiguous`: the providers the key may belong to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ProviderId>,
}

impl ConnectError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            candidates: Vec::new(),
        }
    }
}

impl From<HarnessError> for ConnectError {
    fn from(e: HarnessError) -> Self {
        match e {
            HarnessError::BinaryMissing { .. } => Self::new(
                "runtime_missing",
                "The assistant's runtime is not installed yet.",
            ),
            other => Self::new("failed", other.to_string()),
        }
    }
}

type ConnectResult<T> = Result<T, ConnectError>;

/// A subscription's last known status.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionStatus {
    pub provider: ProviderId,
    pub label: &'static str,
    pub state: SignInState,
    /// The signed-in accounts (email or account id), shown on this computer only.
    pub accounts: Vec<String>,
    /// A plain note shown next to the button (Claude's terms).
    pub note: Option<&'static str>,
}

/// A model server on this computer.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalServer {
    pub running: bool,
    pub models: Vec<String>,
}

/// Process-wide connect state: the last subscription statuses, the last
/// local-server detection and the running sign-in.
#[derive(Default)]
pub struct ConnectState {
    subscriptions: Mutex<HashMap<SubscriptionService, (SignInState, Vec<String>)>>,
    /// Subscriptions were checked at least once in this run.
    checked: Mutex<bool>,
    lmstudio: Mutex<LocalServer>,
    ollama: Mutex<LocalServer>,
    /// One status refresh at a time.
    refreshing: AsyncMutex<()>,
    /// The running sign-in: its number, provider and cancel switch.
    sign_in: AsyncMutex<Option<(u64, SubscriptionService, oneshot::Sender<()>)>>,
    /// Numbers sign-ins, so a finished one never clears a newer one.
    sign_in_seq: std::sync::atomic::AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ConnectState {
    /// Subscriptions that can answer now.
    pub fn signed_in(&self) -> Vec<ProviderId> {
        let subs = lock(&self.subscriptions);
        SubscriptionService::ALL
            .into_iter()
            .filter(|s| matches!(subs.get(s), Some((SignInState::Connected, _))))
            .map(ProviderId::from_subscription)
            .collect()
    }

    pub fn subscription_statuses(&self) -> Vec<SubscriptionStatus> {
        let subs = lock(&self.subscriptions);
        SubscriptionService::ALL
            .into_iter()
            .map(|service| {
                let (state, accounts) = subs
                    .get(&service)
                    .cloned()
                    .unwrap_or((SignInState::SignedOut, Vec::new()));
                SubscriptionStatus {
                    provider: ProviderId::from_subscription(service),
                    label: service.label(),
                    state,
                    accounts,
                    note: (service == SubscriptionService::Claude).then_some(CLAUDE_TERMS_NOTE),
                }
            })
            .collect()
    }

    pub fn lmstudio(&self) -> LocalServer {
        lock(&self.lmstudio).clone()
    }

    pub fn ollama(&self) -> LocalServer {
        lock(&self.ollama).clone()
    }

    fn set_status(&self, service: SubscriptionService, state: SignInState, accounts: Vec<String>) {
        lock(&self.subscriptions).insert(service, (state, accounts));
    }
}

fn data_dir(app: &AppHandle) -> ConnectResult<PathBuf> {
    crate::profile::app_data_dir(app).map_err(|e| {
        ConnectError::new("failed", format!("The app data folder is unavailable: {e}"))
    })
}

/// LM Studio's address: the Advanced override, else `LM_STUDIO_BASE_URL`,
/// else the default.
pub fn lmstudio_url(app: &AppHandle) -> String {
    let saved = data_dir(app)
        .ok()
        .and_then(|dir| SettingsStore::in_dir(&dir).load().ok())
        .and_then(|s| s.models.base_url(ProviderId::LmStudio).map(str::to_string));
    saved
        .or_else(|| {
            std::env::var(shodh_rag::harness::model::LM_STUDIO_URL_VAR)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| LM_STUDIO_DEFAULT_BASE_URL.to_string())
}

/// Detect LM Studio and Ollama on this computer (loopback only).
pub async fn refresh_local(app: &AppHandle, state: &ConnectState, ollama_host: &str) {
    let lm_url = lmstudio_url(app);
    let (lm, ol) = tokio::join!(fetch_lmstudio(&lm_url), fetch_ollama(ollama_host));
    *lock(&state.lmstudio) = match lm {
        Ok(models) => LocalServer {
            running: true,
            models,
        },
        Err(_) => LocalServer::default(),
    };
    *lock(&state.ollama) = match ol {
        Ok(models) => LocalServer {
            running: true,
            models,
        },
        Err(_) => LocalServer::default(),
    };
}

/// Check every subscription with omp's `token` command. Without the
/// runtime installed, every subscription is signed out.
pub async fn refresh_subscriptions(app: &AppHandle, state: &ConnectState, force: bool) {
    let _one = state.refreshing.lock().await;
    if !force && *lock(&state.checked) {
        return;
    }
    let Ok(dir) = data_dir(app) else {
        return;
    };
    let binary = resolve_binary_path(&dir);
    if !binary.is_file() {
        *lock(&state.checked) = true;
        return;
    }
    if let Err(e) = verify_binary(&binary).await {
        tracing::warn!(target: "shodh::connect", "subscription status not checked: {e}");
        return;
    }
    let layout = OmpLayout::new(&dir);
    let checks = SubscriptionService::ALL.map(|service| {
        let binary = binary.clone();
        let layout = layout.clone();
        async move { (service, omp_auth::status(&binary, &layout, service).await) }
    });
    for (service, result) in futures::future::join_all(checks).await {
        match result {
            Ok((status, accounts)) => state.set_status(service, status, accounts),
            Err(e) => {
                tracing::warn!(target: "shodh::connect", provider = service.omp_provider(), "status check failed: {e}")
            }
        }
    }
    *lock(&state.checked) = true;
}

/// A running sign-in's progress.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInEvent {
    pub provider: ProviderId,
    /// `step`, `connected`, `failed` or `cancelled`.
    pub phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<LoginStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn emit(app: &AppHandle, event: SignInEvent) {
    if let Err(e) = app.emit(SIGN_IN_EVENT, event) {
        tracing::debug!(target: "shodh::connect", "emitting sign-in progress failed: {e}");
    }
}

fn subscription_of(provider: ProviderId) -> ConnectResult<SubscriptionService> {
    provider.subscription().ok_or_else(|| {
        ConnectError::new(
            "invalid",
            format!("{} is not a subscription.", provider.label()),
        )
    })
}

/// Start signing in to a subscription. Returns at once; progress arrives as
/// `connect-sign-in` events. One sign-in at a time.
#[tauri::command]
pub async fn connect_sign_in(
    app: AppHandle,
    provider: ProviderId,
    state: State<'_, ConnectState>,
) -> ConnectResult<()> {
    let service = subscription_of(provider)?;
    let mut running = state.sign_in.lock().await;
    if running.is_some() {
        return Err(ConnectError::new(
            "busy",
            "Another sign-in is in progress. Finish or cancel it first.",
        ));
    }
    let dir = data_dir(&app)?;
    let binary = resolve_binary_path(&dir);
    verify_binary(&binary).await?;
    let layout = OmpLayout::new(&dir);
    let mut command = omp_auth::command(&binary, &layout, service, &omp_auth::login_args(service))?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| ConnectError::new("failed", format!("The sign-in could not start: {e}")))?;
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let seq = state
        .sign_in_seq
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    *running = Some((seq, service, cancel_tx));
    drop(running);

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = drive_login(&handle, service, &mut child, cancel_rx).await;
        let _ = child.kill().await;
        let state = handle.state::<ConnectState>();
        {
            let mut slot = state.sign_in.lock().await;
            if slot.as_ref().is_some_and(|(n, _, _)| *n == seq) {
                *slot = None;
            }
        }
        let event = |phase, message: Option<String>| SignInEvent {
            provider,
            phase,
            step: None,
            message,
        };
        if matches!(outcome, LoginOutcome::Cancelled) {
            emit(&handle, event("cancelled", None));
            return;
        }
        // omp's own status decides: an account saved by a login that then
        // exited with an error still counts, and a failure is re-checked so
        // the row shows the truth.
        refresh_subscriptions(&handle, &state, true).await;
        if state.signed_in().contains(&provider) {
            record(&handle, provider, "sign_in");
            emit(&handle, event("connected", None));
            return;
        }
        let message = match outcome {
            LoginOutcome::Failed(message) => message,
            _ => "The account was saved but cannot be used yet. Try signing in again.".into(),
        };
        emit(&handle, event("failed", Some(message)));
    });
    Ok(())
}

/// Act on a sign-in step: open the provider's page (only an https page on
/// its own site).
fn take_step(
    app: &AppHandle,
    service: SubscriptionService,
    step: &LoginStep,
) -> Result<(), String> {
    if let LoginStep::OpenPage { url } = step {
        if !omp_auth::is_sign_in_page(service, url) {
            return Err(
                "The sign-in page was not on the provider's site, so it was not opened.".into(),
            );
        }
        use tauri_plugin_opener::OpenerExt;
        if let Err(e) = app.opener().open_url(url.as_str(), None::<&str>) {
            tracing::warn!(target: "shodh::connect", "browser not opened: {e}");
        }
    }
    Ok(())
}

enum LoginOutcome {
    Saved,
    Cancelled,
    Failed(String),
}

async fn drive_login(
    app: &AppHandle,
    service: SubscriptionService,
    child: &mut tokio::process::Child,
    mut cancel: oneshot::Receiver<()>,
) -> LoginOutcome {
    let provider = ProviderId::from_subscription(service);
    // stdin stays open until the login ends: omp's code prompt races the
    // browser callback, and a closed stdin would end the race as a failure.
    let mut stdin = child.stdin.take();
    let Some(mut stdout) = child.stdout.take() else {
        return LoginOutcome::Failed("The sign-in could not start.".into());
    };
    let stderr = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        let mut text = String::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if text.len() < 16 * 1024 {
                    text.push_str(&line);
                    text.push('\n');
                }
            }
        }
        text
    });
    let mut split = LineSplitter::default();
    let mut parser = LoginParser::default();
    let mut saved = false;
    let mut answered = false;
    let mut buf = vec![0u8; 8192];
    let deadline = tokio::time::sleep(omp_auth::LOGIN_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        let read = tokio::select! {
            _ = &mut cancel => return LoginOutcome::Cancelled,
            _ = &mut deadline => {
                return LoginOutcome::Failed("The sign-in timed out. Try again.".into());
            }
            read = stdout.read(&mut buf) => read,
        };
        let n = match read {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let lines = split.push(&String::from_utf8_lossy(&buf[..n]));
        for line in lines {
            if let Some(step) = parser.feed(&line) {
                if let Err(message) = take_step(app, service, &step) {
                    return LoginOutcome::Failed(message);
                }
                saved |= step == LoginStep::Saved;
                emit(
                    app,
                    SignInEvent {
                        provider,
                        phase: "step",
                        step: Some(step),
                        message: None,
                    },
                );
            }
        }
        if !answered {
            if let (Some(answer), Some(input)) = (
                omp_auth::prompt_answer(service, split.pending()),
                stdin.as_mut(),
            ) {
                answered = true;
                if input.write_all(answer.as_bytes()).await.is_err() || input.flush().await.is_err()
                {
                    return LoginOutcome::Failed("The sign-in could not continue.".into());
                }
            }
        }
    }
    if let Some(step) = split.finish().and_then(|rest| parser.feed(&rest)) {
        saved |= step == LoginStep::Saved;
    }
    drop(stdin);
    let status = tokio::select! {
        _ = &mut cancel => return LoginOutcome::Cancelled,
        status = child.wait() => status,
    };
    let stderr = stderr_task.await.unwrap_or_default();
    match status {
        Ok(_) if saved => LoginOutcome::Saved,
        Ok(status) if status.success() => {
            LoginOutcome::Failed("The sign-in ended without saving an account.".into())
        }
        Ok(_) => LoginOutcome::Failed(omp_auth::failure_message(&stderr)),
        Err(e) => LoginOutcome::Failed(format!("The sign-in did not finish: {e}")),
    }
}

/// Stop the running sign-in.
#[tauri::command]
pub async fn connect_sign_in_cancel(state: State<'_, ConnectState>) -> ConnectResult<()> {
    if let Some((_, _, cancel)) = state.sign_in.lock().await.take() {
        let _ = cancel.send(());
    }
    Ok(())
}

/// Sign out of a subscription (removes the account from Shodh's runtime only).
#[tauri::command]
pub async fn connect_sign_out(
    app: AppHandle,
    provider: ProviderId,
    state: State<'_, ConnectState>,
) -> ConnectResult<()> {
    let service = subscription_of(provider)?;
    let dir = data_dir(&app)?;
    let binary = resolve_binary_path(&dir);
    verify_binary(&binary).await?;
    omp_auth::sign_out(&binary, &OmpLayout::new(&dir), service).await?;
    state.set_status(service, SignInState::SignedOut, Vec::new());
    record(&app, provider, "sign_out");
    Ok(())
}

fn record(app: &AppHandle, provider: ProviderId, action: &str) {
    app.state::<AuditState>().record(AuditRecord::new(
        AuditEventType::SettingsChange,
        serde_json::json!({"action": "provider_connection", "provider": provider.as_str(), "change": action}),
    ));
}

/// A key provider's connection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyStatus {
    pub provider: ProviderId,
    pub label: &'static str,
    /// `vault` (saved in the OS credential store) or `environment` (an
    /// environment variable, which wins; it cannot be removed here).
    pub source: &'static str,
}

/// Providers with a key, and where each key comes from.
pub fn key_statuses(llm: &LLMState) -> Vec<KeyStatus> {
    let env = |name: &str| std::env::var(name).ok();
    let keys = lock(&llm.api_keys).clone();
    ProviderId::KEYED
        .into_iter()
        .filter_map(|provider| {
            let id = provider.as_str();
            let source = if api_key_store::env_key(id, &env).is_some() {
                "environment"
            } else if keys.get(id).is_some_and(|k| !k.trim().is_empty()) {
                "vault"
            } else {
                return None;
            };
            Some(KeyStatus {
                provider,
                label: provider.label(),
                source,
            })
        })
        .collect()
}

/// The result of saving a key.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedKey {
    pub provider: ProviderId,
    pub label: &'static str,
}

/// Which provider a key is for: the person's choice, else its prefix.
pub fn key_provider(key: &str, chosen: Option<ProviderId>) -> ConnectResult<ProviderId> {
    if !is_plausible_key(key) {
        return Err(ConnectError::new(
            "invalid",
            "That does not look like an API key. Paste the whole key, on one line.",
        ));
    }
    if let Some(provider) = chosen {
        if !provider.needs_key() {
            return Err(ConnectError::new(
                "invalid",
                format!("{} does not use an API key.", provider.label()),
            ));
        }
        return Ok(provider);
    }
    match detect_key_provider(key) {
        KeyDetection::Known { provider } => Ok(provider),
        KeyDetection::Ambiguous { candidates } => Err(ConnectError {
            code: "ambiguous",
            message: "Choose which provider this key is for.".into(),
            candidates,
        }),
    }
}

/// Check a pasted key with its provider and save it in the OS credential
/// store. `provider` is the person's choice when the prefix was ambiguous.
#[tauri::command]
pub async fn connect_save_key(
    key: String,
    provider: Option<ProviderId>,
    llm: State<'_, LLMState>,
    audit: State<'_, AuditState>,
) -> ConnectResult<SavedKey> {
    let key = key.trim().to_string();
    let provider = key_provider(&key, provider)?;
    verify_key(provider, &key).await.map_err(|e| match e {
        KeyCheckError::Rejected(_) => ConnectError::new("rejected", e.to_string()),
        KeyCheckError::Unreachable(..) => ConnectError::new("unreachable", e.to_string()),
        KeyCheckError::NoKey(_) => ConnectError::new("invalid", e.to_string()),
    })?;
    let id = provider.as_str();
    let to_store = key.clone();
    tokio::task::spawn_blocking(move || api_key_store::store(id, &to_store))
        .await
        .map_err(|e| ConnectError::new("failed", format!("Credential store task failed: {e}")))?
        .map_err(|e| ConnectError::new("failed", e))?;
    lock(&llm.api_keys).set(id, Some(key));
    audit.record(AuditRecord::new(
        AuditEventType::SettingsChange,
        audit_payload::api_key_change(id, audit_payload::KeyAction::Set),
    ));
    Ok(SavedKey {
        provider,
        label: provider.label(),
    })
}

/// Remove a saved key (an environment key stays in force).
#[tauri::command]
pub async fn connect_remove_key(
    provider: ProviderId,
    llm: State<'_, LLMState>,
    audit: State<'_, AuditState>,
) -> ConnectResult<()> {
    if !provider.needs_key() {
        return Err(ConnectError::new(
            "invalid",
            format!("{} does not use an API key.", provider.label()),
        ));
    }
    let id = provider.as_str();
    tokio::task::spawn_blocking(move || api_key_store::remove(id))
        .await
        .map_err(|e| ConnectError::new("failed", format!("Credential store task failed: {e}")))?
        .map_err(|e| ConnectError::new("failed", e))?;
    lock(&llm.api_keys).set(id, None);
    audit.record(AuditRecord::new(
        AuditEventType::SettingsChange,
        audit_payload::api_key_change(id, audit_payload::KeyAction::Deleted),
    ));
    Ok(())
}

/// At startup, once: copy provider keys from the environment into the OS
/// credential store, then record that this ran.
pub fn migrate_env_keys_once(app: &AppHandle) {
    let Ok(dir) = data_dir(app) else {
        return;
    };
    let store = SettingsStore::in_dir(&dir);
    match store.load() {
        Ok(settings) if settings.models.env_keys_migrated => return,
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(target: "shodh::connect", "key migration skipped: {e}");
            return;
        }
    }
    match api_key_store::migrate_env_keys(&api_key_store::OsVault, |name| std::env::var(name).ok())
    {
        Ok(copied) => {
            if !copied.is_empty() {
                tracing::info!(target: "shodh::connect", providers = ?copied, "environment keys saved in the OS credential store");
            }
            if let Err(e) = store.update(|s| {
                s.models.env_keys_migrated = true;
                Ok(())
            }) {
                tracing::warn!(target: "shodh::connect", "key migration not recorded: {e}");
            }
        }
        // Retried at the next start.
        Err(e) => tracing::warn!(target: "shodh::connect", "environment keys not saved: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_routed_by_prefix_or_by_choice() {
        assert_eq!(
            key_provider("sk-ant-api03-0123456789", None).unwrap(),
            ProviderId::Anthropic
        );
        let ambiguous = key_provider("sk-0123456789abcdef", None).unwrap_err();
        assert_eq!(ambiguous.code, "ambiguous");
        assert_eq!(ambiguous.candidates[0], ProviderId::OpenAI);
        let json = serde_json::to_value(&ambiguous).unwrap();
        assert_eq!(json["candidates"][0], "openai");
        assert!(!json["message"].as_str().unwrap().contains("sk-"));
        assert_eq!(
            key_provider("sk-0123456789abcdef", Some(ProviderId::OpenRouter)).unwrap(),
            ProviderId::OpenRouter
        );
        assert_eq!(
            key_provider("sk-0123456789abcdef", Some(ProviderId::ClaudeSub))
                .unwrap_err()
                .code,
            "invalid"
        );
        assert_eq!(key_provider("short", None).unwrap_err().code, "invalid");
    }

    #[test]
    fn statuses_default_to_signed_out_and_only_connected_ones_answer() {
        let state = ConnectState::default();
        assert!(state.signed_in().is_empty());
        let statuses = state.subscription_statuses();
        assert_eq!(statuses.len(), 4);
        assert!(statuses.iter().all(|s| s.state == SignInState::SignedOut));
        let claude = statuses
            .iter()
            .find(|s| s.provider == ProviderId::ClaudeSub)
            .unwrap();
        assert_eq!(claude.note, Some(CLAUDE_TERMS_NOTE));
        assert!(statuses
            .iter()
            .filter(|s| s.provider != ProviderId::ClaudeSub)
            .all(|s| s.note.is_none()));

        state.set_status(
            SubscriptionService::Copilot,
            SignInState::Connected,
            vec!["octo".into()],
        );
        state.set_status(
            SubscriptionService::Claude,
            SignInState::Expired,
            vec!["a@b".into()],
        );
        assert_eq!(state.signed_in(), vec![ProviderId::CopilotSub]);
    }

    #[test]
    fn errors_serialise_with_a_code() {
        let e: ConnectError = HarnessError::BinaryMissing {
            path: PathBuf::from("/x/omp"),
            version: "18.4.10",
        }
        .into();
        assert_eq!(e.code, "runtime_missing");
        let json = serde_json::to_value(&e).unwrap();
        assert!(json.get("candidates").is_none());
    }
}
