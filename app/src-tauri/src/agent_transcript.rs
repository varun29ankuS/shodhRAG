//! The agent transcript of one answer, reduced in the backend: the same fold of the
//! `agent_event` stream that the Ask view runs (`app/src/features/agent/reducer.ts`,
//! `reduceTranscript`), so the backend can save an answer in exactly the form the UI
//! stores in `ConversationMessage.transcript`, with or without a window open.
//!
//! - **The same input.** It folds each event in its JSON form, as the window receives it
//!   (serialised, then parsed: numbers come out as the UI reads them), plus the few local
//!   actions only the app knows: a steer, an approval answered, a request that failed
//!   before the run started, an interrupt that timed out (`local_*`).
//! - **The same start.** A run starts from the transcript the app saved its answer with
//!   (`startedAnswer` in `features/ask/messageStore.ts`): the run id, the local clock it
//!   started at, its mode, a fallback model and the tries it replaced. None of these are
//!   in the event stream.
//! - **The same output.** [`Transcript::persisted`] is `toPersisted`, and
//!   [`Transcript::checkpoint`] is `toCheckpoint`.
//! - **Kept equal.** The fixtures in `app/tests/fixtures/transcripts` are reduced by both
//!   (`app/tests/transcriptParity.test.ts` and the tests below), and both must give the
//!   fixture's expected transcript and answer text. A change to either reducer changes
//!   both, and the fixtures.
//!
//! String lengths are counted in UTF-16 code units, as JavaScript counts them (the
//! reasoning cap, the command output tail, the live reasoning line).

use serde_json::{json, Map, Value};

use shodh_rag::harness::AgentEvent;

/// Most characters of reasoning kept per answer (`MAX_REASONING_CHARS`).
pub const MAX_REASONING_CHARS: usize = 32 * 1024;

/// Most output characters kept for a command while the answer runs.
pub const MAX_COMMAND_OUTPUT_CHARS: usize = 64 * 1024;

/// Most output characters stored with the conversation per command.
pub const MAX_PERSISTED_COMMAND_OUTPUT_CHARS: usize = 16 * 1024;

/// Note of a command still running when its answer ended.
pub const ENDED_WITH_ANSWER_NOTE: &str = "Ended with the answer";

/// Most memory ids kept with one answer.
pub const MAX_MEMORY_IDS: usize = 32;

/// Characters of reasoning the live status line keeps.
const LIVE_THINKING_CHARS: usize = 600;

/// Longest upstream provider name kept with a provider failure.
const MAX_UPSTREAM_CHARS: usize = 64;

const PLAN_STATUSES: [&str; 4] = ["pending", "in_progress", "done", "blocked"];

const ERROR_KINDS: [&str; 11] = [
    "rate_limited",
    "quota_exhausted",
    "model_unavailable",
    "auth",
    "timeout",
    "network",
    "invalid_request",
    "context_too_long",
    "content_filtered",
    "empty_response",
    "other",
];

/// An event as the window receives it: serialised, then parsed.
pub fn event_value(event: &AgentEvent) -> Value {
    serde_json::to_string(event)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

/// The persisted transcript of a run that started as `seed` (the transcript its answer
/// was first saved with) after `actions` (events in their JSON form, and `local_*`
/// actions), as the Ask view stores it. The agent session folds the same way one action
/// at a time ([`Transcript::apply`]), writing at its checkpoints.
#[cfg(test)]
pub fn reduce_transcript(seed: &Value, actions: &[Value]) -> Value {
    let mut transcript = Transcript::new(seed);
    for action in actions {
        transcript.apply(action);
    }
    transcript.persisted()
}

/// The transcript of one run, folded one action at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    state: Map<String, Value>,
    /// UTF-16 units of reasoning kept so far (the segments' text), so a thinking token
    /// does not count the whole log again.
    reasoning_units: usize,
}

impl Transcript {
    /// Starts from `seed`, the transcript the answer was saved with when it started. A
    /// key the seed lacks starts as `initialTranscript` sets it.
    pub fn new(seed: &Value) -> Self {
        let mut state = Map::new();
        let defaults = json!({
            "providerError": null,
            "fallback": null,
            "runId": "",
            "sessionId": null,
            "model": null,
            "warning": null,
            "status": "starting",
            "startedAtMs": 0,
            "durationMs": null,
            "error": null,
            "errorCode": null,
            "blocks": [],
            "steps": {},
            "plan": null,
            "usage": null,
            "passages": [],
            "groundings": [],
            "interimIds": [],
            "checking": false,
            "thinking": "",
            "interrupting": false,
        });
        if let Value::Object(defaults) = defaults {
            state = defaults;
        }
        if let Value::Object(seed) = seed {
            for (key, value) in seed {
                state.insert(key.clone(), value.clone());
            }
        }
        let reasoning_units = state
            .get("reasoning")
            .and_then(|r| r.get("segments"))
            .and_then(Value::as_array)
            .map_or(0, |segments| {
                segments
                    .iter()
                    .map(|seg| utf16_len(seg.get("text").and_then(Value::as_str).unwrap_or("")))
                    .sum()
            });
        Self {
            state,
            reasoning_units,
        }
    }

