//! The phone's side of the link: pair with the desktop, or connect as a
//! paired phone, then call the desktop's commands and receive its events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use iroh::endpoint::{Connection, RecvStream, VarInt};
use iroh::{Endpoint, EndpointAddr};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::bridge::{LinkEvent, ALPN};
use crate::messages::{read_frame, write_frame, Request, Response, LINK_VERSION};
use crate::pairing::{encode_secret, PairingOffer};

/// Events buffered on the phone before the reader waits for the app.
const EVENT_BUFFER: usize = 1024;

/// Why the phone could not connect.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    /// The desktop refused, with its reason in plain words.
    #[error("{0}")]
    Refused(String),
    /// The desktop runs another Shodh version.
    #[error("Update Shodh on your phone: your computer has version {desktop}.")]
    VersionMismatch { desktop: String },
    /// The connection could not be made or broke.
    #[error("Your computer could not be reached: {0}")]
    Unreachable(String),
}

/// Calls waiting for their result; `closed` once the connection ended, so no
/// call waits for an answer that cannot come.
#[derive(Default)]
struct CallTable {
    waiting: HashMap<u64, oneshot::Sender<Result<Value, Value>>>,
    closed: bool,
}

type Calls = Arc<Mutex<CallTable>>;

/// A connection to the desktop.
pub struct LinkClient {
    connection: Connection,
    requests: mpsc::Sender<Request>,
    calls: Calls,
    events: mpsc::Receiver<LinkEvent>,
    next_call: AtomicU64,
    /// Why the desktop ended the connection, once it did.
    ended: Arc<Mutex<Option<String>>>,
}

/// The desktop's address from a scanned offer: its node id and relay.
pub fn offer_addr(offer: &PairingOffer) -> EndpointAddr {
    let addr = EndpointAddr::new(offer.node_id);
    match &offer.relay_url {
        Some(relay) => addr.with_relay_url(relay.clone()),
        None => addr,
    }
}

impl LinkClient {
    /// Pair with the desktop at `desktop` with the secret of `offer`.
    pub async fn pair(
        endpoint: &Endpoint,
        desktop: impl Into<EndpointAddr>,
        offer: &PairingOffer,
        device_name: &str,
    ) -> Result<Self, ConnectError> {
        let first = Request::Pair {
            version: LINK_VERSION.to_string(),
            device_name: device_name.to_string(),
            secret: encode_secret(&offer.secret),
        };
        Self::open(endpoint, desktop.into(), first).await
    }

    /// Connect as a paired phone.
    pub async fn connect(
        endpoint: &Endpoint,
        desktop: impl Into<EndpointAddr>,
        device_name: &str,
    ) -> Result<Self, ConnectError> {
        let first = Request::Hello {
            version: LINK_VERSION.to_string(),
            device_name: device_name.to_string(),
        };
        Self::open(endpoint, desktop.into(), first).await
    }

