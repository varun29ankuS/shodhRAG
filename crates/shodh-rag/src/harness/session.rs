//! `OmpSession`: one omp process driving one conversation.
//!
//! Tasks per session:
//! - **writer** — the only task that writes to omp's stdin, so concurrent
//!   tool results never interleave;
//! - **reader** — parses stdout frames in order, answers command responses,
//!   normalises frames into [`AgentEvent`]s and dispatches host tools;
//! - **stderr drain** — keeps the pipe empty and the last lines for errors;
//! - one task per in-flight host tool call (aborted on `host_tool_cancel`).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::AbortHandle;
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};

use super::error::HarnessError;
use super::events::AgentEvent;
use super::model::EnvValue;
use super::omp::{normalise, NormaliserState, StepOutcome};
use super::profile::AgentProfile;
use super::protocol::{
    parse_frame, HostToolCallFrame, InboundFrame, MessageUpdateMode, OutboundFrame, ResponseFrame,
    StreamingBehavior, SubagentLevel, ToolResultPayload,
};
use super::sidecar::{self, LaunchSpec};
use super::tools::{ApprovalGate, ToolCall, ToolContext, ToolRegistry};
use super::{truncate_chars, AgentHarness};

/// Longest stdout line accepted. omp's v1 frames are capped at 1 MiB.
const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const STDERR_TAIL_LINES: usize = 20;
/// Messages are sent as one JSONL frame; stay far below the frame limit.
pub const MAX_MESSAGE_CHARS: usize = 200_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Everything needed to start a session.
pub struct SessionConfig {
    pub launch: LaunchSpec,
    pub profile: AgentProfile,
    pub registry: Arc<ToolRegistry>,
}

struct Inner {
    session_id: String,
    model: String,
    profile: AgentProfile,
    registry: Arc<ToolRegistry>,
    approvals: ApprovalGate,
    state: Mutex<NormaliserState>,
    outbound: mpsc::UnboundedSender<OutboundFrame>,
    events: mpsc::UnboundedSender<AgentEvent>,
    pending: Mutex<HashMap<String, oneshot::Sender<ResponseFrame>>>,
    /// host_tool_call id → (step id, task)
    inflight: Mutex<HashMap<String, (String, AbortHandle)>>,
    calls_in_run: AtomicU32,
    next_id: AtomicU64,
    closing: AtomicBool,
    closed: AtomicBool,
    close_writer: Arc<Notify>,
    child: tokio::sync::Mutex<Option<Child>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    secrets: Vec<String>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    fn next_id(&self, prefix: &str) -> String {
        format!(
            "{prefix}{}",
            self.next_id.fetch_add(1, Ordering::Relaxed) + 1
        )
    }

    fn emit(&self, event: AgentEvent) {
        if self.events.send(event).is_err() {
            tracing::debug!(target: "shodh::harness", session = %self.session_id, "event receiver dropped");
        }
    }

    fn send(&self, frame: OutboundFrame) -> Result<(), HarnessError> {
        self.outbound
            .send(frame)
            .map_err(|_| HarnessError::SessionClosed)
    }

    fn ensure_open(&self) -> Result<(), HarnessError> {
        if self.closed.load(Ordering::SeqCst) || self.closing.load(Ordering::SeqCst) {
            Err(HarnessError::SessionClosed)
        } else {
            Ok(())
        }
    }

    /// Redacted stderr tail for error messages.
    fn stderr_summary(&self) -> String {
        let tail = lock(&self.stderr_tail)
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        let mut tail = truncate_chars(&tail, 1_000);
        for secret in &self.secrets {
            if !secret.is_empty() {
                tail = tail.replace(secret.as_str(), "[REDACTED]");
            }
        }
        if tail.is_empty() {
            "no diagnostic output".to_string()
        } else {
            tail
        }
    }