    /// The run's id.
    pub fn run_id(&self) -> &str {
        self.state
            .get("runId")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// The run's status: `starting`, `running`, `completed`, `aborted` or `error`.
    pub fn status(&self) -> &str {
        self.state
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// Whether the run is still going (`isLive`).
    pub fn is_live(&self) -> bool {
        matches!(self.status(), "starting" | "running")
    }

    /// Folds one action (`reduceTranscript`).
    pub fn apply(&mut self, action: &Value) {
        let Some(action) = action.as_object() else {
            return;
        };
        let kind = action.get("type").and_then(Value::as_str).unwrap_or("");
        if !kind.starts_with("local_") {
            let run = action.get("runId").and_then(Value::as_str);
            if run != Some(self.run_id()) {
                return;
            }
        }
        // A finished run only accepts late usage totals.
        if !self.is_live() && kind != "usage" {
            return;
        }
        let Self {
            state: s,
            reasoning_units,
        } = self;
        match kind {
            "run_started" => {
                s.insert("sessionId".into(), field(action, "sessionId"));
                s.insert("model".into(), field(action, "model"));
                s.insert("warning".into(), field(action, "warning"));
                s.insert("status".into(), json!("running"));
            }
            "text_delta" => {
                let delta = str_field(action, "delta");
                if delta.is_empty() {
                    return;
                }
                let id = str_field(action, "messageId");
                s.insert("status".into(), json!("running"));
                append_text(array_mut(s, "blocks"), id, delta);
                s.insert("thinking".into(), json!(""));
            }
            "text_interim" => {
                let id = field(action, "messageId");
                let mut ids: Vec<Value> = s
                    .get("interimIds")
                    .and_then(Value::as_array)
                    .map(|ids| ids.iter().filter(|i| **i != id).cloned().collect())
                    .unwrap_or_default();
                if action.get("interim") == Some(&Value::Bool(true)) {
                    ids.push(id);
                }
                s.insert("interimIds".into(), Value::Array(ids));
            }
            "thinking" => {
                let delta = str_field(action, "delta");
                if delta.is_empty() {
                    return;
                }
                s.insert("status".into(), json!("running"));
                let live = format!(
                    "{}{}",
                    s.get("thinking").and_then(Value::as_str).unwrap_or(""),
                    delta
                );
                s.insert(
                    "thinking".into(),
                    json!(utf16_slice_from(
                        &live,
                        utf16_len(&live).saturating_sub(LIVE_THINKING_CHARS)
                    )),
                );
                let at = s
                    .get("blocks")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                append_reasoning(s, at, delta, reasoning_units);
            }
            "step_started" => {
                let id = str_field(action, "stepId").to_string();
                let steps = object_mut(s, "steps");
                if steps.contains_key(&id) {
                    return;
                }
                let parent = action
                    .get("parentStepId")
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty() && steps.contains_key(*p))
                    .map(str::to_string);
                let step = json!({
                    "id": id,
                    "parentId": parent,
                    "tool": field(action, "tool"),
                    "label": field(action, "label"),
                    "args": field(action, "args"),
                    "tier": field(action, "tier"),
                    "startedAtMs": field(action, "atMs"),
                    "status": "running",
                    "summary": null,
                    "detail": null,
                    "durationMs": null,
                    "progress": null,
                    "approval": null,
                    "children": [],
                });
                steps.insert(id.clone(), step);
                if let Some(parent) = parent {
                    if let Some(children) = steps
                        .get_mut(&parent)
                        .and_then(|p| p.get_mut("children"))
                        .and_then(Value::as_array_mut)
                    {
                        children.push(json!(id));
                    }
                } else {
                    array_mut(s, "blocks").push(json!({ "kind": "step", "stepId": id }));
                }
                s.insert("status".into(), json!("running"));
                s.insert("thinking".into(), json!(""));
            }
            "step_progress" => {
                if let Some(step) = step_mut(s, action) {
                    step.insert("progress".into(), field(action, "text"));
                }
            }
            "approval_requested" => {
                if let Some(step) = step_mut(s, action) {
                    step.insert("status".into(), json!("awaiting_approval"));
                    step.insert(
                        "approval".into(),
                        json!({
                            "label": field(action, "label"),
                            "tier": field(action, "tier"),
                            "preview": field(action, "preview"),
                            "decision": "pending",
                        }),
                    );
                }
            }
            "step_finished" => step_finished(s, action),
            "command_started" => {
                let Some(step) = step_mut(s, action) else {
                    return;
                };
                let command_id = field(action, "commandId");
                if has_command(step, &command_id) {
                    return;
                }
                let time_limit = action
                    .get("timeLimitSecs")
                    .filter(|v| v.is_number())
                    .cloned()
                    .unwrap_or(Value::Null);
                let sandbox = action
                    .get("sandbox")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map_or(Value::Null, |s| json!(s));
                step.insert(
                    "command".into(),
                    start_command(
                        command_id,
                        field(action, "command"),
                        field(action, "atMs"),
                        time_limit,
                        sandbox,
                    ),
                );
            }
            "command_output" => {
                let Some(step) = step_mut(s, action) else {
                    return;
                };
                if !has_command(step, &field(action, "commandId")) {
                    return;
                }
                if let Some(Value::Object(command)) = step.get_mut("command") {
                    append_command_output(command, str_field(action, "text"));
                }
            }
            "command_finished" => {
                let Some(step) = step_mut(s, action) else {
                    return;
                };
                let command_id = field(action, "commandId");
                if has_command(step, &command_id) {
                    if let Some(Value::Object(command)) = step.get_mut("command") {
                        finish_command(command, action);
                    }
                    return;
                }
                if truthy(step.get("command")) {
                    return;
                }
                // No start came (a refused command never starts): the run is made from the call.
                let text = step
                    .get("args")
                    .and_then(|a| a.get("command"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let started = field(step, "startedAtMs");
                let mut run =
                    start_command(command_id, json!(text), started, Value::Null, Value::Null);
                if let Value::Object(command) = &mut run {
                    finish_command(command, action);
                }
                step.insert("command".into(), run);
            }
            "plan_updated" => {
                let items: Vec<Value> = action
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().map(plan_item).collect())
                    .unwrap_or_default();
                let plan = if items.is_empty() {
                    Value::Null
                } else {
                    Value::Array(items)
                };
                s.insert("plan".into(), plan);
            }
            "grounding_started" => {
                s.insert("checking".into(), json!(true));
            }
            "grounding" => {
                array_mut(s, "groundings").push(field(action, "report"));
                s.insert("checking".into(), json!(false));
            }
            "revision_started" => {
                s.insert("checking".into(), json!(false));
                let round = field(action, "round");
                let id = format!("revision-{}", js_number_text(&round));
                array_mut(s, "blocks").push(json!({
                    "kind": "revision",
                    "id": id,
                    "round": round,
                    "reason": field(action, "reason"),
                    "flagged": field(action, "flagged"),
                    "missingNeeds": field(action, "missingNeeds"),
                }));
            }
            "memories_used" => {
                let ids = memory_ids_of(action.get("ids"));
                if !ids.is_empty() {
                    s.insert("memoryIds".into(), json!(ids));
                }
            }
            "usage" => {
                s.insert(
                    "usage".into(),
                    json!({
                        "inputTokens": field(action, "inputTokens"),
                        "outputTokens": field(action, "outputTokens"),
                        "cacheReadTokens": field(action, "cacheReadTokens"),
                        "costUsd": field(action, "costUsd"),
                    }),
                );
            }
            "run_finished" => {
                let status = match action.get("status").and_then(Value::as_str) {
                    Some("completed") => "completed",
                    Some("aborted") => "aborted",
                    _ => "error",
                };
                let duration = field(action, "durationMs");
                let end = number(s.get("startedAtMs")) + number(Some(&duration));
                s.insert("status".into(), json!(status));
                s.insert("durationMs".into(), duration);
                s.insert("error".into(), field(action, "error"));
                let failed = status == "error";
                s.insert(
                    "errorCode".into(),
                    if failed {
                        json!("runtime_error")
                    } else {
                        Value::Null
                    },
                );
                s.insert(
                    "providerError".into(),
                    if failed {
                        read_provider_error(action.get("providerError"))
                    } else {
                        Value::Null
                    },
                );
                let summary = if status == "aborted" {
                    "Interrupted"
                } else {
                    "Did not complete"
                };
                end_steps(s, summary, end);
                s.insert("thinking".into(), json!(""));
                s.insert("interrupting".into(), json!(false));
                s.insert("checking".into(), json!(false));
            }
            "local_interrupt_requested" => {
                s.insert("interrupting".into(), json!(true));
            }
            "local_steer" => {
                array_mut(s, "blocks").push(json!({
                    "kind": "steer",
                    "id": field(action, "id"),
                    "text": field(action, "text"),
                }));
            }
            "local_approval" => {
                let approved = action.get("approved") == Some(&Value::Bool(true));
                if let Some(step) = step_mut(s, action) {
                    if !truthy(step.get("approval")) {
                        return;
                    }
                    if approved {
                        step.insert("status".into(), json!("running"));
                    }
                    if let Some(Value::Object(approval)) = step.get_mut("approval") {
                        approval.insert(
                            "decision".into(),
                            json!(if approved { "approved" } else { "denied" }),
                        );
                    }
                }
            }
            "local_failed" => {
                let at = number(action.get("atMs"));
                // The request failed before the model got the question: no memory went into it.
                s.remove("memoryIds");
                s.insert("status".into(), json!("error"));
                s.insert("error".into(), field(action, "error"));
                s.insert("errorCode".into(), field(action, "code"));
                if let Some(model) = action.get("model").filter(|m| !m.is_null()) {
                    s.insert("model".into(), model.clone());
                }
                let provider = action
                    .get("providerError")
                    .filter(|p| !p.is_null())
                    .or_else(|| s.get("providerError").filter(|p| !p.is_null()))
                    .cloned()
                    .unwrap_or(Value::Null);
                s.insert("providerError".into(), provider);
                let started = number(s.get("startedAtMs"));
                s.insert("durationMs".into(), js_number((at - started).max(0.0)));
                end_steps(s, "Did not complete", at);
                s.insert("thinking".into(), json!(""));
                s.insert("interrupting".into(), json!(false));
            }
            "local_interrupted" => {
                let at = number(action.get("atMs"));
                let started = number(s.get("startedAtMs"));
                s.insert("status".into(), json!("aborted"));
                s.insert("durationMs".into(), js_number((at - started).max(0.0)));
                end_steps(s, "Interrupted", at);
                s.insert("thinking".into(), json!(""));
                s.insert("interrupting".into(), json!(false));
            }
            _ => {}
        }
    }

    /// The form stored with the conversation (`toPersisted`): no transient status text,
    /// command output cut to its stored tail.
    pub fn persisted(&self) -> Value {
        let mut state = self.state.clone();
        if let Some(Value::Object(steps)) = state.get_mut("steps") {
            for step in steps.values_mut() {
                if let Some(Value::Object(command)) = step.get_mut("command") {
                    trim_command_output(command, MAX_PERSISTED_COMMAND_OUTPUT_CHARS);
                }
            }
        }
        state.insert("thinking".into(), json!(""));
        state.insert("interrupting".into(), json!(false));
        state.insert("checking".into(), json!(false));
        Value::Object(state)
    }

    /// The form stored while the run is live and its answer text is complete
    /// (`toCheckpoint`): only a final grounding report, marked as saved while checking.
    pub fn checkpoint(&self) -> Value {
        let mut value = self.persisted();
        if let Value::Object(state) = &mut value {
            let finals: Vec<Value> = state
                .get("groundings")
                .and_then(Value::as_array)
                .map(|all| {
                    all.iter()
                        .filter(|g| g.get("isFinal") == Some(&Value::Bool(true)))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            state.insert("groundings".into(), Value::Array(finals));
            state.insert("savedWhileChecking".into(), json!(true));
        }
        value
    }

    /// The answer text (`answerText`): every text block in order, without replaced
    /// drafts and working notes.
    pub fn answer_text(&self) -> String {
        let mut excluded: Vec<&str> = Vec::new();
        if let Some(reports) = self.state.get("groundings").and_then(Value::as_array) {
            for report in reports {
                if let Some(ids) = report.get("supersededMessageIds").and_then(Value::as_array) {
                    excluded.extend(ids.iter().filter_map(Value::as_str));
                }
            }
        }
        if let Some(ids) = self.state.get("interimIds").and_then(Value::as_array) {
            excluded.extend(ids.iter().filter_map(Value::as_str));
        }
        let blocks = self.state.get("blocks").and_then(Value::as_array);
        blocks
            .into_iter()
            .flatten()
            .filter(|b| b.get("kind").and_then(Value::as_str) == Some("text"))
            .filter(|b| {
                let id = b.get("id").and_then(Value::as_str).unwrap_or("");
                !excluded.contains(&id)
            })
            .map(|b| b.get("text").and_then(Value::as_str).unwrap_or("").trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Whether a batch that started the answer's check makes it a checkpoint
    /// (`startsCheck`): the text is complete and not empty.
    pub fn starts_check(&self, action: &Value) -> bool {
        action.get("type").and_then(Value::as_str) == Some("grounding_started")
            && !self.answer_text().trim().is_empty()
    }
}

fn field(object: &Map<String, Value>, key: &str) -> Value {
    object.get(key).cloned().unwrap_or(Value::Null)
}

fn str_field<'a>(object: &'a Map<String, Value>, key: &str) -> &'a str {
    object.get(key).and_then(Value::as_str).unwrap_or("")
}

/// JavaScript truthiness of a stored value (absent, null, false, 0, "" are not).
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

fn number(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(0.0)
}

/// A number as JavaScript writes it to JSON: whole numbers without a fraction.
fn js_number(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 9.007_199_254_740_992e15 {
        // Whole and within the exact integer range, so the cast is exact.
        json!(value as i64)
    } else {
        json!(value)
    }
}

/// A number as `String(n)` writes it.
fn js_number_text(value: &Value) -> String {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e21 => {
                format!("{}", f as i64)
            }
            Some(f) => format!("{f}"),
            None => n.to_string(),
        },
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

fn whole_number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|f| f.is_finite() && f.fract() == 0.0)
}

fn array_mut<'a>(state: &'a mut Map<String, Value>, key: &str) -> &'a mut Vec<Value> {
    let slot = state.entry(key.to_string()).or_insert_with(|| json!([]));
    if !slot.is_array() {
        *slot = json!([]);
    }
    match slot {
        Value::Array(items) => items,
        _ => unreachable!("set to an array above"),
    }
}

