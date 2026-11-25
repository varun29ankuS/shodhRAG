//! Automation tools: `list_automations` and `show_automation` (read; they open
//! Automations), and `create_automation`, `update_automation`, `delete_automation`,
//! `pause_automation`, `resume_automation` and `run_automation` (they ask first).
//!
//! An automation spends model calls without the user watching, so making one, changing
//! what it does or starting one always asks, and the preview says when it runs and what it
//! may cost. The assistant never decides a run's changes (`engine_approval_decide`) or
//! picks the automations' model: both are the user's.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::engine::schedule::Schedule;
use shodh_rag::engine::store::CreatedBy;
use shodh_rag::harness::events::NavigationTarget;
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::navigate::emit_shown;
use shodh_rag::harness::tools::{
    ApprovalPreview, HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::RiskTier;

use super::{invalid, str_arg, AgentHost};
use crate::automations::{
    estimate_for, planned_calls, when_text, AutomationState, AutomationView, Automations, Draft,
};

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(ListAutomationsTool { host: host.clone() }))?;
    registry.register(Arc::new(ShowAutomationTool { host: host.clone() }))?;
    registry.register(Arc::new(SaveAutomationTool {
        host: host.clone(),
        update: false,
    }))?;
    registry.register(Arc::new(SaveAutomationTool {
        host: host.clone(),
        update: true,
    }))?;
    registry.register(Arc::new(DeleteAutomationTool { host: host.clone() }))?;
    for action in [Action::Pause, Action::Resume, Action::Run] {
        registry.register(Arc::new(AutomationActionTool {
            host: host.clone(),
            action,
        }))?;
    }
    Ok(())
}

async fn service(host: &AgentHost) -> Result<Arc<Automations>, ToolError> {
    host.effects.automations().await.ok_or_else(|| {
        ToolError::Unavailable(
            "Automations need the app database (shodh.db), which could not be opened.".into(),
        )
    })
}

async fn find(host: &AgentHost, id: &str) -> Result<AutomationView, ToolError> {
    let automations = service(host).await?;
    let id = id.to_string();
    tokio::task::spawn_blocking(move || automations.get(&id))
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?
        .map_err(ToolError::NotFound)
}

/// Opens Automations at `automation` (and its run `run`).
fn show(ctx: &ToolContext, automation: &AutomationView, run: Option<String>) {
    emit_shown(
        ctx,
        "automations",
        Some(automation.id.clone()),
        Some(NavigationTarget::Automation {
            automation_id: automation.id.clone(),
            run_id: run,
        }),
        Some(&automation.name),
    );
}

fn state_words(state: AutomationState) -> &'static str {
    match state {
        AutomationState::Scheduled => "scheduled",
        AutomationState::Manual => "runs when started",
        AutomationState::Watching => "runs when what it watches happens",
        AutomationState::Running => "running now",
        AutomationState::Waiting => "waiting (while the user chats, or for its model)",
        AutomationState::Paused => "paused until the provider's limit resets",
        AutomationState::Offline => "waiting for the network",
        AutomationState::Retrying => "trying again soon",
        AutomationState::NeedsYou => "waiting for the user's OK on its changes",
        AutomationState::OnHold => "on hold after failing again and again",
        AutomationState::Off => "turned off by the user",
    }
}

fn line(a: &AutomationView) -> String {
    let last = a
        .last
        .as_ref()
        .and_then(|r| r.summary.clone().or(r.error_detail.clone()))
        .map(|s| format!("; last run: {s}"))
        .unwrap_or_default();
    format!(
        "- [{}] {} ({}; {}{last})",
        a.id,
        a.name,
        a.when,
        state_words(a.state)
    )
}

const SCHEDULE_SCHEMA: &str = r#"{
  "description": "When it runs: a schedule object, or the string \"manual\" (only when started). Kinds: {\"kind\":\"daily\",\"at\":\"09:00\"}, {\"kind\":\"weekdays\",\"at\":\"09:00\"}, {\"kind\":\"weekly\",\"days\":[\"mon\"],\"at\":\"09:00\"}, {\"kind\":\"monthly\",\"day\":1,\"at\":\"09:00\"} (day 1-31 or \"last\"), {\"kind\":\"every_hours\",\"hours\":6,\"from\":\"<RFC 3339>\"}. Times are the user's local time.",
  "oneOf": [
    { "type": "string", "enum": ["manual"] },
    { "type": "object", "properties": { "kind": { "type": "string" } }, "required": ["kind"] }
  ]
}"#;

