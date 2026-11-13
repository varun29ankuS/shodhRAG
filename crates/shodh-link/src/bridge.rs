//! The desktop's side of the link: who may connect, and what a connected
//! phone's requests reach.
//!
//! Every connection is authenticated by iroh (the phone's node id is its key).
//! Before any request, the desktop decides:
//! - a paired phone ([`DeviceRegistry`]) says `Hello` and is let in;
//! - an unpaired one may only say `Pair` with the current one-time secret
//!   ([`PendingPairing`]); anything else is refused and the connection closed.
//!
//! Then each request is checked here, on the desktop:
//! - a command not on [`allowlist::ALLOWED`] is refused;
//! - an approval is granted only for a step this connection was sent and
//!   only for the kinds [`allowlist::remote_may_approve`] allows (the gate
//!   stays pending otherwise, for the computer to answer);
//! - everything else goes to the [`LinkHost`], which runs the desktop's own
//!   command handlers with the desktop's own rules;
//! - every call is recorded ([`LinkHost::record`]) with the phone's name.
//!
//! Events: only `agent_event`s of the sessions the phone subscribed to are
//! sent. A phone that falls behind is disconnected (it reconnects and reloads)
//! instead of making the desktop buffer without bound. A removed phone's live
//! connections are closed at once.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};
use iroh::Endpoint;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, Semaphore};

use crate::allowlist::{self, ApprovalKind, APPROVE_ON_COMPUTER};
use crate::messages::{read_frame, write_frame, FrameError, Request, Response, LINK_VERSION};
use crate::pairing::{
    clean_device_name, decode_secret, AddDeviceError, DeviceRegistry, PairingRefusal,
    PendingPairing,
};

/// The link's ALPN: a connection for any other protocol is not accepted.
pub const ALPN: &[u8] = b"shodh/link/1";

/// The desktop event the bridge forwards.
pub const AGENT_EVENT: &str = "agent_event";

/// How long a new connection has to say who it is.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Connections still saying who they are, at once. More are closed straight
/// away, so unknown nodes cannot hold the desktop's attention.
pub const MAX_HANDSHAKES: usize = 8;

/// Frames queued for one phone. Events beyond it disconnect the phone.
pub const OUTBOX_FRAMES: usize = 512;

/// Approval steps remembered per connection (the ones it was sent).
const MAX_PENDING_STEPS: usize = 256;

/// How long a refusal waits to reach the phone before the connection closes.
const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// QUIC close codes.
const CLOSE_DONE: u32 = 0;
const CLOSE_REFUSED: u32 = 1;
const CLOSE_PROTOCOL: u32 = 2;

/// An event of the desktop, as its window receives it.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkEvent {
    pub name: String,
    pub payload: Value,
}

/// The paired phone a request came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// Its node id (hex), as the connection authenticated it.
    pub node_id: String,
    /// Its name as paired ("Pixel 8").
    pub name: String,
}

/// An approval step the phone was sent (`approval_requested`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingStep {
    pub tool: String,
    pub tier: String,
}

/// How a remote call ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Refused by the desktop's rules for phones, with the reason shown.
    Refused(String),
    /// The command ran and failed.
    Failed,
}

/// One remote call, for the desktop's Activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRecord {
    pub caller: Caller,
    pub command: String,
    pub outcome: Outcome,
}

/// The desktop the bridge serves: its command handlers, its events, its
/// Activity and its clock.
pub trait LinkHost: Send + Sync + 'static {
    /// Run the allowlisted `command` for `caller` with the desktop's own
    /// handler and rules. A refusal by those rules is
    /// `Err({"code": "refused", "message": …})` ([`refused`]).
    fn invoke(
        &self,
        caller: &Caller,
        command: &str,
        args: Value,
    ) -> impl Future<Output = Result<Value, Value>> + Send;

    /// What granting `step` of session `session_id` would allow.
    fn approval_kind(&self, session_id: &str, step: &PendingStep) -> ApprovalKind;

    /// Record a remote call in Activity.
    fn record(&self, record: CallRecord);

    /// The desktop's events from now on.
    fn events(&self) -> broadcast::Receiver<LinkEvent>;

    /// `Err(reason)` while phones may not connect (Local-only mode).
    fn link_allowed(&self) -> Result<(), String>;

    /// Unix time in milliseconds.
    fn now_ms(&self) -> u64;
}

