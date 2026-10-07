//! Live checks against the pinned omp binary (`SHODH_OMP_PATH`), ignored by
//! default: which hosts a session contacts, and whether a hostile code folder
//! can route a Code session to another model.
//!
//! The configured provider is a scripted Anthropic endpoint on localhost
//! (`ANTHROPIC_BASE_URL`). Every other HTTP(S) request goes through a proxy
//! that records its target and refuses it, and canaries listen on the ports of
//! the local servers omp probes (Ollama, llama.cpp, LM Studio, OTLP).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::code_mode::CodeFolder;
use super::model::{EnvValue, OmpModel, Secret};
use super::sidecar::{spawn, LaunchSpec, OmpLayout};

const PRIMARY: &str = "claude-haiku-4-5";
const OTHER: &str = "claude-sonnet-4-5";
const LOCAL_SERVER_PORTS: [u16; 5] = [11434, 8080, 1234, 4317, 4318];

/// Hosts no omp 18.4.10 setting keeps the Anthropic provider from contacting:
/// its model discovery (`/v1/models`, which ignores `ANTHROPIC_BASE_URL`; in
/// the app it is the configured host) and the models.dev catalog mirror its
/// discovery reads model metadata from.
const UNAVOIDABLE: [&str; 2] = ["api.anthropic.com:443", "catalog.stencil.so:443"];

type Log = Arc<Mutex<Vec<String>>>;

fn record(log: &Log, entry: String) {
    log.lock().unwrap_or_else(|e| e.into_inner()).push(entry);
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The request line and body of one HTTP/1.1 request.
async fn read_request(stream: &mut BufReader<TcpStream>) -> Option<(String, Vec<u8>)> {
    let mut line = String::new();
    stream.read_line(&mut line).await.ok()?;
    let request_line = line.trim().to_string();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if stream.read_line(&mut header).await.ok()? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).await.ok()?;
    Some((request_line, body))
}

async fn respond(stream: &mut BufReader<TcpStream>, status: &str, kind: &str, body: &str) {
    // `retry-after-ms` keeps the client's own retries of an error fast.
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nretry-after-ms: 10\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let stream = stream.get_mut();
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn reply_stream(model: &str) -> String {
    let events = [
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "model": model, "content": [], "stop_reason": null, "usage": {"input_tokens": 5, "output_tokens": 1}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi there"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ];
    events
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect()
}

/// The scripted provider: logs `POST <path> <model>` and answers with a
/// short text reply. The first `failures` requests get Anthropic's
/// "overloaded" error instead; once the client's own retries are spent, the
/// session retries or follows its fallback chain.
async fn provider(log: Log, failures: usize) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let failed = Arc::new(Mutex::new(0usize));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (log, failed) = (log.clone(), failed.clone());
            tokio::spawn(async move {
                let mut stream = BufReader::new(stream);
                let Some((line, body)) = read_request(&mut stream).await else {
                    return;
                };
                let model = serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|b| b["model"].as_str().map(str::to_string))
                    .unwrap_or_default();
                let path = line
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                record(&log, format!("{path} {model}"));
                if !path.starts_with("POST /v1/messages") {
                    respond(&mut stream, "404 Not Found", "text/plain", "").await;
                    return;
                }
                let fail = {
                    let mut failed = failed.lock().unwrap_or_else(|e| e.into_inner());
                    *failed += 1;
                    *failed <= failures
                };
                if fail {
                    let error = json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}});
                    respond(
                        &mut stream,
                        "529 Overloaded",
                        "application/json",
                        &error.to_string(),
                    )
                    .await;
                } else {
                    respond(
                        &mut stream,
                        "200 OK",
                        "text/event-stream",
                        &reply_stream(&model),
                    )
                    .await;
                }
            });
        }
    });
    port
}