fn schedule_of(tool: &str, value: Option<&Value>) -> Result<Option<Schedule>, ToolError> {
    match value {
        None => Ok(None),
        Some(Value::String(s)) if s == "manual" => Ok(None),
        Some(value) => serde_json::from_value::<Schedule>(value.clone())
            .map(Some)
            .map_err(|e| invalid(tool, format!("schedule: {e}"))),
    }
}

fn draft_of(
    tool: &str,
    args: &Value,
    current: Option<&AutomationView>,
) -> Result<Draft, ToolError> {
    let name = str_arg(args, "name")
        .map(str::to_string)
        .or_else(|| current.map(|c| c.name.clone()))
        .ok_or_else(|| invalid(tool, "name is required"))?;
    let instruction = str_arg(args, "instruction")
        .map(str::to_string)
        .or_else(|| current.map(|c| c.instruction.clone()))
        .ok_or_else(|| invalid(tool, "instruction is required"))?;
    let schedule = match (args.get("schedule"), current) {
        (Some(value), _) => schedule_of(tool, Some(value))?,
        (None, Some(current)) => current.schedule.clone(),
        (None, None) => None,
    };
    let workspace_id = match (args.get("workspace_id"), current) {
        (Some(Value::Null), _) => None,
        (Some(_), _) => str_arg(args, "workspace_id").map(str::to_string),
        (None, Some(current)) => current.workspace_id.clone(),
        (None, None) => None,
    };
    // What starts it (new files, an alert's papers) is set in the Automations view. It is
    // kept unless the assistant gives a schedule, which replaces it.
    let trigger = match (args.get("schedule"), current) {
        (None, Some(current)) => current.trigger.clone(),
        _ => None,
    };
    Draft {
        name,
        instruction,
        schedule,
        trigger,
        workspace_id,
        answer_only: current.is_some_and(|c| c.answer_only),
    }
    .checked()
    .map_err(|e| invalid(tool, e))
}

pub struct ListAutomationsTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for ListAutomationsTool {
    fn name(&self) -> &'static str {
        app_tools::LIST_AUTOMATIONS
    }
    fn label(&self) -> &'static str {
        "Automations"
    }
    fn label_template(&self) -> &'static str {
        "Looking at the automations"
    }
    fn description(&self) -> &'static str {
        "List the user's automations: saved instructions Shodh runs on a schedule or when the \
         user starts one, each with when it runs, where it stands and its last run. Opens \
         Automations for the user."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let automations = service(&self.host).await?;
        let list = tokio::task::spawn_blocking(move || automations.list())
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
            .map_err(ToolError::Unavailable)?;
        emit_shown(ctx, "automations", None, None, Some("Automations"));
        let text = if list.is_empty() {
            "No automations yet.".to_string()
        } else {
            list.iter().map(line).collect::<Vec<_>>().join("\n")
        };
        Ok(ToolOutput {
            text_for_model: text,
            summary_for_ui: format!("{} automations", list.len()),
            detail: Some(json!({ "count": list.len() })),
        })
    }
}

