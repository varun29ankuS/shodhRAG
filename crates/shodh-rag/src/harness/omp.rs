//! omp frame → [`AgentEvent`] normalisation.
//!
//! [`normalise`] is a pure function over one inbound frame and a mutable
//! [`NormaliserState`]. The session runtime feeds it every stdout frame in
//! order and forwards the resulting events, so there is exactly one emission
//! point for run, step, text and usage events.
//!
//! Ordering facts this relies on (verified against the spike fixture):
//! - `message_update/toolcall_end` (carrying the intent in `arguments.i`)
//!   arrives before `host_tool_call`, whose arguments have the intent stripped.
//! - `host_tool_call` arrives before `tool_execution_start` for the same call.
//! - `tool_execution_end` arrives only after the host wrote `host_tool_result`.
//!
//! With [`NormaliserState::set_hold_completion`] a run that completes is not
//! finished at once: it is *held* so the session can check the answer
//! ([`super::grounding`]) and then either finish it or send one more prompt
//! within the same run (same citation numbers, same tool budget).
//!
//! The answer of a round is its final turn, not every text the model wrote:
//! the text of a message that calls tools ("I'll search for the retainer")
//! is a working note, marked with `TextInterim` at its first tool call and
//! left out of the checked answer. The text was already shown by then, so it
//! stays where it is as a note; nothing streamed is taken back. In a
//! follow-up turn the model replies to the app's check, so each message's
//! opening is held until its preface ("I apologize for the error in my
//! previous response") can be dropped before it is shown
//! ([`super::grounding::preface`]).

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::events::{AgentEvent, RiskTier, RunStatus};
use super::grounding::preface::OpeningGate;
use super::protocol::{
    AssistantMessageEvent, InboundFrame, MessageEndFrame, PromptResultFrame, PromptStatus,
    ResponseFrame, ToolResultPayload,
};
use super::provider_error::classify;
use super::truncate_chars;

/// Display metadata for a registered host tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepMeta {
    /// Label template such as `"Searching {query}"`.
    pub label_template: String,
    pub tier: RiskTier,
}

/// What a host-tool dispatch produced, recorded before `host_tool_result` is
/// written so the later `tool_execution_end` can carry the UI summary.
#[derive(Debug, Clone, PartialEq)]
pub struct StepOutcome {
    pub ok: bool,
    pub summary: String,
    pub detail: Option<Value>,
    /// The host rejected the call's arguments (see `StepFinished`).
    pub invalid_arguments: bool,
}

const MAX_PROGRESS_CHARS: usize = 400;
/// Answer text kept per run for the grounding check.
const MAX_ANSWER_CHARS: usize = 400_000;
const MAX_SUMMARY_CHARS: usize = 160;
const MAX_INTENTS_REMEMBERED: usize = 256;

#[derive(Debug, Clone)]
struct OpenStep {
    started_at_ms: u64,
    outcome: Option<StepOutcome>,
}

#[derive(Debug, Clone, Default)]
struct UsageTotals {
    input: u64,
    output: u64,
    cache_read: u64,
    cost: f64,
}

#[derive(Debug, Clone)]
struct ActiveRun {
    run_id: String,
    started_at_ms: u64,
    /// Prompt command ids whose completion is still outstanding.
    pending_prompts: HashSet<String>,
    usage: UsageTotals,
    worst_status: RunStatus,
    error: Option<String>,
    /// The runtime's hint that the failure is transient.
    retryable: Option<bool>,
    /// Provider error of the latest assistant turn (`stopReason: "error"`); a
    /// later successful turn clears it.
    turn_error: Option<String>,
    /// Text by assistant message, in order, as it was shown.
    messages: Vec<(String, String)>,
    answer_chars: usize,
    /// Messages that call tools: their text is the model's working note
    /// ("I'll search the contracts"), not the answer (see `TextInterim`).
    interim: HashSet<String>,
    /// Index into `messages` where the current round of answering starts
    /// (the first answer, or a follow-up turn of the answer check).
    round_start: usize,
    /// The current round is a follow-up turn of the answer check: the
    /// opening of each of its messages is held until its preface to the app
    /// is dropped ([`OpeningGate`]).
    follow_up: bool,
    gates: HashMap<String, OpeningGate>,
    /// Completed and waiting for the session to finish or continue it.
    held: bool,
    /// A hold the session has not yet picked up.
    settle_pending: bool,
    /// Changes whenever a hold starts or ends, so a check that finishes
    /// after the run moved on is discarded.
    generation: u64,
}

/// A held run, as handed to the session's check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldRun {
    pub run_id: String,
    pub generation: u64,
    /// Answer text by assistant message, in order: every message of the run
    /// but the working notes before tool calls.
    pub messages: Vec<(String, String)>,
}

/// State carried across frames of one omp session.
#[derive(Debug, Clone, Default)]
pub struct NormaliserState {
    catalog: HashMap<String, StepMeta>,
    run: Option<ActiveRun>,
    /// toolCallId → intent captured from the model's tool call.
    intents: HashMap<String, String>,
    /// toolCallId → open step.
    steps: HashMap<String, OpenStep>,
    /// host_tool_call id → toolCallId.
    host_calls: HashMap<String, String>,
    /// Attached to every `RunStarted`.
    model_warning: Option<String>,
    /// Hold completed runs for the session's answer check.
    hold_completion: bool,
}

impl NormaliserState {
    pub fn new(catalog: HashMap<String, StepMeta>) -> Self {
        Self {
            catalog,
            ..Self::default()
        }
    }

    /// Set the warning carried by every `RunStarted` (see `OmpModel::warning`).
    pub fn set_model_warning(&mut self, warning: Option<String>) {
        self.model_warning = warning;
    }

    /// Hold completed runs instead of finishing them (see the module docs).
    pub fn set_hold_completion(&mut self, hold: bool) {
        self.hold_completion = hold;
    }

