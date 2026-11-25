//! Automations: instructions the user saved, which Shodh runs on a schedule or when the
//! user starts one (a `playbook` engine job), each run an agent session the user can open
//! as a conversation afterwards.
//!
//! - **One estimate, never passed.** An agent step plans [`AGENT_STEP_CALLS`] model calls
//!   (the automation profile's tool budget and its answer). The session is stopped at the
//!   estimate, and the run ends with what it has, saying so.
//! - **Chat first.** Before each agent step, when the automations' model counts against the
//!   same limit as chat's: while the user's own answer runs, the run waits (re-checked every
//!   [`YIELD_RECHECK_SECS`]); after a rate limit or a used-up quota from any answer on that
//!   limit, it waits for the reset; and on OpenRouter's free limit, it keeps back a reserve
//!   for chat (the busiest chat day of the last [`RESERVE_DAYS`], at least one call) and
//!   waits for the next day once only that is left. None of these waits is a failure, so
//!   none ever holds an automation. A run the user started skips the chat rules, not the
//!   provider's reset.
//! - **Nothing changes without the user's OK.** The agent's changes are saved, never run
//!   ([`shodh_rag::harness::tools::deferred`]). A run with changes ends waiting for an
//!   approval: one card with a checkbox per change, which makes exactly what it showed.
//!   It expires after [`APPROVAL_DAYS`] and the run is cancelled.
//! - **The screen never moves for a scheduled run.** Its events go to this runner only, its
//!   conversation is saved like any other (Ask's history shows it behind its Automations
//!   filter), and it tells the Inbox: what it found (Info), what it wants to change
//!   (Attention), and an automation put on hold (Attention).
//! - **Missed times.** A run for a time Shodh missed (closed or asleep) is one run for the
//!   latest time, counting the others as missed, and waits [`AFTER_WAKE_MINUTES`] first.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Local, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shodh_rag::engine::clock::next_utc_midnight;
use shodh_rag::engine::runner::{Engine, Handled, JobHandler};
use shodh_rag::engine::schedule::Schedule;
use shodh_rag::engine::steps::{
    Approval, ApprovalOutcome, Step, StepKind, StepState, StepWrite, APPROVAL_DAYS,
};
use shodh_rag::engine::store::{
    parse_instant, CatchUp, CreatedBy, EngineError, HoldReason, Job, JobKind, JobPatch, NewJob,
    NewRun, Overlap, Run, RunState, Trigger, WaitReason,
};
use shodh_rag::harness::model_catalog::ModelRef;
use shodh_rag::harness::profile::AUTOMATION_MAX_TOOL_CALLS;
use shodh_rag::harness::tools::deferred::DeferredCall;
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};

use crate::notifications::Category;

/// The model calls one agent step plans: its tool calls and its answer.
pub const AGENT_STEP_CALLS: u32 = AUTOMATION_MAX_TOOL_CALLS + 1;
/// Most automations at once.
pub const MAX_AUTOMATIONS: usize = 50;
/// Longest name, in characters.
pub const MAX_NAME_CHARS: usize = 80;
/// Longest instruction, in characters.
pub const MAX_INSTRUCTION_CHARS: usize = 4_000;
/// The quota pool of OpenRouter's free models, which every free model counts against.
pub const FREE_POOL: &str = "openrouter:free";
/// OpenRouter's documented daily limit of free-model requests for an account that bought
/// no credits: the reserve is measured against it, so it errs on the side of chat.
pub const FREE_DAILY_LIMIT: u32 = 50;
/// Days of chat the reserve looks back over.
pub const RESERVE_DAYS: u32 = 14;
/// How long a run waits before looking again while the user's answer runs.
pub const YIELD_RECHECK_SECS: i64 = 30;
/// How long a run for a missed time waits after the computer woke or Shodh opened.
pub const AFTER_WAKE_MINUTES: i64 = 2;
/// How long a run waits before trying a model that did not answer again.
pub const MODEL_RETRY_MINUTES: i64 = 15;
/// How long a rate limit with no reset time holds automations back.
pub const SHORT_LIMIT_SECS: i64 = 60;
/// Runs a list or a page reads.
pub const RUNS_SHOWN: usize = 20;

/// Emitted when an automation or one of its runs changed (the view refreshes).
pub const AUTOMATIONS_CHANGED_EVENT: &str = "automations-changed";
/// The one Info item that says automations wait so chat keeps working.
pub const YIELD_ITEM: &str = "automation:yield";

/// Error code of a run whose model could not be reached or set up.
pub const MODEL_UNAVAILABLE: &str = "model_unavailable";

fn approval_item(id: &str) -> String {
    format!("engine:approval:{id}")
}

fn hold_item(job_id: &str) -> String {
    format!("automation-hold:{job_id}")
}

fn run_item(run_id: &str) -> String {
    format!("automation:run:{run_id}")
}

/// The conversation of a run.
pub fn conversation_of(run_id: &str) -> String {
    format!("auto-{run_id}")
}

/// What an automation does, kept in its job's `input_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playbook {
    pub name: String,
    pub instruction: String,
    /// The workspace it works in; `None` is the whole library.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Goes up with every change of what it does.
    #[serde(default)]
    pub revision: u32,
}

impl Playbook {
    fn of(job: &Job) -> Result<Self, String> {
        serde_json::from_value(job.input.clone())
            .map_err(|e| format!("The automation could not be read: {e}"))
    }
}

/// An automation as the user or the assistant writes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub name: String,
    pub instruction: String,
    /// When it runs; `None` only when started.
    #[serde(default)]
    pub schedule: Option<Schedule>,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

impl Draft {
    /// The draft trimmed and checked, or why it cannot be saved.
    pub fn checked(self) -> Result<Self, String> {
        let name = self.name.trim().to_string();
        let instruction = self.instruction.trim().to_string();
        if name.is_empty() {
            return Err("Give the automation a name.".into());
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(format!("A name has at most {MAX_NAME_CHARS} characters."));
        }
        if instruction.is_empty() {
            return Err("Say what Shodh should do.".into());
        }
        if instruction.chars().count() > MAX_INSTRUCTION_CHARS {
            return Err(format!(
                "An instruction has at most {MAX_INSTRUCTION_CHARS} characters."
            ));
        }
        let schedule = self.schedule.map(Schedule::normalized);
        if let Some(schedule) = &schedule {
            schedule.validate().map_err(|e| e.to_string())?;
        }
        let workspace_id = self
            .workspace_id
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty());
        Ok(Self {
            name,
            instruction,
            schedule,
            workspace_id,
        })
    }
}

/// When an automation runs, in words: "Every week on Mon at 09:00".
pub fn when_label(schedule: Option<&Schedule>) -> String {
    match schedule {
        Some(schedule) => schedule.label(true),
        None => "Only when you start it".to_string(),
    }
}

/// The estimate in words, as previews and the form say it.
pub fn estimate_line(schedule: Option<&Schedule>) -> String {
    let per_run = format!("up to {AGENT_STEP_CALLS} model calls a run");
    match schedule.and_then(runs_a_month) {
        Some(runs) => format!(
            "{per_run} (about {} a month)",
            runs.saturating_mul(AGENT_STEP_CALLS)
        ),
        None => per_run,
    }
}

/// About how many times a schedule runs in a month.
fn runs_a_month(schedule: &Schedule) -> Option<u32> {
    match schedule {
        Schedule::Once { .. } => None,
        Schedule::Daily { .. } => Some(30),
        Schedule::Weekdays { .. } => Some(22),
        Schedule::Weekly { days, .. } => {
            Some(u32::try_from(days.len()).unwrap_or(7).saturating_mul(4))
        }
        Schedule::Monthly { .. } => Some(1),
        Schedule::EveryHours { hours, .. } => Some((24 / (*hours).max(1)).saturating_mul(30)),
    }
}

/// The model an automation's run uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunModel {
    /// `None`: the chat's model.
    pub model: Option<ModelRef>,
    /// The quota pool its calls count against.
    pub pool: String,
    /// Whether that is chat's pool too: then automations yield to chat.
    pub shares_chat: bool,
}

/// Why no run can start with the automations' model now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelBlock {
    /// No model is set up at all.
    NoModel,
    /// Local-only mode is on and the model is not on this computer.
    LocalOnly,
    /// The chosen model cannot be used now (its key is gone, its provider signed out).
    Unusable(String),
}

/// What the runner counts calls against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolToday {
    /// Calls counted against the pool today.
    pub used: u32,
    /// The most chat calls on any one of the last [`RESERVE_DAYS`] days.
    pub busiest_chat: u32,
}

/// What a run's agent step is asked to do.
#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub run_id: String,
    pub automation_id: String,
    pub name: String,
    pub conversation_id: String,
    pub instruction: String,
    pub workspace_id: Option<String>,
    pub model: Option<ModelRef>,
    pub planned: u32,
}

/// Why an agent step failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Too many requests for now.
    RateLimited,
    /// The daily quota or credits are used up.
    Quota,
    /// The model did not answer (overloaded, unknown, timed out).
    ModelUnavailable,
    /// The connection broke or never opened.
    Network,
    /// The key was refused, or the model is not set up.
    Setup,
    Other,
}

