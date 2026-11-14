//! The phone link on the desktop: pairing a phone (Settings → Phone), the
//! paired phones, and the bridge that runs a phone's calls with the desktop's
//! own command handlers and rules ([`shodh_link`]).
//!
//! - **Keys.** The desktop's node key and the paired phones are kept in the
//!   OS credential store (`api_key_store`, the store the provider keys and
//!   the audit key use), never in a file.
//! - **When it runs.** Nothing listens until a phone is paired or a pairing
//!   is started, and nothing runs in Local-only mode: the link reaches n0's
//!   relays and address lookup (encrypted end to end; a relay sees only
//!   ciphertext), and Local-only mode means nothing leaves this computer.
//! - **What a phone may do.** Only the commands on
//!   [`shodh_link::allowlist::ALLOWED`]. On top of the bridge's checks, the
//!   desktop refuses here: Code mode and the modes that skip approvals,
//!   questions into a Code session, and any path outside the folders the user
//!   linked. An approval from a phone is granted only for Ask and Creator
//!   steps ([`approval_kind`]).
//! - **Activity.** Every call, pairing and removal is recorded
//!   (`phone_link` events) with the phone's name.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shodh_link::allowlist::{path_shape, ApprovalKind, PathRefusal};
use shodh_link::bridge::{CallRecord, Caller, Link, LinkEvent, LinkHost, Outcome, PendingStep};
use shodh_link::iroh::{Endpoint, SecretKey};
use shodh_link::pairing::{
    new_secret, DeviceRegistry, DeviceStore, PairedDevice, PairingOffer, PendingPairing,
};
use shodh_rag::audit::{AuditEventType, AuditRecord, LOCAL_OWNER};
use shodh_rag::harness::code_mode::{CHANGE_TOOLS, OMP_EXECUTION_TOOLS};
use shodh_rag::harness::modes::AgentMode;
use shodh_rag::harness::run_command::RUN_COMMAND;
use shodh_rag::inbox::InboxItem;
use tauri::{AppHandle, Emitter, Listener, Manager, State};
use tokio::sync::{broadcast, Mutex as AsyncMutex, OnceCell};

use crate::agent_session_commands::{
    self, AgentSessions, AttachedContext, HistoryTurn, RemoteFacts, SendScope, AGENT_EVENT,
};
use crate::agent_tools::files::{containing_root, IndexedRoots, SourceRoots};
use crate::api_key_store;
use crate::audit_commands::AuditState;
use crate::connectors::telegram::{qr_matrix, QrMatrix};
use crate::conversation_commands::{
    ConversationMode, ConversationRecord, CONVERSATION_CHANGED_EVENT,
};
use crate::conversation_turns::SendTurn;
use crate::inbox_commands::INBOX_CHANGED_EVENT;
use crate::memory_learn::TextOrigin;
use crate::model_picker_commands::ModelOverride;
use crate::rag_commands::RagState;

/// Events buffered for the bridge's connections; a phone further behind is
/// disconnected (and reconnects).
const EVENT_BUFFER: usize = 1024;

/// How long pairing waits for the relay before it shows a QR code without
/// one (the phone then finds the desktop through address lookup).
const RELAY_WAIT: Duration = Duration::from_secs(10);

/// Characters of extracted text sent in one part: well inside one 4 MiB
/// frame even when every character is escaped in JSON.
pub const TEXT_PART_CHARS: usize = 500_000;

/// Narrowest and widest page image a phone may ask for, in pixels.
const MIN_PAGE_WIDTH: u32 = 320;
const MAX_PAGE_WIDTH: u32 = 1600;

/// Most bytes of a credential-store entry's text (Windows: 2,560 bytes of
/// UTF-16).
const MAX_CREDENTIAL_UTF16: usize = 1280;

/// Sent to the window when a phone pairs or is removed (Settings → Phone reloads).
pub const PHONE_DEVICES_EVENT: &str = "phone_devices_changed";

fn devices_changed(app: &AppHandle) {
    if let Err(e) = app.emit(PHONE_DEVICES_EVENT, ()) {
        tracing::warn!(target: "shodh::link", error = %e, "the window was not told the paired phones changed");
    }
}

const LOCAL_ONLY: &str =
    "Local-only mode is on, so your phone cannot reach this computer. Turn it off in Settings, Privacy.";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn iso(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(i64::try_from(ms).unwrap_or(i64::MAX))
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

// ── Keys and the paired phones, in the OS credential store ─────────

/// The paired phones as one credential-store entry (JSON). Refused when it
/// would not fit an entry; at most five phones of 40-character names always
/// fit ([`tests::the_most_phones_fit_one_credential_entry`]).
fn devices_json(devices: &[PairedDevice]) -> Result<String, String> {
    let json = serde_json::to_string(devices).map_err(|e| e.to_string())?;
    if json.encode_utf16().count() > MAX_CREDENTIAL_UTF16 {
        return Err("The list of paired phones is too long to keep.".to_string());
    }
    Ok(json)
}

/// The paired phones, kept in the OS credential store.
struct CredentialDevices;

impl DeviceStore for CredentialDevices {
    fn load(&self) -> Result<Vec<PairedDevice>, String> {
        match api_key_store::load_secret(api_key_store::PHONE_DEVICES_USER)? {
            None => Ok(Vec::new()),
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| format!("The list of paired phones could not be read: {e}")),
        }
    }

    fn save(&self, devices: &[PairedDevice]) -> Result<(), String> {
        if devices.is_empty() {
            return api_key_store::remove_secret(api_key_store::PHONE_DEVICES_USER);
        }
        api_key_store::store_secret(api_key_store::PHONE_DEVICES_USER, &devices_json(devices)?)
    }
}

