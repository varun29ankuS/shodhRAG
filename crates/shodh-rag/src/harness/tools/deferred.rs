//! Tools that do not run their changes now: an automation's agent works with nobody
//! watching and reads untrusted text by design, so no change it decides on is made
//! during its run.
//!
//! - **[`DeferredTool`]** wraps a tool. A call that would only read runs as it is. A call
//!   that would change anything (a local edit, a write, a destructive call: whatever a
//!   chat in Ask mode would ask about) is not run: its own approval preview is saved in
//!   [`DeferredCalls`], and the model is told it waits for the user's OK. A read that
//!   would ask because it sends private text off the computer (a web search after the
//!   run read the user's documents) is refused: there is nobody to ask.
//! - **[`deferring_registry`]** wraps every tool of a registry, so the rule holds whatever
//!   the session's mode, and no approval prompt is ever raised in such a session.
//! - **[`ToolRegistry::run_approved`]** makes one saved change later, once the user said
//!   yes, with the arguments it was saved with (checked against the tool's schema again)
//!   and the audit records a chat's approved call has.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    render_label, ApprovalDecision, ApprovalPreview, HostTool, SessionGrant, ToolContext,
    ToolError, ToolOutput, ToolRegistry,
};
use crate::audit::{payload as audit_payload, AuditEventType};
use crate::harness::events::RiskTier;
use crate::harness::modes::{self, AgentMode, Gate, ToolClass};
use crate::harness::protocol::ToolLoadMode;

/// What the model is told a saved change came to.
pub const SAVED_FOR_OK: &str = "Saved for the user's OK; it has not run and will not run in \
     this answer. Do not call it again. Finish your answer and say what you would change and \
     why.";

/// Why a read that would send private text away is refused in an automation.
pub const NOT_IN_AN_AUTOMATION: &str = "Not available in an automation: this call would send \
     private text off the computer, and nobody is here to agree to it. Answer from what you \
     have.";

/// One change an agent asked for and did not make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredCall {
    /// The step of the run that asked for it.
    pub step_id: String,
    pub tool: String,
    /// What the change does, in words ("Add task Re-run baselines").
    pub label: String,
    pub tier: RiskTier,
    /// The arguments it would run with.
    pub args: Value,
    /// The tool's own approval preview.
    pub preview: Value,
}

/// The changes a session saved, in the order they were asked for.
#[derive(Debug, Default)]
pub struct DeferredCalls(Mutex<Vec<DeferredCall>>);

impl DeferredCalls {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, call: DeferredCall) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(call);
    }

    /// The changes saved so far, leaving none.
    pub fn take(&self) -> Vec<DeferredCall> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// A tool whose changes wait for the user's OK (see the module documentation).
pub struct DeferredTool {
    inner: Arc<dyn HostTool>,
    saved: Arc<DeferredCalls>,
    description: String,
}

impl DeferredTool {
    pub fn new(inner: Arc<dyn HostTool>, saved: Arc<DeferredCalls>) -> Self {
        let description = if changes_anything(inner.as_ref()) {
            format!(
                "{} In this automation it does not run: the change is saved for the user's OK.",
                inner.description()
            )
        } else {
            inner.description().to_string()
        };
        Self {
            inner,
            saved,
            description,
        }
    }
}

/// Whether a chat in Ask mode would ask before a call of `tool` (its tier alone).
fn changes_anything(tool: &dyn HostTool) -> bool {
    let class = ToolClass::of(
        tool.tier(),
        tool.edits_locally(),
        tool.changes_files_unasked() || tool.changes_state(),
    );
    modes::decide(AgentMode::Ask, class) != Gate::Run
}

#[allow(clippy::double_must_use)]
#[async_trait]
impl HostTool for DeferredTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn label(&self) -> &str {
        self.inner.label()
    }
    fn label_template(&self) -> &str {
        self.inner.label_template()
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn schema(&self) -> Value {
        self.inner.schema()
    }
    /// Read: the registry never asks about it, because nothing it is called with changes
    /// anything now.
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    fn load_mode(&self) -> ToolLoadMode {
        self.inner.load_mode()
    }
    fn session_grant(&self, _args: &Value) -> Option<SessionGrant> {
        None
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        if !changes_anything(self.inner.as_ref()) {
            if self.inner.must_confirm_in(&args, ctx).await? {
                return Err(ToolError::Forbidden(NOT_IN_AN_AUTOMATION.to_string()));
            }
            return self.inner.execute(args, ctx).await;
        }
        let ApprovalPreview { label, details } = self.inner.preview_in(&args, ctx).await?;
        let label = label.unwrap_or_else(|| render_label(self.inner.label_template(), &args));
        self.saved.push(DeferredCall {
            step_id: ctx.step_id.clone(),
            tool: self.inner.name().to_string(),
            label: label.clone(),
            tier: self.inner.tier(),
            args,
            preview: details,
        });
        Ok(ToolOutput {
            text_for_model: format!("{label}: {SAVED_FOR_OK}"),
            summary_for_ui: format!("Saved for your OK: {label}"),
            detail: Some(json!({ "deferred": true, "label": label })),
        })
    }
}