/// The error value of a refusal, as the phone receives it.
pub fn refused(message: impl Into<String>) -> Value {
    json!({ "code": "refused", "message": message.into() })
}

fn refusal_message(error: &Value) -> Option<String> {
    (error.get("code").and_then(Value::as_str) == Some("refused")).then(|| {
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Refused")
            .to_string()
    })
}

/// The bridge: the paired phones, the current pairing offer and the host.
pub struct Link<H: LinkHost> {
    host: Arc<H>,
    devices: Arc<DeviceRegistry>,
    pairing: Arc<PendingPairing>,
    handshakes: Semaphore,
}

/// What the writer sends.
enum Outgoing {
    Frame(Response),
    /// The last frame: sent, then the connection closes.
    Last(Response, u32),
    /// Close without a frame.
    Close(u32, &'static str),
}

impl<H: LinkHost> Link<H> {
    pub fn new(host: Arc<H>, devices: Arc<DeviceRegistry>, pairing: Arc<PendingPairing>) -> Self {
        Self {
            host,
            devices,
            pairing,
            handshakes: Semaphore::new(MAX_HANDSHAKES),
        }
    }

    pub fn devices(&self) -> &Arc<DeviceRegistry> {
        &self.devices
    }

    pub fn pairing(&self) -> &Arc<PendingPairing> {
        &self.pairing
    }

    /// Accept connections on `endpoint` until it closes.
    pub async fn serve(self: Arc<Self>, endpoint: Endpoint) {
        while let Some(incoming) = endpoint.accept().await {
            let link = self.clone();
            tokio::spawn(async move {
                match incoming.await {
                    Ok(connection) => link.handle(connection).await,
                    Err(e) => {
                        tracing::debug!(target: "shodh::link", error = %e, "an incoming connection failed")
                    }
                }
            });
        }
    }

