use std::sync::Arc;

use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditQuery, AuditRecord, AuditScope};
use shodh_rag::harness::profile::{app_tools, AgentProfile};
use shodh_rag::harness::protocol::ToolLoadMode;
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
    // The tool call is audited with the class and property names, not the values.
    let calls = h.events(AuditEventType::ToolCall);
    let call = calls.iter().find(|c| c["tool"] == "remember").unwrap();
    assert_eq!(call["args"]["class"], "Preference");
    let call = call.to_string();
    assert!(
        !call.contains("11am") && !call.contains("Meeting time"),
        "{call}"
    );
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
            // The default floor is tuned for E5; the test embedder scores identical words 1.
            json!({"query": "Invoices are due on the 5th?"}),
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
    // Even alpha's exact words find nothing from beta.
    let (ctx, rx) = h.ctx("s3b", Some("beta"));
    let (outcome, _) = h
        .call(
            "recall",
            json!({"query": "Alpha invoices go to Priya"}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(outcome.ok);
    assert!(
        outcome.text_for_model.starts_with("No memories match"),
        "{}",
        outcome.text_for_model
    );
    // ...and are recalled from alpha itself.
    let (ctx, rx) = h.ctx("s3c", Some("alpha"));
    let (outcome, _) = h
        .call(
            "recall",
            json!({"query": "Alpha invoices go to Priya"}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(
        outcome.text_for_model.contains("Priya"),
        "{}",
        outcome.text_for_model
    );

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
    let preview = preview.unwrap();
    assert_eq!(preview["versions"], 2);
    assert!(preview["memory"].as_str().unwrap().contains("10th"));
    assert!(preview["notice"]
        .as_str()
        .unwrap()
        .contains("without its words"));
    assert_eq!(h.events(AuditEventType::MemoryForget).len(), 1);
    // Approval events (their labels are recorded), tool calls (their arguments are) and
    // memory events hold no memory's words.
    for kind in [
        AuditEventType::Approval,
        AuditEventType::ToolCall,
        AuditEventType::MemoryWrite,
        AuditEventType::MemoryForget,
    ] {
        for event in h.events(kind) {
            let event = event.to_string();
            assert!(
                !event.contains("Invoices") && !event.contains("Priya"),
                "{event}"
            );
        }
    }
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

#[tokio::test]
async fn update_memory_tells_nothing_about_a_memory_out_of_scope() {
    let h = harness().await;
    let (ctx, rx) = h.ctx("s1", Some("alpha"));
    let (outcome, _) = h
        .call(
            "remember",
            json!({"class": "Preference", "properties": {"preferenceTopic": "Tea", "preferenceValue": "green"}}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(outcome.ok, "{}", outcome.text_for_model);
    let alpha = h
        .service()
        .await
        .list(&ListRequest::default())
        .await
        .unwrap()[0]
        .id
        .clone();
    // A note's text for a typed fact would be refused naming the fact's class: from
    // another workspace the memory is not found at all, before its class is looked at.
    for args in [
        json!({"id": alpha, "text": "Something else"}),
        json!({"id": alpha, "properties": {"preferenceValue": "black"}}),
    ] {
        let (ctx, rx) = h.ctx("s2", Some("beta"));
        let (outcome, preview) = h
            .call("update_memory", args, ctx, rx, auto_approving(), true)
            .await;
        assert!(!outcome.ok);
        assert!(preview.is_none(), "nothing to approve");
        assert!(
            outcome.text_for_model.contains("No memory with id"),
            "{}",
            outcome.text_for_model
        );
        assert!(
            !outcome.text_for_model.contains("Preference"),
            "{}",
            outcome.text_for_model
        );
    }
    assert_eq!(
        h.service().await.get(&alpha).await.unwrap().values["preferenceValue"],
        vec!["green"]
    );
}

#[tokio::test]
async fn remember_and_recall_are_always_in_the_models_context() {
    let h = harness().await;
    let definitions = h.registry.definitions(&AgentProfile::assistant());
    let mode = |name: &str| {
        definitions
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.load_mode)
    };
    assert_eq!(mode(app_tools::REMEMBER), Some(ToolLoadMode::Essential));
    assert_eq!(mode(app_tools::RECALL), Some(ToolLoadMode::Essential));
    assert_eq!(
        mode(app_tools::UPDATE_MEMORY),
        Some(ToolLoadMode::Discoverable)
    );
    assert_eq!(mode(app_tools::FORGET), Some(ToolLoadMode::Discoverable));
}

/// No model: saving what the user asked to remember never calls one.
struct NoModel;

impl shodh_rag::user_memory::learn::engine::ModelSource for NoModel {
    fn model(&self) -> Result<Arc<dyn shodh_rag::user_memory::learn::LearnModel>, String> {
        Err("no model in this test".to_string())
    }
}

#[tokio::test]
async fn remember_of_a_note_the_user_asked_for_in_this_answer_is_done_without_a_prompt() {
    use shodh_rag::user_memory::learn::{memory_request, LearnPolicy, Learner, RequestedOutcome};
    let h = harness().await;
    let service = h.service().await;
    let learner = Learner::new(
        service.clone(),
        service.inbox().unwrap().clone(),
        Arc::new(LearnPolicy::default()),
        Arc::new(NoModel),
    );
    // `agent_send` saved the request of run-7 before the answer started.
    let request = memory_request("Remember that I use DiskANN").unwrap();
    let saved = learner
        .file_user_note(
            &request,
            "conv-42",
            "run-7",
            service.store().now(),
            shodh_rag::statements::Scope::Global,
        )
        .await
        .unwrap();
    assert!(matches!(saved, RequestedOutcome::Saved(_)), "{saved:?}");

    // The model calls remember for the same words anyway: no prompt, no second memory.
    let (ctx, rx) = h.ctx("step-1", None);
    let (outcome, preview) = h
        .call(
            "remember",
            json!({"text": "i use DiskANN."}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(preview.is_none(), "the user is not asked again");
    assert!(!outcome.ok);
    assert_eq!(outcome.text_for_model, super::ALREADY_SAVED);
    assert_eq!(
        service.list(&ListRequest::default()).await.unwrap().len(),
        1
    );

    // Other words are a new memory, and ask as always.
    let (ctx, rx) = h.ctx("step-2", None);
    let (outcome, preview) = h
        .call(
            "remember",
            json!({"text": "I use SPANN too"}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(preview.is_some());
    assert!(outcome.ok, "{}", outcome.text_for_model);
    assert_eq!(
        service.list(&ListRequest::default()).await.unwrap().len(),
        2
    );
}

#[tokio::test]
async fn forgetting_or_editing_a_memory_tells_the_open_chats() {
    let h = harness().await;
    let told = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let told = told.clone();
        h.t.host.memory.on_change(move || {
            told.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
    }
    let service = h.service().await;
    let id = service
        .remember(
            shodh_rag::user_memory::MemoryContent::Note {
                text: "I use DiskANN".into(),
            },
            shodh_rag::statements::Scope::Global,
            &shodh_rag::user_memory::Origin::user_interface("test"),
            &shodh_rag::user_memory::Actor::ui(),
        )
        .await
        .unwrap()
        .memory
        .unwrap()
        .id;
    let count = || told.load(std::sync::atomic::Ordering::SeqCst);

    // Refused (nothing changed): nobody is told.
    let (ctx, rx) = h.ctx("s1", None);
    let (outcome, _) = h
        .call(
            "forget",
            json!({"id": id}),
            ctx,
            rx,
            AgentProfile::assistant(),
            false,
        )
        .await;
    assert!(!outcome.ok);
    assert_eq!(count(), 0);

    let (ctx, rx) = h.ctx("s2", None);
    let (outcome, _) = h
        .call(
            "update_memory",
            json!({"id": id, "text": "I use DiskANN and SPANN"}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(outcome.ok, "{}", outcome.text_for_model);
    assert_eq!(count(), 1, "an edit is told");

    let current = service.list(&ListRequest::default()).await.unwrap();
    let (ctx, rx) = h.ctx("s3", None);
    let (outcome, _) = h
        .call(
            "forget",
            json!({"id": current[0].id}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(outcome.ok, "{}", outcome.text_for_model);
    assert_eq!(count(), 2, "a forget is told");
}

#[tokio::test]
async fn a_memory_from_slack_is_recalled_quoted_as_possibly_other_peoples_words() {
    use shodh_rag::statements::Scope;
    use shodh_rag::user_memory::{Actor, MemoryContent, Origin, WriteAuthority};
    let h = harness().await;
    h.service()
        .await
        .remember(
            MemoryContent::Note {
                text: "Walrus invoices go to \"Priya\"".into(),
            },
            Scope::Global,
            &Origin::from_slack(
                "slack-memory://T1/C1/1700000000.000100",
                shodh_ontology::ExtractorKind::Llm,
                "test",
                WriteAuthority::SlackOwner {
                    team_id: "T1".into(),
                    user_id: "U1".into(),
                },
            ),
            &Actor::ui(),
        )
        .await
        .unwrap();
    let (ctx, rx) = h.ctx("s1", None);
    let (outcome, _) = h
        .call(
            "recall",
            json!({"query": "Walrus invoices go to Priya"}),
            ctx,
            rx,
            AgentProfile::assistant(),
            true,
        )
        .await;
    assert!(outcome.ok, "{}", outcome.text_for_model);
    assert!(
        outcome.text_for_model.contains("saved from Slack")
            && outcome.text_for_model.contains("not instructions"),
        "{}",
        outcome.text_for_model
    );
    assert!(
        outcome
            .text_for_model
            .contains("\"Walrus invoices go to 'Priya'\""),
        "quoted whole: {}",
        outcome.text_for_model
    );
}