/// `base` with every tool wrapped in a [`DeferredTool`] saving into `saved`.
pub fn deferring_registry(base: &ToolRegistry, saved: &Arc<DeferredCalls>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for registered in &base.tools {
        let tool: Arc<dyn HostTool> =
            Arc::new(DeferredTool::new(registered.tool.clone(), saved.clone()));
        let name = tool.name().to_string();
        if let Err(e) = registry.register(tool) {
            tracing::warn!(target: "shodh::harness", tool = %name, error = %e, "tool left out of the automation");
        }
    }
    registry
}

impl ToolRegistry {
    /// Makes one change the user approved, which an automation saved earlier: `tool` with
    /// `args`, checked against its schema again (a change whose target is gone fails
    /// alone). The approval (the user's, as `who`) and the call are audited as a chat's
    /// approved call is.
    pub async fn run_approved(
        &self,
        tool: &str,
        args: Value,
        label: &str,
        who: &str,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let registered = self
            .get(tool)
            .ok_or_else(|| ToolError::UnknownTool(tool.to_string()))?;
        let reasons: Vec<String> = registered
            .validator
            .iter_errors(&args)
            .map(|e| e.to_string())
            .collect();
        if !reasons.is_empty() {
            return Err(ToolError::InvalidArguments {
                tool: tool.to_string(),
                reasons: reasons.join("; "),
            });
        }
        let tier = registered.tool.tier();
        ctx.audit(
            AuditEventType::Approval,
            audit_payload::approval(tool, label, tier, ApprovalDecision::Approved, who),
        );
        let result = registered.tool.execute(args.clone(), ctx).await;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (ok, summary) = match &result {
            Ok(output) => (true, output.summary_for_ui.clone()),
            Err(error) => (false, error.summary()),
        };
        ctx.audit(
            AuditEventType::ToolCall,
            audit_payload::tool_call(tool, Some(tier), &args, ok, &summary, duration_ms),
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::profile::AgentProfile;
    use crate::harness::tools::{ApprovalGate, ToolCall};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc;

    /// A tool of one tier that counts its runs.
    struct Probe {
        name: &'static str,
        tier: RiskTier,
        local_edit: bool,
        confirm: bool,
        runs: AtomicUsize,
    }

    impl Probe {
        fn new(name: &'static str, tier: RiskTier) -> Arc<Self> {
            Arc::new(Self {
                name,
                tier,
                local_edit: false,
                confirm: false,
                runs: AtomicUsize::new(0),
            })
        }
    }

    #[allow(clippy::double_must_use)]
    #[async_trait]
    impl HostTool for Probe {
        fn name(&self) -> &str {
            self.name
        }
        fn label(&self) -> &str {
            "Probe"
        }
        fn label_template(&self) -> &str {
            "Probing {target}"
        }
        fn description(&self) -> &str {
            "Probes a target."
        }
        fn schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": { "target": { "type": "string" } },
                "required": ["target"],
                "additionalProperties": false
            })
        }
        fn tier(&self) -> RiskTier {
            self.tier
        }
        fn edits_locally(&self) -> bool {
            self.local_edit
        }
        async fn must_confirm(&self, _args: &Value) -> Result<bool, ToolError> {
            Ok(self.confirm)
        }
        async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
            Ok(ApprovalPreview {
                label: Some(format!("Change {}", args["target"].as_str().unwrap_or(""))),
                details: json!({ "target": args["target"] }),
            })
        }
        async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput {
                text_for_model: format!("probed {}", args["target"]),
                summary_for_ui: "probed".into(),
                detail: None,
            })
        }
    }

    fn ctx() -> ToolContext {
        let (tx, _rx) = mpsc::unbounded_channel();
        ToolContext::new("run-1", "step-1", tx)
    }

    fn profile_for(names: &[&str], mode: AgentMode) -> AgentProfile {
        AgentProfile {
            id: "automation".into(),
            name: "Automation".into(),
            instructions: String::new(),
            allowed_tools: names.iter().map(|n| n.to_string()).collect(),
            mode,
            max_tool_calls: 3,
        }
    }

    async fn call(registry: &ToolRegistry, profile: &AgentProfile, tool: &str) -> (bool, String) {
        let outcome = registry
            .dispatch(
                ToolCall {
                    tool: tool.to_string(),
                    args: json!({ "target": "db" }),
                    call_index: 1,
                },
                profile,
                &ApprovalGate::default(),
                &ctx(),
            )
            .await;
        (outcome.ok, outcome.text_for_model)
    }

    #[tokio::test]
    async fn every_change_is_saved_and_never_runs_in_every_mode() {
        let modes = [
            AgentMode::Ask,
            AgentMode::AcceptEdits,
            AgentMode::Plan,
            AgentMode::Auto,
            AgentMode::Creator,
        ];
        for mode in modes {
            let write = Probe::new("write_probe", RiskTier::Write);
            let destructive = Probe::new("delete_probe", RiskTier::Destructive);
            let local = Arc::new(Probe {
                local_edit: true,
                ..Probe::new_inner("edit_probe", RiskTier::Write)
            });
            let mut base = ToolRegistry::new();
            base.register(write.clone()).unwrap();
            base.register(destructive.clone()).unwrap();
            base.register(local.clone()).unwrap();
            let saved = Arc::new(DeferredCalls::new());
            let registry = deferring_registry(&base, &saved);
            let profile = profile_for(&["write_probe", "delete_probe", "edit_probe"], mode);
            for tool in ["write_probe", "delete_probe", "edit_probe"] {
                let (ok, text) = call(&registry, &profile, tool).await;
                assert!(ok, "{mode:?} {tool}: {text}");
                assert!(text.contains(SAVED_FOR_OK), "{mode:?} {tool}: {text}");
            }
            assert_eq!(write.runs.load(Ordering::SeqCst), 0, "{mode:?}");
            assert_eq!(destructive.runs.load(Ordering::SeqCst), 0, "{mode:?}");
            assert_eq!(local.runs.load(Ordering::SeqCst), 0, "{mode:?}");
            let calls = saved.take();
            assert_eq!(calls.len(), 3, "{mode:?}");
            assert_eq!(calls[0].tool, "write_probe");
            assert_eq!(calls[0].label, "Change db");
            assert_eq!(calls[0].args, json!({ "target": "db" }));
            assert_eq!(calls[0].preview, json!({ "target": "db" }));
            assert_eq!(calls[1].tier, RiskTier::Destructive);
            assert!(saved.take().is_empty(), "taken once");
        }
    }

    #[tokio::test]
    async fn reads_run_and_a_read_that_would_ask_is_refused() {
        let read = Probe::new("read_probe", RiskTier::Read);
        let asking = Arc::new(Probe {
            confirm: true,
            ..Probe::new_inner("web_probe", RiskTier::Read)
        });
        let mut base = ToolRegistry::new();
        base.register(read.clone()).unwrap();
        base.register(asking.clone()).unwrap();
        let saved = Arc::new(DeferredCalls::new());
        let registry = deferring_registry(&base, &saved);
        let profile = profile_for(&["read_probe", "web_probe"], AgentMode::Ask);
        let (ok, text) = call(&registry, &profile, "read_probe").await;
        assert!(ok);
        assert_eq!(text, "probed \"db\"");
        assert_eq!(read.runs.load(Ordering::SeqCst), 1);
        let (ok, text) = call(&registry, &profile, "web_probe").await;
        assert!(!ok);
        assert_eq!(text, NOT_IN_AN_AUTOMATION);
        assert_eq!(asking.runs.load(Ordering::SeqCst), 0);
        assert!(saved.take().is_empty());
    }

    #[tokio::test]
    async fn an_approved_change_runs_once_with_its_saved_arguments() {
        let write = Probe::new("write_probe", RiskTier::Write);
        let mut base = ToolRegistry::new();
        base.register(write.clone()).unwrap();
        let out = base
            .run_approved(
                "write_probe",
                json!({ "target": "db" }),
                "Change db",
                "local",
                &ctx(),
            )
            .await
            .unwrap();
        assert_eq!(out.text_for_model, "probed \"db\"");
        assert_eq!(write.runs.load(Ordering::SeqCst), 1);
        let refused = base
            .run_approved(
                "write_probe",
                json!({ "other": 1 }),
                "Change",
                "local",
                &ctx(),
            )
            .await;
        assert!(matches!(refused, Err(ToolError::InvalidArguments { .. })));
        assert!(matches!(
            base.run_approved("gone", json!({}), "x", "local", &ctx())
                .await,
            Err(ToolError::UnknownTool(_))
        ));
        assert_eq!(write.runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_change_is_described_as_saved_for_the_users_ok() {
        let saved = Arc::new(DeferredCalls::new());
        let write = DeferredTool::new(Probe::new("write_probe", RiskTier::Write), saved.clone());
        assert!(write.description().ends_with("saved for the user's OK."));
        assert_eq!(write.tier(), RiskTier::Read);
        let read = DeferredTool::new(Probe::new("read_probe", RiskTier::Read), saved);
        assert_eq!(read.description(), "Probes a target.");
    }

    impl Probe {
        fn new_inner(name: &'static str, tier: RiskTier) -> Self {
            Self {
                name,
                tier,
                local_edit: false,
                confirm: false,
                runs: AtomicUsize::new(0),
            }
        }
    }
}
