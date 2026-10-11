//! A Model Context Protocol client: stdio (a child process speaking JSON-RPC
//! lines) and streamable HTTP (each request POSTed; the answer is JSON or an
//! SSE stream).
//!
//! Only what tools need: `initialize`, `tools/list` (paged) and `tools/call`.
//! The server's notifications are ignored and its requests are answered
//! (`ping` with an empty result, anything else "method not found"), so a
//! chatty server never stalls a call. Every request has a deadline.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use super::config::Transport;
use crate::harness::truncate_chars;

/// Protocol revision Shodh asks for (servers may answer with an older one).
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Deadline of `initialize` and `tools/list`.
pub const SETUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Deadline of one `tools/call` (indexing a repository can take minutes).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Longest line or SSE event accepted from a server.
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Most `tools/list` pages read.
const MAX_PAGES: usize = 20;

const STDERR_TAIL_LINES: usize = 12;

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    #[error("the server could not be started: {0}")]
    Spawn(String),
    #[error("the server stopped ({0})")]
    Closed(String),
    #[error("the server did not answer {0} in time")]
    Timeout(String),
    #[error("the server answered {method} with an error: {message}")]
    Rpc { method: String, message: String },
    #[error("{0}")]
    Http(String),
    #[error("the server's answer could not be read: {0}")]
    Protocol(String),
}

/// A tool as the server lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "empty_schema")]
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: Option<ToolAnnotations>,
}

fn empty_schema() -> Value {
    json!({ "type": "object" })
}

/// The hints a server gives about a tool (MCP `ToolAnnotations`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub read_only_hint: Option<bool>,
    #[serde(default)]
    pub destructive_hint: Option<bool>,
}

impl ToolInfo {
    /// Whether the server marks the tool as not changing anything.
    pub fn read_only(&self) -> bool {
        self.annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false)
    }

    /// Whether the server marks the tool as destructive (the default for a
    /// tool that is not read-only, per the specification).
    pub fn destructive(&self) -> bool {
        !self.read_only()
            && self
                .annotations
                .as_ref()
                .and_then(|a| a.destructive_hint)
                .unwrap_or(true)
    }
}

/// The result of `tools/call`.
#[derive(Debug, Clone, PartialEq)]
pub struct CallResult {
    /// The content as text: text blocks as they are, other blocks described.
    pub text: String,
    pub is_error: bool,
}