/// Records the first line of every connection on `listener` and refuses it.
fn refuse_all(listener: TcpListener, log: Log, label: String) {
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (log, label) = (log.clone(), label.clone());
            tokio::spawn(async move {
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                let _ = stream.read_line(&mut line).await;
                record(&log, format!("{label} {}", line.trim()));
                respond(&mut stream, "403 Forbidden", "text/plain", "").await;
            });
        }
    });
}

/// The refusing proxy (its port) and the canaries; every connection they see
/// goes to `log`.
async fn refusing_network(log: &Log) -> u16 {
    let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = proxy.local_addr().unwrap().port();
    refuse_all(proxy, log.clone(), "proxy".into());
    for canary in LOCAL_SERVER_PORTS {
        let listener = TcpListener::bind(("127.0.0.1", canary))
            .await
            .unwrap_or_else(|e| panic!("port {canary} is in use ({e}); stop that server first"));
        refuse_all(listener, log.clone(), format!("localhost:{canary}"));
    }
    port
}

fn spec(data: &std::path::Path, provider: u16, proxy: u16, code: Option<CodeFolder>) -> LaunchSpec {
    let binary = std::path::PathBuf::from(std::env::var_os("SHODH_OMP_PATH").unwrap());
    let proxy = format!("http://127.0.0.1:{proxy}");
    let mut env = vec![
        (
            "ANTHROPIC_API_KEY".to_string(),
            EnvValue::Secret(Secret::new("sk-ant-live-test")),
        ),
        (
            "ANTHROPIC_BASE_URL".to_string(),
            EnvValue::Plain(format!("http://127.0.0.1:{provider}")),
        ),
    ];
    for name in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        env.push((name.to_string(), EnvValue::Plain(proxy.clone())));
    }
    for name in ["NO_PROXY", "no_proxy"] {
        env.push((
            name.to_string(),
            EnvValue::Plain("127.0.0.1,localhost".into()),
        ));
    }
    LaunchSpec {
        binary,
        layout: OmpLayout::new(data),
        model: OmpModel {
            model_arg: format!("anthropic/{PRIMARY}"),
            provider_label: "Anthropic",
            is_local: false,
            env,
            warning: None,
        },
        system_prompt: "Reply briefly.".into(),
        session_id: "live".into(),
        code,
        host_tools: Vec::new(),
    }
}

