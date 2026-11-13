//! The phone's side of the link, for the app's window.
//!
//! At start the phone reads its node key from secure storage, binds its iroh
//! endpoint (the same ALPN and version as the desktop, from `shodh-link`) and
//! reconnects to the computer it paired with. The window calls the
//! computer's commands through [`link_invoke`] (only those on the phone
//! link's allowlist; the computer checks them again) and receives the
//! computer's events as [`DESKTOP_EVENT`], and the connection's state as
//! [`STATUS_EVENT`].

use std::ffi::c_char;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use shodh_link::allowlist::allowed;
use shodh_link::client::{ConnectError, LinkClient};
use shodh_link::iroh::Endpoint;
use shodh_link::pairing::PairingOffer;
use shodh_link::phone::{self, SecretVault, DESKTOP_ENTRY, NODE_KEY_ENTRY};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::OnceCell;

use crate::vault::KeystoreVault;

/// The connection's state changed (payload: [`Status`]).
pub const STATUS_EVENT: &str = "link://status";

/// An event of the computer (payload: `{ name, payload }`, the event as the
/// computer's own window receives it).
pub const DESKTOP_EVENT: &str = "link://event";

/// The connection to the computer, as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Status {
    /// Reading the key and starting the link.
    Starting,
    /// This phone has not paired with a computer.
    Unpaired,
    Connecting,
    Connected,
    /// The computer could not be reached, or the connection ended.
    Offline {
        message: String,
    },
    /// The computer said no (this phone was removed, or is not known): pair
    /// again.
    Refused {
        message: String,
    },
    /// The phone and the computer run different Shodh versions.
    Update {
        message: String,
    },
    /// The phone's secure storage or the link could not start.
    Failed {
        message: String,
    },
}

impl From<ConnectError> for Status {
    fn from(error: ConnectError) -> Self {
        match error {
            ConnectError::Refused(message) => Status::Refused { message },
            e @ ConnectError::VersionMismatch { .. } => Status::Update {
                message: e.to_string(),
            },
            e @ ConnectError::Unreachable(_) => Status::Offline {
                message: e.to_string(),
            },
        }
    }
}

/// What the phone is: its secure storage and its endpoint.
struct Identity {
    vault: Arc<dyn SecretVault>,
    endpoint: Endpoint,
}

/// The connection in use, numbered so a connection that ended after it was
/// replaced changes nothing.
struct Current {
    generation: u64,
    client: Arc<LinkClient>,
}

/// Managed state: the phone's link.
pub struct Link {
    device_name: String,
    identity: OnceCell<Identity>,
    current: Mutex<Option<Current>>,
    generation: AtomicU64,
    status: Mutex<Status>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn refused(message: &str) -> Value {
    json!({ "code": "refused", "message": message })
}

fn not_connected() -> Value {
    json!({ "code": "offline", "message": "Your phone is not connected to your computer." })
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("The phone link stopped: {e}"))?
}

impl Link {
    pub fn new(device_name: String) -> Self {
        Self {
            device_name,
            identity: OnceCell::new(),
            current: Mutex::new(None),
            generation: AtomicU64::new(0),
            status: Mutex::new(Status::Starting),
        }
    }

    /// The phone's key from secure storage (a new one on the first start) and
    /// its endpoint. A failure is not kept: the next call tries again.
    async fn identity(&self) -> Result<&Identity, String> {
        self.identity
            .get_or_try_init(|| async {
                let opened = blocking(KeystoreVault::open).await?;
                let vault: Arc<dyn SecretVault> = Arc::new(opened);
                let reader = vault.clone();
                let key = blocking(move || phone::phone_key(reader.as_ref())).await?;
                let endpoint = shodh_link::phone_endpoint(key)
                    .await
                    .map_err(|e| format!("The phone link could not start: {e}"))?;
                tracing::info!("Phone link started: node {}", endpoint.id().fmt_short());
                Ok::<Identity, String>(Identity { vault, endpoint })
            })
            .await
    }

