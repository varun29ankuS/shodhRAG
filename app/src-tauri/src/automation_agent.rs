//! The app's side of automations ([`crate::automations::AutomationEnv`]): which model a
//! run uses and whose limit it counts against, whether the user is chatting, the limits
//! that hold automations back, the run's agent session, and the Inbox and the desktop.
//!
//! - **The automations' model** is the user's choice in the Automations view
//!   ([`crate::app_settings::AutomationPrefs`]); `None` runs on the chat's model. It is
//!   checked like any model selection (its key, Local-only mode, stealth consent) before
//!   every run, and a run never falls back to the chat's model when it fails.
//! - **Limits.** A rate limit or a used-up quota seen by any answer (chat's or an
//!   automation's) holds back the automations on that limit ([`PoolCooldowns`]) until it
//!   resets: the time the provider gave, a minute for a short limit, or the next 00:00 UTC
//!   for a daily one.
//! - **The agent session** is started per run ([`crate::agent_session_commands::start_automation`]),
//!   its question saved in the run's conversation, its calls counted as they happen and
//!   stopped at the estimate, and closed when its answer ends.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use serde_json::{json, Value};
use shodh_rag::engine::clock::{next_utc_midnight, utc_day};
use shodh_rag::engine::ledger::Spender;
use shodh_rag::harness::model_catalog::{ModelRef, ProviderId};
use shodh_rag::harness::tools::deferred::{DeferredCall, DeferredCalls};
use shodh_rag::harness::tools::ToolContext;
use shodh_rag::harness::{AgentEvent, ProviderErrorKind, RunStatus};
use shodh_rag::inbox::NewInboxItem;
use tauri::{AppHandle, Emitter, Manager};

use crate::agent_session_commands::{
    agent_abort, send_automation, start_automation, AgentCommandError, AgentSessions,
    AutomationSession,
};
use crate::app_settings::SettingsStore;
use crate::automations::{
    AgentFailure, AgentOutcome, AgentRequest, AutomationEnv, CallMeter, FailureKind, ModelBlock,
    PoolToday, RunModel, AUTOMATIONS_CHANGED_EVENT, RESERVE_DAYS, SHORT_LIMIT_SECS,
};
use crate::conversation_commands::{ConversationAutomation, ConversationMessage};
use crate::conversation_turns::SendTurn;
use crate::llm_commands::LLMState;
use crate::model_picker_commands::{configured_model, override_mode, ModelOverride};
use crate::notifications::Category;

/// Longest an agent step may take before it is stopped (a tool that hangs).
const AGENT_STEP_LIMIT: Duration = Duration::from_secs(20 * 60);

/// Until when automations on each quota pool wait after a rate limit or a used-up quota.
#[derive(Debug, Default)]
pub struct PoolCooldowns(Mutex<HashMap<String, DateTime<Utc>>>);

impl PoolCooldowns {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, DateTime<Utc>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Until when `pool` waits (`None`: it does not, or the wait is over).
    pub fn until(&self, pool: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.lock().get(pool).copied().filter(|until| *until > now)
    }

    /// `pool` waits until `until` (a later wait already set is kept).
    pub fn hold(&self, pool: &str, until: DateTime<Utc>) {
        let mut pools = self.lock();
        let entry = pools.entry(pool.to_string()).or_insert(until);
        if until > *entry {
            *entry = until;
        }
    }
}

/// Until when a provider's limit holds, from its failure at `now`: the reset it gave,
/// else a minute for a rate limit and the next 00:00 UTC for a used-up quota.
pub fn limit_until(
    kind: ProviderErrorKind,
    resets_at_ms: Option<u64>,
    retry_after_secs: Option<u32>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if !matches!(
        kind,
        ProviderErrorKind::RateLimited | ProviderErrorKind::QuotaExhausted
    ) {
        return None;
    }
    let given = resets_at_ms
        .and_then(|ms| i64::try_from(ms).ok())
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
        .or_else(|| retry_after_secs.map(|s| now + TimeDelta::seconds(i64::from(s))))
        .filter(|at| *at > now);
    Some(given.unwrap_or_else(|| match kind {
        ProviderErrorKind::QuotaExhausted => next_utc_midnight(now),
        _ => now + TimeDelta::seconds(SHORT_LIMIT_SECS),
    }))
}