    async fn command(&self, frame: OutboundFrame) -> Result<ResponseFrame, HarnessError> {
        let Some(id) = frame.command_id().map(str::to_string) else {
            self.send(frame)?;
            return Err(HarnessError::CommandFailed {
                command: "frame".into(),
                error: "not a command".into(),
            });
        };
        let command = match &frame {
            OutboundFrame::Prompt { .. } => "prompt",
            OutboundFrame::Abort { .. } => "abort",
            OutboundFrame::SetHostTools { .. } => "set_host_tools",
            OutboundFrame::SetEventFilter { .. } => "set_event_filter",
            OutboundFrame::SetSubagentSubscription { .. } => "set_subagent_subscription",
            OutboundFrame::GetSessionStats { .. } => "get_session_stats",
            OutboundFrame::HostToolUpdate { .. } | OutboundFrame::HostToolResult { .. } => "frame",
        };
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id.clone(), tx);
        if let Err(e) = self.send(frame) {
            lock(&self.pending).remove(&id);
            return Err(e);
        }
        let response = match tokio::time::timeout(COMMAND_TIMEOUT, rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => return Err(HarnessError::SessionClosed),
            Err(_) => {
                lock(&self.pending).remove(&id);
                return Err(HarnessError::Timeout(command.to_string()));
            }
        };
        if response.success {
            Ok(response)
        } else {
            Err(HarnessError::CommandFailed {
                command: command.to_string(),
                error: response
                    .error
                    .clone()
                    .unwrap_or_else(|| "no reason given".to_string()),
            })
        }
    }

    fn handle_frame(
        self: &Arc<Self>,
        frame: InboundFrame,
        ready: &mut Option<oneshot::Sender<()>>,
    ) {
        match &frame {
            InboundFrame::Ready(r) => {
                tracing::debug!(target: "shodh::harness", session = %self.session_id, protocol = r.protocol_version, "omp ready");
                if let Some(tx) = ready.take() {
                    let _ = tx.send(());
                }
            }
            InboundFrame::Response(response) => {
                if let Some(id) = &response.id {
                    if let Some(tx) = lock(&self.pending).remove(id) {
                        let _ = tx.send(response.clone());
                    }
                }
            }
            InboundFrame::Malformed { frame_type, error } => {
                tracing::warn!(target: "shodh::harness", session = %self.session_id, frame_type, error, "unexpected omp frame shape; skipped");
            }
            _ => {}
        }

        let events = {
            let mut state = lock(&self.state);
            normalise(&frame, &mut state, now_ms())
        };
        let run_ended = events
            .iter()
            .any(|e| matches!(e, AgentEvent::RunFinished { .. }));
        for event in events {
            self.emit(event);
        }
        if run_ended {
            self.approvals.cancel_all();
            self.abort_inflight();
        }

        match frame {
            InboundFrame::HostToolCall(call) => self.dispatch_host_tool(call),
            InboundFrame::HostToolCancel(cancel) => self.cancel_host_tool(&cancel.target_id),
            _ => {}
        }
    }

    fn dispatch_host_tool(self: &Arc<Self>, call: HostToolCallFrame) {
        let run_id = lock(&self.state).active_run_id().map(str::to_string);
        let Some(run_id) = run_id else {
            let _ = self.send(OutboundFrame::HostToolResult {
                id: call.id,
                result: ToolResultPayload::text("No answer is running; the tool call was ignored."),
                is_error: true,
            });
            return;
        };
        let call_index = self.calls_in_run.fetch_add(1, Ordering::SeqCst) + 1;
        let ctx = ToolContext::new(run_id, call.tool_call_id.clone(), self.events.clone())
            .with_host_call(call.id.clone(), self.outbound.clone());
        let host_id = call.id.clone();
        let step_id = call.tool_call_id.clone();
        let tool_call = ToolCall {
            tool: call.tool_name,
            args: call.arguments,
            call_index,
        };

        // Hold the map while spawning so a fast task cannot finish (and try
        // to remove its entry) before the entry exists.
        let mut inflight = lock(&self.inflight);
        let inner = Arc::clone(self);
        let task_host_id = host_id.clone();
        let task_step_id = step_id.clone();
        let handle = tokio::spawn(async move {
            let outcome = inner
                .registry
                .dispatch(tool_call, &inner.profile, &inner.approvals, &ctx)
                .await;
            // Record the UI summary before omp can answer with tool_execution_end.
            lock(&inner.state).record_outcome(
                &task_step_id,
                StepOutcome {
                    ok: outcome.ok,
                    summary: outcome.summary.clone(),
                    detail: outcome.detail.clone(),
                },
            );
            lock(&inner.inflight).remove(&task_host_id);
            if inner
                .send(OutboundFrame::HostToolResult {
                    id: task_host_id,
                    result: ToolResultPayload::text(outcome.text_for_model),
                    is_error: !outcome.ok,
                })
                .is_err()
            {
                tracing::debug!(target: "shodh::harness", "omp writer closed before a tool result");
            }
        });
        inflight.insert(host_id, (step_id, handle.abort_handle()));
    }

    fn cancel_host_tool(&self, host_call_id: &str) {
        let entry = lock(&self.inflight).remove(host_call_id);
        if let Some((step_id, handle)) = entry {
            handle.abort();
            self.approvals.cancel(&step_id);
            lock(&self.state).record_outcome(
                &step_id,
                StepOutcome {
                    ok: false,
                    summary: "Cancelled".to_string(),
                    detail: None,
                },
            );
        }
    }

    fn abort_inflight(&self) {
        for (_, (_, handle)) in lock(&self.inflight).drain() {
            handle.abort();
        }
    }

    fn on_eof(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let reason = if self.closing.load(Ordering::SeqCst) {
            "The session was closed".to_string()
        } else {
            format!(
                "The agent runtime stopped unexpectedly ({})",
                self.stderr_summary()
            )
        };
        let events = lock(&self.state).fail_run(&reason, now_ms());
        for event in events {
            self.emit(event);
        }
        self.approvals.cancel_all();
        self.abort_inflight();
        lock(&self.pending).clear();
        if !self.closing.load(Ordering::SeqCst) {
            tracing::warn!(target: "shodh::harness", session = %self.session_id, "omp exited unexpectedly");
        }
    }

    fn validate_message(text: &str) -> Result<&str, HarnessError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(HarnessError::EmptyMessage);
        }
        if trimmed.starts_with('/') {
            return Err(HarnessError::SlashCommand);
        }
        let len = trimmed.chars().count();
        if len > MAX_MESSAGE_CHARS {
            return Err(HarnessError::MessageTooLong(len));
        }
        Ok(trimmed)
    }

    fn start_run(&self, text: &str, run_id: Option<String>) -> Result<String, HarnessError> {
        self.ensure_open()?;
        let message = Self::validate_message(text)?;
        let prompt_id = self.next_id("p");
        let run_id = run_id
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        {
            let mut state = lock(&self.state);
            if state.active_run_id().is_some() {
                return Err(HarnessError::RunInProgress);
            }
            let started =
                state.begin_run(&run_id, &self.session_id, &self.model, &prompt_id, now_ms());
            self.calls_in_run.store(0, Ordering::SeqCst);
            self.emit(started);
        }
        tracing::info!(target: "shodh::audit", event = "question", session = %self.session_id, run_id = %run_id, profile = %self.profile.id, model = %self.model, chars = message.chars().count(), "agent prompt");
        if let Err(e) = self.send(OutboundFrame::Prompt {
            id: prompt_id,
            message: message.to_string(),
            streaming_behavior: None,
        }) {
            let events = lock(&self.state).fail_run("The agent session has ended", now_ms());
            for event in events {
                self.emit(event);
            }
            return Err(e);
        }
        Ok(run_id)
    }
}