/// How an agent step failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFailure {
    pub kind: FailureKind,
    pub detail: String,
    /// When the provider said its limit resets.
    pub resets_at: Option<DateTime<Utc>>,
}

/// What an agent step came to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentOutcome {
    pub calls: u32,
    /// The answer's text.
    pub answer: String,
    /// The changes it saved for the user's OK.
    pub writes: Vec<DeferredCall>,
    /// The session was stopped at the estimate.
    pub stopped_at_estimate: bool,
    /// Where it would have taken the user (navigation targets), in order.
    pub navigations: Vec<Value>,
    pub failure: Option<AgentFailure>,
}

/// Counts a run's model calls as they happen.
#[async_trait]
pub trait CallMeter: Send + Sync {
    /// One more call; returns the run's calls so far.
    async fn call(&self) -> u32;
}

/// What the runner needs of the app.
#[async_trait]
pub trait AutomationEnv: Send + Sync {
    /// The automations' model now, checked.
    fn run_model(&self) -> Result<RunModel, ModelBlock>;
    /// Whether one of the user's own answers is running.
    fn user_answering(&self) -> bool;
    /// Until when automations on `pool` wait after a rate limit or used-up quota.
    fn cooldown(&self, pool: &str) -> Option<DateTime<Utc>>;
    /// Automations on `pool` wait until `until`.
    fn hold_pool(&self, pool: &str, until: DateTime<Utc>);
    /// Today's calls against `pool` and the busiest chat day.
    async fn pool_today(&self, pool: &str) -> Option<PoolToday>;
    /// Makes the conversation of a run (titled, in its workspace, marked as the
    /// automation's), so it can be opened before its first answer.
    async fn prepare_conversation(&self, request: &AgentRequest) -> Result<(), String>;
    /// Runs the agent step, stopping it at `request.planned` calls.
    async fn run_agent(&self, request: &AgentRequest, meter: &dyn CallMeter) -> AgentOutcome;
    /// Makes one change the user approved. Returns what it did, in words.
    async fn run_approved(
        &self,
        conversation_id: &str,
        write: &DeferredCall,
    ) -> Result<String, String>;
    async fn post(&self, item: NewInboxItem);
    async fn resolve(&self, id: &str);
    fn notify(&self, category: Category, title: &str, body: String);
    fn changed(&self);
}

/// The first line of an answer, short: a run's summary.
fn summary_of(answer: &str) -> String {
    let line = answer
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = line.chars().take(140).collect();
    if line.chars().count() > 140 {
        out.push('…');
    }
    out
}

/// "Wants to make 3 changes" / "Wants to add a task".
fn approval_title(writes: &[DeferredCall]) -> String {
    match writes {
        [one] => format!("Wants to: {}", one.label),
        many => format!("Wants to make {} changes", many.len()),
    }
}

fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, EngineError> + Send + 'static,
) -> impl std::future::Future<Output = Result<T, String>> {
    async move {
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }
}

/// Counts a run's calls in its row and renews its lease with each.
struct RunMeter {
    engine: Arc<Engine>,
    run_id: String,
    token: Option<String>,
}

#[async_trait]
impl CallMeter for RunMeter {
    async fn call(&self) -> u32 {
        let store = self.engine.store().clone();
        let (id, token, now) = (self.run_id.clone(), self.token.clone(), self.engine.now());
        let used = blocking(move || {
            if let Some(token) = &token {
                store.beat(&id, token, now)?;
            }
            store.add_used(&id, 1)
        })
        .await;
        used.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "a run's model call could not be counted");
            0
        })
    }
}

/// The playbook jobs' handler.
pub struct PlaybookHandler {
    engine: Arc<Engine>,
    env: Arc<dyn AutomationEnv>,
}

impl PlaybookHandler {
    async fn put_step(
        &self,
        run_id: &str,
        idx: u32,
        kind: StepKind,
        state: StepState,
        output: Value,
        calls: u32,
    ) {
        let store = self.engine.store().clone();
        let (run_id, now) = (run_id.to_string(), self.engine.now());
        let written = blocking(move || {
            let empty = json!({});
            store
                .put_step(
                    &StepWrite {
                        run_id: &run_id,
                        idx,
                        kind,
                        state,
                        input: &empty,
                        output: &output,
                        calls,
                    },
                    now,
                )
                .map(|_| ())
        })
        .await;
        if let Err(e) = written {
            tracing::warn!(error = %e, "a run's step could not be recorded");
        }
    }

    /// The chat rules and the provider's reset, before an agent step.
    async fn wait_before(&self, run: &Run, model: &RunModel) -> Option<Handled> {
        let now = self.engine.now();
        if let Some(until) = self.env.cooldown(&model.pool).filter(|u| *u > now) {
            self.post_yield(until).await;
            return Some(Handled::Defer {
                at: until,
                reason: WaitReason::Quota,
            });
        }
        // The user asked for this run now: it does not wait for their chat.
        if run.trigger == Trigger::Manual || !model.shares_chat {
            return None;
        }
        if self.env.user_answering() {
            return Some(Handled::Defer {
                at: now + TimeDelta::seconds(YIELD_RECHECK_SECS),
                reason: WaitReason::Yield,
            });
        }
        if model.pool == FREE_POOL {
            if let Some(today) = self.env.pool_today(&model.pool).await {
                let left = FREE_DAILY_LIMIT.saturating_sub(today.used);
                let reserve = today.busiest_chat.max(1);
                if left < AGENT_STEP_CALLS.saturating_add(reserve) {
                    let until = next_utc_midnight(now);
                    self.post_yield(until).await;
                    return Some(Handled::Defer {
                        at: until,
                        reason: WaitReason::Quota,
                    });
                }
            }
        }
        None
    }

    /// The one Info item that says automations wait so chat keeps working (updated,
    /// never repeated).
    async fn post_yield(&self, until: DateTime<Utc>) {
        self.env
            .post(NewInboxItem {
                id: YIELD_ITEM.to_string(),
                kind: InboxKind::Automation,
                status: InboxStatus::Working,
                title: "Automations paused so your chat keeps working".to_string(),
                detail: Some(format!(
                    "They go on after {}.",
                    until.with_timezone(&Local).format("%H:%M")
                )),
                link: Some(InboxLink::new("automations", None)),
                data: json!({ "paused": "chat", "resumesAt": until.to_rfc3339() }),
            })
            .await;
    }

    async fn after_approval(&self, job: &Job, playbook: &Playbook, approval: Approval) -> Handled {
        let now = self.engine.now();
        match approval.decision {
            None if approval.is_overdue(now) => {
                let store = self.engine.store().clone();
                let id = approval.id.clone();
                let expired = blocking(move || {
                    store.decide_approval(&id, ApprovalOutcome::Expired, &json!({}), now)
                })
                .await;
                if let Err(e) = expired {
                    return Handled::Transient {
                        code: "approval_unsaved".into(),
                        detail: e,
                    };
                }
                self.env.resolve(&approval_item(&approval.id)).await;
                Handled::Skipped {
                    summary: format!(
                        "Expired: nobody approved these changes in {APPROVAL_DAYS} days."
                    ),
                }
            }
            None => Handled::Waiting {
                reason: WaitReason::Approval,
            },
            Some(ApprovalOutcome::Expired) => Handled::Skipped {
                summary: format!("Expired: nobody approved these changes in {APPROVAL_DAYS} days."),
            },
            Some(ApprovalOutcome::Denied) => {
                let summary = "You declined its changes.".to_string();
                self.tell_done(job, playbook, &approval.run_id, &summary)
                    .await;
                Handled::Succeeded {
                    summary,
                    scheduled_for: None,
                }
            }
            Some(ApprovalOutcome::Approved | ApprovalOutcome::Partly) => {
                let made = approval.result["made"].as_u64().unwrap_or(0);
                let failed = approval.result["failed"].as_u64().unwrap_or(0);
                let mut summary = match made {
                    1 => "Made 1 change you approved.".to_string(),
                    n => format!("Made {n} changes you approved."),
                };
                if failed > 0 {
                    summary.push_str(&format!(" {failed} could not be made."));
                }
                self.tell_done(job, playbook, &approval.run_id, &summary)
                    .await;
                Handled::Succeeded {
                    summary,
                    scheduled_for: None,
                }
            }
        }
    }

    /// The notify step: one Info item per run.
    async fn tell_done(&self, job: &Job, playbook: &Playbook, run_id: &str, summary: &str) {
        self.env
            .post(NewInboxItem {
                id: run_item(run_id),
                kind: InboxKind::Automation,
                status: InboxStatus::Done,
                title: playbook.name.clone(),
                detail: Some(summary.to_string()),
                link: Some(InboxLink::new(
                    "automations",
                    Some(json!({ "kind": "automation", "automationId": job.id, "runId": run_id })),
                )),
                data: json!({ "jobId": job.id, "runId": run_id }),
            })
            .await;
        self.put_step(
            run_id,
            1,
            StepKind::Notify,
            StepState::Done,
            json!({ "summary": summary }),
            0,
        )
        .await;
    }