/// The desktop's node key: the stored one, or a new one stored when none is.
/// Any other failure to read it is an error, never a new key: a new key would
/// silently unpair every phone.
fn node_key_with(
    load: impl FnOnce() -> Result<Option<String>, String>,
    store: impl FnOnce(&str) -> Result<(), String>,
) -> Result<SecretKey, String> {
    match load()? {
        Some(hex_key) => {
            let bytes: [u8; 32] = hex::decode(hex_key.trim())
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or("The phone link's key in the OS credential store is damaged.")?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        None => {
            let key = SecretKey::generate();
            store(&hex::encode(key.to_bytes()))?;
            Ok(key)
        }
    }
}

fn node_key() -> Result<SecretKey, String> {
    node_key_with(
        || api_key_store::load_secret(api_key_store::PHONE_NODE_KEY_USER),
        |value| api_key_store::store_secret(api_key_store::PHONE_NODE_KEY_USER, value),
    )
}

// ── State ──────────────────────────────────────────────────────────

struct Running {
    endpoint: Endpoint,
    link: Arc<Link<TauriHost>>,
}

/// Managed state of the phone link.
pub struct PhoneLinkState {
    devices: OnceCell<Arc<DeviceRegistry>>,
    pairing: Arc<PendingPairing>,
    /// The desktop's `agent_event`s, for the bridge's connections.
    events: broadcast::Sender<LinkEvent>,
    running: AsyncMutex<Option<Running>>,
}

impl PhoneLinkState {
    async fn devices(&self) -> Result<Arc<DeviceRegistry>, String> {
        self.devices
            .get_or_try_init(|| async {
                DeviceRegistry::open(Box::new(CredentialDevices)).map(Arc::new)
            })
            .await
            .cloned()
    }
}

/// Manage the state and forward the desktop's events the bridge sends on: agent
/// events, which conversation the desktop saved, and that the Inbox changed.
pub fn manage(app: &AppHandle) {
    let (events, _) = broadcast::channel(EVENT_BUFFER);
    for name in [AGENT_EVENT, CONVERSATION_CHANGED_EVENT, INBOX_CHANGED_EVENT] {
        let forward = events.clone();
        app.listen_any(name, move |event| {
            // Parsed only while a phone is connected.
            if forward.receiver_count() == 0 {
                return;
            }
            match serde_json::from_str::<Value>(event.payload()) {
                Ok(payload) => {
                    let _ = forward.send(LinkEvent {
                        name: name.to_string(),
                        payload,
                    });
                }
                Err(e) => {
                    tracing::warn!(target: "shodh::link", error = %e, event = name, "an event could not be read for the phone link")
                }
            }
        });
    }
    app.manage(PhoneLinkState {
        devices: OnceCell::new(),
        pairing: Arc::new(PendingPairing::default()),
        events,
        running: AsyncMutex::new(None),
    });
}

fn local_only(app: &AppHandle) -> bool {
    let Ok(dir) = crate::profile::app_data_dir(app) else {
        return true;
    };
    crate::app_settings::SettingsStore::in_dir(&dir)
        .load()
        .map(|s| s.policy.local_only)
        // Settings that cannot be read keep the link off.
        .unwrap_or(true)
}

/// At startup: listen for phones only when one is paired (and Local-only
/// mode is off).
pub fn start_saved(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Settings can change before the link is managed at startup.
        let Some(state) = app.try_state::<PhoneLinkState>() else {
            return;
        };
        let paired = match state.devices().await {
            Ok(devices) => !devices.is_empty(),
            Err(e) => {
                tracing::warn!(target: "shodh::link", error = %e, "the paired phones could not be read");
                false
            }
        };
        if paired && !local_only(&app) {
            if let Err(e) = ensure_running(&app).await {
                tracing::warn!(target: "shodh::link", error = %e, "the phone link did not start");
            }
        }
    });
}

/// After a settings change: Local-only mode on closes every phone's
/// connection; off again listens for paired phones.
pub fn settings_changed(app: &AppHandle, settings: &crate::app_settings::AppSettings) {
    if settings.policy.local_only {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { stop(&app).await });
    } else {
        start_saved(app);
    }
}

async fn ensure_running(app: &AppHandle) -> Result<Arc<Link<TauriHost>>, String> {
    if local_only(app) {
        return Err(LOCAL_ONLY.to_string());
    }
    let state = app
        .try_state::<PhoneLinkState>()
        .ok_or("The phone link is still starting. Try again in a moment.")?;
    let devices = state.devices().await?;
    let mut running = state.running.lock().await;
    if let Some(running) = running.as_ref() {
        return Ok(running.link.clone());
    }
    let key = tauri::async_runtime::spawn_blocking(node_key)
        .await
        .map_err(|e| e.to_string())??;
    let endpoint = shodh_link::desktop_endpoint(key)
        .await
        .map_err(|e| format!("The phone link could not start: {e}"))?;
    let host = Arc::new(TauriHost {
        app: app.clone(),
        events: state.events.clone(),
    });
    let link = Arc::new(Link::new(host, devices, state.pairing.clone()));
    tauri::async_runtime::spawn(link.clone().serve(endpoint.clone()));
    tracing::info!(target: "shodh::link", "the phone link is listening");
    *running = Some(Running {
        endpoint,
        link: link.clone(),
    });
    Ok(link)
}

async fn stop(app: &AppHandle) {
    let Some(state) = app.try_state::<PhoneLinkState>() else {
        return;
    };
    let taken = state.running.lock().await.take();
    if let Some(running) = taken {
        running.endpoint.close().await;
        tracing::info!(target: "shodh::link", "the phone link stopped");
    }
}

/// Stop listening when no phone is paired and no pairing is open.
async fn stop_if_idle(app: &AppHandle) {
    let Some(state) = app.try_state::<PhoneLinkState>() else {
        return;
    };
    let idle = match state.devices().await {
        Ok(devices) => devices.is_empty() && !state.pairing.open(now_ms()),
        Err(_) => false,
    };
    if idle {
        stop(app).await;
    }
}

// ── Commands (Settings → Phone) ────────────────────────────────────

/// A pairing QR code and when it stops working. The code itself is never
/// sent as text: only the modules to draw.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingView {
    pub qr: QrMatrix,
    pub expires_at: String,
}

/// Start pairing a phone: a new one-time code (five minutes, single use) as
/// a QR code. A code shown before stops working.
#[tauri::command]
pub async fn phone_pairing_start(
    app: AppHandle,
    state: State<'_, PhoneLinkState>,
) -> Result<PairingView, String> {
    let link = ensure_running(&app).await?;
    let endpoint = state
        .running
        .lock()
        .await
        .as_ref()
        .map(|r| r.endpoint.clone())
        .ok_or("The phone link stopped. Try again.")?;
    // The relay hint helps the phone reach this computer at once; without one
    // it still does, through address lookup.
    let _ = tokio::time::timeout(RELAY_WAIT, endpoint.online()).await;
    let relay_url = endpoint.addr().relay_urls().next().cloned();
    let secret = new_secret()?;
    let expires_at = link.pairing().issue(secret, now_ms());
    let offer = PairingOffer {
        node_id: endpoint.id(),
        relay_url,
        secret,
        expires_at,
    };
    let qr = qr_matrix(&offer.to_qr())?;
    // Stop listening again when the code expires unused and no phone is paired.
    let later = app.clone();
    tauri::async_runtime::spawn(async move {
        let wait = expires_at.saturating_sub(now_ms());
        tokio::time::sleep(Duration::from_millis(wait)).await;
        stop_if_idle(&later).await;
    });
    Ok(PairingView {
        qr,
        expires_at: iso(expires_at),
    })
}

/// A paired phone, as Settings → Phone lists it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub node_id: String,
    pub name: String,
    pub paired_at: String,
    pub last_seen: Option<String>,
}