/// Send one prompt and collect omp's frames until one of type `until`
/// arrives; then stay up a few seconds so late background requests are seen.
async fn run_prompt(spec: &LaunchSpec, until: &str) -> Vec<Value> {
    let mut process = spawn(spec).await.unwrap();
    let mut lines = BufReader::new(process.stdout).lines();
    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        let line = tokio::time::timeout_at(deadline, lines.next_line())
            .await
            .unwrap_or_else(|_| panic!("no {until} within 180 s"))
            .unwrap()
            .unwrap_or_else(|| panic!("omp exited before {until}"));
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let kind = frame["type"].as_str().unwrap_or_default().to_string();
        if kind == "ready" && !frames.iter().any(|f: &Value| f["type"] == "ready") {
            let prompt = json!({"type": "prompt", "id": "p1", "message": "Say hi"});
            process
                .stdin
                .write_all(
                    format!(
                        "{prompt}
"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
        frames.push(frame);
        if kind == until {
            break;
        }
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    let _ = process.child.kill().await;
    frames
}

/// The assistant's text in an `agent_end` frame.
fn reply(frames: &[Value]) -> String {
    frames
        .iter()
        .filter(|f| f["type"] == "agent_end")
        .flat_map(|f| f["messages"].as_array().cloned().unwrap_or_default())
        .filter(|m| m["role"] == "assistant")
        .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
        .filter_map(|c| c["text"].as_str().map(str::to_string))
        .collect()
}

fn assert_only_the_provider_was_contacted(network: &Log) {
    let contacted = entries(network);
    let unexpected: Vec<&String> = contacted
        .iter()
        .filter(|entry| {
            !UNAVOIDABLE
                .iter()
                .any(|host| entry.starts_with(&format!("proxy CONNECT {host} ")))
        })
        .collect();
    assert!(unexpected.is_empty(), "omp contacted {unexpected:#?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the omp binary at SHODH_OMP_PATH and free local server ports"]
async fn sessions_contact_only_the_configured_provider_and_ignore_the_folders_models() {
    let network: Log = Arc::default();
    let proxy = refusing_network(&network).await;
    let data = tempfile::tempdir().unwrap();

    // Research: the reply arrives, nothing else is contacted.
    let calls: Log = Arc::default();
    let port = provider(calls.clone(), 0).await;
    let frames = run_prompt(&spec(data.path(), port, proxy, None), "agent_end").await;
    assert_eq!(reply(&frames), "hi there");
    assert_eq!(entries(&calls), [format!("POST /v1/messages {PRIMARY}")]);
    assert_only_the_provider_was_contacted(&network);

    // Code: a folder whose own config names other models (fallback chains,
    // model roles). The primary keeps failing; the session must retry it
    // (`auto_retry_start`) without switching to the folder's model.
    let folder = tempfile::tempdir().unwrap();
    let omp_dir = folder.path().join(".omp");
    std::fs::create_dir_all(&omp_dir).unwrap();
    let other = format!("anthropic/{OTHER}");
    std::fs::write(
        omp_dir.join("settings.json"),
        json!({
            "retry": { "fallbackChains": {
                "default": [other],
                "anthropic/claude-haiku-4-5": [other],
                "anthropic/*": [other]
            } },
            "modelRoles": { "default": other, "smol": other }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        omp_dir.join("config.yml"),
        format!("modelRoles:\n  default: {other}\n  smol: {other}\n  slow: {other}\n"),
    )
    .unwrap();
    let calls: Log = Arc::default();
    let port = provider(calls.clone(), usize::MAX).await;
    let code = CodeFolder::open(folder.path()).unwrap();
    let frames = run_prompt(
        &spec(data.path(), port, proxy, Some(code)),
        "auto_retry_start",
    )
    .await;
    let switched: Vec<&Value> = frames
        .iter()
        .filter(|f| f["type"] == "retry_fallback_applied")
        .collect();
    assert!(
        switched.is_empty(),
        "the folder's fallback was used: {switched:?}"
    );
    let seen = entries(&calls);
    assert!(!seen.is_empty());
    assert!(
        seen.iter()
            .all(|c| c == &format!("POST /v1/messages {PRIMARY}")),
        "the folder's models were used: {seen:?}"
    );
    assert_only_the_provider_was_contacted(&network);
}

/// A scripted Anthropic provider whose first answer calls `tool` and whose
/// next answers are text. Logs the tools each request offered.
async fn calling_provider(log: Log, tool: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let answered = Arc::new(Mutex::new(0usize));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (log, answered) = (log.clone(), answered.clone());
            tokio::spawn(async move {
                let mut stream = BufReader::new(stream);
                let Some((line, body)) = read_request(&mut stream).await else {
                    return;
                };
                if !line.starts_with("POST /v1/messages") {
                    respond(&mut stream, "404 Not Found", "text/plain", "").await;
                    return;
                }
                let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let offered: Vec<&str> = request["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t["name"].as_str())
                    .collect();
                record(&log, format!("tools {}", offered.join(",")));
                let first = {
                    let mut answered = answered.lock().unwrap_or_else(|e| e.into_inner());
                    *answered += 1;
                    *answered == 1
                };
                let model = request["model"].as_str().unwrap_or_default().to_string();
                let body = if first {
                    let events = [
                        (
                            "message_start",
                            json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "model": model, "content": [], "stop_reason": null, "usage": {"input_tokens": 5, "output_tokens": 1}}}),
                        ),
                        (
                            "content_block_start",
                            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_1", "name": tool, "input": {}}}),
                        ),
                        (
                            "content_block_delta",
                            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"q\":\"x\"}"}}),
                        ),
                        (
                            "content_block_stop",
                            json!({"type": "content_block_stop", "index": 0}),
                        ),
                        (
                            "message_delta",
                            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 2}}),
                        ),
                        ("message_stop", json!({"type": "message_stop"})),
                    ];
                    events
                        .iter()
                        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
                        .collect()
                } else {
                    reply_stream(&model)
                };
                respond(&mut stream, "200 OK", "text/event-stream", &body).await;
            });
        }
    });
    port
}