    async fn agent_step(&self, job: &Job, run: &Run, playbook: &Playbook) -> Handled {
        let model = match self.env.run_model() {
            Ok(model) => model,
            Err(ModelBlock::LocalOnly) => {
                return Handled::Skipped {
                    summary: "Skipped: Local-only mode is on; pick a local model for automations."
                        .to_string(),
                }
            }
            Err(ModelBlock::NoModel) => {
                return Handled::RetryAt {
                    at: self.engine.now() + TimeDelta::minutes(MODEL_RETRY_MINUTES),
                    reason: WaitReason::ModelUnavailable,
                    code: MODEL_UNAVAILABLE.into(),
                    detail: "No model is set up.".into(),
                }
            }
            Err(ModelBlock::Unusable(why)) => {
                return Handled::RetryAt {
                    at: self.engine.now() + TimeDelta::minutes(MODEL_RETRY_MINUTES),
                    reason: WaitReason::ModelUnavailable,
                    code: MODEL_UNAVAILABLE.into(),
                    detail: why,
                }
            }
        };
        if let Some(wait) = self.wait_before(run, &model).await {
            return wait;
        }
        let store = self.engine.store().clone();
        let id = run.id.clone();
        let conversation_id = conversation_of(&run.id);
        let conversation = conversation_id.clone();
        if let Err(e) = blocking(move || {
            store.set_planned(&id, AGENT_STEP_CALLS)?;
            store.set_conversation(&id, &conversation)
        })
        .await
        {
            return Handled::Transient {
                code: "run_unsaved".into(),
                detail: e,
            };
        }
        let request = AgentRequest {
            run_id: run.id.clone(),
            automation_id: job.id.clone(),
            name: playbook.name.clone(),
            conversation_id,
            instruction: playbook.instruction.clone(),
            workspace_id: playbook.workspace_id.clone(),
            model: model.model.clone(),
            planned: AGENT_STEP_CALLS,
        };
        if let Err(e) = self.env.prepare_conversation(&request).await {
            return Handled::Transient {
                code: "conversation_unsaved".into(),
                detail: e,
            };
        }
        self.put_step(
            &run.id,
            0,
            StepKind::Agent,
            StepState::Running,
            json!({}),
            0,
        )
        .await;
        self.env.changed();
        let meter = RunMeter {
            engine: self.engine.clone(),
            run_id: run.id.clone(),
            token: run.run_token.clone(),
        };
        let outcome = self.env.run_agent(&request, &meter).await;
        let summary = if outcome.stopped_at_estimate {
            format!(
                "Stopped at its estimate of {AGENT_STEP_CALLS} calls. {}",
                summary_of(&outcome.answer)
            )
            .trim()
            .to_string()
        } else {
            summary_of(&outcome.answer)
        };
        let output = json!({
            "summary": summary,
            "stoppedAtEstimate": outcome.stopped_at_estimate,
            "changes": outcome.writes.len(),
            "navigations": outcome.navigations,
            "error": outcome.failure.as_ref().map(|f| f.detail.clone()),
        });
        let state = if outcome.failure.is_some() {
            StepState::Failed
        } else {
            StepState::Done
        };
        self.put_step(&run.id, 0, StepKind::Agent, state, output, outcome.calls)
            .await;
        if let Some(failure) = outcome.failure {
            return self.failed(&model, failure);
        }
        if !outcome.writes.is_empty() {
            return self.ask_approval(job, run, playbook, outcome.writes).await;
        }
        let summary = if summary.is_empty() {
            "Ran; it found nothing to report.".to_string()
        } else {
            summary
        };
        self.tell_done(job, playbook, &run.id, &summary).await;
        Handled::Succeeded {
            summary,
            scheduled_for: None,
        }
    }

    fn failed(&self, model: &RunModel, failure: AgentFailure) -> Handled {
        let now = self.engine.now();
        match failure.kind {
            FailureKind::RateLimited | FailureKind::Quota => {
                let until = failure.resets_at.filter(|at| *at > now).unwrap_or_else(|| {
                    if failure.kind == FailureKind::Quota {
                        next_utc_midnight(now)
                    } else {
                        now + TimeDelta::seconds(SHORT_LIMIT_SECS)
                    }
                });
                self.env.hold_pool(&model.pool, until);
                Handled::Defer {
                    at: until,
                    reason: WaitReason::Quota,
                }
            }
            FailureKind::ModelUnavailable => Handled::RetryAt {
                at: now + TimeDelta::minutes(MODEL_RETRY_MINUTES),
                reason: WaitReason::ModelUnavailable,
                code: MODEL_UNAVAILABLE.into(),
                detail: failure.detail,
            },
            FailureKind::Network => Handled::RetryAt {
                at: now + TimeDelta::minutes(MODEL_RETRY_MINUTES),
                reason: WaitReason::Offline,
                code: "offline".into(),
                detail: failure.detail,
            },
            FailureKind::Setup => Handled::Permanent {
                code: MODEL_UNAVAILABLE.into(),
                detail: failure.detail,
            },
            FailureKind::Other => Handled::Permanent {
                code: "agent_failed".into(),
                detail: failure.detail,
            },
        }
    }

    async fn ask_approval(
        &self,
        job: &Job,
        run: &Run,
        playbook: &Playbook,
        writes: Vec<DeferredCall>,
    ) -> Handled {
        let title = approval_title(&writes);
        let store = self.engine.store().clone();
        let (job_id, run_id, now) = (job.id.clone(), run.id.clone(), self.engine.now());
        let approval_id = uuid::Uuid::new_v4().to_string();
        let writes_json = serde_json::to_value(&writes).unwrap_or_else(|_| json!([]));
        let title_saved = title.clone();
        let opened = blocking(move || {
            store.open_approval(
                &approval_id,
                &job_id,
                &run_id,
                &title_saved,
                &writes_json,
                now,
            )
        })
        .await;
        let approval = match opened {
            Ok(approval) => approval,
            Err(e) => {
                return Handled::Transient {
                    code: "approval_unsaved".into(),
                    detail: e,
                }
            }
        };
        self.post_approval(job, playbook, &approval).await;
        self.env.notify(
            Category::NeedsYou,
            &playbook.name,
            format!("{title}. Review it in Automations."),
        );
        Handled::Waiting {
            reason: WaitReason::Approval,
        }
    }

    async fn post_approval(&self, job: &Job, playbook: &Playbook, approval: &Approval) {
        self.env
            .post(NewInboxItem {
                id: approval_item(&approval.id),
                kind: InboxKind::Automation,
                status: InboxStatus::NeedsYou,
                title: format!("{}: {}", playbook.name, approval.title),
                detail: None,
                link: Some(InboxLink::new(
                    "automations",
                    Some(json!({
                        "kind": "automation",
                        "automationId": job.id,
                        "runId": approval.run_id,
                    })),
                )),
                data: json!({
                    "jobId": job.id,
                    "runId": approval.run_id,
                    "approvalId": approval.id,
                }),
            })
            .await;
    }
}

#[async_trait]
impl JobHandler for PlaybookHandler {
    fn kind(&self) -> JobKind {
        JobKind::Playbook
    }

    async fn run(&self, job: &Job, run: &Run) -> Handled {
        let playbook = match Playbook::of(job) {
            Ok(playbook) => playbook,
            Err(detail) => {
                return Handled::Permanent {
                    code: "unreadable".into(),
                    detail,
                }
            }
        };
        let store = self.engine.store().clone();
        let id = run.id.clone();
        match blocking(move || store.approval_of_run(&id)).await {
            Ok(Some(approval)) => return self.after_approval(job, &playbook, approval).await,
            Ok(None) => {}
            Err(e) => {
                return Handled::Transient {
                    code: "approval_unreadable".into(),
                    detail: e,
                }
            }
        }
        // A run for a missed time gives the computer a moment after it woke.
        if run.trigger == Trigger::CatchUp {
            let now = self.engine.now();
            if let Some(started) = run.started_at.as_deref().and_then(parse_instant) {
                let ready = started + TimeDelta::minutes(AFTER_WAKE_MINUTES);
                if now < ready {
                    return Handled::Defer {
                        at: ready,
                        reason: WaitReason::Backoff,
                    };
                }
            }
        }
        self.agent_step(job, run, &playbook).await
    }

    async fn folded(&self, _job: &Job, _run: &Run) {
        self.env.changed();
    }

    async fn held(&self, job: &Job, run: &Run) {
        let model = run.error_code.as_deref() == Some(MODEL_UNAVAILABLE);
        if model {
            let store = self.engine.store().clone();
            let (id, now) = (job.id.clone(), self.engine.now());
            if let Err(e) = blocking(move || {
                store.update_job(
                    &id,
                    &JobPatch {
                        hold_reason: Some(Some(HoldReason::ModelUnavailable)),
                        ..JobPatch::default()
                    },
                    now,
                )
            })
            .await
            {
                tracing::warn!(error = %e, "the hold's reason could not be saved");
            }
        }
        let name = Playbook::of(job).map_or_else(|_| job.title.clone(), |p| p.name);
        let why = run
            .error_detail
            .clone()
            .unwrap_or_else(|| "it failed 3 times in a row".to_string());
        self.env
            .post(NewInboxItem {
                id: hold_item(&job.id),
                kind: InboxKind::Automation,
                status: InboxStatus::NeedsYou,
                title: format!("{name} is on hold"),
                detail: Some(why.clone()),
                link: Some(InboxLink::new(
                    "automations",
                    Some(json!({ "kind": "automation", "automationId": job.id, "runId": null })),
                )),
                data: json!({ "jobId": job.id, "fix": "resume", "resume": "automation", "model": model }),
            })
            .await;
        self.env
            .notify(Category::Problems, &format!("{name} is on hold"), why);
        self.env.changed();
    }
}