    /// Whether the active run is held.
    pub fn is_held(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.held)
    }

    /// The held run, once: the session's check picks it up here.
    pub fn take_settled(&mut self) -> Option<HeldRun> {
        let run = self.run.as_mut()?;
        if !(run.held && run.settle_pending) {
            return None;
        }
        run.settle_pending = false;
        Some(HeldRun {
            run_id: run.run_id.clone(),
            generation: run.generation,
            messages: run
                .messages
                .iter()
                .filter(|(id, _)| !run.interim.contains(id))
                .cloned()
                .collect(),
        })
    }

    fn held_matches(&self, run_id: &str, generation: u64) -> bool {
        self.run
            .as_ref()
            .is_some_and(|r| r.held && r.run_id == run_id && r.generation == generation)
    }

    /// Whether `generation` of `run_id` is still held (the check may act).
    pub fn still_held(&self, run_id: &str, generation: u64) -> bool {
        self.held_matches(run_id, generation)
    }

    /// Finish a held run (the check is done). Empty when the run moved on.
    pub fn finish_held(&mut self, run_id: &str, generation: u64, now_ms: u64) -> Vec<AgentEvent> {
        if !self.held_matches(run_id, generation) {
            return Vec::new();
        }
        self.finish_run(now_ms)
    }

    /// Continue a held run with one more prompt. False when the run moved on.
    pub fn resume_held(&mut self, run_id: &str, generation: u64, prompt_id: &str) -> bool {
        if !self.held_matches(run_id, generation) {
            return false;
        }
        if let Some(run) = self.run.as_mut() {
            run.held = false;
            run.settle_pending = false;
            run.generation += 1;
            run.pending_prompts.insert(prompt_id.to_string());
            run.round_start = run.messages.len();
            run.follow_up = true;
            run.gates.clear();
        }
        true
    }

    /// End a held run as interrupted (the user stopped it during the check).
    pub fn abort_held(&mut self, now_ms: u64) -> Vec<AgentEvent> {
        match self.run.as_mut() {
            Some(run) if run.held => {
                run.worst_status = RunStatus::Aborted;
                self.finish_run(now_ms)
            }
            _ => Vec::new(),
        }
    }

    /// Start a run for a freshly sent prompt and return its `RunStarted`.
    pub fn begin_run(
        &mut self,
        run_id: &str,
        session_id: &str,
        model: &str,
        prompt_id: &str,
        now_ms: u64,
    ) -> AgentEvent {
        let mut pending_prompts = HashSet::new();
        pending_prompts.insert(prompt_id.to_string());
        self.run = Some(ActiveRun {
            run_id: run_id.to_string(),
            started_at_ms: now_ms,
            pending_prompts,
            usage: UsageTotals::default(),
            worst_status: RunStatus::Completed,
            error: None,
            retryable: None,
            turn_error: None,
            messages: Vec::new(),
            answer_chars: 0,
            interim: HashSet::new(),
            round_start: 0,
            follow_up: false,
            gates: HashMap::new(),
            held: false,
            settle_pending: false,
            generation: 0,
        });
        self.steps.clear();
        self.host_calls.clear();
        AgentEvent::RunStarted {
            run_id: run_id.to_string(),
            session_id: session_id.to_string(),
            model: model.to_string(),
            at_ms: now_ms,
            warning: self.model_warning.clone(),
        }
    }

    /// Attach a steering prompt to the active run. Its own `prompt_result`
    /// is then absorbed instead of producing a separate run. A held run
    /// continues (its pending check is discarded).
    pub fn attach_prompt(&mut self, prompt_id: &str) -> bool {
        match self.run.as_mut() {
            Some(run) => {
                if run.held {
                    run.held = false;
                    run.settle_pending = false;
                    run.generation += 1;
                }
                run.pending_prompts.insert(prompt_id.to_string());
                true
            }
            None => false,
        }
    }

    pub fn active_run_id(&self) -> Option<&str> {
        self.run.as_ref().map(|r| r.run_id.as_str())
    }

    pub fn tier_of(&self, tool: &str) -> Option<RiskTier> {
        self.catalog.get(tool).map(|m| m.tier)
    }

    /// Record the dispatch outcome for a step (called before the result is
    /// written to omp).
    pub fn record_outcome(&mut self, step_id: &str, outcome: StepOutcome) {
        if let Some(step) = self.steps.get_mut(step_id) {
            step.outcome = Some(outcome);
        }
    }

    /// The step a `host_tool_call` id belongs to.
    pub fn step_for_host_call(&self, host_call_id: &str) -> Option<&str> {
        self.host_calls.get(host_call_id).map(String::as_str)
    }

    /// Open the step of a tool call before omp reports it (a Code session's
    /// guard asks for approval before `tool_execution_start`). `None` when no
    /// run is active or the step is already open.
    pub fn open_step(
        &mut self,
        tool_call_id: &str,
        tool: &str,
        args: &Value,
        now_ms: u64,
    ) -> Option<AgentEvent> {
        let run_id = self.active_run_id()?.to_string();
        if self.steps.contains_key(tool_call_id) {
            return None;
        }
        Some(self.start_step(run_id, tool_call_id, tool, args, None, now_ms))
    }

    /// End the active run with an error (e.g. the sidecar exited).
    pub fn fail_run(&mut self, error: &str, now_ms: u64) -> Vec<AgentEvent> {
        match self.run.as_mut() {
            Some(run) => {
                run.worst_status = RunStatus::Error;
                run.error = Some(error.to_string());
                let mut events = self.end_round();
                events.extend(self.finish_run(now_ms));
                events
            }
            None => Vec::new(),
        }
    }

    fn finish_run(&mut self, now_ms: u64) -> Vec<AgentEvent> {
        let Some(run) = self.run.take() else {
            return Vec::new();
        };
        // A run whose last model turn failed did not produce an answer, even
        // when the runtime reports the prompt itself as completed.
        let (status, error) = match (run.worst_status, run.error, run.turn_error) {
            (RunStatus::Completed, None, Some(turn_error)) => (RunStatus::Error, Some(turn_error)),
            (status, error, _) => (status, error),
        };
        let provider_error = match (status, error.as_deref()) {
            (RunStatus::Error, Some(message)) => Some(classify(message, run.retryable, now_ms)),
            _ => None,
        };
        let interrupted_summary = match status {
            RunStatus::Aborted => "Interrupted",
            RunStatus::Error | RunStatus::Completed => "Did not complete",
        };
        let mut events: Vec<AgentEvent> = self
            .steps
            .drain()
            .map(|(step_id, step)| {
                let outcome = step.outcome.unwrap_or(StepOutcome {
                    ok: false,
                    summary: interrupted_summary.to_string(),
                    detail: None,
                    invalid_arguments: false,
                });
                AgentEvent::StepFinished {
                    run_id: run.run_id.clone(),
                    step_id,
                    ok: outcome.ok,
                    summary: outcome.summary,
                    detail: outcome.detail,
                    duration_ms: now_ms.saturating_sub(step.started_at_ms),
                    invalid_arguments: outcome.invalid_arguments,
                }
            })
            .collect();
        self.host_calls.clear();
        events.push(AgentEvent::RunFinished {
            run_id: run.run_id,
            status,
            duration_ms: now_ms.saturating_sub(run.started_at_ms),
            error,
            provider_error,
        });
        events
    }

    fn remember_intent(&mut self, tool_call_id: &str, intent: String) {
        if self.intents.len() >= MAX_INTENTS_REMEMBERED {
            self.intents.clear();
        }
        self.intents.insert(tool_call_id.to_string(), intent);
    }

    fn label_for(
        &self,
        tool_call_id: &str,
        tool: &str,
        args: &Value,
        intent: Option<&str>,
    ) -> String {
        if let Some(intent) = intent
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| self.intents.get(tool_call_id).map(String::as_str))
        {
            return intent.to_string();
        }
        match self.catalog.get(tool) {
            Some(meta) => render_label(&meta.label_template, args),
            None => tool.replace('_', " "),
        }
    }

    fn start_step(
        &mut self,
        run_id: String,
        tool_call_id: &str,
        tool: &str,
        args: &Value,
        intent: Option<&str>,
        now_ms: u64,
    ) -> AgentEvent {
        let label = self.label_for(tool_call_id, tool, args, intent);
        // Unknown tools are shown at the most restrictive tier.
        let tier = self.tier_of(tool).unwrap_or(RiskTier::Destructive);
        self.steps.insert(
            tool_call_id.to_string(),
            OpenStep {
                started_at_ms: now_ms,
                outcome: None,
            },
        );
        AgentEvent::StepStarted {
            run_id,
            step_id: tool_call_id.to_string(),
            parent_step_id: None,
            tool: tool.to_string(),
            label,
            args: strip_intent(args),
            tier,
            at_ms: now_ms,
        }
    }

    fn on_prompt_completion(
        &mut self,
        prompt_id: Option<&str>,
        status: RunStatus,
        error: Option<String>,
        now_ms: u64,
    ) -> Vec<AgentEvent> {
        let Some(run) = self.run.as_mut() else {
            return Vec::new();
        };
        let Some(prompt_id) = prompt_id else {
            return Vec::new();
        };
        if !run.pending_prompts.remove(prompt_id) {
            return Vec::new();
        }
        run.worst_status = worse(run.worst_status, status);
        if error.is_some() {
            run.error = error;
        }
        if !run.pending_prompts.is_empty() {
            return Vec::new();
        }
        let mut events = self.end_round();
        let Some(run) = self.run.as_mut() else {
            return events;
        };
        if self.hold_completion && run.worst_status == RunStatus::Completed {
            run.held = true;
            run.settle_pending = true;
            run.generation += 1;
            return events;
        }
        events.extend(self.finish_run(now_ms));
        events
    }

    /// The round of answering ended: held openings are shown, and when every
    /// text of the round is a working note, the last one is the answer after
    /// all (a model that wrote its answer and then updated its task list).
    fn end_round(&mut self) -> Vec<AgentEvent> {
        let mut events = self.flush_gates();
        let Some(run) = self.run.as_mut() else {
            return events;
        };
        let round = run.messages.get(run.round_start..).unwrap_or(&[]);
        let answered = round
            .iter()
            .any(|(id, text)| !run.interim.contains(id) && !text.trim().is_empty());
        if answered {
            return events;
        }
        let last_note = round
            .iter()
            .rev()
            .find(|(_, text)| !text.trim().is_empty())
            .map(|(id, _)| id.clone());
        if let Some(id) = last_note {
            run.interim.remove(&id);
            events.push(AgentEvent::TextInterim {
                run_id: run.run_id.clone(),
                message_id: id,
                interim: false,
            });
        }
        events
    }

    /// Streamed text of `message_id`. In a follow-up turn it passes the
    /// message's opening gate first (preface to the app); so does a message
    /// begun after a tool call of the run (narration opening the answer, see
    /// [`super::grounding::preface`]). A message that started streaming
    /// before is never gated midway.
    fn on_text(&mut self, run_id: String, message_id: &str, delta: &str) -> Vec<AgentEvent> {
        let shown = match self.run.as_mut() {
            Some(run) if run.follow_up => run
                .gates
                .entry(message_id.to_string())
                .or_insert_with(OpeningGate::preface)
                .push(delta),
            Some(run)
                if run.gates.contains_key(message_id)
                    || (!run.interim.is_empty()
                        && !run.messages.iter().any(|(id, _)| id == message_id)) =>
            {
                run.gates
                    .entry(message_id.to_string())
                    .or_insert_with(OpeningGate::narration)
                    .push(delta)
            }
            Some(_) => delta.to_string(),
            None => return Vec::new(),
        };
        self.show_text(run_id, message_id, shown)
    }

    /// Records and emits text exactly as the transcript shows it.
    fn show_text(&mut self, run_id: String, message_id: &str, text: String) -> Vec<AgentEvent> {
        if text.is_empty() {
            return Vec::new();
        }
        self.record_text(message_id, &text);
        vec![AgentEvent::TextDelta {
            run_id,
            message_id: message_id.to_string(),
            delta: text,
        }]
    }

    /// The text of `message_id` ended: what its opening gate holds is shown.
    fn flush_gate(&mut self, message_id: &str) -> Vec<AgentEvent> {
        let Some(run) = self.run.as_mut() else {
            return Vec::new();
        };
        let Some(gate) = run.gates.get_mut(message_id) else {
            return Vec::new();
        };
        let text = gate.finish();
        let run_id = run.run_id.clone();
        self.show_text(run_id, message_id, text)
    }

    fn flush_gates(&mut self) -> Vec<AgentEvent> {
        let mut ids: Vec<String> = self
            .run
            .as_ref()
            .map(|run| run.gates.keys().cloned().collect())
            .unwrap_or_default();
        ids.sort();
        let mut events = Vec::new();
        for id in ids {
            events.extend(self.flush_gate(&id));
        }
        events
    }

    /// `message_id` calls tools: its text is a working note, not answer.
    fn mark_interim(&mut self, message_id: &str) -> Vec<AgentEvent> {
        let Some(run) = self.run.as_mut() else {
            return Vec::new();
        };
        if !run.interim.insert(message_id.to_string()) {
            return Vec::new();
        }
        vec![AgentEvent::TextInterim {
            run_id: run.run_id.clone(),
            message_id: message_id.to_string(),
            interim: true,
        }]
    }

    fn record_text(&mut self, message_id: &str, delta: &str) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        if run.answer_chars + delta.len() > MAX_ANSWER_CHARS {
            return;
        }
        run.answer_chars += delta.len();
        match run.messages.last_mut() {
            Some((id, text)) if id == message_id => text.push_str(delta),
            _ => match run.messages.iter_mut().find(|(id, _)| id == message_id) {
                Some((_, text)) => text.push_str(delta),
                None => run
                    .messages
                    .push((message_id.to_string(), delta.to_string())),
            },
        }
    }

    fn on_response(&mut self, response: &ResponseFrame, now_ms: u64) -> Vec<AgentEvent> {
        if response.command != "prompt" {
            return Vec::new();
        }
        if !response.success {
            let error = response
                .error
                .clone()
                .unwrap_or_else(|| "omp rejected the prompt".to_string());
            return self.on_prompt_completion(
                response.id.as_deref(),
                RunStatus::Error,
                Some(error),
                now_ms,
            );
        }
        if response.completed_locally() {
            return self.on_prompt_completion(
                response.id.as_deref(),
                RunStatus::Completed,
                None,
                now_ms,
            );
        }
        Vec::new()
    }

    fn on_prompt_result(&mut self, result: &PromptResultFrame, now_ms: u64) -> Vec<AgentEvent> {
        let (status, error) = match result.status {
            PromptStatus::Completed => (RunStatus::Completed, None),
            PromptStatus::Aborted => (RunStatus::Aborted, None),
            PromptStatus::Error | PromptStatus::Unknown => (
                RunStatus::Error,
                Some(
                    result
                        .error
                        .as_ref()
                        .map(|e| e.message.clone())
                        .unwrap_or_else(|| "The model provider returned an error".to_string()),
                ),
            ),
        };
        if let (Some(run), Some(e)) = (self.run.as_mut(), result.error.as_ref()) {
            if e.retryable.is_some() {
                run.retryable = e.retryable;
            }
        }
        self.on_prompt_completion(result.id.as_deref(), status, error, now_ms)
    }

    fn on_message_end(&mut self, frame: &MessageEndFrame, run_id: String) -> Vec<AgentEvent> {
        if frame.message.role != "assistant" {
            return Vec::new();
        }
        // The message's text ended; a message that calls tools is a working
        // note (normally marked at its first `toolcall_end` already).
        let calls = frame.message.tool_calls();
        let mut events = match &frame.message_id {
            Some(id) => {
                let mut events = self.flush_gate(id);
                if !calls.is_empty() {
                    events.extend(self.mark_interim(id));
                }
                events
            }
            None => self.flush_gates(),
        };
        for call in calls {
            if let Some(intent) = call.intent() {
                self.remember_intent(&call.id, intent);
            }
        }
        if let Some(run) = self.run.as_mut() {
            run.turn_error = (frame.message.stop_reason.as_deref() == Some("error")).then(|| {
                frame
                    .message
                    .error_message
                    .clone()
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or_else(|| "The model provider returned an error".to_string())
            });
        }
        let Some(usage) = frame.message.usage.as_ref() else {
            return events;
        };
        let cost = usage.cost.as_ref().map(|c| c.total).unwrap_or(0.0);
        if usage.input == 0 && usage.output == 0 && usage.cache_read == 0 && cost == 0.0 {
            return events;
        }
        let Some(run) = self.run.as_mut() else {
            return events;
        };
        run.usage.input += usage.input;
        run.usage.output += usage.output;
        run.usage.cache_read += usage.cache_read;
        run.usage.cost += cost;
        events.push(AgentEvent::Usage {
            run_id,
            input_tokens: run.usage.input,
            output_tokens: run.usage.output,
            cache_read_tokens: run.usage.cache_read,
            cost_usd: run.usage.cost,
        });
        events
    }
}

