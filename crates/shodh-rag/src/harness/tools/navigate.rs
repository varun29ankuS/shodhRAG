//! `open_view`: switches the app tab. The conversation keeps streaming in
//! the dock (spec §10a, Conversation dock).

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{opt_str, req_str, HostTool, ToolContext, ToolError, ToolOutput};
use crate::harness::events::{AgentEvent, RiskTier};

pub const OPEN_VIEW: &str = "open_view";

/// Views the agent may open.
pub const VIEWS: [&str; 4] = ["ask", "library", "calendar", "settings"];

pub struct OpenViewTool;

#[async_trait]
impl HostTool for OpenViewTool {
    fn name(&self) -> &'static str {
        OPEN_VIEW
    }
    fn label(&self) -> &'static str {
        "Open view"
    }
    fn label_template(&self) -> &'static str {
        "Opening {view}"
    }
    fn description(&self) -> &'static str {
        "Switch the app to a view so the user can see a result: ask (conversation), library \
         (indexed folders), calendar (tasks and events), settings. Optionally focus an item by id, \
         e.g. the id of a task you just created."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "view": {"type": "string", "enum": VIEWS},
                "focus": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "required": ["view"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let view = req_str(&args, "view", OPEN_VIEW)?;
        if !VIEWS.contains(&view) {
            return Err(ToolError::InvalidArguments {
                tool: OPEN_VIEW.to_string(),
                reasons: format!("unknown view {view}"),
            });
        }
        let focus = opt_str(&args, "focus").map(str::to_string);
        ctx.emit(AgentEvent::Navigated {
            run_id: ctx.run_id.clone(),
            view: view.to_string(),
            focus: focus.clone(),
        });
        Ok(ToolOutput {
            text_for_model: format!("The {view} view is now open for the user."),
            summary_for_ui: format!("Opened {view}"),
            detail: focus.map(|f| json!({ "focus": f })),
        })
    }
}