fn device_view(device: PairedDevice) -> DeviceView {
    DeviceView {
        node_id: device.node_id,
        name: device.name,
        paired_at: iso(device.paired_at),
        last_seen: device.last_seen.map(iso),
    }
}

/// The paired phones.
#[tauri::command]
pub async fn phone_devices(state: State<'_, PhoneLinkState>) -> Result<Vec<DeviceView>, String> {
    Ok(state
        .devices()
        .await?
        .list()
        .into_iter()
        .map(device_view)
        .collect())
}

/// Remove a paired phone: its connections close at once and its key is
/// refused from now on. Only the user removes a phone (agent-coverage.json).
#[tauri::command]
pub async fn phone_device_remove(
    app: AppHandle,
    state: State<'_, PhoneLinkState>,
    node_id: String,
) -> Result<bool, String> {
    let devices = state.devices().await?;
    let name = devices.get(&node_id).map(|d| d.name);
    let removed = devices.remove(&node_id)?;
    if let Some(name) = name.filter(|_| removed) {
        app.state::<AuditState>()
            .record(activity_record(&CallRecord {
                caller: Caller {
                    node_id: node_id.clone(),
                    name,
                },
                command: "remove".to_string(),
                outcome: Outcome::Done,
            }));
    }
    if removed {
        devices_changed(&app);
    }
    stop_if_idle(&app).await;
    Ok(removed)
}

// ── Activity ───────────────────────────────────────────────────────

/// The Activity entry of a phone link event: the phone's name, the command
/// and how it ended (with the reason of a refusal). Never the arguments: a
/// question or a path stays out of this entry (a question is recorded by the
/// session itself, as for a question typed on the desktop).
pub fn activity_record(record: &CallRecord) -> AuditRecord {
    let (outcome, reason) = match &record.outcome {
        Outcome::Done => ("done", None),
        Outcome::Refused(reason) => ("refused", Some(reason.as_str())),
        Outcome::Failed => ("failed", None),
    };
    let mut payload = json!({
        "device": record.caller.name,
        "command": record.command,
        "outcome": outcome,
    });
    if let Some(reason) = reason {
        payload["reason"] = json!(reason);
    }
    // The user's own removal is the user's; everything else is the phone's.
    let principal = if record.command == "remove" {
        LOCAL_OWNER.to_string()
    } else {
        format!("phone:{}", record.caller.node_id)
    };
    AuditRecord::new(AuditEventType::PhoneLink, payload).principal(principal)
}

// ── The desktop as the bridge's host ───────────────────────────────

struct TauriHost {
    app: AppHandle,
    events: broadcast::Sender<LinkEvent>,
}

impl LinkHost for TauriHost {
    fn invoke(
        &self,
        _caller: &Caller,
        command: &str,
        args: Value,
    ) -> impl std::future::Future<Output = Result<Value, Value>> + Send {
        let app = self.app.clone();
        let command = command.to_string();
        async move { dispatch(&app, &command, args).await }
    }

    fn approval_kind(&self, session_id: &str, step: &PendingStep) -> ApprovalKind {
        approval_kind(
            self.app.state::<AgentSessions>().remote_facts(session_id),
            step,
        )
    }

    fn record(&self, record: CallRecord) {
        self.app
            .state::<AuditState>()
            .record(activity_record(&record));
        if record.command == "pair" && record.outcome == Outcome::Done {
            devices_changed(&self.app);
        }
    }

    fn events(&self) -> broadcast::Receiver<LinkEvent> {
        self.events.subscribe()
    }

    fn link_allowed(&self) -> Result<(), String> {
        if local_only(&self.app) {
            Err(LOCAL_ONLY.to_string())
        } else {
            Ok(())
        }
    }

    fn now_ms(&self) -> u64 {
        now_ms()
    }
}

// ── The desktop's rules for phones ─────────────────────────────────

/// What granting `step` would allow: Ask and Creator steps in a research
/// session; a command, an edit or anything at all in a Code session is not
/// a phone's to allow. A session that is not live is unknown.
pub fn approval_kind(facts: Option<RemoteFacts>, step: &PendingStep) -> ApprovalKind {
    let Some(facts) = facts else {
        return ApprovalKind::Unknown;
    };
    let tool = step.tool.as_str();
    if tool == RUN_COMMAND || OMP_EXECUTION_TOOLS.contains(&tool) {
        ApprovalKind::CodeCommand
    } else if CHANGE_TOOLS.contains(&tool) {
        ApprovalKind::FileEdit
    } else if facts.code {
        ApprovalKind::CodeStep
    } else if facts.agent_mode == AgentMode::Creator {
        ApprovalKind::CreatorStep
    } else {
        ApprovalKind::AskStep
    }
}

/// Modes a phone may answer in: the ones in which every change asks (Ask,
/// Plan) and Creator, whose steps stay inside its creation folder. Accept
/// edits and Auto run steps without asking, so only the computer picks them.
fn phone_mode(mode: Option<AgentMode>) -> Result<AgentMode, Value> {
    match mode.unwrap_or(AgentMode::Ask) {
        AgentMode::AcceptEdits | AgentMode::Auto => Err(shodh_link::bridge::refused(
            "This mode runs steps without asking, so only your computer can choose it.",
        )),
        allowed => Ok(allowed),
    }
}

const CODE_ON_COMPUTER: &str =
    "Code mode works on your computer only. Continue this conversation there.";

/// The session a phone's question goes to must be one where every step that
/// is not the phone's to allow still asks the computer.
fn check_session(facts: Option<RemoteFacts>) -> Result<(), Value> {
    match facts {
        Some(facts) if facts.code => Err(shodh_link::bridge::refused(CODE_ON_COMPUTER)),
        Some(facts) => phone_mode(Some(facts.agent_mode)).map(|_| ()).map_err(|_| {
            shodh_link::bridge::refused(
                "This conversation runs steps without asking on your computer. Continue it there.",
            )
        }),
        // Not live: the command itself says so.
        None => Ok(()),
    }
}

/// A path from the phone, admitted only inside a linked folder: the shape
/// first (absolute, no `.` or `..`, not a URL), then the real location
/// (links followed) inside one of `roots`.
pub fn within_linked(raw: &str, roots: &[PathBuf]) -> Result<PathBuf, PathRefusal> {
    let path = path_shape(raw)?;
    containing_root(&path, roots)
        .map(|_| path.clone())
        .ok_or(PathRefusal::OutsideLinkedFolders)
}

fn path_refused(refusal: PathRefusal) -> Value {
    shodh_link::bridge::refused(refusal.to_string())
}

/// One entry of a folder listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub name: String,
    pub path: String,
}

/// A folder of the Library: its child folders and the indexed files directly
/// in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderListing {
    pub path: String,
    pub folders: Vec<FolderEntry>,
    pub files: Vec<FolderEntry>,
}