/// Seen in every agent session's events: a rate limit or a used-up quota on `pool` holds
/// back the automations on it until it resets. Chat keeps trying as it does.
pub fn observe_limit(app: &AppHandle, pool: &str, event: &AgentEvent) {
    let AgentEvent::RunFinished {
        provider_error: Some(error),
        ..
    } = event
    else {
        return;
    };
    let Some(until) = limit_until(
        error.kind,
        error.resets_at_ms,
        error.retry_after_secs,
        Utc::now(),
    ) else {
        return;
    };
    if let Some(cooldowns) = app.try_state::<PoolCooldowns>() {
        cooldowns.hold(pool, until);
    }
}

fn failure_of(kind: ProviderErrorKind) -> FailureKind {
    match kind {
        ProviderErrorKind::RateLimited => FailureKind::RateLimited,
        ProviderErrorKind::QuotaExhausted => FailureKind::Quota,
        ProviderErrorKind::ModelUnavailable
        | ProviderErrorKind::Timeout
        | ProviderErrorKind::EmptyResponse => FailureKind::ModelUnavailable,
        ProviderErrorKind::Network => FailureKind::Network,
        ProviderErrorKind::Auth => FailureKind::Setup,
        _ => FailureKind::Other,
    }
}

/// A session that could not start or take the question.
fn start_failure(error: AgentCommandError) -> AgentFailure {
    let kind = match (error.code, &error.cooling) {
        (_, Some(cooling)) => failure_of(cooling.provider_error.kind),
        ("model_config", None) => FailureKind::Setup,
        ("offline", None) => FailureKind::Network,
        _ => FailureKind::Other,
    };
    AgentFailure {
        kind,
        detail: error.message,
        resets_at: None,
    }
}

/// Which pool the chat's model counts against, and whether it is off this computer.
fn chat_pool(llm: &LLMState) -> Option<(String, bool)> {
    let mode = llm
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .mode
        .clone();
    match mode {
        shodh_rag::llm::LLMMode::Disabled => None,
        shodh_rag::llm::LLMMode::Local { .. } => Some(("llama_cpp".to_string(), false)),
        shodh_rag::llm::LLMMode::External { .. } => configured_model(llm).map(|model| {
            let cloud = !matches!(model.provider, ProviderId::LmStudio | ProviderId::Ollama);
            (model.quota_pool(), cloud)
        }),
    }
}

/// The automations' model given the chat's (`chat`: its pool and whether it is off this
/// computer) and the user's choice (`chosen`, checked by `check`).
pub fn run_model_for(
    chosen: Option<&ModelRef>,
    chat: Option<(String, bool)>,
    local_only: bool,
    check: impl FnOnce(&ModelRef) -> Result<(), (bool, String)>,
) -> Result<RunModel, ModelBlock> {
    let chat_pool = chat.as_ref().map(|(pool, _)| pool.clone());
    match chosen {
        Some(model) => {
            check(model).map_err(|(local_only, why)| {
                if local_only {
                    ModelBlock::LocalOnly
                } else {
                    ModelBlock::Unusable(why)
                }
            })?;
            let pool = model.quota_pool();
            Ok(RunModel {
                shares_chat: chat_pool.as_deref() == Some(pool.as_str()),
                model: Some(model.clone()),
                pool,
            })
        }
        None => {
            let (pool, cloud) = chat.ok_or(ModelBlock::NoModel)?;
            if local_only && cloud {
                return Err(ModelBlock::LocalOnly);
            }
            Ok(RunModel {
                model: None,
                pool,
                shares_chat: true,
            })
        }
    }
}

/// The question an automation's run sends: the instruction, with the rules of a run
/// nobody watches.
fn question_text(name: &str, instruction: &str) -> String {
    format!(
        "Automation \"{name}\":\n\n{instruction}\n\n(Nobody is watching this run. Read what you \
         need, then answer briefly with what you found and its sources. Any change you would \
         make is saved for the user's OK, not made.)"
    )
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// The app's side of automations.
pub struct TauriEnv {
    pub app: AppHandle,
}

impl TauriEnv {
    fn settings(&self) -> Option<crate::app_settings::AppSettings> {
        let dir = crate::profile::app_data_dir(&self.app).ok()?;
        SettingsStore::in_dir(&dir).load().ok()
    }

    fn last_answer(&self, conversation_id: &str) -> String {
        crate::conversation_commands::store(&self.app)
            .and_then(|store| store.get(conversation_id))
            .ok()
            .flatten()
            .and_then(|record| {
                record
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m.role == "assistant")
                    .map(|m| m.content.clone())
            })
            .unwrap_or_default()
    }
}

