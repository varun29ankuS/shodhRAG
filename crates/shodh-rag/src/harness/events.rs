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

/// Whether retrieved passages cover an information need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageState {
    Covered,
    Missing,
}

/// One item of the run's task list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: String,
    pub text: String,
    pub status: PlanStatus,
    /// The item is an information need of the question (a part the answer
    /// must find in the sources), checked against the retrieved passages.
    /// Absent in events from older builds.
    #[serde(default)]
    pub need: bool,
    /// For a need: whether a retrieved passage covers it, once checked.
    #[serde(default)]
    pub coverage: Option<CoverageState>,
    /// For a covered need: the passages that cover it, best first.
    #[serde(default)]
    pub evidence: Vec<u32>,
}

impl PlanItem {
    /// A plain task-list item.
    pub fn task(id: impl Into<String>, text: impl Into<String>, status: PlanStatus) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            status,
            need: false,
            coverage: None,
            evidence: Vec::new(),
        }
    }
}

/// How one claim of an answer relates to its sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimOutcome {
    /// The cited passages support it.
    Supported,
    /// The cited passages are on topic but support it only in part.
    Weak,
    /// The cited passages do not support it, or lack a number it states.
    Unsupported,
    /// A factual statement without a citation, in an answer built from sources.
    UncitedFactual,
    /// It cites a number that no source of this answer has.
    InvalidCitation,
    /// Its only sources cannot be checked (a search provider's answer fragments).
    Unchecked,
}

/// How support was scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoringMethod {
    /// The local entailment (NLI) model on the cited text, plus number checks.
    Entailment,
    /// The local cross-encoder (relevance only, no entailment model
    /// installed), plus number checks.
    CrossEncoder,
    /// Word overlap and number checks only (the model is not installed).
    Lexical,
}

/// What a claim was written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    Sentence,
    ListItem,
    TableRow,
}

/// The verdict on one claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimCheck {
    /// The text block (assistant message) the claim is in.
    pub message_id: String,
    /// The claim as plain text.
    pub text: String,
    /// Exact text of the message after which the claim's flag goes.
    pub anchor: String,
    pub kind: ClaimKind,
    pub outcome: ClaimOutcome,
    /// Passage numbers it cites.
    pub cited: Vec<u32>,
    /// Cited numbers that no passage of this answer has.
    pub invalid: Vec<u32>,
    /// Best support score of the cited passages, 0..=1.
    pub support: Option<f32>,
    /// Numbers it states that its cited passages do not contain.
    pub missing_numbers: Vec<String>,
    /// For a flagged claim: the passage that comes closest to supporting it.
    pub closest: Option<u32>,
    pub closest_score: Option<f32>,
}

/// Counts over the checked claims of one answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingSummary {
    /// Claims checked: every cited claim and every uncited factual one.
    pub checked: u32,
    pub supported: u32,
    pub weak: u32,
    pub unsupported: u32,
    pub uncited: u32,
    pub invalid: u32,
    pub unchecked: u32,
    /// (supported + weak / 2) / (checked − unchecked), 0..=1; `None` when
    /// nothing checkable was claimed.
    pub score: Option<f32>,
}

/// Coverage of one information need of the question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NeedCheck {
    /// Task-list item id.
    pub id: String,
    pub text: String,
    pub state: CoverageState,
    /// Passages that cover it, best first.
    pub passages: Vec<u32>,
}

/// The grounding check of an answer after one round of answering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingReport {
    /// 0 for the first answer, then one per follow-up turn.
    pub round: u32,
    /// The last check of the run: this is the answer's grounding.
    pub is_final: bool,
    pub method: ScoringMethod,
    pub summary: GroundingSummary,
    /// Checked claims, in reading order.
    pub claims: Vec<ClaimCheck>,
    pub needs: Vec<NeedCheck>,
    /// Text blocks (message ids) the claims come from.
    pub message_ids: Vec<String>,
    /// Text blocks replaced by a revised answer (kept in the transcript as
    /// an earlier draft).
    pub superseded_message_ids: Vec<String>,
}