fn worse(a: RunStatus, b: RunStatus) -> RunStatus {
    fn rank(s: RunStatus) -> u8 {
        match s {
            RunStatus::Completed => 0,
            RunStatus::Aborted => 1,
            RunStatus::Error => 2,
        }
    }
    if rank(b) > rank(a) {
        b
    } else {
        a
    }
}

/// Remove omp's `i` (intent) argument from displayed arguments.
fn strip_intent(args: &Value) -> Value {
    match args {
        Value::Object(map) if map.contains_key("i") => {
            let mut map = map.clone();
            map.remove("i");
            Value::Object(map)
        }
        other => other.clone(),
    }
}

/// Fill `{name}` placeholders from string or number arguments. Placeholders
/// without a value are dropped. A `[...]` group is kept only when every
/// placeholder inside it has a value, so optional phrases disappear whole:
/// `"Listing tasks[ due by {due_to}]"`. Values are quoted unless the
/// placeholder is written `{name!}`.
pub fn render_label(template: &str, args: &Value) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('[') {
        out.push_str(&fill_placeholders(&rest[..open], args).0);
        let after = &rest[open + 1..];
        match after.find(']') {
            Some(close) => {
                let (group, complete) = fill_placeholders(&after[..close], args);
                if complete {
                    out.push_str(&group);
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&fill_placeholders(&rest[open..], args).0);
                rest = "";
            }
        }
    }
    out.push_str(&fill_placeholders(rest, args).0);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Fill the placeholders of `segment`; the flag is false when any was empty.