fn object_mut<'a>(state: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let slot = state.entry(key.to_string()).or_insert_with(|| json!({}));
    if !slot.is_object() {
        *slot = json!({});
    }
    match slot {
        Value::Object(map) => map,
        _ => unreachable!("set to an object above"),
    }
}

/// The step an action names, when the transcript has it.
fn step_mut<'a>(
    state: &'a mut Map<String, Value>,
    action: &Map<String, Value>,
) -> Option<&'a mut Map<String, Value>> {
    let id = action.get("stepId").and_then(Value::as_str)?;
    match state.get_mut("steps")?.get_mut(id)? {
        Value::Object(step) => Some(step),
        _ => None,
    }
}

fn has_command(step: &Map<String, Value>, command_id: &Value) -> bool {
    truthy(step.get("command"))
        && step.get("command").and_then(|c| c.get("commandId")) == Some(command_id)
}

fn append_text(blocks: &mut Vec<Value>, id: &str, delta: &str) {
    let is_text = |b: &Value| {
        b.get("kind").and_then(Value::as_str) == Some("text")
            && b.get("id").and_then(Value::as_str) == Some(id)
    };
    let at = match blocks.last() {
        Some(last) if is_text(last) => Some(blocks.len() - 1),
        _ => blocks.iter().position(is_text),
    };
    match at {
        Some(index) => {
            if let Some(Value::Object(block)) = blocks.get_mut(index) {
                match block.get_mut("text") {
                    Some(Value::String(text)) => text.push_str(delta),
                    _ => {
                        block.insert("text".into(), json!(delta));
                    }
                }
            }
        }
        None => blocks.push(json!({ "kind": "text", "id": id, "text": delta })),
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `text.slice(0, end)` in UTF-16 code units.
fn utf16_slice_to(text: &str, end: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().take(end).collect();
    String::from_utf16_lossy(&units)
}

/// `text.slice(start)` in UTF-16 code units.
fn utf16_slice_from(text: &str, start: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().skip(start).collect();
    String::from_utf16_lossy(&units)
}

/// The state's reasoning log with `delta` appended at block position `at`, within
/// [`MAX_REASONING_CHARS`] (`appendReasoning`); `used` counts the units kept so far.
fn append_reasoning(state: &mut Map<String, Value>, at: usize, delta: &str, used: &mut usize) {
    let log = object_mut(state, "reasoning");
    if log.get("truncated") == Some(&Value::Bool(true)) {
        return;
    }
    let room = MAX_REASONING_CHARS.saturating_sub(*used);
    let delta_len = utf16_len(delta);
    let kept = if delta_len <= room {
        delta.to_string()
    } else {
        utf16_slice_to(delta, room)
    };
    let kept_len = utf16_len(&kept);
    let truncated = kept_len < delta_len;
    let segments = array_mut(log, "segments");
    if kept_len > 0 {
        *used += kept_len;
        let joins = segments
            .last()
            .and_then(|last| last.get("at"))
            .and_then(Value::as_f64)
            .is_some_and(|last_at| last_at == at as f64);
        if joins {
            if let Some(Value::String(text)) =
                segments.last_mut().and_then(|last| last.get_mut("text"))
            {
                text.push_str(&kept);
            }
        } else {
            segments.push(json!({ "at": at, "text": kept }));
        }
    }
    log.insert("truncated".into(), json!(truncated));
}

fn step_finished(s: &mut Map<String, Value>, action: &Map<String, Value>) {
    let ok = action.get("ok") == Some(&Value::Bool(true));
    let invalid_arguments = !ok && action.get("invalidArguments") == Some(&Value::Bool(true));
    let tool = {
        let Some(step) = step_mut(s, action) else {
            return;
        };
        step.insert("status".into(), json!(if ok { "done" } else { "failed" }));
        step.insert("summary".into(), field(action, "summary"));
        step.insert("detail".into(), field(action, "detail"));
        step.insert("durationMs".into(), field(action, "durationMs"));
        step.insert("progress".into(), Value::Null);
        if let Some(Value::Object(approval)) = step.get_mut("approval") {
            if approval.get("decision").and_then(Value::as_str) == Some("pending") {
                approval.insert(
                    "decision".into(),
                    json!(if ok { "approved" } else { "denied" }),
                );
            }
        }
        if invalid_arguments {
            step.insert("invalidArguments".into(), json!(true));
        }
        field(step, "tool")
    };
    if !ok {
        return;
    }
    // The tool's earlier calls refused for their arguments were the model correcting itself.
    for step in object_mut(s, "steps").values_mut() {
        if let Value::Object(step) = step {
            if step.get("tool") == Some(&tool)
                && step.get("status").and_then(Value::as_str) == Some("failed")
                && truthy(step.get("invalidArguments"))
            {
                step.insert("status".into(), json!("retried"));
            }
        }
    }
    let incoming = passages_from_detail(action.get("detail"));
    if !incoming.is_empty() {
        let passages = array_mut(s, "passages");
        merge_passages(passages, incoming);
    }
}

/// `existing` with `incoming` replacing the passages of the same number, ordered by
/// number (`mergePassages`).
fn merge_passages(existing: &mut Vec<Value>, incoming: Vec<Value>) {
    let key = |p: &Value| p.get("n").and_then(Value::as_f64).unwrap_or(0.0);
    let mut merged: Vec<Value> = Vec::new();
    for passage in existing.drain(..).chain(incoming) {
        match merged.iter().position(|p| key(p) == key(&passage)) {
            Some(index) => merged[index] = passage,
            None => merged.push(passage),
        }
    }
    merged.sort_by(|a, b| key(a).total_cmp(&key(b)));
    *existing = merged;
}

/// A non-empty string, else nothing (`str`).
fn non_empty(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn is_http_url(value: &str) -> bool {
    let lower = value.get(..8).map(str::to_ascii_lowercase);
    lower.as_deref().is_some_and(|p| p.starts_with("https://"))
        || value
            .get(..7)
            .map(str::to_ascii_lowercase)
            .is_some_and(|p| p == "http://")
}

/// A positive whole citation number.
fn citation_number(value: Option<&Value>) -> Option<&Value> {
    value.filter(|v| whole_number(Some(v)).is_some_and(|n| n >= 1.0))
}

/// Numbered sources carried by a step's detail: web sources first, then document
/// passages (`passagesFromDetail`).
fn passages_from_detail(detail: Option<&Value>) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(Value::Object(detail)) = detail else {
        return out;
    };
    if let Some(Value::Array(sources)) = detail.get("webSources") {
        for entry in sources {
            let Value::Object(entry) = entry else {
                continue;
            };
            let Some(n) = citation_number(entry.get("n")) else {
                continue;
            };
            let Some(url) = non_empty(entry.get("url")).filter(|u| is_http_url(u)) else {
                continue;
            };
            out.push(json!({
                "n": n,
                "file": non_empty(entry.get("title")).unwrap_or(url),
                "path": url,
                "page": null,
                "heading": null,
                "score": 0,
                "text": entry.get("snippet").and_then(Value::as_str).unwrap_or(""),
                "web": true,
            }));
        }
    }
    let Some(Value::Array(passages)) = detail.get("passages") else {
        return out;
    };
    for entry in passages {
        let Value::Object(entry) = entry else {
            continue;
        };
        let Some(n) = citation_number(entry.get("n")) else {
            continue;
        };
        let Some(path) = non_empty(entry.get("path")) else {
            continue;
        };
        let page = match entry.get("page") {
            Some(v @ Value::Number(_)) => json!(js_number_text(v)),
            other => non_empty(other).map_or(Value::Null, |p| json!(p)),
        };
        let score = entry
            .get("score")
            .filter(|s| s.is_number())
            .cloned()
            .unwrap_or(json!(0));
        let mut passage = json!({
            "n": n,
            "file": non_empty(entry.get("file")).unwrap_or(path),
            "path": path,
            "page": page,
            "heading": non_empty(entry.get("heading")),
            "score": score,
            "text": entry.get("text").and_then(Value::as_str).unwrap_or(""),
            "section": non_empty(entry.get("section")),
            "regions": parse_regions(entry.get("regions")),
        });
        if let Value::Object(p) = &mut passage {
            if let Some(display) = non_empty(entry.get("display_path")) {
                p.insert("displayPath".into(), json!(display));
            }
            if let Some(link) = slack_permalink(path, non_empty(entry.get("url"))) {
                p.insert("url".into(), json!(link));
            }
        }
        out.push(passage);
    }
    out
}

/// The permalink a Slack thread's passage carries: only for a `slack://` passage, only https.
fn slack_permalink<'a>(path: &str, url: Option<&'a str>) -> Option<&'a str> {
    let url = url?;
    if !path.starts_with("slack://") {
        return None;
    }
    let scheme = url.get(..8)?;
    let rest = &url[8..];
    (scheme.eq_ignore_ascii_case("https://")
        && !rest.is_empty()
        && !rest.chars().any(char::is_whitespace))
    .then_some(url)
}