pub struct ShowAutomationTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for ShowAutomationTool {
    fn name(&self) -> &'static str {
        app_tools::SHOW_AUTOMATION
    }
    fn label(&self) -> &'static str {
        "Show automation"
    }
    fn label_template(&self) -> &'static str {
        "Showing an automation"
    }
    fn description(&self) -> &'static str {
        "Read and show one automation (its id from list_automations): its instruction, when \
         it runs, and its latest runs with what each found and the model calls it made. Opens \
         it in Automations; `run_id` opens one of its runs."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "minLength": 1, "maxLength": 200 },
                "run_id": { "type": "string", "minLength": 1, "maxLength": 200 }
            },
            "required": ["id"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let id = str_arg(&args, "id").ok_or_else(|| invalid(self.name(), "id is required"))?;
        let automation = find(&self.host, id).await?;
        let automations = service(&self.host).await?;
        let job = automation.id.clone();
        let runs = tokio::task::spawn_blocking(move || automations.runs(&job, 5))
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
            .map_err(ToolError::Unavailable)?;
        let run = str_arg(&args, "run_id").map(str::to_string);
        if let Some(run) = &run {
            if !runs.iter().any(|r| &r.id == run) {
                return Err(ToolError::NotFound(format!("No recent run {run} of it.")));
            }
        }
        show(ctx, &automation, run);
        let mut lines = vec![
            line(&automation),
            format!("Instruction: {}", automation.instruction),
            format!(
                "Each run: {}.",
                estimate_for(automation.schedule.as_ref(), automation.planned_calls)
            ),
        ];
        for r in &runs {
            lines.push(format!(
                "- run [{}] {:?}, {} of {} calls: {}",
                r.id,
                r.state,
                r.used_calls,
                r.planned_calls,
                r.summary
                    .clone()
                    .or(r.error_detail.clone())
                    .unwrap_or_default()
            ));
        }
        Ok(ToolOutput {
            text_for_model: lines.join("\n"),
            summary_for_ui: format!("Showed {}", automation.name),
            detail: Some(json!({ "id": automation.id, "runs": runs.len() })),
        })
    }
}

/// `create_automation` and `update_automation`.
pub struct SaveAutomationTool {
    host: Arc<AgentHost>,
    update: bool,
}

impl SaveAutomationTool {
    async fn current(&self, args: &Value) -> Result<Option<AutomationView>, ToolError> {
        if !self.update {
            return Ok(None);
        }
        let id = str_arg(args, "id").ok_or_else(|| invalid(self.name(), "id is required"))?;
        find(&self.host, id).await.map(Some)
    }
}

