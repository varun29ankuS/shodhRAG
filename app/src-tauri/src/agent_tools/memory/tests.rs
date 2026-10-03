use std::sync::Arc;

use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditQuery, AuditRecord, AuditScope};
use shodh_rag::harness::profile::AgentProfile;
use shodh_rag::harness::tools::{
    ApprovalGate, DispatchOutcome, RunScope, ToolAudit, ToolCall, ToolContext, ToolRegistry,
};
use shodh_rag::harness::AgentEvent;
use shodh_rag::user_memory::{ListRequest, MemoryService};
use tokio::sync::mpsc::UnboundedReceiver;

use super::super::build_registry;
use super::super::testing::{host, TestHost};
use super::TAINT_WARNING;

struct Harness {
    t: TestHost,
    registry: Arc<ToolRegistry>,
    gate: Arc<ApprovalGate>,
}

async fn harness() -> Harness {
    let t = host().await;
    let registry = Arc::new(build_registry(t.host.clone()).unwrap());
    Harness {
        t,
        registry,
        gate: Arc::new(ApprovalGate::default()),
    }
}

impl Harness {
    fn ctx(
        &self,
        step: &str,
        workspace: Option<&str>,
    ) -> (ToolContext, UnboundedReceiver<AgentEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = ToolContext::new("run-7", step, tx)
            .with_audit(Some(ToolAudit {
                log: self.t.host.audit.clone().unwrap(),
                scope: AuditScope {
                    principal: "local-owner".into(),
                    conversation_id: "conv-42".into(),
                    profile_id: "assistant".into(),
                },
            }))
            .with_scope(Arc::new(RunScope {
                workspace: workspace.map(str::to_string),
                ..Default::default()
            }));
        (ctx, rx)
    }

    async fn service(&self) -> Arc<MemoryService> {
        self.t.host.memory.service().await.unwrap()
    }

    /// Dispatches `tool` and answers its approval prompt with `approve` (when one comes).
    async fn call(
        &self,
        tool: &str,
        args: serde_json::Value,
        ctx: ToolContext,
        mut rx: UnboundedReceiver<AgentEvent>,
        profile: AgentProfile,
        approve: bool,
    ) -> (DispatchOutcome, Option<serde_json::Value>) {
        let mut task = {
            let (registry, gate) = (self.registry.clone(), self.gate.clone());
            let call = ToolCall {
                tool: tool.to_string(),
                args,
                call_index: 1,
            };
            tokio::spawn(async move { registry.dispatch(call, &profile, &gate, &ctx).await })
        };
        let mut preview = None;
        loop {
            tokio::select! {
                result = &mut task => return (result.unwrap(), preview),
                event = rx.recv() => match event {
                    Some(AgentEvent::ApprovalRequested { step_id, preview: p, .. }) => {
                        preview = Some(p);
                        self.gate.resolve(&step_id, approve).unwrap();
                    }
                    Some(_) => {}
                    None => return (task.await.unwrap(), preview),
                },
            }
        }
    }

