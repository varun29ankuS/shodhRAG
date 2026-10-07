//! `OmpSession`: one omp process driving one conversation.
//!
//! Tasks per session:
//! - **writer** — the only task that writes to omp's stdin, so concurrent
//!   tool results never interleave;
//! - **reader** — parses stdout frames in order, answers command responses,
//!   normalises frames into [`AgentEvent`]s and dispatches host tools;
//! - **stderr drain** — keeps the pipe empty and the last lines for errors;
//! - one task per in-flight host tool call (aborted on `host_tool_cancel`);
//! - with grounding on, one check per completed answer: the run is held
//!   ([`NormaliserState::set_hold_completion`]), the answer is verified
//!   against its passages ([`super::grounding`]) and the run either
//!   finishes or continues with one bounded follow-up prompt.

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
use super::events::{AgentEvent, ClaimCheck, GroundingReport, NeedCheck, ScoringMethod};
use super::grounding::followup::{self, Next, Rounds};
use super::grounding::verify::{check_needs, summarise, verify_answer, Thresholds};
use super::grounding::{AnswerMessage, Evidence, GroundingConfig, OpenedText, VerifyInput};
use super::model::EnvValue;
use super::omp::{normalise, HeldRun, NormaliserState, StepOutcome};
use super::profile::AgentProfile;
use super::protocol::{
    parse_frame, HostToolCallFrame, InboundFrame, MessageUpdateMode, OutboundFrame, ResponseFrame,
    StreamingBehavior, SubagentLevel, ToolResultPayload,
};
use super::sidecar::{self, LaunchSpec};
use super::tools::plan::UPDATE_PLAN;
use super::tools::{
    ApprovalGate, RunPassages, RunPlan, RunScope, ToolAudit, ToolCall, ToolContext, ToolRegistry,
};
use super::{truncate_chars, AgentHarness};

/// Longest stdout line accepted. omp's v1 frames are capped at 1 MiB.
const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const STDERR_TAIL_LINES: usize = 20;
/// Messages are sent as one JSONL frame; stay far below the frame limit.
pub const MAX_MESSAGE_CHARS: usize = 200_000;
/// Longest the model-backed answer check may take; after it, the check
/// falls back to word and number checks (the models keep running on their
/// blocking thread and their result is dropped).
const CHECK_TIMEOUT: Duration = Duration::from_secs(45);

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
    /// Audit log and scope for this conversation's tool calls, approvals and
    /// retrievals. `None` disables auditing (e.g. the log failed to open).
    pub audit: Option<ToolAudit>,
    /// Check every completed answer against its passages before the run
    /// ends. `None` finishes runs as soon as the model does.
    pub grounding: Option<GroundingConfig>,
}

/// The grounding state of the active run.
#[derive(Debug, Default)]
struct GroundingRun {
    run_id: String,
    rounds: Rounds,
    /// Follow-up turns so far (the next report's round).
    round: u32,
    /// Messages of the run that earlier rounds consumed.
    round_start: usize,
    /// The text blocks that are the answer, and their checks.
    answer_ids: Vec<String>,
    answer_chars: usize,
    answer_checks: Vec<ClaimCheck>,
    method: Option<ScoringMethod>,
}

struct Inner {
    session_id: String,
    model: String,
    profile: AgentProfile,
    registry: Arc<ToolRegistry>,
    audit: Option<ToolAudit>,
    approvals: ApprovalGate,
    state: Mutex<NormaliserState>,
    outbound: mpsc::UnboundedSender<OutboundFrame>,
    events: mpsc::UnboundedSender<AgentEvent>,
    pending: Mutex<HashMap<String, oneshot::Sender<ResponseFrame>>>,
    /// host_tool_call id → (step id, task)
    inflight: Mutex<HashMap<String, (String, AbortHandle)>>,
    calls_in_run: AtomicU32,
    /// Citation numbers handed out in the active run (see `search_documents`).
    passages_in_run: Arc<RunPassages>,
    /// The active run's task list (information needs are checked).
    plan_in_run: Arc<RunPlan>,
    grounding: Option<GroundingConfig>,
    grounding_run: Mutex<GroundingRun>,
    /// What the user limited the active run to.
    scope: Mutex<Arc<RunScope>>,
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