#[async_trait]
impl HostTool for SaveAutomationTool {
    fn name(&self) -> &'static str {
        if self.update {
            app_tools::UPDATE_AUTOMATION
        } else {
            app_tools::CREATE_AUTOMATION
        }
    }
    fn label(&self) -> &'static str {
        if self.update {
            "Change automation"
        } else {
            "New automation"
        }
    }
    fn label_template(&self) -> &'static str {
        if self.update {
            "Changing the automation"
        } else {
            "Saving the automation {name}"
        }
    }
    fn description(&self) -> &'static str {
        if self.update {
            "Change an automation (its id from list_automations): its name, instruction, \
             schedule or workspace; what is left out stays. Asks the user first."
        } else {
            "Save an automation: an instruction Shodh runs on a schedule (or only when the user \
             starts it) in an agent session that reads, and saves any change for the user's OK. \
             Each run makes at most 4 model calls. Asks the user first, showing when it runs \
             and what it may cost."
        }
    }
    fn schema(&self) -> Value {
        let schedule: Value = serde_json::from_str(SCHEDULE_SCHEMA).unwrap_or_else(|_| json!({}));
        let mut properties = json!({
            "name": { "type": "string", "minLength": 1, "maxLength": 80 },
            "instruction": { "type": "string", "minLength": 1, "maxLength": 4000 },
            "schedule": schedule,
            "workspace_id": { "type": ["string", "null"], "maxLength": 200 }
        });
        if self.update {
            properties["id"] = json!({ "type": "string", "minLength": 1, "maxLength": 200 });
            json!({
                "type": "object",
                "properties": properties,
                "required": ["id"],
                "additionalProperties": false
            })
        } else {
            json!({
                "type": "object",
                "properties": properties,
                "required": ["name", "instruction"],
                "additionalProperties": false
            })
        }
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let current = self.current(args).await?;
        let draft = draft_of(self.name(), args, current.as_ref())?;
        let verb = if self.update { "Change" } else { "Save" };
        Ok(ApprovalPreview {
            label: Some(format!("{verb} the automation {}", draft.name)),
            details: json!({
                "name": draft.name,
                "instruction": draft.instruction,
                "when": when_text(draft.schedule.as_ref(), draft.trigger.as_ref()),
                "estimate": estimate_for(draft.schedule.as_ref(), planned_calls(draft.answer_only)),
                "workspaceId": draft.workspace_id,
            }),
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let current = self.current(&args).await?;
        let draft = draft_of(self.name(), &args, current.as_ref())?;
        let automations = service(&self.host).await?;
        let run_id = ctx.run_id.clone();
        let saved = match current {
            Some(current) => {
                tokio::task::spawn_blocking(move || automations.update(&current.id, draft)).await
            }
            None => {
                tokio::task::spawn_blocking(move || {
                    automations.create(draft, CreatedBy::Agent, json!({ "runId": run_id }))
                })
                .await
            }
        }
        .map_err(|e| ToolError::Failed(e.to_string()))?
        .map_err(|e| invalid(self.name(), e))?;
        self.host.effects.automations_changed();
        show(ctx, &saved, None);
        Ok(ToolOutput {
            text_for_model: format!(
                "Saved the automation {} ({}). Each run: {}.",
                saved.name,
                saved.when,
                estimate_for(saved.schedule.as_ref(), saved.planned_calls)
            ),
            summary_for_ui: format!("Saved {}", saved.name),
            detail: Some(json!({ "id": saved.id })),
        })
    }
}

pub struct DeleteAutomationTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for DeleteAutomationTool {
    fn name(&self) -> &'static str {
        app_tools::DELETE_AUTOMATION
    }
    fn label(&self) -> &'static str {
        "Delete automation"
    }
    fn label_template(&self) -> &'static str {
        "Deleting an automation"
    }
    fn description(&self) -> &'static str {
        "Delete an automation (its id from list_automations) with its runs. Asks the user first."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string", "minLength": 1, "maxLength": 200 } },
            "required": ["id"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Destructive
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let id = str_arg(args, "id").ok_or_else(|| invalid(self.name(), "id is required"))?;
        let automation = find(&self.host, id).await?;
        Ok(ApprovalPreview {
            label: Some(format!(
                "Delete the automation {} and its runs",
                automation.name
            )),
            details: json!({ "name": automation.name, "when": automation.when }),
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let id = str_arg(&args, "id").ok_or_else(|| invalid(self.name(), "id is required"))?;
        let automation = find(&self.host, id).await?;
        let deleted = service(&self.host)
            .await?
            .delete(&automation.id)
            .await
            .map_err(ToolError::Failed)?;
        self.host.effects.automations_changed();
        emit_shown(ctx, "automations", None, None, Some("Automations"));
        Ok(ToolOutput {
            text_for_model: if deleted {
                format!("Deleted the automation {}.", automation.name)
            } else {
                format!("The automation {} was already gone.", automation.name)
            },
            summary_for_ui: format!("Deleted {}", automation.name),
            detail: Some(json!({ "id": automation.id })),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Pause,
    Resume,
    Run,
}

/// `pause_automation`, `resume_automation` and `run_automation`.
pub struct AutomationActionTool {
    host: Arc<AgentHost>,
    action: Action,
}

#[async_trait]
impl HostTool for AutomationActionTool {
    fn name(&self) -> &'static str {
        match self.action {
            Action::Pause => app_tools::PAUSE_AUTOMATION,
            Action::Resume => app_tools::RESUME_AUTOMATION,
            Action::Run => app_tools::RUN_AUTOMATION,
        }
    }
    fn label(&self) -> &'static str {
        match self.action {
            Action::Pause => "Pause automation",
            Action::Resume => "Resume automation",
            Action::Run => "Run automation",
        }
    }
    fn label_template(&self) -> &'static str {
        match self.action {
            Action::Pause => "Pausing an automation",
            Action::Resume => "Resuming an automation",
            Action::Run => "Starting an automation",
        }
    }
    fn description(&self) -> &'static str {
        match self.action {
            Action::Pause => {
                "Turn an automation off (its id from list_automations): nothing starts it until \
                 it is resumed. Asks the user first."
            }
            Action::Resume => {
                "Turn an automation on again (its id from list_automations), also one on hold. \
                 Asks the user first."
            }
            Action::Run => {
                "Start a run of an automation now (its id from list_automations): up to 4 model \
                 calls. Asks the user first. Its result shows in Automations and its \
                 conversation."
            }
        }
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string", "minLength": 1, "maxLength": 200 } },
            "required": ["id"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    // Not a local edit: what runs without the user, and when, is theirs to change, so
    // pausing, resuming and starting an automation ask in every mode.
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let id = str_arg(args, "id").ok_or_else(|| invalid(self.name(), "id is required"))?;
        let automation = find(&self.host, id).await?;
        let label = match self.action {
            Action::Pause => format!("Pause the automation {}", automation.name),
            Action::Resume => format!("Resume the automation {}", automation.name),
            Action::Run => format!(
                "Run the automation {} now (up to {} model calls)",
                automation.name, automation.planned_calls
            ),
        };
        Ok(ApprovalPreview {
            label: Some(label),
            details: json!({ "name": automation.name, "when": automation.when }),
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let id = str_arg(&args, "id").ok_or_else(|| invalid(self.name(), "id is required"))?;
        let automation = find(&self.host, id).await?;
        let automations = service(&self.host).await?;
        let (text, run) = match self.action {
            Action::Pause | Action::Resume => {
                let on = self.action == Action::Resume;
                let view = automations
                    .set_enabled(&automation.id, on)
                    .await
                    .map_err(ToolError::Failed)?;
                let text = if on {
                    format!("Resumed {}: {}.", view.name, view.when)
                } else {
                    format!(
                        "Paused {}: nothing starts it until it is resumed.",
                        view.name
                    )
                };
                (text, None)
            }
            Action::Run => {
                let run = automations
                    .run_now(&automation.id)
                    .await
                    .map_err(ToolError::Failed)?;
                (
                    format!(
                        "Started {}. Its result shows in Automations when the run ends.",
                        automation.name
                    ),
                    Some(run.id),
                )
            }
        };
        self.host.effects.automations_changed();
        show(ctx, &automation, run);
        Ok(ToolOutput {
            text_for_model: text,
            summary_for_ui: self.label().to_string(),
            detail: Some(json!({ "id": automation.id })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use shodh_rag::harness::RiskTier;

    fn create_args() -> Value {
        json!({
            "name": "Weekly digest",
            "instruction": "Summarise what changed in my thesis sources.",
            "schedule": { "kind": "weekly", "days": ["mon"], "at": "09:00" }
        })
    }

    fn tools(t: &testing::TestHost) -> (SaveAutomationTool, SaveAutomationTool) {
        (
            SaveAutomationTool {
                host: t.host.clone(),
                update: false,
            },
            SaveAutomationTool {
                host: t.host.clone(),
                update: true,
            },
        )
    }

    #[tokio::test]
    async fn creating_previews_the_schedule_and_estimate_then_shows_it() {
        let t = testing::host().await;
        let (create, _) = tools(&t);
        assert_eq!(create.tier(), RiskTier::Write);
        let preview = create.preview(&create_args()).await.unwrap();
        assert_eq!(
            preview.label.as_deref(),
            Some("Save the automation Weekly digest")
        );
        assert_eq!(preview.details["when"], "Every week on Mon at 09:00");
        assert_eq!(
            preview.details["estimate"],
            "up to 4 model calls a run (about 16 a month)"
        );
        let (ctx, mut rx) = testing::ctx();
        let out = create.execute(create_args(), &ctx).await.unwrap();
        assert!(out
            .text_for_model
            .contains("Saved the automation Weekly digest"));
        let shown = testing::expect_navigation(&mut rx, "create_automation");
        assert_eq!(shown.view, "automations");
        let list = t.effects.automations.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].created_by, CreatedBy::Agent);
        assert!(matches!(
            shown.target,
            Some(NavigationTarget::Automation { ref automation_id, run_id: None }) if *automation_id == list[0].id
        ));
    }

    #[tokio::test]
    async fn a_bad_schedule_is_refused_with_its_reason() {
        let t = testing::host().await;
        let (create, _) = tools(&t);
        let mut args = create_args();
        args["schedule"] = json!({ "kind": "weekly", "days": [], "at": "09:00" });
        let refused = create.preview(&args).await.unwrap_err();
        assert!(
            refused.to_string().contains("at least one day"),
            "{refused}"
        );
        args["schedule"] = json!("manual");
        let manual = create.preview(&args).await.unwrap();
        assert_eq!(manual.details["when"], "Only when you start it");
        assert_eq!(manual.details["estimate"], "up to 4 model calls a run");
    }

    #[tokio::test]
    async fn listing_and_showing_open_automations() {
        let t = testing::host().await;
        let (create, update) = tools(&t);
        let (ctx, mut rx) = testing::ctx();
        create.execute(create_args(), &ctx).await.unwrap();
        testing::expect_navigation(&mut rx, "create_automation");
        let id = t.effects.automations.list().unwrap()[0].id.clone();
        let list = ListAutomationsTool {
            host: t.host.clone(),
        };
        let out = list.execute(json!({}), &ctx).await.unwrap();
        assert!(out
            .text_for_model
            .contains("Weekly digest (Every week on Mon at 09:00; scheduled)"));
        let shown = testing::expect_navigation(&mut rx, "list_automations");
        assert_eq!(shown.view, "automations");
        assert_eq!(shown.target, None);
        let show = ShowAutomationTool {
            host: t.host.clone(),
        };
        let out = show.execute(json!({ "id": id }), &ctx).await.unwrap();
        assert!(out.text_for_model.contains("Instruction: Summarise"));
        let shown = testing::expect_navigation(&mut rx, "show_automation");
        assert!(matches!(
            shown.target,
            Some(NavigationTarget::Automation { .. })
        ));
        let out = update
            .execute(json!({ "id": id, "name": "Monday digest" }), &ctx)
            .await
            .unwrap();
        assert!(out
            .text_for_model
            .contains("Monday digest (Every week on Mon at 09:00)"));
        testing::expect_navigation(&mut rx, "update_automation");
        assert!(show
            .execute(json!({ "id": "auto-missing" }), &ctx)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn pausing_resuming_running_and_deleting_ask_and_show_it() {
        let t = testing::host().await;
        let (create, _) = tools(&t);
        let (ctx, mut rx) = testing::ctx();
        create.execute(create_args(), &ctx).await.unwrap();
        testing::expect_navigation(&mut rx, "create_automation");
        let id = t.effects.automations.list().unwrap()[0].id.clone();
        let action = |action| AutomationActionTool {
            host: t.host.clone(),
            action,
        };
        let pause = action(Action::Pause);
        assert!(
            !pause.edits_locally(),
            "what runs without the user asks in every mode"
        );
        pause.execute(json!({ "id": id }), &ctx).await.unwrap();
        testing::expect_navigation(&mut rx, "pause_automation");
        assert_eq!(
            t.effects.automations.get(&id).unwrap().state,
            AutomationState::Off
        );
        let resume = action(Action::Resume);
        resume.execute(json!({ "id": id }), &ctx).await.unwrap();
        testing::expect_navigation(&mut rx, "resume_automation");
        assert_eq!(
            t.effects.automations.get(&id).unwrap().state,
            AutomationState::Scheduled
        );
        let run = action(Action::Run);
        assert!(!run.edits_locally());
        let preview = run.preview(&json!({ "id": id })).await.unwrap();
        assert_eq!(
            preview.label.as_deref(),
            Some("Run the automation Weekly digest now (up to 4 model calls)")
        );
        run.execute(json!({ "id": id }), &ctx).await.unwrap();
        let shown = testing::expect_navigation(&mut rx, "run_automation");
        assert!(matches!(
            shown.target,
            Some(NavigationTarget::Automation {
                run_id: Some(_),
                ..
            })
        ));
        let delete = DeleteAutomationTool {
            host: t.host.clone(),
        };
        assert_eq!(delete.tier(), RiskTier::Destructive);
        let preview = delete.preview(&json!({ "id": id })).await.unwrap();
        assert_eq!(
            preview.label.as_deref(),
            Some("Delete the automation Weekly digest and its runs")
        );
        delete.execute(json!({ "id": id }), &ctx).await.unwrap();
        testing::expect_navigation(&mut rx, "delete_automation");
        assert!(t.effects.automations.list().unwrap().is_empty());
    }
}