fn same_name(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// The child folders and files of `folder`, from the indexed files' paths
/// (as `list_indexed_sources` spells them): the same tree the desktop's
/// Library builds from the index.
pub fn folder_children(folder: &Path, files: &[String]) -> FolderListing {
    let parts = |p: &Path| -> Vec<std::ffi::OsString> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| c.as_os_str().to_os_string())
            .collect()
    };
    let base = parts(folder);
    let mut folders: Vec<FolderEntry> = Vec::new();
    let mut listed: Vec<FolderEntry> = Vec::new();
    for file in files {
        let path = Path::new(file);
        let own = parts(path);
        if own.len() <= base.len() || !base.iter().zip(&own).all(|(a, b)| same_name(a, b)) {
            continue;
        }
        let child = &own[base.len()];
        let name = child.to_string_lossy().into_owned();
        let child_path = folder.join(child).display().to_string();
        if own.len() == base.len() + 1 {
            listed.push(FolderEntry {
                name,
                path: path.display().to_string(),
            });
        } else if !folders.iter().any(|f| same_name(f.name.as_ref(), child)) {
            folders.push(FolderEntry {
                name,
                path: child_path,
            });
        }
    }
    let key = |e: &FolderEntry| e.name.to_lowercase();
    folders.sort_by_key(key);
    listed.sort_by_key(key);
    listed.dedup_by(|a, b| a.path == b.path);
    FolderListing {
        path: folder.display().to_string(),
        folders,
        files: listed,
    }
}

/// Part `part` of `text`, in [`TEXT_PART_CHARS`]-character parts.
pub fn text_part(text: &str, part: usize) -> (String, usize) {
    let total = text.chars().count();
    let parts = total.div_ceil(TEXT_PART_CHARS).max(1);
    let piece: String = text
        .chars()
        .skip(part.saturating_mul(TEXT_PART_CHARS))
        .take(TEXT_PART_CHARS)
        .collect();
    (piece, parts)
}

// ── Running a phone's call ─────────────────────────────────────────

fn invalid(message: impl std::fmt::Display) -> Value {
    json!({ "code": "invalid_request", "message": message.to_string() })
}

fn args<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, Value> {
    serde_json::from_value(args).map_err(invalid)
}

