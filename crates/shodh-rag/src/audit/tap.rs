//! Builds the `answer` event of each run from the session's event stream:
//! status, duration, model, token usage and cost, which numbered passages
//! the answer cites (`[n]` → file and page), and its grounding (counts and
//! score of the final check; never claim text). Text blocks a revised answer
//! replaced do not count as the answer.
//!
//! Usage totals are cumulative and are only emitted while the run is active,
//! so they are final when `RunFinished` arrives.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{json, Value};

use super::payload::is_cloud;
use crate::harness::events::{AgentEvent, CoverageState, GroundingReport, RunStatus};

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
    /// Answer text by text block, in order.
    messages: Vec<(String, String)>,
    text_chars: usize,
    /// The final grounding check, or the latest one.
    grounding: Option<GroundingReport>,
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
            AgentEvent::TextDelta {
                run_id,
                message_id,
                delta,
            } => {
                if let Some(run) = self.runs.get_mut(run_id) {
                    if run.text_chars + delta.len() <= MAX_ANSWER_CHARS {
                        run.text_chars += delta.len();
                        match run
                            .messages
                            .iter_mut()
                            .rev()
                            .find(|(id, _)| id == message_id)
                        {
                            Some((_, text)) => text.push_str(delta),
                            None => run.messages.push((message_id.clone(), delta.clone())),
                        }
                    }
                }
                None
            }
            AgentEvent::Grounding { run_id, report } => {
                if let Some(run) = self.runs.get_mut(run_id) {
                    run.messages
                        .retain(|(id, _)| !report.superseded_message_ids.contains(id));
                    run.grounding = Some(report.clone());
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
                let text: String = run
                    .messages
                    .iter()
                    .map(|(_, t)| t.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let cited = cited_numbers(&text);
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
                        "answer_chars": text.chars().count(),
                        "citations": citations,
                        "grounding": run.grounding.as_ref().map(grounding_payload),
                    }),
                })
            }
            AgentEvent::Thinking { .. }
            | AgentEvent::StepStarted { .. }
            | AgentEvent::StepProgress { .. }
            | AgentEvent::ApprovalRequested { .. }
            | AgentEvent::PlanUpdated { .. }
            | AgentEvent::Navigated { .. }
            | AgentEvent::RevisionStarted { .. } => None,
        }
    }
}

/// The audited grounding of an answer: counts, score, method and rounds.
/// The answer's text is not stored, so neither are its claims.
pub fn grounding_payload(report: &GroundingReport) -> Value {
    let s = &report.summary;
    let covered = report
        .needs
        .iter()
        .filter(|n| n.state == CoverageState::Covered)
        .count();
    json!({
        "checked": s.checked,
        "supported": s.supported,
        "weak": s.weak,
        "unsupported": s.unsupported,
        "uncited": s.uncited,
        "invalid": s.invalid,
        "unchecked": s.unchecked,
        "score": s.score,
        "method": report.method,
        "rounds": report.round,
        "final": report.is_final,
        "needs_total": report.needs.len(),
        "needs_covered": covered,
    })
}

/// Citation numbers in `text`, as the transcript renders them (the grammar
/// of [`crate::harness::grounding::citations`]), outside code.
pub fn cited_numbers(text: &str) -> BTreeSet<u64> {
    crate::harness::grounding::citations::cited_numbers(text)
        .into_iter()
        .map(u64::from)
        .collect()
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
        let text = "Sixty days [1]. See also [2, 3] and [Document 4][5]. 【6†source】 [7-8]\n\
                    ```\nlet x = a[9];\n```\nNot a cite: [x] or the shape [4, 9, 1].";
        assert_eq!(
            cited_numbers(text).into_iter().collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn revised_answers_audit_only_the_final_text_and_its_grounding() {
        use crate::harness::events::{
            ClaimCheck, ClaimKind, ClaimOutcome, GroundingSummary, ScoringMethod,
        };
        let mut tap = RunAuditTap::new();
        tap.observe(&started("r1"));
        for (message, delta) in [
            ("m1", "Fee is 900 EUR [1]."),
            ("m2", "Fee is 1,200 EUR [2]."),
        ] {
            tap.observe(&AgentEvent::TextDelta {
                run_id: "r1".into(),
                message_id: message.into(),
                delta: delta.into(),
            });
        }
        let report = GroundingReport {
            round: 1,
            is_final: true,
            method: ScoringMethod::Entailment,
            summary: GroundingSummary {
                checked: 1,
                supported: 1,
                score: Some(1.0),
                ..GroundingSummary::default()
            },
            claims: vec![ClaimCheck {
                message_id: "m2".into(),
                text: "Fee is 1,200 EUR.".into(),
                anchor: "Fee is 1,200 EUR [2].".into(),
                kind: ClaimKind::Sentence,
                outcome: ClaimOutcome::Supported,
                cited: vec![2],
                invalid: vec![],
                support: Some(0.9),
                missing_numbers: vec![],
                closest: None,
                closest_score: None,
            }],
            needs: vec![],
            message_ids: vec!["m2".into()],
            superseded_message_ids: vec!["m1".into()],
        };
        tap.observe(&AgentEvent::Grounding {
            run_id: "r1".into(),
            report,
        });
        let answer = tap
            .observe(&AgentEvent::RunFinished {
                run_id: "r1".into(),
                status: RunStatus::Completed,
                duration_ms: 10,
                error: None,
            })
            .unwrap();
        let p = &answer.payload;
        assert_eq!(p["citations"].as_array().unwrap().len(), 1);
        assert_eq!(
            p["citations"][0]["n"], 2,
            "the superseded draft's [1] is not the answer's"
        );
        assert_eq!(p["grounding"]["supported"], 1);
        assert_eq!(p["grounding"]["score"], 1.0);
        assert_eq!(p["grounding"]["method"], "entailment");
        assert_eq!(p["grounding"]["rounds"], 1);
        assert!(!p.to_string().contains("1,200"), "claim text is not stored");
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
