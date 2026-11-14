//! The link end to end: a desktop and a phone, two in-process iroh nodes on
//! the loopback interface (no relay, no address lookup, so no internet).
//!
//! The desktop's host is scripted here: CI has no agent runtime, so the
//! events are the shapes the desktop's sessions emit (`agent_event` with
//! `{ sessionId, event }`), sent when the phone's calls reach the host. Every
//! wait is on a message or a channel; timeouts only bound a failure.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, SecretKey};
use serde_json::{json, Value};
use shodh_link::allowlist::{ApprovalKind, APPROVE_ON_COMPUTER};
use shodh_link::bridge::{
    CallRecord, Caller, Link, LinkEvent, LinkHost, Outcome, PendingStep, AGENT_EVENT, NOT_ON_PHONE,
    NOT_PAIRED, REMOVED,
};
use shodh_link::client::{ConnectError, LinkClient};
use shodh_link::messages::{read_frame, write_frame, Request, Response};
use shodh_link::pairing::{
    new_secret, DeviceRegistry, DeviceStore, PairedDevice, PairingOffer, PairingRefusal,
    PendingPairing, PAIRING_TTL_MS,
};
use shodh_link::phone::{
    connect_to_kept, kept_desktop, pair_and_keep, phone_key, PairError, SecretVault,
};
use shodh_link::{ALPN, LINK_VERSION};
use tokio::sync::broadcast;

/// Bounds a wait that should end at once; only a failure ever reaches it.
const BOUND: Duration = Duration::from_secs(30);

async fn within<T>(what: &str, fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(BOUND, fut)
        .await
        .unwrap_or_else(|_| panic!("{what} did not happen"))
}

#[derive(Default)]
struct MemoryStore(Mutex<Vec<PairedDevice>>);

impl DeviceStore for MemoryStore {
    fn load(&self) -> Result<Vec<PairedDevice>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, devices: &[PairedDevice]) -> Result<(), String> {
        *self.0.lock().unwrap() = devices.to_vec();
        Ok(())
    }
}

/// The scripted desktop.
struct Desktop {
    events: broadcast::Sender<LinkEvent>,
    /// Every call that reached the desktop's handlers: (phone, command, args).
    calls: Mutex<Vec<(String, String, Value)>>,
    records: Mutex<Vec<CallRecord>>,
    now: AtomicU64,
}

impl Desktop {
    fn new() -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Self {
            events,
            calls: Mutex::default(),
            records: Mutex::default(),
            now: AtomicU64::new(1_000_000),
        })
    }

    fn emit(&self, session: &str, event: Value) {
        let _ = self.events.send(LinkEvent {
            name: AGENT_EVENT.to_string(),
            payload: json!({ "sessionId": session, "event": event }),
        });
    }

    fn commands(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, c, _)| c.clone())
            .collect()
    }
}

impl LinkHost for Desktop {
    fn invoke(
        &self,
        caller: &Caller,
        command: &str,
        args: Value,
    ) -> impl Future<Output = Result<Value, Value>> + Send {
        self.calls
            .lock()
            .unwrap()
            .push((caller.name.clone(), command.to_string(), args.clone()));
        let result = match command {
            "agent_start" => Ok(json!("session-1")),
            "agent_send" => {
                let session = args["sessionId"].as_str().unwrap_or_default().to_string();
                let text = args["text"].as_str().unwrap_or_default();
                if text.contains("run a command") {
                    self.emit(&session, json!({ "type": "approval_requested", "runId": "run-2", "stepId": "cmd-1", "tool": "run_command", "label": "Run cargo test", "tier": "destructive", "preview": {} }));
                } else {
                    self.emit(&session, json!({ "type": "run_started", "runId": "run-1", "sessionId": session, "model": "test/model", "atMs": 1, "warning": null }));
                    self.emit(&session, json!({ "type": "text_delta", "runId": "run-1", "messageId": "m1", "delta": "Self-RAG retrieves " }));
                    self.emit(&session, json!({ "type": "step_finished", "runId": "run-1", "stepId": "search-1", "ok": true, "summary": "2 passages", "detail": { "citations": [{ "number": 1, "source": "C:/papers/self-rag.pdf", "page": 4 }] }, "durationMs": 5, "invalidArguments": false }));
                    self.emit(&session, json!({ "type": "approval_requested", "runId": "run-1", "stepId": "fetch-1", "tool": "fetch_url", "label": "Fetch arxiv.org", "tier": "write", "preview": { "url": "https://arxiv.org/abs/2310.11511" } }));
                }
                Ok(json!("run-1"))
            }
            "agent_approve" => {
                let session = args["sessionId"].as_str().unwrap_or_default().to_string();
                self.emit(&session, json!({ "type": "step_finished", "runId": "run-1", "stepId": args["stepId"], "ok": true, "summary": "fetched", "detail": null, "durationMs": 3, "invalidArguments": false }));
                self.emit(&session, json!({ "type": "text_delta", "runId": "run-1", "messageId": "m1", "delta": "on demand [1]." }));
                Ok(Value::Null)
            }
            "get_source_file_info" => {
                Ok(json!({ "kind": "pdf", "fileName": "self-rag.pdf", "path": args["filePath"] }))
            }
            "read_source_page" => {
                Ok(json!({ "page": args["page"], "pages": 12, "png": "iVBORw0KGgo=" }))
            }
            other => {
                Err(json!({ "code": "runtime_error", "message": format!("no script for {other}") }))
            }
        };
        async move { result }
    }