/// Layout boxes on cited pages (`parseRegions`); null when there are none.
fn parse_regions(value: Option<&Value>) -> Value {
    let parsed;
    let raw = match value {
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(v) => {
                parsed = v;
                &parsed
            }
            Err(_) => return Value::Null,
        },
        Some(v) => v,
        None => return Value::Null,
    };
    let Value::Array(entries) = raw else {
        return Value::Null;
    };
    let mut out = Vec::new();
    for entry in entries {
        let Value::Object(r) = entry else { continue };
        let finite = |key: &str| r.get(key).and_then(Value::as_f64).filter(|f| f.is_finite());
        if finite("page").is_none_or(|p| p.fract() != 0.0 || p < 1.0) {
            continue;
        }
        let (Some(x0), Some(y0), Some(x1), Some(y1)) =
            (finite("x0"), finite("y0"), finite("x1"), finite("y1"))
        else {
            continue;
        };
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        out.push(json!({
            "page": field(r, "page"),
            "x0": field(r, "x0"),
            "y0": field(r, "y0"),
            "x1": field(r, "x1"),
            "y1": field(r, "y1"),
        }));
    }
    if out.is_empty() {
        Value::Null
    } else {
        Value::Array(out)
    }
}

/// A plan item from an event (`planItem`).
fn plan_item(item: &Value) -> Value {
    let get = |key: &str| item.get(key).cloned().unwrap_or(Value::Null);
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .filter(|s| PLAN_STATUSES.contains(s))
        .unwrap_or("pending");
    let coverage = match item.get("coverage").and_then(Value::as_str) {
        Some(c @ ("covered" | "missing")) => json!(c),
        _ => Value::Null,
    };
    let evidence: Vec<Value> = item
        .get("evidence")
        .and_then(Value::as_array)
        .map(|e| {
            e.iter()
                .filter(|n| whole_number(Some(n)).is_some())
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut out = json!({
        "id": get("id"),
        "text": get("text"),
        "status": status,
        "need": item.get("need") == Some(&Value::Bool(true)),
        "coverage": coverage,
        "evidence": evidence,
    });
    if status == "blocked" {
        if let Some(reason) = item
            .get("reason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            if let Value::Object(o) = &mut out {
                o.insert("reason".into(), json!(reason));
            }
        }
    }
    out
}

/// Memory ids: non-empty strings, once each, at most [`MAX_MEMORY_IDS`] (`memoryIdsOf`).
fn memory_ids_of(value: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in value.and_then(Value::as_array).into_iter().flatten() {
        if let Some(id) = non_empty(Some(id)) {
            if !out.iter().any(|seen| seen == id) {
                out.push(id.to_string());
            }
        }
    }
    out.truncate(MAX_MEMORY_IDS);
    out
}

/// A provider failure from a `run_finished` event (`readProviderError`); null when absent
/// or malformed.
fn read_provider_error(value: Option<&Value>) -> Value {
    let Some(Value::Object(v)) = value else {
        return Value::Null;
    };
    let Some(kind) = v
        .get("kind")
        .and_then(Value::as_str)
        .filter(|k| ERROR_KINDS.contains(k))
    else {
        return Value::Null;
    };
    let whole = |key: &str| -> Value {
        v.get(key)
            .filter(|x| whole_number(Some(x)).is_some_and(|n| n >= 0.0))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let mut out = Map::new();
    out.insert("kind".into(), json!(kind));
    out.insert("status".into(), whole("status"));
    out.insert("retryAfterSecs".into(), whole("retryAfterSecs"));
    let upstream = v
        .get("upstream")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if !upstream.is_empty() && utf16_len(upstream) <= MAX_UPSTREAM_CHARS {
        out.insert("upstream".into(), json!(upstream));
    }
    let resets = whole("resetsAtMs");
    if !resets.is_null() {
        out.insert("resetsAtMs".into(), resets);
    }
    Value::Object(out)
}

fn start_command(
    command_id: Value,
    command: Value,
    at_ms: Value,
    time_limit_secs: Value,
    sandbox: Value,
) -> Value {
    json!({
        "commandId": command_id,
        "command": command,
        "state": "running",
        "startedAtMs": at_ms,
        "timeLimitSecs": time_limit_secs,
        "sandbox": sandbox,
        "exitCode": null,
        "note": null,
        "durationMs": null,
        "output": "",
        "outputCut": false,
    })
}

/// The last `max` UTF-16 units of `text`, never starting inside a surrogate pair (`tail`).
fn tail(text: &str, max: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max {
        return text.to_string();
    }
    let mut start = units.len() - max;
    if (0xdc00..=0xdfff).contains(&units[start]) {
        start += 1;
    }
    String::from_utf16_lossy(&units[start..])
}

fn append_command_output(command: &mut Map<String, Value>, text: &str) {
    if text.is_empty() {
        return;
    }
    let combined = format!("{}{}", str_field(command, "output"), text);
    let output = tail(&combined, MAX_COMMAND_OUTPUT_CHARS);
    let cut = truthy(command.get("outputCut")) || utf16_len(&output) < utf16_len(&combined);
    command.insert("output".into(), json!(output));
    command.insert("outputCut".into(), json!(cut));
}

fn trim_command_output(command: &mut Map<String, Value>, max: usize) {
    let output = str_field(command, "output");
    if utf16_len(output) <= max {
        return;
    }
    let kept = tail(output, max);
    command.insert("output".into(), json!(kept));
    command.insert("outputCut".into(), json!(true));
}

/// The command as its end was reported (`finishCommandRun`).
fn finish_command(command: &mut Map<String, Value>, end: &Map<String, Value>) {
    command.insert("state".into(), field(end, "state"));
    command.insert("exitCode".into(), field(end, "exitCode"));
    command.insert("note".into(), field(end, "note"));
    command.insert(
        "durationMs".into(),
        js_number(number(end.get("durationMs")).max(0.0)),
    );
}

/// The run ended at `at_ms`: steps still open fail with `summary` (`closeOpenSteps`), and
/// commands still running stop (`stopRunningCommands`).
fn end_steps(s: &mut Map<String, Value>, summary: &str, at_ms: f64) {
    for step in object_mut(s, "steps").values_mut() {
        let Value::Object(step) = step else { continue };
        let status = step.get("status").and_then(Value::as_str).unwrap_or("");
        if status == "running" || status == "awaiting_approval" {
            step.insert("status".into(), json!("failed"));
            if step.get("summary").is_none_or(Value::is_null) {
                step.insert("summary".into(), json!(summary));
            }
            if step.get("durationMs").is_none_or(Value::is_null) {
                let started = number(step.get("startedAtMs"));
                step.insert("durationMs".into(), js_number((at_ms - started).max(0.0)));
            }
            if let Some(Value::Object(approval)) = step.get_mut("approval") {
                if approval.get("decision").and_then(Value::as_str) == Some("pending") {
                    approval.insert("decision".into(), json!("denied"));
                }
            }
        }
        if !truthy(step.get("command")) {
            continue;
        }
        if let Some(Value::Object(command)) = step.get_mut("command") {
            if command.get("state").and_then(Value::as_str) == Some("running") {
                let started = number(command.get("startedAtMs"));
                command.insert("state".into(), json!("stopped"));
                command.insert("note".into(), json!(ENDED_WITH_ANSWER_NOTE));
                command.insert("durationMs".into(), js_number((at_ms - started).max(0.0)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures")
            .join("transcripts")
    }

    /// Equality as JavaScript sees it: numbers by value (`1` and `1.0` are one number),
    /// objects by their keys whatever their order.
    fn js_equal(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
            (Value::Array(x), Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(a, b)| js_equal(a, b))
            }
            (Value::Object(x), Value::Object(y)) => {
                x.len() == y.len()
                    && x.iter()
                        .all(|(k, v)| y.get(k).is_some_and(|w| js_equal(v, w)))
            }
            _ => a == b,
        }
    }

    /// An action of a fixture as the backend folds it: an event goes through the real
    /// [`AgentEvent`] type (so a fixture can only hold events the harness can send) and
    /// back to the JSON the window receives.
    fn backend_action(action: &Value) -> Value {
        let kind = action.get("type").and_then(Value::as_str).unwrap_or("");
        if kind.starts_with("local_") {
            return action.clone();
        }
        let event: AgentEvent = serde_json::from_value(action.clone())
            .unwrap_or_else(|e| panic!("not an AgentEvent ({e}): {action}"));
        event_value(&event)
    }

    #[test]
    fn fixtures_reduce_to_the_saved_transcript() {
        let mut names: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
            .expect("the transcript fixtures are in app/tests/fixtures/transcripts")
            .map(|e| e.expect("a fixture entry").path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        names.sort();
        assert!(names.len() >= 5, "every fixture is read: {names:?}");
        for path in names {
            let name = path.display().to_string();
            let text = std::fs::read_to_string(&path).expect("a readable fixture");
            let fixture: Value = serde_json::from_str(&text).expect("a JSON fixture");
            let actions: Vec<Value> = fixture["actions"]
                .as_array()
                .expect("actions")
                .iter()
                .map(backend_action)
                .collect();
            let mut transcript = Transcript::new(&fixture["seed"]);
            for action in &actions {
                transcript.apply(action);
            }
            let actual = transcript.persisted();
            assert!(
                js_equal(&actual, &fixture["expected"]),
                "{name}: the transcript differs\nactual:   {}\nexpected: {}",
                serde_json::to_string(&actual).unwrap_or_default(),
                serde_json::to_string(&fixture["expected"]).unwrap_or_default(),
            );
            assert_eq!(
                transcript.answer_text(),
                fixture["expectedText"].as_str().expect("expectedText"),
                "{name}: the answer text differs"
            );
            assert!(
                js_equal(&reduce_transcript(&fixture["seed"], &actions), &actual),
                "{name}: reduce_transcript is the same fold"
            );
            if let Some(expected) = fixture.get("expectedCheckpoint") {
                // The checkpoint is taken where the fixture's check started.
                let at = fixture["checkpointAfter"]
                    .as_u64()
                    .expect("checkpointAfter") as usize;
                let mut partial = Transcript::new(&fixture["seed"]);
                for action in &actions[..at] {
                    partial.apply(action);
                }
                assert!(
                    partial.starts_check(&actions[at - 1]),
                    "{name}: the check starts there"
                );
                let checkpoint = partial.checkpoint();
                assert!(
                    js_equal(&checkpoint, expected),
                    "{name}: the checkpoint differs\nactual:   {}",
                    serde_json::to_string(&checkpoint).unwrap_or_default(),
                );
            }
        }
    }

    #[test]
    fn events_of_another_run_and_after_the_end_change_nothing() {
        let seed = json!({ "runId": "run-a", "startedAtMs": 1000 });
        let mut t = Transcript::new(&seed);
        let before = t.clone();
        t.apply(&json!({ "type": "text_delta", "runId": "run-b", "messageId": "m", "delta": "x" }));
        assert_eq!(t, before);
        t.apply(&json!({ "type": "run_finished", "runId": "run-a", "status": "completed", "durationMs": 5, "error": null, "providerError": null }));
        let finished = t.clone();
        t.apply(
            &json!({ "type": "text_delta", "runId": "run-a", "messageId": "m", "delta": "late" }),
        );
        assert_eq!(t, finished);
        // Late usage totals are still taken.
        t.apply(&json!({ "type": "usage", "runId": "run-a", "inputTokens": 1, "outputTokens": 2, "cacheReadTokens": 0, "costUsd": 0.5 }));
        assert_eq!(t.persisted()["usage"]["outputTokens"], json!(2));
    }

    #[test]
    fn command_output_keeps_its_tail_in_utf16_units() {
        let mut command = Map::new();
        command.insert("output".into(), json!(""));
        command.insert("outputCut".into(), json!(false));
        let long = "a".repeat(MAX_COMMAND_OUTPUT_CHARS + 10);
        append_command_output(&mut command, &long);
        assert_eq!(
            utf16_len(str_field(&command, "output")),
            MAX_COMMAND_OUTPUT_CHARS
        );
        assert_eq!(command["outputCut"], json!(true));
        // A tail never starts inside a surrogate pair.
        let emoji = format!("x{}", "\u{1F600}".repeat(3));
        assert_eq!(tail(&emoji, 3), "\u{1F600}");
    }
}
