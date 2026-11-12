//! `suggest_mode`: the agent suggests another mode; the user decides.
//!
//! The agent can never change the mode ([`crate::harness::modes`]). This tool
//! changes nothing: it records the suggestion on its step, and the app shows
//! it under the answer as a button the user may click.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{req_str, HostTool, ToolContext, ToolError, ToolOutput};
use crate::harness::events::RiskTier;
use crate::harness::modes::AgentMode;
use crate::harness::protocol::ToolLoadMode;

pub const SUGGEST_MODE: &str = "suggest_mode";

/// Longest reason shown on the button's line.
const MAX_REASON_CHARS: usize = 160;

/// What the model is told when the user already passed on a suggestion.
pub const SUGGESTION_IGNORED: &str = "The user went on without the mode you suggested earlier in \
     this conversation, so no more suggestions are shown in it. Carry on in the current mode and \
     do not suggest a mode again.";

pub struct SuggestModeTool;

#[async_trait]
impl HostTool for SuggestModeTool {
    fn name(&self) -> &'static str {
        SUGGEST_MODE
    }
    fn label(&self) -> &'static str {
        "Suggest a mode"
    }
    fn label_template(&self) -> &'static str {
        "Suggesting {mode}"
    }
    fn description(&self) -> &'static str {
        "Suggest that the user switch this conversation to another mode: plan (read and write \
         a plan first), acceptEdits (edits run without asking), auto (edits and low-risk \
         commands run without asking) or ask (everything asks). This changes nothing: the \
         user sees a button and decides. Call it at most once per answer, only when another \
         mode clearly fits the task better, with one short reason."
    }
    fn schema(&self) -> Value {
        let modes: Vec<&str> = AgentMode::SUGGESTABLE.iter().map(|m| m.as_str()).collect();
        json!({
            "type": "object",
            "properties": {
                "mode": {"type": "string", "enum": modes},
                "reason": {"type": "string", "minLength": 1, "maxLength": MAX_REASON_CHARS}
            },
            "required": ["mode", "reason"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    fn load_mode(&self) -> ToolLoadMode {
        ToolLoadMode::Essential
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        // Once the user went on without an earlier suggestion, the conversation
        // gets no more: asking again is noise, not help.
        if ctx.scope().suggestion_ignored {
            return Err(ToolError::Forbidden(SUGGESTION_IGNORED.to_string()));
        }
        let name = req_str(&args, "mode", SUGGEST_MODE)?;
        let mode = AgentMode::parse(name)
            .filter(|mode| AgentMode::SUGGESTABLE.contains(mode))
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: SUGGEST_MODE.to_string(),
                reasons: format!(
                    "`mode` must be one of ask, acceptEdits, plan or auto, not {name}"
                ),
            })?;
        let reason = req_str(&args, "reason", SUGGEST_MODE)?;
        let reason: String = reason.chars().take(MAX_REASON_CHARS).collect();
        Ok(ToolOutput {
            text_for_model: format!(
                "The user now sees a button to switch to {}. The mode has not changed: \
                 continue in the current mode, and do not suggest it again in this answer.",
                mode.label()
            ),
            summary_for_ui: format!("Suggested {}", mode.label()),
            detail: Some(json!({ "suggestedMode": mode.as_str(), "reason": reason })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::profile::AgentProfile;
    use crate::harness::tools::{ApprovalGate, ToolCall, ToolRegistry};

    async fn suggest(
        profile: &AgentProfile,
        args: Value,
    ) -> crate::harness::tools::DispatchOutcome {
        let mut registry = ToolRegistry::new();
        registry
            .register(std::sync::Arc::new(SuggestModeTool))
            .unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = ToolContext::new("run-1", "step-1", tx);
        registry
            .dispatch(
                ToolCall {
                    tool: SUGGEST_MODE.into(),
                    args,
                    call_index: 1,
                },
                profile,
                &ApprovalGate::default(),
                &ctx,
            )
            .await
    }

    #[tokio::test]
    async fn a_suggestion_changes_nothing_and_is_offered_in_every_mode() {
        for mode in AgentMode::ALL {
            let profile = AgentProfile {
                allowed_tools: vec![SUGGEST_MODE.into()],
                mode,
                ..AgentProfile::assistant()
            };
            let outcome = suggest(
                &profile,
                json!({"mode": "auto", "reason": "Many small edits and test runs"}),
            )
            .await;
            assert!(outcome.ok, "{}", outcome.text_for_model);
            assert!(outcome.text_for_model.contains("has not changed"));
            assert_eq!(
                outcome.detail,
                Some(json!({"suggestedMode": "auto", "reason": "Many small edits and test runs"}))
            );
            // The profile, and so the session's mode, is untouched.
            assert_eq!(profile.mode, mode);
        }
    }

    #[tokio::test]
    async fn no_suggestion_once_the_user_went_on_without_one() {
        use crate::harness::tools::RunScope;
        let profile = AgentProfile {
            allowed_tools: vec![SUGGEST_MODE.into()],
            ..AgentProfile::assistant()
        };
        let mut registry = ToolRegistry::new();
        registry
            .register(std::sync::Arc::new(SuggestModeTool))
            .unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx =
            ToolContext::new("run-2", "step-1", tx).with_scope(std::sync::Arc::new(RunScope {
                suggestion_ignored: true,
                ..RunScope::default()
            }));
        let outcome = registry
            .dispatch(
                ToolCall {
                    tool: SUGGEST_MODE.into(),
                    args: json!({"mode": "plan", "reason": "Look first"}),
                    call_index: 1,
                },
                &profile,
                &ApprovalGate::default(),
                &ctx,
            )
            .await;
        assert!(!outcome.ok);
        assert!(
            outcome
                .text_for_model
                .contains("do not suggest a mode again"),
            "{}",
            outcome.text_for_model
        );
        // No button reaches the answer.
        assert!(outcome.detail.is_none());
    }

    #[tokio::test]
    async fn only_real_modes_can_be_suggested() {
        let profile = AgentProfile {
            allowed_tools: vec![SUGGEST_MODE.into()],
            ..AgentProfile::assistant()
        };
        for mode in ["bypass", "creator", "Auto", ""] {
            let outcome = suggest(&profile, json!({"mode": mode, "reason": "x"})).await;
            assert!(!outcome.ok, "{mode}");
            assert!(outcome.detail.is_none());
        }
    }
}