/// Where an automation stands, for its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationState {
    /// Runs on its schedule.
    Scheduled,
    /// Runs only when started.
    Manual,
    /// A run is going.
    Running,
    /// Waiting while the user chats, or for its model.
    Waiting,
    /// Waiting for the provider's limit to reset.
    Paused,
    /// Waiting for the network.
    Offline,
    /// Trying again after a failure, or a moment after a wake.
    Retrying,
    /// Waiting for the user's OK on its changes.
    NeedsYou,
    /// Stopped after failing again and again.
    OnHold,
    /// Turned off by the user.
    Off,
}

/// A run as the view shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    #[serde(flatten)]
    pub run: Run,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<Approval>,
}

/// An automation as the view and the assistant see it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationView {
    pub id: String,
    pub name: String,
    pub instruction: String,
    pub workspace_id: Option<String>,
    pub schedule: Option<Schedule>,
    /// When it runs, in words.
    pub when: String,
    pub next_due_at: Option<String>,
    pub enabled: bool,
    pub hold_reason: Option<HoldReason>,
    pub state: AutomationState,
    pub active: Option<Run>,
    pub last: Option<Run>,
    /// The model calls one run plans.
    pub planned_calls: u32,
    pub created_by: CreatedBy,
}

fn state_of(job: &Job, active: Option<&Run>) -> AutomationState {
    if !job.enabled {
        return AutomationState::Off;
    }
    if job.hold_reason.is_some() {
        return AutomationState::OnHold;
    }
    match active {
        Some(run) => match (run.state, run.wait_reason) {
            (RunState::Waiting, Some(WaitReason::Approval)) => AutomationState::NeedsYou,
            (RunState::Waiting, Some(WaitReason::Quota)) => AutomationState::Paused,
            (RunState::Waiting, Some(WaitReason::Offline)) => AutomationState::Offline,
            (RunState::Waiting, Some(WaitReason::Yield | WaitReason::ModelUnavailable)) => {
                AutomationState::Waiting
            }
            (RunState::Waiting, _) => AutomationState::Retrying,
            _ => AutomationState::Running,
        },
        None if job.schedule.is_some() => AutomationState::Scheduled,
        None => AutomationState::Manual,
    }
}

/// The user's decision on a run's changes: which to make, by their index.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub approval_id: String,
    pub chosen: Vec<usize>,
}

/// Automations over the engine.
pub struct Automations {
    engine: Arc<Engine>,
    env: Arc<dyn AutomationEnv>,
    handler: Arc<dyn JobHandler>,
    /// Decisions being made now, so a double click never makes a change twice.
    deciding: Mutex<HashMap<String, ()>>,
}

impl Automations {
    pub fn new(engine: Arc<Engine>, env: Arc<dyn AutomationEnv>) -> Self {
        let handler: Arc<dyn JobHandler> = Arc::new(PlaybookHandler {
            engine: engine.clone(),
            env: env.clone(),
        });
        Self {
            engine,
            env,
            handler,
            deciding: Mutex::new(HashMap::new()),
        }
    }

    pub fn handler(&self) -> Arc<dyn JobHandler> {
        self.handler.clone()
    }