/// Flatten `tools/call`'s result into text.
pub fn call_result(result: &Value) -> CallResult {
    let mut parts: Vec<String> = Vec::new();
    for block in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
            "resource" => {
                let resource = block.get("resource").unwrap_or(&Value::Null);
                match resource.get("text").and_then(Value::as_str) {
                    Some(text) => parts.push(text.to_string()),
                    None => parts.push(format!(
                        "[binary resource {}]",
                        resource.get("uri").and_then(Value::as_str).unwrap_or("")
                    )),
                }
            }
            "resource_link" => parts.push(format!(
                "[resource {}]",
                block.get("uri").and_then(Value::as_str).unwrap_or("")
            )),
            "image" | "audio" => parts.push(format!("[{kind} omitted]")),
            _ => {}
        }
    }
    if parts.is_empty() {
        if let Some(structured) = result.get("structuredContent") {
            parts.push(structured.to_string());
        }
    }
    CallResult {
        text: parts.join("\n"),
        is_error: result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value, McpError>>>>;
type Writer = Arc<tokio::sync::Mutex<Box<dyn AsyncWrite + Send + Unpin>>>;

/// The stdio side: the writer, the pending requests, the child.
struct StdioConn {
    writer: Writer,
    pending: Arc<Pending>,
    closed: Arc<AtomicBool>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    child: Mutex<Option<tokio::process::Child>>,
}

struct HttpConn {
    client: reqwest::Client,
    url: String,
    headers: Vec<(String, String)>,
    session: Mutex<Option<String>>,
    protocol: Mutex<Option<String>>,
}

enum Conn {
    Stdio(StdioConn),
    Http(HttpConn),
}

/// A connected, initialised MCP server.
pub struct McpClient {
    conn: Conn,
    next_id: AtomicU64,
    /// Secret values (env, headers) removed from error text.
    secrets: Vec<String>,
    /// What the server said about itself in `initialize`.
    pub server_name: Option<String>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `command` as it can be started: on Windows a bare name such as `npx` is
/// looked up on `PATH` with the `PATHEXT` extensions (`npx.cmd`), which
/// starting a process does not do by itself.
pub fn resolve_command(command: &str, path: Option<&str>, pathext: Option<&str>) -> PathBuf {
    let candidate = PathBuf::from(command);
    if !cfg!(windows) || candidate.extension().is_some() || candidate.components().count() > 1 {
        return candidate;
    }
    let extensions: Vec<String> = pathext
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    for dir in std::env::split_paths(path.unwrap_or("")) {
        for ext in &extensions {
            let full = dir.join(format!("{command}{ext}"));
            if full.is_file() {
                return full;
            }
        }
    }
    candidate
}

impl McpClient {
    /// Connect to the server `transport` describes and initialise it.
    pub async fn connect(transport: &Transport) -> Result<Self, McpError> {
        match transport {
            Transport::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                let path = env
                    .get("PATH")
                    .cloned()
                    .or_else(|| std::env::var("PATH").ok());
                let program = resolve_command(
                    command,
                    path.as_deref(),
                    std::env::var("PATHEXT").ok().as_deref(),
                );
                let mut cmd = tokio::process::Command::new(&program);
                cmd.args(args)
                    .envs(env)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
                if let Some(cwd) = cwd {
                    cmd.current_dir(cwd);
                }
                #[cfg(windows)]
                {
                    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                    cmd.creation_flags(CREATE_NO_WINDOW);
                }
                let mut child = cmd
                    .spawn()
                    .map_err(|e| McpError::Spawn(format!("{}: {e}", program.display())))?;
                let stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| McpError::Spawn("no stdin pipe".into()))?;
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| McpError::Spawn("no stdout pipe".into()))?;
                let stderr = child.stderr.take();
                let client = Self::over_io(stdout, stdin, Some(child), env.values().cloned());
                if let (Some(stderr), Conn::Stdio(conn)) = (stderr, &client.conn) {
                    tokio::spawn(drain_stderr(stderr, conn.stderr.clone()));
                }
                client.initialise().await
            }
            Transport::Http { url, headers } => {
                let client = reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(15))
                    .user_agent(concat!("shodh/", env!("CARGO_PKG_VERSION")))
                    .build()
                    .map_err(|e| McpError::Http(e.to_string()))?;
                let this = Self {
                    conn: Conn::Http(HttpConn {
                        client,
                        url: url.clone(),
                        headers: headers
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                        session: Mutex::new(None),
                        protocol: Mutex::new(None),
                    }),
                    next_id: AtomicU64::new(0),
                    secrets: headers.values().filter(|v| v.len() >= 4).cloned().collect(),
                    server_name: None,
                };
                this.initialise().await
            }
        }
    }

    /// A client over a byte stream pair (a child's pipes, or a test's).
    fn over_io(
        reader: impl AsyncRead + Send + Unpin + 'static,
        writer: impl AsyncWrite + Send + Unpin + 'static,
        child: Option<tokio::process::Child>,
        secrets: impl Iterator<Item = String>,
    ) -> Self {
        let pending: Arc<Pending> = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let boxed: Box<dyn AsyncWrite + Send + Unpin> = Box::new(writer);
        let writer: Writer = Arc::new(tokio::sync::Mutex::new(boxed));
        // The reader answers the server's own requests through the same writer.
        tokio::spawn(read_loop(
            reader,
            pending.clone(),
            closed.clone(),
            writer.clone(),
        ));
        Self {
            conn: Conn::Stdio(StdioConn {
                writer,
                pending,
                closed,
                stderr: Arc::default(),
                child: Mutex::new(child),
            }),
            next_id: AtomicU64::new(0),
            secrets: secrets.filter(|v| v.len() >= 4).collect(),
            server_name: None,
        }
    }

    async fn initialise(mut self) -> Result<Self, McpError> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "shodh", "version": env!("CARGO_PKG_VERSION") }
                }),
                SETUP_TIMEOUT,
            )
            .await?;
        self.server_name = result
            .get("serverInfo")
            .and_then(|i| i.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Conn::Http(http) = &self.conn {
            *lock(&http.protocol) = result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        self.notify("notifications/initialized").await?;
        Ok(self)
    }

    /// The server's tools (every page).
    pub async fn list_tools(&self) -> Result<Vec<ToolInfo>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let page = self.request("tools/list", params, SETUP_TIMEOUT).await?;
            let listed: Vec<ToolInfo> =
                serde_json::from_value(page.get("tools").cloned().unwrap_or_else(|| json!([])))
                    .map_err(|e| McpError::Protocol(format!("tools/list: {e}")))?;
            tools.extend(listed);
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// Call `tool` with `arguments`.
    pub async fn call_tool(&self, tool: &str, arguments: Value) -> Result<CallResult, McpError> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": tool, "arguments": arguments }),
                CALL_TIMEOUT,
            )
            .await?;
        Ok(call_result(&result))
    }

    /// Whether the connection still works (a stdio server has not exited).
    pub fn is_alive(&self) -> bool {
        match &self.conn {
            Conn::Stdio(conn) => !conn.closed.load(Ordering::SeqCst),
            Conn::Http(_) => true,
        }
    }

    /// Stop the server (stdio) and fail what is pending.
    pub fn shutdown(&self) {
        if let Conn::Stdio(conn) = &self.conn {
            conn.closed.store(true, Ordering::SeqCst);
            if let Some(mut child) = lock(&conn.child).take() {
                if let Err(e) = child.start_kill() {
                    tracing::debug!(target: "shodh::mcp", error = %e, "stopping an MCP server failed");
                }
            }
            for (_, tx) in lock(&conn.pending).drain() {
                let _ = tx.send(Err(McpError::Closed("disconnected".into())));
            }
        }
    }

    fn redact(&self, text: &str) -> String {
        let mut text = truncate_chars(text, 600);
        for secret in &self.secrets {
            text = text.replace(secret.as_str(), "[REDACTED]");
        }
        text
    }

    fn stderr_summary(&self) -> String {
        match &self.conn {
            Conn::Stdio(conn) => {
                let tail: Vec<String> = lock(&conn.stderr).iter().cloned().collect();
                if tail.is_empty() {
                    "no diagnostic output".to_string()
                } else {
                    self.redact(&tail.join(" | "))
                }
            }
            Conn::Http(_) => String::new(),
        }
    }

    async fn notify(&self, method: &str) -> Result<(), McpError> {
        let message = json!({ "jsonrpc": "2.0", "method": method });
        match &self.conn {
            Conn::Stdio(conn) => write_line(&conn.writer, &message).await,
            Conn::Http(http) => self.post(http, &message, None).await.map(|_| ()),
        }
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let outcome = match &self.conn {
            Conn::Stdio(conn) => {
                let (tx, rx) = oneshot::channel();
                lock(&conn.pending).insert(id, tx);
                if conn.closed.load(Ordering::SeqCst) {
                    lock(&conn.pending).remove(&id);
                    return Err(McpError::Closed(self.stderr_summary()));
                }
                if let Err(e) = write_line(&conn.writer, &message).await {
                    lock(&conn.pending).remove(&id);
                    return Err(e);
                }
                match tokio::time::timeout(timeout, rx).await {
                    Ok(Ok(outcome)) => outcome,
                    Ok(Err(_)) => Err(McpError::Closed(self.stderr_summary())),
                    Err(_) => {
                        lock(&conn.pending).remove(&id);
                        Err(McpError::Timeout(method.to_string()))
                    }
                }
            }
            Conn::Http(http) => {
                match tokio::time::timeout(timeout, self.post(http, &message, Some(id))).await {
                    Ok(Ok(Some(reply))) => reply_outcome(reply),
                    Ok(Ok(None)) => Err(McpError::Protocol(format!("no answer to {method}"))),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(McpError::Timeout(method.to_string())),
                }
            }
        };
        outcome.map_err(|e| match e {
            McpError::Rpc { message, .. } => McpError::Rpc {
                method: method.to_string(),
                message: self.redact(&message),
            },
            McpError::Closed(reason) if matches!(self.conn, Conn::Stdio(_)) => {
                McpError::Closed(self.redact(&reason))
            }
            other => other,
        })
    }

    /// POST one message; returns the reply with `id` (if one is expected).
    async fn post(
        &self,
        http: &HttpConn,
        message: &Value,
        id: Option<u64>,
    ) -> Result<Option<Value>, McpError> {
        let mut request = http
            .client
            .post(&http.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .body(message.to_string());
        for (name, value) in &http.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(session) = lock(&http.session).clone() {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(protocol) = lock(&http.protocol).clone() {
            request = request.header("MCP-Protocol-Version", protocol);
        }
        let mut response = request
            .send()
            .await
            .map_err(|e| McpError::Http(self.redact(&format!("{}: {e}", http.url))))?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *lock(&http.session) = Some(session.to_string());
        }
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(McpError::Http(self.redact(&format!(
                "{} answered {status}: {}",
                http.url,
                body.trim()
            ))));
        }
        let Some(id) = id else {
            return Ok(None);
        };
        let event_stream = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        if !event_stream {
            let body = response
                .bytes()
                .await
                .map_err(|e| McpError::Http(e.to_string()))?;
            if body.len() > MAX_MESSAGE_BYTES {
                return Err(McpError::Protocol("the answer is too large".into()));
            }
            let value: Value =
                serde_json::from_slice(&body).map_err(|e| McpError::Protocol(e.to_string()))?;
            // A batch answer is a list.
            let reply = match value {
                Value::Array(items) => items.into_iter().find(|m| m.get("id") == Some(&json!(id))),
                single => Some(single),
            };
            return Ok(reply);
        }
        let mut buffer: Vec<u8> = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| McpError::Http(e.to_string()))?
        {
            buffer.extend_from_slice(&chunk);
            if buffer.len() > MAX_MESSAGE_BYTES {
                return Err(McpError::Protocol("an event is too large".into()));
            }
            while let Some(end) = event_end(&buffer) {
                let event: Vec<u8> = buffer.drain(..end).collect();
                if let Some(reply) = sse_message(&event).filter(|m| m.get("id") == Some(&json!(id)))
                {
                    return Ok(Some(reply));
                }
            }
        }
        // The stream ended: the last event may lack its blank line.
        Ok(sse_message(&buffer).filter(|m| m.get("id") == Some(&json!(id))))
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn reply_outcome(reply: Value) -> Result<Value, McpError> {
    if let Some(error) = reply.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| error.to_string());
        return Err(McpError::Rpc {
            method: String::new(),
            message,
        });
    }
    Ok(reply.get("result").cloned().unwrap_or(Value::Null))
}