    async fn open(
        endpoint: &Endpoint,
        desktop: EndpointAddr,
        first: Request,
    ) -> Result<Self, ConnectError> {
        let unreachable = |e: String| ConnectError::Unreachable(e);
        let connection = endpoint
            .connect(desktop, ALPN)
            .await
            .map_err(|e| unreachable(e.to_string()))?;
        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|e| unreachable(e.to_string()))?;
        write_frame(&mut send, &first)
            .await
            .map_err(|e| unreachable(e.to_string()))?;
        let answer: Response = read_frame(&mut recv)
            .await
            .map_err(|e| unreachable(e.to_string()))?;
        match answer {
            Response::Welcome { .. } => {}
            Response::VersionMismatch { desktop } => {
                connection.close(VarInt::from_u32(0), b"version");
                return Err(ConnectError::VersionMismatch { desktop });
            }
            Response::Refused { reason } => {
                connection.close(VarInt::from_u32(0), b"refused");
                return Err(ConnectError::Refused(reason));
            }
            Response::Result { .. } | Response::Event { .. } => {
                connection.close(VarInt::from_u32(2), b"protocol");
                return Err(unreachable("the computer answered out of turn".to_string()));
            }
        }
        let (requests, mut outbox) = mpsc::channel::<Request>(64);
        tokio::spawn(async move {
            while let Some(request) = outbox.recv().await {
                if write_frame(&mut send, &request).await.is_err() {
                    return;
                }
            }
            let _ = send.finish();
        });
        let calls: Calls = Arc::default();
        let ended = Arc::new(Mutex::new(None));
        let (events_tx, events) = mpsc::channel(EVENT_BUFFER);
        tokio::spawn(read_loop(
            connection.clone(),
            recv,
            calls.clone(),
            events_tx,
            ended.clone(),
        ));
        Ok(Self {
            connection,
            requests,
            calls,
            events,
            next_call: AtomicU64::new(1),
            ended,
        })
    }

    /// Call the desktop's `command`. A broken connection is
    /// `Err({"code": "offline", …})`.
    pub async fn invoke(&self, command: &str, args: Value) -> Result<Value, Value> {
        let call_id = self.next_call.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
            if calls.closed {
                return Err(offline(self.ended()));
            }
            calls.waiting.insert(call_id, tx);
        }
        let request = Request::Invoke {
            command: command.to_string(),
            args,
            call_id,
        };
        if self.requests.send(request).await.is_err() {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .waiting
                .remove(&call_id);
            return Err(offline(self.ended()));
        }
        rx.await.unwrap_or_else(|_| Err(offline(self.ended())))
    }

    /// Receive the events of these sessions only.
    pub async fn subscribe(&self, session_ids: Vec<String>) -> Result<(), Value> {
        self.requests
            .send(Request::Subscribe { session_ids })
            .await
            .map_err(|_| offline(self.ended()))
    }

    /// The next event; `None` once the connection ended.
    pub async fn next_event(&mut self) -> Option<LinkEvent> {
        self.events.recv().await
    }

    /// Why the desktop ended the connection (removed, refused), once it did.
    pub fn ended(&self) -> Option<String> {
        self.ended.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Wait until the connection is closed, by either side.
    pub async fn closed(&self) {
        self.connection.closed().await;
    }

    /// Close the connection.
    pub fn close(&self) {
        self.connection.close(VarInt::from_u32(0), b"bye");
    }
}

fn offline(reason: Option<String>) -> Value {
    json!({
        "code": "offline",
        "message": reason.unwrap_or_else(|| "The connection to your computer closed.".to_string()),
    })
}

async fn read_loop(
    connection: Connection,
    mut recv: RecvStream,
    calls: Calls,
    events: mpsc::Sender<LinkEvent>,
    ended: Arc<Mutex<Option<String>>>,
) {
    // The desktop's last word (a refusal) is read before the connection closes:
    // the phone closes it, once it has read it.
    let mut last_word = false;
    while !last_word {
        let Ok(response) = read_frame::<_, Response>(&mut recv).await else {
            break;
        };
        match response {
            Response::Result { call_id, value } => {
                let waiting = calls
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .waiting
                    .remove(&call_id);
                if let Some(waiting) = waiting {
                    let _ = waiting.send(value);
                }
            }
            Response::Event { name, payload } => {
                if events.send(LinkEvent { name, payload }).await.is_err() {
                    break;
                }
            }
            Response::Refused { reason } => {
                *ended.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
                last_word = true;
            }
            Response::VersionMismatch { desktop } => {
                *ended.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(ConnectError::VersionMismatch { desktop }.to_string());
                last_word = true;
            }
            Response::Welcome { .. } => {}
        }
    }
    // Calls still waiting will not be answered.
    let reason = ended.lock().unwrap_or_else(|e| e.into_inner()).clone();
    {
        let mut calls = calls.lock().unwrap_or_else(|e| e.into_inner());
        calls.closed = true;
        for (_, waiting) in calls.waiting.drain() {
            let _ = waiting.send(Err(offline(reason.clone())));
        }
    }
    connection.close(VarInt::from_u32(0), b"done");
}