async fn write_loop(
    mut stdin: ChildStdin,
    mut frames: mpsc::UnboundedReceiver<OutboundFrame>,
    close: Arc<Notify>,
) {
    loop {
        tokio::select! {
            biased;
            _ = close.notified() => break,
            frame = frames.recv() => {
                let Some(frame) = frame else { break };
                let mut line = match frame.to_line() {
                    Ok(line) => line,
                    Err(e) => {
                        tracing::error!(target: "shodh::harness", error = %e, "could not encode an omp frame");
                        continue;
                    }
                };
                line.push('\n');
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    tracing::warn!(target: "shodh::harness", error = %e, "writing to omp failed");
                    break;
                }
                if let Err(e) = stdin.flush().await {
                    tracing::warn!(target: "shodh::harness", error = %e, "flushing omp stdin failed");
                    break;
                }
            }
        }
    }
    // Dropping stdin closes the pipe; omp then drains and exits.
    drop(stdin);
}

async fn read_loop(inner: Arc<Inner>, stdout: ChildStdout, ready: oneshot::Sender<()>) {
    let mut ready = Some(ready);
    let mut lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_FRAME_BYTES));
    while let Some(item) = lines.next().await {
        match item {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }
                match parse_frame(&line) {
                    Ok(frame) => inner.handle_frame(frame, &mut ready),
                    Err(e) => {
                        tracing::warn!(target: "shodh::harness", session = %inner.session_id, error = %e, "unparseable omp output; skipped")
                    }
                }
            }
            Err(LinesCodecError::MaxLineLengthExceeded) => {
                tracing::warn!(target: "shodh::harness", session = %inner.session_id, "omp frame exceeded {MAX_FRAME_BYTES} bytes; skipped");
            }
            Err(LinesCodecError::Io(e)) => {
                tracing::warn!(target: "shodh::harness", session = %inner.session_id, error = %e, "reading omp output failed");
                break;
            }
        }
    }
    inner.on_eof();
}