    fn view(&self, job: Job) -> Result<AutomationView, String> {
        let playbook = Playbook::of(&job)?;
        let active = self
            .engine
            .store()
            .active_run(&job.id)
            .map_err(|e| e.to_string())?;
        let last = self
            .engine
            .store()
            .runs(&job.id, 4)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| !r.state.is_active());
        Ok(AutomationView {
            state: state_of(&job, active.as_ref()),
            when: when_label(job.schedule.as_ref()),
            id: job.id,
            name: playbook.name,
            instruction: playbook.instruction,
            workspace_id: playbook.workspace_id,
            schedule: job.schedule,
            next_due_at: job.next_due_at,
            enabled: job.enabled,
            hold_reason: job.hold_reason,
            active,
            last,
            planned_calls: AGENT_STEP_CALLS,
            created_by: job.created_by,
        })
    }

    fn job(&self, id: &str) -> Result<Job, String> {
        self.engine
            .store()
            .job(id)
            .map_err(|e| e.to_string())?
            .filter(|j| j.kind == JobKind::Playbook)
            .ok_or_else(|| format!("No automation {id}."))
    }

    /// Every automation, oldest first.
    pub fn list(&self) -> Result<Vec<AutomationView>, String> {
        self.engine
            .store()
            .jobs(JobKind::Playbook)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|job| self.view(job))
            .collect()
    }

    pub fn get(&self, id: &str) -> Result<AutomationView, String> {
        self.view(self.job(id)?)
    }

    /// Saves a new automation.
    pub fn create(
        &self,
        draft: Draft,
        created_by: CreatedBy,
        provenance: Value,
    ) -> Result<AutomationView, String> {
        let draft = draft.checked()?;
        let count = self
            .engine
            .store()
            .jobs(JobKind::Playbook)
            .map_err(|e| e.to_string())?
            .len();
        if count >= MAX_AUTOMATIONS {
            return Err(format!(
                "Shodh keeps at most {MAX_AUTOMATIONS} automations. Delete one first."
            ));
        }
        let id = format!("auto-{}", uuid::Uuid::new_v4().simple());
        let playbook = Playbook {
            name: draft.name.clone(),
            instruction: draft.instruction,
            workspace_id: draft.workspace_id,
            revision: 1,
        };
        let job = self
            .engine
            .store()
            .create_job(
                &NewJob {
                    id: id.clone(),
                    kind: JobKind::Playbook,
                    subject: id.clone(),
                    title: draft.name,
                    schedule: draft.schedule,
                    input: serde_json::to_value(&playbook).map_err(|e| e.to_string())?,
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 3,
                    created_by,
                    provenance,
                },
                self.engine.now(),
            )
            .map_err(|e| e.to_string())?;
        self.env.changed();
        self.view(job)
    }

    /// Changes what an automation does or when.
    pub fn update(&self, id: &str, draft: Draft) -> Result<AutomationView, String> {
        let job = self.job(id)?;
        let draft = draft.checked()?;
        let mut playbook = Playbook::of(&job)?;
        playbook.name = draft.name.clone();
        playbook.instruction = draft.instruction;
        playbook.workspace_id = draft.workspace_id;
        playbook.revision = playbook.revision.saturating_add(1);
        let store = self.engine.store();
        let now = self.engine.now();
        let patch = JobPatch {
            title: Some(draft.name),
            schedule: draft.schedule.clone(),
            input: Some(serde_json::to_value(&playbook).map_err(|e| e.to_string())?),
            ..JobPatch::default()
        };
        store
            .update_job(id, &patch, now)
            .map_err(|e| e.to_string())?;
        if draft.schedule.is_none() && job.schedule.is_some() {
            // Only when started from now on.
            clear_schedule(store, id).map_err(|e| e.to_string())?;
        }
        self.env.changed();
        self.get(id)
    }

    /// Deletes an automation with its runs; its Inbox items go too.
    pub async fn delete(&self, id: &str) -> Result<bool, String> {
        let job = self.job(id)?;
        let store = self.engine.store().clone();
        let job_id = job.id.clone();
        let open: Vec<String> = blocking(move || {
            Ok(store
                .open_approvals()?
                .into_iter()
                .filter(|a| a.job_id == job_id)
                .map(|a| a.id)
                .collect())
        })
        .await?;
        let store = self.engine.store().clone();
        let job_id = job.id.clone();
        let deleted = blocking(move || store.delete_job(&job_id)).await?;
        for approval in open {
            self.env.resolve(&approval_item(&approval)).await;
        }
        self.env.resolve(&hold_item(&job.id)).await;
        self.env.changed();
        Ok(deleted)
    }

    /// Turns an automation off (nothing starts it) or on again; turning it on clears a hold.
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<AutomationView, String> {
        let job = self.job(id)?;
        let patch = JobPatch {
            enabled: Some(enabled),
            hold_reason: enabled.then_some(None),
            reset_failures: enabled,
            ..JobPatch::default()
        };
        let store = self.engine.store().clone();
        let (job_id, now) = (job.id.clone(), self.engine.now());
        blocking(move || store.update_job(&job_id, &patch, now)).await?;
        if enabled {
            self.env.resolve(&hold_item(&job.id)).await;
        }
        self.env.changed();
        self.get(id)
    }

    /// Starts a run of an automation now (its conversation exists when this returns).
    /// Refused while a run of it is active.
    pub async fn run_now(&self, id: &str) -> Result<Run, String> {
        let job = self.job(id)?;
        if job.hold_reason.is_some() || !job.enabled {
            return Err("Turn the automation on first.".into());
        }
        let playbook = Playbook::of(&job)?;
        let store = self.engine.store().clone();
        let run_id = uuid::Uuid::new_v4().to_string();
        let new = NewRun {
            id: run_id.clone(),
            job_id: job.id.clone(),
            trigger: Trigger::Manual,
            scheduled_for: None,
            missed: 0,
            state: RunState::Queued,
            summary: None,
        };
        let now = self.engine.now();
        let inserted = blocking(move || store.insert_run(&new, now)).await?;
        let run = inserted.map_err(|_| "It is already running.".to_string())?;
        let request = AgentRequest {
            run_id: run.id.clone(),
            automation_id: job.id.clone(),
            name: playbook.name,
            conversation_id: conversation_of(&run.id),
            instruction: playbook.instruction,
            workspace_id: playbook.workspace_id,
            model: None,
            planned: AGENT_STEP_CALLS,
        };
        if let Err(e) = self.env.prepare_conversation(&request).await {
            tracing::warn!(error = %e, "a run's conversation could not be made yet");
        }
        let (engine, handler, job_id) = (self.engine.clone(), self.handler.clone(), job.id.clone());
        let started = run.id.clone();
        tokio::spawn(async move {
            if let Err(e) = engine.run_now(&handler, &job_id, &started).await {
                tracing::warn!(error = %e, "a run started by the user could not run");
            }
        });
        self.env.changed();
        let store = self.engine.store().clone();
        let id = run.id.clone();
        Ok(blocking(move || store.run(&id)).await?.unwrap_or(run))
    }

    /// An automation's latest runs, newest first.
    pub fn runs(&self, id: &str, limit: usize) -> Result<Vec<Run>, String> {
        let job = self.job(id)?;
        self.engine
            .store()
            .runs(&job.id, limit.clamp(1, RUNS_SHOWN))
            .map_err(|e| e.to_string())
    }

    /// One run with its steps and its approval.
    pub fn run(&self, run_id: &str) -> Result<RunView, String> {
        let store = self.engine.store();
        let run = store
            .run(run_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No run {run_id}."))?;
        self.job(&run.job_id)?;
        Ok(RunView {
            steps: store.steps(run_id).map_err(|e| e.to_string())?,
            approval: store.approval_of_run(run_id).map_err(|e| e.to_string())?,
            run,
        })
    }

    /// Makes the changes the user chose of a run's approval, then lets the run finish.
    /// The decision is recorded first, so a crash never makes a change twice.
    pub async fn decide(&self, decision: Decision) -> Result<RunView, String> {
        {
            let mut deciding = self.deciding.lock().unwrap_or_else(|e| e.into_inner());
            if deciding.contains_key(&decision.approval_id) {
                return Err("That decision is being saved.".into());
            }
            deciding.insert(decision.approval_id.clone(), ());
        }
        let result = self.decide_once(&decision).await;
        self.deciding
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&decision.approval_id);
        result
    }

    async fn decide_once(&self, decision: &Decision) -> Result<RunView, String> {
        let store = self.engine.store().clone();
        let id = decision.approval_id.clone();
        let approval = blocking(move || store.approval(&id))
            .await?
            .ok_or_else(|| "These changes are no longer waiting.".to_string())?;
        if approval.decision.is_some() {
            return Err("These changes were already decided.".into());
        }
        let writes: Vec<DeferredCall> =
            serde_json::from_value(approval.writes.clone()).map_err(|e| e.to_string())?;
        let mut chosen: Vec<usize> = decision
            .chosen
            .iter()
            .copied()
            .filter(|i| *i < writes.len())
            .collect();
        chosen.sort_unstable();
        chosen.dedup();
        let outcome = match chosen.len() {
            0 => ApprovalOutcome::Denied,
            n if n == writes.len() => ApprovalOutcome::Approved,
            _ => ApprovalOutcome::Partly,
        };
        let store = self.engine.store().clone();
        let (id, now) = (approval.id.clone(), self.engine.now());
        let first = json!({ "chosen": chosen, "made": 0, "failed": 0 });
        let decided = blocking(move || store.decide_approval(&id, outcome, &first, now))
            .await?
            .ok_or_else(|| "These changes were already decided.".to_string())?;
        let conversation = conversation_of(&decided.run_id);
        let mut results = Vec::new();
        let (mut made, mut failed) = (0u64, 0u64);
        for index in &chosen {
            let write = &writes[*index];
            match self.env.run_approved(&conversation, write).await {
                Ok(text) => {
                    made += 1;
                    results.push(json!({ "index": index, "ok": true, "text": text }));
                }
                Err(error) => {
                    failed += 1;
                    results.push(json!({ "index": index, "ok": false, "text": error }));
                }
            }
        }
        let store = self.engine.store().clone();
        let id = decided.id.clone();
        let result =
            json!({ "chosen": chosen, "made": made, "failed": failed, "results": results });
        blocking(move || set_approval_result(&store, &id, &result)).await?;
        self.env.resolve(&approval_item(&decided.id)).await;
        let resumed = self
            .engine
            .resume(&self.handler, &decided.job_id, &decided.run_id)
            .await
            .map_err(|e| e.to_string())?;
        if resumed.is_none() {
            tracing::warn!(run = %decided.run_id, "an approved run was not waiting any more");
        }
        self.env.changed();
        self.run(&decided.run_id)
    }

    /// At launch: every approval nobody decided is in the Inbox again (the Inbox drops
    /// session approvals when it opens).
    pub async fn launch(&self) {
        let store = self.engine.store().clone();
        let open = match blocking(move || store.open_approvals()).await {
            Ok(open) => open,
            Err(e) => {
                tracing::warn!(error = %e, "open approvals could not be read");
                return;
            }
        };
        let handler = PlaybookHandler {
            engine: self.engine.clone(),
            env: self.env.clone(),
        };
        for approval in open {
            let Ok(job) = self.job(&approval.job_id) else {
                continue;
            };
            let Ok(playbook) = Playbook::of(&job) else {
                continue;
            };
            handler.post_approval(&job, &playbook, &approval).await;
        }
    }

    /// Ends the runs whose approval expired. Call on every turn of the loop.
    pub async fn sweep(&self) {
        let store = self.engine.store().clone();
        let now = self.engine.now();
        let overdue: Vec<Approval> = match blocking(move || store.open_approvals()).await {
            Ok(open) => open.into_iter().filter(|a| a.is_overdue(now)).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "open approvals could not be read");
                return;
            }
        };
        for approval in overdue {
            if let Err(e) = self
                .engine
                .resume(&self.handler, &approval.job_id, &approval.run_id)
                .await
            {
                tracing::warn!(error = %e, "an expired approval's run could not end");
            }
            self.env.changed();
        }
    }

    /// When the loop should look again for an approval to expire.
    pub fn next_expiry(&self) -> Option<DateTime<Utc>> {
        self.engine
            .store()
            .open_approvals()
            .ok()?
            .iter()
            .filter_map(|a| parse_instant(&a.expires_at))
            .min()
    }
}

/// Makes an automation run only when started (its schedule removed).
fn clear_schedule(
    store: &shodh_rag::engine::store::EngineStore,
    id: &str,
) -> Result<(), EngineError> {
    store.clear_schedule(id)
}

fn set_approval_result(
    store: &shodh_rag::engine::store::EngineStore,
    id: &str,
    result: &Value,
) -> Result<(), EngineError> {
    store.set_approval_result(id, result)
}

// ── Commands ────────────────────────────────────────────────────────

async fn service(app: &tauri::AppHandle) -> Result<Arc<Automations>, String> {
    crate::engine_commands::automations_for_command(app).await
}

/// Every automation.
#[tauri::command]
pub async fn automations_list(app: tauri::AppHandle) -> Result<Vec<AutomationView>, String> {
    let automations = service(&app).await?;
    tokio::task::spawn_blocking(move || automations.list())
        .await
        .map_err(|e| e.to_string())?
}

/// One automation.
#[tauri::command]
pub async fn automation_get(app: tauri::AppHandle, id: String) -> Result<AutomationView, String> {
    let automations = service(&app).await?;
    tokio::task::spawn_blocking(move || automations.get(&id))
        .await
        .map_err(|e| e.to_string())?
}

/// An automation's latest runs, newest first.
#[tauri::command]
pub async fn automation_runs(
    app: tauri::AppHandle,
    id: String,
    limit: Option<usize>,
) -> Result<Vec<Run>, String> {
    let automations = service(&app).await?;
    tokio::task::spawn_blocking(move || automations.runs(&id, limit.unwrap_or(RUNS_SHOWN)))
        .await
        .map_err(|e| e.to_string())?
}

/// One run, with its steps and the changes it waits on.
#[tauri::command]
pub async fn automation_run_get(app: tauri::AppHandle, run_id: String) -> Result<RunView, String> {
    let automations = service(&app).await?;
    tokio::task::spawn_blocking(move || automations.run(&run_id))
        .await
        .map_err(|e| e.to_string())?
}

/// Saves a new automation the user wrote.
#[tauri::command]
pub async fn automation_create(
    app: tauri::AppHandle,
    draft: Draft,
) -> Result<AutomationView, String> {
    let automations = service(&app).await?;
    let view = tokio::task::spawn_blocking(move || {
        automations.create(draft, CreatedBy::User, json!({ "via": "automations_view" }))
    })
    .await
    .map_err(|e| e.to_string())??;
    crate::engine_commands::wake_automations(&app);
    Ok(view)
}