fn fill_placeholders(segment: &str, args: &Value) -> (String, bool) {
    let mut out = String::with_capacity(segment.len());
    let mut complete = true;
    let mut rest = segment;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let raw_key = &after[..close];
                let (key, quoted) = match raw_key.strip_suffix('!') {
                    Some(k) => (k, false),
                    None => (raw_key, true),
                };
                let value = match args.get(key) {
                    Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
                    Some(Value::Number(n)) => Some(n.to_string()),
                    _ => None,
                };
                match value {
                    Some(value) => {
                        let value = truncate_chars(value.trim(), 60);
                        if quoted {
                            out.push('“');
                            out.push_str(&value);
                            out.push('”');
                        } else {
                            out.push_str(&value);
                        }
                    }
                    None => complete = false,
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    (out, complete)
}

/// How omp's tool-argument validation answers when a call's arguments do
/// not fit the tool's schema (and its coercions cannot fix them).
const RUNTIME_VALIDATION_PREFIX: &str = "Validation failed for tool";
/// Transcript summary of a call refused for its arguments (the host's
/// `ToolError::InvalidArguments` summary).
pub const INVALID_ARGUMENTS_SUMMARY: &str = "Invalid arguments";

/// The runtime's validation message, when `payload` is one.
fn runtime_validation_error(payload: Option<&ToolResultPayload>) -> Option<String> {
    let text = payload.map(ToolResultPayload::joined_text)?;
    let text = text.trim();
    text.starts_with(RUNTIME_VALIDATION_PREFIX)
        .then(|| truncate_chars(text, MAX_PROGRESS_CHARS))
}

fn first_line_summary(payload: Option<&ToolResultPayload>, ok: bool) -> String {
    let text = payload
        .map(ToolResultPayload::joined_text)
        .unwrap_or_default();
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    match line {
        Some(line) => truncate_chars(line, MAX_SUMMARY_CHARS),
        None if ok => "Done".to_string(),
        None => "Failed".to_string(),
    }
}

/// Convert one omp frame into zero or more agent events.
pub fn normalise(
    frame: &InboundFrame,
    state: &mut NormaliserState,
    now_ms: u64,
) -> Vec<AgentEvent> {
    match frame {
        InboundFrame::Response(response) => state.on_response(response, now_ms),
        InboundFrame::PromptResult(result) => state.on_prompt_result(result, now_ms),
        InboundFrame::MessageUpdate(update) => match &update.assistant_message_event {
            AssistantMessageEvent::ToolcallEnd { tool_call } => {
                if let Some(intent) = tool_call.intent() {
                    state.remember_intent(&tool_call.id, intent);
                }
                // The text before the call led into it: a working note.
                let mut events = state.flush_gate(&update.message_id);
                events.extend(state.mark_interim(&update.message_id));
                events
            }
            AssistantMessageEvent::TextDelta { delta } => {
                match state.active_run_id().map(str::to_string) {
                    Some(run_id) if !delta.is_empty() => {
                        state.on_text(run_id, &update.message_id, delta)
                    }
                    _ => Vec::new(),
                }
            }
            AssistantMessageEvent::ThinkingDelta { delta } => match state.active_run_id() {
                Some(run_id) if !delta.is_empty() => vec![AgentEvent::Thinking {
                    run_id: run_id.to_string(),
                    message_id: update.message_id.clone(),
                    delta: delta.clone(),
                }],
                _ => Vec::new(),
            },
            AssistantMessageEvent::Other => Vec::new(),
        },
        InboundFrame::MessageEnd(end) => match state.active_run_id().map(str::to_string) {
            Some(run_id) => state.on_message_end(end, run_id),
            None => Vec::new(),
        },
        InboundFrame::HostToolCall(call) => {
            let Some(run_id) = state.active_run_id().map(str::to_string) else {
                return Vec::new();
            };
            state
                .host_calls
                .insert(call.id.clone(), call.tool_call_id.clone());
            if state.steps.contains_key(&call.tool_call_id) {
                return Vec::new();
            }
            vec![state.start_step(
                run_id,
                &call.tool_call_id,
                &call.tool_name,
                &call.arguments,
                None,
                now_ms,
            )]
        }
        InboundFrame::ToolExecutionStart(start) => {
            let Some(run_id) = state.active_run_id().map(str::to_string) else {
                return Vec::new();
            };
            if state.steps.contains_key(&start.tool_call_id) {
                return Vec::new();
            }
            vec![state.start_step(
                run_id,
                &start.tool_call_id,
                &start.tool_name,
                &start.args,
                start.intent.as_deref(),
                now_ms,
            )]
        }
        InboundFrame::ToolExecutionUpdate(update) => {
            let Some(run_id) = state.active_run_id() else {
                return Vec::new();
            };
            if !state.steps.contains_key(&update.tool_call_id) {
                return Vec::new();
            }
            let text = update
                .partial_result
                .as_ref()
                .map(ToolResultPayload::joined_text)
                .unwrap_or_default();
            let text = text.trim();
            if text.is_empty() {
                return Vec::new();
            }
            vec![AgentEvent::StepProgress {
                run_id: run_id.to_string(),
                step_id: update.tool_call_id.clone(),
                text: truncate_chars(text, MAX_PROGRESS_CHARS),
            }]
        }
        InboundFrame::ToolExecutionEnd(end) => {
            let Some(run_id) = state.active_run_id().map(str::to_string) else {
                return Vec::new();
            };
            let Some(step) = state.steps.remove(&end.tool_call_id) else {
                return Vec::new();
            };
            state
                .host_calls
                .retain(|_, step_id| step_id != &end.tool_call_id);
            let ok = !end.is_error;
            let (summary, detail, invalid_arguments) = match step.outcome {
                Some(outcome) => (outcome.summary, outcome.detail, outcome.invalid_arguments),
                None => match runtime_validation_error(end.result.as_ref()).filter(|_| !ok) {
                    // The runtime refused the call before the host saw it.
                    Some(message) => (
                        INVALID_ARGUMENTS_SUMMARY.to_string(),
                        Some(serde_json::json!({ "error": message })),
                        true,
                    ),
                    None => (first_line_summary(end.result.as_ref(), ok), None, false),
                },
            };
            vec![AgentEvent::StepFinished {
                run_id,
                step_id: end.tool_call_id.clone(),
                ok,
                summary,
                detail,
                duration_ms: now_ms.saturating_sub(step.started_at_ms),
                invalid_arguments,
            }]
        }
        InboundFrame::Ready(_)
        | InboundFrame::AgentStart
        | InboundFrame::AgentEnd(_)
        | InboundFrame::TurnEnd
        | InboundFrame::SessionSettled
        | InboundFrame::HostToolCancel(_)
        | InboundFrame::ExtensionUiRequest(_)
        | InboundFrame::AvailableCommands(_)
        | InboundFrame::Other { .. }
        | InboundFrame::Malformed { .. } => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::protocol::parse_frame;
    use serde_json::json;

    const FIXTURE: &str = include_str!("fixtures/omp-spike-events.jsonl");

    fn catalog() -> HashMap<String, StepMeta> {
        let mut c = HashMap::new();
        c.insert(
            "search_documents".to_string(),
            StepMeta {
                label_template: "Searching {query}".to_string(),
                tier: RiskTier::Read,
            },
        );
        c
    }

    /// Replay the fixture the way the session runtime does: a run begins when
    /// a prompt is acknowledged (the runtime begins it when sending).
    fn replay() -> Vec<AgentEvent> {
        let mut state = NormaliserState::new(catalog());
        let mut events = Vec::new();
        let mut now = 1_000u64;
        for line in FIXTURE.lines().filter(|l| !l.trim().is_empty()) {
            now += 5;
            let frame = parse_frame(line).unwrap();
            if let InboundFrame::Response(r) = &frame {
                if r.command == "prompt" {
                    let id = r.id.clone().unwrap();
                    events.push(state.begin_run(
                        &format!("run-{id}"),
                        "session-1",
                        "openrouter/test",
                        &id,
                        now,
                    ));
                }
            }
            events.extend(normalise(&frame, &mut state, now));
        }
        events
    }

    fn run_events<'a>(events: &'a [AgentEvent], run: &str) -> Vec<&'a AgentEvent> {
        events.iter().filter(|e| e.run_id() == run).collect()
    }

    #[test]
    fn fixture_run_one_produces_labelled_steps_text_usage_and_completion() {
        let events = replay();
        let run = run_events(&events, "run-p5");

        let labels: Vec<&str> = run
            .iter()
            .filter_map(|e| match e {
                AgentEvent::StepStarted { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                "Finding Acme MSA notice period",
                "Finding notice period termination terms"
            ]
        );
        for event in &run {
            if let AgentEvent::StepStarted { args, tier, .. } = event {
                assert!(args.get("i").is_none());
                assert_eq!(*tier, RiskTier::Read);
            }
        }

        let finished: Vec<bool> = run
            .iter()
            .filter_map(|e| match e {
                AgentEvent::StepFinished { ok, .. } => Some(*ok),
                _ => None,
            })
            .collect();
        assert_eq!(finished, vec![true, true]);

        let text: String = run
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert!(text.contains("60"));

        let last_usage = run
            .iter()
            .rev()
            .find_map(|e| match e {
                AgentEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cost_usd,
                    ..
                } => Some((*input_tokens, *output_tokens, *cache_read_tokens, *cost_usd)),
                _ => None,
            })
            .unwrap();
        assert_eq!(last_usage, (3700, 366, 3240, 0.0));

        match run.last().unwrap() {
            AgentEvent::RunFinished { status, error, .. } => {
                assert_eq!(*status, RunStatus::Completed);
                assert!(error.is_none());
            }
            other => panic!("last event is {other:?}"),
        }
        assert!(matches!(
            run[0],
            AgentEvent::RunStarted { warning: None, .. }
        ));
    }

    #[test]
    fn text_deltas_reconstruct_each_message_exactly() {
        let events = replay();
        let mut by_message: HashMap<String, String> = HashMap::new();
        for event in &events {
            if let AgentEvent::TextDelta {
                message_id, delta, ..
            } = event
            {
                by_message
                    .entry(message_id.clone())
                    .or_default()
                    .push_str(delta);
            }
        }
        let mut checked = 0;
        for line in FIXTURE.lines().filter(|l| !l.trim().is_empty()) {
            let value: Value = serde_json::from_str(line).unwrap();
            if value["type"] == "message_update"
                && value["assistantMessageEvent"]["type"] == "text_end"
            {
                let id = value["messageId"].as_str().unwrap();
                let content = value["assistantMessageEvent"]["content"].as_str().unwrap();
                assert_eq!(
                    by_message.get(id).map(String::as_str),
                    Some(content),
                    "{id}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 4);
        // Sub-agent message updates never leak into the parent transcript.
        assert!(!by_message
            .values()
            .any(|t| t.contains("BLOCKED - requested tools")));
    }

    #[test]
    fn fixture_runs_two_and_three_finish_with_their_status() {
        let events = replay();
        let status_of = |run: &str| {
            events.iter().find_map(|e| match e {
                AgentEvent::RunFinished { run_id, status, .. } if run_id == run => Some(*status),
                _ => None,
            })
        };
        assert_eq!(status_of("run-p6"), Some(RunStatus::Completed));
        assert_eq!(status_of("run-p8"), Some(RunStatus::Aborted));
        // Run two's built-in `task` step comes from tool_execution_start with its intent.
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::StepStarted { run_id, tool, label, tier, .. }
                if run_id == "run-p6" && tool == "task"
                    && label == "Running isolation test with researcher agent"
                    && *tier == RiskTier::Destructive
        )));
        let run_finished = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::RunFinished { .. }))
            .count();
        assert_eq!(run_finished, 3);
    }

    #[test]
    fn model_warning_rides_on_run_started() {
        let mut state = NormaliserState::new(catalog());
        state.set_model_warning(Some("logs prompts".into()));
        match state.begin_run("r", "s", "m", "p", 0) {
            AgentEvent::RunStarted { warning, .. } => {
                assert_eq!(warning.as_deref(), Some("logs prompts"))
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn usage_only_comes_from_assistant_messages() {
        let mut state = NormaliserState::new(catalog());
        state.begin_run("r", "s", "m", "p", 0);
        let user_end = parse_frame(
            r#"{"type":"message_end","messageId":"m1","message":{"role":"user","content":[],"usage":{"input":99,"output":1}}}"#,
        )
        .unwrap();
        assert!(normalise(&user_end, &mut state, 1).is_empty());
    }

    #[test]
    fn recorded_outcome_becomes_the_step_summary() {
        let mut state = NormaliserState::new(catalog());
        state.begin_run("r", "s", "m", "p", 0);
        let call = parse_frame(
            r#"{"type":"host_tool_call","id":"h1","toolCallId":"t1","toolName":"search_documents","arguments":{"query":"acme"}}"#,
        )
        .unwrap();
        let started = normalise(&call, &mut state, 10);
        match &started[0] {
            AgentEvent::StepStarted { label, .. } => assert_eq!(label, "Searching “acme”"),
            other => panic!("{other:?}"),
        }
        assert_eq!(state.step_for_host_call("h1"), Some("t1"));
        state.record_outcome(
            "t1",
            StepOutcome {
                ok: true,
                summary: "4 passages from 2 files".into(),
                detail: Some(json!({"files": 2})),
                invalid_arguments: false,
            },
        );
        let end = parse_frame(
            r#"{"type":"tool_execution_end","toolCallId":"t1","toolName":"search_documents","result":{"content":[{"type":"text","text":"[...]"}]},"isError":false}"#,
        )
        .unwrap();
        let finished = normalise(&end, &mut state, 25);
        assert_eq!(
            finished,
            vec![AgentEvent::StepFinished {
                run_id: "r".into(),
                step_id: "t1".into(),
                ok: true,
                summary: "4 passages from 2 files".into(),
                detail: Some(json!({"files": 2})),
                duration_ms: 15,
                invalid_arguments: false,
            }]
        );
    }

    #[test]
    fn steer_prompts_join_the_active_run() {
        let mut state = NormaliserState::new(catalog());
        state.begin_run("r", "s", "m", "p1", 0);
        assert!(state.attach_prompt("p2"));
        let steer_done = parse_frame(
            r#"{"type":"prompt_result","id":"p2","agentInvoked":true,"status":"completed","sessionSettled":false}"#,
        )
        .unwrap();
        assert!(normalise(&steer_done, &mut state, 5).is_empty());
        let main_done = parse_frame(
            r#"{"type":"prompt_result","id":"p1","agentInvoked":true,"status":"completed","sessionSettled":true}"#,
        )
        .unwrap();
        let events = normalise(&main_done, &mut state, 9);
        assert!(matches!(
            events.last(),
            Some(AgentEvent::RunFinished {
                status: RunStatus::Completed,
                duration_ms: 9,
                ..
            })
        ));
        assert!(state.active_run_id().is_none());
    }

    #[test]
    fn a_call_the_runtime_refused_for_its_arguments_is_marked() {
        let mut state = NormaliserState::new(catalog());
        state.begin_run("r", "s", "m", "p1", 0);
        // omp validates arguments itself: the host never sees this call.
        let start = parse_frame(
            r#"{"type":"tool_execution_start","toolCallId":"t1","toolName":"search_documents","args":{"query":"pruning","sources":null}}"#,
        )
        .unwrap();
        normalise(&start, &mut state, 1);
        let end = parse_frame(
            r#"{"type":"tool_execution_end","toolCallId":"t1","toolName":"search_documents","result":{"content":[{"type":"text","text":"Validation failed for tool \"search_documents\":\n  - sources: must be array\n\nReceived arguments:\n{}"}]},"isError":true}"#,
        )
        .unwrap();
        match normalise(&end, &mut state, 10).as_slice() {
            [AgentEvent::StepFinished {
                ok: false,
                summary,
                detail: Some(detail),
                invalid_arguments: true,
                ..
            }] => {
                assert_eq!(summary, INVALID_ARGUMENTS_SUMMARY);
                assert!(detail["error"]
                    .as_str()
                    .unwrap()
                    .contains("sources: must be array"));
            }
            other => panic!("{other:?}"),
        }
        // Any other failure is not.
        normalise(&start, &mut state, 11);
        let failed = parse_frame(
            r#"{"type":"tool_execution_end","toolCallId":"t1","toolName":"search_documents","result":{"content":[{"type":"text","text":"Search failed: index locked"}]},"isError":true}"#,
        )
        .unwrap();
        assert!(matches!(
            normalise(&failed, &mut state, 12).as_slice(),
            [AgentEvent::StepFinished {
                invalid_arguments: false,
                ..
            }]
        ));
    }

    #[test]
    fn open_steps_close_when_the_run_ends_or_fails() {
        let mut state = NormaliserState::new(catalog());
        state.begin_run("r", "s", "m", "p1", 0);
        let call = parse_frame(
            r#"{"type":"host_tool_call","id":"h1","toolCallId":"t1","toolName":"search_documents","arguments":{"query":"x"}}"#,
        )
        .unwrap();
        normalise(&call, &mut state, 1);
        let events = state.fail_run("omp exited", 7);
        assert!(matches!(
            &events[0],
            AgentEvent::StepFinished { ok: false, step_id, .. } if step_id == "t1"
        ));
        assert!(matches!(
            &events[1],
            AgentEvent::RunFinished { status: RunStatus::Error, error: Some(e), .. } if e == "omp exited"
        ));
    }

    #[test]
    fn prompt_errors_and_local_completion_finish_the_run() {
        let mut state = NormaliserState::new(catalog());
        state.begin_run("r", "s", "m", "p1", 0);
        let local = parse_frame(
            r#"{"id":"p1","type":"response","command":"prompt","success":true,"data":{"agentInvoked":false}}"#,
        )
        .unwrap();
        assert!(matches!(
            normalise(&local, &mut state, 3).last(),
            Some(AgentEvent::RunFinished {
                status: RunStatus::Completed,
                ..
            })
        ));

        state.begin_run("r2", "s", "m", "p2", 10);
        let failed = parse_frame(
            r#"{"type":"prompt_result","id":"p2","agentInvoked":true,"status":"error","error":{"message":"401 Unauthorized","retryable":false}}"#,
        )
        .unwrap();
        assert!(matches!(
            normalise(&failed, &mut state, 12).last(),
            Some(AgentEvent::RunFinished { status: RunStatus::Error, error: Some(e), .. }) if e == "401 Unauthorized"
        ));
    }

    #[test]
    fn provider_errors_are_classified_on_the_finished_run() {
        use crate::harness::provider_error::ProviderErrorKind;
        // A prompt that fails outright, with the runtime's retry hint.
        let mut state = NormaliserState::new(catalog());
        state.begin_run("r", "s", "m", "p1", 0);
        let failed = parse_frame(
            r#"{"type":"prompt_result","id":"p1","agentInvoked":true,"status":"error","error":{"message":"429 {\"error\":{\"message\":\"Rate limit exceeded: free-models-per-min.\",\"code\":429}}","retryable":true}}"#,
        )
        .unwrap();
        match normalise(&failed, &mut state, 5).last() {
            Some(AgentEvent::RunFinished {
                status: RunStatus::Error,
                provider_error: Some(e),
                ..
            }) => assert_eq!(e.kind, ProviderErrorKind::RateLimited),
            other => panic!("unexpected {other:?}"),
        }

        // The model turn fails (stopReason "error") but the prompt reports completed.
        state.begin_run("r2", "s", "m", "p2", 10);
        let turn = parse_frame(
            r#"{"type":"message_end","messageId":"m1","message":{"role":"assistant","content":[],"stopReason":"error","errorMessage":"401 {\"error\":{\"message\":\"No auth credentials found\",\"code\":401}}"}}"#,
        )
        .unwrap();
        assert!(normalise(&turn, &mut state, 11).is_empty());
        match normalise(&completed("p2"), &mut state, 12).last() {
            Some(AgentEvent::RunFinished {
                status: RunStatus::Error,
                error: Some(message),
                provider_error: Some(e),
                ..
            }) => {
                assert!(message.contains("No auth credentials"));
                assert_eq!(e.kind, ProviderErrorKind::Auth);
            }
            other => panic!("unexpected {other:?}"),
        }

        // A failed turn followed by a successful one (the runtime retried) completes.
        state.begin_run("r3", "s", "m", "p3", 20);
        normalise(&turn, &mut state, 21);
        let ok_turn = parse_frame(
            r#"{"type":"message_end","messageId":"m2","message":{"role":"assistant","content":[],"stopReason":"stop"}}"#,
        )
        .unwrap();
        normalise(&ok_turn, &mut state, 22);
        assert!(matches!(
            normalise(&completed("p3"), &mut state, 23).last(),
            Some(AgentEvent::RunFinished {
                status: RunStatus::Completed,
                provider_error: None,
                ..
            })
        ));
    }

    fn completed(prompt: &str) -> InboundFrame {
        parse_frame(&format!(
            r#"{{"type":"prompt_result","id":"{prompt}","agentInvoked":true,"status":"completed","sessionSettled":true}}"#
        ))
        .unwrap()
    }

    fn text(message: &str, delta: &str) -> InboundFrame {
        parse_frame(&format!(
            r#"{{"type":"message_update","messageId":"{message}","assistantMessageEvent":{{"type":"text_delta","delta":"{delta}"}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn held_runs_wait_for_the_check_and_then_finish() {
        let mut state = NormaliserState::new(catalog());
        state.set_hold_completion(true);
        state.begin_run("r", "s", "m", "p1", 0);
        normalise(&text("m1", "Sixty days [1]."), &mut state, 1);
        assert!(
            normalise(&completed("p1"), &mut state, 5).is_empty(),
            "held, not finished"
        );
        assert!(state.is_held());
        let held = state.take_settled().unwrap();
        assert_eq!(
            held.messages,
            vec![("m1".to_string(), "Sixty days [1].".to_string())]
        );
        assert!(state.take_settled().is_none(), "picked up once");
        assert!(
            state.finish_held("r", held.generation + 1, 9).is_empty(),
            "stale check"
        );
        let events = state.finish_held("r", held.generation, 9);
        assert!(matches!(
            events.last(),
            Some(AgentEvent::RunFinished {
                status: RunStatus::Completed,
                duration_ms: 9,
                ..
            })
        ));
        assert!(state.active_run_id().is_none());
    }

    #[test]
    fn a_held_run_can_continue_with_one_more_prompt() {
        let mut state = NormaliserState::new(catalog());
        state.set_hold_completion(true);
        state.begin_run("r", "s", "m", "p1", 0);
        normalise(&completed("p1"), &mut state, 1);
        let held = state.take_settled().unwrap();
        assert!(state.resume_held("r", held.generation, "p2"));
        assert!(!state.is_held());
        assert!(
            !state.resume_held("r", held.generation, "p3"),
            "already resumed"
        );
        normalise(&text("m2", "Revised."), &mut state, 2);
        assert!(normalise(&completed("p2"), &mut state, 3).is_empty());
        let again = state.take_settled().unwrap();
        assert_eq!(again.messages.len(), 1);
        assert_ne!(again.generation, held.generation);
    }

    /// A text delta frame for any text (escaped as JSON).
    fn text_frame(message: &str, delta: &str) -> InboundFrame {
        let frame = json!({
            "type": "message_update",
            "messageId": message,
            "assistantMessageEvent": {"type": "text_delta", "delta": delta},
        });
        parse_frame(&frame.to_string()).unwrap()
    }

    fn tool_call(message: &str, call: &str) -> InboundFrame {
        parse_frame(&format!(
            r#"{{"type":"message_update","messageId":"{message}","assistantMessageEvent":{{"type":"toolcall_end","toolCall":{{"type":"toolCall","id":"{call}","name":"search_documents","arguments":{{"query":"retainer"}}}}}}}}"#
        ))
        .unwrap()
    }

    fn shown_text(events: &[AgentEvent]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_fixture_preamble_before_its_searches_is_a_working_note() {
        let events = replay();
        let run = run_events(&events, "run-p5");
        let interim: Vec<(&str, bool)> = run
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextInterim {
                    message_id,
                    interim,
                    ..
                } => Some((message_id.as_str(), *interim)),
                _ => None,
            })
            .collect();
        assert_eq!(interim, vec![("msg-2", true)], "msg-5 is the answer");
        let marked = run
            .iter()
            .position(|e| matches!(e, AgentEvent::TextInterim { .. }))
            .unwrap();
        let first_step = run
            .iter()
            .position(|e| matches!(e, AgentEvent::StepStarted { .. }))
            .unwrap();
        assert!(marked < first_step, "marked before its steps show");
    }

    #[test]
    fn the_answer_checked_is_the_final_turn_not_the_preamble() {
        let mut state = NormaliserState::new(catalog());
        state.set_hold_completion(true);
        state.begin_run("r", "s", "m", "p1", 0);
        let mut events = Vec::new();
        events.extend(normalise(
            &text("m1", "I'll search for the monthly retainer."),
            &mut state,
            1,
        ));
        events.extend(normalise(&tool_call("m1", "t1"), &mut state, 2));
        events.extend(normalise(
            &text("m2", "The retainer is INR 2,75,000 [11]."),
            &mut state,
            3,
        ));
        events.extend(normalise(&completed("p1"), &mut state, 4));
        assert_eq!(
            shown_text(&events),
            "I'll search for the monthly retainer.The retainer is INR 2,75,000 [11].",
            "nothing streamed is taken back"
        );
        let held = state.take_settled().unwrap();
        assert_eq!(
            held.messages,
            vec![(
                "m2".to_string(),
                "The retainer is INR 2,75,000 [11].".to_string()
            )]
        );
    }

    #[test]
    fn a_round_with_only_a_note_keeps_it_as_the_answer() {
        let mut state = NormaliserState::new(catalog());
        state.set_hold_completion(true);
        state.begin_run("r", "s", "m", "p1", 0);
        normalise(&text("m1", "Sixty days [1]."), &mut state, 1);
        normalise(&tool_call("m1", "t1"), &mut state, 2);
        let events = normalise(&completed("p1"), &mut state, 3);
        assert_eq!(
            events,
            vec![AgentEvent::TextInterim {
                run_id: "r".into(),
                message_id: "m1".into(),
                interim: false,
            }]
        );
        let held = state.take_settled().unwrap();
        assert_eq!(held.messages.len(), 1);
    }

    #[test]
    fn a_follow_up_turn_shows_its_answer_without_the_apology() {
        let mut state = NormaliserState::new(catalog());
        state.set_hold_completion(true);
        state.begin_run("r", "s", "m", "p1", 0);
        normalise(&text("m1", "Interest is 2% per month [1]."), &mut state, 1);
        normalise(&completed("p1"), &mut state, 2);
        let held = state.take_settled().unwrap();
        assert!(state.resume_held("r", held.generation, "p2"));

        let answer = "Late payments accrue 1.5% per month [2].";
        let turn = format!(
            "I apologize for the error in my previous response. Let me provide a corrected \
             answer with proper citations.\n\n{answer}"
        );
        let mut events = Vec::new();
        // Streamed in small pieces, as providers do.
        let chars: Vec<char> = turn.chars().collect();
        for piece in chars.chunks(5) {
            let piece: String = piece.iter().collect();
            events.extend(normalise(&text_frame("m2", &piece), &mut state, 3));
        }
        events.extend(normalise(&completed("p2"), &mut state, 4));
        assert_eq!(shown_text(&events), answer);
        let again = state.take_settled().unwrap();
        assert_eq!(
            again.messages,
            vec![
                (
                    "m1".to_string(),
                    "Interest is 2% per month [1].".to_string()
                ),
                ("m2".to_string(), answer.to_string()),
            ],
            "the stored text is the shown text"
        );
    }

    #[test]
    fn first_answers_are_streamed_as_written() {
        let mut state = NormaliserState::new(catalog());
        state.begin_run("r", "s", "m", "p1", 0);
        let events = normalise(
            &text("m1", "I apologize, the documents say nothing."),
            &mut state,
            1,
        );
        assert_eq!(
            shown_text(&events),
            "I apologize, the documents say nothing."
        );
    }

    #[test]
    fn steering_or_stopping_a_held_run_discards_its_check() {
        let mut state = NormaliserState::new(catalog());
        state.set_hold_completion(true);
        state.begin_run("r", "s", "m", "p1", 0);
        normalise(&completed("p1"), &mut state, 1);
        let held = state.take_settled().unwrap();
        assert!(state.attach_prompt("p2"));
        assert!(!state.still_held("r", held.generation));
        assert!(state.finish_held("r", held.generation, 4).is_empty());
        normalise(&completed("p2"), &mut state, 5);
        state.take_settled().unwrap();
        let events = state.abort_held(6);
        assert!(matches!(
            events.last(),
            Some(AgentEvent::RunFinished {
                status: RunStatus::Aborted,
                ..
            })
        ));
    }

    #[test]
    fn failed_runs_are_never_held() {
        let mut state = NormaliserState::new(catalog());
        state.set_hold_completion(true);
        state.begin_run("r", "s", "m", "p1", 0);
        let failed = parse_frame(
            r#"{"type":"prompt_result","id":"p1","agentInvoked":true,"status":"aborted"}"#,
        )
        .unwrap();
        assert!(matches!(
            normalise(&failed, &mut state, 2).last(),
            Some(AgentEvent::RunFinished {
                status: RunStatus::Aborted,
                ..
            })
        ));
    }

    #[test]
    fn labels_render_placeholders() {
        assert_eq!(
            render_label("Searching {query}", &json!({"query": "  invoices "})),
            "Searching “invoices”"
        );
        assert_eq!(render_label("Opening {path}", &json!({})), "Opening");
        assert_eq!(
            render_label(
                "Page {page} of {path}",
                &json!({"page": 3, "path": "a.pdf"})
            ),
            "Page “3” of “a.pdf”"
        );
        assert_eq!(render_label("Broken {brace", &json!({})), "Broken {brace");
    }

    #[test]
    fn optional_label_groups_drop_whole() {
        let template = "Listing tasks[ due {due_from!} to {due_to!}][ matching {text}]";
        assert_eq!(render_label(template, &json!({})), "Listing tasks");
        assert_eq!(
            render_label(
                template,
                &json!({"due_from": "2026-10-05", "due_to": "2026-10-11"})
            ),
            "Listing tasks due 2026-10-05 to 2026-10-11"
        );
        assert_eq!(
            render_label(template, &json!({"due_from": "2026-10-05", "text": "gst"})),
            "Listing tasks matching “gst”"
        );
        assert_eq!(
            render_label("Unclosed [group {x}", &json!({"x": "y"})),
            "Unclosed [group “y”"
        );
    }
}
