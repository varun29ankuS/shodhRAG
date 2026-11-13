//! The phone link end to end with the desktop's own saving: a desktop and a phone, two
//! in-process iroh nodes on the loopback interface (no relay, no address lookup, so no
//! internet), the desktop's conversation store and turn recorder (`conversation_turns`),
//! and the desktop's rules for phones (approval kinds, Activity records).
//!
//! CI has no agent runtime, so the session is scripted: its events are the desktop's own
//! `AgentEvent`s, saved by the recorder as the session's forwarder saves them and sent to
//! the bridge as `agent_event`. Every wait is on a message; timeouts only bound a failure.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use shodh_link::client::LinkClient;
use shodh_link::iroh::endpoint::presets;
use shodh_link::iroh::{EndpointAddr, SecretKey};
use shodh_link::pairing::DeviceStore;
use shodh_rag::harness::{AgentEvent, RiskTier, RunStatus};

use super::*;
use crate::conversation_commands::ConversationStore;
use crate::conversation_turns::{StoreSink, TurnRecorder};

const BOUND: Duration = Duration::from_secs(30);

async fn within<T>(what: &str, fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(BOUND, fut)
        .await
        .unwrap_or_else(|_| panic!("{what} did not happen"))
}

#[derive(Default)]
struct MemoryDevices(Mutex<Vec<PairedDevice>>);

impl DeviceStore for MemoryDevices {
    fn load(&self) -> Result<Vec<PairedDevice>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, devices: &[PairedDevice]) -> Result<(), String> {
        *self.0.lock().unwrap() = devices.to_vec();
        Ok(())
    }
}

type Sink = StoreSink<Box<dyn Fn(&str) + Send + Sync>>;

/// The desktop: its conversation store and turn recorder, and a scripted session.
struct Desktop {
    events: broadcast::Sender<LinkEvent>,
    turns: TurnRecorder,
    sink: Sink,
    records: Mutex<Vec<AuditRecord>>,
    now: AtomicU64,
}

const SESSION: &str = "session-1";
const RUN: &str = "run-1";

impl Desktop {
    /// One event of the session: saved by the recorder, then sent to the windows and the
    /// phones, as the session's forwarder does.
    fn emit(&self, event: AgentEvent) {
        self.turns.observe(&self.sink, SESSION, &event);
        let _ = self.events.send(LinkEvent {
            name: AGENT_EVENT.to_string(),
            payload: json!({ "sessionId": SESSION, "event": event }),
        });
    }

    fn delta(&self, text: &str) {
        self.emit(AgentEvent::TextDelta {
            run_id: RUN.into(),
            message_id: "m1".into(),
            delta: text.into(),
        });
    }
}

impl LinkHost for Desktop {
    fn invoke(
        &self,
        _caller: &Caller,
        command: &str,
        args: Value,
    ) -> impl std::future::Future<Output = Result<Value, Value>> + Send {
        let result = match command {
            "agent_start" => Ok(json!(SESSION)),
            "agent_send" => {
                let turn: SendTurn = serde_json::from_value(args["turn"].clone()).map_err(invalid);
                match turn {
                    Err(e) => Err(e),
                    Ok(turn) => match self.turns.begin(&self.sink, SESSION, RUN, turn) {
                        Err(e) => Err(json!(e)),
                        Ok(()) => {
                            self.emit(AgentEvent::RunStarted {
                                run_id: RUN.into(),
                                session_id: SESSION.into(),
                                model: "test/model".into(),
                                at_ms: 1,
                                warning: None,
                            });
                            self.delta("Sixty days ");
                            self.emit(AgentEvent::StepFinished {
                                run_id: RUN.into(),
                                step_id: "search-1".into(),
                                ok: true,
                                summary: "2 passages".into(),
                                detail: Some(json!({ "citations": [{ "number": 1, "source": "C:/contracts/lease.pdf", "page": 4 }] })),
                                duration_ms: 5,
                                invalid_arguments: false,
                            });
                            self.emit(AgentEvent::ApprovalRequested {
                                run_id: RUN.into(),
                                step_id: "fetch-1".into(),
                                tool: "fetch_url".into(),
                                label: "Fetch the lease register".into(),
                                tier: RiskTier::Write,
                                preview: json!({ "url": "https://example.org/register" }),
                            });
                            Ok(json!(RUN))
                        }
                    },
                }
            }
            "agent_approve" => {
                self.emit(AgentEvent::StepFinished {
                    run_id: RUN.into(),
                    step_id: "fetch-1".into(),
                    ok: true,
                    summary: "fetched".into(),
                    detail: None,
                    duration_ms: 3,
                    invalid_arguments: false,
                });
                Ok(Value::Null)
            }
            "load_conversation" => {
                let id = args["conversationId"].as_str().unwrap_or_default();
                reply(self.sink.store.get(id))
            }
            other => {
                Err(json!({ "code": "runtime_error", "message": format!("no script for {other}") }))
            }
        };
        async move { result }
    }