/// Changes an automation.
#[tauri::command]
pub async fn automation_update(
    app: tauri::AppHandle,
    id: String,
    draft: Draft,
) -> Result<AutomationView, String> {
    let automations = service(&app).await?;
    let view = tokio::task::spawn_blocking(move || automations.update(&id, draft))
        .await
        .map_err(|e| e.to_string())??;
    crate::engine_commands::wake_automations(&app);
    Ok(view)
}

/// Deletes an automation and its runs.
#[tauri::command]
pub async fn automation_delete(app: tauri::AppHandle, id: String) -> Result<bool, String> {
    service(&app).await?.delete(&id).await
}

/// Turns an automation off.
#[tauri::command]
pub async fn automation_pause(app: tauri::AppHandle, id: String) -> Result<AutomationView, String> {
    let view = service(&app).await?.set_enabled(&id, false).await?;
    crate::engine_commands::wake_automations(&app);
    Ok(view)
}

/// Turns an automation on again (clearing a hold).
#[tauri::command]
pub async fn automation_resume(
    app: tauri::AppHandle,
    id: String,
) -> Result<AutomationView, String> {
    let view = service(&app).await?.set_enabled(&id, true).await?;
    crate::engine_commands::wake_automations(&app);
    Ok(view)
}

/// Starts a run now. Its conversation exists when this returns.
#[tauri::command]
pub async fn automation_run_now(app: tauri::AppHandle, id: String) -> Result<Run, String> {
    service(&app).await?.run_now(&id).await
}

/// The user's decision on a run's changes. User-only: the assistant never approves its
/// own steps.
#[tauri::command]
pub async fn engine_approval_decide(
    app: tauri::AppHandle,
    decision: Decision,
) -> Result<RunView, String> {
    let view = service(&app).await?.decide(decision).await?;
    crate::engine_commands::wake_automations(&app);
    Ok(view)
}