async fn drain_stderr(stderr: ChildStderr, tail: Arc<Mutex<VecDeque<String>>>) {
    let mut lines = BufReader::new(stderr).lines();
    // Ends at EOF or on a read error; either way the pipe is finished.
    while let Ok(Some(line)) = lines.next_line().await {
        let line = truncate_chars(line.trim(), 500);
        if line.is_empty() {
            continue;
        }
        let mut tail = lock(&tail);
        if tail.len() >= STDERR_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

/// A running omp session.
pub struct OmpSession {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for OmpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OmpSession")
            .field("session_id", &self.inner.session_id)
            .field("model", &self.inner.model)
            .field("profile", &self.inner.profile.id)
            .finish()
    }
}

impl OmpSession {
    /// Spawn omp, wait for `ready`, and register the profile's host tools.
    /// Returns the session and its event stream.
    pub async fn start(
        config: SessionConfig,
    ) -> Result<(OmpSession, mpsc::UnboundedReceiver<AgentEvent>), HarnessError> {
        let SessionConfig {
            launch,
            profile,
            registry,
        } = config;
        let process = sidecar::spawn(&launch).await?;

        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let close_writer = Arc::new(Notify::new());
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let secrets = launch
            .model
            .env
            .iter()
            .filter_map(|(_, v)| match v {
                EnvValue::Secret(s) => Some(s.expose().to_string()),
                EnvValue::Plain(_) => None,
            })
            .collect();

        let inner = Arc::new(Inner {
            session_id: launch.session_id.clone(),
            model: launch.model.model_arg.clone(),
            state: Mutex::new(NormaliserState::new(registry.catalog())),
            profile,
            registry,
            approvals: ApprovalGate::default(),
            outbound: out_tx,
            events: events_tx,
            pending: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            calls_in_run: AtomicU32::new(0),
            next_id: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            close_writer: close_writer.clone(),
            child: tokio::sync::Mutex::new(Some(process.child)),
            stderr_tail: stderr_tail.clone(),
            secrets,
        });

        tokio::spawn(write_loop(process.stdin, out_rx, close_writer));
        tokio::spawn(drain_stderr(process.stderr, stderr_tail));
        let (ready_tx, ready_rx) = oneshot::channel();
        tokio::spawn(read_loop(Arc::clone(&inner), process.stdout, ready_tx));

        let session = OmpSession { inner };
        if let Err(e) = session.initialise(ready_rx).await {
            session.shutdown_inner().await;
            return Err(e);
        }
        Ok((session, events_rx))
    }

    async fn initialise(&self, ready: oneshot::Receiver<()>) -> Result<(), HarnessError> {
        let inner = &self.inner;
        match tokio::time::timeout(READY_TIMEOUT, ready).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return Err(HarnessError::NotReady(inner.stderr_summary())),
            Err(_) => {
                return Err(HarnessError::NotReady(format!(
                    "no ready frame within {} s ({})",
                    READY_TIMEOUT.as_secs(),
                    inner.stderr_summary()
                )))
            }
        }
        inner
            .command(OutboundFrame::SetEventFilter {
                id: inner.next_id("c"),
                events: None,
                message_updates: MessageUpdateMode::Delta,
            })
            .await?;
        // Sub-agents are host-side (`delegate`, ADR 0001); omp's own sub-agent
        // frames are not consumed, so they are not requested.
        inner
            .command(OutboundFrame::SetSubagentSubscription {
                id: inner.next_id("c"),
                level: SubagentLevel::Off,
            })
            .await?;
        let tools = inner.registry.definitions(&inner.profile);
        let response = inner
            .command(OutboundFrame::SetHostTools {
                id: inner.next_id("c"),
                tools,
            })
            .await?;
        tracing::info!(
            target: "shodh::harness",
            session = %inner.session_id,
            tools = %response.data.map(|d| d.to_string()).unwrap_or_default(),
            "omp session ready"
        );
        Ok(())
    }

    async fn shutdown_inner(&self) {
        let inner = &self.inner;
        if inner.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        inner.approvals.cancel_all();
        inner.close_writer.notify_one();
        let child = inner.child.lock().await.take();
        if let Some(mut child) = child {
            match tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await {
                Ok(_) => {}
                Err(_) => {
                    if let Err(e) = child.kill().await {
                        tracing::warn!(target: "shodh::harness", session = %inner.session_id, error = %e, "killing omp failed");
                    }
                }
            }
        }
        inner.abort_inflight();
        tracing::info!(target: "shodh::harness", session = %inner.session_id, "omp session closed");
    }

    pub fn profile(&self) -> &AgentProfile {
        &self.inner.profile
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst) || self.inner.closing.load(Ordering::SeqCst)
    }

    pub fn active_run_id(&self) -> Option<String> {
        lock(&self.inner.state).active_run_id().map(str::to_string)
    }
}