    fn approval_kind(&self, _session_id: &str, step: &PendingStep) -> ApprovalKind {
        match step.tool.as_str() {
            "run_command" => ApprovalKind::CodeCommand,
            "edit" | "write" => ApprovalKind::FileEdit,
            _ => ApprovalKind::AskStep,
        }
    }

    fn record(&self, record: CallRecord) {
        self.records.lock().unwrap().push(record);
    }

    fn events(&self) -> broadcast::Receiver<LinkEvent> {
        self.events.subscribe()
    }

    fn link_allowed(&self) -> Result<(), String> {
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

/// An endpoint on the loopback interface only: no relay, no address lookup.
async fn local_endpoint(accepts: bool) -> Endpoint {
    let builder = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .expect("a loopback address");
    let builder = if accepts {
        builder.alpns(vec![ALPN.to_vec()])
    } else {
        builder
    };
    builder.bind().await.expect("the endpoint binds")
}

fn direct_addr(endpoint: &Endpoint) -> EndpointAddr {
    let socket = endpoint.bound_sockets()[0];
    EndpointAddr::new(endpoint.id()).with_ip_addr(socket)
}

struct World {
    desktop: Arc<Desktop>,
    link: Arc<Link<Desktop>>,
    endpoint: Endpoint,
}

impl World {
    async fn new() -> Self {
        let desktop = Desktop::new();
        let devices = Arc::new(DeviceRegistry::open(Box::<MemoryStore>::default()).unwrap());
        let link = Arc::new(Link::new(
            desktop.clone(),
            devices,
            Arc::new(PendingPairing::default()),
        ));
        let endpoint = local_endpoint(true).await;
        tokio::spawn(link.clone().serve(endpoint.clone()));
        Self {
            desktop,
            link,
            endpoint,
        }
    }

    fn offer(&self) -> PairingOffer {
        let secret = new_secret().unwrap();
        let expires_at = self.link.pairing().issue(secret, self.desktop.now_ms());
        PairingOffer {
            node_id: self.endpoint.id(),
            relay_url: None,
            secret,
            expires_at,
        }
    }

    /// The offer as the phone gets it: through the QR string.
    fn scanned(&self) -> PairingOffer {
        PairingOffer::from_qr(&self.offer().to_qr()).unwrap()
    }

    fn addr(&self) -> EndpointAddr {
        direct_addr(&self.endpoint)
    }
}

async fn next_agent_event(client: &mut LinkClient) -> Value {
    let event = within("an event", client.next_event())
        .await
        .expect("the connection is open");
    assert_eq!(event.name, AGENT_EVENT);
    event.payload
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_ask_stream_approve_open_over_the_link() {
    let world = World::new().await;
    let phone = local_endpoint(false).await;
    let offer = world.scanned();

    // Pair.
    let mut client = within(
        "pairing",
        LinkClient::pair(&phone, world.addr(), &offer, "Pixel 8"),
    )
    .await
    .expect("the phone pairs");
    let paired = world.link.devices().list();
    assert_eq!(paired.len(), 1);
    assert_eq!(paired[0].name, "Pixel 8");
    assert_eq!(
        paired[0].node_id,
        phone.id().to_string(),
        "the phone is known by the key its connection proved"
    );

    // Start a session and ask.
    let session = within(
        "agent_start",
        client.invoke("agent_start", json!({ "conversationId": "c1" })),
    )
    .await
    .expect("the session starts");
    assert_eq!(session, json!("session-1"));
    client.subscribe(vec!["session-1".into()]).await.unwrap();
    let run = within(
        "agent_send",
        client.invoke(
            "agent_send",
            json!({ "sessionId": "session-1", "text": "What is Self-RAG?", "requestId": "r1" }),
        ),
    )
    .await
    .expect("the question is sent");
    assert_eq!(run, json!("run-1"));

    // The streamed answer arrives over the link: tokens, a citation and an approval.
    let started = next_agent_event(&mut client).await;
    assert_eq!(started["event"]["type"], "run_started");
    let token = next_agent_event(&mut client).await;
    assert_eq!(token["sessionId"], "session-1");
    assert_eq!(token["event"]["delta"], "Self-RAG retrieves ");
    let cited = next_agent_event(&mut client).await;
    assert_eq!(cited["event"]["detail"]["citations"][0]["page"], 4);
    let asked = next_agent_event(&mut client).await;
    assert_eq!(asked["event"]["type"], "approval_requested");
    assert_eq!(asked["event"]["stepId"], "fetch-1");

    // "Allow for the rest of this conversation" is never granted from a phone.
    let all = within(
        "a for-conversation approval",
        client.invoke(
            "agent_approve",
            json!({ "sessionId": "session-1", "stepId": "fetch-1", "approved": true, "forConversation": true }),
        ),
    )
    .await
    .expect_err("refused");
    assert_eq!(all["message"], APPROVE_ON_COMPUTER);
    assert!(!world
        .desktop
        .commands()
        .contains(&"agent_approve".to_string()));

    // The Ask step is approved from the phone; the answer goes on.
    within(
        "the approval",
        client.invoke(
            "agent_approve",
            json!({ "sessionId": "session-1", "stepId": "fetch-1", "approved": true, "reviewToken": "a-token-the-phone-must-not-pass" }),
        ),
    )
    .await
    .expect("an Ask step may be approved from the phone");
    let approve_args = world
        .desktop
        .calls
        .lock()
        .unwrap()
        .iter()
        .find(|(_, c, _)| c == "agent_approve")
        .map(|(_, _, a)| a.clone())
        .unwrap();
    assert_eq!(approve_args["forConversation"], false);
    assert_eq!(approve_args["reviewToken"], Value::Null);
    let finished = next_agent_event(&mut client).await;
    assert_eq!(finished["event"]["type"], "step_finished");
    let rest = next_agent_event(&mut client).await;
    assert_eq!(rest["event"]["delta"], "on demand [1].");

    // Open the cited document's page.
    let info = within(
        "file info",
        client.invoke(
            "get_source_file_info",
            json!({ "filePath": "C:/papers/self-rag.pdf" }),
        ),
    )
    .await
    .unwrap();
    assert_eq!(info["kind"], "pdf");
    let page = within(
        "a page",
        client.invoke(
            "read_source_page",
            json!({ "filePath": "C:/papers/self-rag.pdf", "page": 4, "width": 1080 }),
        ),
    )
    .await
    .unwrap();
    assert_eq!(page["page"], 4);

    // Disconnect mid-answer and reconnect as the paired phone: the rest of the
    // answer streams to the new connection.
    client.close();
    within("the close", client.closed()).await;
    let mut again = within(
        "reconnecting",
        LinkClient::connect(&phone, world.addr(), "Pixel 8"),
    )
    .await
    .expect("a paired phone reconnects");
    again.subscribe(vec!["session-1".into()]).await.unwrap();
    // The subscription is in place once a call made after it is answered.
    within(
        "a call after subscribing",
        again.invoke(
            "get_source_file_info",
            json!({ "filePath": "C:/papers/self-rag.pdf" }),
        ),
    )
    .await
    .unwrap();
    world.desktop.emit("session-1", json!({ "type": "text_delta", "runId": "run-1", "messageId": "m1", "delta": " It critiques itself." }));
    world.desktop.emit("session-1", json!({ "type": "run_finished", "runId": "run-1", "status": "completed", "durationMs": 40, "error": null, "providerError": null }));
    let tail = next_agent_event(&mut again).await;
    assert_eq!(tail["event"]["delta"], " It critiques itself.");
    let end = next_agent_event(&mut again).await;
    assert_eq!(end["event"]["type"], "run_finished");

    // Every call was recorded with the phone's name.
    let records = world.desktop.records.lock().unwrap().clone();
    assert!(records.iter().all(|r| r.caller.name == "Pixel 8"));
    assert!(records
        .iter()
        .any(|r| r.command == "agent_approve" && r.outcome == Outcome::Done));
    assert!(records.iter().any(|r| r.command == "agent_approve"
        && r.outcome == Outcome::Refused(APPROVE_ON_COMPUTER.to_string())));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_code_approval_is_refused() {
    let world = World::new().await;
    let phone = local_endpoint(false).await;
    let mut client = LinkClient::pair(&phone, world.addr(), &world.scanned(), "Pixel 8")
        .await
        .unwrap();
    client.subscribe(vec!["session-1".into()]).await.unwrap();
    within(
        "agent_send",
        client.invoke(
            "agent_send",
            json!({ "sessionId": "session-1", "text": "run a command", "requestId": "r1" }),
        ),
    )
    .await
    .unwrap();
    let asked = next_agent_event(&mut client).await;
    assert_eq!(asked["event"]["tool"], "run_command");
    let refusal = within(
        "the approval",
        client.invoke(
            "agent_approve",
            json!({ "sessionId": "session-1", "stepId": "cmd-1", "approved": true }),
        ),
    )
    .await
    .expect_err("a Code command is never approved from a phone");
    assert_eq!(refusal["code"], "refused");
    assert_eq!(refusal["message"], APPROVE_ON_COMPUTER);
    // The gate stays pending: the desktop's handler was never called.
    assert!(!world
        .desktop
        .commands()
        .contains(&"agent_approve".to_string()));

    // A step the phone was never sent (another session's, or made up) is refused too.
    let unknown = within(
        "an unknown approval",
        client.invoke(
            "agent_approve",
            json!({ "sessionId": "session-9", "stepId": "x", "approved": true }),
        ),
    )
    .await
    .expect_err("refused");
    assert_eq!(unknown["message"], APPROVE_ON_COMPUTER);
    assert!(!world
        .desktop
        .commands()
        .contains(&"agent_approve".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_outside_the_allowlist_is_refused_on_the_desktop() {
    let world = World::new().await;
    let phone = local_endpoint(false).await;
    let client = LinkClient::pair(&phone, world.addr(), &world.scanned(), "Pixel 8")
        .await
        .unwrap();
    for command in [
        "save_conversation",
        "clear_all_documents",
        "set_app_policy",
        "read_source_bytes",
    ] {
        let refusal = within(command, client.invoke(command, json!({})))
            .await
            .expect_err("refused");
        assert_eq!(refusal["code"], "refused");
        assert_eq!(refusal["message"], NOT_ON_PHONE);
    }
    assert!(
        world.desktop.commands().is_empty(),
        "nothing reached a handler"
    );
    let records = world.desktop.records.lock().unwrap().clone();
    assert_eq!(records[0].command, "pair");
    assert_eq!(records[0].outcome, Outcome::Done);
    let calls = &records[1..];
    assert_eq!(calls.len(), 4, "each refusal is recorded");
    assert!(calls
        .iter()
        .all(|r| r.caller.name == "Pixel 8" && matches!(r.outcome, Outcome::Refused(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_of_unsubscribed_sessions_are_not_sent_over_the_link() {
    let world = World::new().await;
    let phone = local_endpoint(false).await;
    let mut client = LinkClient::pair(&phone, world.addr(), &world.scanned(), "Pixel 8")
        .await
        .unwrap();
    client.subscribe(vec!["mine".into()]).await.unwrap();
    within(
        "a call after subscribing",
        client.invoke("agent_start", json!({})),
    )
    .await
    .unwrap();
    world.desktop.emit(
        "someone-else",
        json!({ "type": "text_delta", "delta": "private" }),
    );
    world
        .desktop
        .emit("mine", json!({ "type": "text_delta", "delta": "public" }));
    // Events arrive in order, so the first one the phone sees is its own.
    let first = next_agent_event(&mut client).await;
    assert_eq!(first["sessionId"], "mine");
    assert_eq!(first["event"]["delta"], "public");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_device_is_refused() {
    let world = World::new().await;
    let phone = local_endpoint(false).await;
    let client = LinkClient::pair(&phone, world.addr(), &world.scanned(), "Pixel 8")
        .await
        .unwrap();
    within("a call", client.invoke("agent_start", json!({})))
        .await
        .unwrap();
    let calls_before = world.desktop.commands().len();

    // Removing it closes its live connection at once.
    assert!(world
        .link
        .devices()
        .remove(&phone.id().to_string())
        .unwrap());
    within("the removal to close the connection", client.closed()).await;
    assert_eq!(client.ended().as_deref(), Some(REMOVED));
    let after = client
        .invoke("agent_start", json!({}))
        .await
        .expect_err("closed");
    assert_eq!(after["code"], "offline");

    // Its next connection, with its old key, is closed before any request.
    let retry = within(
        "the retry",
        LinkClient::connect(&phone, world.addr(), "Pixel 8"),
    )
    .await;
    assert_eq!(
        retry.err(),
        Some(ConnectError::Refused(NOT_PAIRED.to_string()))
    );
    assert_eq!(
        world.desktop.commands().len(),
        calls_before,
        "nothing reached a handler"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_node_is_refused() {
    let world = World::new().await;
    let stranger = local_endpoint(false).await;
    let hello = within(
        "hello",
        LinkClient::connect(&stranger, world.addr(), "Stranger"),
    )
    .await;
    assert_eq!(
        hello.err(),
        Some(ConnectError::Refused(NOT_PAIRED.to_string()))
    );

    // A first frame that skips the hello is refused the same way, and never runs.
    let connection = within("connect", stranger.connect(world.addr(), ALPN))
        .await
        .unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    write_frame(
        &mut send,
        &Request::Invoke {
            command: "agent_start".into(),
            args: json!({}),
            call_id: 1,
        },
    )
    .await
    .unwrap();
    let answer: Response = within("the refusal", read_frame(&mut recv)).await.unwrap();
    assert_eq!(
        answer,
        Response::Refused {
            reason: NOT_PAIRED.to_string()
        }
    );
    assert!(world.desktop.commands().is_empty());
    assert!(world.link.devices().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_or_expired_pairing_code_is_refused_over_the_link() {
    let world = World::new().await;
    let offer = world.scanned();
    let first = local_endpoint(false).await;
    LinkClient::pair(&first, world.addr(), &offer, "Pixel 8")
        .await
        .expect("the first scan pairs");
    // The same code from another phone (a photo of the QR, a replay).
    let thief = local_endpoint(false).await;
    let replay = within(
        "the replay",
        LinkClient::pair(&thief, world.addr(), &offer, "Thief"),
    )
    .await;
    assert_eq!(
        replay.err(),
        Some(ConnectError::Refused(PairingRefusal::Used.to_string()))
    );
    // A code scanned after its five minutes.
    let late = world.scanned();
    world
        .desktop
        .now
        .fetch_add(PAIRING_TTL_MS, Ordering::SeqCst);
    let expired = within(
        "the late scan",
        LinkClient::pair(&thief, world.addr(), &late, "Thief"),
    )
    .await;
    assert_eq!(
        expired.err(),
        Some(ConnectError::Refused(PairingRefusal::Expired.to_string()))
    );
    let paired = world.link.devices().list();
    assert_eq!(paired.len(), 1);
    assert_eq!(paired[0].node_id, first.id().to_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_version_mismatch_says_update() {
    let world = World::new().await;
    let phone = local_endpoint(false).await;
    let offer = world.scanned();
    let connection = within("connect", phone.connect(world.addr(), ALPN))
        .await
        .unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    write_frame(
        &mut send,
        &Request::Pair {
            version: "0.0.1".into(),
            device_name: "Old phone".into(),
            secret: shodh_link::pairing::encode_secret(&offer.secret),
        },
    )
    .await
    .unwrap();
    let answer: Response = within("the answer", read_frame(&mut recv)).await.unwrap();
    assert_eq!(
        answer,
        Response::VersionMismatch {
            desktop: LINK_VERSION.to_string()
        }
    );
    let message = ConnectError::VersionMismatch {
        desktop: LINK_VERSION.to_string(),
    }
    .to_string();
    assert!(
        message.starts_with("Update Shodh on your phone"),
        "{message}"
    );
    // The phone that must update keeps its code: it pairs once it is the same version.
    LinkClient::pair(&phone, world.addr(), &offer, "Updated phone")
        .await
        .expect("the code still works");
}

/// The phone's secure storage, in memory.
#[derive(Default)]
struct MemoryVault(Mutex<std::collections::HashMap<String, Vec<u8>>>);

impl SecretVault for MemoryVault {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
    fn store(&self, name: &str, value: &[u8]) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert(name.to_string(), value.to_vec());
        Ok(())
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

/// A phone's endpoint on the loopback interface with its kept key.
async fn phone_endpoint_with(key: SecretKey) -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .expect("a loopback address")
        .bind()
        .await
        .expect("the endpoint binds")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_keeps_its_desktop_and_reconnects_with_its_kept_key() {
    let world = World::new().await;
    let vault = MemoryVault::default();
    let key = phone_key(&vault).unwrap();
    let phone = phone_endpoint_with(key.clone()).await;

    // A code past its five minutes is refused on the phone, without dialling.
    let mut stale = world.scanned();
    stale.expires_at = world.desktop.now_ms();
    let refused = pair_and_keep(
        &phone,
        &vault,
        &stale,
        world.addr(),
        "Pixel 8",
        world.desktop.now_ms(),
    )
    .await;
    assert_eq!(refused.err(), Some(PairError::Expired));
    assert!(kept_desktop(&vault).unwrap().is_none());
    assert!(world.link.devices().list().is_empty());

    // Pair, and the desktop is kept without the one-time secret.
    let offer = world.scanned();
    let mut client = within(
        "pairing",
        pair_and_keep(
            &phone,
            &vault,
            &offer,
            world.addr(),
            "Pixel 8",
            world.desktop.now_ms(),
        ),
    )
    .await
    .expect("the phone pairs");
    let kept = kept_desktop(&vault).unwrap().expect("the desktop is kept");
    assert_eq!(kept.node_id, world.endpoint.id().to_string());
    assert_eq!(kept.addr().unwrap().id, world.endpoint.id());

    // Events are read on their own task while calls are made from this one.
    let mut events = client.take_events().expect("the stream");
    assert!(client.take_events().is_none(), "the stream is taken once");
    let session = within(
        "agent_start",
        client.invoke("agent_start", json!({ "conversationId": "conv-1" })),
    )
    .await
    .unwrap();
    client
        .subscribe(vec![session.as_str().unwrap().to_string()])
        .await
        .unwrap();
    // The reader hands the stream back: a dropped stream would end the
    // connection, as an app that stopped reading does.
    let reader = tokio::spawn(async move {
        let first = events.recv().await;
        (first, events)
    });
    within(
        "agent_send",
        client.invoke(
            "agent_send",
            json!({ "sessionId": session, "text": "What does Self-RAG retrieve?", "requestId": "r1" }),
        ),
    )
    .await
    .unwrap();
    let (first, _events) = within("the first event", reader).await.unwrap();
    let first = first.expect("an event");
    assert_eq!(first.payload["event"]["type"], "run_started");
    assert!(client.next_event().await.is_none(), "the stream was taken");

    // The app restarts: the same key from secure storage, the kept desktop.
    client.close();
    within("the close", client.closed()).await;
    phone.close().await;
    let again = phone_key(&vault).unwrap();
    assert_eq!(again.public(), key.public());
    let restarted = phone_endpoint_with(again).await;
    let back = within(
        "reconnecting",
        connect_to_kept(&restarted, world.addr(), "Pixel 8"),
    )
    .await
    .expect("a kept phone is let back in");
    within(
        "agent_start after the restart",
        back.invoke("agent_start", json!({ "conversationId": "conv-1" })),
    )
    .await
    .expect("its calls reach the desktop");
    assert_eq!(world.link.devices().list().len(), 1, "paired once");

    // A phone with another key is not let in with the kept desktop alone.
    let stranger = local_endpoint(false).await;
    let refused = within(
        "a stranger",
        connect_to_kept(&stranger, world.addr(), "Pixel 8"),
    )
    .await
    .err();
    assert!(
        matches!(refused, Some(ConnectError::Refused(_))),
        "{refused:?}"
    );
}