    fn approval_kind(&self, _session_id: &str, step: &PendingStep) -> ApprovalKind {
        approval_kind(
            Some(RemoteFacts {
                code: false,
                agent_mode: AgentMode::Ask,
            }),
            step,
        )
    }

    fn record(&self, record: CallRecord) {
        self.records.lock().unwrap().push(activity_record(&record));
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

async fn local_endpoint(accepts: bool) -> Endpoint {
    let builder = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .expect("a loopback address");
    let builder = if accepts {
        builder.alpns(vec![shodh_link::ALPN.to_vec()])
    } else {
        builder
    };
    builder.bind().await.expect("the endpoint binds")
}

fn direct_addr(endpoint: &Endpoint) -> EndpointAddr {
    EndpointAddr::new(endpoint.id()).with_ip_addr(endpoint.bound_sockets()[0])
}

/// The next agent event, past the "conversation saved" notices the desktop also sends.
async fn next_agent_event(client: &mut LinkClient) -> Value {
    loop {
        let event = within("an event", client.next_event())
            .await
            .expect("the connection is open");
        if event.name == AGENT_EVENT {
            return event.payload;
        }
        assert_eq!(event.name, CONVERSATION_CHANGED_EVENT);
        assert_eq!(event.payload["id"], "conv-1");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_ask_stream_approve_open() {
    let dir = tempfile::tempdir().unwrap();
    let (events, _) = broadcast::channel(256);
    let changed = events.clone();
    let desktop = Arc::new(Desktop {
        events,
        turns: TurnRecorder::default(),
        sink: StoreSink {
            store: ConversationStore::in_dir(dir.path()),
            // Every save tells the windows and the phones, as the app does.
            changed: Box::new(move |id: &str| {
                let _ = changed.send(LinkEvent {
                    name: CONVERSATION_CHANGED_EVENT.to_string(),
                    payload: json!({ "id": id }),
                });
            }),
        },
        records: Mutex::default(),
        now: AtomicU64::new(1_763_000_000_000),
    });
    let devices = Arc::new(DeviceRegistry::open(Box::<MemoryDevices>::default()).unwrap());
    let pairing = Arc::new(PendingPairing::default());
    let link = Arc::new(Link::new(desktop.clone(), devices.clone(), pairing.clone()));
    let endpoint = local_endpoint(true).await;
    tokio::spawn(link.clone().serve(endpoint.clone()));

    // Pair, through the QR string as the phone scans it.
    let secret = new_secret().unwrap();
    let offer = PairingOffer {
        node_id: endpoint.id(),
        relay_url: None,
        secret,
        expires_at: pairing.issue(secret, desktop.now_ms()),
    };
    let scanned = PairingOffer::from_qr(&offer.to_qr()).unwrap();
    let phone = local_endpoint(false).await;
    let mut client = within(
        "pairing",
        LinkClient::pair(&phone, direct_addr(&endpoint), &scanned, "Pixel 8"),
    )
    .await
    .expect("the phone pairs");
    assert_eq!(devices.list()[0].node_id, phone.id().to_string());

    // Ask: the question names its conversation, so the desktop saves the answer.
    within(
        "agent_start",
        client.invoke("agent_start", json!({ "conversationId": "conv-1" })),
    )
    .await
    .unwrap();
    client.subscribe(vec![SESSION.into()]).await.unwrap();
    let turn = json!({
        "conversationId": "conv-1",
        "question": { "id": "q1", "role": "user", "content": "What is the notice period?", "timestamp": "2026-11-13T10:00:00.000Z" },
        "answer": {
            "id": "a1", "role": "assistant", "content": "", "timestamp": "2026-11-13T10:00:00.000Z",
            "transcript": { "runId": RUN, "startedAtMs": 1_763_000_000_000u64, "status": "starting", "agentMode": "ask" }
        }
    });
    let run = within(
        "agent_send",
        client.invoke(
            "agent_send",
            json!({ "sessionId": SESSION, "text": "What is the notice period?", "requestId": "r1", "turn": turn }),
        ),
    )
    .await
    .expect("the question is sent");
    assert_eq!(run, json!(RUN));

    // The answer streams over the link: tokens, a citation, an approval.
    assert_eq!(
        next_agent_event(&mut client).await["event"]["type"],
        "run_started"
    );
    assert_eq!(
        next_agent_event(&mut client).await["event"]["delta"],
        "Sixty days "
    );
    let cited = next_agent_event(&mut client).await;
    assert_eq!(cited["event"]["detail"]["citations"][0]["page"], 4);
    let asked = next_agent_event(&mut client).await;
    assert_eq!(asked["event"]["type"], "approval_requested");

    // The Ask step is approved from the phone.
    within(
        "the approval",
        client.invoke(
            "agent_approve",
            json!({ "sessionId": SESSION, "stepId": "fetch-1", "approved": true }),
        ),
    )
    .await
    .expect("an Ask step may be approved from the phone");
    assert_eq!(
        next_agent_event(&mut client).await["event"]["type"],
        "step_finished"
    );

    // The question and the answer as it started are saved before the phone goes.
    let saved = desktop.sink.store.get("conv-1").unwrap().expect("saved");
    let ids: Vec<&str> = saved.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["q1", "a1"]);

    // The phone drops off mid-answer; the answer finishes while it is away.
    client.close();
    within("the close", client.closed()).await;
    desktop.delta("notice [1].");
    desktop.emit(AgentEvent::GroundingStarted {
        run_id: RUN.into(),
        round: 0,
    });
    desktop.emit(AgentEvent::RunFinished {
        run_id: RUN.into(),
        status: RunStatus::Completed,
        duration_ms: 900,
        error: None,
        provider_error: None,
    });

    // It reconnects, and the full answer is in the store.
    let again = within(
        "reconnecting",
        LinkClient::connect(&phone, direct_addr(&endpoint), "Pixel 8"),
    )
    .await
    .expect("a paired phone reconnects");
    let record = within(
        "load_conversation",
        again.invoke("load_conversation", json!({ "conversationId": "conv-1" })),
    )
    .await
    .expect("the conversation loads");
    let messages = record["messages"].as_array().expect("its messages");
    assert_eq!(messages.len(), 2, "one question, one answer");
    let answer = &messages[1];
    assert_eq!(answer["id"], "a1");
    let text = answer["content"].as_str().unwrap();
    assert!(
        text.contains("Sixty days") && text.contains("notice [1]."),
        "the whole answer, also what came while the phone was away: {text}"
    );
    assert_eq!(answer["transcript"]["status"], "completed");
    assert!(
        !desktop.turns.records(RUN),
        "the run is saved and done with"
    );

    // Every call is in Activity with the phone's name, never its arguments.
    let records = desktop.records.lock().unwrap().clone();
    for command in [
        "pair",
        "agent_start",
        "agent_send",
        "agent_approve",
        "load_conversation",
    ] {
        assert!(
            records.iter().any(|r| r.payload["command"] == command
                && r.payload["device"] == "Pixel 8"
                && r.payload["outcome"] == "done"),
            "{command} is recorded"
        );
    }
    assert!(records
        .iter()
        .all(|r| r.event_type == AuditEventType::PhoneLink
            && r.principal == format!("phone:{}", phone.id())));
    assert!(!records
        .iter()
        .any(|r| r.payload.to_string().contains("notice period")));
}