impl Drop for OmpSession {
    fn drop(&mut self) {
        // Closing stdin makes omp exit; the reader then releases the child,
        // which is killed on drop if it is still alive.
        self.inner.closing.store(true, Ordering::SeqCst);
        self.inner.approvals.cancel_all();
        self.inner.close_writer.notify_one();
    }
}

#[async_trait]
impl AgentHarness for OmpSession {
    fn session_id(&self) -> &str {
        &self.inner.session_id
    }

    fn model(&self) -> &str {
        &self.inner.model
    }

    async fn prompt(&self, text: &str, run_id: Option<String>) -> Result<String, HarnessError> {
        self.inner.start_run(text, run_id)
    }

    async fn steer(&self, text: &str) -> Result<String, HarnessError> {
        let inner = &self.inner;
        inner.ensure_open()?;
        let message = Inner::validate_message(text)?;
        let prompt_id = inner.next_id("p");
        let run_id = {
            let mut state = lock(&inner.state);
            match state.active_run_id().map(str::to_string) {
                Some(run_id) => {
                    state.attach_prompt(&prompt_id);
                    // Send under the state lock so the run cannot finish in between.
                    inner.send(OutboundFrame::Prompt {
                        id: prompt_id,
                        message: message.to_string(),
                        streaming_behavior: Some(StreamingBehavior::Steer),
                    })?;
                    Some(run_id)
                }
                None => None,
            }
        };
        match run_id {
            Some(run_id) => {
                tracing::info!(target: "shodh::audit", event = "steer", session = %inner.session_id, run_id = %run_id, "agent steer");
                Ok(run_id)
            }
            None => inner.start_run(message, None),
        }
    }

    async fn abort(&self) -> Result<(), HarnessError> {
        let inner = &self.inner;
        inner.ensure_open()?;
        inner.approvals.cancel_all();
        inner
            .command(OutboundFrame::Abort {
                id: inner.next_id("c"),
            })
            .await?;
        Ok(())
    }

    fn approve(&self, step_id: &str, approved: bool) -> Result<(), HarnessError> {
        self.inner
            .approvals
            .resolve(step_id, approved)
            .map_err(|_| HarnessError::NoPendingApproval(step_id.to_string()))?;
        tracing::info!(target: "shodh::audit", event = "approval", session = %self.inner.session_id, step_id, approved, "approval decision");
        Ok(())
    }

    async fn shutdown(&self) {
        self.shutdown_inner().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_validated() {
        assert!(matches!(
            Inner::validate_message("   "),
            Err(HarnessError::EmptyMessage)
        ));
        assert!(matches!(
            Inner::validate_message(" /security scan"),
            Err(HarnessError::SlashCommand)
        ));
        assert_eq!(
            Inner::validate_message("  What is the notice period? ").unwrap(),
            "What is the notice period?"
        );
        let long = "x".repeat(MAX_MESSAGE_CHARS + 1);
        assert!(matches!(
            Inner::validate_message(&long),
            Err(HarnessError::MessageTooLong(_))
        ));
    }
}