    /// Serve one connection until it ends.
    pub async fn handle(self: Arc<Self>, connection: Connection) {
        let node_id = connection.remote_id().to_string();
        let Ok(permit) = self.handshakes.try_acquire() else {
            connection.close(VarInt::from_u32(CLOSE_REFUSED), b"busy");
            return;
        };
        let opened = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            let (send, mut recv) = connection.accept_bi().await.map_err(|e| e.to_string())?;
            let first: Request = read_frame(&mut recv).await.map_err(|e| e.to_string())?;
            Ok::<_, String>((send, recv, first))
        })
        .await;
        let (mut send, recv, first) = match opened {
            Ok(Ok(opened)) => opened,
            Ok(Err(e)) => {
                tracing::debug!(target: "shodh::link", error = %e, "a connection ended before it said who it is");
                connection.close(VarInt::from_u32(CLOSE_PROTOCOL), b"no hello");
                return;
            }
            Err(_) => {
                connection.close(VarInt::from_u32(CLOSE_PROTOCOL), b"too slow");
                return;
            }
        };
        // Pairings, and refused connections, are recorded with the name the phone gave.
        let attempt = match &first {
            Request::Pair { device_name, .. } => Some(("pair", device_name.clone())),
            Request::Hello { device_name, .. } => Some(("connect", device_name.clone())),
            Request::Invoke { .. } | Request::Subscribe { .. } => None,
        };
        let admitted = self.admit(&node_id, first);
        if let Some((command, claimed)) = attempt {
            let outcome = match &admitted {
                Ok(_) if command == "pair" => Some(Outcome::Done),
                Ok(_) => None,
                Err(Response::Refused { reason }) => Some(Outcome::Refused(reason.clone())),
                Err(Response::VersionMismatch { desktop }) => Some(Outcome::Refused(format!(
                    "The phone runs another Shodh version than this computer ({desktop})."
                ))),
                Err(_) => Some(Outcome::Failed),
            };
            if let Some(outcome) = outcome {
                let name = match &admitted {
                    Ok(caller) => caller.name.clone(),
                    Err(_) => clean_device_name(&claimed),
                };
                self.host.record(CallRecord {
                    caller: Caller {
                        node_id: node_id.clone(),
                        name,
                    },
                    command: command.to_string(),
                    outcome,
                });
            }
        }
        let caller = match admitted {
            Ok(caller) => caller,
            Err(response) => {
                finish_with(&connection, &mut send, &response, CLOSE_REFUSED).await;
                return;
            }
        };
        drop(permit);
        self.session(connection, send, recv, caller).await;
    }

    /// Who the first frame says the phone is, and whether it may stay.
    fn admit(&self, node_id: &str, first: Request) -> Result<Caller, Response> {
        if let Err(reason) = self.host.link_allowed() {
            return Err(Response::Refused { reason });
        }
        let now = self.host.now_ms();
        match first {
            Request::Hello { version, .. } => {
                if version != LINK_VERSION {
                    return Err(Response::VersionMismatch {
                        desktop: LINK_VERSION.to_string(),
                    });
                }
                let Some(device) = self.devices.get(node_id) else {
                    return Err(Response::Refused {
                        reason: NOT_PAIRED.to_string(),
                    });
                };
                self.devices.touch(node_id, now);
                Ok(Caller {
                    node_id: node_id.to_string(),
                    name: device.name,
                })
            }
            Request::Pair {
                version,
                device_name,
                secret,
            } => {
                // The version first: a phone that must update keeps its code.
                if version != LINK_VERSION {
                    return Err(Response::VersionMismatch {
                        desktop: LINK_VERSION.to_string(),
                    });
                }
                let refuse = |reason: String| Response::Refused { reason };
                let secret = decode_secret(&secret)
                    .ok_or_else(|| refuse(PairingRefusal::Unknown.to_string()))?;
                if !self.devices.has_room_for(node_id) {
                    return Err(refuse(AddDeviceError::Full.to_string()));
                }
                self.pairing
                    .redeem(&secret, now)
                    .map_err(|r| refuse(r.to_string()))?;
                let device = self
                    .devices
                    .add(node_id, &device_name, now)
                    .map_err(|e| refuse(e.to_string()))?;
                tracing::info!(target: "shodh::link", phone = %device.name, "a phone was paired");
                Ok(Caller {
                    node_id: node_id.to_string(),
                    name: device.name,
                })
            }
            Request::Invoke { .. } | Request::Subscribe { .. } => Err(Response::Refused {
                reason: NOT_PAIRED.to_string(),
            }),
        }
    }

    async fn session(
        &self,
        connection: Connection,
        send: SendStream,
        recv: RecvStream,
        caller: Caller,
    ) {
        let (out, outbox) = mpsc::channel::<Outgoing>(OUTBOX_FRAMES);
        let writer = tokio::spawn(write_loop(connection.clone(), send, outbox));
        let (requests_tx, mut requests) = mpsc::channel::<Result<Request, FrameError>>(16);
        let reader = tokio::spawn(read_loop(recv, requests_tx));
        let mut events = self.host.events();
        let mut revoked = self.devices.revocations();
        let mut subscribed: HashSet<String> = HashSet::new();
        let mut pending: HashMap<(String, String), PendingStep> = HashMap::new();

        let welcome = Response::Welcome {
            desktop: LINK_VERSION.to_string(),
        };
        if out.send(Outgoing::Frame(welcome)).await.is_err() {
            reader.abort();
            return;
        }
        // The device may have been removed between its hello and now.
        if self.devices.get(&caller.node_id).is_none() {
            let _ = out.send(removed()).await;
        } else {
            loop {
                tokio::select! {
                    request = requests.recv() => {
                        let Some(request) = request else { break };
                        let request = match request {
                            Ok(request) => request,
                            Err(FrameError::Closed) => break,
                            Err(e) => {
                                tracing::debug!(target: "shodh::link", error = %e, "a phone sent a frame that is not valid");
                                let _ = out.send(Outgoing::Close(CLOSE_PROTOCOL, "bad frame")).await;
                                break;
                            }
                        };
                        // Belt and braces: the registry is the allowlist of phones.
                        if self.devices.get(&caller.node_id).is_none() {
                            let _ = out.send(removed()).await;
                            break;
                        }
                        match request {
                            Request::Subscribe { session_ids } => {
                                subscribed = session_ids.into_iter().collect();
                                pending.retain(|(session, _), _| subscribed.contains(session));
                            }
                            Request::Invoke { command, args, call_id } => {
                                match self.check(&command, args, &mut pending) {
                                    Ok(args) => self.run(&caller, command, args, call_id, &out),
                                    Err(reason) => {
                                        self.host.record(CallRecord {
                                            caller: caller.clone(),
                                            command,
                                            outcome: Outcome::Refused(reason.to_string()),
                                        });
                                        let refusal = Response::Result { call_id, value: Err(refused(reason)) };
                                        if out.send(Outgoing::Frame(refusal)).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            Request::Hello { .. } | Request::Pair { .. } => {
                                let _ = out.send(Outgoing::Close(CLOSE_PROTOCOL, "already said hello")).await;
                                break;
                            }
                        }
                    }
                    event = events.recv() => match event {
                        Ok(event) => {
                            if let Some(frame) = forward(&event, &subscribed, &mut pending) {
                                if out.try_send(Outgoing::Frame(frame)).is_err() {
                                    let _ = out.try_send(Outgoing::Close(CLOSE_DONE, "fell behind"));
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let _ = out.try_send(Outgoing::Close(CLOSE_DONE, "fell behind"));
                            break;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            let _ = out.send(Outgoing::Close(CLOSE_DONE, "desktop closing")).await;
                            break;
                        }
                    },
                    gone = revoked.recv() => match gone {
                        Ok(id) if id == caller.node_id => {
                            let _ = out.send(removed()).await;
                            break;
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                            if self.devices.get(&caller.node_id).is_none() {
                                let _ = out.send(removed()).await;
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                }
            }
        }
        reader.abort();
        drop(out);
        // Calls still running hold senders; the writer ends with the last of them
        // or with the connection.
        let _ = writer.await;
    }

    /// The bridge's own checks of one call: the allowlist, and for an
    /// approval, that this phone was sent the step and may grant its kind.
    /// Returns the arguments to run it with, or why it is refused.
    fn check(
        &self,
        command: &str,
        args: Value,
        pending: &mut HashMap<(String, String), PendingStep>,
    ) -> Result<Value, &'static str> {
        if !allowlist::allowed(command) {
            return Err(NOT_ON_PHONE);
        }
        if command != "agent_approve" {
            return Ok(args);
        }
        let answer = approval(&args).ok_or("This approval is not valid.")?;
        let key = (answer.session_id.clone(), answer.step_id.clone());
        // Only a step this phone was sent: anything else waits for the computer.
        let step = pending.get(&key).ok_or(APPROVE_ON_COMPUTER)?;
        if answer.approved {
            let kind = if answer.for_conversation {
                ApprovalKind::AllowForConversation
            } else {
                self.host.approval_kind(&answer.session_id, step)
            };
            if !allowlist::remote_may_approve(&kind) {
                // The gate stays pending: nothing reaches the session.
                return Err(APPROVE_ON_COMPUTER);
            }
        }
        pending.remove(&key);
        // Never "for the rest of the conversation", never a review token.
        Ok(json!({
            "sessionId": answer.session_id,
            "stepId": answer.step_id,
            "approved": answer.approved,
            "forConversation": false,
            "reviewToken": Value::Null,
        }))
    }

    /// Run one checked call in its own task, so a long answer never holds up
    /// events or other calls.
    fn run(
        &self,
        caller: &Caller,
        command: String,
        args: Value,
        call_id: u64,
        out: &mpsc::Sender<Outgoing>,
    ) {
        let host = self.host.clone();
        let caller = caller.clone();
        let out = out.clone();
        tokio::spawn(async move {
            let value = host.invoke(&caller, &command, args).await;
            let outcome = match &value {
                Ok(_) => Outcome::Done,
                Err(error) => refusal_message(error).map_or(Outcome::Failed, Outcome::Refused),
            };
            host.record(CallRecord {
                caller,
                command,
                outcome,
            });
            let _ = out
                .send(Outgoing::Frame(Response::Result { call_id, value }))
                .await;
        });
    }
}

/// What the phone is told when it is not (or no longer) paired.
pub const NOT_PAIRED: &str =
    "This phone is not paired with your computer. Pair it again in Shodh on your computer: Settings, Phone.";

/// What the phone is told for a command it may not call.
pub const NOT_ON_PHONE: &str = "Your phone cannot do this. Use Shodh on your computer.";

/// What a removed phone is told before its connection closes.
pub const REMOVED: &str = "This phone was removed from Shodh on your computer.";

fn removed() -> Outgoing {
    Outgoing::Last(
        Response::Refused {
            reason: REMOVED.to_string(),
        },
        CLOSE_REFUSED,
    )
}

/// An approval answer from the phone.
struct Answer {
    session_id: String,
    step_id: String,
    approved: bool,
    for_conversation: bool,
}

fn approval(args: &Value) -> Option<Answer> {
    Some(Answer {
        session_id: args.get("sessionId")?.as_str()?.to_string(),
        step_id: args.get("stepId")?.as_str()?.to_string(),
        approved: args.get("approved")?.as_bool()?,
        for_conversation: args
            .get("forConversation")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// The frame for `event`, when this phone subscribed to its session; the
/// approval steps it carries are remembered so the phone can answer them.
fn forward(
    event: &LinkEvent,
    subscribed: &HashSet<String>,
    pending: &mut HashMap<(String, String), PendingStep>,
) -> Option<Response> {
    if event.name != AGENT_EVENT {
        return None;
    }
    let session = event.payload.get("sessionId")?.as_str()?;
    if !subscribed.contains(session) {
        return None;
    }
    let inner = event.payload.get("event");
    let kind = inner.and_then(|e| e.get("type")).and_then(Value::as_str);
    let step = inner.and_then(|e| e.get("stepId")).and_then(Value::as_str);
    match (kind, step) {
        (Some("approval_requested"), Some(step)) if pending.len() < MAX_PENDING_STEPS => {
            let field = |name: &str| {
                inner
                    .and_then(|e| e.get(name))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            pending.insert(
                (session.to_string(), step.to_string()),
                PendingStep {
                    tool: field("tool"),
                    tier: field("tier"),
                },
            );
        }
        (Some("step_finished"), Some(step)) => {
            pending.remove(&(session.to_string(), step.to_string()));
        }
        _ => {}
    }
    Some(Response::Event {
        name: event.name.clone(),
        payload: event.payload.clone(),
    })
}

async fn read_loop(mut recv: RecvStream, requests: mpsc::Sender<Result<Request, FrameError>>) {
    loop {
        let frame = read_frame::<_, Request>(&mut recv).await;
        let end = frame.is_err();
        if requests.send(frame).await.is_err() || end {
            return;
        }
    }
}

async fn write_loop(
    connection: Connection,
    mut send: SendStream,
    mut outbox: mpsc::Receiver<Outgoing>,
) {
    while let Some(next) = outbox.recv().await {
        match next {
            Outgoing::Frame(response) => {
                if let Err(e) = write_response(&mut send, &response).await {
                    tracing::debug!(target: "shodh::link", error = %e, "a phone's connection failed");
                    connection.close(VarInt::from_u32(CLOSE_DONE), b"write failed");
                    return;
                }
            }
            Outgoing::Last(response, code) => {
                finish_with(&connection, &mut send, &response, code).await;
                return;
            }
            Outgoing::Close(code, reason) => {
                connection.close(VarInt::from_u32(code), reason.as_bytes());
                return;
            }
        }
    }
    let _ = send.finish();
    connection.close(VarInt::from_u32(CLOSE_DONE), b"bye");
}

/// Write `response`; a result too large for one frame is sent as an error
/// result instead, so the phone hears why and the connection stays.
async fn write_response(send: &mut SendStream, response: &Response) -> Result<(), FrameError> {
    match write_frame(send, response).await {
        Err(FrameError::TooLarge(size)) => {
            let Response::Result { call_id, .. } = response else {
                return Err(FrameError::TooLarge(size));
            };
            let error = Response::Result {
                call_id: *call_id,
                value: Err(json!({
                    "code": "too_large",
                    "message": "This is too large to send to your phone. Open it on your computer.",
                })),
            };
            write_frame(send, &error).await
        }
        other => other,
    }
}

/// Send `response` and finish the stream; the phone closes the connection
/// once it has read it (closing first would discard what it has not read
/// yet). A phone that does not is closed after [`CLOSE_GRACE`].
async fn finish_with(
    connection: &Connection,
    send: &mut SendStream,
    response: &Response,
    code: u32,
) {
    if write_frame(send, response).await.is_ok() && send.finish().is_ok() {
        let _ = tokio::time::timeout(CLOSE_GRACE, connection.closed()).await;
    }
    connection.close(VarInt::from_u32(code), b"closed");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_event(session: &str, event: Value) -> LinkEvent {
        LinkEvent {
            name: AGENT_EVENT.to_string(),
            payload: json!({ "sessionId": session, "event": event }),
        }
    }

    #[test]
    fn events_of_unsubscribed_sessions_are_not_sent() {
        let subscribed: HashSet<String> = ["s1".to_string()].into_iter().collect();
        let mut pending = HashMap::new();
        let mine = agent_event("s1", json!({ "type": "text_delta", "delta": "Self-RAG" }));
        let other = agent_event("s2", json!({ "type": "text_delta", "delta": "private" }));
        let other_approval = agent_event(
            "s2",
            json!({ "type": "approval_requested", "stepId": "x", "tool": "fetch_url", "tier": "write" }),
        );
        let not_agent = LinkEvent {
            name: "settings_changed".to_string(),
            payload: json!({ "sessionId": "s1" }),
        };
        assert!(forward(&mine, &subscribed, &mut pending).is_some());
        assert!(forward(&other, &subscribed, &mut pending).is_none());
        assert!(forward(&other_approval, &subscribed, &mut pending).is_none());
        assert!(
            pending.is_empty(),
            "an approval of another session is never answerable"
        );
        assert!(forward(&not_agent, &subscribed, &mut pending).is_none());
        assert!(forward(&mine, &HashSet::new(), &mut pending).is_none());
    }

    #[test]
    fn forwarded_approvals_are_remembered_until_their_step_finishes() {
        let subscribed: HashSet<String> = ["s1".to_string()].into_iter().collect();
        let mut pending = HashMap::new();
        let asked = agent_event(
            "s1",
            json!({ "type": "approval_requested", "stepId": "t1", "tool": "fetch_url", "tier": "write" }),
        );
        forward(&asked, &subscribed, &mut pending).unwrap();
        assert_eq!(
            pending.get(&("s1".to_string(), "t1".to_string())),
            Some(&PendingStep {
                tool: "fetch_url".into(),
                tier: "write".into()
            })
        );
        let done = agent_event("s1", json!({ "type": "step_finished", "stepId": "t1" }));
        forward(&done, &subscribed, &mut pending).unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn an_approval_answer_is_read_strictly() {
        let ok = approval(&json!({ "sessionId": "s", "stepId": "t", "approved": true })).unwrap();
        assert!(ok.approved && !ok.for_conversation);
        let all = approval(
            &json!({ "sessionId": "s", "stepId": "t", "approved": true, "forConversation": true }),
        )
        .unwrap();
        assert!(all.for_conversation);
        assert!(approval(&json!({ "sessionId": "s", "stepId": "t", "approved": "yes" })).is_none());
        assert!(approval(&json!({ "stepId": "t", "approved": true })).is_none());
    }

    #[test]
    fn a_refusal_is_told_apart_from_a_failure() {
        assert_eq!(refusal_message(&refused("no")), Some("no".to_string()));
        assert_eq!(
            refusal_message(&json!({ "code": "runtime_error", "message": "x" })),
            None
        );
        assert_eq!(refusal_message(&json!("plain error")), None);
    }
}