/// Why the harness asked the model for a follow-up turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionReason {
    /// Re-ground or remove flagged claims.
    Repair,
    /// Search for parts of the question that no passage covers.
    Coverage,
    /// Both.
    RepairAndCoverage,
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
    /// Open a generated visual (gallery) in the focus pop-out, at a version (the latest
    /// when absent).
    Visual {
        visual_id: String,
        version: Option<u32>,
    },
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
    /// The grounding check of the answer so far (after each round of
    /// answering; `report.is_final` on the last one, before `RunFinished`).
    Grounding {
        run_id: String,
        report: GroundingReport,
    },
    /// The harness asked the model for one more turn: to re-ground flagged
    /// claims and/or to search for uncovered parts of the question.
    RevisionStarted {
        run_id: String,
        round: u32,
        reason: RevisionReason,
        /// Flagged claims the model was asked to fix.
        flagged: u32,
        /// Information needs still without a covering passage.
        missing_needs: Vec<String>,
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
            | AgentEvent::Grounding { run_id, .. }
            | AgentEvent::RevisionStarted { run_id, .. }
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
                        text: "Find the notice period".into(),
                        status: PlanStatus::InProgress,
                        need: true,
                        coverage: Some(CoverageState::Covered),
                        evidence: vec![3],
                    }],
                },
                "plan_updated",
                vec![
                    "runId", "items", "id", "text", "status", "need", "coverage", "evidence",
                ],
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
                AgentEvent::Grounding {
                    run_id: "r".into(),
                    report: sample_report(),
                },
                "grounding",
                vec!["runId", "report"],
            ),
            (
                AgentEvent::RevisionStarted {
                    run_id: "r".into(),
                    round: 1,
                    reason: RevisionReason::RepairAndCoverage,
                    flagged: 2,
                    missing_needs: vec!["Renewal fee".into()],
                },
                "revision_started",
                vec!["runId", "round", "reason", "flagged", "missingNeeds"],
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

    fn sample_report() -> GroundingReport {
        GroundingReport {
            round: 0,
            is_final: true,
            method: ScoringMethod::Entailment,
            summary: GroundingSummary {
                checked: 2,
                supported: 1,
                weak: 0,
                unsupported: 0,
                uncited: 0,
                invalid: 1,
                unchecked: 0,
                score: Some(0.5),
            },
            claims: vec![ClaimCheck {
                message_id: "m1".into(),
                text: "The fee is 90 EUR.".into(),
                anchor: "The fee is 90 EUR [7].".into(),
                kind: ClaimKind::Sentence,
                outcome: ClaimOutcome::InvalidCitation,
                cited: vec![7],
                invalid: vec![7],
                support: None,
                missing_numbers: vec!["90".into()],
                closest: Some(2),
                closest_score: Some(0.41),
            }],
            needs: vec![NeedCheck {
                id: "2".into(),
                text: "Renewal fee".into(),
                state: CoverageState::Missing,
                passages: vec![],
            }],
            message_ids: vec!["m1".into()],
            superseded_message_ids: vec![],
        }
    }

    /// Keys of the grounding report, at every level, as the TS mirror spells them.
    const REPORT_KEYS: [&str; 27] = [
        "round",
        "isFinal",
        "method",
        "summary",
        "claims",
        "needs",
        "messageIds",
        "supersededMessageIds",
        "checked",
        "supported",
        "weak",
        "unsupported",
        "uncited",
        "invalid",
        "unchecked",
        "score",
        "messageId",
        "anchor",
        "kind",
        "outcome",
        "cited",
        "support",
        "missingNumbers",
        "closest",
        "closestScore",
        "passages",
        "state",
    ];

    fn collect_keys(value: &Value, out: &mut std::collections::BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    out.insert(k.clone());
                    collect_keys(v, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|i| collect_keys(i, out)),
            _ => {}
        }
    }

    #[test]
    fn grounding_report_serialises_every_pinned_key_and_round_trips() {
        let value = serde_json::to_value(sample_report()).unwrap();
        let mut keys = std::collections::BTreeSet::new();
        collect_keys(&value, &mut keys);
        for key in REPORT_KEYS {
            assert!(keys.contains(key), "report is missing {key}: {value}");
            assert!(
                TS_CONTRACT.contains(&format!("{key}:")),
                "events.ts is missing report key {key}"
            );
        }
        assert_eq!(value["claims"][0]["outcome"], "invalid_citation");
        assert_eq!(value["claims"][0]["kind"], "sentence");
        assert_eq!(value["method"], "entailment");
        assert_eq!(value["needs"][0]["state"], "missing");
        let back: GroundingReport = serde_json::from_value(value).unwrap();
        assert_eq!(back, sample_report());
    }

    #[test]
    fn plan_items_from_older_builds_parse_without_need_fields() {
        let old = json!({"type": "plan_updated", "runId": "r", "items": [{"id": "1", "text": "Search", "status": "done"}]});
        let event: AgentEvent = serde_json::from_value(old).unwrap();
        assert_eq!(
            event,
            AgentEvent::PlanUpdated {
                run_id: "r".into(),
                items: vec![PlanItem::task("1", "Search", PlanStatus::Done)],
            }
        );
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
            (
                NavigationTarget::Visual {
                    visual_id: "v1".into(),
                    version: Some(2),
                },
                "visual",
                vec!["visualId", "version"],
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
            "\"covered\"",
            "\"missing\"",
            "\"supported\"",
            "\"weak\"",
            "\"unsupported\"",
            "\"uncited_factual\"",
            "\"invalid_citation\"",
            "\"unchecked\"",
            "\"entailment\"",
            "\"cross_encoder\"",
            "\"lexical\"",
            "\"sentence\"",
            "\"list_item\"",
            "\"table_row\"",
            "\"repair\"",
            "\"coverage\"",
            "\"repair_and_coverage\"",
        ] {
            assert!(
                TS_CONTRACT.contains(literal),
                "events.ts is missing {literal}"
            );
        }
    }
}