    fn set_status(&self, app: &AppHandle, status: Status) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = status.clone();
        if let Err(e) = app.emit(STATUS_EVENT, &status) {
            tracing::warn!("The window was not told the link's state: {e}");
        }
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Start the link and reconnect to the kept computer, if there is one.
    pub async fn start(&self, app: &AppHandle) {
        if let Err(message) = self.identity().await {
            tracing::error!("The phone link did not start: {message}");
            self.set_status(app, Status::Failed { message });
            return;
        }
        self.reconnect(app).await;
    }

    /// Connect to the kept computer again.
    pub async fn reconnect(&self, app: &AppHandle) -> Status {
        let status = self.try_reconnect(app).await;
        if status != Status::Connected {
            self.set_status(app, status.clone());
        }
        status
    }

    async fn try_reconnect(&self, app: &AppHandle) -> Status {
        let identity = match self.identity().await {
            Ok(identity) => identity,
            Err(message) => return Status::Failed { message },
        };
        let vault = identity.vault.clone();
        let kept = match blocking(move || phone::kept_desktop(vault.as_ref())).await {
            Ok(kept) => kept,
            Err(message) => return Status::Refused { message },
        };
        let Some(desktop) = kept else {
            return Status::Unpaired;
        };
        let addr = match desktop.addr() {
            Ok(addr) => addr,
            Err(message) => return Status::Refused { message },
        };
        self.set_status(app, Status::Connecting);
        match phone::connect_to_kept(&identity.endpoint, addr, &self.device_name).await {
            Ok(client) => {
                self.install(app, client);
                Status::Connected
            }
            Err(e) => {
                tracing::info!("The computer was not reached: {e}");
                e.into()
            }
        }
    }

    /// Pair with the computer of a scanned or pasted code, and keep it.
    pub async fn pair(&self, app: &AppHandle, code: &str) -> Result<(), String> {
        let offer = PairingOffer::from_qr(code).map_err(|e| e.to_string())?;
        let identity = self.identity().await?;
        let client = phone::pair_with_offer(
            &identity.endpoint,
            identity.vault.as_ref(),
            &offer,
            &self.device_name,
            now_ms(),
        )
        .await
        .map_err(|e| e.to_string())?;
        tracing::info!("Paired with the computer {}", offer.node_id.fmt_short());
        self.install(app, client);
        Ok(())
    }

