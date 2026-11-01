//! Indexing control tools: `pause_indexing`, `resume_indexing`,
//! `cancel_indexing` and `system_status`.
//!
//! The assistant operates indexing as the status strip does, visibly: every
//! call opens the System status panel with the part it is about highlighted
//! ([`super::HostEffects::show_system_status`]), and pause, resume and cancel
//! ask for approval like the other changes it makes, and are audited.
//! Whether search may use the graphics card stays the user's choice.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::audit::AuditEventType;
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::{
    ApprovalPreview, HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::RiskTier;

use super::{str_arg, AgentHost};

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(PauseTool {
        host: host.clone(),
        pause: true,
    }))?;
    registry.register(Arc::new(PauseTool {
        host: host.clone(),
        pause: false,
    }))?;
    registry.register(Arc::new(CancelTool { host: host.clone() }))?;
    registry.register(Arc::new(StatusTool { host: host.clone() }))?;
    Ok(())
}

/// The audit record of an indexing control the assistant used.
fn control_payload(action: &str, source_id: Option<&str>, runs: Option<usize>) -> Value {
    json!({
        "action": action,
        "sourceId": source_id,
        "runs": runs,
        "via": "agent",
    })
}

pub struct PauseTool {
    host: Arc<AgentHost>,
    /// Pause (`true`) or resume.
    pause: bool,
}

#[async_trait]
impl HostTool for PauseTool {
    fn name(&self) -> &'static str {
        if self.pause {
            app_tools::PAUSE_INDEXING
        } else {
            app_tools::RESUME_INDEXING
        }
    }
    fn label(&self) -> &'static str {
        if self.pause {
            "Pause indexing"
        } else {
            "Resume indexing"
        }
    }
    fn label_template(&self) -> &'static str {
        if self.pause {
            "Pausing indexing"
        } else {
            "Resuming indexing"
        }
    }
    fn description(&self) -> &'static str {
        if self.pause {
            "Pause all indexing (folders, synced folders, files added in chats, scans being read). \
             It stays paused, also after Shodh restarts, until it is resumed."
        } else {
            "Resume indexing that was paused."
        }
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, _args: &Value) -> Result<ApprovalPreview, ToolError> {
        Ok(ApprovalPreview {
            label: Some(self.label().to_string()),
            details: json!({ "paused": self.pause }),
        })
    }
    async fn execute(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let effects = &self.host.effects;
        let was = effects.indexing_paused();
        effects.set_indexing_paused(self.pause);
        effects.show_system_status("indexing");
        if was != self.pause {
            ctx.audit(
                AuditEventType::SettingsChange,
                control_payload(
                    if self.pause {
                        "indexing_paused"
                    } else {
                        "indexing_resumed"
                    },
                    None,
                    None,
                ),
            );
        }
        let text = match (was, self.pause) {
            (true, true) => "Indexing was already paused.",
            (false, true) => "Indexing is paused. It stays paused until it is resumed.",
            (false, false) => "Indexing was not paused.",
            (true, false) => "Indexing is resumed.",
        };
        Ok(ToolOutput {
            text_for_model: text.to_string(),
            summary_for_ui: self.label().to_string(),
            detail: Some(json!({ "paused": self.pause, "wasPaused": was })),
        })
    }
}

pub struct CancelTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for CancelTool {
    fn name(&self) -> &'static str {
        app_tools::CANCEL_INDEXING
    }
    fn label(&self) -> &'static str {
        "Cancel indexing"
    }
    fn label_template(&self) -> &'static str {
        "Cancelling indexing[ of {source_id}]"
    }
    fn description(&self) -> &'static str {
        "Cancel the indexing in progress of one source (source_id from list_sources or \
         system_status), or of everything without one. What was indexed stays; the rest is \
         indexed the next time the folder is indexed."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source_id": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let source = str_arg(args, "source_id");
        Ok(ApprovalPreview {
            label: Some(match source {
                Some(id) => format!("Cancel the indexing of {id}"),
                None => "Cancel all indexing in progress".to_string(),
            }),
            details: json!({ "sourceId": source }),
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let source = str_arg(&args, "source_id");
        let effects = &self.host.effects;
        let runs = effects.cancel_indexing(source);
        effects.show_system_status("indexing");
        ctx.audit(
            AuditEventType::SourceChange,
            control_payload("indexing_cancelled", source, Some(runs)),
        );
        let text = match (runs, source) {
            (0, Some(id)) => format!("Nothing of {id} was being indexed."),
            (0, None) => "Nothing was being indexed.".to_string(),
            (n, Some(id)) => {
                format!("Cancelled {n} indexing run(s) of {id}; what was indexed stays.")
            }
            (n, None) => format!("Cancelled {n} indexing run(s); what was indexed stays."),
        };
        Ok(ToolOutput {
            text_for_model: text,
            summary_for_ui: "Cancelled indexing".to_string(),
            detail: Some(json!({ "sourceId": source, "runs": runs })),
        })
    }
}

pub struct StatusTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for StatusTool {
    fn name(&self) -> &'static str {
        app_tools::SYSTEM_STATUS
    }
    fn label(&self) -> &'static str {
        "System status"
    }
    fn label_template(&self) -> &'static str {
        "Reading the system status"
    }
    fn description(&self) -> &'static str {
        "What runs search on this computer (graphics card or CPU, and why), its measured speed \
         and whether it runs slower than usual, the reranker and scan reader, whether indexing \
         is paused, and what is being indexed now. Use it to answer why indexing is slow or \
         what is running; the System status panel opens for the user."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let status = self.host.effects.system_status().await;
        self.host.effects.show_system_status("search");
        Ok(ToolOutput {
            text_for_model: status.to_string(),
            summary_for_ui: "Read the system status".to_string(),
            detail: Some(status),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;

    #[tokio::test]
    async fn pausing_resuming_and_cancelling_act_on_indexing_and_open_the_panel() {
        let t = testing::host().await;
        let (ctx, _rx) = testing::ctx();
        let pause = PauseTool {
            host: t.host.clone(),
            pause: true,
        };
        let out = pause.execute(json!({}), &ctx).await.unwrap();
        assert!(t.effects.indexing_state.is_paused());
        assert!(out.text_for_model.contains("paused"));
        let resume = PauseTool {
            host: t.host.clone(),
            pause: false,
        };
        resume.execute(json!({}), &ctx).await.unwrap();
        assert!(!t.effects.indexing_state.is_paused());

        let run = t
            .effects
            .indexing_state
            .begin_run_at("folder-papers", "C:/Papers")
            .unwrap();
        let cancel = CancelTool {
            host: t.host.clone(),
        };
        let out = cancel
            .execute(json!({ "source_id": "folder-papers" }), &ctx)
            .await
            .unwrap();
        assert!(run.stopped());
        assert_eq!(out.detail.unwrap()["runs"], 1);
        let shown = t
            .effects
            .shown_status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(shown, ["indexing", "indexing", "indexing"]);
    }

    #[tokio::test]
    async fn the_status_tool_reads_the_status_and_shows_it() {
        let t = testing::host().await;
        let (ctx, _rx) = testing::ctx();
        t.effects.indexing_state.pause();
        let status = StatusTool {
            host: t.host.clone(),
        };
        assert_eq!(status.tier(), RiskTier::Read);
        let out = status.execute(json!({}), &ctx).await.unwrap();
        assert_eq!(out.detail.unwrap()["indexingPaused"], true);
        assert!(t
            .effects
            .shown_status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&"search".to_string()));
    }
}