/// Chooses the model automations run with (`None`: the chat's), checked like any model
/// selection. User-only: it decides whose quota automations spend.
#[tauri::command]
pub async fn automations_model_set(
    app: tauri::AppHandle,
    model: Option<ModelRef>,
) -> Result<crate::app_settings::AppSettings, String> {
    use tauri::Manager;
    if let Some(model) = &model {
        let llm = app.state::<crate::llm_commands::LLMState>();
        crate::model_picker_commands::override_mode(
            &app,
            &llm,
            &crate::model_picker_commands::ModelOverride {
                model: model.clone(),
                automatic: false,
                failure: None,
            },
        )
        .map_err(|e| e.message)?;
    }
    let dir = crate::profile::app_data_dir(&app)
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;
    let (settings, ()) = crate::app_settings::SettingsStore::in_dir(&dir)
        .update(|s| {
            s.automations.model = model;
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    crate::app_settings::broadcast(&app, &settings);
    crate::engine_commands::wake_automations(&app);
    Ok(settings)
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::collections::VecDeque;

    /// The app's side of automations, scripted: the agent steps' outcomes in order, and
    /// everything the runner told the Inbox and the desktop.
    #[derive(Default)]
    pub struct FakeEnv {
        pub model: Mutex<Option<Result<RunModel, ModelBlock>>>,
        pub answering: Mutex<bool>,
        pub cooldowns: Mutex<HashMap<String, DateTime<Utc>>>,
        pub pool: Mutex<Option<PoolToday>>,
        pub outcomes: Mutex<VecDeque<AgentOutcome>>,
        pub requests: Mutex<Vec<AgentRequest>>,
        pub prepared: Mutex<Vec<String>>,
        pub approved: Mutex<Vec<DeferredCall>>,
        pub refuse_approved: Mutex<Vec<String>>,
        pub posted: Mutex<Vec<NewInboxItem>>,
        pub resolved: Mutex<Vec<String>>,
        pub notified: Mutex<Vec<(Category, String)>>,
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl FakeEnv {
        pub fn free_model() -> RunModel {
            RunModel {
                model: None,
                pool: FREE_POOL.to_string(),
                shares_chat: true,
            }
        }

        pub fn script(&self, outcome: AgentOutcome) {
            lock(&self.outcomes).push_back(outcome);
        }

        pub fn posted_ids(&self) -> Vec<String> {
            lock(&self.posted).iter().map(|i| i.id.clone()).collect()
        }

        pub fn requests(&self) -> usize {
            lock(&self.requests).len()
        }
    }

    #[async_trait]
    impl AutomationEnv for FakeEnv {
        fn run_model(&self) -> Result<RunModel, ModelBlock> {
            lock(&self.model)
                .clone()
                .unwrap_or_else(|| Ok(Self::free_model()))
        }
        fn user_answering(&self) -> bool {
            *lock(&self.answering)
        }
        fn cooldown(&self, pool: &str) -> Option<DateTime<Utc>> {
            lock(&self.cooldowns).get(pool).copied()
        }
        fn hold_pool(&self, pool: &str, until: DateTime<Utc>) {
            lock(&self.cooldowns).insert(pool.to_string(), until);
        }
        async fn pool_today(&self, _pool: &str) -> Option<PoolToday> {
            *lock(&self.pool)
        }
        async fn prepare_conversation(&self, request: &AgentRequest) -> Result<(), String> {
            lock(&self.prepared).push(request.conversation_id.clone());
            Ok(())
        }
        async fn run_agent(&self, request: &AgentRequest, meter: &dyn CallMeter) -> AgentOutcome {
            lock(&self.requests).push(request.clone());
            let outcome = lock(&self.outcomes).pop_front().unwrap_or_default();
            for _ in 0..outcome.calls {
                meter.call().await;
            }
            outcome
        }
        async fn run_approved(
            &self,
            _conversation_id: &str,
            write: &DeferredCall,
        ) -> Result<String, String> {
            if lock(&self.refuse_approved).contains(&write.label) {
                return Err(format!("{} could not be made", write.label));
            }
            lock(&self.approved).push(write.clone());
            Ok(format!("Made: {}", write.label))
        }
        async fn post(&self, item: NewInboxItem) {
            lock(&self.posted).push(item);
        }
        async fn resolve(&self, id: &str) {
            lock(&self.resolved).push(id.to_string());
        }
        fn notify(&self, category: Category, title: &str, _body: String) {
            lock(&self.notified).push((category, title.to_string()));
        }
        fn changed(&self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeEnv;
    use super::*;
    use crate::task_repeat::testing::TestClock;
    use chrono::{NaiveTime, TimeZone};
    use shodh_rag::engine::store::EngineStore;
    use shodh_rag::harness::RiskTier;

    struct Env {
        _dir: tempfile::TempDir,
        clock: Arc<TestClock>,
        engine: Arc<Engine>,
        env: Arc<FakeEnv>,
        automations: Automations,
    }

    fn start() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 25, 6, 0, 0).unwrap()
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        let clock = Arc::new(TestClock(Mutex::new(start())));
        let engine = Arc::new(Engine::with_clock(store, clock.clone(), "automations-test"));
        let env = Arc::new(FakeEnv::default());
        let automations = Automations::new(engine.clone(), env.clone());
        Env {
            _dir: dir,
            clock,
            engine,
            env,
            automations,
        }
    }

    fn draft(schedule: Option<Schedule>) -> Draft {
        Draft {
            name: "Weekly digest".into(),
            instruction: "Summarise what changed in my thesis sources.".into(),
            schedule,
            workspace_id: Some("thesis".into()),
        }
    }

    fn daily() -> Schedule {
        Schedule::Daily {
            at: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
        }
    }

    fn answered(calls: u32, answer: &str) -> AgentOutcome {
        AgentOutcome {
            calls,
            answer: answer.into(),
            ..AgentOutcome::default()
        }
    }

    fn write(label: &str) -> DeferredCall {
        DeferredCall {
            step_id: "s1".into(),
            tool: "create_task".into(),
            label: label.into(),
            tier: RiskTier::Write,
            args: json!({ "title": label }),
            preview: json!({ "title": label }),
        }
    }

    async fn run_now(e: &Env, id: &str) -> Run {
        let run = e.automations.run_now(id).await.unwrap();
        // The run goes in the background: wait for it to leave the queue.
        for _ in 0..200 {
            let now = e.engine.store().run(&run.id).unwrap().unwrap();
            if now.state != RunState::Queued && now.state != RunState::Running {
                return now;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the run did not finish");
    }

    #[tokio::test]
    async fn a_run_answers_counts_its_calls_and_tells_the_inbox_once() {
        let e = env();
        let auto = e
            .automations
            .create(draft(None), CreatedBy::User, json!({}))
            .unwrap();
        assert_eq!(auto.state, AutomationState::Manual);
        assert_eq!(auto.when, "Only when you start it");
        e.env
            .script(answered(3, "## Four new papers\nDetails follow."));
        let run = run_now(&e, &auto.id).await;
        assert_eq!(run.state, RunState::Succeeded);
        assert_eq!(run.summary.as_deref(), Some("Four new papers"));
        assert_eq!(run.planned_calls, AGENT_STEP_CALLS);
        assert_eq!(run.used_calls, 3);
        assert_eq!(run.conversation_id, Some(conversation_of(&run.id)));
        let prepared = e.env.prepared.lock().unwrap().clone();
        assert!(
            !prepared.is_empty(),
            "the conversation exists before the first answer"
        );
        assert!(prepared.iter().all(|c| *c == conversation_of(&run.id)));
        assert_eq!(
            e.env.posted_ids(),
            vec![run_item(&run.id)],
            "one Info item per run"
        );
        let steps = e.automations.run(&run.id).unwrap().steps;
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].kind, StepKind::Agent);
        assert_eq!(steps[0].calls, 3);
        assert_eq!(steps[1].kind, StepKind::Notify);
        assert!(
            e.env.notified.lock().unwrap().is_empty(),
            "no desktop notice for a success"
        );
        let request = e.env.requests.lock().unwrap()[0].clone();
        assert_eq!(request.planned, AGENT_STEP_CALLS);
        assert_eq!(request.workspace_id.as_deref(), Some("thesis"));
    }

    #[tokio::test]
    async fn a_run_stopped_at_its_estimate_says_so() {
        let e = env();
        let auto = e
            .automations
            .create(draft(None), CreatedBy::User, json!({}))
            .unwrap();
        e.env.script(AgentOutcome {
            calls: AGENT_STEP_CALLS,
            answer: "Found two papers so far.".into(),
            stopped_at_estimate: true,
            ..AgentOutcome::default()
        });
        let run = run_now(&e, &auto.id).await;
        assert_eq!(run.used_calls, AGENT_STEP_CALLS);
        assert!(
            run.used_calls <= run.planned_calls,
            "never past the estimate"
        );
        assert_eq!(
            run.summary.as_deref(),
            Some("Stopped at its estimate of 4 calls. Found two papers so far.")
        );
    }

    #[tokio::test]
    async fn changes_wait_for_one_approval_and_make_exactly_what_was_chosen() {
        let e = env();
        let auto = e
            .automations
            .create(draft(None), CreatedBy::User, json!({}))
            .unwrap();
        e.env.script(AgentOutcome {
            calls: 2,
            answer: "Three tasks would help.".into(),
            writes: vec![
                write("Add task A"),
                write("Add task B"),
                write("Add task C"),
            ],
            ..AgentOutcome::default()
        });
        let run = run_now(&e, &auto.id).await;
        assert_eq!(run.state, RunState::Waiting);
        assert_eq!(run.wait_reason, Some(WaitReason::Approval));
        assert!(e.env.approved.lock().unwrap().is_empty(), "nothing ran");
        let view = e.automations.get(&auto.id).unwrap();
        assert_eq!(view.state, AutomationState::NeedsYou);
        let approval = e.automations.run(&run.id).unwrap().approval.unwrap();
        assert_eq!(approval.title, "Wants to make 3 changes");
        assert_eq!(e.env.posted_ids(), vec![approval_item(&approval.id)]);
        assert_eq!(e.env.notified.lock().unwrap()[0].0, Category::NeedsYou);
        e.env
            .refuse_approved
            .lock()
            .unwrap()
            .push("Add task C".into());
        let done = e
            .automations
            .decide(Decision {
                approval_id: approval.id.clone(),
                chosen: vec![2, 0, 0, 9],
            })
            .await
            .unwrap();
        let approved: Vec<String> = e
            .env
            .approved
            .lock()
            .unwrap()
            .iter()
            .map(|w| w.label.clone())
            .collect();
        assert_eq!(
            approved,
            vec!["Add task A"],
            "only what was chosen, once each"
        );
        assert_eq!(done.run.state, RunState::Succeeded);
        assert_eq!(
            done.run.summary.as_deref(),
            Some("Made 1 change you approved. 1 could not be made.")
        );
        let decided = done.approval.unwrap();
        assert_eq!(decided.decision, Some(ApprovalOutcome::Partly));
        assert!(e
            .env
            .resolved
            .lock()
            .unwrap()
            .contains(&approval_item(&approval.id)));
        assert!(
            e.automations
                .decide(Decision {
                    approval_id: approval.id,
                    chosen: vec![1],
                })
                .await
                .is_err(),
            "a second decision changes nothing"
        );
        assert_eq!(e.env.approved.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn declining_every_change_makes_none() {
        let e = env();
        let auto = e
            .automations
            .create(draft(None), CreatedBy::User, json!({}))
            .unwrap();
        e.env.script(AgentOutcome {
            calls: 1,
            writes: vec![write("Add task A")],
            ..AgentOutcome::default()
        });
        let run = run_now(&e, &auto.id).await;
        let approval = e.automations.run(&run.id).unwrap().approval.unwrap();
        assert_eq!(approval.title, "Wants to: Add task A");
        let done = e
            .automations
            .decide(Decision {
                approval_id: approval.id,
                chosen: vec![],
            })
            .await
            .unwrap();
        assert!(e.env.approved.lock().unwrap().is_empty());
        assert_eq!(
            done.run.summary.as_deref(),
            Some("You declined its changes.")
        );
    }

    #[tokio::test]
    async fn approvals_are_posted_again_at_launch_and_expire_after_seven_days() {
        let e = env();
        let auto = e
            .automations
            .create(draft(None), CreatedBy::User, json!({}))
            .unwrap();
        e.env.script(AgentOutcome {
            calls: 1,
            writes: vec![write("Add task A")],
            ..AgentOutcome::default()
        });
        let run = run_now(&e, &auto.id).await;
        let approval = e.automations.run(&run.id).unwrap().approval.unwrap();
        e.env.posted.lock().unwrap().clear();
        e.automations.launch().await;
        assert_eq!(e.env.posted_ids(), vec![approval_item(&approval.id)]);
        e.clock
            .set(start() + TimeDelta::days(APPROVAL_DAYS) - TimeDelta::minutes(1));
        e.automations.sweep().await;
        assert_eq!(
            e.engine.store().run(&run.id).unwrap().unwrap().state,
            RunState::Waiting
        );
        e.clock.set(start() + TimeDelta::days(APPROVAL_DAYS));
        e.automations.sweep().await;
        let ended = e.engine.store().run(&run.id).unwrap().unwrap();
        assert_eq!(ended.state, RunState::Skipped);
        assert_eq!(
            ended.summary.as_deref(),
            Some("Expired: nobody approved these changes in 7 days.")
        );
        assert!(e.env.approved.lock().unwrap().is_empty());
        assert_eq!(
            e.engine
                .store()
                .approval(&approval.id)
                .unwrap()
                .unwrap()
                .decision,
            Some(ApprovalOutcome::Expired)
        );
    }

    /// A daily automation at 09:00, made an hour before the first one.
    async fn scheduled(e: &Env) -> String {
        e.clock.set(at_nine() - TimeDelta::hours(1));
        let auto = e
            .automations
            .create(draft(Some(daily())), CreatedBy::User, json!({}))
            .unwrap();
        auto.id
    }

    fn at_nine() -> DateTime<Utc> {
        shodh_rag::engine::clock::resolve_local(
            &Local,
            chrono::NaiveDate::from_ymd_opt(2026, 11, 26)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_scheduled_run_waits_while_the_user_chats_and_then_runs() {
        let e = env();
        let id = scheduled(&e).await;
        *e.env.answering.lock().unwrap() = true;
        e.clock.set(at_nine());
        e.engine.tick(&e.automations.handler()).await.unwrap();
        let waiting = e.engine.store().active_run(&id).unwrap().unwrap();
        assert_eq!(waiting.wait_reason, Some(WaitReason::Yield));
        assert_eq!(waiting.attempt, 0);
        assert_eq!(e.env.requests(), 0, "no model call while the user chats");
        assert_eq!(
            e.automations.get(&id).unwrap().state,
            AutomationState::Waiting
        );
        *e.env.answering.lock().unwrap() = false;
        e.env.script(answered(1, "Done."));
        e.clock
            .set(at_nine() + TimeDelta::seconds(YIELD_RECHECK_SECS));
        e.engine.tick(&e.automations.handler()).await.unwrap();
        assert_eq!(e.env.requests(), 1);
        let runs = e.engine.store().runs(&id, 5).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Succeeded);
    }

    #[tokio::test]
    async fn a_run_on_another_limit_does_not_wait_for_chat() {
        let e = env();
        let id = scheduled(&e).await;
        *e.env.answering.lock().unwrap() = true;
        *e.env.model.lock().unwrap() = Some(Ok(RunModel {
            model: Some(ModelRef::new(
                shodh_rag::harness::model_catalog::ProviderId::LmStudio,
                "qwen3-8b",
            )),
            pool: "lmstudio".into(),
            shares_chat: false,
        }));
        e.env.script(answered(1, "Done."));
        e.clock.set(at_nine());
        e.engine.tick(&e.automations.handler()).await.unwrap();
        assert_eq!(e.env.requests(), 1);
        assert_eq!(
            e.engine.store().runs(&id, 5).unwrap()[0].state,
            RunState::Succeeded
        );
    }

    #[tokio::test]
    async fn after_a_rate_limit_automations_wait_for_the_reset_without_failing() {
        let e = env();
        let id = scheduled(&e).await;
        let reset = at_nine() + TimeDelta::minutes(40);
        e.env.script(AgentOutcome {
            calls: 0,
            failure: Some(AgentFailure {
                kind: FailureKind::RateLimited,
                detail: "429".into(),
                resets_at: Some(reset),
            }),
            ..AgentOutcome::default()
        });
        e.clock.set(at_nine());
        e.engine.tick(&e.automations.handler()).await.unwrap();
        let waiting = e.engine.store().active_run(&id).unwrap().unwrap();
        assert_eq!(waiting.wait_reason, Some(WaitReason::Quota));
        assert_eq!(waiting.attempt, 0, "a limit is not a failure");
        assert_eq!(
            waiting.next_attempt_at.as_deref().and_then(parse_instant),
            Some(reset)
        );
        assert_eq!(
            e.env.cooldown(FREE_POOL),
            Some(reset),
            "every automation on the limit waits"
        );
        // Before the reset, a run started from a tick does not call the model again.
        e.clock.set(reset - TimeDelta::minutes(1));
        e.engine.tick(&e.automations.handler()).await.unwrap();
        assert_eq!(e.env.requests(), 1);
        e.env.script(answered(1, "Done."));
        e.clock.set(reset);
        e.engine.tick(&e.automations.handler()).await.unwrap();
        assert_eq!(e.env.requests(), 2);
        assert_eq!(
            e.engine.store().runs(&id, 5).unwrap()[0].state,
            RunState::Succeeded
        );
        assert_eq!(
            e.engine.store().job(&id).unwrap().unwrap().failure_streak,
            0
        );
    }

    #[tokio::test]
    async fn at_the_chat_reserve_automations_pause_until_the_next_day_with_one_item() {
        let e = env();
        let id = scheduled(&e).await;
        // 50 a day; chat's busiest day was 9 calls; 38 used: 12 left, and 4 + 9 > 12.
        *e.env.pool.lock().unwrap() = Some(PoolToday {
            used: 38,
            busiest_chat: 9,
        });
        e.clock.set(at_nine());
        e.engine.tick(&e.automations.handler()).await.unwrap();
        let waiting = e.engine.store().active_run(&id).unwrap().unwrap();
        assert_eq!(waiting.wait_reason, Some(WaitReason::Quota));
        assert_eq!(
            waiting.next_attempt_at.as_deref().and_then(parse_instant),
            Some(next_utc_midnight(at_nine()))
        );
        assert_eq!(e.env.requests(), 0);
        assert_eq!(
            e.automations.get(&id).unwrap().state,
            AutomationState::Paused
        );
        assert_eq!(e.env.posted_ids(), vec![YIELD_ITEM.to_string()]);
        // 37 used: 13 left, exactly enough for the run and the reserve.
        *e.env.pool.lock().unwrap() = Some(PoolToday {
            used: 37,
            busiest_chat: 9,
        });
        e.env.script(answered(1, "Done."));
        e.clock.set(next_utc_midnight(at_nine()));
        e.engine.tick(&e.automations.handler()).await.unwrap();
        assert_eq!(e.env.requests(), 1);
    }

    #[tokio::test]
    async fn a_run_the_user_starts_skips_the_chat_rules_but_not_a_reset() {
        let e = env();
        let auto = e
            .automations
            .create(draft(None), CreatedBy::User, json!({}))
            .unwrap();
        *e.env.answering.lock().unwrap() = true;
        *e.env.pool.lock().unwrap() = Some(PoolToday {
            used: 49,
            busiest_chat: 9,
        });
        e.env.script(answered(1, "Done."));
        let run = run_now(&e, &auto.id).await;
        assert_eq!(run.state, RunState::Succeeded);
        e.env.hold_pool(FREE_POOL, start() + TimeDelta::minutes(5));
        let held = run_now(&e, &auto.id).await;
        assert_eq!(held.wait_reason, Some(WaitReason::Quota));
        assert_eq!(e.env.requests(), 1);
    }

    #[tokio::test]
    async fn a_missed_time_runs_once_two_minutes_after_the_wake_and_counts_the_rest() {
        let e = env();
        let id = scheduled(&e).await;
        let woke = at_nine() + TimeDelta::days(2) + TimeDelta::hours(3);
        e.clock.set(woke);
        e.engine.tick(&e.automations.handler()).await.unwrap();
        let waiting = e.engine.store().active_run(&id).unwrap().unwrap();
        assert_eq!(waiting.trigger, Trigger::CatchUp);
        assert_eq!(waiting.missed, 2);
        assert_eq!(e.env.requests(), 0, "it waits a moment first");
        e.env.script(answered(1, "Done."));
        e.clock.set(woke + TimeDelta::minutes(AFTER_WAKE_MINUTES));
        e.engine.tick(&e.automations.handler()).await.unwrap();
        assert_eq!(e.env.requests(), 1);
        let runs = e.engine.store().runs(&id, 5).unwrap();
        assert_eq!(runs.len(), 1, "one run for the missed times");
        assert_eq!(runs[0].state, RunState::Succeeded);
    }

    #[tokio::test]
    async fn no_model_holds_after_three_runs_and_says_check_the_model() {
        let e = env();
        let id = scheduled(&e).await;
        *e.env.model.lock().unwrap() = Some(Err(ModelBlock::NoModel));
        // Three attempts make a failed run; three failed runs hold the automation.
        for day in 0..3 {
            let base = at_nine() + TimeDelta::days(day);
            for attempt in 0..3 {
                e.clock
                    .set(base + TimeDelta::minutes(MODEL_RETRY_MINUTES * attempt));
                e.engine.tick(&e.automations.handler()).await.unwrap();
            }
        }
        let job = e.engine.store().job(&id).unwrap().unwrap();
        assert_eq!(job.hold_reason, Some(HoldReason::ModelUnavailable));
        assert_eq!(
            e.automations.get(&id).unwrap().state,
            AutomationState::OnHold
        );
        assert!(e.env.posted_ids().contains(&hold_item(&id)));
        assert_eq!(e.env.requests(), 0, "never another model in its place");
        let resumed = e.automations.set_enabled(&id, true).await.unwrap();
        assert_eq!(resumed.state, AutomationState::Scheduled);
        assert!(e.env.resolved.lock().unwrap().contains(&hold_item(&id)));
    }

    #[tokio::test]
    async fn local_only_with_a_remote_model_skips_and_never_holds() {
        let e = env();
        let auto = e
            .automations
            .create(draft(None), CreatedBy::User, json!({}))
            .unwrap();
        *e.env.model.lock().unwrap() = Some(Err(ModelBlock::LocalOnly));
        let run = run_now(&e, &auto.id).await;
        assert_eq!(run.state, RunState::Skipped);
        assert!(run.summary.unwrap().contains("Local-only"));
        assert_eq!(
            e.engine
                .store()
                .job(&auto.id)
                .unwrap()
                .unwrap()
                .failure_streak,
            0
        );
    }

    #[tokio::test]
    async fn a_second_run_now_is_refused_while_one_is_active() {
        let e = env();
        let auto = e
            .automations
            .create(draft(None), CreatedBy::User, json!({}))
            .unwrap();
        e.env.script(AgentOutcome {
            calls: 1,
            writes: vec![write("Add task A")],
            ..AgentOutcome::default()
        });
        run_now(&e, &auto.id).await;
        let again = e.automations.run_now(&auto.id).await;
        assert_eq!(again.unwrap_err(), "It is already running.");
    }

    #[tokio::test]
    async fn drafts_are_checked_and_an_update_raises_the_revision() {
        let e = env();
        assert!(e
            .automations
            .create(
                Draft {
                    name: " ".into(),
                    ..draft(None)
                },
                CreatedBy::User,
                json!({})
            )
            .is_err());
        assert!(e
            .automations
            .create(
                Draft {
                    schedule: Some(Schedule::Weekly {
                        days: vec![],
                        at: NaiveTime::from_hms_opt(9, 0, 0).unwrap()
                    }),
                    ..draft(None)
                },
                CreatedBy::User,
                json!({})
            )
            .is_err());
        let auto = e
            .automations
            .create(
                draft(Some(daily())),
                CreatedBy::Agent,
                json!({ "conversationId": "c1" }),
            )
            .unwrap();
        assert_eq!(auto.when, "Every day at 09:00");
        let changed = e
            .automations
            .update(
                &auto.id,
                Draft {
                    name: "Daily digest".into(),
                    schedule: None,
                    ..draft(None)
                },
            )
            .unwrap();
        assert_eq!(changed.name, "Daily digest");
        assert_eq!(changed.schedule, None, "only when started from now on");
        let job = e.engine.store().job(&auto.id).unwrap().unwrap();
        assert_eq!(Playbook::of(&job).unwrap().revision, 2);
        assert!(e.automations.delete(&auto.id).await.unwrap());
        assert!(e.automations.list().unwrap().is_empty());
    }

    #[test]
    fn the_estimate_reads_per_run_and_per_month() {
        assert_eq!(estimate_line(None), "up to 4 model calls a run");
        assert_eq!(
            estimate_line(Some(&Schedule::Weekly {
                days: vec![shodh_rag::engine::schedule::WeekDay::Mon],
                at: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            })),
            "up to 4 model calls a run (about 16 a month)"
        );
        assert_eq!(
            estimate_line(Some(&daily())),
            "up to 4 model calls a run (about 120 a month)"
        );
    }
}
