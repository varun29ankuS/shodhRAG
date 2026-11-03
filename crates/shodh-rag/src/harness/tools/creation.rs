//! `finish_creation`: a Creator mode answer says what it built and what it
//! chose.
//!
//! Creator mode asks no clarifying questions ([`crate::harness::modes`]): the
//! agent picks sensible defaults. This tool changes nothing. It records, on
//! its step, a one-line summary and the choices the user did not specify; the
//! app shows them on the finish card ("Here's what I chose", with Keep,
//! Change something and Delete it) as plain text, never as actions.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{req_str, HostTool, ToolContext, ToolError, ToolOutput};
use crate::harness::events::RiskTier;
use crate::harness::protocol::ToolLoadMode;

pub const FINISH_CREATION: &str = "finish_creation";

/// Longest summary kept, in characters.
pub const MAX_SUMMARY_CHARS: usize = 200;
/// Most choices kept.
pub const MAX_CHOICES: usize = 12;
/// Longest choice kept, in characters.
pub const MAX_CHOICE_CHARS: usize = 160;

pub struct FinishCreationTool;

/// `text` on one line, trimmed, at most `max` characters.
fn one_line(text: &str, max: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

#[async_trait]
impl HostTool for FinishCreationTool {
    fn name(&self) -> &'static str {
        FINISH_CREATION
    }
    fn label(&self) -> &'static str {
        "Finish the creation"
    }
    fn label_template(&self) -> &'static str {
        "Finishing: {summary}"
    }
    fn description(&self) -> &'static str {
        "Call once, when what the user asked for is built and checked: a one-line summary of \
         what you made, and every choice you made that the user did not specify (the stack, \
         the layout, the colours, names, sizes, sample data), each as one short plain line \
         such as \"Plain HTML, CSS and JavaScript, no build step\". The user sees them on the \
         finish card and can keep the result, change something or delete it. This changes \
         nothing."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "minLength": 1, "maxLength": MAX_SUMMARY_CHARS},
                "choices": {
                    "type": "array",
                    "items": {"type": "string", "minLength": 1, "maxLength": MAX_CHOICE_CHARS},
                    "maxItems": MAX_CHOICES
                }
            },
            "required": ["summary", "choices"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    fn load_mode(&self) -> ToolLoadMode {
        ToolLoadMode::Essential
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let summary = one_line(
            req_str(&args, "summary", FINISH_CREATION)?,
            MAX_SUMMARY_CHARS,
        );
        if summary.is_empty() {
            return Err(ToolError::InvalidArguments {
                tool: FINISH_CREATION.to_string(),
                reasons: "`summary` must say what was built".to_string(),
            });
        }
        let choices: Vec<String> = args
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: FINISH_CREATION.to_string(),
                reasons: "`choices` must be a list of short lines".to_string(),
            })?
            .iter()
            .filter_map(Value::as_str)
            .map(|choice| one_line(choice, MAX_CHOICE_CHARS))
            .filter(|choice| !choice.is_empty())
            .take(MAX_CHOICES)
            .collect();
        Ok(ToolOutput {
            text_for_model: "The finish card now shows your summary and choices, with Keep, \
                             Change something and Delete it for the user. End the answer with \
                             the \"Here's what I chose\" section; do not call this again."
                .to_string(),
            summary_for_ui: format!("Finished: {summary}"),
            detail: Some(json!({ "summary": summary, "choices": choices })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::modes::AgentMode;
    use crate::harness::profile::AgentProfile;
    use crate::harness::tools::{ApprovalGate, ToolCall, ToolRegistry};

    async fn finish(args: Value) -> crate::harness::tools::DispatchOutcome {
        let mut registry = ToolRegistry::new();
        registry
            .register(std::sync::Arc::new(FinishCreationTool))
            .unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = ToolContext::new("run-1", "step-1", tx);
        let profile = AgentProfile {
            allowed_tools: vec![FINISH_CREATION.into()],
            mode: AgentMode::Creator,
            ..AgentProfile::assistant()
        };
        registry
            .dispatch(
                ToolCall {
                    tool: FINISH_CREATION.into(),
                    args,
                    call_index: 1,
                },
                &profile,
                &ApprovalGate::default(),
                &ctx,
            )
            .await
    }

    #[tokio::test]
    async fn the_finish_card_gets_the_summary_and_the_choices_as_plain_lines() {
        let outcome = finish(json!({
            "summary": "  A pomodoro timer\n that runs in the browser ",
            "choices": [
                "Plain HTML, CSS and JavaScript, no build step",
                "   ",
                "25 minute focus,\n5 minute break"
            ]
        }))
        .await;
        assert!(outcome.ok, "{}", outcome.text_for_model);
        assert_eq!(
            outcome.detail,
            Some(json!({
                "summary": "A pomodoro timer that runs in the browser",
                "choices": [
                    "Plain HTML, CSS and JavaScript, no build step",
                    "25 minute focus, 5 minute break"
                ]
            }))
        );
    }

    #[tokio::test]
    async fn it_changes_nothing_and_runs_without_asking() {
        // Read tier: it runs in every mode that offers it, Creator included.
        assert_eq!(FinishCreationTool.tier(), RiskTier::Read);
        let outcome = finish(json!({"summary": "A game", "choices": []})).await;
        assert!(outcome.ok, "{}", outcome.text_for_model);
    }

    #[tokio::test]
    async fn a_summary_is_required() {
        for args in [
            json!({"summary": "   ", "choices": []}),
            json!({"choices": ["x"]}),
            json!({"summary": "A game"}),
        ] {
            let outcome = finish(args.clone()).await;
            assert!(!outcome.ok, "{args}");
            assert!(outcome.detail.is_none(), "{args}");
        }
    }

    #[tokio::test]
    async fn the_schema_caps_the_list_and_each_line() {
        let most: Vec<String> = (0..MAX_CHOICES).map(|i| format!("choice {i}")).collect();
        let outcome = finish(json!({"summary": "x", "choices": most})).await;
        assert!(outcome.ok, "{}", outcome.text_for_model);
        let too_many: Vec<String> = (0..=MAX_CHOICES).map(|i| format!("choice {i}")).collect();
        let outcome = finish(json!({"summary": "x", "choices": too_many})).await;
        assert!(!outcome.ok && outcome.invalid_arguments);
        let long = "y".repeat(MAX_CHOICE_CHARS + 1);
        let outcome = finish(json!({"summary": "x", "choices": [long]})).await;
        assert!(!outcome.ok && outcome.invalid_arguments);
        assert_eq!(
            one_line(&"y".repeat(MAX_CHOICE_CHARS * 2), MAX_CHOICE_CHARS)
                .chars()
                .count(),
            MAX_CHOICE_CHARS
        );
    }
}
