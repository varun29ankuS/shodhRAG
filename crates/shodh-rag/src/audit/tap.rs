//! Builds the `answer` event of each run from the session's event stream:
//! status, duration, model, token usage and cost, and which numbered
//! passages the answer cites (`[n]` → file and page).
//!
//! Usage totals are cumulative and are only emitted while the run is active,
//! so they are final when `RunFinished` arrives.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use super::payload::is_cloud;
use crate::harness::events::{AgentEvent, RunStatus};

/// Answer text kept per run for citation extraction.
const MAX_ANSWER_CHARS: usize = 400_000;

#[derive(Debug, Clone)]
struct CitedPassage {
    file: Value,
    path: Value,
    page: Value,
}

#[derive(Debug, Default)]
struct RunState {
    model: String,
    text: String,
    passages: HashMap<u64, CitedPassage>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cost_usd: f64,
    tool_steps: u64,
}

/// Per-session accumulator. Feed it every event in order.
#[derive(Debug, Default)]
pub struct RunAuditTap {
    runs: HashMap<String, RunState>,
}

/// A finished run's `answer` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct AnswerAudit {
    pub run_id: String,
    pub payload: Value,
}

impl RunAuditTap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one event; returns the `answer` payload when a run finishes.
    pub fn observe(&mut self, event: &AgentEvent) -> Option<AnswerAudit> {
        match event {
            AgentEvent::RunStarted { run_id, model, .. } => {
                self.runs.insert(
                    run_id.clone(),
                    RunState {
                        model: model.clone(),
                        ..RunState::default()
                    },
                );
                None
            }
            AgentEvent::TextDelta { run_id, delta, .. } => {
                if let Some(run) = self.runs.get_mut(run_id) {
                    if run.text.len() + delta.len() <= MAX_ANSWER_CHARS {
                        run.text.push_str(delta);
                    }
                }
                None
            }
            AgentEvent::StepFinished { run_id, detail, .. } => {
                if let Some(run) = self.runs.get_mut(run_id) {
                    run.tool_steps += 1;
                    if let Some(passages) = detail
                        .as_ref()
                        .and_then(|d| d.get("passages"))
                        .and_then(Value::as_array)
                    {
                        for p in passages {
                            if let Some(n) = p.get("n").and_then(Value::as_u64) {
                                run.passages.insert(
                                    n,
                                    CitedPassage {
                                        file: p.get("file").cloned().unwrap_or(Value::Null),
                                        path: p.get("path").cloned().unwrap_or(Value::Null),
                                        page: p.get("page").cloned().unwrap_or(Value::Null),
                                    },
                                );
                            }
                        }
                    }
                }
                None
            }
            AgentEvent::Usage {
                run_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cost_usd,
            } => {
                if let Some(run) = self.runs.get_mut(run_id) {
                    run.input_tokens = *input_tokens;
                    run.output_tokens = *output_tokens;
                    run.cache_read_tokens = *cache_read_tokens;
                    run.cost_usd = *cost_usd;
                }
                None
            }
            AgentEvent::RunFinished {
                run_id,
                status,
                duration_ms,
                error,
            } => {
                let run = self.runs.remove(run_id).unwrap_or_default();
                let cited = cited_numbers(&run.text);
                let citations: Vec<Value> = cited
                    .iter()
                    .map(|n| match run.passages.get(n) {
                        Some(p) => json!({"n": n, "file": p.file, "path": p.path, "page": p.page}),
                        None => json!({"n": n, "file": null, "path": null, "page": null}),
                    })
                    .collect();
                let status = match status {
                    RunStatus::Completed => "completed",
                    RunStatus::Aborted => "aborted",
                    RunStatus::Error => "error",
                };
                let (provider, _) = run
                    .model
                    .split_once('/')
                    .unwrap_or((run.model.as_str(), ""));
                Some(AnswerAudit {
                    run_id: run_id.clone(),
                    payload: json!({
                        "status": status,
                        "error": error,
                        "duration_ms": duration_ms,
                        "model": run.model,
                        "provider": provider,
                        "cloud": is_cloud(&run.model),
                        "tokens_in": run.input_tokens,
                        "tokens_out": run.output_tokens,
                        "cache_read_tokens": run.cache_read_tokens,
                        "cost_usd": run.cost_usd,
                        "tool_steps": run.tool_steps,
                        "answer_chars": run.text.chars().count(),
                        "citations": citations,
                    }),
                })
            }
            AgentEvent::Thinking { .. }
            | AgentEvent::StepStarted { .. }
            | AgentEvent::StepProgress { .. }
            | AgentEvent::ApprovalRequested { .. }
            | AgentEvent::PlanUpdated { .. }
            | AgentEvent::Navigated { .. }
            | AgentEvent::Grounding { .. }
            | AgentEvent::RevisionStarted { .. } => None,
        }
    }
}

