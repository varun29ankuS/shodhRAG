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

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::events::{AgentEvent, RiskTier, RunStatus};
use super::protocol::{
    AssistantMessageEvent, InboundFrame, MessageEndFrame, PromptResultFrame, PromptStatus,
    ResponseFrame, ToolResultPayload,
};
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
}

const MAX_PROGRESS_CHARS: usize = 400;
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
    /// is then absorbed instead of producing a separate run.
    pub fn attach_prompt(&mut self, prompt_id: &str) -> bool {
        match self.run.as_mut() {
            Some(run) => {
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

    /// End the active run with an error (e.g. the sidecar exited).
    pub fn fail_run(&mut self, error: &str, now_ms: u64) -> Vec<AgentEvent> {
        match self.run.as_mut() {
            Some(run) => {
                run.worst_status = RunStatus::Error;
                run.error = Some(error.to_string());
                self.finish_run(now_ms)
            }
            None => Vec::new(),
        }
    }

    fn finish_run(&mut self, now_ms: u64) -> Vec<AgentEvent> {
        let Some(run) = self.run.take() else {
            return Vec::new();
        };
        let interrupted_summary = match run.worst_status {
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
                });
                AgentEvent::StepFinished {
                    run_id: run.run_id.clone(),
                    step_id,
                    ok: outcome.ok,
                    summary: outcome.summary,
                    detail: outcome.detail,
                    duration_ms: now_ms.saturating_sub(step.started_at_ms),
                }
            })
            .collect();
        self.host_calls.clear();
        events.push(AgentEvent::RunFinished {
            run_id: run.run_id,
            status: run.worst_status,
            duration_ms: now_ms.saturating_sub(run.started_at_ms),
            error: run.error,
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
        if run.pending_prompts.is_empty() {
            self.finish_run(now_ms)
        } else {
            Vec::new()
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
        self.on_prompt_completion(result.id.as_deref(), status, error, now_ms)
    }

    fn on_message_end(&mut self, frame: &MessageEndFrame, run_id: String) -> Vec<AgentEvent> {
        if frame.message.role != "assistant" {
            return Vec::new();
        }
        for call in frame.message.tool_calls() {
            if let Some(intent) = call.intent() {
                self.remember_intent(&call.id, intent);
            }
        }
        let Some(usage) = frame.message.usage.as_ref() else {
            return Vec::new();
        };
        let cost = usage.cost.as_ref().map(|c| c.total).unwrap_or(0.0);
        if usage.input == 0 && usage.output == 0 && usage.cache_read == 0 && cost == 0.0 {
            return Vec::new();
        }
        let Some(run) = self.run.as_mut() else {
            return Vec::new();
        };
        run.usage.input += usage.input;
        run.usage.output += usage.output;
        run.usage.cache_read += usage.cache_read;
        run.usage.cost += cost;
        vec![AgentEvent::Usage {
            run_id,
            input_tokens: run.usage.input,
            output_tokens: run.usage.output,
            cache_read_tokens: run.usage.cache_read,
            cost_usd: run.usage.cost,
        }]
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
                Vec::new()
            }
            AssistantMessageEvent::TextDelta { delta } => match state.active_run_id() {
                Some(run_id) if !delta.is_empty() => vec![AgentEvent::TextDelta {
                    run_id: run_id.to_string(),
                    message_id: update.message_id.clone(),
                    delta: delta.clone(),
                }],
                _ => Vec::new(),
            },
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
            let (summary, detail) = match step.outcome {
                Some(outcome) => (outcome.summary, outcome.detail),
                None => (first_line_summary(end.result.as_ref(), ok), None),
            };
            vec![AgentEvent::StepFinished {
                run_id,
                step_id: end.tool_call_id.clone(),
                ok,
                summary,
                detail,
                duration_ms: now_ms.saturating_sub(step.started_at_ms),
            }]
        }
        InboundFrame::Ready(_)
        | InboundFrame::AgentStart
        | InboundFrame::AgentEnd(_)
        | InboundFrame::TurnEnd
        | InboundFrame::SessionSettled
        | InboundFrame::HostToolCancel(_)
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
