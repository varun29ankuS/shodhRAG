//! `update_plan`: replaces the run's task list shown beside the transcript.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{HostTool, ToolContext, ToolError, ToolOutput};
use crate::harness::events::{AgentEvent, PlanItem, PlanStatus, RiskTier};

pub const UPDATE_PLAN: &str = "update_plan";

const MAX_ITEMS: usize = 20;

pub struct UpdatePlanTool;

fn parse_status(value: &str) -> Option<PlanStatus> {
    match value {
        "pending" => Some(PlanStatus::Pending),
        "in_progress" => Some(PlanStatus::InProgress),
        "done" => Some(PlanStatus::Done),
        _ => None,
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
        "Replace the task list shown to the user for this answer. Send the full list each time, \
         with each item's status (pending, in_progress, done). Use it for work with three or more steps."
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
                            "status": {"type": "string", "enum": ["pending", "in_progress", "done"]},
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
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| (index + 1).to_string());
            let need = item.get("need").and_then(Value::as_bool).unwrap_or(false);
            items.push(PlanItem {
                need,
                ..PlanItem::task(id, text, status)
            });
        }
        let done = items
            .iter()
            .filter(|i| i.status == PlanStatus::Done)
            .count();
        let total = items.len();
        ctx.emit(AgentEvent::PlanUpdated {
            run_id: ctx.run_id.clone(),
            items,
        });
        Ok(ToolOutput {
            text_for_model: format!("Task list updated: {done} of {total} done."),
            summary_for_ui: format!("{done} of {total} done"),
            detail: None,
        })
    }
}
