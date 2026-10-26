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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::AbortHandle;
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};

use super::code_mode::{
    approval_for, auto_approval, change_target, changed_between, code_catalog, command_eligible,
    create_branch, git_state, inventory_matches, note_review, parse_guard_request, plan_change,
    target_in_folder, tree_snapshot, ApprovalLevel, ChangePlan, ChangeTarget, CodeBranch,
    CodeBranchStore, CodeFolder, CodeSettings, GateContext, GuardRequest, ReviewState, ReviewStore,
    TreeSnapshot, ALLOW_ANSWER, CODE_TOOLS, GUARD_COMMAND, IGNORE_RULES_UNDO, OMP_EXECUTION_TOOLS,
    UNTRACKED_UNDO,
};
use super::error::HarnessError;
use super::events::{
    AgentEvent, ClaimCheck, GroundingReport, NeedCheck, RevisionReason, RiskTier, ScoringMethod,
};
use super::grounding::absence::flag_unverified_absence;
use super::grounding::followup::{self, Next, Retrieval, RetrieveResult, Rounds};
use super::grounding::verify::{check_needs, summarise, verify_answer, Thresholds};
use super::grounding::{AnswerMessage, Evidence, GroundingConfig, OpenedText, VerifyInput};
use super::model::EnvValue;
use super::omp::{normalise, HeldRun, NormaliserState, StepOutcome};
use super::profile::AgentProfile;
use super::protocol::{
    parse_frame, ExtensionUiRequestFrame, HostToolCallFrame, InboundFrame, MessageUpdateMode,
    OutboundFrame, ResponseFrame, StreamingBehavior, SubagentLevel, ToolResultPayload,
};
use super::run_command::{
    self, asked_timeout, CommandExecutor, CommandLimits, CommandSpec, RunControl, RUN_COMMAND,
};
use super::sidecar::{self, LaunchSpec};
use super::tools::plan::UPDATE_PLAN;
use super::tools::search::{truncate_query, SEARCH_DOCUMENTS};
use super::tools::{
    unset_markers_as_null, ApprovalDecision, ApprovalGate, DispatchOutcome, RunPassages, RunPlan,
    RunScope, ToolAudit, ToolCall, ToolContext, ToolError, ToolRegistry,
};
use super::{truncate_chars, AgentHarness};
use crate::audit::{payload as audit_payload, AuditEventType, LOCAL_OWNER, SYSTEM_PRINCIPAL};

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
/// How long a Code session waits for its guard to announce itself.
const GUARD_TIMEOUT: Duration = Duration::from_secs(15);

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
    /// Code mode (with `launch.code` set): the conversation whose branch the
    /// session works on. `None` for Research sessions.
    pub code: Option<CodeSession>,
}

/// Code mode settings of a session; its folder is `LaunchSpec::code`.
#[derive(Debug, Clone)]
pub struct CodeSession {
    pub conversation_id: String,
    pub branches: CodeBranchStore,
    /// The workspace's data folder, whose `code.json` sets the approval
    /// level ([`CodeSettings`]); read at every decision, so a change applies
    /// to the running session. `None`: every change asks.
    pub settings_dir: Option<std::path::PathBuf>,
    /// Runs the session's `run_command` calls
    /// ([`run_command::ContainedExecutor`] in the app).
    pub executor: Arc<dyn CommandExecutor>,
    /// The conversation's scratch folder: its commands' home and temporary
    /// folder ([`run_command::scratch_dir`]).
    pub scratch: std::path::PathBuf,
}

/// A command allowed to run whose end the session waits for: the folder as
/// it was before (or why that is not known), compared once it ends.
struct RunningCommand {
    label: String,
    before: Result<TreeSnapshot, String>,
}

/// A comparison of the code folder still to be recorded. While any is
/// pending, decisions wait ([`CodeRuntime::settled`]), so the next command
/// is never decided before the last one's changes are listed. Dropped
/// before [`PendingCompare::done`] (the task was stopped), what changed is
/// recorded as unknown.
struct PendingCompare {
    code: Arc<CodeRuntime>,
    /// The call compared ([`ReviewState::uncompared`]).
    id: String,
    what: String,
    done: bool,
}

impl PendingCompare {
    fn new(code: &Arc<CodeRuntime>, id: String, what: String) -> Self {
        code.compares.send_modify(|pending| *pending += 1);
        PendingCompare {
            code: Arc::clone(code),
            id,
            what,
            done: false,
        }
    }

    fn done(mut self) {
        self.done = true;
    }
}

impl Drop for PendingCompare {
    fn drop(&mut self) {
        if !self.done {
            let reason = format!(
                "{} stopped before Shodh could see what it changed",
                self.what
            );
            let id = self.id.clone();
            if let Err(e) = self
                .code
                .reviews
                .update(&self.code.conversation_id, |state| {
                    state.compared(&id, Err(reason))
                })
            {
                tracing::warn!(target: "shodh::harness", error = %e, "could not record an unknown change; the record is unreadable, so commands ask");
            }
        }
        self.code
            .compares
            .send_modify(|pending| *pending = pending.saturating_sub(1));
    }
}

/// A Code session's state: its folder and branch, the guard's presence,
/// open approval dialogs and running tool calls (for the audit log), and
/// what the user has not reviewed ([`ReviewState`]).
struct CodeRuntime {
    folder: CodeFolder,
    conversation_id: String,
    branches: CodeBranchStore,
    reviews: ReviewStore,
    settings_dir: Option<std::path::PathBuf>,
    /// One change decision at a time: planning, the prompt and the branch
    /// switch of a change finish before the next change is considered.
    turn: tokio::sync::Mutex<()>,
    guard_loaded: tokio::sync::watch::Sender<bool>,
    /// Open approval dialog id → step id.
    dialogs: Mutex<HashMap<String, String>>,
    /// Running tool call id → (tool, arguments, start).
    calls: Mutex<HashMap<String, (String, serde_json::Value, std::time::Instant)>>,
    /// Commands allowed to run that have not ended, by tool call id.
    running: Mutex<HashMap<String, RunningCommand>>,
    /// Comparisons of the folder not yet recorded ([`PendingCompare`]).
    compares: tokio::sync::watch::Sender<usize>,
    executor: Arc<dyn CommandExecutor>,
    scratch: std::path::PathBuf,
}

impl CodeRuntime {
    fn new(folder: CodeFolder, session: CodeSession) -> Self {
        let CodeSession {
            conversation_id,
            branches,
            settings_dir,
            executor,
            scratch,
        } = session;
        CodeRuntime {
            folder,
            conversation_id,
            reviews: ReviewStore::beside(&branches),
            branches,
            settings_dir,
            executor,
            scratch,
            turn: tokio::sync::Mutex::new(()),
            guard_loaded: tokio::sync::watch::channel(false).0,
            dialogs: Mutex::new(HashMap::new()),
            calls: Mutex::new(HashMap::new()),
            running: Mutex::new(HashMap::new()),
            compares: tokio::sync::watch::channel(0).0,
        }
    }

    /// The workspace's Code settings now (asking for everything when they
    /// cannot be read).
    fn settings(&self) -> CodeSettings {
        let ask = || CodeSettings {
            approval: ApprovalLevel::AskEveryTime,
            ..CodeSettings::default()
        };
        let Some(dir) = &self.settings_dir else {
            return ask();
        };
        CodeSettings::load(dir).unwrap_or_else(|e| {
            tracing::warn!(target: "shodh::harness", error = %e, "code settings unreadable; every change asks");
            ask()
        })
    }

    /// What the next change needs (git and the store are read off the runtime).
    async fn plan(&self) -> ChangePlan {
        let root = self.folder.root().to_path_buf();
        let folder = self.folder.display();
        let store = self.branches.clone();
        let conversation = self.conversation_id.clone();
        tokio::task::spawn_blocking(move || {
            let recorded = store.get(&conversation).unwrap_or_else(|e| {
                tracing::warn!(target: "shodh::harness", error = %e, "code branch record unreadable");
                None
            });
            plan_change(&git_state(&root), recorded.as_ref(), &folder)
        })
        .await
        .unwrap_or_else(|e| ChangePlan::Refuse(format!("Checking the code folder failed: {e}")))
    }

    /// The file an edit or write of `request` lands in, classified
    /// ([`change_target`]; git is asked off the runtime). Commands have none.
    async fn target(&self, request: &GuardRequest, plan: &ChangePlan) -> ChangeTarget {
        if !matches!(request.tool.as_str(), "edit" | "write") {
            return ChangeTarget::Unknown;
        }
        let root = self.folder.root().to_path_buf();
        let request = request.clone();
        let in_repo = !matches!(plan, ChangePlan::NotRepo);
        tokio::task::spawn_blocking(move || change_target(&root, &request, in_repo))
            .await
            .unwrap_or(ChangeTarget::Unknown)
    }

    /// Wait until every comparison of the folder is recorded.
    async fn settled(&self) {
        let mut pending = self.compares.subscribe();
        // The sender lives as long as `self`, so the wait cannot fail.
        let _ = pending.wait_for(|count| *count == 0).await;
    }

    /// What the user has not reviewed in this conversation (read off the
    /// runtime). A record that cannot be read counts as changed.
    async fn review(&self) -> ReviewState {
        let store = self.reviews.clone();
        let conversation = self.conversation_id.clone();
        tokio::task::spawn_blocking(move || {
            store
                .get(&conversation)
                .unwrap_or_else(|e| ReviewState::unreadable(&e))
        })
        .await
        .unwrap_or_else(|e| ReviewState::unreadable(&format!("reading it did not finish: {e}")))
    }

    /// Change what the user has not reviewed (off the runtime).
    async fn update_review<F>(&self, change: F) -> Result<(), String>
    where
        F: FnOnce(&mut ReviewState) + Send + 'static,
    {
        let store = self.reviews.clone();
        let conversation = self.conversation_id.clone();
        tokio::task::spawn_blocking(move || store.update(&conversation, change).map(|_| ()))
            .await
            .map_err(|e| format!("recording what changed did not finish: {e}"))?
    }

