//! The `AgentEvent` contract shared by the Rust harness and the React transcript.
//!
//! Every event is serialised as a JSON object with a `type` discriminator in
//! snake_case and camelCase field names. Optional fields are always present
//! (`null` when absent) so the TypeScript mirror in
//! `app/src/features/agent/events.ts` can rely on every key existing.

use serde::{Deserialize, Serialize};

/// Risk tier of a host tool. Decides whether a call needs user approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskTier {
    /// Runs without approval.
    Read,
    /// Needs approval unless the profile allows writes.
    Write,
    /// Always needs approval.
    Destructive,
}

/// Terminal state of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Aborted,
    Error,
}

/// State of a single task-list item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Done,
}

/// One item of the run's task list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: String,
    pub text: String,
    pub status: PlanStatus,
}

/// Where a `navigated` event points inside a view. Serialised with a `kind`
/// discriminator in snake_case and camelCase fields; optional fields are
/// always present (`null` when absent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum NavigationTarget {
    /// Open a document in the viewer, optionally at a page and passage.
    Document {
        path: String,
        page: Option<u32>,
        passage: Option<String>,
    },
    /// Show a calendar date, task or event.
    Calendar {
        date: Option<String>,
        task_id: Option<String>,
        event_id: Option<String>,
    },
    /// Open a saved conversation.
    Conversation { conversation_id: String },
    /// Show the audit log filtered to these event types, tool, range and text.
    Audit {
        types: Vec<String>,
        tool: Option<String>,
        from: Option<String>,
        to: Option<String>,
        text: Option<String>,
    },
    /// Show an indexed folder source in the Library.
    Source { source_id: String },
}

/// A normalised agent event. Produced from omp frames and from host-tool
/// execution, consumed by the transcript UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    RunStarted {
        run_id: String,
        session_id: String,
        model: String,
        at_ms: u64,
        /// Shown to the user for the whole run, e.g. a data-handling warning
        /// about the selected model.
        warning: Option<String>,
    },
    TextDelta {
        run_id: String,
        message_id: String,
        delta: String,
    },
    Thinking {
        run_id: String,
        message_id: String,
        delta: String,
    },
    StepStarted {
        run_id: String,
        step_id: String,
        parent_step_id: Option<String>,
        tool: String,
        label: String,
        args: serde_json::Value,
        tier: RiskTier,
        at_ms: u64,
    },
    StepProgress {
        run_id: String,
        step_id: String,
        text: String,
    },
    StepFinished {
        run_id: String,
        step_id: String,
        ok: bool,
        summary: String,
        detail: Option<serde_json::Value>,
        duration_ms: u64,
    },
    ApprovalRequested {
        run_id: String,
        step_id: String,
        tool: String,
        label: String,
        tier: RiskTier,
        preview: serde_json::Value,
    },
    PlanUpdated {
        run_id: String,
        items: Vec<PlanItem>,
    },
    Navigated {
        run_id: String,
        view: String,
        focus: Option<String>,
        /// What to show inside the view. Absent in events from older
        /// builds, which only switched the view.
        #[serde(default)]
        target: Option<NavigationTarget>,
    },
    Usage {
        run_id: String,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cost_usd: f64,
    },
    RunFinished {
        run_id: String,
        status: RunStatus,
        duration_ms: u64,
        error: Option<String>,
    },
}