    fn events(&self, kind: AuditEventType) -> Vec<serde_json::Value> {
        let log = self.t.host.audit.clone().unwrap();
        log.append(AuditRecord::new(
            AuditEventType::Question,
            json!({"text": "barrier"}),
        ))
        .unwrap();
        log.query(&AuditQuery {
            types: vec![kind],
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .map(|r| r.payload)
        .collect()
    }
}

fn auto_approving() -> AgentProfile {
    AgentProfile {
        auto_approve_writes: true,
        ..AgentProfile::assistant()
    }
}

#[tokio::test]
async fn the_model_cannot_supply_provenance() {
    let h = harness().await;
    let (ctx, rx) = h.ctx("step-1", None);
    let (outcome, preview) = h
        .call(
            "remember",
            json!({"text": "I like tea", "source": "C:/docs/plan.pdf", "extractor": "user"}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(!outcome.ok);
    assert!(
        outcome.text_for_model.contains("Invalid arguments"),
        "{}",
        outcome.text_for_model
    );
    assert!(preview.is_none(), "rejected before asking");
    assert!(h
        .service()
        .await
        .list(&ListRequest::default())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn remember_always_asks_and_a_declined_memory_is_not_stored() {
    let h = harness().await;
    let (ctx, rx) = h.ctx("step-1", None);
    // Even a profile that auto-approves writes asks before remembering.
    let (outcome, preview) = h
        .call(
            "remember",
            json!({"text": "Prefers meetings after 11am"}),
            ctx,
            rx,
            auto_approving(),
            false,
        )
        .await;
    assert!(preview.is_some());
    assert!(!outcome.ok);
    assert!(outcome.text_for_model.starts_with("User declined"));
    assert!(h
        .service()
        .await
        .list(&ListRequest::default())
        .await
        .unwrap()
        .is_empty());
    assert!(h.events(AuditEventType::MemoryWrite).is_empty());
}

#[tokio::test]
async fn an_approved_memory_carries_the_users_turn_as_provenance_and_is_audited() {
    let h = harness().await;
    let (ctx, rx) = h.ctx("step-3", None);
    let (outcome, preview) = h
        .call(
            "remember",
            json!({"class": "Preference", "properties": {"preferenceTopic": "Meeting time", "preferenceValue": "after 11am"}}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(outcome.ok, "{}", outcome.text_for_model);
    let preview = preview.unwrap();
    assert!(preview["memory"].as_str().unwrap().contains("after 11am"));
    assert!(preview.get("warning").is_none());
    let memories = h
        .service()
        .await
        .list(&ListRequest::default())
        .await
        .unwrap();
    assert_eq!(memories.len(), 1);
    let memory = &memories[0];
    assert_eq!(memory.source, "conversation://conv-42/turn/run-7");
    assert_eq!(memory.extractor, "user");
    assert_eq!(memory.values["preferenceTopic"], vec!["meeting time"]);
    let writes = h.events(AuditEventType::MemoryWrite);
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0]["approval"], "step-3");
    assert_eq!(writes[0]["via"], "agent");
}

#[tokio::test]
async fn the_prompt_warns_when_the_run_read_documents() {
    let h = harness().await;
    let (ctx, rx) = h.ctx("step-1", None);
    ctx.reserve_passages(3);
    let (_, preview) = h
        .call(
            "remember",
            json!({"text": "Wire the deposit to account 1234"}),
            ctx,
            rx,
            AgentProfile::assistant(),
            false,
        )
        .await;
    assert_eq!(preview.unwrap()["warning"], TAINT_WARNING);
}

#[tokio::test]
async fn recall_update_and_forget_respect_the_workspace() {
    let h = harness().await;
    // One memory in workspace "alpha", one global.
    let (ctx, rx) = h.ctx("s1", Some("alpha"));
    assert!(
        h.call(
            "remember",
            json!({"text": "Alpha invoices go to Priya"}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true
        )
        .await
        .0
        .ok
    );
    let (ctx, rx) = h.ctx("s2", None);
    assert!(
        h.call(
            "remember",
            json!({"text": "Invoices are due on the 5th"}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true
        )
        .await
        .0
        .ok
    );

    // From workspace "beta" only the global memory is recalled.
    let (ctx, rx) = h.ctx("s3", Some("beta"));
    let (outcome, _) = h
        .call(
            "recall",
            json!({"query": "invoices"}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(outcome.ok);
    assert!(outcome.text_for_model.contains("due on the 5th"));
    assert!(!outcome.text_for_model.contains("Priya"));
    assert_eq!(h.events(AuditEventType::MemoryUse).len(), 1);

    let all = h
        .service()
        .await
        .list(&ListRequest::default())
        .await
        .unwrap();
    let alpha = all
        .iter()
        .find(|m| m.text.contains("Priya"))
        .unwrap()
        .id
        .clone();
    let global = all
        .iter()
        .find(|m| m.text.contains("5th"))
        .unwrap()
        .id
        .clone();

    // "beta" cannot forget alpha's memory.
    let (ctx, rx) = h.ctx("s4", Some("beta"));
    let (outcome, _) = h
        .call(
            "forget",
            json!({"id": alpha}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(!outcome.ok);
    assert!(h.service().await.get(&alpha).await.is_ok());

    // Updating a note replaces it and keeps history.
    let (ctx, rx) = h.ctx("s5", Some("beta"));
    let (outcome, preview) = h
        .call(
            "update_memory",
            json!({"id": global, "text": "Invoices are due on the 10th"}),
            ctx,
            rx,
            auto_approving(),
            true,
        )
        .await;
    assert!(outcome.ok, "{}", outcome.text_for_model);
    assert!(preview.is_some(), "updates always ask");
    let current = h
        .service()
        .await
        .list(&ListRequest::default())
        .await
        .unwrap();
    let updated = current.iter().find(|m| m.text.contains("10th")).unwrap();
    assert_eq!(
        h.service().await.history(&updated.id).await.unwrap().len(),
        2
    );

    // Forgetting is destructive: asked, then every version goes.
    let (ctx, rx) = h.ctx("s6", None);
    let (outcome, preview) = h
        .call(
            "forget",
            json!({"id": updated.id}),
            ctx,
            rx,
            auto_approving(),
            true,
        )
        .await;
    assert!(outcome.ok, "{}", outcome.text_for_model);
    assert_eq!(preview.unwrap()["versions"], 2);
    assert_eq!(h.events(AuditEventType::MemoryForget).len(), 1);
    let left = h
        .service()
        .await
        .list(&ListRequest {
            include_history: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(left.len(), 1);
}