        let (events, settled) = {
            let mut state = lock(&self.state);
            let events = normalise(&frame, &mut state, now_ms());
            (events, state.take_settled())
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
        if let Some(held) = settled {
            let inner = Arc::clone(self);
            tokio::spawn(async move { inner.check_answer(held).await });
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
        // Task-list updates do not count against the per-answer budget.
        let call_index = if call.tool_name == UPDATE_PLAN {
            self.calls_in_run.load(Ordering::SeqCst)
        } else {
            self.calls_in_run.fetch_add(1, Ordering::SeqCst) + 1
        };
        let ctx = ToolContext::new(run_id, call.tool_call_id.clone(), self.events.clone())
            .with_host_call(call.id.clone(), self.outbound.clone())
            .with_run_passages(Arc::clone(&self.passages_in_run))
            .with_run_plan(Arc::clone(&self.plan_in_run))
            .with_scope(Arc::clone(&lock(&self.scope)))
            .with_audit(self.audit.clone());
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

    fn start_run(
        &self,
        text: &str,
        run_id: Option<String>,
        scope: RunScope,
    ) -> Result<String, HarnessError> {
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
            self.passages_in_run.reset();
            self.plan_in_run.reset();
            *lock(&self.grounding_run) = GroundingRun {
                run_id: run_id.clone(),
                ..GroundingRun::default()
            };
            *lock(&self.scope) = Arc::new(scope);
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

impl Inner {
    fn finish_held(&self, held: &HeldRun) {
        let events = lock(&self.state).finish_held(&held.run_id, held.generation, now_ms());
        let ended = !events.is_empty();
        for event in events {
            self.emit(event);
        }
        if ended {
            self.approvals.cancel_all();
            self.abort_inflight();
        }
    }

    /// Check a completed answer, then finish the run or continue it with
    /// one follow-up prompt (see [`followup::decide`]).
    async fn check_answer(self: Arc<Self>, held: HeldRun) {
        let Some(config) = self.grounding.clone() else {
            self.finish_held(&held);
            return;
        };
        let (round, round_start, previous_ids, previous_chars, previous_checks, rounds) = {
            let g = lock(&self.grounding_run);
            if g.run_id != held.run_id {
                drop(g);
                self.finish_held(&held);
                return;
            }
            (
                g.round,
                g.round_start,
                g.answer_ids.clone(),
                g.answer_chars,
                g.answer_checks.clone(),
                g.rounds,
            )
        };
        let messages: Vec<AnswerMessage> = held
            .messages
            .iter()
            .skip(round_start)
            .filter(|(_, text)| !text.trim().is_empty())
            .map(|(id, text)| AnswerMessage {
                id: id.clone(),
                text: text.clone(),
            })
            .collect();
        let passages: Vec<Evidence> = self
            .passages_in_run
            .all()
            .into_iter()
            .map(|p| Evidence {
                n: p.n,
                path: p.path,
                text: p.text,
                checkable: p.checkable,
            })
            .collect();
        let opened: Vec<OpenedText> = self
            .passages_in_run
            .opened()
            .into_iter()
            .map(|(path, text)| OpenedText { path, text })
            .collect();
        let needs: Vec<(String, String)> = self
            .plan_in_run
            .items()
            .into_iter()
            .filter(|i| i.need && !i.text.trim().is_empty())
            .map(|i| (i.id, i.text))
            .collect();
        let auto_repair = config.follow_ups && (config.auto_repair)();
        let scorers = (config.scorers)();
        if !messages.is_empty() || !needs.is_empty() {
            self.emit(AgentEvent::GroundingStarted {
                run_id: held.run_id.clone(),
                round,
            });
        }

        let (checks, need_checks, method) = {
            let job = (
                messages.clone(),
                passages.clone(),
                opened.clone(),
                needs.clone(),
            );
            let checked = tokio::time::timeout(
                CHECK_TIMEOUT,
                tokio::task::spawn_blocking(move || {
                    let (messages, passages, opened, needs) = job;
                    run_checks(&messages, &passages, &opened, &needs, Some(&scorers))
                }),
            )
            .await;
            match checked {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    tracing::warn!(target: "shodh::grounding", run_id = %held.run_id, error = %e, "answer check failed; using word and number checks");
                    run_checks(&messages, &passages, &opened, &needs, None)
                }
                Err(_) => {
                    tracing::warn!(target: "shodh::grounding", run_id = %held.run_id, "answer check timed out; using word and number checks");
                    run_checks(&messages, &passages, &opened, &needs, None)
                }
            }
        };

        // A follow-up turn's text replaces the answer only when it is an
        // answer in its own right.
        let new_ids: Vec<String> = messages.iter().map(|m| m.id.clone()).collect();
        let new_chars: usize = messages.iter().map(|m| m.text.chars().count()).sum();
        let (answer_ids, answer_chars, answer_checks, superseded) =
            if round == 0 || followup::replaces_previous(previous_chars, new_chars, checks.len()) {
                (new_ids.clone(), new_chars, checks, previous_ids.clone())
            } else {
                (
                    previous_ids.clone(),
                    previous_chars,
                    previous_checks,
                    new_ids.clone(),
                )
            };

        if !self.lock_still_held(&held) {
            return;
        }
        if !need_checks.is_empty() {
            let items = self.plan_in_run.set_coverage(&need_checks);
            self.emit(AgentEvent::PlanUpdated {
                run_id: held.run_id.clone(),
                items,
            });
        }
        let calls_used = self.calls_in_run.load(Ordering::SeqCst);
        let calls_left = if config.follow_ups {
            self.profile.max_tool_calls.saturating_sub(calls_used)
        } else {
            0
        };
        let next = followup::decide(
            &answer_checks,
            &need_checks,
            rounds,
            auto_repair,
            calls_left,
        );
        let report = |is_final: bool| GroundingReport {
            round,
            is_final,
            method,
            summary: summarise(&answer_checks),
            claims: answer_checks.clone(),
            needs: need_checks.clone(),
            message_ids: answer_ids.clone(),
            superseded_message_ids: superseded.clone(),
        };
        {
            let mut g = lock(&self.grounding_run);
            g.answer_ids = answer_ids.clone();
            g.answer_chars = answer_chars;
            g.answer_checks = answer_checks.clone();
            g.method = Some(method);
        }
        match next {
            Next::Finish => {
                let worth_reporting =
                    !answer_checks.is_empty() || !need_checks.is_empty() || round > 0;
                if worth_reporting {
                    let report = report(true);
                    tracing::info!(target: "shodh::grounding", run_id = %held.run_id, round, checked = report.summary.checked, supported = report.summary.supported, score = ?report.summary.score, "answer grounding");
                    self.emit(AgentEvent::Grounding {
                        run_id: held.run_id.clone(),
                        report,
                    });
                }
                self.finish_held(&held);
            }
            Next::FollowUp {
                reason,
                repair,
                missing_needs,
                prompt,
            } => {
                let missing_texts: Vec<String> = need_checks
                    .iter()
                    .filter(|n| missing_needs.contains(&n.id))
                    .map(|n| n.text.clone())
                    .collect();
                let prompt_id = self.next_id("p");
                // Under the state lock, in this order: the round is counted
                // and the revision announced before the prompt goes out, so
                // the follow-up turn can never be checked as an earlier
                // round (which would allow a second repair) and its text
                // never reaches the transcript before the revision row.
                let mut state = lock(&self.state);
                if !state.resume_held(&held.run_id, held.generation, &prompt_id) {
                    return;
                }
                {
                    let mut g = lock(&self.grounding_run);
                    g.round += 1;
                    g.round_start = held.messages.len();
                    if !repair.is_empty() {
                        g.rounds.repair += 1;
                    }
                    if !missing_needs.is_empty() {
                        g.rounds.coverage += 1;
                    }
                }
                tracing::info!(target: "shodh::grounding", run_id = %held.run_id, round = round + 1, ?reason, flagged = repair.len(), missing = missing_texts.len(), "answer follow-up turn");
                self.emit(AgentEvent::Grounding {
                    run_id: held.run_id.clone(),
                    report: report(false),
                });
                self.emit(AgentEvent::RevisionStarted {
                    run_id: held.run_id.clone(),
                    round: round + 1,
                    reason,
                    flagged: u32::try_from(repair.len()).unwrap_or(u32::MAX),
                    missing_needs: missing_texts,
                });
                let sent = self.send(OutboundFrame::Prompt {
                    id: prompt_id,
                    message: prompt,
                    streaming_behavior: None,
                });
                if sent.is_err() {
                    let events = state.fail_run("The agent session has ended", now_ms());
                    drop(state);
                    for event in events {
                        self.emit(event);
                    }
                }
            }
        }
    }

    fn lock_still_held(&self, held: &HeldRun) -> bool {
        lock(&self.state).still_held(&held.run_id, held.generation)
    }
}

/// Verify the answer and check the needs; without `scorers`, by words and
/// numbers only.
fn run_checks(
    messages: &[AnswerMessage],
    passages: &[Evidence],
    opened: &[OpenedText],
    needs: &[(String, String)],
    scorers: Option<&super::grounding::ScorerSet>,
) -> (Vec<ClaimCheck>, Vec<NeedCheck>, ScoringMethod) {
    let thresholds: &Thresholds = &super::grounding::THRESHOLDS;
    let input = VerifyInput {
        messages,
        passages,
        opened,
    };
    let views = scorers.map(|s| s.scorers()).unwrap_or_default();
    let (checks, method) = verify_answer(&input, views, thresholds);
    let need_checks = check_needs(needs, passages, views.relevance, thresholds);
    (checks, need_checks, method)
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
            audit,
            grounding,
        } = config;
        let started = std::time::Instant::now();
        let process = sidecar::spawn(&launch).await?;
        let spawn_ms = started.elapsed().as_millis();

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
            state: Mutex::new({
                let mut state = NormaliserState::new(registry.catalog());
                state.set_model_warning(launch.model.warning.clone());
                state.set_hold_completion(grounding.is_some());
                state
            }),
            profile,
            registry,
            audit,
            approvals: ApprovalGate::default(),
            outbound: out_tx,
            events: events_tx,
            pending: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            calls_in_run: AtomicU32::new(0),
            passages_in_run: Arc::new(RunPassages::new()),
            plan_in_run: Arc::new(RunPlan::new()),
            grounding,
            grounding_run: Mutex::new(GroundingRun::default()),
            scope: Mutex::new(Arc::new(RunScope::default())),
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
        tracing::info!(
            target: "shodh::harness",
            session = %session.inner.session_id,
            spawn_ms,
            total_ms = started.elapsed().as_millis(),
            "omp session started"
        );
        Ok((session, events_rx))
    }

    async fn initialise(&self, ready: oneshot::Receiver<()>) -> Result<(), HarnessError> {
        let inner = &self.inner;
        let waited = std::time::Instant::now();
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
        let ready_ms = waited.elapsed().as_millis();
        let configured = std::time::Instant::now();
        // The three setup commands are independent: send them back to back
        // and await the responses together instead of one round trip each.
        // Sub-agents are host-side (`delegate`, ADR 0001); omp's own sub-agent
        // frames are not consumed, so they are not requested.
        let (_, _, response) = tokio::try_join!(
            inner.command(OutboundFrame::SetEventFilter {
                id: inner.next_id("c"),
                events: None,
                message_updates: MessageUpdateMode::Delta,
            }),
            inner.command(OutboundFrame::SetSubagentSubscription {
                id: inner.next_id("c"),
                level: SubagentLevel::Off,
            }),
            inner.command(OutboundFrame::SetHostTools {
                id: inner.next_id("c"),
                tools: inner.registry.definitions(&inner.profile),
            }),
        )?;
        tracing::info!(
            target: "shodh::harness",
            session = %inner.session_id,
            ready_ms,
            setup_ms = configured.elapsed().as_millis(),
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

    async fn prompt_scoped(
        &self,
        text: &str,
        run_id: Option<String>,
        scope: RunScope,
    ) -> Result<String, HarnessError> {
        self.inner.start_run(text, run_id, scope)
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
                    // A held run has no turn in progress to steer: the message
                    // starts the next turn of the same run.
                    let behavior = if state.is_held() {
                        None
                    } else {
                        Some(StreamingBehavior::Steer)
                    };
                    state.attach_prompt(&prompt_id);
                    // Send under the state lock so the run cannot finish in between.
                    inner.send(OutboundFrame::Prompt {
                        id: prompt_id,
                        message: message.to_string(),
                        streaming_behavior: behavior,
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
            // A new run started by steering keeps the scope of the conversation's last
            // answer (a workspace chat stays limited to the workspace's sources).
            None => {
                let scope = RunScope::clone(&lock(&inner.scope));
                inner.start_run(message, None, scope)
            }
        }
    }

    async fn abort(&self) -> Result<(), HarnessError> {
        let inner = &self.inner;
        inner.ensure_open()?;
        inner.approvals.cancel_all();
        // While its answer is being checked, omp has no turn running, so the
        // run is stopped here.
        let held_events = lock(&inner.state).abort_held(now_ms());
        if !held_events.is_empty() {
            for event in held_events {
                inner.emit(event);
            }
            inner.abort_inflight();
            return Ok(());
        }
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
    use crate::harness::events::{ClaimOutcome, RevisionReason, RunStatus};
    use crate::harness::grounding::GroundingConfig;
    use crate::harness::tools::CitedPassage;

    /// A session without an omp process: frames are fed to `handle_frame`
    /// and what the session writes to omp is read from `outbound`.
    fn offline_inner(
        grounding: Option<GroundingConfig>,
    ) -> (
        Arc<Inner>,
        mpsc::UnboundedReceiver<AgentEvent>,
        mpsc::UnboundedReceiver<OutboundFrame>,
    ) {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let registry = Arc::new(ToolRegistry::new());
        let inner = Arc::new(Inner {
            session_id: "s".into(),
            model: "test/model".into(),
            state: Mutex::new({
                let mut state = NormaliserState::new(registry.catalog());
                state.set_hold_completion(grounding.is_some());
                state
            }),
            profile: AgentProfile::assistant(),
            registry,
            audit: None,
            approvals: ApprovalGate::default(),
            outbound: out_tx,
            events: events_tx,
            pending: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            calls_in_run: AtomicU32::new(0),
            passages_in_run: Arc::new(RunPassages::new()),
            plan_in_run: Arc::new(RunPlan::new()),
            grounding,
            grounding_run: Mutex::new(GroundingRun::default()),
            scope: Mutex::new(Arc::new(RunScope::default())),
            next_id: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            close_writer: Arc::new(Notify::new()),
            child: tokio::sync::Mutex::new(None),
            stderr_tail: Arc::new(Mutex::new(VecDeque::new())),
            secrets: Vec::new(),
        });
        (inner, events_rx, out_rx)
    }

    fn feed(inner: &Arc<Inner>, line: &str) {
        inner.handle_frame(parse_frame(line).unwrap(), &mut None);
    }

    fn say(inner: &Arc<Inner>, message: &str, text: &str) {
        let frame = serde_json::json!({
            "type": "message_update",
            "messageId": message,
            "assistantMessageEvent": {"type": "text_delta", "delta": text}
        });
        feed(inner, &frame.to_string());
    }

    fn complete(inner: &Arc<Inner>, prompt_id: &str) {
        feed(
            inner,
            &format!(
                r#"{{"type":"prompt_result","id":"{prompt_id}","agentInvoked":true,"status":"completed","sessionSettled":true}}"#
            ),
        );
    }

    fn sent_prompt(out: &mut mpsc::UnboundedReceiver<OutboundFrame>) -> (String, String) {
        match out.try_recv() {
            Ok(OutboundFrame::Prompt { id, message, .. }) => (id, message),
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    async fn next_event(events: &mut mpsc::UnboundedReceiver<AgentEvent>) -> AgentEvent {
        tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("an event within 10 s")
            .expect("event channel open")
    }

    /// Events up to and including the next one `stop` accepts.
    async fn events_until(
        events: &mut mpsc::UnboundedReceiver<AgentEvent>,
        stop: impl Fn(&AgentEvent) -> bool,
    ) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        loop {
            let event = next_event(events).await;
            let done = stop(&event);
            out.push(event);
            if done {
                return out;
            }
        }
    }

    fn start(inner: &Arc<Inner>) -> String {
        inner
            .start_run(
                "What is the notice period?",
                Some("run-1".into()),
                RunScope::default(),
            )
            .unwrap();
        inner.passages_in_run.reserve(1);
        inner.passages_in_run.record(CitedPassage {
            n: 1,
            file: "msa.pdf".into(),
            path: "c:/docs/msa.pdf".into(),
            page: Some("4".into()),
            web: false,
            text: "Either party may terminate the agreement with sixty days written notice.".into(),
            checkable: true,
        });
        "run-1".into()
    }

    #[tokio::test]
    async fn a_flagged_answer_is_repaired_once_within_the_same_run() {
        let (inner, mut events, mut out) = offline_inner(Some(GroundingConfig::lexical()));
        start(&inner);
        let (first_prompt, _) = sent_prompt(&mut out);
        say(
            &inner,
            "m1",
            "Either party may terminate the agreement with 90 days written notice [1].",
        );
        complete(&inner, &first_prompt);

        let round0 = events_until(&mut events, |e| {
            matches!(e, AgentEvent::RevisionStarted { .. })
        })
        .await;
        let report = round0
            .iter()
            .find_map(|e| match e {
                AgentEvent::Grounding { report, .. } => Some(report.clone()),
                _ => None,
            })
            .expect("a grounding report");
        assert!(!report.is_final);
        assert_eq!(report.claims[0].outcome, ClaimOutcome::Unsupported);
        assert_eq!(report.claims[0].missing_numbers, vec!["90"]);
        assert!(matches!(
            round0.last(),
            Some(AgentEvent::RevisionStarted {
                round: 1,
                reason: RevisionReason::Repair,
                flagged: 1,
                ..
            })
        ));
        assert!(
            !round0
                .iter()
                .any(|e| matches!(e, AgentEvent::RunFinished { .. })),
            "the run stays open"
        );
        let (repair_prompt, message) = sent_prompt(&mut out);
        assert!(message.contains("does not contain 90"), "{message}");

        say(
            &inner,
            "m2",
            "Either party may terminate the agreement with 60 days written notice [1].",
        );
        complete(&inner, &repair_prompt);
        let round1 =
            events_until(&mut events, |e| matches!(e, AgentEvent::RunFinished { .. })).await;
        let last = round1
            .iter()
            .find_map(|e| match e {
                AgentEvent::Grounding { report, .. } => Some(report.clone()),
                _ => None,
            })
            .expect("a final report");
        assert!(last.is_final);
        assert_eq!(last.round, 1);
        assert_eq!(last.message_ids, vec!["m2"]);
        assert_eq!(
            last.superseded_message_ids,
            vec!["m1"],
            "the draft is kept, replaced"
        );
        assert_eq!(last.summary.supported, 1);
        assert!(matches!(
            round1.last(),
            Some(AgentEvent::RunFinished {
                status: RunStatus::Completed,
                ..
            })
        ));
        assert!(out.try_recv().is_err(), "no further prompt");
    }

    #[tokio::test]
    async fn a_second_failure_is_flagged_not_repaired_again() {
        let (inner, mut events, mut out) = offline_inner(Some(GroundingConfig::lexical()));
        start(&inner);
        let (first_prompt, _) = sent_prompt(&mut out);
        say(&inner, "m1", "The fee is 900 EUR per year [1].");
        complete(&inner, &first_prompt);
        events_until(&mut events, |e| {
            matches!(e, AgentEvent::RevisionStarted { .. })
        })
        .await;
        let (repair_prompt, _) = sent_prompt(&mut out);
        say(&inner, "m2", "The fee is 950 EUR per year [1].");
        complete(&inner, &repair_prompt);
        let round1 =
            events_until(&mut events, |e| matches!(e, AgentEvent::RunFinished { .. })).await;
        let last = round1
            .iter()
            .find_map(|e| match e {
                AgentEvent::Grounding { report, .. } => Some(report.clone()),
                _ => None,
            })
            .unwrap();
        assert!(last.is_final);
        assert_eq!(last.claims[0].outcome, ClaimOutcome::Unsupported);
        assert!(!round1
            .iter()
            .any(|e| matches!(e, AgentEvent::RevisionStarted { .. })));
        assert!(out.try_recv().is_err(), "never a second repair turn");
    }

    #[tokio::test]
    async fn stopping_during_the_check_ends_the_run() {
        let (inner, _events, mut out) = offline_inner(Some(GroundingConfig::lexical()));
        start(&inner);
        let (first_prompt, _) = sent_prompt(&mut out);
        say(&inner, "m1", "Hello.");
        // Hold the run, then stop it before the check acts.
        {
            let mut state = lock(&inner.state);
            let frame = parse_frame(&format!(
                r#"{{"type":"prompt_result","id":"{first_prompt}","agentInvoked":true,"status":"completed"}}"#
            ))
            .unwrap();
            normalise(&frame, &mut state, now_ms());
            assert!(state.is_held());
            let ended = state.abort_held(now_ms());
            assert!(matches!(
                ended.last(),
                Some(AgentEvent::RunFinished {
                    status: RunStatus::Aborted,
                    ..
                })
            ));
        }
    }

    #[tokio::test]
    async fn without_grounding_runs_finish_at_once() {
        let (inner, mut events, mut out) = offline_inner(None);
        start(&inner);
        let (first_prompt, _) = sent_prompt(&mut out);
        say(&inner, "m1", "The notice period is 90 days [1].");
        complete(&inner, &first_prompt);
        let all = events_until(&mut events, |e| matches!(e, AgentEvent::RunFinished { .. })).await;
        assert!(!all
            .iter()
            .any(|e| matches!(e, AgentEvent::Grounding { .. })));
    }

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