impl AgentEvent {
    /// The run this event belongs to.
    pub fn run_id(&self) -> &str {
        match self {
            AgentEvent::RunStarted { run_id, .. }
            | AgentEvent::TextDelta { run_id, .. }
            | AgentEvent::Thinking { run_id, .. }
            | AgentEvent::StepStarted { run_id, .. }
            | AgentEvent::StepProgress { run_id, .. }
            | AgentEvent::StepFinished { run_id, .. }
            | AgentEvent::ApprovalRequested { run_id, .. }
            | AgentEvent::PlanUpdated { run_id, .. }
            | AgentEvent::Navigated { run_id, .. }
            | AgentEvent::Usage { run_id, .. }
            | AgentEvent::RunFinished { run_id, .. } => run_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// The TypeScript mirror. Every key pinned below must appear in it.
    const TS_CONTRACT: &str = include_str!("../../../../app/src/features/agent/events.ts");

    fn samples() -> Vec<(AgentEvent, &'static str, Vec<&'static str>)> {
        vec![
            (
                AgentEvent::RunStarted {
                    run_id: "r".into(),
                    session_id: "s".into(),
                    model: "anthropic/claude".into(),
                    at_ms: 1,
                    warning: Some("careful".into()),
                },
                "run_started",
                vec!["runId", "sessionId", "model", "atMs", "warning"],
            ),
            (
                AgentEvent::TextDelta {
                    run_id: "r".into(),
                    message_id: "m".into(),
                    delta: "hi".into(),
                },
                "text_delta",
                vec!["runId", "messageId", "delta"],
            ),
            (
                AgentEvent::Thinking {
                    run_id: "r".into(),
                    message_id: "m".into(),
                    delta: "hmm".into(),
                },
                "thinking",
                vec!["runId", "messageId", "delta"],
            ),
            (
                AgentEvent::StepStarted {
                    run_id: "r".into(),
                    step_id: "st".into(),
                    parent_step_id: None,
                    tool: "search_documents".into(),
                    label: "Searching".into(),
                    args: json!({"query": "x"}),
                    tier: RiskTier::Read,
                    at_ms: 2,
                },
                "step_started",
                vec![
                    "runId",
                    "stepId",
                    "parentStepId",
                    "tool",
                    "label",
                    "args",
                    "tier",
                    "atMs",
                ],
            ),
            (
                AgentEvent::StepProgress {
                    run_id: "r".into(),
                    step_id: "st".into(),
                    text: "working".into(),
                },
                "step_progress",
                vec!["runId", "stepId", "text"],
            ),
            (
                AgentEvent::StepFinished {
                    run_id: "r".into(),
                    step_id: "st".into(),
                    ok: true,
                    summary: "3 passages".into(),
                    detail: Some(json!({"n": 3})),
                    duration_ms: 12,
                },
                "step_finished",
                vec!["runId", "stepId", "ok", "summary", "detail", "durationMs"],
            ),
            (
                AgentEvent::ApprovalRequested {
                    run_id: "r".into(),
                    step_id: "st".into(),
                    tool: "create_task".into(),
                    label: "Creating task".into(),
                    tier: RiskTier::Write,
                    preview: json!({"title": "Pay invoice"}),
                },
                "approval_requested",
                vec!["runId", "stepId", "tool", "label", "tier", "preview"],
            ),
            (
                AgentEvent::PlanUpdated {
                    run_id: "r".into(),
                    items: vec![PlanItem {
                        id: "1".into(),
                        text: "Search".into(),
                        status: PlanStatus::InProgress,
                    }],
                },
                "plan_updated",
                vec!["runId", "items", "id", "text", "status"],
            ),
            (
                AgentEvent::Navigated {
                    run_id: "r".into(),
                    view: "calendar".into(),
                    focus: Some("task-1".into()),
                    target: Some(NavigationTarget::Calendar {
                        date: Some("2026-10-30".into()),
                        task_id: Some("task-1".into()),
                        event_id: None,
                    }),
                },
                "navigated",
                vec!["runId", "view", "focus", "target"],
            ),
            (
                AgentEvent::Usage {
                    run_id: "r".into(),
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_read_tokens: 2,
                    cost_usd: 0.01,
                },
                "usage",
                vec![
                    "runId",
                    "inputTokens",
                    "outputTokens",
                    "cacheReadTokens",
                    "costUsd",
                ],
            ),
            (
                AgentEvent::RunFinished {
                    run_id: "r".into(),
                    status: RunStatus::Completed,
                    duration_ms: 100,
                    error: None,
                },
                "run_finished",
                vec!["runId", "status", "durationMs", "error"],
            ),
        ]
    }

    #[test]
    fn every_variant_round_trips_with_pinned_keys() {
        for (event, tag, keys) in samples() {
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(value["type"], Value::String(tag.to_string()), "{tag}");
            let object = value.as_object().unwrap();
            for key in &keys {
                let present = object.contains_key(*key)
                    || object.values().any(|v| {
                        v.as_array()
                            .map(|items| items.iter().any(|i| i.get(*key).is_some()))
                            .unwrap_or(false)
                    });
                assert!(present, "{tag} is missing key {key}: {value}");
            }
            let back: AgentEvent = serde_json::from_value(value).unwrap();
            assert_eq!(back, event);
        }
    }

    fn targets() -> Vec<(NavigationTarget, &'static str, Vec<&'static str>)> {
        vec![
            (
                NavigationTarget::Document {
                    path: "c:/docs/a.pdf".into(),
                    page: Some(4),
                    passage: Some("notice period".into()),
                },
                "document",
                vec!["path", "page", "passage"],
            ),
            (
                NavigationTarget::Calendar {
                    date: Some("2026-10-30".into()),
                    task_id: None,
                    event_id: Some("e1".into()),
                },
                "calendar",
                vec!["date", "taskId", "eventId"],
            ),
            (
                NavigationTarget::Conversation {
                    conversation_id: "c1".into(),
                },
                "conversation",
                vec!["conversationId"],
            ),
            (
                NavigationTarget::Audit {
                    types: vec!["tool_call".into()],
                    tool: Some("web_search".into()),
                    from: None,
                    to: None,
                    text: None,
                },
                "audit",
                vec!["types", "tool", "from", "to", "text"],
            ),
            (
                NavigationTarget::Source {
                    source_id: "s1".into(),
                },
                "source",
                vec!["sourceId"],
            ),
        ]
    }

    #[test]
    fn navigation_targets_round_trip_with_pinned_keys() {
        for (target, kind, keys) in targets() {
            let value = serde_json::to_value(&target).unwrap();
            assert_eq!(value["kind"], Value::String(kind.to_string()));
            for key in keys {
                assert!(value.get(key).is_some(), "{kind} is missing {key}: {value}");
            }
            let back: NavigationTarget = serde_json::from_value(value).unwrap();
            assert_eq!(back, target);
        }
    }

    #[test]
    fn navigated_without_a_target_still_parses() {
        let old = json!({"type": "navigated", "runId": "r", "view": "library", "focus": null});
        let event: AgentEvent = serde_json::from_value(old).unwrap();
        assert_eq!(
            event,
            AgentEvent::Navigated {
                run_id: "r".into(),
                view: "library".into(),
                focus: None,
                target: None,
            }
        );
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["target"], Value::Null);
    }

    #[test]
    fn optional_fields_serialise_as_null() {
        let event = AgentEvent::RunFinished {
            run_id: "r".into(),
            status: RunStatus::Aborted,
            duration_ms: 1,
            error: None,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["error"], Value::Null);
        assert_eq!(value["status"], json!("aborted"));
    }

    #[test]
    fn enums_use_snake_case() {
        assert_eq!(
            serde_json::to_value(RiskTier::Destructive).unwrap(),
            json!("destructive")
        );
        assert_eq!(
            serde_json::to_value(PlanStatus::InProgress).unwrap(),
            json!("in_progress")
        );
    }

    #[test]
    fn typescript_mirror_declares_every_tag_and_key() {
        for (_, tag, keys) in samples() {
            assert!(
                TS_CONTRACT.contains(&format!("type: \"{tag}\"")),
                "events.ts is missing variant {tag}"
            );
            for key in keys {
                assert!(
                    TS_CONTRACT.contains(&format!("{key}:")),
                    "events.ts is missing key {key} (variant {tag})"
                );
            }
        }
        for (_, kind, keys) in targets() {
            assert!(
                TS_CONTRACT.contains(&format!("kind: \"{kind}\"")),
                "events.ts is missing navigation target {kind}"
            );
            for key in keys {
                assert!(
                    TS_CONTRACT.contains(&format!("{key}:")),
                    "events.ts is missing key {key} (target {kind})"
                );
            }
        }
        for literal in [
            "\"read\"",
            "\"write\"",
            "\"destructive\"",
            "\"completed\"",
            "\"aborted\"",
            "\"error\"",
            "\"pending\"",
            "\"in_progress\"",
            "\"done\"",
        ] {
            assert!(
                TS_CONTRACT.contains(literal),
                "events.ts is missing {literal}"
            );
        }
    }
}