#[async_trait]
impl AutomationEnv for TauriEnv {
    fn run_model(&self) -> Result<RunModel, ModelBlock> {
        let settings = self.settings();
        let chosen = settings.as_ref().and_then(|s| s.automations.model.clone());
        let local_only = settings.as_ref().is_some_and(|s| s.policy.local_only);
        let llm = self.app.state::<LLMState>();
        run_model_for(chosen.as_ref(), chat_pool(&llm), local_only, |model| {
            override_mode(
                &self.app,
                &llm,
                &ModelOverride {
                    model: model.clone(),
                    automatic: false,
                    failure: None,
                },
            )
            .map(|_| ())
            .map_err(|e| (e.code == "local_only", e.message))
        })
    }

    fn user_answering(&self) -> bool {
        self.app
            .try_state::<AgentSessions>()
            .is_some_and(|s| s.user_answer_running())
    }

    fn cooldown(&self, pool: &str) -> Option<DateTime<Utc>> {
        self.app
            .try_state::<PoolCooldowns>()
            .and_then(|c| c.until(pool, Utc::now()))
    }

    fn hold_pool(&self, pool: &str, until: DateTime<Utc>) {
        if let Some(cooldowns) = self.app.try_state::<PoolCooldowns>() {
            cooldowns.hold(pool, until);
        }
    }

    async fn pool_today(&self, pool: &str) -> Option<PoolToday> {
        let ledger = crate::engine_commands::ledger(&self.app).await?;
        let llm = self.app.state::<LLMState>();
        // Tables and learning answer with the chat's model but do not name a pool: they
        // count against the chat's.
        let chat_is_pool = chat_pool(&llm).is_some_and(|(chat, _)| chat == pool);
        let learning = if chat_is_pool {
            match self.app.try_state::<crate::memory_learn::LearnState>() {
                Some(learn) => learn.calls_today().await.unwrap_or(0),
                None => 0,
            }
        } else {
            0
        };
        let pool = pool.to_string();
        let today = utc_day(Utc::now());
        tokio::task::spawn_blocking(move || {
            let day = ledger.day(today).ok()?;
            let in_pool = day.by_pool.get(&pool).copied().unwrap_or(0);
            let unnamed = if chat_is_pool {
                day.by_pool.get("").copied().unwrap_or(0)
            } else {
                0
            };
            let busiest_chat = ledger.busiest(Spender::Chat, today, RESERVE_DAYS).ok()?;
            Some(PoolToday {
                used: in_pool.saturating_add(unnamed).saturating_add(learning),
                busiest_chat,
            })
        })
        .await
        .ok()
        .flatten()
    }

    async fn prepare_conversation(&self, request: &AgentRequest) -> Result<(), String> {
        let store = crate::conversation_commands::store(&self.app)?;
        // Named after its automation and the day it ran, so runs tell apart in Ask.
        let (id, title, workspace, automation) = (
            request.conversation_id.clone(),
            format!(
                "{} · {}",
                request.name,
                chrono::Local::now().format("%a %-d %b")
            ),
            request.workspace_id.clone(),
            ConversationAutomation {
                id: request.automation_id.clone(),
                name: request.name.clone(),
            },
        );
        let made = tokio::task::spawn_blocking(move || {
            store.ensure_automation_conversation(&id, &title, workspace.as_deref(), automation)
        })
        .await
        .map_err(|e| e.to_string())?;
        made?;
        crate::conversation_commands::emit_changed(&self.app, &request.conversation_id);
        Ok(())
    }