/// A host tool that records its calls.
struct Ping {
    calls: Arc<Mutex<Vec<Value>>>,
}

#[async_trait::async_trait]
impl crate::harness::tools::HostTool for Ping {
    fn name(&self) -> &str {
        "probe__ping"
    }
    fn label(&self) -> &str {
        "Ping"
    }
    fn label_template(&self) -> &str {
        "Pinging"
    }
    fn description(&self) -> &str {
        "Ping the probe."
    }
    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {"q": {"type": "string"}}})
    }
    fn tier(&self) -> crate::harness::RiskTier {
        crate::harness::RiskTier::Read
    }
    fn load_mode(&self) -> crate::harness::protocol::ToolLoadMode {
        crate::harness::protocol::ToolLoadMode::Essential
    }
    async fn execute(
        &self,
        args: Value,
        _ctx: &crate::harness::tools::ToolContext,
    ) -> Result<crate::harness::tools::ToolOutput, crate::harness::tools::ToolError> {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(args);
        Ok(crate::harness::tools::ToolOutput {
            text_for_model: "pong".into(),
            summary_for_ui: "pong".into(),
            detail: None,
        })
    }
}

/// A Code session offers its host tools next to omp's own, the guard lets
/// them through, and calls reach the registry.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the omp binary at SHODH_OMP_PATH"]
async fn code_sessions_run_their_host_tools() {
    use crate::harness::code_mode::CodeBranchStore;
    use crate::harness::profile::AgentProfile;
    use crate::harness::session::{CodeSession, OmpSession, SessionConfig};
    use crate::harness::tools::ToolRegistry;
    use crate::harness::{AgentEvent, AgentHarness};

    let log: Log = Arc::default();
    let port = calling_provider(log.clone(), "probe__ping").await;
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("main.rs"), "fn main() {}").unwrap();
    let calls: Arc<Mutex<Vec<Value>>> = Arc::default();
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(Ping {
            calls: calls.clone(),
        }))
        .unwrap();
    let profile = AgentProfile {
        allowed_tools: vec!["probe__ping".into()],
        ..AgentProfile::assistant()
    };
    // Port 9 refuses connections: nothing but the scripted provider answers.
    let launch = spec(
        data.path(),
        port,
        9,
        Some(CodeFolder::open(folder.path()).unwrap()),
    );
    let (session, mut events) = OmpSession::start(SessionConfig {
        launch,
        profile,
        registry: Arc::new(registry),
        audit: None,
        grounding: None,
        code: Some(CodeSession {
            conversation_id: "conv".into(),
            branches: CodeBranchStore::in_dir(data.path()),
            settings_dir: None,
        }),
    })
    .await
    .unwrap();
    session.prompt("Ping the probe.", None).await.unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(event) = events.recv().await {
            if let AgentEvent::RunFinished { status, .. } = event {
                return Some(status);
            }
        }
        None
    })
    .await
    .unwrap();
    session.shutdown().await;
    assert!(finished.is_some());
    let seen = entries(&log);
    let offered = seen.iter().find(|e| e.starts_with("tools ")).unwrap();
    assert!(
        offered.contains("probe__ping") && offered.contains("bash"),
        "{offered}"
    );
    assert_eq!(
        calls.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        [json!({"q": "x"})]
    );
}