/// Compile a literal pattern once. The patterns are constants covered by the
/// unit tests; a failure disables that pattern instead of panicking.
fn cached(cell: &'static OnceLock<Option<Regex>>, pattern: &str) -> Option<&'static Regex> {
    cell.get_or_init(|| Regex::new(pattern).ok()).as_ref()
}

static FENCED_CODE: OnceLock<Option<Regex>> = OnceLock::new();
static BRACKET_CITATIONS: OnceLock<Option<Regex>> = OnceLock::new();
static LENTICULAR_CITATIONS: OnceLock<Option<Regex>> = OnceLock::new();

/// Citation numbers in `text`, as the transcript renders them: `[n]`,
/// `[n, m]`, `[Document n]` and `【n†…】`, outside fenced code blocks.
pub fn cited_numbers(text: &str) -> BTreeSet<u64> {
    let prose = match cached(&FENCED_CODE, r"(?s)```.*?```") {
        Some(re) => re.replace_all(text, " ").into_owned(),
        None => text.to_string(),
    };
    let mut out = BTreeSet::new();
    if let Some(re) = cached(
        &BRACKET_CITATIONS,
        r"(?i)\[(?:Document\s+)?(\d+(?:\s*,\s*(?:Document\s+)?\d+)*)\]",
    ) {
        for caps in re.captures_iter(&prose) {
            if let Some(group) = caps.get(1) {
                for part in group.as_str().split(',') {
                    let digits: String = part.chars().filter(char::is_ascii_digit).collect();
                    if let Ok(n) = digits.parse::<u64>() {
                        out.insert(n);
                    }
                }
            }
        }
    }
    if let Some(re) = cached(&LENTICULAR_CITATIONS, r"【(\d+)†[^】]*】") {
        for caps in re.captures_iter(&prose) {
            if let Some(n) = caps.get(1).and_then(|m| m.as_str().parse::<u64>().ok()) {
                out.insert(n);
            }
        }
    }
    out
}

/// Group cited passages by file for display: path → pages.
pub fn cited_files(payload: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for c in payload
        .get("citations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(path) = c.get("path").and_then(Value::as_str) {
            let pages = out.entry(path.to_string()).or_default();
            if let Some(page) = c.get("page").and_then(Value::as_str) {
                pages.insert(page.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citation_patterns_match_the_transcript() {
        let text = "Sixty days [1]. See also [2, 3] and [Document 4][5]. 【6†source】\n\
                    ```\nlet x = a[7];\n```\nNot a cite: [x].";
        assert_eq!(
            cited_numbers(text).into_iter().collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6]
        );
    }

    fn started(run: &str) -> AgentEvent {
        AgentEvent::RunStarted {
            run_id: run.into(),
            session_id: "s".into(),
            model: "anthropic/claude-x".into(),
            at_ms: 0,
            warning: None,
        }
    }

    #[test]
    fn finished_runs_report_usage_and_cited_files() {
        let mut tap = RunAuditTap::new();
        assert!(tap.observe(&started("r1")).is_none());
        tap.observe(&AgentEvent::StepFinished {
            run_id: "r1".into(),
            step_id: "st".into(),
            ok: true,
            summary: "2 passages".into(),
            detail: Some(json!({"passages": [
                {"n": 1, "file": "a.pdf", "path": "c:/a.pdf", "page": "4", "score": 0.4, "text": "t"},
                {"n": 2, "file": "b.pdf", "path": "c:/b.pdf", "score": 0.3, "text": "u"}
            ]})),
            duration_ms: 10,
        });
        for delta in ["Notice is sixty", " days [1]."] {
            tap.observe(&AgentEvent::TextDelta {
                run_id: "r1".into(),
                message_id: "m".into(),
                delta: delta.into(),
            });
        }
        tap.observe(&AgentEvent::Usage {
            run_id: "r1".into(),
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 5,
            cost_usd: 0.01,
        });
        tap.observe(&AgentEvent::Usage {
            run_id: "r1".into(),
            input_tokens: 300,
            output_tokens: 40,
            cache_read_tokens: 5,
            cost_usd: 0.03,
        });
        let answer = tap
            .observe(&AgentEvent::RunFinished {
                run_id: "r1".into(),
                status: RunStatus::Completed,
                duration_ms: 1234,
                error: None,
            })
            .unwrap();
        assert_eq!(answer.run_id, "r1");
        let p = &answer.payload;
        assert_eq!(p["status"], "completed");
        assert_eq!(p["tokens_in"], 300, "usage totals are cumulative");
        assert_eq!(p["tokens_out"], 40);
        assert_eq!(p["cost_usd"], 0.03);
        assert_eq!(p["provider"], "anthropic");
        assert_eq!(p["cloud"], true);
        assert_eq!(p["citations"].as_array().unwrap().len(), 1);
        assert_eq!(p["citations"][0]["path"], "c:/a.pdf");
        assert_eq!(p["citations"][0]["page"], "4");
        let files = cited_files(p);
        assert_eq!(files.len(), 1);
        assert!(files["c:/a.pdf"].contains("4"));
        assert!(
            !p.to_string().contains("sixty"),
            "answer text is not stored"
        );
    }

    #[test]
    fn unknown_runs_still_produce_an_answer_event() {
        let mut tap = RunAuditTap::new();
        let answer = tap
            .observe(&AgentEvent::RunFinished {
                run_id: "r9".into(),
                status: RunStatus::Error,
                duration_ms: 0,
                error: Some("omp exited".into()),
            })
            .unwrap();
        assert_eq!(answer.payload["status"], "error");
        assert_eq!(answer.payload["error"], "omp exited");
    }
}