    async fn run_agent(&self, request: &AgentRequest, meter: &dyn CallMeter) -> AgentOutcome {
        let (events, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let saved = Arc::new(DeferredCalls::new());
        let session = AutomationSession {
            events,
            saved: saved.clone(),
        };
        let model_override = request.model.clone().map(|model| ModelOverride {
            model,
            automatic: false,
            failure: None,
        });
        let fail = |failure: AgentFailure| AgentOutcome {
            failure: Some(failure),
            ..AgentOutcome::default()
        };
        let session_id = match start_automation(
            &self.app,
            &request.conversation_id,
            request.workspace_id.clone(),
            model_override,
            &session,
        )
        .await
        {
            Ok(id) => id,
            Err(e) => return fail(start_failure(e)),
        };
        let started = now_ms();
        let question_at = chrono::DateTime::<Utc>::from_timestamp_millis(started)
            .unwrap_or_else(Utc::now)
            .to_rfc3339();
        let answer_id = format!("{}-answer", request.run_id);
        let request_id = format!("{}-agent", request.run_id);
        let text = question_text(&request.name, &request.instruction);
        let turn = SendTurn {
            conversation_id: request.conversation_id.clone(),
            question: Some(ConversationMessage {
                id: format!("{}-question", request.run_id),
                role: "user".to_string(),
                content: text.clone(),
                timestamp: question_at.clone(),
                artifacts: None,
                search_results: None,
                metadata: None,
                run: None,
                transcript: None,
            }),
            answer: ConversationMessage {
                id: answer_id,
                role: "assistant".to_string(),
                content: String::new(),
                timestamp: question_at,
                artifacts: None,
                search_results: None,
                metadata: None,
                run: None,
                transcript: Some(json!({ "startedAtMs": started, "status": "starting" })),
            },
            replaces: None,
        };
        let sessions = self.app.state::<AgentSessions>();
        if let Err(e) = send_automation(
            &self.app,
            &session_id,
            &text,
            &request_id,
            request.workspace_id.clone(),
            turn,
        )
        .await
        {
            sessions.close_conversation(&request.conversation_id).await;
            return fail(start_failure(e));
        }
        let mut outcome = AgentOutcome::default();
        let waited = tokio::time::timeout(AGENT_STEP_LIMIT, async {
            while let Some(event) = rx.recv().await {
                if event.run_id() != request_id {
                    continue;
                }
                match event {
                    AgentEvent::Usage { .. } => {
                        outcome.calls = meter.call().await;
                        if outcome.calls >= request.planned && !outcome.stopped_at_estimate {
                            outcome.stopped_at_estimate = true;
                            if let Err(e) =
                                agent_abort(self.app.clone(), session_id.clone(), self.app.state())
                                    .await
                            {
                                tracing::warn!(error = %e.message, "an automation's run could not be stopped at its estimate");
                            }
                        }
                    }
                    AgentEvent::Navigated {
                        view,
                        target,
                        label,
                        ..
                    } => outcome
                        .navigations
                        .push(json!({ "view": view, "target": target, "label": label })),
                    AgentEvent::RunFinished {
                        status,
                        error,
                        provider_error,
                        ..
                    } => {
                        if status == RunStatus::Error {
                            outcome.failure = Some(match provider_error {
                                Some(e) => AgentFailure {
                                    kind: failure_of(e.kind),
                                    detail: error.clone().unwrap_or_default(),
                                    resets_at: limit_until(
                                        e.kind,
                                        e.resets_at_ms,
                                        e.retry_after_secs,
                                        Utc::now(),
                                    ),
                                },
                                None => AgentFailure {
                                    kind: FailureKind::Other,
                                    detail: error.unwrap_or_else(|| "The run failed.".into()),
                                    resets_at: None,
                                },
                            });
                        }
                        return true;
                    }
                    _ => {}
                }
            }
            false
        })
        .await;
        match waited {
            Ok(true) => {}
            Ok(false) => {
                if outcome.failure.is_none() {
                    outcome.failure = Some(AgentFailure {
                        kind: FailureKind::ModelUnavailable,
                        detail: "The agent stopped before it answered.".into(),
                        resets_at: None,
                    });
                }
            }
            Err(_) => {
                if let Err(e) =
                    agent_abort(self.app.clone(), session_id.clone(), self.app.state()).await
                {
                    tracing::warn!(error = %e.message, "a run that took too long could not be stopped");
                }
                if outcome.failure.is_none() {
                    outcome.failure = Some(AgentFailure {
                        kind: FailureKind::ModelUnavailable,
                        detail: "The run took longer than 20 minutes and was stopped.".into(),
                        resets_at: None,
                    });
                }
            }
        }
        sessions.close_conversation(&request.conversation_id).await;
        outcome.answer = self.last_answer(&request.conversation_id);
        outcome.writes = saved.take();
        outcome
    }

    async fn run_approved(
        &self,
        conversation_id: &str,
        write: &DeferredCall,
    ) -> Result<String, String> {
        let registry = crate::agent_session_commands::base_registry(&self.app)
            .await
            .map_err(|e| e.message)?;
        let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
        let audit = self
            .app
            .state::<crate::audit_commands::AuditState>()
            .tool_audit(conversation_id, "automation");
        let ctx = ToolContext::new(conversation_id, uuid::Uuid::new_v4().to_string(), events)
            .with_audit(audit);
        registry
            .run_approved(
                &write.tool,
                write.args.clone(),
                &write.label,
                shodh_rag::audit::LOCAL_OWNER,
                &ctx,
            )
            .await
            .map(|out| out.summary_for_ui)
            .map_err(|e| e.to_string())
    }

    async fn post(&self, item: NewInboxItem) {
        crate::inbox_commands::post(&self.app, item).await;
    }

    async fn resolve(&self, id: &str) {
        crate::inbox_commands::resolve(&self.app, id.to_string()).await;
    }

    fn notify(&self, category: Category, title: &str, body: String) {
        crate::notifications::notify(&self.app, category, title.to_string(), body);
    }

    fn changed(&self) {
        if let Err(e) = self.app.emit(AUTOMATIONS_CHANGED_EVENT, Value::Null) {
            tracing::warn!(error = %e, "could not emit {}", AUTOMATIONS_CHANGED_EVENT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 25, h, 0, 0).unwrap()
    }

    #[test]
    fn a_limit_lasts_until_its_reset_or_a_sensible_default() {
        let now = at(10);
        let reset = at(11);
        let ms = u64::try_from(reset.timestamp_millis()).unwrap();
        assert_eq!(
            limit_until(ProviderErrorKind::RateLimited, Some(ms), None, now),
            Some(reset)
        );
        assert_eq!(
            limit_until(ProviderErrorKind::RateLimited, None, Some(30), now),
            Some(now + TimeDelta::seconds(30))
        );
        assert_eq!(
            limit_until(ProviderErrorKind::RateLimited, None, None, now),
            Some(now + TimeDelta::seconds(SHORT_LIMIT_SECS))
        );
        assert_eq!(
            limit_until(ProviderErrorKind::QuotaExhausted, None, None, now),
            Some(next_utc_midnight(now))
        );
        let past = u64::try_from(at(9).timestamp_millis()).unwrap();
        assert_eq!(
            limit_until(ProviderErrorKind::QuotaExhausted, Some(past), None, now),
            Some(next_utc_midnight(now)),
            "a reset already past is no reset"
        );
        assert_eq!(
            limit_until(ProviderErrorKind::Network, None, None, now),
            None
        );
    }

    #[test]
    fn a_pool_keeps_its_latest_wait() {
        let cooldowns = PoolCooldowns::default();
        cooldowns.hold("openrouter:free", at(11));
        cooldowns.hold("openrouter:free", at(10));
        assert_eq!(cooldowns.until("openrouter:free", at(9)), Some(at(11)));
        assert_eq!(
            cooldowns.until("openrouter:free", at(11)),
            None,
            "over at its reset"
        );
        assert_eq!(cooldowns.until("lmstudio", at(9)), None);
    }

    fn free(id: &str) -> ModelRef {
        ModelRef::new(ProviderId::OpenRouter, id)
    }

    #[test]
    fn the_automations_model_says_whether_it_shares_chats_limit() {
        let chat = Some(("openrouter:free".to_string(), true));
        let ok = |_: &ModelRef| Ok(());
        let same = run_model_for(None, chat.clone(), false, ok).unwrap();
        assert!(same.shares_chat);
        assert_eq!(same.pool, "openrouter:free");
        let other_free = run_model_for(
            Some(&free("meta-llama/llama-3.3-70b-instruct:free")),
            chat.clone(),
            false,
            ok,
        )
        .unwrap();
        assert!(
            other_free.shares_chat,
            "another free OpenRouter model counts against the same 50 a day"
        );
        let local = run_model_for(
            Some(&ModelRef::new(ProviderId::LmStudio, "qwen3-8b")),
            chat.clone(),
            false,
            ok,
        )
        .unwrap();
        assert!(!local.shares_chat);
        assert_eq!(local.pool, "lmstudio");
        assert_eq!(
            run_model_for(None, None, false, ok).unwrap_err(),
            ModelBlock::NoModel
        );
        assert_eq!(
            run_model_for(None, chat.clone(), true, ok).unwrap_err(),
            ModelBlock::LocalOnly
        );
        assert_eq!(
            run_model_for(Some(&free("x:free")), chat.clone(), true, |_| Err((
                true,
                "Local-only".into()
            )))
            .unwrap_err(),
            ModelBlock::LocalOnly
        );
        assert_eq!(
            run_model_for(Some(&free("x:free")), chat, false, |_| Err((
                false,
                "No key".into()
            )))
            .unwrap_err(),
            ModelBlock::Unusable("No key".into())
        );
    }

    #[test]
    fn the_question_names_the_automation_and_its_rules() {
        let text = question_text("Weekly digest", "Summarise the week.");
        assert!(text.starts_with("Automation \"Weekly digest\":\n\nSummarise the week."));
        assert!(text.contains("saved for the user's OK"));
    }
}
