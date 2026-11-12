//! `update_plan`: replaces the run's checklist, which the user sees above the
//! answer while it runs and, folded to its counts, on the finished answer.
//!
//! The checklist is the agent's own account of where the work stands, so its
//! states mean something: an item is done only when it is finished, and a
//! blocked item always says why (a blocked item without a reason is refused).
//! The user can read it but not edit it.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{HostTool, ToolContext, ToolError, ToolOutput};
use crate::harness::events::{AgentEvent, PlanItem, PlanStatus, RiskTier};

pub const UPDATE_PLAN: &str = "update_plan";

const MAX_ITEMS: usize = 20;

/// Longest reason a blocked item keeps.
pub const MAX_REASON_CHARS: usize = 200;

pub struct UpdatePlanTool;

fn parse_status(value: &str) -> Option<PlanStatus> {
    match value {
        "pending" => Some(PlanStatus::Pending),
        "in_progress" => Some(PlanStatus::InProgress),
        "done" => Some(PlanStatus::Done),
        "blocked" => Some(PlanStatus::Blocked),
        _ => None,
    }
}

/// Where a list stands in words, e.g. "3 of 5 done, 1 blocked".
fn progress_words(items: &[PlanItem]) -> String {
    let done = items
        .iter()
        .filter(|i| i.status == PlanStatus::Done)
        .count();
    let blocked = items
        .iter()
        .filter(|i| i.status == PlanStatus::Blocked)
        .count();
    let total = items.len();
    if blocked > 0 {
        format!("{done} of {total} done, {blocked} blocked")
    } else {
        format!("{done} of {total} done")
    }
}

#[async_trait]
impl HostTool for UpdatePlanTool {
    fn name(&self) -> &'static str {
        UPDATE_PLAN
    }
    fn label(&self) -> &'static str {
        "Update plan"
    }
    fn label_template(&self) -> &'static str {
        "Updating the task list"
    }
    fn description(&self) -> &'static str {
        "Replace the checklist the user sees above this answer. Keep one when the work takes \
         three or more steps; a quick answer needs none. Send the whole list on every call, each \
         item with its status: pending, in_progress, done or blocked. Mark an item done only \
         once it is really finished (a step whose check failed is not done). A blocked item \
         needs a reason: one short sentence on what stops it. Before the final answer, send the \
         list once more as it really stands. Write each item as a short step the user \
         understands, starting with a verb and at most eight words, e.g. \"Find the training \
         objective\" or \"Compare results with Mamba\"; never tool names, search queries, ids or \
         file paths. For a question with several parts, list each part the answer must find in \
         the sources as an item with need: true (a short phrase naming the information, e.g. \
         \"notice period of the Acme contract\"); the app checks each need against the passages \
         you retrieve and shows whether it was covered."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "maxItems": MAX_ITEMS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "minLength": 1, "maxLength": 40},
                            "text": {"type": "string", "minLength": 1, "maxLength": 200},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "done", "blocked"]},
                            "reason": {"type": "string", "maxLength": MAX_REASON_CHARS, "description": "For a blocked item: what stops it, in one short sentence."},
                            "need": {"type": "boolean"}
                        },
                        "required": ["text", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["items"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let raw = args.get("items").and_then(Value::as_array).ok_or_else(|| {
            ToolError::InvalidArguments {
                tool: UPDATE_PLAN.to_string(),
                reasons: "`items` must be an array".to_string(),
            }
        })?;
        let mut items = Vec::with_capacity(raw.len());
        for (index, item) in raw.iter().enumerate() {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .and_then(parse_status)
                .ok_or_else(|| ToolError::InvalidArguments {
                    tool: UPDATE_PLAN.to_string(),
                    reasons: format!("/items/{index}/status is not a known status"),
                })?;
            // A reason belongs to a blocked item only; one on another item is dropped.
            let reason = item
                .get("reason")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(|r| r.chars().take(MAX_REASON_CHARS).collect::<String>());
            let reason = match status {
                PlanStatus::Blocked => Some(reason.ok_or_else(|| ToolError::InvalidArguments {
                    tool: UPDATE_PLAN.to_string(),
                    reasons: format!(
                        "/items/{index} is blocked without a reason: say in one short sentence what stops it"
                    ),
                })?),
                PlanStatus::Pending | PlanStatus::InProgress | PlanStatus::Done => None,
            };
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| (index + 1).to_string());
            let need = item.get("need").and_then(Value::as_bool).unwrap_or(false);
            items.push(PlanItem {
                need,
                reason,
                ..PlanItem::task(id, text, status)
            });
        }
        let progress = progress_words(&items);
        // Stored for the harness's need check; coverage found earlier stays.
        let items = ctx.record_plan(items);
        ctx.emit(AgentEvent::PlanUpdated {
            run_id: ctx.run_id.clone(),
            items,
        });
        Ok(ToolOutput {
            text_for_model: format!("Task list updated: {progress}."),
            summary_for_ui: progress,
            detail: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn update(args: Value) -> (Result<ToolOutput, ToolError>, Vec<AgentEvent>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = ToolContext::new("run-1", "step-1", tx);
        let result = UpdatePlanTool.execute(args, &ctx).await;
        drop(ctx);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        (result, events)
    }

    #[tokio::test]
    async fn a_blocked_item_keeps_its_reason_and_the_counts_say_so() {
        let (result, events) = update(json!({"items": [
            {"text": "Read the failing test", "status": "done"},
            {"text": "Run the build", "status": "blocked", "reason": "  The build needs a signing key  "},
            {"text": "Fix the parser", "status": "pending", "reason": "ignored"}
        ]}))
        .await;
        let output = result.unwrap();
        assert_eq!(output.summary_for_ui, "1 of 3 done, 1 blocked");
        assert_eq!(
            output.text_for_model,
            "Task list updated: 1 of 3 done, 1 blocked."
        );
        let [AgentEvent::PlanUpdated { items, .. }] = events.as_slice() else {
            panic!("{events:?}");
        };
        assert_eq!(items[1].status, PlanStatus::Blocked);
        assert_eq!(
            items[1].reason.as_deref(),
            Some("The build needs a signing key")
        );
        // A reason on an item that is not blocked is not kept.
        assert_eq!(items[2].reason, None);
    }

    #[tokio::test]
    async fn a_blocked_item_without_a_reason_is_refused_and_nothing_is_shown() {
        for item in [
            json!({"text": "Run the build", "status": "blocked"}),
            json!({"text": "Run the build", "status": "blocked", "reason": "   "}),
        ] {
            let (result, events) = update(json!({ "items": [item] })).await;
            match result {
                Err(ToolError::InvalidArguments { reasons, .. }) => {
                    assert!(reasons.contains("without a reason"), "{reasons}")
                }
                other => panic!("{other:?}"),
            }
            assert!(events.is_empty(), "{events:?}");
        }
    }

    #[test]
    fn the_description_holds_the_checklist_rules() {
        let text = UpdatePlanTool.description();
        for rule in [
            "three or more steps",
            "a quick answer needs none",
            "done only once it is really finished",
            "A blocked item needs a reason",
            "Before the final answer",
        ] {
            assert!(text.contains(rule), "{rule}");
        }
        let schema = UpdatePlanTool.schema();
        assert_eq!(
            schema["properties"]["items"]["items"]["properties"]["status"]["enum"],
            json!(["pending", "in_progress", "done", "blocked"])
        );
    }
}