/// End (exclusive) of the first complete SSE event in `buffer`.
fn event_end(buffer: &[u8]) -> Option<usize> {
    let lf = buffer.windows(2).position(|w| w == b"\n\n").map(|p| p + 2);
    let crlf = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The JSON-RPC message in one SSE event (its `data:` lines joined).
fn sse_message(event: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(event);
    let data: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|d| d.strip_prefix(' ').unwrap_or(d))
        .collect();
    if data.is_empty() {
        return None;
    }
    serde_json::from_str(&data.join("\n")).ok()
}

async fn write_line(writer: &Writer, message: &Value) -> Result<(), McpError> {
    let mut line = message.to_string();
    line.push('\n');
    let mut writer = writer.lock().await;
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| McpError::Closed(e.to_string()))?;
    writer
        .flush()
        .await
        .map_err(|e| McpError::Closed(e.to_string()))
}

async fn read_loop(
    reader: impl AsyncRead + Unpin,
    pending: Arc<Pending>,
    closed: Arc<AtomicBool>,
    writer: Writer,
) {
    let mut lines = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        match lines.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(n) if n > MAX_MESSAGE_BYTES => continue,
            Ok(_) => {}
        }
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            // Servers sometimes print logs to stdout; they are not messages.
            continue;
        };
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str);
        match (id, method) {
            // The server's own request: answer it.
            (Some(id), Some(method)) => {
                let reply = if method == "ping" {
                    json!({ "jsonrpc": "2.0", "id": id, "result": {} })
                } else {
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } })
                };
                if let Err(e) = write_line(&writer, &reply).await {
                    tracing::debug!(target: "shodh::mcp", error = %e, "answering an MCP server request failed");
                }
            }
            // A reply to one of ours.
            (Some(id), None) => {
                if let Some(id) = id.as_u64() {
                    if let Some(tx) = lock(&pending).remove(&id) {
                        let _ = tx.send(reply_outcome(message));
                    }
                }
            }
            // A notification.
            _ => {}
        }
    }
    closed.store(true, Ordering::SeqCst);
    for (_, tx) in lock(&pending).drain() {
        let _ = tx.send(Err(McpError::Closed("the server exited".into())));
    }
}

async fn drain_stderr(stderr: tokio::process::ChildStderr, tail: Arc<Mutex<VecDeque<String>>>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut tail = lock(&tail);
        if tail.len() == STDERR_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(truncate_chars(line.trim(), 300));
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