/// A command's result as the phone receives it: the value, or the error as
/// the desktop window would.
fn reply<T: Serialize, E: Serialize>(result: Result<T, E>) -> Result<Value, Value> {
    let encode = |v: Result<Value, serde_json::Error>| {
        v.unwrap_or_else(|e| json!({ "code": "runtime_error", "message": e.to_string() }))
    };
    match result {
        Ok(value) => Ok(encode(serde_json::to_value(value))),
        Err(error) => Err(encode(serde_json::to_value(error))),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartArgs {
    conversation_id: String,
    profile_id: Option<String>,
    instructions: Option<String>,
    parent_conversation_id: Option<String>,
    model_override: Option<ModelOverride>,
    mode: Option<ConversationMode>,
    workspace_id: Option<String>,
    agent_mode: Option<AgentMode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendArgs {
    session_id: String,
    text: String,
    request_id: String,
    attached_context: Option<Vec<AttachedContext>>,
    history: Option<Vec<HistoryTurn>>,
    scope: Option<SendScope>,
    text_origin: Option<TextOrigin>,
    typed_text: Option<String>,
    prior_run_ids: Option<Vec<String>>,
    review_action: Option<bool>,
    suggestion_ignored: Option<bool>,
    /// The question and the answer as it starts, which the desktop saves
    /// (`conversation_turns`): required from a phone, so every answer it asks for is
    /// kept by the desktop even when the phone goes away mid-answer.
    turn: Option<SendTurn>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionArgs {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApproveArgs {
    session_id: String,
    step_id: String,
    approved: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationArgs {
    conversation_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileArgs {
    file_path: String,
    part: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FolderArgs {
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageArgs {
    file_path: String,
    /// 1-based.
    page: usize,
    width: u32,
}

async fn linked_roots(app: &AppHandle) -> Result<Vec<PathBuf>, Value> {
    IndexedRoots {
        rag: app.state::<RagState>().rag.clone(),
    }
    .folders()
    .await
    .map_err(|e| json!({ "code": "unavailable", "message": e.to_string() }))
}

/// The file a phone named, admitted only inside a linked folder.
async fn linked_file(app: &AppHandle, raw: &str) -> Result<String, Value> {
    let roots = linked_roots(app).await?;
    within_linked(raw, &roots)
        .map(|p| p.display().to_string())
        .map_err(path_refused)
}

/// One conversation as the phone's list shows it (`conversations_list`):
/// no messages, which as a whole exceed one frame. `mode` is the record's
/// (`"research"`, `"code"`) and `agentMode` the agent mode picked for it
/// (`"creator"` among them); each is null when the record has none, which
/// the phone reads as Research.
pub fn phone_conversation_summary(record: &ConversationRecord) -> Value {
    json!({
        "id": record.id,
        "title": record.title,
        "createdAt": record.created_at,
        "updatedAt": record.updated_at,
        "pinned": record.pinned,
        "workspaceId": record.workspace_id,
        "mode": record.mode,
        "agentMode": record.agent_mode,
        "messageCount": record.messages.len(),
    })
}

/// An approval waiting on the computer, as the phone's Approvals lists it:
/// the step, its conversation and title, when it was asked, and whether the
/// phone may allow it (`phoneMay`, the desktop's own rule). An item without
/// its tool is sent without one: the bridge then never lets a phone answer
/// it, and it shows "Approve this on your computer".
pub fn phone_approval(item: &InboxItem, facts: Option<RemoteFacts>) -> Value {
    let text = |name: &str| {
        item.data
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let tool = text("tool");
    let phone_may = tool.as_ref().is_some_and(|tool| {
        let step = PendingStep {
            tool: tool.clone(),
            tier: String::new(),
        };
        shodh_link::allowlist::remote_may_approve(&approval_kind(facts, &step))
    });
    json!({
        "sessionId": text("sessionId"),
        "stepId": text("stepId"),
        "conversationId": text("conversationId"),
        "tool": tool,
        "title": item.title,
        "requestedAt": item.created_at,
        "phoneMay": phone_may,
    })
}

/// Run an allowlisted command for a phone with the desktop's own handler.
async fn dispatch(app: &AppHandle, command: &str, raw: Value) -> Result<Value, Value> {
    use crate::source_viewer_commands as viewer;
    match command {
        "agent_start" => {
            let a: StartArgs = args(raw)?;
            if a.mode == Some(ConversationMode::Code) {
                return Err(shodh_link::bridge::refused(CODE_ON_COMPUTER));
            }
            if a.profile_id.is_some() {
                return Err(shodh_link::bridge::refused(
                    "Your phone uses Shodh's own assistant. Choose another on your computer.",
                ));
            }
            let agent_mode = phone_mode(a.agent_mode)?;
            reply(
                agent_session_commands::agent_start(
                    app.clone(),
                    a.conversation_id,
                    None,
                    a.instructions,
                    a.parent_conversation_id,
                    a.model_override,
                    a.mode,
                    a.workspace_id,
                    Some(agent_mode),
                    app.state(),
                    app.state(),
                    app.state(),
                    app.state(),
                    app.state(),
                    app.state(),
                    app.state(),
                )
                .await,
            )
        }
        "agent_send" => {
            let a: SendArgs = args(raw)?;
            check_session(app.state::<AgentSessions>().remote_facts(&a.session_id))?;
            if a.review_action == Some(true) {
                return Err(shodh_link::bridge::refused(CODE_ON_COMPUTER));
            }
            if a.turn.is_none() {
                return Err(invalid(
                    "A question from your phone names its conversation, so your computer saves the answer.",
                ));
            }
            reply(
                agent_session_commands::agent_send(
                    app.clone(),
                    a.session_id,
                    a.text,
                    a.request_id,
                    a.attached_context,
                    a.history,
                    a.scope,
                    a.text_origin,
                    a.typed_text,
                    a.prior_run_ids,
                    None,
                    a.suggestion_ignored,
                    a.turn,
                    app.state(),
                    app.state(),
                    app.state(),
                    app.state(),
                    app.state(),
                    app.state(),
                )
                .await,
            )
        }
        "agent_abort" => {
            let a: SessionArgs = args(raw)?;
            reply(agent_session_commands::agent_abort(app.clone(), a.session_id, app.state()).await)
        }
        "agent_approve" => {
            // The bridge sent only an answer this phone may give, without "for the
            // rest of the conversation" or a review token.
            let a: ApproveArgs = args(raw)?;
            reply(
                agent_session_commands::agent_approve(
                    a.session_id,
                    a.step_id,
                    a.approved,
                    Some(false),
                    None,
                    app.clone(),
                    app.state(),
                )
                .await,
            )
        }
        "approvals_pending" => {
            let items = crate::inbox_commands::pending_approvals(app)
                .await
                .map_err(|e| json!({ "code": "unavailable", "message": e }))?;
            let sessions = app.state::<AgentSessions>();
            let list: Vec<Value> = items
                .iter()
                .map(|item| {
                    let session = item.data["sessionId"].as_str().unwrap_or_default();
                    phone_approval(item, sessions.remote_facts(session))
                })
                .collect();
            Ok(Value::Array(list))
        }
        "workspaces_list" => reply(
            crate::workspace_commands::workspaces_list(app.clone(), app.state(), Some(false)).await,
        ),
        "conversations_list" => {
            let records = crate::conversation_commands::load_conversations(
                app.clone(),
                app.state(),
                app.state(),
                app.state(),
            )
            .await
            .map_err(|e| json!(e))?;
            Ok(Value::Array(
                records.iter().map(phone_conversation_summary).collect(),
            ))
        }
        "load_conversation" => {
            let a: ConversationArgs = args(raw)?;
            reply(
                crate::conversation_commands::load_conversation(app.clone(), a.conversation_id)
                    .await,
            )
        }
        "library_folders" => {
            let a: FolderArgs = args(raw)?;
            let roots = linked_roots(app).await?;
            match a.path {
                None => {
                    let mut folders: Vec<FolderEntry> = roots
                        .iter()
                        .map(|root| FolderEntry {
                            name: root
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| root.display().to_string()),
                            path: root.display().to_string(),
                        })
                        .collect();
                    folders.sort_by_key(|f| f.name.to_lowercase());
                    reply::<_, String>(Ok(json!({ "folders": folders })))
                }
                Some(raw_path) => {
                    let folder = within_linked(&raw_path, &roots).map_err(path_refused)?;
                    let sources = crate::database_commands::list_indexed_sources(app.state())
                        .await
                        .map_err(|e| json!(e))?;
                    let files: Vec<String> = sources
                        .into_iter()
                        .map(|s| s.display_path)
                        .filter(|p| !p.contains("://"))
                        .collect();
                    reply::<_, String>(Ok(folder_children(&folder, &files)))
                }
            }
        }
        "get_source_file_info" => {
            let a: FileArgs = args(raw)?;
            let path = linked_file(app, &a.file_path).await?;
            reply(viewer::get_source_file_info(app.state(), path).await)
        }
        "get_pdf_info" => {
            let a: FileArgs = args(raw)?;
            let path = linked_file(app, &a.file_path).await?;
            reply(viewer::get_pdf_info(app.state(), path).await)
        }
        "read_source_table" => {
            let a: FileArgs = args(raw)?;
            let path = linked_file(app, &a.file_path).await?;
            reply(viewer::read_source_table(app.state(), path).await)
        }
        "read_source_text" => {
            let a: FileArgs = args(raw)?;
            let path = linked_file(app, &a.file_path).await?;
            let source = reply(viewer::read_source_text(app.state(), path).await)?;
            let text = source["text"].as_str().unwrap_or_default();
            let part = a.part.unwrap_or(0);
            let (piece, parts) = text_part(text, part);
            if part >= parts {
                return Err(invalid(format!("This text has {parts} parts.")));
            }
            Ok(json!({
                "text": piece,
                "part": part,
                "parts": parts,
                "truncated": source["truncated"],
                "totalChars": source["totalChars"],
            }))
        }
        "read_source_page" => {
            let a: PageArgs = args(raw)?;
            let path = linked_file(app, &a.file_path).await?;
            // The viewer's own check: an indexed regular file, at its real location.
            let info = reply(viewer::get_source_file_info(app.state(), path).await)?;
            let kind = info["kind"].as_str().unwrap_or_default().to_string();
            let real = info["path"].as_str().unwrap_or_default().to_string();
            let width = a.width.clamp(MIN_PAGE_WIDTH, MAX_PAGE_WIDTH);
            tauri::async_runtime::spawn_blocking(move || render_page(&real, &kind, a.page, width))
                .await
                .map_err(|e| json!({ "code": "runtime_error", "message": e.to_string() }))?
        }
        other => Err(shodh_link::bridge::refused(format!(
            "Your phone cannot do this ({other}). Use Shodh on your computer."
        ))),
    }
}

/// One PDF page (1-based) or an image, as a PNG `width` pixels wide at most.
fn render_page(path: &str, kind: &str, page: usize, width: u32) -> Result<Value, Value> {
    use base64::Engine;
    use shodh_rag::processing::ocr::raster::{load_image, PdfPages};
    let failed = |e: String| json!({ "code": "readFailed", "message": e });
    let (image, pages) = match kind {
        "pdf" => {
            let bytes =
                std::fs::read(path).map_err(|e| failed(format!("Could not read the file: {e}")))?;
            if bytes.len() as u64 > crate::source_viewer_commands::MAX_SOURCE_BYTES {
                return Err(failed("This file is too large to show.".to_string()));
            }
            let pdf = PdfPages::open(&bytes).map_err(|e| failed(e.to_string()))?;
            if page == 0 || page > pdf.count() {
                return Err(invalid(format!("This document has {} pages.", pdf.count())));
            }
            // About the phone's width for a Letter or A4 page, never above 200 dpi.
            let dpi = ((width as f32 / 8.27).ceil() as u32).clamp(72, 200);
            let image = pdf
                .render(page - 1, dpi)
                .map_err(|e| failed(e.to_string()))?;
            (image, pdf.count())
        }
        "image" => {
            if page != 1 {
                return Err(invalid("An image has one page."));
            }
            (
                load_image(Path::new(path)).map_err(|e| failed(e.to_string()))?,
                1,
            )
        }
        _ => {
            return Err(json!({
                "code": "unsupported",
                "message": "This file is shown as text, not as pages.",
            }))
        }
    };
    let image = if image.width() > width {
        let height =
            ((image.height() as u64 * width as u64) / image.width().max(1) as u64).max(1) as u32;
        image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let (shown_width, shown_height) = image.dimensions();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| failed(e.to_string()))?;
    Ok(json!({
        "page": page,
        "pages": pages,
        "width": shown_width,
        "height": shown_height,
        "png": base64::engine::general_purpose::STANDARD.encode(&png),
    }))
}

#[cfg(test)]
mod end_to_end;

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_link::pairing::{MAX_DEVICES, MAX_DEVICE_NAME_CHARS};

    fn step(tool: &str) -> PendingStep {
        PendingStep {
            tool: tool.to_string(),
            tier: "write".to_string(),
        }
    }

    fn facts(code: bool, agent_mode: AgentMode) -> Option<RemoteFacts> {
        Some(RemoteFacts { code, agent_mode })
    }

    #[test]
    fn the_link_is_the_app_version() {
        assert_eq!(shodh_link::LINK_VERSION, env!("CARGO_PKG_VERSION"));
        // The events the bridge forwards are the ones the desktop emits.
        assert_eq!(shodh_link::bridge::AGENT_EVENT, AGENT_EVENT);
        assert_eq!(
            shodh_link::bridge::CONVERSATION_CHANGED,
            CONVERSATION_CHANGED_EVENT
        );
        assert_eq!(shodh_link::bridge::INBOX_CHANGED, INBOX_CHANGED_EVENT);
        assert_eq!(
            shodh_link::bridge::PENDING_APPROVALS,
            "approvals_pending",
            "the bridge remembers the steps of the read the desktop serves"
        );
    }

    /// The phone's Conversations: each summary says the conversation's mode and agent
    /// mode as the desktop saved them (read through the record's own rules for unknown
    /// values), null for a record without them, and carries none of the messages.
    #[test]
    fn the_conversation_summary_includes_the_mode() {
        let record = |extra: Value| {
            let mut stored = json!({
                "id": "c1",
                "title": "Fix the flaky test",
                "messages": [
                    {"id": "q", "role": "user", "content": "secret question", "timestamp": "2026-11-14T09:00:00Z"},
                    {"id": "a", "role": "assistant", "content": "secret answer", "timestamp": "2026-11-14T09:01:00Z"}
                ],
                "createdAt": "2026-11-14T09:00:00Z",
                "updatedAt": "2026-11-14T09:01:00Z",
                "pinned": false,
            });
            for (key, value) in extra.as_object().unwrap() {
                stored[key] = value.clone();
            }
            serde_json::from_value::<ConversationRecord>(stored).unwrap()
        };

        let older = phone_conversation_summary(&record(json!({})));
        assert_eq!(older["mode"], Value::Null, "an older record is Research");
        assert_eq!(older["agentMode"], Value::Null);
        assert_eq!(older["workspaceId"], Value::Null);
        assert_eq!(older["messageCount"], 2);
        assert_eq!(older["title"], "Fix the flaky test");
        assert!(
            !older.to_string().contains("secret"),
            "no message text goes to the list"
        );

        let code = phone_conversation_summary(&record(
            json!({"mode": "code", "agentMode": "acceptEdits", "workspaceId": "w1"}),
        ));
        assert_eq!(code["mode"], "code");
        assert_eq!(code["agentMode"], "acceptEdits");
        assert_eq!(code["workspaceId"], "w1");

        let creator =
            phone_conversation_summary(&record(json!({"mode": "code", "agentMode": "creator"})));
        assert_eq!(creator["agentMode"], "creator");

        let research = phone_conversation_summary(&record(json!({"mode": "research"})));
        assert_eq!(research["mode"], "research");

        // A mode from a newer version: the record reads it as none (Research), and an
        // unknown agent mode as Ask, never a looser one.
        let newer =
            phone_conversation_summary(&record(json!({"mode": "hologram", "agentMode": "warp"})));
        assert_eq!(newer["mode"], Value::Null);
        assert_eq!(newer["agentMode"], "ask");
    }

    /// The phone's Approvals: each waiting step says whether the phone may allow it,
    /// by the desktop's own rule, and a step without its tool never may.
    #[test]
    fn approvals_listed_for_a_phone_say_which_it_may_answer() {
        use shodh_rag::inbox::{InboxKind, InboxStatus};
        let item = |tool: Option<&str>| InboxItem {
            id: "approval:s1:t1".to_string(),
            kind: InboxKind::Approval,
            status: InboxStatus::NeedsYou,
            title: "Fetch arxiv.org".to_string(),
            detail: None,
            link: None,
            data: match tool {
                Some(tool) => {
                    json!({ "sessionId": "s1", "stepId": "t1", "tool": tool, "conversationId": "c1" })
                }
                None => json!({ "sessionId": "s1", "stepId": "t1" }),
            },
            created_at: "2026-11-13T10:00:00.000Z".to_string(),
            updated_at: "2026-11-13T10:00:00.000Z".to_string(),
        };
        let ask = facts(false, AgentMode::Ask);
        let listed = phone_approval(&item(Some("fetch_url")), ask);
        assert_eq!(
            listed,
            json!({
                "sessionId": "s1",
                "stepId": "t1",
                "conversationId": "c1",
                "tool": "fetch_url",
                "title": "Fetch arxiv.org",
                "requestedAt": "2026-11-13T10:00:00.000Z",
                "phoneMay": true,
            })
        );
        assert_eq!(
            phone_approval(&item(Some("fetch_url")), facts(false, AgentMode::Creator))["phoneMay"],
            true
        );
        // Code mode, a command or an edit anywhere, and a session that is not live.
        assert_eq!(
            phone_approval(&item(Some("fetch_url")), facts(true, AgentMode::Ask))["phoneMay"],
            false
        );
        assert_eq!(
            phone_approval(&item(Some(RUN_COMMAND)), ask)["phoneMay"],
            false
        );
        for tool in CHANGE_TOOLS {
            assert_eq!(phone_approval(&item(Some(tool)), ask)["phoneMay"], false);
        }
        assert_eq!(
            phone_approval(&item(Some("fetch_url")), None)["phoneMay"],
            false
        );
        // Without its tool (an item from an older build), never.
        let unplaced = phone_approval(&item(None), ask);
        assert_eq!(unplaced["phoneMay"], false);
        assert_eq!(unplaced["tool"], Value::Null);
        assert_eq!(unplaced["stepId"], "t1");
    }

    /// Every allowlisted command is one the desktop registers (and runs here with its
    /// own handler), or a bridge-only operation the desktop window does not have; each
    /// has its arm in `dispatch`.
    #[test]
    fn every_allowlisted_command_is_a_real_command_or_a_bridge_operation() {
        use shodh_link::allowlist::{EntryKind, ALLOWED};
        let registered = crate::agent_coverage::tests::registered_commands();
        let source = include_str!("phone_link.rs");
        let body = &source[source.find("async fn dispatch(").unwrap()..];
        for entry in ALLOWED {
            match entry.kind {
                EntryKind::Command => assert!(
                    registered.contains(entry.command),
                    "{} is not a registered command",
                    entry.command
                ),
                EntryKind::BridgeOnly => assert!(
                    !registered.contains(entry.command),
                    "{} is a registered command; list it as one",
                    entry.command
                ),
            }
            assert!(
                body.contains(&format!("\"{}\" =>", entry.command)),
                "dispatch has no arm for {}",
                entry.command
            );
        }
    }

    #[test]
    fn a_remote_code_approval_is_refused() {
        use shodh_link::allowlist::remote_may_approve;
        // Code mode: its commands, its edits and every other step.
        assert_eq!(
            approval_kind(facts(true, AgentMode::Ask), &step(RUN_COMMAND)),
            ApprovalKind::CodeCommand
        );
        for tool in CHANGE_TOOLS {
            assert_eq!(
                approval_kind(facts(true, AgentMode::Ask), &step(tool)),
                ApprovalKind::FileEdit
            );
        }
        assert_eq!(
            approval_kind(facts(true, AgentMode::Plan), &step("search_documents")),
            ApprovalKind::CodeStep
        );
        for tool in OMP_EXECUTION_TOOLS {
            assert_eq!(
                approval_kind(facts(false, AgentMode::Ask), &step(tool)),
                ApprovalKind::CodeCommand,
                "{tool} runs a program, whatever the session"
            );
        }
        // A file edit outside Code mode is still a file edit.
        assert_eq!(
            approval_kind(facts(false, AgentMode::Creator), &step("write")),
            ApprovalKind::FileEdit
        );
        // A session that is not live is unknown.
        assert_eq!(
            approval_kind(None, &step("fetch_url")),
            ApprovalKind::Unknown
        );
        for (f, s) in [
            (facts(true, AgentMode::Ask), RUN_COMMAND),
            (facts(true, AgentMode::Ask), "edit"),
            (facts(true, AgentMode::Ask), "fetch_url"),
            (facts(false, AgentMode::Ask), "write"),
            (None, "fetch_url"),
        ] {
            assert!(!remote_may_approve(&approval_kind(f, &step(s))), "{s}");
        }
        // Ask and Creator steps may be approved from the phone.
        assert_eq!(
            approval_kind(facts(false, AgentMode::Ask), &step("fetch_url")),
            ApprovalKind::AskStep
        );
        assert_eq!(
            approval_kind(facts(false, AgentMode::Creator), &step("export_document")),
            ApprovalKind::CreatorStep
        );
    }

    #[test]
    fn a_phone_never_picks_a_mode_that_skips_approvals_or_code_mode() {
        assert_eq!(
            phone_mode(None).unwrap(),
            AgentMode::Ask,
            "the default asks"
        );
        for mode in [AgentMode::Ask, AgentMode::Plan, AgentMode::Creator] {
            assert_eq!(phone_mode(Some(mode)).unwrap(), mode);
        }
        for mode in [AgentMode::AcceptEdits, AgentMode::Auto] {
            assert_eq!(phone_mode(Some(mode)).unwrap_err()["code"], "refused");
        }
        assert_eq!(
            check_session(facts(true, AgentMode::Ask)).unwrap_err()["message"],
            CODE_ON_COMPUTER
        );
        assert!(check_session(facts(false, AgentMode::Auto)).is_err());
        assert!(check_session(facts(false, AgentMode::AcceptEdits)).is_err());
        assert!(check_session(facts(false, AgentMode::Ask)).is_ok());
        assert!(check_session(facts(false, AgentMode::Creator)).is_ok());
        assert!(
            check_session(None).is_ok(),
            "the command reports a closed session"
        );
    }

    #[test]
    fn a_path_outside_linked_folders_is_refused() {
        let linked = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let papers = linked.path().join("papers");
        std::fs::create_dir_all(papers.join("2023")).unwrap();
        let inside = papers.join("2023").join("self-rag.pdf");
        std::fs::write(&inside, b"%PDF-1.4").unwrap();
        let secret = outside.path().join("passwords.txt");
        std::fs::write(&secret, b"not for the phone").unwrap();
        let roots = vec![linked.path().to_path_buf()];
        let text = |p: &Path| p.display().to_string();

        // Inside, at any depth, and the linked folder itself.
        assert_eq!(within_linked(&text(&inside), &roots), Ok(inside.clone()));
        assert!(within_linked(&text(linked.path()), &roots).is_ok());
        assert!(within_linked(&text(&papers), &roots).is_ok());

        let refused = Err(PathRefusal::OutsideLinkedFolders);
        // An absolute path outside.
        assert_eq!(within_linked(&text(&secret), &roots), refused);
        assert_eq!(within_linked(&text(outside.path()), &roots), refused);
        // `..` out of the linked folder (and `..` that would stay inside: never accepted).
        let up = format!(
            "{}{sep}..{sep}{}{sep}passwords.txt",
            text(linked.path()),
            outside.path().file_name().unwrap().to_string_lossy(),
            sep = std::path::MAIN_SEPARATOR
        );
        assert_eq!(within_linked(&up, &roots), refused);
        let round = format!(
            "{}{sep}2023{sep}..{sep}2023{sep}self-rag.pdf",
            text(&papers),
            sep = std::path::MAIN_SEPARATOR
        );
        assert_eq!(within_linked(&round, &roots), refused);
        // Relative paths, `.`, URLs.
        assert_eq!(within_linked("papers/self-rag.pdf", &roots), refused);
        assert_eq!(within_linked("..", &roots), refused);
        assert_eq!(
            within_linked(
                &format!("{}{}.", text(&papers), std::path::MAIN_SEPARATOR),
                &roots
            ),
            refused
        );
        assert_eq!(
            within_linked("file:///etc/passwd", &roots),
            Err(PathRefusal::NotAFile)
        );
        // A folder that only starts with the linked folder's name.
        let sibling = format!(
            "{}-other{}x.pdf",
            text(linked.path()),
            std::path::MAIN_SEPARATOR
        );
        assert_eq!(within_linked(&sibling, &roots), refused);
        // Nothing is inside when nothing is linked.
        assert_eq!(within_linked(&text(&inside), &[]), refused);

        // A link inside the linked folder that points outside it.
        #[cfg(unix)]
        {
            let link = linked.path().join("escape");
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
            assert_eq!(
                within_linked(&text(&link.join("passwords.txt")), &roots),
                refused
            );
        }
    }

    #[test]
    fn a_folder_lists_its_child_folders_and_files() {
        let sep = std::path::MAIN_SEPARATOR;
        let root = std::env::temp_dir().join("shodh-library");
        let at = |rest: &str| {
            format!(
                "{}{sep}{}",
                root.display(),
                rest.replace('/', &sep.to_string())
            )
        };
        let files = vec![
            at("b.pdf"),
            at("A.md"),
            at("papers/self-rag.pdf"),
            at("papers/2023/crag.pdf"),
            at("papers/2023/deep/x.txt"),
            at("notes/todo.md"),
            "https://example.org/page".to_string(),
            format!("{}-other{sep}y.pdf", root.display()),
        ];
        let top = folder_children(&root, &files);
        let names =
            |entries: &[FolderEntry]| entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&top.folders), vec!["notes", "papers"]);
        assert_eq!(names(&top.files), vec!["A.md", "b.pdf"]);
        let papers = folder_children(Path::new(&top.folders[1].path), &files);
        assert_eq!(names(&papers.folders), vec!["2023"]);
        assert_eq!(names(&papers.files), vec!["self-rag.pdf"]);
        let year = folder_children(Path::new(&papers.folders[0].path), &files);
        assert_eq!(names(&year.folders), vec!["deep"]);
        assert_eq!(names(&year.files), vec!["crag.pdf"]);
        assert_eq!(year.files[0].path, at("papers/2023/crag.pdf"));
    }

    #[test]
    fn long_text_is_sent_in_parts() {
        let text = "é".repeat(TEXT_PART_CHARS + 10);
        let (first, parts) = text_part(&text, 0);
        assert_eq!(parts, 2);
        assert_eq!(first.chars().count(), TEXT_PART_CHARS);
        let (second, _) = text_part(&text, 1);
        assert_eq!(second.chars().count(), 10);
        assert_eq!(text_part("", 0), (String::new(), 1));
        // A part of the largest text fits one frame even with every character escaped.
        let worst = "\u{1}".repeat(TEXT_PART_CHARS);
        let frame = serde_json::to_vec(&json!({ "text": worst })).unwrap();
        assert!(frame.len() < shodh_link::messages::MAX_FRAME_BYTES);
    }

    #[test]
    fn each_remote_call_is_recorded_in_activity_with_the_device_name() {
        let dir = tempfile::tempdir().unwrap();
        let log = shodh_rag::audit::AuditLog::open(dir.path().join("shodh.db"), None).unwrap();
        let caller = Caller {
            node_id: "ab".repeat(32),
            name: "Pixel 8".to_string(),
        };
        let calls = [
            ("agent_send", Outcome::Done),
            (
                "agent_approve",
                Outcome::Refused("Approve this on your computer".into()),
            ),
            ("read_source_page", Outcome::Failed),
        ];
        for (command, outcome) in calls.clone() {
            log.append(activity_record(&CallRecord {
                caller: caller.clone(),
                command: command.to_string(),
                outcome,
            }))
            .unwrap();
        }
        log.append(activity_record(&CallRecord {
            caller: caller.clone(),
            command: "remove".to_string(),
            outcome: Outcome::Done,
        }))
        .unwrap();
        let rows = log
            .query(&shodh_rag::audit::AuditQuery {
                types: vec![AuditEventType::PhoneLink],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 4);
        // Newest first.
        let removal = &rows[0];
        assert_eq!(removal.principal, LOCAL_OWNER, "the user removed it");
        assert_eq!(removal.payload["command"], "remove");
        for (row, (command, _)) in rows[1..].iter().rev().zip(calls.iter()) {
            assert_eq!(row.event_type, "phone_link");
            assert_eq!(row.payload["device"], "Pixel 8");
            assert_eq!(row.payload["command"], *command);
            assert_eq!(row.principal, format!("phone:{}", caller.node_id));
        }
        assert_eq!(rows[2].payload["outcome"], "refused");
        assert_eq!(rows[2].payload["reason"], "Approve this on your computer");
        assert_eq!(rows[1].payload["outcome"], "failed");
        // The phone's arguments (questions, paths) are never in the entry.
        for row in &rows {
            let keys: Vec<&String> = row.payload.as_object().unwrap().keys().collect();
            assert!(keys
                .iter()
                .all(|k| ["device", "command", "outcome", "reason"].contains(&k.as_str())));
        }
        assert!(log.verify().unwrap().ok);
    }

    #[test]
    fn the_most_phones_fit_one_credential_entry() {
        // The longest names, of characters that take two UTF-16 units each.
        let devices: Vec<PairedDevice> = (0..MAX_DEVICES)
            .map(|i| PairedDevice {
                node_id: format!("{i:0>64}"),
                name: "😀".repeat(MAX_DEVICE_NAME_CHARS),
                paired_at: u64::MAX / 2,
                last_seen: Some(u64::MAX / 2),
            })
            .collect();
        let json = devices_json(&devices).expect("five phones fit");
        let back: Vec<PairedDevice> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, devices);
        let mut too_many = devices.clone();
        too_many.extend(devices.clone());
        assert!(devices_json(&too_many).is_err());
    }

    #[test]
    fn the_node_key_is_kept_and_never_replaced_on_a_read_error() {
        use std::cell::RefCell;
        let stored = RefCell::new(None::<String>);
        let first = node_key_with(
            || Ok(stored.borrow().clone()),
            |v| {
                *stored.borrow_mut() = Some(v.to_string());
                Ok(())
            },
        )
        .unwrap();
        let again = node_key_with(
            || Ok(stored.borrow().clone()),
            |_| panic!("not stored again"),
        )
        .unwrap();
        assert_eq!(first.public(), again.public(), "the same key every start");
        // A credential store that cannot be read is an error, not a new identity.
        assert!(node_key_with(|| Err("locked".to_string()), |_| panic!("no new key")).is_err());
        assert!(node_key_with(|| Ok(Some("zz".into())), |_| panic!("no new key")).is_err());
    }
}