    /// Use `client` from now on: its events go to the window, and its end is
    /// shown.
    fn install(&self, app: &AppHandle, mut client: LinkClient) {
        let events = client.take_events();
        let client = Arc::new(client);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let previous = self
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(Current {
                generation,
                client: client.clone(),
            });
        if let Some(previous) = previous {
            previous.client.close();
        }
        self.set_status(app, Status::Connected);
        tracing::info!("Connected to the computer");
        let Some(mut events) = events else {
            return;
        };
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = events.recv().await {
                let forwarded = json!({ "name": event.name, "payload": event.payload });
                if let Err(e) = app.emit(DESKTOP_EVENT, forwarded) {
                    tracing::warn!("An event of the computer did not reach the window: {e}");
                }
            }
            app.state::<Link>().ended(&app, generation, client.ended());
        });
    }

    /// The connection `generation` ended, with the computer's reason when it
    /// gave one.
    fn ended(&self, app: &AppHandle, generation: u64, reason: Option<String>) {
        {
            let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
            match current.as_ref() {
                Some(c) if c.generation == generation => *current = None,
                _ => return,
            }
        }
        let status = match reason {
            Some(message) => Status::Refused { message },
            None => Status::Offline {
                message: "The connection to your computer closed.".to_string(),
            },
        };
        tracing::info!("The connection to the computer ended: {status:?}");
        self.set_status(app, status);
    }

    fn client(&self) -> Option<Arc<LinkClient>> {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|c| c.client.clone())
    }

    /// Call one of the computer's commands. Only commands on the phone link's
    /// allowlist are sent; the computer checks them again.
    pub async fn invoke(&self, command: &str, args: Value) -> Result<Value, Value> {
        if !allowed(command) {
            return Err(refused("Your phone can't do this. Use your computer."));
        }
        let client = self.client().ok_or_else(not_connected)?;
        client.invoke(command, args).await
    }

    pub async fn subscribe(&self, session_ids: Vec<String>) -> Result<(), Value> {
        let client = self.client().ok_or_else(not_connected)?;
        client.subscribe(session_ids).await
    }

    /// Start over when the secure storage could not be read (for example
    /// after the app's data was restored to another phone, whose Keystore
    /// does not have the key): this phone gets a new key and pairs again as
    /// a new phone. Refused while the link works.
    pub async fn reset(&self, app: &AppHandle) -> Result<(), String> {
        if self.identity.get().is_some() {
            return Err("The phone link works: there is nothing to start over.".to_string());
        }
        blocking(|| {
            let vault = KeystoreVault::open()?;
            vault.remove(DESKTOP_ENTRY)?;
            vault.remove(NODE_KEY_ENTRY)
        })
        .await?;
        tracing::warn!("The phone link's key and paired computer were removed by the user");
        self.start(app).await;
        match self.status() {
            Status::Failed { message } => Err(message),
            _ => Ok(()),
        }
    }

    /// The window shows its first screen: the line CI's launch test waits
    /// for, written once the key was read and the endpoint is bound.
    pub async fn ready(&self, screen: &str) -> Result<(), String> {
        match self.identity().await {
            Ok(identity) => {
                tracing::info!(
                    "Shodh ready: the {screen} screen is shown (phone node {})",
                    identity.endpoint.id().fmt_short()
                );
                Ok(())
            }
            Err(message) => {
                tracing::error!("Shodh not ready: {message}");
                Err(message)
            }
        }
    }
}

/// The phone's model ("Pixel 8"), the name the computer lists it under.
pub fn device_name() -> String {
    let mut value: [c_char; libc::PROP_VALUE_MAX as usize] = [0; libc::PROP_VALUE_MAX as usize];
    // SAFETY: the name is NUL-terminated and the buffer has the
    // PROP_VALUE_MAX bytes the call may write.
    let len =
        unsafe { libc::__system_property_get(c"ro.product.model".as_ptr(), value.as_mut_ptr()) };
    let len = usize::try_from(len).unwrap_or(0).min(value.len());
    let bytes: Vec<u8> = value[..len].iter().map(|&c| c as u8).collect();
    let name = String::from_utf8_lossy(&bytes).trim().to_string();
    if name.is_empty() {
        "Android phone".to_string()
    } else {
        name
    }
}

// ── Commands ───────────────────────────────────────────────────────

#[tauri::command]
pub fn link_status(link: State<'_, Link>) -> Status {
    link.status()
}

#[tauri::command]
pub async fn link_pair(app: AppHandle, code: String, link: State<'_, Link>) -> Result<(), String> {
    link.pair(&app, &code).await
}

#[tauri::command]
pub async fn link_connect(app: AppHandle, link: State<'_, Link>) -> Result<Status, String> {
    Ok(link.reconnect(&app).await)
}

#[tauri::command]
pub async fn link_invoke(
    command: String,
    args: Value,
    link: State<'_, Link>,
) -> Result<Value, Value> {
    link.invoke(&command, args).await
}

#[tauri::command]
pub async fn link_subscribe(session_ids: Vec<String>, link: State<'_, Link>) -> Result<(), Value> {
    link.subscribe(session_ids).await
}

#[tauri::command]
pub async fn link_reset(app: AppHandle, link: State<'_, Link>) -> Result<(), String> {
    link.reset(&app).await
}

#[tauri::command]
pub async fn app_ready(screen: String, link: State<'_, Link>) -> Result<(), String> {
    if !matches!(screen.as_str(), "pair" | "connected" | "offline" | "failed") {
        return Err(format!("{screen} is not a screen of this app"));
    }
    link.ready(&screen).await
}