    /// `request`'s file relative to the folder (`/`-separated), when the
    /// guard named one inside it.
    fn relative_target(&self, request: &GuardRequest) -> Option<String> {
        target_in_folder(self.folder.root(), request).map(|relative| {
            relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
    }

    /// Record that `request`'s file changed without asking, so "Discard
    /// changes" commits it even if git later ignores it, and so commands ask
    /// until the user approves one whose prompt lists it ([`ReviewState`]).
    async fn remember_auto_applied(&self, request: &GuardRequest) -> Result<(), String> {
        let relative = self
            .relative_target(request)
            .ok_or_else(|| "the changed file is not known".to_string())?;
        let store = self.branches.clone();
        let conversation = self.conversation_id.clone();
        let file = relative.clone();
        tokio::task::spawn_blocking(move || {
            let mut record = store
                .get(&conversation)?
                .ok_or_else(|| "the conversation has no branch".to_string())?;
            if !record.auto_applied.contains(&file) {
                record.auto_applied.push(file);
                store.put(&conversation, record)?;
            }
            Ok::<(), String>(())
        })
        .await
        .map_err(|e| format!("recording the change did not finish: {e}"))??;
        self.update_review(move |state| state.add_files([relative]))
            .await
    }

    /// Switch to a new branch and record it for the conversation.
    async fn switch_branch(&self) -> Result<CodeBranch, String> {
        let folder = self.folder.clone();
        let store = self.branches.clone();
        let conversation = self.conversation_id.clone();
        tokio::task::spawn_blocking(move || {
            let branch = create_branch(&folder, &chrono::Utc::now().to_rfc3339())?;
            store.put(&conversation, branch.clone())?;
            Ok(branch)
        })
        .await
        .map_err(|e| format!("the branch switch did not finish: {e}"))?
    }

    /// What git shows of the folder now (off the runtime).
    async fn snapshot(&self) -> Result<TreeSnapshot, String> {
        let root = self.folder.root().to_path_buf();
        tokio::task::spawn_blocking(move || tree_snapshot(&root))
            .await
            .map_err(|e| format!("looking at the folder did not finish: {e}"))?
    }

    /// A command allowed to run that has not ended, if any (its label).
    fn running_label(&self) -> Option<String> {
        lock(&self.running)
            .values()
            .next()
            .map(|running| running.label.clone())
    }

    /// The command of `request` is allowed: note how the folder looks, and
    /// record, before it may start, that its changes are not compared yet
    /// ([`ReviewState::uncompared`]; removed when it ends,
    /// [`CodeRuntime::command_ended`]). `Err` when that cannot be recorded:
    /// the command must not run.
    async fn command_allowed(&self, request: &GuardRequest) -> Result<(), String> {
        let before = self.snapshot().await;
        let command = request
            .input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let label = format!(
            "`{}`",
            truncate_chars(
                &command.split_whitespace().collect::<Vec<_>>().join(" "),
                80
            )
        );
        let id = request.tool_call_id.clone();
        let entry = label.clone();
        self.update_review(move |state| state.start_call(&id, &entry))
            .await?;
        lock(&self.running).insert(
            request.tool_call_id.clone(),
            RunningCommand { label, before },
        );
        Ok(())
    }

    /// The tool call `tool_call_id` ended: when it was a command allowed to
    /// run, list what it changed in the folder (compared off the runtime).
    fn command_ended(self: &Arc<Self>, tool_call_id: &str) {
        let Some(running) = lock(&self.running).remove(tool_call_id) else {
            return;
        };
        let pending = PendingCompare::new(
            self,
            tool_call_id.to_string(),
            format!("The command {}", running.label),
        );
        let code = Arc::clone(self);
        let id = tool_call_id.to_string();
        tokio::spawn(async move {
            let what = format!("the command {}", running.label);
            code.record_changes(&id, running.before, &what).await;
            pending.done();
        });
    }

    /// Every command still listed as running ended with the run or the
    /// session: list what they changed.
    fn commands_ended(self: &Arc<Self>) {
        let ids: Vec<String> = lock(&self.running).keys().cloned().collect();
        for id in ids {
            self.command_ended(&id);
        }
    }

    /// Compare the folder with `before` and record the result of call `id`:
    /// what changed is listed as not reviewed, or why that cannot be told.
    async fn record_changes(&self, id: &str, before: Result<TreeSnapshot, String>, what: &str) {
        let after = self.snapshot().await;
        let root = self.folder.root().to_path_buf();
        let changed = match (before, after) {
            (Ok(before), Ok(after)) => {
                tokio::task::spawn_blocking(move || changed_between(&root, &before, &after))
                    .await
                    .unwrap_or_else(|e| Err(format!("comparing did not finish: {e}")))
            }
            (Err(e), _) | (_, Err(e)) => Err(e),
        };
        let changed = changed.map_err(|e| format!("after {what}, {e}"));
        let id = id.to_string();
        if let Err(e) = self
            .update_review(move |state| state.compared(&id, changed))
            .await
        {
            tracing::warn!(target: "shodh::harness", error = %e, "could not record what changed; the record is unreadable, so commands ask");
        }
    }
}

/// The grounding state of the active run.
#[derive(Debug, Default)]
struct GroundingRun {
    run_id: String,
    /// The user's question ([`RunScope::question`], else the sent message):
    /// what the check searches the documents for when the answer did not.
    question: String,
    /// Citation numbers the run started with (passages carried into a side
    /// discussion): numbers above it were handed out by tools of this run.
    seeded: u32,
    /// The message only asked to remember something ([`RunScope::memory_instruction`]).
    memory_only: bool,
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
    /// Tools that succeeded in the active run: a claim that the user has
    /// none of some data stands only on one that reads it
    /// ([`flag_unverified_absence`]).
    tools_ok_in_run: Mutex<HashSet<String>>,
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
    /// Why the host closed the session, shown as the error of an answer it
    /// stops ([`OmpSession::shutdown_because`]); `None`: "The session was
    /// closed".
    close_reason: Mutex<Option<String>>,
    closed: AtomicBool,
    close_writer: Arc<Notify>,
    child: tokio::sync::Mutex<Option<Child>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    secrets: Vec<String>,
    code: Option<Arc<CodeRuntime>>,
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
            OutboundFrame::GetState { .. } => "get_state",
            OutboundFrame::HostToolUpdate { .. }
            | OutboundFrame::HostToolResult { .. }
            | OutboundFrame::ExtensionUiResponse { .. } => "frame",
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
            InboundFrame::AvailableCommands(update) => {
                if let Some(code) = &self.code {
                    let loaded = update.commands.iter().any(|c| {
                        c.name == GUARD_COMMAND && c.source.as_deref() == Some("extension")
                    });
                    code.guard_loaded.send_replace(loaded);
                }
            }
            // Host tools are audited by the registry; these are omp's own.
            InboundFrame::ToolExecutionStart(start)
                if CODE_TOOLS.contains(&start.tool_name.as_str()) =>
            {
                if let Some(code) = &self.code {
                    lock(&code.calls).insert(
                        start.tool_call_id.clone(),
                        (
                            start.tool_name.clone(),
                            start.args.clone(),
                            std::time::Instant::now(),
                        ),
                    );
                }
            }
            InboundFrame::ToolExecutionEnd(end) => {
                if let Some(code) = &self.code {
                    // Before the next frame (the next decision) is handled.
                    code.command_ended(&end.tool_call_id);
                    let call = lock(&code.calls).remove(&end.tool_call_id);
                    let run_id = lock(&self.state).active_run_id().map(str::to_string);
                    if let (Some((tool, args, started)), Some(run_id)) = (call, run_id) {
                        let text = end
                            .result
                            .as_ref()
                            .map(ToolResultPayload::joined_text)
                            .unwrap_or_default();
                        let summary = text.lines().map(str::trim).find(|l| !l.is_empty());
                        let tier = code_catalog().get(&tool).map(|m| m.tier);
                        self.audit(
                            &run_id,
                            AuditEventType::ToolCall,
                            audit_payload::tool_call(
                                &tool,
                                tier,
                                &args,
                                !end.is_error,
                                &truncate_chars(summary.unwrap_or(""), 160),
                                u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                            ),
                        );
                    }
                }
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
            if let Some(code) = &self.code {
                code.commands_ended();
            }
        }
        if let Some(held) = settled {
            let inner = Arc::clone(self);
            tokio::spawn(async move { inner.check_answer(held).await });
        }

        match frame {
            InboundFrame::HostToolCall(call) => self.dispatch_host_tool(call),
            InboundFrame::HostToolCancel(cancel) => self.cancel_host_tool(&cancel.target_id),
            InboundFrame::ExtensionUiRequest(request) => self.handle_ui_request(request),
            _ => {}
        }
    }

    /// Queue an audit event for `run_id` in the session's scope (no-op
    /// without an audit log).
    fn audit(&self, run_id: &str, event_type: AuditEventType, payload: serde_json::Value) {
        if let Some(audit) = &self.audit {
            audit
                .log
                .submit(audit.scope.record(run_id, event_type, payload));
        }
    }

    fn principal(&self) -> &str {
        self.audit
            .as_ref()
            .map(|a| a.scope.principal.as_str())
            .unwrap_or(LOCAL_OWNER)
    }

    /// omp's dialog requests. A Code session answers its guard's approval
    /// requests; every other dialog is declined at once so omp never waits
    /// on a prompt nobody sees (omp's own tool prompts are switched off in
    /// Code mode, so one appearing is refused, not approved).
    fn handle_ui_request(self: &Arc<Self>, request: ExtensionUiRequestFrame) {
        if request.method == "cancel" {
            if let (Some(code), Some(target)) = (&self.code, &request.target_id) {
                let step = lock(&code.dialogs).remove(target);
                if let Some(step) = step {
                    self.approvals.cancel(&step);
                }
            }
            return;
        }
        if !request.is_dialog() {
            return;
        }
        let guarded = self.code.as_ref().and_then(|code| {
            parse_guard_request(
                &request.id,
                request.title.as_deref(),
                request.placeholder.as_deref(),
            )
            .map(|guarded| (Arc::clone(code), guarded))
        });
        match guarded {
            Some((code, guarded)) => {
                let inner = Arc::clone(self);
                let ui_id = request.id.clone();
                tokio::spawn(async move {
                    let answer = match inner.decide_guarded(&code, &guarded).await {
                        Ok(_) => ALLOW_ANSWER.to_string(),
                        Err(reason) => reason,
                    };
                    if inner
                        .send(OutboundFrame::ExtensionUiResponse {
                            id: ui_id,
                            value: Some(answer),
                            cancelled: false,
                        })
                        .is_err()
                    {
                        tracing::debug!(target: "shodh::harness", "omp writer closed before an approval answer");
                    }
                });
            }
            None => {
                let title = request.title.as_deref().unwrap_or("");
                tracing::warn!(target: "shodh::harness", session = %self.session_id, method = %request.method, title = %truncate_chars(title, 120), "omp dialog declined");
                if let Some(run_id) = lock(&self.state).active_run_id() {
                    if title.starts_with("Allow tool:") {
                        self.audit(
                            run_id,
                            AuditEventType::Approval,
                            json!({
                                "tool": title.lines().next().unwrap_or(title).trim_start_matches("Allow tool:").trim(),
                                "label": truncate_chars(title, 300),
                                "decision": "refused",
                                "who": SYSTEM_PRINCIPAL,
                                "reason": "the runtime asked outside Shodh's approval prompt",
                            }),
                        );
                    }
                }
                if self
                    .send(OutboundFrame::ExtensionUiResponse {
                        id: request.id,
                        value: None,
                        cancelled: true,
                    })
                    .is_err()
                {
                    tracing::debug!(target: "shodh::harness", "omp writer closed before a dialog answer");
                }
            }
        }
    }

    /// Decide one guarded change (an edit or write the guard asked about, or
    /// a `run_command` call): check the code folder, ask the user, and switch
    /// to the conversation's branch first when this is the first change.
    /// `Ok` carries the settings it was decided under (a command runs with
    /// the limits its prompt showed); `Err` the reason the model is given.
    async fn decide_guarded(
        &self,
        code: &CodeRuntime,
        request: &GuardRequest,
    ) -> Result<CodeSettings, String> {
        let (run_id, opened) = {
            let mut state = lock(&self.state);
            let run_id = state.active_run_id().map(str::to_string);
            let opened = state.open_step(
                &request.tool_call_id,
                &request.tool,
                &request.input,
                now_ms(),
            );
            (run_id, opened)
        };
        let Some(run_id) = run_id else {
            return Err("No answer is running.".to_string());
        };
        if let Some(event) = opened {
            self.emit(event);
        }
        let _turn = code.turn.lock().await;
        // What earlier commands and tools changed is listed before anything
        // is decided on it.
        code.settled().await;
        let plan = code.plan().await;
        let undo = match &plan {
            ChangePlan::Refuse(reason) => {
                self.refuse(&run_id, request, ChangeTarget::Unknown, reason);
                return Err(reason.clone());
            }
            ChangePlan::Continue(branch) => format!(
                "On branch {}; \"Discard changes\" returns to {}.",
                branch.branch, branch.base
            ),
            ChangePlan::CreateBranch => "Shodh first switches to a new branch, so this can be \
                                         undone with \"Discard changes\"."
                .to_string(),
            ChangePlan::NotRepo => {
                "Not possible: the code folder is not a git repository.".to_string()
            }
        };
        let target = code.target(request, &plan).await;
        if target == ChangeTarget::GitFolder {
            let reason = "Code mode does not change files in the repository's .git folder.";
            self.refuse(&run_id, request, target, reason);
            return Err(reason.to_string());
        }
        // An ignored file is not in the branch's commit: Discard cannot restore
        // it. A .gitignore decides what the commit takes in.
        let undo = match target {
            _ if plan == ChangePlan::NotRepo => undo,
            _ if target.untracked() => UNTRACKED_UNDO.to_string(),
            ChangeTarget::IgnoreRules => IGNORE_RULES_UNDO.to_string(),
            _ => undo,
        };
        let settings = code.settings();
        let review = code.review().await;
        let running = code.running_label();
        let unasked_writer = self.registry.unasked_file_writer(&self.profile);
        let gate = GateContext {
            review: &review,
            running: running.as_deref(),
            unasked_writer: unasked_writer.as_deref(),
        };
        // Decided once, before the prompt: the settings may change while it
        // waits, and what is cleared must match what was shown.
        let eligible = command_eligible(&settings, request, &plan);
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut approval = approval_for(request, Some(&undo), target);
        if request.tool == RUN_COMMAND {
            // The limits the command runs with, from these same settings.
            let limits = CommandLimits::for_call(&settings, asked_timeout(&request.input));
            if let Some(map) = approval.preview.as_object_mut() {
                for (key, text) in limits.prompt_rows() {
                    map.insert(key.to_string(), json!(text));
                }
            }
        }
        let shown = note_review(&mut approval, request, &gate, eligible, &token);
        if let Some(reason) = auto_approval(&settings, request, &plan, target, &gate) {
            self.audit(
                &run_id,
                AuditEventType::Approval,
                json!({
                    "tool": request.tool,
                    "label": approval.label,
                    "decision": "auto_approved",
                    "who": SYSTEM_PRINCIPAL,
                    "reason": reason,
                }),
            );
            if plan == ChangePlan::CreateBranch {
                let branch = code.switch_branch().await.map_err(|e| {
                    format!(
                        "Shodh could not switch to a new branch, so the change was not made: {e}"
                    )
                })?;
                self.announce_branch(&run_id, &branch);
            }
            if matches!(request.tool.as_str(), "edit" | "write") {
                code.remember_auto_applied(request).await.map_err(|e| {
                    format!(
                        "Shodh could not record the change for \"Discard changes\", so it was \
                         not made: {e}"
                    )
                })?;
            } else {
                code.command_allowed(request).await.map_err(|e| {
                    format!("Shodh could not record this command, so it was not run: {e}")
                })?;
            }
            return Ok(settings);
        }
        if let Some(ui_id) = &request.ui_id {
            lock(&code.dialogs).insert(ui_id.clone(), request.tool_call_id.clone());
        }
        self.emit(AgentEvent::ApprovalRequested {
            run_id: run_id.clone(),
            step_id: request.tool_call_id.clone(),
            tool: request.tool.clone(),
            label: approval.label.clone(),
            tier: approval.tier,
            preview: approval.preview.clone(),
        });
        let decision = self.approvals.wait(&request.tool_call_id).await;
        let token_back = self.approvals.take_review_token(&request.tool_call_id);
        if let Some(ui_id) = &request.ui_id {
            lock(&code.dialogs).remove(ui_id);
        }
        let who = match decision {
            ApprovalDecision::Approved | ApprovalDecision::Denied => self.principal(),
            ApprovalDecision::TimedOut | ApprovalDecision::Cancelled => SYSTEM_PRINCIPAL,
        };
        self.audit(
            &run_id,
            AuditEventType::Approval,
            audit_payload::approval(&request.tool, &approval.label, approval.tier, decision, who),
        );
        match decision {
            ApprovalDecision::Approved => {}
            ApprovalDecision::Denied => {
                return Err(
                    "The user declined this. Do not attempt it another way; ask the \
                            user how to continue."
                        .to_string(),
                )
            }
            ApprovalDecision::TimedOut => {
                return Err("Nobody approved this in time, so it was not done.".to_string())
            }
            ApprovalDecision::Cancelled => {
                return Err("The answer was stopped before this was approved.".to_string())
            }
        }
        if plan == ChangePlan::CreateBranch {
            let branch = code.switch_branch().await.map_err(|e| {
                format!("Shodh could not switch to a new branch, so the change was not made: {e}")
            })?;
            self.announce_branch(&run_id, &branch);
        }
        if matches!(request.tool.as_str(), "edit" | "write") {
            // The user saw only part of the content: no review of the file.
            if approval.preview_cut {
                let file = code.relative_target(request);
                code.update_review(move |state| match file {
                    Some(file) => state.add_files([file]),
                    None => state.mark_unknown(
                        "a change whose preview was cut landed in a file Shodh could not name"
                            .to_string(),
                    ),
                })
                .await
                .map_err(|e| {
                    format!(
                        "Shodh could not record that this change was not fully shown, so it \
                         was not made: {e}"
                    )
                })?;
            }
            return Ok(settings);
        }
        // Only the prompt that showed every listed file sends the token back
        // (not the Inbox, not a shortcut): then exactly what it showed counts
        // as reviewed. If this cannot be recorded, later commands ask again.
        if let Some(shown) = shown.filter(|_| token_back.as_deref() == Some(token.as_str())) {
            if let Err(e) = code
                .update_review(move |state| state.clear_shown(&shown))
                .await
            {
                tracing::warn!(target: "shodh::harness", session = %self.session_id, error = %e, "could not record the review of unreviewed files; commands keep asking");
            }
        }
        // Nothing a command starts outlives it (its job or process group
        // ends with it), so a background process is no longer recorded as
        // running on: edits ask only while the command itself runs.
        code.command_allowed(request)
            .await
            .map_err(|e| format!("Shodh could not record this command, so it was not run: {e}"))?;
        Ok(settings)
    }

    /// Audit a guarded call Shodh refuses without asking.
    fn refuse(&self, run_id: &str, request: &GuardRequest, target: ChangeTarget, reason: &str) {
        self.audit(
            run_id,
            AuditEventType::Approval,
            json!({
                "tool": request.tool,
                "label": approval_for(request, None, target).label,
                "decision": "refused",
                "who": SYSTEM_PRINCIPAL,
                "reason": reason,
            }),
        );
    }

    /// Show and audit the switch to the conversation's branch.
    fn announce_branch(&self, run_id: &str, branch: &CodeBranch) {
        tracing::info!(target: "shodh::audit", event = "code_change", session = %self.session_id, branch = %branch.branch, base = %branch.base, "switched to the code branch");
        self.audit(
            run_id,
            AuditEventType::CodeChange,
            json!({
                "action": "created",
                "branch": branch.branch,
                "base": branch.base,
                "folder": branch.folder,
            }),
        );
        let step_id = format!("{run_id}-branch");
        self.emit(AgentEvent::StepStarted {
            run_id: run_id.to_string(),
            step_id: step_id.clone(),
            parent_step_id: None,
            tool: "git_branch".to_string(),
            label: format!("Switched to branch {}", branch.branch),
            args: json!({ "branch": branch.branch, "base": branch.base }),
            tier: RiskTier::Write,
            at_ms: now_ms(),
        });
        self.emit(AgentEvent::StepFinished {
            run_id: run_id.to_string(),
            step_id,
            ok: true,
            summary: format!(
                "This conversation's changes are on {}. \"Discard changes\" returns to {}.",
                branch.branch, branch.base
            ),
            detail: Some(json!({ "branch": branch.branch, "base": branch.base })),
            duration_ms: 0,
            invalid_arguments: false,
        });
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
        // A Code session's commands are its own, not the registry's.
        if call.tool_name == RUN_COMMAND {
            if let Some(code) = &self.code {
                self.dispatch_run_command(Arc::clone(code), call, run_id, call_index);
                return;
            }
        }
        let ctx = ToolContext::new(run_id, call.tool_call_id.clone(), self.events.clone())
            .with_host_call(call.id.clone(), self.outbound.clone())
            .with_run_passages(Arc::clone(&self.passages_in_run))
            .with_run_plan(Arc::clone(&self.plan_in_run))
            .with_scope(Arc::clone(&lock(&self.scope)))
            .with_audit(self.audit.clone());
        let host_id = call.id.clone();
        let step_id = call.tool_call_id.clone();
        let task_tool = call.tool_name.clone();
        let tool_call = ToolCall {
            tool: call.tool_name,
            args: call.arguments,
            call_index,
        };

        // A tool that can change the code folder (an MCP server's): what it
        // changed is listed as not reviewed. Registered before the task, so
        // the next decision waits for the comparison.
        let compare = self
            .code
            .as_ref()
            .filter(|_| self.registry.may_change_files(&tool_call.tool))
            .map(|code| {
                (
                    Arc::clone(code),
                    PendingCompare::new(
                        code,
                        step_id.clone(),
                        format!("The tool {}", tool_call.tool),
                    ),
                )
            });

        // Hold the map while spawning so a fast task cannot finish (and try
        // to remove its entry) before the entry exists.
        let mut inflight = lock(&self.inflight);
        let inner = Arc::clone(self);
        let task_host_id = host_id.clone();
        let task_step_id = step_id.clone();
        let handle = tokio::spawn(async move {
            let before = match &compare {
                Some((code, _)) => {
                    // Recorded before the call: one cut off by a crash still counts.
                    let id = task_step_id.clone();
                    let label = format!("the tool {task_tool}");
                    if let Err(e) = code
                        .update_review(move |state| state.start_call(&id, &label))
                        .await
                    {
                        tracing::warn!(target: "shodh::harness", error = %e, "could not record a tool call before it ran; the record is unreadable, so commands ask");
                    }
                    Some(code.snapshot().await)
                }
                None => None,
            };
            let outcome = inner
                .registry
                .dispatch(tool_call, &inner.profile, &inner.approvals, &ctx)
                .await;
            if let (Some((code, pending)), Some(before)) = (compare, before) {
                code.record_changes(&task_step_id, before, &format!("the tool {task_tool}"))
                    .await;
                pending.done();
            }
            if outcome.ok {
                lock(&inner.tools_ok_in_run).insert(task_tool);
            }
            // Record the UI summary before omp can answer with tool_execution_end.
            lock(&inner.state).record_outcome(
                &task_step_id,
                StepOutcome {
                    ok: outcome.ok,
                    summary: outcome.summary.clone(),
                    detail: outcome.detail.clone(),
                    invalid_arguments: outcome.invalid_arguments,
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

    /// Run a Code session's `run_command` call in its own task: the registry's
    /// checks (arguments, the per-answer budget, the audit record) without
    /// the registry, the decision of [`Inner::decide_guarded`] (no "allow
    /// for this conversation"), then the command in its own process with
    /// live status, and what it changed in the folder listed before the
    /// result goes back. Cancelling the task stops the command's processes.
    fn dispatch_run_command(
        self: &Arc<Self>,
        code: Arc<CodeRuntime>,
        call: HostToolCallFrame,
        run_id: String,
        call_index: u32,
    ) {
        let host_id = call.id.clone();
        let step_id = call.tool_call_id.clone();
        let mut inflight = lock(&self.inflight);
        let inner = Arc::clone(self);
        let task_host_id = host_id.clone();
        let task_step_id = step_id.clone();
        let handle = tokio::spawn(async move {
            let started = std::time::Instant::now();
            let (outcome, input) = inner
                .run_command_call(&code, &run_id, &task_step_id, call.arguments, call_index)
                .await;
            let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            if outcome.ok {
                lock(&inner.tools_ok_in_run).insert(RUN_COMMAND.to_string());
            }
            inner.audit(
                &run_id,
                AuditEventType::ToolCall,
                audit_payload::tool_call(
                    RUN_COMMAND,
                    Some(run_command::TIER),
                    &input,
                    outcome.ok,
                    &outcome.summary,
                    duration_ms,
                ),
            );
            tracing::info!(
                target: "shodh::audit",
                event = "tool_call",
                session = %inner.session_id,
                run_id = %run_id,
                step_id = %task_step_id,
                tool = RUN_COMMAND,
                ok = outcome.ok,
                summary = %outcome.summary,
                duration_ms,
                "code command"
            );
            // Record the UI summary before omp can answer with tool_execution_end.
            lock(&inner.state).record_outcome(
                &task_step_id,
                StepOutcome {
                    ok: outcome.ok,
                    summary: outcome.summary.clone(),
                    detail: outcome.detail.clone(),
                    invalid_arguments: outcome.invalid_arguments,
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
                tracing::debug!(target: "shodh::harness", "omp writer closed before a command result");
            }
        });
        inflight.insert(host_id, (step_id, handle.abort_handle()));
    }

    /// One `run_command` call, from its arguments to its result. Returns the
    /// outcome and the arguments as audited.
    async fn run_command_call(
        self: &Arc<Self>,
        code: &Arc<CodeRuntime>,
        run_id: &str,
        step_id: &str,
        arguments: serde_json::Value,
        call_index: u32,
    ) -> (DispatchOutcome, serde_json::Value) {
        let failed = |error: ToolError| DispatchOutcome {
            ok: false,
            text_for_model: error.to_string(),
            summary: error.summary(),
            detail: None,
            invalid_arguments: matches!(error, ToolError::InvalidArguments { .. }),
        };
        let arguments = unset_markers_as_null(arguments, &run_command::schema());
        let args = match run_command::parse_args(&arguments) {
            Ok(args) => args,
            Err(reasons) => {
                let error = ToolError::InvalidArguments {
                    tool: RUN_COMMAND.to_string(),
                    reasons,
                };
                return (failed(error), arguments);
            }
        };
        let input = args.input();
        if call_index > self.profile.max_tool_calls {
            let error = ToolError::BudgetExhausted {
                max: self.profile.max_tool_calls,
            };
            return (failed(error), input);
        }
        let request = GuardRequest {
            ui_id: None,
            tool_call_id: step_id.to_string(),
            tool: RUN_COMMAND.to_string(),
            input: input.clone(),
            target: None,
        };
        let settings = match self.decide_guarded(code, &request).await {
            Ok(settings) => settings,
            Err(reason) => {
                let summary = reason
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(|line| truncate_chars(line, 160))
                    .unwrap_or_else(|| "Not run".to_string());
                let outcome = DispatchOutcome {
                    ok: false,
                    text_for_model: reason,
                    summary,
                    detail: None,
                    invalid_arguments: false,
                };
                return (outcome, input);
            }
        };
        let limits = CommandLimits::for_call(&settings, args.timeout_secs);
        let command_id = uuid::Uuid::new_v4().simple().to_string();
        let spec = CommandSpec {
            command: args.command.clone(),
            folder: code.folder.root().to_path_buf(),
            scratch: code.scratch.clone(),
            limits,
        };
        let control = RunControl {
            command_id: command_id.clone(),
            on_started: Box::new({
                let events = self.events.clone();
                let (run_id, step_id) = (run_id.to_string(), step_id.to_string());
                let (command_id, command) = (command_id.clone(), args.command.clone());
                move || {
                    let started = AgentEvent::CommandStarted {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                        command_id: command_id.clone(),
                        command: command.clone(),
                        time_limit_secs: limits.time.as_secs(),
                        at_ms: now_ms(),
                    };
                    if events.send(started).is_err() {
                        tracing::debug!(target: "shodh::harness", "event receiver dropped");
                    }
                }
            }),
            on_output: Box::new({
                let events = self.events.clone();
                let (run_id, step_id) = (run_id.to_string(), step_id.to_string());
                let command_id = command_id.clone();
                move |text| {
                    let output = AgentEvent::CommandOutput {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                        command_id: command_id.clone(),
                        text,
                    };
                    if events.send(output).is_err() {
                        tracing::debug!(target: "shodh::harness", "event receiver dropped");
                    }
                }
            }),
        };
        let ran = code.executor.run(spec, control).await;
        // What it changed is listed before its result goes back, so the
        // next decision waits for the comparison.
        code.command_ended(step_id);
        self.emit(AgentEvent::CommandFinished {
            run_id: run_id.to_string(),
            step_id: step_id.to_string(),
            command_id: command_id.clone(),
            state: ran.state,
            exit_code: ran.exit_code,
            note: ran.note.clone(),
            duration_ms: u64::try_from(ran.duration.as_millis()).unwrap_or(u64::MAX),
        });
        let outcome = DispatchOutcome {
            ok: ran.succeeded(),
            text_for_model: ran.text_for_model(&limits),
            summary: truncate_chars(&ran.summary(&limits), 160),
            detail: Some(json!({
                "commandId": command_id,
                "state": ran.state,
                "exitCode": ran.exit_code,
            })),
            invalid_arguments: false,
        };
        (outcome, input)
    }

    fn cancel_host_tool(&self, host_call_id: &str) {
        let entry = lock(&self.inflight).remove(host_call_id);
        if let Some((step_id, handle)) = entry {
            handle.abort();
            self.approvals.cancel(&step_id);
            // A command stopped this way: list what it changed.
            if let Some(code) = &self.code {
                code.command_ended(&step_id);
            }
            lock(&self.state).record_outcome(
                &step_id,
                StepOutcome {
                    ok: false,
                    summary: "Cancelled".to_string(),
                    detail: None,
                    invalid_arguments: false,
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
            lock(&self.close_reason)
                .clone()
                .unwrap_or_else(|| "The session was closed".to_string())
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
        if let Some(code) = &self.code {
            code.commands_ended();
        }
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
            lock(&self.tools_ok_in_run).clear();
            self.passages_in_run.reset();
            self.passages_in_run
                .seed(&scope.carried, scope.carried_through);
            // A message the app composed, or one carrying memories, history or
            // instructions, is private content the web tools must not send
            // away unasked; the session keeps that for its later runs.
            if scope.private_context || scope.own_words.is_none() {
                self.passages_in_run.mark_private();
            }
            self.plan_in_run.reset();
            let question = scope
                .question
                .as_deref()
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .unwrap_or(message)
                .to_string();
            *lock(&self.grounding_run) = GroundingRun {
                run_id: run_id.clone(),
                question,
                seeded: self.passages_in_run.issued(),
                memory_only: scope.memory_instruction,
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
        let tools_ok: HashSet<String> = lock(&self.tools_ok_in_run).clone();
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
                tools_ok.clone(),
            );
            let checked = tokio::time::timeout(
                CHECK_TIMEOUT,
                tokio::task::spawn_blocking(move || {
                    let (messages, passages, opened, needs, tools_ok) = job;
                    run_checks(
                        &messages,
                        &passages,
                        &opened,
                        &needs,
                        &tools_ok,
                        Some(&scorers),
                    )
                }),
            )
            .await;
            match checked {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    tracing::warn!(target: "shodh::grounding", run_id = %held.run_id, error = %e, "answer check failed; using word and number checks");
                    run_checks(&messages, &passages, &opened, &needs, &tools_ok, None)
                }
                Err(_) => {
                    tracing::warn!(target: "shodh::grounding", run_id = %held.run_id, "answer check timed out; using word and number checks");
                    run_checks(&messages, &passages, &opened, &needs, &tools_ok, None)
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
        // The answer had sources of its own when a search succeeded (even
        // one that found nothing) or when any tool of the run handed out
        // passages: a web search, a fetched page, an opened document, a
        // paper or figure tool. Counting numbers beyond the seeded ones
        // covers every tool that numbers passages, including app tools,
        // without a list to keep in step with the registry; the carried
        // passages of a side discussion are not counted.
        let (seeded, memory_only) = {
            let g = lock(&self.grounding_run);
            (g.seeded, g.memory_only)
        };
        // A request to remember answered with an acknowledgement alone has no question for
        // the documents: that answer is not sent back to search them. Repair and every
        // other follow-up run as usual (an acknowledgement has no claims to repair).
        let acknowledged = memory_only && {
            let answer: Vec<&str> = messages.iter().map(|m| m.text.as_str()).collect();
            followup::is_acknowledgement(&answer.join(" "), calls_used)
        };
        let retrieval = Retrieval {
            sourced: followup::searched(&tools_ok) || self.passages_in_run.issued() > seeded,
            available: self.can_search_documents() && !acknowledged,
        };
        let next = followup::decide(
            &answer_checks,
            &need_checks,
            rounds,
            auto_repair,
            calls_left,
            retrieval,
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
            Next::Retrieve { flagged } => {
                let flagged = u32::try_from(flagged.len()).unwrap_or(u32::MAX);
                self.retrieve_for_answer(&held, round, flagged, report(false))
                    .await;
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

    /// Whether the session can search the user's documents itself: the tool
    /// is registered and the profile allows it (not in Code mode).
    fn can_search_documents(&self) -> bool {
        self.profile.allows(SEARCH_DOCUMENTS) && self.registry.describe(SEARCH_DOCUMENTS).is_some()
    }

    /// The answer cited passages, or said what the documents hold, without
    /// any search ([`Next::Retrieve`]): search the documents for the user's
    /// question through the registry, as the model's own call would be (the
    /// answer's scope, its citation numbers, the audit log, a step in the
    /// transcript), then ask for the answer again from what was found.
    /// `report` is the check of the answer being replaced.
    async fn retrieve_for_answer(
        self: &Arc<Self>,
        held: &HeldRun,
        round: u32,
        flagged: u32,
        report: GroundingReport,
    ) {
        let question = lock(&self.grounding_run).question.clone();
        let args = json!({ "query": truncate_query(&question) });
        let step_id = format!("{}-{}", held.run_id, self.next_id("check-search-"));
        // Under the state lock, as for any follow-up: the round is counted
        // (so a turn checked later is never this round again, and the
        // search is never made twice) and announced before anything of the
        // turn reaches the transcript. The search is started and registered
        // among the in-flight calls under the same lock, so a stop can never
        // fall between the check and the registration and leave a search no
        // run owns (it would number passages in the next run).
        let (task, issued_before, limited_to) = {
            let mut state = lock(&self.state);
            if !state.still_held(&held.run_id, held.generation) {
                return;
            }
            {
                let mut g = lock(&self.grounding_run);
                g.round += 1;
                g.round_start = held.messages.len();
                g.rounds.retrieve += 1;
            }
            tracing::info!(target: "shodh::grounding", run_id = %held.run_id, round = round + 1, flagged, "answer had no sources; searching the documents for the question");
            self.emit(AgentEvent::Grounding {
                run_id: held.run_id.clone(),
                report,
            });
            self.emit(AgentEvent::RevisionStarted {
                run_id: held.run_id.clone(),
                round: round + 1,
                reason: RevisionReason::Retrieve,
                flagged,
                missing_needs: Vec::new(),
            });
            if let Some(event) = state.open_step(&step_id, SEARCH_DOCUMENTS, &args, now_ms()) {
                self.emit(event);
            }

            let call_index = self.calls_in_run.fetch_add(1, Ordering::SeqCst) + 1;
            let issued_before = self.passages_in_run.issued();
            let scope = Arc::clone(&lock(&self.scope));
            // What a search that finds nothing did not cover: the answer's
            // limit, never the whole library when the answer is limited.
            let limited_to = (!scope.is_empty()).then(|| match &scope.workspace_name {
                Some(name) if scope.restricted => {
                    format!("the sources of the workspace \"{name}\"")
                }
                _ => "the sources this answer is limited to".to_string(),
            });
            let ctx = ToolContext::new(held.run_id.clone(), step_id.clone(), self.events.clone())
                .with_run_passages(Arc::clone(&self.passages_in_run))
                .with_run_plan(Arc::clone(&self.plan_in_run))
                .with_scope(scope)
                .with_audit(self.audit.clone());
            let call = ToolCall {
                tool: SEARCH_DOCUMENTS.to_string(),
                args,
                call_index,
            };
            // Lock order state -> inflight; nothing takes the state lock
            // while holding inflight.
            let mut inflight = lock(&self.inflight);
            let inner = Arc::clone(self);
            let handle = tokio::spawn(async move {
                inner
                    .registry
                    .dispatch(call, &inner.profile, &inner.approvals, &ctx)
                    .await
            });
            inflight.insert(step_id.clone(), (step_id.clone(), handle.abort_handle()));
            (handle, issued_before, limited_to)
        };
        let joined = task.await;
        lock(&self.inflight).remove(&step_id);
        let outcome = match joined {
            Ok(outcome) => outcome,
            // Stopped with the run, which closed the step.
            Err(e) if e.is_cancelled() => return,
            Err(e) => {
                tracing::error!(target: "shodh::grounding", run_id = %held.run_id, error = %e, "the answer check's search did not finish");
                DispatchOutcome {
                    ok: false,
                    text_for_model: "the search stopped unexpectedly".to_string(),
                    summary: "Did not complete".to_string(),
                    detail: None,
                    invalid_arguments: false,
                }
            }
        };
        let result = if !outcome.ok {
            RetrieveResult::Failed {
                error: outcome.text_for_model.clone(),
            }
        } else if self.passages_in_run.issued() > issued_before {
            RetrieveResult::Found {
                text: outcome.text_for_model.clone(),
            }
        } else {
            RetrieveResult::Empty
        };
        let prompt = followup::retrieve_prompt(&result, issued_before > 0, limited_to.as_deref());
        let prompt_id = self.next_id("p");
        let mut state = lock(&self.state);
        if let Some(event) = state.close_step(
            &step_id,
            StepOutcome {
                ok: outcome.ok,
                summary: outcome.summary,
                detail: outcome.detail,
                invalid_arguments: outcome.invalid_arguments,
            },
            now_ms(),
        ) {
            self.emit(event);
        }
        if !state.resume_held(&held.run_id, held.generation, &prompt_id) {
            // Stopped or steered meanwhile: the model never saw this search's
            // passages, so they must not make a later turn count as sourced.
            let mut g = lock(&self.grounding_run);
            if g.run_id == held.run_id {
                g.seeded = g.seeded.max(self.passages_in_run.issued());
            }
            return;
        }
        // Only once its passages go to the model (not when the user stopped
        // or steered the run meanwhile) does the run count as searched: a
        // later "the documents do not cover it" then stands on this search.
        if outcome.ok {
            lock(&self.tools_ok_in_run).insert(SEARCH_DOCUMENTS.to_string());
        }
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

/// Verify the answer and check the needs; without `scorers`, by words and
/// numbers only. `tools_ok` are the tools that succeeded in the run, which
/// back claims that the user has none of some data.
fn run_checks(
    messages: &[AnswerMessage],
    passages: &[Evidence],
    opened: &[OpenedText],
    needs: &[(String, String)],
    tools_ok: &HashSet<String>,
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
    let checks = flag_unverified_absence(messages, tools_ok, checks);
    let need_checks = check_needs(needs, passages, views.relevance, thresholds);
    (checks, need_checks, method)
}

/// Tool names in a `get_state` response (`dumpTools[].name`).
fn tool_inventory(state: Option<&serde_json::Value>) -> Vec<String> {
    state
        .and_then(|s| s.get("dumpTools"))
        .and_then(serde_json::Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t.get("name").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
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
            mut launch,
            profile,
            registry,
            audit,
            grounding,
            code,
        } = config;
        let code = match (&launch.code, code) {
            (None, None) => None,
            (Some(folder), Some(session)) => {
                Some(Arc::new(CodeRuntime::new(folder.clone(), session)))
            }
            _ => {
                return Err(HarnessError::CodeGuard(
                    "the code folder and its conversation must be set together".to_string(),
                ))
            }
        };
        // A Code session's host tools (MCP servers, skills) are named to the
        // guard, which lets them through to Shodh's own approval and audit.
        if code.is_some() {
            let names: Vec<String> = registry
                .definitions(&profile)
                .into_iter()
                .map(|d| d.name)
                .collect();
            if let Some(clash) = names.iter().find(|n| {
                CODE_TOOLS.contains(&n.as_str())
                    || OMP_EXECUTION_TOOLS.contains(&n.as_str())
                    || n.as_str() == RUN_COMMAND
            }) {
                return Err(HarnessError::CodeGuard(format!(
                    "the host tool {clash} has the name of a Code mode tool"
                )));
            }
            // Shodh's own command tool, which the session runs itself.
            launch.host_tools = names
                .into_iter()
                .chain(std::iter::once(RUN_COMMAND.to_string()))
                .collect();
        }
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

        // A Code session reads the user's code with omp's own tools, which no
        // passage records: it holds private content from the start.
        let passages_in_run = Arc::new(RunPassages::new());
        if code.is_some() {
            passages_in_run.mark_private();
        }
        let inner = Arc::new(Inner {
            session_id: launch.session_id.clone(),
            model: launch.model.model_arg.clone(),
            state: Mutex::new({
                // A Code session's steps are omp's own tools and its host tools.
                let catalog = if code.is_some() {
                    let mut catalog = registry.catalog();
                    catalog.extend(code_catalog());
                    catalog
                } else {
                    registry.catalog()
                };
                let mut state = NormaliserState::new(catalog);
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
            tools_ok_in_run: Mutex::new(HashSet::new()),
            passages_in_run,
            plan_in_run: Arc::new(RunPlan::new()),
            grounding,
            grounding_run: Mutex::new(GroundingRun::default()),
            scope: Mutex::new(Arc::new(RunScope::default())),
            next_id: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            close_reason: Mutex::new(None),
            closed: AtomicBool::new(false),
            close_writer: close_writer.clone(),
            child: tokio::sync::Mutex::new(Some(process.child)),
            stderr_tail: stderr_tail.clone(),
            secrets,
            code,
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
        if let Some(code) = &inner.code {
            return self.initialise_code(code, ready_ms).await;
        }
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

    /// Set up a Code session and prove its safety checks are in place: the
    /// guard announced itself and the tools are exactly the Code tools (no
    /// sub-agents, no eval, no browser). Either failing stops the session.
    async fn initialise_code(
        &self,
        code: &CodeRuntime,
        ready_ms: u128,
    ) -> Result<(), HarnessError> {
        let inner = &self.inner;
        // The registry's tools (MCP servers, skills) and Shodh's own command
        // tool, which the session runs itself (`dispatch_run_command`).
        let mut host = inner.registry.definitions(&inner.profile);
        host.push(run_command::definition());
        let host_names: Vec<String> = host.iter().map(|d| d.name.clone()).collect();
        inner
            .command(OutboundFrame::SetEventFilter {
                id: inner.next_id("c"),
                events: None,
                message_updates: MessageUpdateMode::Delta,
            })
            .await?;
        // The inventory is read after the host tools are set.
        inner
            .command(OutboundFrame::SetHostTools {
                id: inner.next_id("c"),
                tools: host,
            })
            .await?;
        let state = inner
            .command(OutboundFrame::GetState {
                id: inner.next_id("c"),
            })
            .await?;
        let tools = tool_inventory(state.data.as_ref());
        if !inventory_matches(&tools, &host_names) {
            return Err(HarnessError::CodeGuard(format!(
                "the runtime offered the tools {tools:?}"
            )));
        }
        let mut loaded = code.guard_loaded.subscribe();
        match tokio::time::timeout(GUARD_TIMEOUT, loaded.wait_for(|loaded| *loaded)).await {
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => {
                return Err(HarnessError::CodeGuard(format!(
                    "the folder guard did not load ({})",
                    inner.stderr_summary()
                )))
            }
        }
        tracing::info!(
            target: "shodh::harness",
            session = %inner.session_id,
            ready_ms,
            folder = %code.folder.display(),
            "omp code session ready"
        );
        Ok(())
    }

    /// Close the session like [`AgentHarness::shutdown`]; an answer it stops
    /// ends with `reason` as its error, in the user's words (for example,
    /// that the provider's key was removed).
    pub async fn shutdown_because(&self, reason: impl Into<String>) {
        {
            let mut slot = lock(&self.inner.close_reason);
            if slot.is_none() {
                *slot = Some(reason.into());
            }
        }
        self.shutdown_inner().await;
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
                let mut scope = RunScope::clone(&lock(&inner.scope));
                // The steer text is this run's question, not the last one's,
                // and the app has not said it is the user's own words.
                scope.question = None;
                scope.own_words = None;
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

    fn approve_reviewed(&self, step_id: &str, token: &str) -> Result<(), HarnessError> {
        self.inner
            .approvals
            .resolve_reviewed(step_id, token)
            .map_err(|_| HarnessError::NoPendingApproval(step_id.to_string()))?;
        tracing::info!(target: "shodh::audit", event = "approval", session = %self.inner.session_id, step_id, approved = true, reviewed = true, "approval decision");
        Ok(())
    }

    fn approve_for_conversation(&self, step_id: &str) -> Result<String, HarnessError> {
        let grant = self
            .inner
            .approvals
            .resolve_for_session(step_id)
            .map_err(|_| HarnessError::NoPendingApproval(step_id.to_string()))?;
        let run_id = lock(&self.inner.state)
            .active_run_id()
            .map(str::to_string)
            .unwrap_or_default();
        self.inner.audit(
            &run_id,
            AuditEventType::Approval,
            json!({
                "step_id": step_id,
                "decision": "allowed_for_conversation",
                "grant": grant.label,
                "key": grant.key,
                "who": self.inner.principal(),
            }),
        );
        tracing::info!(target: "shodh::audit", event = "approval", session = %self.inner.session_id, step_id, grant = %grant.key, "allowed for the conversation");
        Ok(grant.label)
    }

    async fn shutdown(&self) {
        self.shutdown_inner().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::events::{ClaimOutcome, RunStatus};
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
        offline_session(grounding, None)
    }

    fn offline_session(
        grounding: Option<GroundingConfig>,
        code: Option<Arc<CodeRuntime>>,
    ) -> (
        Arc<Inner>,
        mpsc::UnboundedReceiver<AgentEvent>,
        mpsc::UnboundedReceiver<OutboundFrame>,
    ) {
        offline_session_with(grounding, code, ToolRegistry::new())
    }

    fn offline_session_with(
        grounding: Option<GroundingConfig>,
        code: Option<Arc<CodeRuntime>>,
        registry: ToolRegistry,
    ) -> (
        Arc<Inner>,
        mpsc::UnboundedReceiver<AgentEvent>,
        mpsc::UnboundedReceiver<OutboundFrame>,
    ) {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let registry = Arc::new(registry);
        let catalog = if code.is_some() {
            code_catalog()
        } else {
            registry.catalog()
        };
        let inner = Arc::new(Inner {
            session_id: "s".into(),
            model: "test/model".into(),
            state: Mutex::new({
                let mut state = NormaliserState::new(catalog);
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
            tools_ok_in_run: Mutex::new(HashSet::new()),
            passages_in_run: Arc::new(RunPassages::new()),
            plan_in_run: Arc::new(RunPlan::new()),
            grounding,
            grounding_run: Mutex::new(GroundingRun::default()),
            scope: Mutex::new(Arc::new(RunScope::default())),
            next_id: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            close_reason: Mutex::new(None),
            closed: AtomicBool::new(false),
            close_writer: Arc::new(Notify::new()),
            child: tokio::sync::Mutex::new(None),
            stderr_tail: Arc::new(Mutex::new(VecDeque::new())),
            secrets: Vec::new(),
            code,
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

    /// The error of the answer a closed session stopped.
    async fn stopped_with(
        inner: &Arc<Inner>,
        events: &mut mpsc::UnboundedReceiver<AgentEvent>,
    ) -> Option<String> {
        inner.on_eof();
        events_until(events, |e| matches!(e, AgentEvent::RunFinished { .. }))
            .await
            .into_iter()
            .find_map(|e| match e {
                AgentEvent::RunFinished { error, .. } => Some(error),
                _ => None,
            })
            .flatten()
    }

    #[tokio::test]
    async fn an_answer_stopped_by_the_host_says_why() {
        let (inner, mut events, _out) = offline_inner(None);
        start(&inner);
        *lock(&inner.close_reason) =
            Some("Stopped because the OpenRouter key was removed.".to_string());
        inner.closing.store(true, Ordering::SeqCst);
        assert_eq!(
            stopped_with(&inner, &mut events).await.as_deref(),
            Some("Stopped because the OpenRouter key was removed.")
        );

        // Closed without a reason: the plain default.
        let (inner, mut events, _out) = offline_inner(None);
        start(&inner);
        inner.closing.store(true, Ordering::SeqCst);
        assert_eq!(
            stopped_with(&inner, &mut events).await.as_deref(),
            Some("The session was closed")
        );
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
    async fn the_checked_answer_has_neither_the_preamble_nor_the_repair_apology() {
        let (inner, mut events, mut out) = offline_inner(Some(GroundingConfig::lexical()));
        start(&inner);
        let (first_prompt, _) = sent_prompt(&mut out);
        say(
            &inner,
            "m0",
            "I'll search the agreement for the notice period.",
        );
        feed(
            &inner,
            r#"{"type":"message_update","messageId":"m0","assistantMessageEvent":{"type":"toolcall_end","toolCall":{"type":"toolCall","id":"t1","name":"update_plan","arguments":{}}}}"#,
        );
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
        assert_eq!(
            report.message_ids,
            vec!["m1"],
            "the preamble is not checked"
        );
        assert_eq!(report.claims.len(), 1);

        let (repair_prompt, _) = sent_prompt(&mut out);
        let answer = "Either party may terminate the agreement with 60 days written notice [1].";
        for piece in [
            "I apologize for the error in my previous ",
            "response. Here is the corrected answer:\n",
            "\nEither party may terminate the agreement ",
            "with 60 days written notice [1].",
        ] {
            say(&inner, "m2", piece);
        }
        complete(&inner, &repair_prompt);
        let round1 =
            events_until(&mut events, |e| matches!(e, AgentEvent::RunFinished { .. })).await;
        let shown: String = round1
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta {
                    message_id, delta, ..
                } if message_id == "m2" => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(shown, answer, "the apology is never shown");
        let last = round1
            .iter()
            .find_map(|e| match e {
                AgentEvent::Grounding { report, .. } => Some(report.clone()),
                _ => None,
            })
            .expect("a final report");
        assert_eq!(last.message_ids, vec!["m2"]);
        assert!(
            answer.contains(&last.claims[0].anchor),
            "flags anchor in the shown text"
        );
        assert_eq!(last.summary.supported, 1);
    }

    /// `list_tasks` reading the user's (synthetic) task list.
    struct ThreeTasks;

    #[async_trait::async_trait]
    impl crate::harness::tools::HostTool for ThreeTasks {
        fn name(&self) -> &str {
            crate::harness::profile::app_tools::LIST_TASKS
        }
        fn label(&self) -> &str {
            "List tasks"
        }
        fn label_template(&self) -> &str {
            "Listing tasks"
        }
        fn description(&self) -> &str {
            "List the user's tasks."
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false})
        }
        fn tier(&self) -> RiskTier {
            RiskTier::Read
        }
        async fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: &crate::harness::tools::ToolContext,
        ) -> Result<crate::harness::tools::ToolOutput, crate::harness::tools::ToolError> {
            Ok(crate::harness::tools::ToolOutput {
                text_for_model: "3 tasks matched.\n[{\"title\": \"Submit expense report\"}, \
                                 {\"title\": \"Book dentist appointment\"}, \
                                 {\"title\": \"Renew library card\"}]"
                    .into(),
                summary_for_ui: "3 tasks".into(),
                detail: None,
            })
        }
    }

    #[tokio::test]
    async fn saying_the_user_has_no_tasks_without_reading_them_is_repaired_with_list_tasks() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ThreeTasks)).unwrap();
        let (inner, mut events, mut out) =
            offline_session_with(Some(GroundingConfig::lexical()), None, registry);
        inner
            .start_run(
                "what are my tasks?",
                Some("run-1".into()),
                RunScope::default(),
            )
            .unwrap();
        let (first_prompt, _) = sent_prompt(&mut out);
        // The answer v0.0.1 gave: no tool call, and no tasks.
        say(&inner, "m1", "You have no tasks right now.");
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
        assert_eq!(report.claims.len(), 1);
        assert_eq!(report.claims[0].outcome, ClaimOutcome::UnverifiedAbsence);
        assert_eq!(report.summary.unsupported, 1);
        let (repair_prompt, message) = sent_prompt(&mut out);
        assert!(
            message.contains("call list_tasks or show_calendar"),
            "{message}"
        );

        // The model reads the tasks and answers from them.
        feed(
            &inner,
            r#"{"type":"host_tool_call","id":"h1","toolCallId":"t1","toolName":"list_tasks","arguments":{}}"#,
        );
        let result = tokio::time::timeout(Duration::from_secs(10), out.recv())
            .await
            .expect("a tool result within 10 s")
            .expect("outbound open");
        assert!(
            matches!(
                &result,
                OutboundFrame::HostToolResult {
                    is_error: false,
                    ..
                }
            ),
            "{result:?}"
        );
        say(
            &inner,
            "m2",
            "You have 3 tasks: Submit expense report, Book dentist appointment and Renew library card.",
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
        assert!(last
            .claims
            .iter()
            .all(|c| c.outcome != ClaimOutcome::UnverifiedAbsence));
        assert!(out.try_recv().is_err(), "no further prompt");

        // In the next answer the earlier list_tasks call does not count: a
        // new "no tasks" needs a new read.
        inner
            .start_run("and now?", Some("run-2".into()), RunScope::default())
            .unwrap();
        let (third_prompt, _) = sent_prompt(&mut out);
        say(&inner, "m3", "You have no tasks left.");
        complete(&inner, &third_prompt);
        let round = events_until(&mut events, |e| {
            matches!(e, AgentEvent::RevisionStarted { .. })
        })
        .await;
        assert!(round.iter().any(|e| matches!(
            e,
            AgentEvent::Grounding { report, .. }
                if report.claims.iter().any(|c| c.outcome == ClaimOutcome::UnverifiedAbsence)
        )));
    }

    /// `search_documents` over one (synthetic) indexed paper, or over none;
    /// it keeps the queries it was called with.
    struct OnePaper {
        passage: Option<&'static str>,
        queries: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::harness::tools::HostTool for OnePaper {
        fn name(&self) -> &str {
            SEARCH_DOCUMENTS
        }
        fn label(&self) -> &str {
            "Search documents"
        }
        fn label_template(&self) -> &str {
            "Searching {query}"
        }
        fn description(&self) -> &str {
            "Search the user's indexed documents."
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string", "minLength": 1}},
                "required": ["query"],
                "additionalProperties": false
            })
        }
        fn tier(&self) -> RiskTier {
            RiskTier::Read
        }
        async fn execute(
            &self,
            args: serde_json::Value,
            ctx: &crate::harness::tools::ToolContext,
        ) -> Result<crate::harness::tools::ToolOutput, crate::harness::tools::ToolError> {
            let query = args["query"].as_str().unwrap_or_default().to_string();
            lock(&self.queries).push(query);
            let Some(text) = self.passage else {
                return Ok(crate::harness::tools::ToolOutput {
                    text_for_model: "No passages matched.".into(),
                    summary_for_ui: "No matching passages".into(),
                    detail: Some(serde_json::json!({ "passages": [] })),
                });
            };
            let n = ctx.reserve_passages(1);
            ctx.record_passage(CitedPassage {
                n,
                file: "freshdiskann.pdf".into(),
                path: "c:/papers/freshdiskann.pdf".into(),
                page: Some("3".into()),
                web: false,
                text: text.into(),
                checkable: true,
            });
            Ok(crate::harness::tools::ToolOutput {
                text_for_model: format!("[{{\"n\": {n}, \"text\": \"{text}\"}}]"),
                summary_for_ui: "1 passage from 1 file".into(),
                detail: None,
            })
        }
    }

    /// A Research session with `search_documents`, a run started with a
    /// composed message whose question is `question`, and the first prompt.
    fn research_run(
        passage: Option<&'static str>,
        question: &str,
    ) -> (
        Arc<Inner>,
        mpsc::UnboundedReceiver<AgentEvent>,
        mpsc::UnboundedReceiver<OutboundFrame>,
        Arc<Mutex<Vec<String>>>,
        String,
    ) {
        research_run_with(passage, question, GroundingConfig::lexical(), &[])
    }

    /// [`research_run`] with `config` and, besides `search_documents`, a
    /// [`Numbered`] tool under each of `numbering`.
    fn research_run_with(
        passage: Option<&'static str>,
        question: &str,
        config: GroundingConfig,
        numbering: &[&'static str],
    ) -> (
        Arc<Inner>,
        mpsc::UnboundedReceiver<AgentEvent>,
        mpsc::UnboundedReceiver<OutboundFrame>,
        Arc<Mutex<Vec<String>>>,
        String,
    ) {
        let queries = Arc::new(Mutex::new(Vec::new()));
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(OnePaper {
                passage,
                queries: Arc::clone(&queries),
            }))
            .unwrap();
        for &name in numbering {
            registry.register(Arc::new(Numbered { name })).unwrap();
        }
        let (inner, events, mut out) = offline_session_with(Some(config), None, registry);
        inner
            .start_run(
                &format!("[Memories the user saved]\nprefers short answers\n\n{question}"),
                Some("run-1".into()),
                RunScope {
                    question: Some(question.into()),
                    ..RunScope::default()
                },
            )
            .unwrap();
        let (first_prompt, _) = sent_prompt(&mut out);
        (inner, events, out, queries, first_prompt)
    }

    /// A tool other than a search that numbers what it shows the model, as
    /// `web_search`, `fetch_url` and `open_document` do.
    struct Numbered {
        name: &'static str,
    }

    #[async_trait::async_trait]
    impl crate::harness::tools::HostTool for Numbered {
        fn name(&self) -> &str {
            self.name
        }
        fn label(&self) -> &str {
            self.name
        }
        fn label_template(&self) -> &str {
            "Reading"
        }
        fn description(&self) -> &str {
            "Shows the model a numbered passage."
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false})
        }
        fn tier(&self) -> RiskTier {
            RiskTier::Read
        }
        async fn execute(
            &self,
            _args: serde_json::Value,
            ctx: &crate::harness::tools::ToolContext,
        ) -> Result<crate::harness::tools::ToolOutput, crate::harness::tools::ToolError> {
            let n = ctx.reserve_passages(1);
            ctx.record_passage(CitedPassage {
                n,
                file: "SPANN overview".into(),
                path: "https://example.org/spann".into(),
                page: None,
                web: true,
                text: "SPANN keeps the posting list centroids in memory.".into(),
                checkable: true,
            });
            Ok(crate::harness::tools::ToolOutput {
                text_for_model: format!("[{n}] SPANN keeps the posting list centroids in memory."),
                summary_for_ui: "1 passage".into(),
                detail: None,
            })
        }
    }

    const SPANN_QUESTION: &str =
        "How do FreshDiskANN and SPANN keep search fast when the index no longer fits in RAM?";

    #[tokio::test]
    async fn an_answer_that_skipped_the_search_gets_one_and_is_answered_from_it() {
        let passage =
            "FreshDiskANN keeps a compressed graph in memory and the full vectors on SSD.";
        let (inner, mut events, mut out, queries, first_prompt) =
            research_run(Some(passage), SPANN_QUESTION);
        // The reported answer: no tool call, citations of passages it never had.
        say(
            &inner,
            "m1",
            "FreshDiskANN keeps the graph on SSD [1]. SPANN keeps centroids in memory [2].",
        );
        complete(&inner, &first_prompt);

        let round0 = events_until(&mut events, |e| {
            matches!(e, AgentEvent::StepFinished { .. })
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
        assert!(report
            .claims
            .iter()
            .all(|c| c.outcome == ClaimOutcome::InvalidCitation));
        let revision = round0
            .iter()
            .position(|e| {
                matches!(
                    e,
                    AgentEvent::RevisionStarted {
                        round: 1,
                        reason: RevisionReason::Retrieve,
                        ..
                    }
                )
            })
            .expect("a retrieve revision");
        let started = round0
            .iter()
            .position(|e| {
                matches!(e, AgentEvent::StepStarted { tool, label, args, .. }
                    if tool == SEARCH_DOCUMENTS
                        && label.starts_with("Searching “How do FreshDiskANN")
                        && args["query"] == SPANN_QUESTION)
            })
            .expect("the search is a step of the transcript");
        assert!(revision < started, "the revision row comes first");
        assert!(matches!(
            round0.last(),
            Some(AgentEvent::StepFinished { ok: true, summary, .. }) if summary == "1 passage from 1 file"
        ));
        assert_eq!(
            *lock(&queries),
            vec![SPANN_QUESTION.to_string()],
            "searched with the question as typed, not the composed message"
        );

        let (retrieve_prompt, message) = match next_frame(&mut out).await {
            OutboundFrame::Prompt { id, message, .. } => (id, message),
            other => panic!("expected the follow-up prompt, got {other:?}"),
        };
        assert!(message.contains(passage), "{message}");
        assert!(message.contains("using only these passages"), "{message}");
        assert!(!message.contains("Use only passage numbers you were given"));

        let answer =
            "FreshDiskANN keeps a compressed graph in memory and the full vectors on SSD [1]. \
                      The documents do not cover SPANN.";
        say(&inner, "m2", answer);
        complete(&inner, &retrieve_prompt);
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
        assert_eq!(last.superseded_message_ids, vec!["m1"]);
        assert!(last
            .claims
            .iter()
            .all(|c| c.outcome != ClaimOutcome::InvalidCitation));
        assert_eq!(last.summary.supported, 1);
        assert!(matches!(
            round1.last(),
            Some(AgentEvent::RunFinished {
                status: RunStatus::Completed,
                ..
            })
        ));
        assert!(out.try_recv().is_err(), "no further prompt");
        assert_eq!(lock(&queries).len(), 1, "one search");
    }

    /// A run of `text` that only asked to remember something (`memory_instruction`), over
    /// a library with one passage; returns the run's parts and the search queries made.
    fn memory_run(
        text: &str,
    ) -> (
        Arc<Inner>,
        mpsc::UnboundedReceiver<AgentEvent>,
        mpsc::UnboundedReceiver<OutboundFrame>,
        Arc<Mutex<Vec<String>>>,
        String,
    ) {
        let queries = Arc::new(Mutex::new(Vec::new()));
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(OnePaper {
                passage: Some("FreshDiskANN keeps a compressed graph in memory."),
                queries: Arc::clone(&queries),
            }))
            .unwrap();
        let (inner, events, mut out) =
            offline_session_with(Some(GroundingConfig::lexical()), None, registry);
        inner
            .start_run(
                text,
                Some("run-1".into()),
                RunScope {
                    memory_instruction: true,
                    ..RunScope::default()
                },
            )
            .unwrap();
        let (first_prompt, _) = sent_prompt(&mut out);
        (inner, events, out, queries, first_prompt)
    }

    #[tokio::test]
    async fn an_acknowledged_request_to_remember_is_not_searched_for() {
        let (inner, mut events, mut out, queries, first_prompt) =
            memory_run("Remember that I use DiskANN");
        // An acknowledgement that an ordinary answer would be searched for: it says the
        // documents do not have something, without having searched them.
        say(
            &inner,
            "m1",
            "Noted, I'll remember that you use DiskANN. I couldn't find any information about \
             DiskANN in your documents.",
        );
        complete(&inner, &first_prompt);
        let seen = events_until(&mut events, |e| {
            matches!(
                e,
                AgentEvent::RevisionStarted { .. } | AgentEvent::RunFinished { .. }
            )
        })
        .await;
        assert!(
            !seen.iter().any(|e| matches!(
                e,
                AgentEvent::RevisionStarted {
                    reason: RevisionReason::Retrieve,
                    ..
                }
            )),
            "no forced search: {seen:?}"
        );
        assert!(lock(&queries).is_empty(), "the documents were not searched");
        // Repair still runs: the refusal is sent back to be checked, as in any answer.
        assert!(
            seen.iter().any(|e| matches!(
                e,
                AgentEvent::RevisionStarted {
                    reason: RevisionReason::Repair,
                    ..
                }
            )),
            "{seen:?}"
        );
        assert!(matches!(
            next_frame(&mut out).await,
            OutboundFrame::Prompt { .. }
        ));
    }

    #[tokio::test]
    async fn a_substantive_answer_to_a_request_to_remember_gets_the_usual_follow_ups() {
        let (inner, mut events, _out, queries, first_prompt) =
            memory_run("Remember that I use DiskANN");
        // More than an acknowledgement: it cites a passage it never had.
        say(
            &inner,
            "m1",
            "Noted. FreshDiskANN keeps the graph on SSD [1], which suits your DiskANN work.",
        );
        complete(&inner, &first_prompt);
        let seen = events_until(&mut events, |e| {
            matches!(
                e,
                AgentEvent::RevisionStarted { .. } | AgentEvent::RunFinished { .. }
            )
        })
        .await;
        assert!(
            seen.iter().any(|e| matches!(
                e,
                AgentEvent::RevisionStarted {
                    reason: RevisionReason::Retrieve,
                    ..
                }
            )),
            "{seen:?}"
        );
        events_until(&mut events, |e| {
            matches!(e, AgentEvent::StepFinished { .. })
        })
        .await;
        assert_eq!(lock(&queries).len(), 1, "the documents were searched");
    }

    #[tokio::test]
    async fn when_the_search_finds_nothing_the_answer_may_say_so() {
        let (inner, mut events, mut out, queries, first_prompt) =
            research_run(None, SPANN_QUESTION);
        say(&inner, "m1", "SPANN keeps centroids in memory [1].");
        complete(&inner, &first_prompt);
        events_until(&mut events, |e| {
            matches!(e, AgentEvent::StepFinished { .. })
        })
        .await;
        let (retrieve_prompt, message) = match next_frame(&mut out).await {
            OutboundFrame::Prompt { id, message, .. } => (id, message),
            other => panic!("expected the follow-up prompt, got {other:?}"),
        };
        assert!(message.contains("no passage matched"), "{message}");

        // The refusal, now made after a real search, stands.
        say(
            &inner,
            "m2",
            "I don't have any documents indexed that contain information about SPANN.",
        );
        complete(&inner, &retrieve_prompt);
        let round1 =
            events_until(&mut events, |e| matches!(e, AgentEvent::RunFinished { .. })).await;
        assert!(!round1.iter().any(|e| matches!(
            e,
            AgentEvent::Grounding { report, .. }
                if report.claims.iter().any(|c| c.outcome == ClaimOutcome::UnverifiedAbsence)
        )));
        assert!(!round1
            .iter()
            .any(|e| matches!(e, AgentEvent::RevisionStarted { .. })));
        assert!(out.try_recv().is_err(), "no further prompt");
        assert_eq!(lock(&queries).len(), 1);
    }

    #[tokio::test]
    async fn an_answer_that_searched_is_not_searched_again() {
        let (inner, mut events, mut out, queries, first_prompt) = research_run(
            Some("SPANN keeps the posting list centroids in memory."),
            SPANN_QUESTION,
        );
        // The model's own search, then a citation of a number it did not get.
        feed(
            &inner,
            r#"{"type":"host_tool_call","id":"h1","toolCallId":"t1","toolName":"search_documents","arguments":{"query":"SPANN memory"}}"#,
        );
        assert!(matches!(
            next_frame(&mut out).await,
            OutboundFrame::HostToolResult {
                is_error: false,
                ..
            }
        ));
        say(
            &inner,
            "m1",
            "SPANN keeps the posting list centroids in memory [4].",
        );
        complete(&inner, &first_prompt);
        let round0 = events_until(&mut events, |e| {
            matches!(e, AgentEvent::RevisionStarted { .. })
        })
        .await;
        assert!(matches!(
            round0.last(),
            Some(AgentEvent::RevisionStarted {
                reason: RevisionReason::Repair,
                ..
            })
        ));
        let (_, message) = sent_prompt(&mut out);
        assert!(message.contains("Use only passage numbers you were given"));
        assert_eq!(lock(&queries).len(), 1, "only the model's search");
    }

    #[tokio::test]
    async fn an_answer_with_web_or_opened_passages_is_repaired_not_searched_again() {
        for tool in ["web_search", "open_document"] {
            let (inner, mut events, mut out, queries, first_prompt) = research_run_with(
                Some("SPANN keeps the posting list centroids in memory."),
                SPANN_QUESTION,
                GroundingConfig::lexical(),
                &[tool],
            );
            feed(
                &inner,
                &format!(
                    r#"{{"type":"host_tool_call","id":"h1","toolCallId":"t1","toolName":"{tool}","arguments":{{}}}}"#
                ),
            );
            assert!(
                matches!(
                    next_frame(&mut out).await,
                    OutboundFrame::HostToolResult {
                        is_error: false,
                        ..
                    }
                ),
                "{tool}"
            );
            // One bad number next to the passage the tool gave.
            say(
                &inner,
                "m1",
                "SPANN keeps the posting list centroids in memory [7].",
            );
            complete(&inner, &first_prompt);
            let round0 = events_until(&mut events, |e| {
                matches!(e, AgentEvent::RevisionStarted { .. })
            })
            .await;
            assert!(
                matches!(
                    round0.last(),
                    Some(AgentEvent::RevisionStarted {
                        reason: RevisionReason::Repair,
                        ..
                    })
                ),
                "{tool}: {:?}",
                round0.last()
            );
            let (_, message) = sent_prompt(&mut out);
            assert!(
                message.contains("Use only passage numbers you were given"),
                "{tool}: {message}"
            );
            assert!(lock(&queries).is_empty(), "{tool}: no document search");
        }
    }

    /// `search_documents` that finds one passage once `release` is notified.
    struct HeldSearch {
        release: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl crate::harness::tools::HostTool for HeldSearch {
        fn name(&self) -> &str {
            SEARCH_DOCUMENTS
        }
        fn label(&self) -> &str {
            "Search documents"
        }
        fn label_template(&self) -> &str {
            "Searching {query}"
        }
        fn description(&self) -> &str {
            "Search the user's indexed documents."
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string", "minLength": 1}},
                "required": ["query"],
                "additionalProperties": false
            })
        }
        fn tier(&self) -> RiskTier {
            RiskTier::Read
        }
        async fn execute(
            &self,
            _args: serde_json::Value,
            ctx: &crate::harness::tools::ToolContext,
        ) -> Result<crate::harness::tools::ToolOutput, crate::harness::tools::ToolError> {
            self.release.notified().await;
            let n = ctx.reserve_passages(1);
            ctx.record_passage(CitedPassage {
                n,
                file: "spann.pdf".into(),
                path: "c:/papers/spann.pdf".into(),
                page: Some("2".into()),
                web: false,
                text: "SPANN keeps the posting list centroids in memory.".into(),
                checkable: true,
            });
            Ok(crate::harness::tools::ToolOutput {
                text_for_model: format!(
                    "[{{\"n\": {n}, \"text\": \"SPANN keeps the posting list centroids in memory.\"}}]"
                ),
                summary_for_ui: "1 passage from 1 file".into(),
                detail: None,
            })
        }
    }

    #[tokio::test]
    async fn passages_of_a_search_the_user_steered_past_do_not_count_as_sources() {
        let release = Arc::new(Notify::new());
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(HeldSearch {
                release: Arc::clone(&release),
            }))
            .unwrap();
        let (inner, mut events, mut out) =
            offline_session_with(Some(GroundingConfig::lexical()), None, registry);
        inner
            .start_run(SPANN_QUESTION, Some("run-1".into()), RunScope::default())
            .unwrap();
        let (first_prompt, _) = sent_prompt(&mut out);
        say(&inner, "m1", "SPANN keeps centroids in memory [1].");
        complete(&inner, &first_prompt);
        // The app's search has started and waits.
        events_until(
            &mut events,
            |e| matches!(e, AgentEvent::StepStarted { tool, .. } if tool == SEARCH_DOCUMENTS),
        )
        .await;
        // The user steers the held run (as `steer` does), then the search ends.
        assert!(lock(&inner.state).attach_prompt("p-steer"));
        release.notify_one();
        events_until(&mut events, |e| {
            matches!(e, AgentEvent::StepFinished { .. })
        })
        .await;
        assert!(out.try_recv().is_err(), "the retrieve prompt is not sent");
        assert!(
            !lock(&inner.tools_ok_in_run).contains(SEARCH_DOCUMENTS),
            "the model never saw the search"
        );

        // The steered turn cites a number nothing gave it.
        say(
            &inner,
            "m2",
            "SPANN keeps the posting list centroids in memory [9].",
        );
        complete(&inner, "p-steer");
        let round = events_until(&mut events, |e| {
            matches!(e, AgentEvent::RevisionStarted { .. })
        })
        .await;
        assert!(matches!(
            round.last(),
            Some(AgentEvent::RevisionStarted {
                reason: RevisionReason::Repair,
                ..
            })
        ));
        let message = match next_frame(&mut out).await {
            OutboundFrame::Prompt { message, .. } => message,
            other => panic!("expected the repair prompt, got {other:?}"),
        };
        assert!(
            !message.contains("Use only passage numbers you were given"),
            "{message}"
        );
        assert!(message.contains("Call search_documents now"), "{message}");
    }

    #[tokio::test]
    async fn with_automatic_follow_ups_off_the_documents_are_not_searched() {
        let off = GroundingConfig {
            auto_repair: Arc::new(|| false),
            ..GroundingConfig::lexical()
        };
        let (inner, mut events, mut out, queries, first_prompt) = research_run_with(
            Some("FreshDiskANN keeps a compressed graph in memory."),
            SPANN_QUESTION,
            off,
            &[],
        );
        say(&inner, "m1", "FreshDiskANN keeps the graph on SSD [1].");
        complete(&inner, &first_prompt);
        let all = events_until(&mut events, |e| matches!(e, AgentEvent::RunFinished { .. })).await;
        assert!(!all
            .iter()
            .any(|e| matches!(e, AgentEvent::RevisionStarted { .. })));
        assert!(!all
            .iter()
            .any(|e| matches!(e, AgentEvent::StepStarted { .. })));
        assert!(out.try_recv().is_err(), "no follow-up prompt");
        assert!(lock(&queries).is_empty());
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
    async fn a_side_run_checks_carried_passages_under_their_own_numbers() {
        // A side discussion: follow-ups off, the run seeded with the
        // passages the parent answer cites, under its numbers 1, 2, 3, 7, 8.
        let side = GroundingConfig {
            follow_ups: false,
            ..GroundingConfig::lexical()
        };
        let (inner, mut events, mut out) = offline_inner(Some(side));
        let carried = |n: u32, page: &str, text: &str| CitedPassage {
            n,
            file: "spann.pdf".into(),
            path: "c:/papers/spann.pdf".into(),
            page: Some(page.into()),
            web: false,
            text: text.into(),
            checkable: true,
        };
        inner
            .start_run(
                "What is closure augmentation?",
                Some("run-side".into()),
                RunScope {
                    carried: vec![
                        carried(
                            1,
                            "1",
                            "SPANN augments the posting list with points in the closure of each cluster.",
                        ),
                        carried(
                            2,
                            "2",
                            "Posting lists are expanded by adding the points in the closure of the cluster boundary.",
                        ),
                        carried(
                            3,
                            "3",
                            "SPANN keeps centroids in memory and posting lists on disk for partial search.",
                        ),
                        carried(7, "7", "Recall at ten is compared against DiskANN on SPACEV1B."),
                        carried(8, "10", "SPANN reaches high recall with low memory and latency."),
                    ],
                    ..RunScope::default()
                },
            )
            .unwrap();
        let (first_prompt, _) = sent_prompt(&mut out);
        say(
            &inner,
            "m1",
            "Posting lists are expanded by adding the points in the closure of the cluster boundary [3]. \
             Posting lists are expanded by adding the points in the closure of the cluster boundary [2].",
        );
        complete(&inner, &first_prompt);
        let all = events_until(&mut events, |e| matches!(e, AgentEvent::RunFinished { .. })).await;
        let report = all
            .iter()
            .find_map(|e| match e {
                AgentEvent::Grounding { report, .. } => Some(report.clone()),
                _ => None,
            })
            .expect("a final report");
        assert!(report.is_final);
        // The numbers the check reports are the carried numbers.
        assert_eq!(report.claims[0].outcome, ClaimOutcome::Unsupported);
        assert_eq!(report.claims[0].cited, vec![3]);
        assert_eq!(report.claims[0].closest, Some(2));
        assert_eq!(report.claims[0].suggested, Some(2));
        assert_eq!(report.claims[1].outcome, ClaimOutcome::Supported);
        assert_eq!(report.claims[1].cited, vec![2]);
        // Side threads are flagged, never repaired.
        assert!(!all
            .iter()
            .any(|e| matches!(e, AgentEvent::RevisionStarted { .. })));
        assert!(out.try_recv().is_err(), "no follow-up prompt");
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

    /// Stands in for the process runner: records each command, says it
    /// started, sends one line of output, and ends it with exit 0, at once or,
    /// while `hold` is set, once the test releases it ([`TestExecutor::release`]).
    #[derive(Debug)]
    struct TestExecutor {
        ran: Mutex<Vec<String>>,
        started: tokio::sync::watch::Sender<usize>,
        hold: AtomicBool,
        release: tokio::sync::Semaphore,
    }

    impl TestExecutor {
        fn new() -> Arc<Self> {
            Arc::new(TestExecutor {
                ran: Mutex::new(Vec::new()),
                started: tokio::sync::watch::channel(0).0,
                hold: AtomicBool::new(false),
                release: tokio::sync::Semaphore::new(0),
            })
        }

        fn ran(&self) -> Vec<String> {
            lock(&self.ran).clone()
        }

        fn count(&self) -> usize {
            lock(&self.ran).len()
        }

        fn holding(&self) -> bool {
            self.hold.load(Ordering::SeqCst)
        }

        fn hold(&self, hold: bool) {
            self.hold.store(hold, Ordering::SeqCst);
        }

        /// Wait until `n` commands have started.
        async fn started(&self, n: usize) {
            let mut started = self.started.subscribe();
            tokio::time::timeout(
                Duration::from_secs(10),
                started.wait_for(|count| *count >= n),
            )
            .await
            .expect("the command started within 10 s")
            .expect("the executor is alive");
        }

        /// Let one held command end.
        fn release(&self) {
            self.release.add_permits(1);
        }
    }

    #[async_trait]
    impl CommandExecutor for TestExecutor {
        async fn run(&self, spec: CommandSpec, control: RunControl) -> run_command::CommandOutcome {
            let hold = self.holding();
            lock(&self.ran).push(spec.command.clone());
            (control.on_started)();
            self.started.send_modify(|count| *count += 1);
            (control.on_output)(format!("ran {}\n", spec.command));
            if hold {
                if let Ok(permit) = self.release.acquire().await {
                    permit.forget();
                }
            }
            run_command::CommandOutcome {
                state: crate::harness::events::CommandState::Done,
                exit_code: Some(0),
                note: None,
                output: format!("ran {}\n", spec.command),
                duration: Duration::from_millis(5),
            }
        }
    }

    /// A Code session over `folder` (not a git repository: no branch work).
    fn code_runtime(folder: &std::path::Path, data: &std::path::Path) -> Arc<CodeRuntime> {
        code_runtime_with(folder, data, TestExecutor::new())
    }

    /// A Code session over `folder` whose commands `executor` runs.
    fn code_runtime_with(
        folder: &std::path::Path,
        data: &std::path::Path,
        executor: Arc<TestExecutor>,
    ) -> Arc<CodeRuntime> {
        Arc::new(CodeRuntime::new(
            CodeFolder::open(folder).unwrap(),
            CodeSession {
                conversation_id: "conv-1".into(),
                branches: CodeBranchStore::in_dir(data),
                settings_dir: Some(data.to_path_buf()),
                executor,
                scratch: data.join("scratch"),
            },
        ))
    }

    fn guard_dialog(
        ui_id: &str,
        tool_call_id: &str,
        tool: &str,
        input: serde_json::Value,
    ) -> String {
        guard_dialog_at(ui_id, tool_call_id, tool, input, None)
    }

    /// A guard request whose edit or write lands in `target` (as the guard
    /// resolves it).
    fn guard_dialog_at(
        ui_id: &str,
        tool_call_id: &str,
        tool: &str,
        input: serde_json::Value,
        target: Option<&std::path::Path>,
    ) -> String {
        let mut request = json!({"toolCallId": tool_call_id, "tool": tool, "input": input});
        if let Some(target) = target {
            request["target"] = json!(target.display().to_string());
        }
        json!({
            "type": "extension_ui_request",
            "id": ui_id,
            "method": "input",
            "title": "shodh-approval",
            "placeholder": request.to_string(),
        })
        .to_string()
    }

    async fn next_frame(out: &mut mpsc::UnboundedReceiver<OutboundFrame>) -> OutboundFrame {
        tokio::time::timeout(Duration::from_secs(10), out.recv())
            .await
            .expect("a frame within 10 s")
            .expect("writer open")
    }

    /// Guard request `n` for an edit or write (dialog `ui-{n}`, tool call
    /// `call-{n}`).
    fn guard_request(
        n: &str,
        tool: &str,
        input: serde_json::Value,
        target: Option<&std::path::Path>,
    ) -> (String, String) {
        (
            n.to_string(),
            guard_dialog_at(
                &format!("ui-{n}"),
                &format!("call-{n}"),
                tool,
                input,
                target,
            ),
        )
    }

    /// omp's `run_command` call `n` (host call `h-{n}`, tool call `call-{n}`).
    fn command_call(n: &str, arguments: serde_json::Value) -> (String, String) {
        (
            n.to_string(),
            json!({
                "type": "host_tool_call",
                "id": format!("h-{n}"),
                "toolCallId": format!("call-{n}"),
                "toolName": RUN_COMMAND,
                "arguments": arguments,
            })
            .to_string(),
        )
    }

    fn is_command(frame: &str) -> bool {
        frame.contains("\"host_tool_call\"")
    }

    /// The result of `run_command` call `n`: whether it is an error, and its
    /// text.
    async fn command_result(
        out: &mut mpsc::UnboundedReceiver<OutboundFrame>,
        n: &str,
    ) -> (bool, String) {
        match next_frame(out).await {
            OutboundFrame::HostToolResult {
                id,
                result,
                is_error,
            } => {
                assert_eq!(id, format!("h-{n}"));
                (is_error, result.joined_text())
            }
            other => panic!("expected the result of call-{n}, got {other:?}"),
        }
    }

    /// Feed call `n` ([`guard_request`], [`command_call`]) and expect it to
    /// run without a prompt: an edit is allowed; a command runs (to its end,
    /// unless the executor holds it).
    async fn runs_unasked(
        inner: &Arc<Inner>,
        executor: &TestExecutor,
        out: &mut mpsc::UnboundedReceiver<OutboundFrame>,
        (n, frame): (String, String),
    ) {
        let before = executor.count();
        feed(inner, &frame);
        if !is_command(&frame) {
            match next_frame(out).await {
                OutboundFrame::ExtensionUiResponse { id, value, .. } => {
                    assert_eq!(id, format!("ui-{n}"));
                    assert_eq!(value.as_deref(), Some(ALLOW_ANSWER), "call-{n}");
                }
                other => panic!("expected call-{n} to run unasked, got {other:?}"),
            }
            return;
        }
        if executor.holding() {
            executor.started(before + 1).await;
            return;
        }
        let (is_error, text) = command_result(out, &n).await;
        assert!(!is_error, "call-{n} failed: {text}");
        assert!(text.starts_with("Exit code 0"), "{text}");
        assert_eq!(executor.count(), before + 1, "call-{n} ran once");
    }

    /// Let held command `n` end and expect its result.
    async fn finish(
        executor: &TestExecutor,
        out: &mut mpsc::UnboundedReceiver<OutboundFrame>,
        n: &str,
    ) {
        executor.release();
        let (is_error, text) = command_result(out, n).await;
        assert!(!is_error, "call-{n} failed: {text}");
    }

    /// How a test answers a prompt.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Answer {
        Deny,
        /// A yes without the review token: the Inbox, a keyboard shortcut.
        ApproveElsewhere,
        /// The prompt's own Approve button: sends the token when the prompt
        /// has one (`ApprovalPrompt`, `reviewTokenFor`).
        ApproveInPrompt,
        /// A yes with a token this prompt did not issue.
        ApproveWithStaleToken,
    }

    /// Feed call `n` ([`guard_request`], [`command_call`]), expect a prompt
    /// before anything runs, answer it and return the prompt's preview. A
    /// declined command never runs; an approved one runs (to its end, unless
    /// the executor holds it).
    async fn asks(
        inner: &Arc<Inner>,
        executor: &TestExecutor,
        events: &mut mpsc::UnboundedReceiver<AgentEvent>,
        out: &mut mpsc::UnboundedReceiver<OutboundFrame>,
        (n, frame): (String, String),
        answer: Answer,
    ) -> serde_json::Value {
        use crate::harness::code_mode::REVIEW_TOKEN_KEY;
        let before = executor.count();
        feed(inner, &frame);
        let seen = events_until(events, |e| {
            matches!(e, AgentEvent::ApprovalRequested { .. })
        })
        .await;
        let preview = match seen.last() {
            Some(AgentEvent::ApprovalRequested {
                step_id, preview, ..
            }) => {
                assert_eq!(*step_id, format!("call-{n}"));
                preview.clone()
            }
            other => panic!("expected a prompt for call-{n}, got {other:?}"),
        };
        assert!(
            out.try_recv().is_err(),
            "call-{n} was answered before the user decided"
        );
        assert_eq!(
            executor.count(),
            before,
            "call-{n} ran before the user decided"
        );
        let step = format!("call-{n}");
        match (
            answer,
            preview.get(REVIEW_TOKEN_KEY).and_then(|t| t.as_str()),
        ) {
            (Answer::Deny, _) => inner.approvals.resolve(&step, false).unwrap(),
            (Answer::ApproveInPrompt, Some(token)) => {
                inner.approvals.resolve_reviewed(&step, token).unwrap()
            }
            (Answer::ApproveWithStaleToken, _) => inner
                .approvals
                .resolve_reviewed(&step, "0123456789abcdef")
                .unwrap(),
            _ => inner.approvals.resolve(&step, true).unwrap(),
        }
        if !is_command(&frame) {
            match next_frame(out).await {
                OutboundFrame::ExtensionUiResponse { id, value, .. } => {
                    assert_eq!(id, format!("ui-{n}"));
                    assert_eq!(
                        value.as_deref() == Some(ALLOW_ANSWER),
                        answer != Answer::Deny
                    );
                }
                other => panic!("expected the answer to call-{n}, got {other:?}"),
            }
            return preview;
        }
        if answer == Answer::Deny {
            let (is_error, text) = command_result(out, &n).await;
            assert!(is_error, "a declined command is an error");
            assert!(text.contains("declined"), "{text}");
            assert_eq!(executor.count(), before, "a declined command never runs");
        } else if executor.holding() {
            executor.started(before + 1).await;
        } else {
            let (is_error, text) = command_result(out, &n).await;
            assert!(!is_error, "call-{n} failed: {text}");
            assert_eq!(executor.count(), before + 1, "call-{n} ran once");
        }
        preview
    }

    #[tokio::test]
    async fn guard_requests_and_commands_become_approvals_and_their_answers() {
        let folder = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let exec = TestExecutor::new();
        let (inner, mut events, mut out) = offline_session(
            None,
            Some(code_runtime_with(
                folder.path(),
                data.path(),
                Arc::clone(&exec),
            )),
        );
        inner
            .start_run("Fix the build", Some("run-1".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut out);

        // A command: the step opens first, then the prompt (destructive, with
        // the not-undoable warning because the folder has no git, and the
        // limits it runs with).
        feed(
            &inner,
            &command_call("1", json!({"command": "rm -rf build"})).1,
        );
        let seen = events_until(&mut events, |e| {
            matches!(e, AgentEvent::ApprovalRequested { .. })
        })
        .await;
        assert!(matches!(
            &seen[seen.len() - 2],
            AgentEvent::StepStarted { step_id, tool, .. } if step_id == "call-1" && tool == RUN_COMMAND
        ));
        match seen.last() {
            Some(AgentEvent::ApprovalRequested {
                step_id,
                tier,
                preview,
                label,
                ..
            }) => {
                assert_eq!(step_id, "call-1");
                assert_eq!(*tier, RiskTier::Destructive);
                assert_eq!(label, "Run: rm -rf build");
                assert_eq!(preview["warning"], "Deletes files and folders recursively.");
                assert!(preview["undo"]
                    .as_str()
                    .unwrap()
                    .contains("not a git repository"));
                assert_eq!(preview["time_limit"], "10 minutes (the workspace's limit)");
                assert_eq!(
                    preview["limits"],
                    "Memory 4 GB, CPU 80%, at most 64 processes at once. Every process the \
                     command starts is stopped when it ends."
                );
            }
            other => panic!("expected an approval request, got {other:?}"),
        }
        assert!(exec.ran().is_empty(), "nothing runs before the answer");

        // Approved: it runs, with live status, and the model gets its result.
        inner.approvals.resolve("call-1", true).unwrap();
        let seen = events_until(&mut events, |e| {
            matches!(e, AgentEvent::CommandFinished { .. })
        })
        .await;
        let started = seen.iter().find_map(|e| match e {
            AgentEvent::CommandStarted {
                step_id,
                command_id,
                command,
                time_limit_secs,
                at_ms,
                ..
            } => Some((
                step_id.clone(),
                command_id.clone(),
                command.clone(),
                *time_limit_secs,
                *at_ms,
            )),
            _ => None,
        });
        let (step, command_id, command, limit, at_ms) = started.expect("command_started");
        assert_eq!(step, "call-1");
        assert_eq!(command, "rm -rf build");
        assert_eq!(limit, 600);
        // Clock time, as the UI counts elapsed time from it.
        assert!(at_ms > 1_600_000_000_000, "{at_ms}");
        assert!(seen.iter().any(|e| matches!(
            e,
            AgentEvent::CommandOutput { command_id: id, text, .. }
                if *id == command_id && text == "ran rm -rf build\n"
        )));
        match seen.last() {
            Some(AgentEvent::CommandFinished {
                step_id,
                command_id: id,
                state,
                exit_code,
                ..
            }) => {
                assert_eq!(step_id, "call-1");
                assert_eq!(*id, command_id);
                assert_eq!(*state, crate::harness::events::CommandState::Done);
                assert_eq!(*exit_code, Some(0));
            }
            other => panic!("expected command_finished, got {other:?}"),
        }
        let (is_error, text) = command_result(&mut out, "1").await;
        assert!(!is_error);
        assert!(text.starts_with("Exit code 0"), "{text}");
        assert!(text.contains("ran rm -rf build"), "{text}");
        assert_eq!(exec.ran(), ["rm -rf build"]);

        // Declined: the reason goes back instead of "allow".
        feed(
            &inner,
            &guard_dialog(
                "ui-2",
                "call-2",
                "write",
                json!({"path": "a.txt", "content": "x"}),
            ),
        );
        events_until(&mut events, |e| {
            matches!(e, AgentEvent::ApprovalRequested { .. })
        })
        .await;
        inner.approvals.resolve("call-2", false).unwrap();
        match next_frame(&mut out).await {
            OutboundFrame::ExtensionUiResponse { id, value, .. } => {
                assert_eq!(id, "ui-2");
                let reason = value.unwrap();
                assert_ne!(reason, ALLOW_ANSWER);
                assert!(reason.contains("declined"));
            }
            other => panic!("expected the dialog answer, got {other:?}"),
        }

        // omp closing the dialog (e.g. the turn was aborted) cancels the prompt.
        feed(
            &inner,
            &guard_dialog(
                "ui-3",
                "call-3",
                "edit",
                json!({"path": "a.txt", "old_string": "x", "new_string": "y"}),
            ),
        );
        events_until(&mut events, |e| {
            matches!(e, AgentEvent::ApprovalRequested { .. })
        })
        .await;
        feed(
            &inner,
            r#"{"type":"extension_ui_request","id":"ui-c","method":"cancel","targetId":"ui-3"}"#,
        );
        match next_frame(&mut out).await {
            OutboundFrame::ExtensionUiResponse { id, value, .. } => {
                assert_eq!(id, "ui-3");
                assert!(value.unwrap().contains("stopped"));
            }
            other => panic!("expected the dialog answer, got {other:?}"),
        }
        assert!(!inner.approvals.is_pending("call-3"));

        // omp cancelling a command waiting for its answer: the prompt goes,
        // and the command never runs.
        feed(
            &inner,
            &command_call("4", json!({"command": "cargo build"})).1,
        );
        events_until(&mut events, |e| {
            matches!(e, AgentEvent::ApprovalRequested { .. })
        })
        .await;
        feed(
            &inner,
            r#"{"type":"host_tool_cancel","id":"x-4","targetId":"h-4"}"#,
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while inner.approvals.is_pending("call-4") {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the prompt closed");
        assert_eq!(exec.ran(), ["rm -rf build"]);
    }

    #[tokio::test]
    async fn approval_levels_skip_the_prompt_only_where_allowed() {
        let folder = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(folder.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["-c", "init.defaultBranch=main", "init", "--quiet"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(folder.path().join("lib.rs"), "fn a() {}\n").unwrap();
        std::fs::write(folder.path().join(".gitignore"), ".env\n").unwrap();
        git(&["add", "--all"]);
        git(&["commit", "--quiet", "-m", "first"]);
        CodeSettings {
            approval: ApprovalLevel::Trusted,
            allowlist: vec!["cargo test -p app".into()],
            ..CodeSettings::default()
        }
        .save(data.path())
        .unwrap();
        let exec = TestExecutor::new();
        let code = code_runtime_with(folder.path(), data.path(), Arc::clone(&exec));
        let root = code.folder.root().to_path_buf();
        let (inner, mut events, mut out) = offline_session(None, Some(Arc::clone(&code)));
        inner
            .start_run("Fix the build", Some("run-1".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut out);

        // An allowlisted command and an edit run without a prompt; the first
        // change switches to the conversation's branch.
        let lib = root.join("lib.rs");
        runs_unasked(
            &inner,
            &exec,
            &mut out,
            command_call("1", json!({"command": "cargo test -p app"})),
        )
        .await;
        runs_unasked(
            &inner,
            &exec,
            &mut out,
            guard_request(
                "2",
                "edit",
                json!({"path": "lib.rs", "old_string": "a", "new_string": "b"}),
                Some(lib.as_path()),
            ),
        )
        .await;
        let branch = code.branches.get("conv-1").unwrap().expect("a branch");
        assert!(branch.branch.starts_with("shodh/"));

        // A destructive command asks even when it starts like an allowed one,
        // and so do one that is not on the list, one with more words than the
        // listed one, an edit whose file can run programs, an edit of a file
        // git ignores (Discard cannot restore it) and an edit the guard named
        // no file for.
        let changes = [
            command_call("3", json!({"command": "cargo test; rm -rf src"})),
            command_call("4", json!({"command": "cargo build"})),
            command_call(
                "5",
                json!({"command": "cargo test -p app --config build.rustc-wrapper=x"}),
            ),
            guard_request(
                "6",
                "write",
                json!({"path": "package.json", "content": "{}"}),
                Some(&root.join("package.json")),
            ),
            guard_request(
                "7",
                "write",
                json!({"path": ".env", "content": "A=1"}),
                Some(&root.join(".env")),
            ),
            guard_request(
                "8",
                "edit",
                json!({"path": "lib.rs", "old_string": "b", "new_string": "c"}),
                None,
            ),
        ];
        for (n, frame) in changes {
            feed(&inner, &frame);
            let seen = events_until(&mut events, |e| {
                matches!(e, AgentEvent::ApprovalRequested { .. })
            })
            .await;
            match seen.last() {
                Some(AgentEvent::ApprovalRequested {
                    step_id,
                    tier,
                    preview,
                    ..
                }) => {
                    assert_eq!(*step_id, format!("call-{n}"));
                    if n == "6" {
                        assert_eq!(*tier, RiskTier::Destructive);
                        assert!(preview["warning"].as_str().unwrap().contains("npm"));
                    }
                    if n == "7" {
                        assert_eq!(preview["undo"], UNTRACKED_UNDO);
                    }
                }
                other => panic!("expected an approval prompt, got {other:?}"),
            }
            inner
                .approvals
                .resolve(&format!("call-{n}"), false)
                .unwrap();
            match next_frame(&mut out).await {
                OutboundFrame::HostToolResult { id, is_error, .. } => {
                    assert_eq!(id, format!("h-{n}"));
                    assert!(is_error);
                }
                OutboundFrame::ExtensionUiResponse { id, value, .. } => {
                    assert_eq!(id, format!("ui-{n}"));
                    assert_ne!(value.as_deref(), Some(ALLOW_ANSWER));
                }
                other => panic!("expected the answer to call-{n}, got {other:?}"),
            }
        }
        assert_eq!(exec.ran(), ["cargo test -p app"]);

        // Back to "Ask every time": the running session asks again.
        CodeSettings::default().save(data.path()).unwrap();
        asks(
            &inner,
            &exec,
            &mut events,
            &mut out,
            guard_request("9", "write", json!({"path": "b.rs", "content": "x"}), None),
            Answer::ApproveElsewhere,
        )
        .await;
        asks(
            &inner,
            &exec,
            &mut events,
            &mut out,
            command_call("10", json!({"command": "cargo test -p app"})),
            Answer::Deny,
        )
        .await;
    }

    /// The files a command prompt listed as changed without review.
    fn listed(preview: &serde_json::Value) -> Vec<String> {
        use crate::harness::code_mode::UNREVIEWED_KEY;
        preview[UNREVIEWED_KEY]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .filter_map(|f| f.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A git repository with one commit (`lib.rs`), at Trusted with
    /// `cargo test -p app` allowed.
    fn trusted_repo() -> (tempfile::TempDir, tempfile::TempDir) {
        let folder = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(folder.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["-c", "init.defaultBranch=main", "init", "--quiet"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(folder.path().join("lib.rs"), "fn a() {}\n").unwrap();
        git(&["add", "--all"]);
        git(&["commit", "--quiet", "-m", "first"]);
        CodeSettings {
            approval: ApprovalLevel::Trusted,
            allowlist: vec!["cargo test -p app".into()],
            ..CodeSettings::default()
        }
        .save(data.path())
        .unwrap();
        (folder, data)
    }

    fn test_command() -> serde_json::Value {
        json!({"command": "cargo test -p app"})
    }

    fn edit_of(path: &str) -> serde_json::Value {
        json!({"path": path, "old_string": "a", "new_string": "b"})
    }

    #[tokio::test]
    async fn files_count_as_reviewed_only_from_the_prompt_that_listed_them() {
        use crate::harness::code_mode::{REVIEW_NOTE_KEY, REVIEW_TOKEN_KEY};
        let (folder, data) = trusted_repo();
        let exec = TestExecutor::new();
        let code = code_runtime_with(folder.path(), data.path(), Arc::clone(&exec));
        let root = code.folder.root().to_path_buf();
        let lib = root.join("lib.rs");
        let (inner, mut events, mut out) = offline_session(None, Some(Arc::clone(&code)));
        inner
            .start_run("Fix the tests", Some("run-1".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut out);
        let (x, ev, o) = (&*exec, &mut events, &mut out);

        // Nothing changed yet: the allowlisted command runs by itself.
        runs_unasked(&inner, x, o, command_call("1", test_command())).await;

        // An edit applied without asking: the command asks and lists it.
        runs_unasked(
            &inner,
            x,
            o,
            guard_request("2", "edit", edit_of("lib.rs"), Some(&lib)),
        )
        .await;
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call("3", test_command()),
            Answer::Deny,
        )
        .await;
        assert_eq!(listed(&preview), ["lib.rs"]);
        assert!(preview[REVIEW_NOTE_KEY]
            .as_str()
            .unwrap()
            .contains("marks them reviewed"));
        assert!(preview.get(REVIEW_TOKEN_KEY).is_some());

        // A yes from the Inbox runs the command but reviews nothing.
        asks(
            &inner,
            x,
            ev,
            o,
            command_call("4", test_command()),
            Answer::ApproveElsewhere,
        )
        .await;
        // Neither does a yes with a token this prompt did not issue.
        asks(
            &inner,
            x,
            ev,
            o,
            command_call("4b", test_command()),
            Answer::ApproveWithStaleToken,
        )
        .await;
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call("5", test_command()),
            Answer::ApproveInPrompt,
        )
        .await;
        assert_eq!(listed(&preview), ["lib.rs"]);
        // The prompt's own yes did.
        runs_unasked(&inner, x, o, command_call("6", test_command())).await;

        // Approving another command reviews nothing, and says so.
        runs_unasked(
            &inner,
            x,
            o,
            guard_request("7", "edit", edit_of("lib.rs"), Some(&lib)),
        )
        .await;
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call("8", json!({"command": "git status"})),
            Answer::ApproveInPrompt,
        )
        .await;
        assert_eq!(listed(&preview), ["lib.rs"]);
        assert!(preview.get(REVIEW_TOKEN_KEY).is_none());
        assert!(preview[REVIEW_NOTE_KEY]
            .as_str()
            .unwrap()
            .contains("does not mark them reviewed"));
        asks(
            &inner,
            x,
            ev,
            o,
            command_call("9", test_command()),
            Answer::ApproveInPrompt,
        )
        .await;

        // Twelve files: every one is listed, and every one is cleared.
        for i in 0..12 {
            let path = format!("f{i}.rs");
            let target = root.join(&path);
            runs_unasked(
                &inner,
                x,
                o,
                guard_request(&format!("e{i}"), "edit", edit_of(&path), Some(&target)),
            )
            .await;
        }
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call("10", test_command()),
            Answer::ApproveInPrompt,
        )
        .await;
        let files = listed(&preview);
        assert_eq!(files.len(), 12, "{files:?}");
        assert!(files.iter().any(|f| f == "f11.rs"));
        runs_unasked(&inner, x, o, command_call("11", test_command())).await;

        // An approved write whose preview was cut is no review of the file.
        let conftest = root.join("conftest.py");
        let long = format!(
            "import os\n{}\nos.system('payload')\n",
            "# fixture\n".repeat(300)
        );
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            guard_request(
                "12",
                "write",
                json!({"path": "conftest.py", "content": long}),
                Some(&conftest),
            ),
            Answer::ApproveInPrompt,
        )
        .await;
        assert!(preview["preview_cut"].as_str().is_some());
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call("13", test_command()),
            Answer::ApproveInPrompt,
        )
        .await;
        assert_eq!(listed(&preview), ["conftest.py"]);

        // What an approved command changes is listed: a file it created,
        // and a file it renamed into conftest.py (written while it runs).
        x.hold(true);
        asks(
            &inner,
            x,
            ev,
            o,
            command_call(
                "14",
                json!({"command": "python gen.py && git mv lib.rs conftest.py"}),
            ),
            Answer::ApproveInPrompt,
        )
        .await;
        std::fs::create_dir_all(root.join("generated")).unwrap();
        std::fs::write(root.join("generated").join("test_gen.py"), "import os\n").unwrap();
        let moved = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["mv", "lib.rs", "conftest.py"])
            .status()
            .unwrap();
        assert!(moved.success());
        x.hold(false);
        finish(x, o, "14").await;
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call("15", test_command()),
            Answer::ApproveInPrompt,
        )
        .await;
        let files = listed(&preview);
        for file in ["generated/test_gen.py", "conftest.py", "lib.rs"] {
            assert!(files.iter().any(|f| f == file), "{file} in {files:?}");
        }
        runs_unasked(&inner, x, o, command_call("16", test_command())).await;

        // The list outlives the session: a new one for the conversation asks.
        x.hold(true);
        asks(
            &inner,
            x,
            ev,
            o,
            command_call("17", json!({"command": "python gen.py"})),
            Answer::ApproveInPrompt,
        )
        .await;
        std::fs::write(root.join("generated").join("test_more.py"), "x = 2\n").unwrap();
        x.hold(false);
        finish(x, o, "17").await;
        code.settled().await;
        let again_exec = TestExecutor::new();
        let (again, mut again_events, mut again_out) = offline_session(
            None,
            Some(code_runtime_with(
                folder.path(),
                data.path(),
                Arc::clone(&again_exec),
            )),
        );
        again
            .start_run("Run the tests", Some("run-2".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut again_out);
        let preview = asks(
            &again,
            &again_exec,
            &mut again_events,
            &mut again_out,
            command_call("18", test_command()),
            Answer::Deny,
        )
        .await;
        assert_eq!(listed(&preview), ["generated/test_more.py"]);
    }

    #[tokio::test]
    async fn commands_with_their_own_arguments_or_alongside_others_ask() {
        use crate::harness::code_mode::BACKGROUND_NOTE;
        let (folder, data) = trusted_repo();
        let exec = TestExecutor::new();
        let code = code_runtime_with(folder.path(), data.path(), Arc::clone(&exec));
        let root = code.folder.root().to_path_buf();
        let lib = root.join("lib.rs");
        let (inner, mut events, mut out) = offline_session(None, Some(Arc::clone(&code)));
        inner
            .start_run("Fix the tests", Some("run-1".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut out);
        let (x, ev, o) = (&*exec, &mut events, &mut out);

        // Arguments run_command does not take (omp's shell had a folder, a
        // background service name) and an impossible time limit are refused
        // before anything is asked or run.
        for (n, extra) in [
            ("1", json!({"cwd": "node_modules/left-pad"})),
            ("2", json!({"name": "t"})),
            ("2b", json!({"timeout_secs": 0})),
        ] {
            let mut arguments = test_command();
            for (key, value) in extra.as_object().unwrap() {
                arguments[key] = value.clone();
            }
            feed(&inner, &command_call(n, arguments).1);
            let (is_error, text) = command_result(o, n).await;
            assert!(is_error, "call-{n}");
            assert!(
                text.starts_with("Invalid arguments for run_command"),
                "{text}"
            );
        }
        assert!(x.ran().is_empty());

        // A time limit of its own: asks, and the prompt says which limit
        // applies (one longer than the workspace's is cut to it).
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call(
                "3",
                json!({"command": "cargo test -p app", "timeout_secs": 90}),
            ),
            Answer::Deny,
        )
        .await;
        assert_eq!(preview["time_limit"], "90 seconds (asked by the assistant)");
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call(
                "3b",
                json!({"command": "cargo test -p app", "timeout_secs": 7200}),
            ),
            Answer::Deny,
        )
        .await;
        assert_eq!(preview["time_limit"], "10 minutes (the workspace's limit)");

        // Two commands at the same time: the second asks, and so does an
        // edit while the first runs (it may run what the edit changes).
        x.hold(true);
        runs_unasked(&inner, x, o, command_call("5", test_command())).await;
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call("6", test_command()),
            Answer::Deny,
        )
        .await;
        assert!(preview["running_now"]
            .as_str()
            .unwrap()
            .contains("cargo test -p app"));
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            guard_request("6b", "edit", edit_of("lib.rs"), Some(&lib)),
            Answer::Deny,
        )
        .await;
        assert!(preview["running_now"]
            .as_str()
            .unwrap()
            .contains("cargo test -p app"));
        x.hold(false);
        finish(x, o, "5").await;
        runs_unasked(&inner, x, o, command_call("7", test_command())).await;

        // A command that starts a process meant to keep running: the prompt
        // says Shodh stops it when the command ends...
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call("8", json!({"command": "npm run dev &"})),
            Answer::ApproveInPrompt,
        )
        .await;
        assert_eq!(preview["keeps_running"], BACKGROUND_NOTE);
        // ...so nothing is left running, and edits apply by themselves again.
        runs_unasked(
            &inner,
            x,
            o,
            guard_request("9", "edit", edit_of("lib.rs"), Some(&lib)),
        )
        .await;

        // A process recorded as running on before commands ran in their own
        // process is still honoured: edits ask, in this session and the next.
        code.reviews
            .update("conv-1", |state| {
                state.background.push("`npm run dev &`".to_string())
            })
            .unwrap();
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            guard_request("10", "edit", edit_of("lib.rs"), Some(&lib)),
            Answer::Deny,
        )
        .await;
        assert!(preview["running_process"]
            .as_str()
            .unwrap()
            .contains("npm run dev"));
        let again_exec = TestExecutor::new();
        let (again, mut again_events, mut again_out) = offline_session(
            None,
            Some(code_runtime_with(
                folder.path(),
                data.path(),
                Arc::clone(&again_exec),
            )),
        );
        again
            .start_run("Go on", Some("run-2".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut again_out);
        asks(
            &again,
            &again_exec,
            &mut again_events,
            &mut again_out,
            guard_request("11", "edit", edit_of("lib.rs"), Some(&lib)),
            Answer::Deny,
        )
        .await;
    }

    #[tokio::test]
    async fn a_command_over_the_answers_tool_budget_is_refused() {
        let (folder, data) = trusted_repo();
        let exec = TestExecutor::new();
        let code = code_runtime_with(folder.path(), data.path(), Arc::clone(&exec));
        let (inner, _events, mut out) = offline_session(None, Some(code));
        inner
            .start_run("Fix the tests", Some("run-1".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut out);
        inner
            .calls_in_run
            .store(inner.profile.max_tool_calls, Ordering::SeqCst);
        feed(&inner, &command_call("1", test_command()).1);
        let (is_error, text) = command_result(&mut out, "1").await;
        assert!(is_error);
        assert!(text.contains("budget"), "{text}");
        assert!(exec.ran().is_empty());
    }

    #[tokio::test]
    async fn outside_git_what_a_command_changed_is_unknown() {
        use crate::harness::code_mode::REVIEW_NOTE_KEY;
        let folder = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        CodeSettings {
            approval: ApprovalLevel::Trusted,
            allowlist: vec!["cargo test -p app".into()],
            ..CodeSettings::default()
        }
        .save(data.path())
        .unwrap();
        let exec = TestExecutor::new();
        let code = code_runtime_with(folder.path(), data.path(), Arc::clone(&exec));
        let (inner, mut events, mut out) = offline_session(None, Some(Arc::clone(&code)));
        inner
            .start_run("Fix the tests", Some("run-1".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut out);
        let (x, ev, o) = (&*exec, &mut events, &mut out);

        runs_unasked(&inner, x, o, command_call("1", test_command())).await;
        x.hold(true);
        let preview = asks(
            &inner,
            x,
            ev,
            o,
            command_call("2", test_command()),
            Answer::ApproveInPrompt,
        )
        .await;
        assert!(preview[REVIEW_NOTE_KEY]
            .as_str()
            .unwrap()
            .contains("not a git repository"));
        // The prompt's yes cleared what it showed; the command it allowed is
        // listed while it runs...
        let state = code.reviews.get("conv-1").unwrap();
        assert!(state.unknown.is_none() && state.files.is_empty());
        assert_eq!(state.uncompared.keys().collect::<Vec<_>>(), ["call-2"]);
        // ...and once the command ends, what it changed is unknown again.
        x.hold(false);
        finish(x, o, "2").await;
        asks(
            &inner,
            x,
            ev,
            o,
            command_call("3", test_command()),
            Answer::Deny,
        )
        .await;
    }

    #[tokio::test]
    async fn a_command_cut_off_before_its_end_still_makes_the_next_ask() {
        use crate::harness::code_mode::REVIEW_NOTE_KEY;
        let (folder, data) = trusted_repo();
        let exec = TestExecutor::new();
        let code = code_runtime_with(folder.path(), data.path(), Arc::clone(&exec));
        let (inner, _events, mut out) = offline_session(None, Some(Arc::clone(&code)));
        inner
            .start_run("Fix the tests", Some("run-1".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut out);
        // It never ends: the app quit, or the session closed, mid-command.
        exec.hold(true);
        runs_unasked(&inner, &exec, &mut out, command_call("1", test_command())).await;
        let again_exec = TestExecutor::new();
        let (again, mut again_events, mut again_out) = offline_session(
            None,
            Some(code_runtime_with(
                folder.path(),
                data.path(),
                Arc::clone(&again_exec),
            )),
        );
        again
            .start_run("Go on", Some("run-2".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut again_out);
        let preview = asks(
            &again,
            &again_exec,
            &mut again_events,
            &mut again_out,
            command_call("2", test_command()),
            Answer::Deny,
        )
        .await;
        assert!(preview[REVIEW_NOTE_KEY]
            .as_str()
            .unwrap()
            .contains("has not compared what `cargo test -p app` changed"));
    }

    /// Stands in for an MCP server's write tool: writes a test file into the
    /// code folder when called (named as a tool the profile allows).
    struct WritesATest {
        folder: std::path::PathBuf,
    }

    #[async_trait::async_trait]
    impl crate::harness::tools::HostTool for WritesATest {
        fn name(&self) -> &str {
            crate::harness::profile::app_tools::LIST_TASKS
        }
        fn label(&self) -> &str {
            "files: write_file"
        }
        fn label_template(&self) -> &str {
            "Writing a file"
        }
        fn description(&self) -> &str {
            "Write a file."
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false})
        }
        fn tier(&self) -> RiskTier {
            RiskTier::Read
        }
        fn may_change_files(&self) -> bool {
            true
        }
        async fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: &crate::harness::tools::ToolContext,
        ) -> Result<crate::harness::tools::ToolOutput, crate::harness::tools::ToolError> {
            std::fs::create_dir_all(self.folder.join("tests")).unwrap();
            std::fs::write(self.folder.join("tests").join("test_mcp.py"), "import os\n").unwrap();
            Ok(crate::harness::tools::ToolOutput {
                text_for_model: "Wrote tests/test_mcp.py".to_string(),
                summary_for_ui: "Wrote a file".to_string(),
                detail: None,
            })
        }
    }

    #[tokio::test]
    async fn files_an_mcp_tool_writes_are_listed() {
        let (folder, data) = trusted_repo();
        let exec = TestExecutor::new();
        let code = code_runtime_with(folder.path(), data.path(), Arc::clone(&exec));
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(WritesATest {
                folder: code.folder.root().to_path_buf(),
            }))
            .unwrap();
        let (inner, mut events, mut out) =
            offline_session_with(None, Some(Arc::clone(&code)), registry);
        inner
            .start_run("Add a test", Some("run-1".into()), RunScope::default())
            .unwrap();
        let _ = sent_prompt(&mut out);
        // The conversation's branch first (a first change needs a clean tree).
        runs_unasked(&inner, &exec, &mut out, command_call("0", test_command())).await;
        feed(
            &inner,
            r#"{"type":"host_tool_call","id":"h1","toolCallId":"t1","toolName":"list_tasks","arguments":{}}"#,
        );
        match next_frame(&mut out).await {
            OutboundFrame::HostToolResult { is_error, .. } => assert!(!is_error),
            other => panic!("expected the tool result, got {other:?}"),
        }
        let preview = asks(
            &inner,
            &exec,
            &mut events,
            &mut out,
            command_call("1", test_command()),
            Answer::Deny,
        )
        .await;
        assert_eq!(listed(&preview), ["tests/test_mcp.py"]);
    }

    #[tokio::test]
    async fn other_dialogs_are_declined_at_once() {
        let folder = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (inner, mut events, mut out) =
            offline_session(None, Some(code_runtime(folder.path(), data.path())));
        // omp's own tool prompt, and a guard-titled request for a read tool
        // (the guard never asks about those): both refused, nothing shown.
        feed(
            &inner,
            r#"{"type":"extension_ui_request","id":"n1","method":"select","title":"Allow tool: bash\nCommand: ls","options":["Approve","Deny"]}"#,
        );
        feed(
            &inner,
            &guard_dialog("n2", "call-9", "read", json!({"path": "a.txt"})),
        );
        for expected in ["n1", "n2"] {
            match next_frame(&mut out).await {
                OutboundFrame::ExtensionUiResponse {
                    id,
                    value,
                    cancelled,
                } => {
                    assert_eq!(id, expected);
                    assert_eq!(value, None);
                    assert!(cancelled);
                }
                other => panic!("expected a declined dialog, got {other:?}"),
            }
        }
        // Presentation updates need no answer.
        feed(
            &inner,
            r#"{"type":"extension_ui_request","id":"w1","method":"setWidget","widgetKey":"autoresearch"}"#,
        );
        assert!(out.try_recv().is_err());
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn the_guard_announcement_and_tool_inventory_are_read() {
        let folder = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let code = code_runtime(folder.path(), data.path());
        let (inner, _events, _out) = offline_session(None, Some(Arc::clone(&code)));
        feed(
            &inner,
            r#"{"type":"available_commands_update","commands":[{"name":"model","source":"builtin"},{"name":"shodh-guard","source":"file"}]}"#,
        );
        assert!(
            !*code.guard_loaded.borrow(),
            "a file command is not the guard"
        );
        feed(
            &inner,
            r#"{"type":"available_commands_update","commands":[{"name":"model","source":"builtin"},{"name":"shodh-guard","source":"extension"}]}"#,
        );
        assert!(*code.guard_loaded.borrow());

        let state = json!({"dumpTools": [{"name": "read"}, {"name": "task"}]});
        assert_eq!(tool_inventory(Some(&state)), vec!["read", "task"]);
        assert!(tool_inventory(None).is_empty());
    }
}
