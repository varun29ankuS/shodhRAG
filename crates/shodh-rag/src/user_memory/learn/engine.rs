//! The learner: runs extraction and decisions for completed turns, keeps the inbox, and
//! applies suggestions — on the user's Accept, or under the automatic-learning policy.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use shodh_ontology::{ExtractorKind, Statement};

use super::decide::{self, DecidedBy, Decision, DecisionRecord, Deterministic, Judged};
use super::extract::{self, Candidate, SourceText, TurnInput, MAX_SOURCE_CHARS};
use super::inbox::{
    AppliedOutcome, Inbox, NewProposal, Proposal, ProposalAction, ProposalOrigin, ProposalStatus,
    ProposalView, PutOutcomeView, Rejection, RequestedFiling, RequestedOutcome, StatusEvent, Undo,
    UsageCounter,
};
use super::{explicit, sensitivity};
use super::{
    normalise_text, LearnError, LearnMode, LearnModel, LearnPolicy, LearnResult, PolicySource,
};
use crate::audit::{AuditEventType, LOCAL_OWNER};
use crate::statements::dynamics::LinkState;
use crate::statements::{render_text, PutIntent, PutOutcome, Scope, StatementError};
use crate::user_memory::guard::{Origin, WriteAuthority};
use crate::user_memory::{Actor, MemoryContent, MemoryError, MemoryService};

/// Where the learner gets its model; looked up per use (the user can change it).
pub trait ModelSource: Send + Sync {
    /// The model for learning, or why there is none.
    fn model(&self) -> Result<Arc<dyn LearnModel>, String>;
}

/// What learning from some turns did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnReport {
    /// Why nothing was extracted, if nothing was.
    pub skipped: Option<String>,
    /// Suggestions created (waiting or learned).
    pub proposed: Vec<String>,
    /// Of those, applied automatically.
    pub learned: Vec<String>,
    /// Candidates dropped, by reason.
    pub dropped: BTreeMap<String, usize>,
    /// Candidates not suggested again (already waiting, applied or recently rejected).
    pub suppressed: usize,
}

/// What a consolidation did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsolidationReport {
    /// Why it did not run, if it did not.
    pub skipped: Option<String>,
    /// Episode clusters examined.
    pub clusters: usize,
    /// Suggestions created.
    pub proposed: Vec<String>,
    /// Of those, applied automatically.
    pub learned: Vec<String>,
    /// Candidates dropped, by reason.
    pub dropped: BTreeMap<String, usize>,
}

/// Which allowance a model call is counted against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallBudget {
    /// Today's caps ([`LearnPolicy::caps`]).
    Daily,
    /// The one-time allowance for learning from what the user wrote in the removed legacy
    /// memory store ([`super::inbox::LEGACY_IMPORT_CALLS`]): never today's calls.
    LegacyImport,
}

/// Reason code of an answer the model stopped at its token limit.
pub const TRUNCATED: &str = "truncated";
/// Reason code of a judgement that is not a decision on a memory shown.
pub const INVALID_JUDGEMENT: &str = "invalid_judgement";

/// The learning pipeline over one memory service.
pub struct Learner {
    pub(crate) service: Arc<MemoryService>,
    pub(crate) inbox: Arc<Inbox>,
    policy: Arc<dyn PolicySource>,
    models: Arc<dyn ModelSource>,
}

impl std::fmt::Debug for Learner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Learner").finish_non_exhaustive()
    }
}

/// How a suggestion is authorised when applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Authority {
    /// The user accepted it.
    User,
    /// The automatic-learning policy applied it.
    Policy,
}

impl Learner {
    /// A learner writing through `service`, keeping suggestions in `inbox`.
    pub fn new(
        service: Arc<MemoryService>,
        inbox: Arc<Inbox>,
        policy: Arc<dyn PolicySource>,
        models: Arc<dyn ModelSource>,
    ) -> Self {
        Self {
            service,
            inbox,
            policy,
            models,
        }
    }

    /// The inbox.
    pub fn inbox(&self) -> &Arc<Inbox> {
        &self.inbox
    }

    /// The memory service.
    pub fn service(&self) -> &Arc<MemoryService> {
        &self.service
    }

    /// The policy now.
    pub fn policy(&self) -> LearnPolicy {
        self.policy.policy()
    }

    pub(crate) fn now(&self) -> DateTime<Utc> {
        self.service.store().now()
    }

    pub(crate) fn model(&self) -> Result<Arc<dyn LearnModel>, String> {
        self.models.model()
    }

    /// Calls the model under the current policy and today's caps. `turn` is the turn the
    /// call learns from, if any (kept with a cut-off answer).
    pub(crate) async fn call_model(
        &self,
        model: &dyn LearnModel,
        prompt: &str,
        max_tokens: usize,
        turn: Option<&str>,
        budget: CallBudget,
    ) -> LearnResult<String> {
        let policy = self.policy();
        if policy.mode == LearnMode::Off {
            return Err(LearnError::Disabled);
        }
        match budget {
            CallBudget::Daily => {
                self.inbox
                    .reserve_call(prompt.chars().count(), &policy.caps, self.now())?
            }
            CallBudget::LegacyImport => self.inbox.reserve_legacy_call()?,
        }
        let answer = match model.complete(prompt, max_tokens).await {
            Ok(answer) => answer,
            Err(LearnError::Truncated(partial)) => {
                // Counted here, once, for every step: a cut-off answer is an invalid one.
                let now = self.now();
                self.inbox.record_output(partial.chars().count(), now)?;
                self.inbox.count(UsageCounter::Invalid, 1, now)?;
                let detail = format!("the model stopped at its {max_tokens}-token limit");
                self.log_rejections(
                    &model.model_id(),
                    &[Rejection::with_app_detail(TRUNCATED, detail, partial.as_str()).about(turn)],
                );
                return Err(LearnError::Truncated(partial));
            }
            Err(e) => return Err(e),
        };
        self.inbox
            .record_output(answer.chars().count(), self.now())?;
        Ok(answer)
    }

    /// Keeps why answers of `model`, or memories in them, were not used (Settings →
    /// Memory shows the reasons). Failing to keep them is logged, never fatal.
    pub(crate) fn log_rejections(&self, model: &str, rejections: &[Rejection]) {
        if let Err(e) = self.inbox.record_rejections(model, rejections, self.now()) {
            tracing::warn!(target: "shodh::memory", error = %e, "why learning dropped an answer was not kept");
        }
    }

    /// Learns from completed turns of one conversation: extracts candidates from the
    /// user's text, decides each against what is remembered and files suggestions
    /// (applying the eligible ones in automatic mode).
    pub async fn learn_from_turns(&self, turns: &[TurnInput]) -> LearnResult<TurnReport> {
        self.learn_from(turns, CallBudget::Daily).await
    }

    /// [`Self::learn_from_turns`] for messages of the removed legacy memory store, counted
    /// against its one-time allowance ([`super::inbox::LEGACY_IMPORT_CALLS`]) instead of
    /// today's calls; [`LearnError::BudgetExhausted`] once it is spent.
    pub async fn learn_from_legacy(&self, turns: &[TurnInput]) -> LearnResult<TurnReport> {
        self.learn_from(turns, CallBudget::LegacyImport).await
    }

    async fn learn_from(&self, turns: &[TurnInput], budget: CallBudget) -> LearnResult<TurnReport> {
        let mut report = TurnReport::default();
        let policy = self.policy();
        if policy.mode == LearnMode::Off {
            return Err(LearnError::Disabled);
        }
        // Newest turns first within the size bound, whole turns only, then in order. A
        // turn with nothing about the user (a question about the documents) is not sent.
        let mut sources: Vec<SourceText> = Vec::new();
        let mut size = 0;
        let mut wrote = false;
        for (i, turn) in turns.iter().enumerate().rev() {
            let source = SourceText::from_turn(i, turn);
            if source.text.is_empty() {
                continue;
            }
            wrote = true;
            if !extract::worth_learning(&source.text) {
                continue;
            }
            let len = source.text.chars().count();
            if size + len > MAX_SOURCE_CHARS {
                break;
            }
            size += len;
            sources.push(source);
        }
        sources.reverse();
        if sources.is_empty() {
            report.skipped = Some(
                if wrote {
                    "nothing the user said was about themselves"
                } else {
                    "nothing the user wrote"
                }
                .to_string(),
            );
            return Ok(report);
        }
        let ontology = self.service.store().ontology();
        let joined: String = sources
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let slice = extract::slice_for_turns(ontology, &joined);
        let slice_classes: BTreeSet<String> = slice
            .matched_classes()
            .into_iter()
            .map(str::to_string)
            .collect();
        let model = self.model().map_err(LearnError::ModelUnavailable)?;
        let today = self.now().format("%Y-%m-%d").to_string();
        let prompt = extract::extraction_prompt(&slice, &sources, &today);
        // Rejections not about one cited turn are about the newest turn of the batch.
        let newest = sources.last().map(|s| s.turn_id.clone());
        let newest = newest.as_deref();
        let model_id = model.model_id();
        let answer = self
            .call_model(
                model.as_ref(),
                &prompt,
                extract::EXTRACTION_MAX_TOKENS,
                newest,
                budget,
            )
            .await?;
        let parsed = match extract::parse_answer(&answer) {
            Ok(parsed) => parsed,
            Err(e) => {
                self.inbox.count(UsageCounter::Invalid, 1, self.now())?;
                let detail = match &e {
                    LearnError::InvalidOutput(detail) => detail.clone(),
                    other => other.to_string(),
                };
                self.log_rejections(
                    &model_id,
                    &[
                        Rejection::new(extract::INVALID_JSON, detail, answer.as_str())
                            .about(newest),
                    ],
                );
                return Err(e);
            }
        };
        let validated = extract::validate(
            &self.service,
            &slice_classes,
            parsed.candidates,
            &sources,
            &model_id,
        );
        let rejections: Vec<Rejection> = parsed
            .rejections
            .into_iter()
            .chain(validated.rejections.iter().cloned())
            .map(|r| r.about(newest))
            .collect();
        self.log_rejections(&model_id, &rejections);
        if parsed.malformed > 0 {
            report
                .dropped
                .insert("malformed".to_string(), parsed.malformed);
        }
        for (code, n) in &validated.dropped {
            *report.dropped.entry(code.clone()).or_insert(0) += n;
        }
        let now = self.now();
        self.inbox
            .count(UsageCounter::Refused, validated.ungrounded(), now)?;
        self.inbox.count(
            UsageCounter::Invalid,
            validated.dropped_total() - validated.ungrounded() + parsed.malformed,
            now,
        )?;
        for candidate in validated.candidates {
            match self
                .propose_candidate(
                    &candidate,
                    ProposalOrigin::Turn,
                    &model,
                    &mut report.proposed,
                    &mut report.learned,
                    &mut report.suppressed,
                    budget,
                )
                .await
            {
                Ok(()) => {}
                Err(LearnError::BudgetExhausted(what)) => {
                    report.skipped = Some(format!("today's limit is reached ({what})"));
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(report)
    }

    /// Decides one validated candidate, files it and applies it when the policy allows.
    pub(crate) async fn propose_candidate(
        &self,
        candidate: &Candidate,
        origin: ProposalOrigin,
        model: &Arc<dyn LearnModel>,
        proposed: &mut Vec<String>,
        learned: &mut Vec<String>,
        suppressed: &mut usize,
        budget: CallBudget,
    ) -> LearnResult<()> {
        let record = self.decide(candidate, model, budget).await?;
        // A new memory goes where it was learned (the conversation's workspace, or global);
        // a change to an existing memory stays in that memory's scope, so a fact learned in
        // a workspace never moves a global memory out of every other chat.
        let scope = match record.decision.target() {
            Some(target) => match self.service.get(target).await {
                Ok(existing) => existing.scope,
                Err(e) => {
                    tracing::debug!(target: "shodh::memory", error = %e, "target memory unreadable; learned scope used");
                    candidate.source.scope.clone()
                }
            },
            None => candidate.source.scope.clone(),
        };
        if matches!(record.decision, Decision::Noop { .. })
            && record.decided_by != DecidedBy::Undecided
        {
            // Already remembered. Reinforcing it is not worth a suggestion in ask mode;
            // in automatic mode it is applied (and listed) like any other.
            if self.policy().mode != LearnMode::Auto {
                *suppressed += 1;
                return Ok(());
            }
        }
        let content = record
            .content
            .clone()
            .unwrap_or_else(|| candidate.content.clone());
        let text = match &record.content {
            Some(merged) => self
                .render_content(
                    merged,
                    &candidate.source.source,
                    model,
                    candidate.confidence,
                )
                .unwrap_or_else(|| candidate.text.clone()),
            None => candidate.text.clone(),
        };
        let action = ProposalAction::Remember {
            content,
            text: text.clone(),
            decision: record.decision.clone(),
            decided_by: record.decided_by,
            target_text: record.target_text.clone(),
            evidence: candidate.evidence.clone(),
            model: model.model_id(),
            at: candidate.source.at,
            source: candidate.source.source.clone(),
            by_user: false,
        };
        let new = NewProposal {
            origin,
            fingerprint: remember_fingerprint(&scope, candidate.valid.class(), &text),
            scope,
            conversation_id: Some(candidate.source.conversation_id.clone()),
            turn_id: Some(candidate.source.turn_id.clone()),
            action,
            confidence: candidate.confidence,
            sensitive: candidate.sensitive.clone(),
        };
        self.file(new, proposed, learned, suppressed).await
    }

    /// Saves what the user explicitly asked Shodh to remember (`request`, matched by
    /// [`explicit::memory_request`] on their message of turn `turn_id`) as a note in their
    /// own words, without a model: it is stored with extractor `user` and authorised by
    /// the request itself, as an Accept of the note's suggestion ([`Self::accept`]), so the
    /// suggestion reads as accepted and Undo works as for any accepted suggestion. Asking
    /// is not learning: the learning mode does not apply (Off included) and the daily caps
    /// do not hold it back. A sensitive note ([`sensitivity::classify_text`]) is filed as a
    /// suggestion and waits for the user. A note already saved is not saved again; one the
    /// user rejected or undid before is saved again, since they asked again
    /// ([`Inbox::insert_requested`]). A suggestion of the same note that waits (the
    /// learner's, or a save of the user's that failed) is replaced by this one, so the
    /// note is the user's own words of this turn; a sensitive one waiting keeps waiting.
    pub async fn file_user_note(
        &self,
        request: &explicit::MemoryRequest,
        conversation_id: &str,
        turn_id: &str,
        at: DateTime<Utc>,
        scope: Scope,
    ) -> LearnResult<RequestedOutcome> {
        let content = MemoryContent::Note {
            text: request.note.clone(),
        };
        let app_version = self.service.app_version().to_string();
        let origin = Origin::approved_in_conversation(
            conversation_id,
            turn_id,
            "explicit-request",
            &app_version,
        );
        // The text the memory will have, so the learner's own note of the same words is
        // recognised as the same suggestion.
        let text = self.service.preview(content.clone(), &origin)?;
        let mut sensitive = sensitivity::classify_text(&request.note);
        sensitive.sort();
        sensitive.dedup();
        let fingerprint = remember_fingerprint(&scope, "Note", &text);
        // Saved before, unless the user forgot that memory since.
        let mut applied_kept = false;
        for id in self.inbox.applied_memories(&fingerprint)? {
            if self.service.get(&id).await.is_ok() {
                applied_kept = true;
                break;
            }
        }
        let new = NewProposal {
            origin: ProposalOrigin::Turn,
            fingerprint,
            scope,
            conversation_id: Some(conversation_id.to_string()),
            turn_id: Some(turn_id.to_string()),
            action: ProposalAction::Remember {
                content,
                text,
                decision: Decision::Add,
                decided_by: DecidedBy::Rule,
                target_text: None,
                evidence: request.evidence.clone(),
                model: app_version,
                at,
                source: origin.source.clone(),
                by_user: true,
            },
            confidence: 1.0,
            sensitive,
        };
        let proposal = match self.inbox.insert_requested(new, self.now(), applied_kept)? {
            RequestedFiling::Kept => return Ok(RequestedOutcome::AlreadyKept),
            // It waits for the user's approval already.
            RequestedFiling::Waiting(_) => return Ok(RequestedOutcome::AlreadyKept),
            RequestedFiling::Inserted(proposal) if !proposal.sensitive.is_empty() => {
                return Ok(RequestedOutcome::Filed(proposal))
            }
            RequestedFiling::Inserted(proposal) => proposal,
        };
        let actor = request_actor(conversation_id, turn_id);
        match self.accept(&proposal.id, None, &actor).await {
            // A memory of the same words was already stored (entered in Settings, or by
            // the assistant): nothing new was saved and there is nothing to undo.
            Ok(view) if !view.undoable => Ok(RequestedOutcome::AlreadyKept),
            Ok(view) => Ok(RequestedOutcome::Saved(Box::new(view))),
            Err(e) => {
                // Said by what the suggestion is now: saved meanwhile (accepted in another
                // window), waiting for the user (the write failed and it can be accepted
                // again), or neither (it no longer applies): then the request is left to
                // the assistant.
                let now: ProposalView = self.inbox.get(&proposal.id)?.into();
                match now.status {
                    ProposalStatus::Accepted | ProposalStatus::Learned if now.undoable => {
                        Ok(RequestedOutcome::Saved(Box::new(now)))
                    }
                    ProposalStatus::Accepted | ProposalStatus::Learned => {
                        Ok(RequestedOutcome::AlreadyKept)
                    }
                    ProposalStatus::Pending | ProposalStatus::Failed => {
                        tracing::warn!(target: "shodh::memory", error = %e, "the note the user asked for was not saved; it waits for them");
                        Ok(RequestedOutcome::Filed(Box::new(
                            self.inbox.get(&proposal.id)?,
                        )))
                    }
                    _ => Err(e),
                }
            }
        }
    }

    /// Stores a suggestion and applies it automatically when eligible.
    pub(crate) async fn file(
        &self,
        new: NewProposal,
        proposed: &mut Vec<String>,
        learned: &mut Vec<String>,
        suppressed: &mut usize,
    ) -> LearnResult<()> {
        let policy = self.policy();
        // Stopped while this was being worked out: file nothing.
        if policy.mode == LearnMode::Off {
            return Err(LearnError::Disabled);
        }
        let Some(proposal) = self.inbox.insert(new, &policy.caps, self.now())? else {
            *suppressed += 1;
            return Ok(());
        };
        proposed.push(proposal.id.clone());
        if auto_eligible(&proposal, &policy) && self.apply_automatically(&proposal).await? {
            learned.push(proposal.id.clone());
        }
        Ok(())
    }

    fn render_content(
        &self,
        content: &MemoryContent,
        source: &str,
        model: &Arc<dyn LearnModel>,
        confidence: f64,
    ) -> Option<String> {
        let origin = llm_origin(source, &model.model_id(), confidence, "render");
        let statement = self
            .service
            .build_statement_at(content.clone(), &origin, self.now())
            .ok()?;
        let ontology = self.service.store().ontology();
        let valid = ontology.validate(&statement).ok()?;
        Some(render_text(ontology, &valid))
    }

    /// DECIDE for one candidate: by rule, else by the model (when memories may be shared
    /// with it and today's caps allow), else undecided.
    async fn decide(
        &self,
        candidate: &Candidate,
        model: &Arc<dyn LearnModel>,
        budget: CallBudget,
    ) -> LearnResult<DecisionRecord> {
        let ontology = self.service.store().ontology();
        let neighbours = decide::neighbours(
            &self.service,
            &candidate.valid,
            &candidate.text,
            &candidate.source.scope,
        )
        .await?;
        let close = match decide::decide_by_rule(
            ontology,
            &candidate.valid,
            &candidate.statement,
            &candidate.text,
            &neighbours,
        ) {
            Deterministic::Decided(record) => return Ok(record),
            Deterministic::Ambiguous(close) => close,
        };
        if !self.policy().share_memories_with_model {
            return Ok(decide::undecided());
        }
        let keys: Vec<String> = (1..=close.len()).map(|i| format!("N{i}")).collect();
        let shown: Vec<(String, String)> = keys
            .iter()
            .zip(&close)
            .map(|(k, &i)| (k.clone(), neighbours[i].stored.text.clone()))
            .collect();
        let prompt = decide::judge_prompt(&candidate.text, &shown);
        let turn = Some(candidate.source.turn_id.as_str());
        let answer = match self
            .call_model(
                model.as_ref(),
                &prompt,
                decide::JUDGE_MAX_TOKENS,
                turn,
                budget,
            )
            .await
        {
            Ok(answer) => answer,
            Err(LearnError::BudgetExhausted(_))
            | Err(LearnError::Model(_))
            | Err(LearnError::Truncated(_)) => return Ok(decide::undecided()),
            Err(e) => return Err(e),
        };
        let Some(judged) = decide::parse_judgement(&answer, &keys) else {
            self.inbox.count(UsageCounter::Invalid, 1, self.now())?;
            let detail = "not a decision on one of the memories shown";
            self.log_rejections(
                &model.model_id(),
                &[
                    Rejection::with_app_detail(INVALID_JUDGEMENT, detail, answer.as_str())
                        .about(turn),
                ],
            );
            return Ok(decide::undecided());
        };
        let neighbour = match &judged {
            Judged::Add => None,
            Judged::Noop(i) | Judged::Update(i) | Judged::Supersede(i) => {
                close.get(*i).map(|&n| &neighbours[n])
            }
        };
        Ok(decide::from_judgement(
            ontology,
            &judged,
            &candidate.statement,
            neighbour,
        ))
    }

    /// Applies an eligible suggestion under the automatic-learning policy. The policy is
    /// read again first: if the user switched to ask or off meanwhile, it stays waiting.
    async fn apply_automatically(&self, proposal: &Proposal) -> LearnResult<bool> {
        if !auto_eligible(proposal, &self.policy()) {
            return Ok(false);
        }
        self.inbox
            .transition(&proposal.id, StatusEvent::AutoApply, self.now())?;
        let actor = learn_actor(proposal);
        match self.apply(proposal, Authority::Policy, None, &actor).await {
            Ok((outcome, _)) => {
                self.inbox.record_outcome(&proposal.id, &outcome, None)?;
                if let Some(memory) = evolve_target(proposal, &outcome) {
                    // Boxed: evolution files suggestions, which may be applied here again
                    // (links and revisions never evolve further, so this is one level).
                    if let Err(e) = Box::pin(self.evolve(&memory, proposal)).await {
                        tracing::debug!(target: "shodh::memory", error = %e, "evolution skipped");
                    }
                }
                Ok(true)
            }
            Err(e) => {
                self.settle_failure(&proposal.id, &e)?;
                // Sensitive content refused by the memory service: it waits for the user.
                Ok(false)
            }
        }
    }

    /// The user accepts suggestion `id` (optionally edited: `edit` replaces the memory).
    /// It is moved to accepted before anything is written, so it is applied at most once.
    pub async fn accept(
        &self,
        id: &str,
        edit: Option<MemoryContent>,
        actor: &Actor,
    ) -> LearnResult<ProposalView> {
        let proposal = self.inbox.transition(id, StatusEvent::Accept, self.now())?;
        self.finish_accept(&proposal, edit, actor).await
    }

    /// Accepts several suggestions as they are. New memories are stored together
    /// ([`MemoryService::put_learned_many`]); other suggestions are applied one by one.
    /// Returns each one's result, in order.
    pub async fn accept_many(
        &self,
        ids: &[String],
        actor: &Actor,
    ) -> Vec<(String, LearnResult<ProposalView>)> {
        let mut out: Vec<(String, Option<LearnResult<ProposalView>>)> = Vec::new();
        let mut batch = Vec::new();
        let mut batched = Vec::new();
        for id in ids {
            // Moved to accepted before anything is written, as in `accept`.
            let proposal = match self.inbox.transition(id, StatusEvent::Accept, self.now()) {
                Ok(proposal) => proposal,
                Err(e) => {
                    out.push((id.clone(), Some(Err(e))));
                    continue;
                }
            };
            match self.remember_write(&proposal) {
                Some(Ok(item)) => {
                    batch.push(item);
                    batched.push(out.len());
                    out.push((id.clone(), None));
                }
                Some(Err(e)) => {
                    let result = self.settle_failure(id, &e).and(Err(e));
                    out.push((id.clone(), Some(result)));
                }
                None => {
                    let result = self.finish_accept(&proposal, None, actor).await;
                    out.push((id.clone(), Some(result)));
                }
            }
        }
        if !batch.is_empty() {
            match self.service.put_learned_many(batch, actor).await {
                Ok(outcomes) => {
                    for (slot, outcome) in batched.into_iter().zip(outcomes) {
                        let id = out[slot].0.clone();
                        let result = match outcome {
                            Ok(written) => match &written.outcome {
                                PutOutcome::Conflict { existing, .. } => Err(LearnError::Stale(
                                    format!("it now conflicts with memory `{existing}`"),
                                )),
                                outcome => Ok((applied(outcome), None)),
                            },
                            Err(e) => Err(e.into()),
                        };
                        out[slot].1 = Some(self.record_accept(&id, result));
                    }
                }
                Err(e) => {
                    let message = e.to_string();
                    for slot in batched {
                        let id = out[slot].0.clone();
                        let result = self.record_accept(
                            &id,
                            Err(LearnError::Memory(MemoryError::InvalidInput(
                                message.clone(),
                            ))),
                        );
                        out[slot].1 = Some(result);
                    }
                }
            }
        }
        out.into_iter()
            .map(|(id, result)| {
                let result = result.unwrap_or_else(|| {
                    Err(LearnError::Memory(MemoryError::InvalidInput(
                        "the suggestion was not applied".to_string(),
                    )))
                });
                (id, result)
            })
            .collect()
    }

    /// Applies an accepted suggestion and records the result (see [`Self::accept`]).
    async fn finish_accept(
        &self,
        proposal: &Proposal,
        edit: Option<MemoryContent>,
        actor: &Actor,
    ) -> LearnResult<ProposalView> {
        let result = self.apply(proposal, Authority::User, edit, actor).await;
        self.record_accept(&proposal.id, result)
    }

    /// Records what applying accepted suggestion `id` did.
    fn record_accept(
        &self,
        id: &str,
        result: LearnResult<(AppliedOutcome, Option<ProposalAction>)>,
    ) -> LearnResult<ProposalView> {
        match result {
            Ok((outcome, edited)) => {
                self.inbox.record_outcome(id, &outcome, edited.as_ref())?;
                Ok(self.inbox.get(id)?.into())
            }
            Err(e) => {
                self.settle_failure(id, &e)?;
                Err(e)
            }
        }
    }

    /// The write of an accepted, unedited new-memory suggestion, as [`Self::apply`]
    /// makes it. `None` for other suggestions.
    #[allow(clippy::type_complexity)]
    fn remember_write(
        &self,
        proposal: &Proposal,
    ) -> Option<LearnResult<(Statement, Scope, PutIntent, Origin)>> {
        let ProposalAction::Remember {
            content,
            decision,
            at,
            source,
            model,
            by_user,
            ..
        } = &proposal.action
        else {
            return None;
        };
        let origin = if *by_user {
            user_origin(source, model, &proposal.id)
        } else {
            Origin {
                source: source.clone(),
                extractor: ExtractorKind::Llm,
                extractor_version: model.clone(),
                confidence: proposal.confidence,
                authority: WriteAuthority::UserApproval {
                    step_id: proposal.id.clone(),
                },
            }
        };
        let content = with_valid_from(content.clone(), *at);
        Some(
            self.service
                .build_statement_at(content, &origin, *at)
                .map(|statement| (statement, proposal.scope.clone(), decision.intent(), origin))
                .map_err(Into::into),
        )
    }

    /// The user rejects suggestion `id`; the same suggestion is not made again for a while.
    pub fn reject(&self, id: &str) -> LearnResult<ProposalView> {
        self.inbox.transition(id, StatusEvent::Reject, self.now())?;
        Ok(self.inbox.get(id)?.into())
    }

    /// Undoes an accepted or learned suggestion. When the memory it stored or changed was
    /// forgotten there is nothing left to undo: the suggestion is returned as it is, with
    /// `forgotten` set, so the caller can say so instead of reporting a failure. A memory
    /// found missing only now (forgotten before suggestions were marked) marks it then.
    pub async fn undo(&self, id: &str, actor: &Actor) -> LearnResult<ProposalView> {
        let current = self.inbox.get(id)?;
        if current.forgotten_at.is_some() {
            return Ok(current.into());
        }
        let proposal = self.inbox.transition(id, StatusEvent::Undo, self.now())?;
        let Some(undo) = proposal.outcome.as_ref().and_then(|o| o.undo.clone()) else {
            self.inbox
                .restore_status(id, ProposalStatus::Undone, proposal.status)?;
            return Err(LearnError::InvalidTransition {
                from: proposal.status,
                action: "undone (it changed nothing that can be undone)",
            });
        };
        let result: LearnResult<()> = async {
            match &undo {
                // The user's own note: Undo means "I did not mean that", so its words are
                // erased as a forget erases them (a note is never a change of another
                // memory, so there is nothing to reopen). Not once it was edited: erasing
                // every version would drop the edit too, so it is refused as a revert is.
                Undo::Revert { id } if is_users_note(&proposal) => {
                    if let Some(newer) = self.edited_since(id).await? {
                        return Err(LearnError::Memory(MemoryError::Statement(
                            StatementError::InvalidSupersede {
                                target: id.clone(),
                                reason: format!("it was changed again since (by `{newer}`)"),
                            },
                        )));
                    }
                    self.service.forget(id, None, actor).await?;
                }
                Undo::Revert { id } => {
                    self.service.revert_learned(id, actor).await?;
                }
                Undo::Unarchive { id } => self.service.unarchive(id, actor).await?,
                Undo::Unretire { retire, keep } => {
                    self.service.unretire(retire, keep, actor).await?
                }
                Undo::RestoreLink { a, b, previous } => {
                    let link = previous.map(|(weight, co_activations, updated_at)| LinkState {
                        weight,
                        co_activations,
                        updated_at,
                    });
                    self.service
                        .store()
                        .dynamics()
                        .set_link(a, b, link.as_ref())?;
                    self.service.audit(
                        actor,
                        AuditEventType::MemoryWrite,
                        json!({"action": "undo_link", "ids": [a, b], "extractor": "llm", "via": actor.via}),
                    );
                }
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => Ok(self.inbox.get(id)?.into()),
            Err(LearnError::Memory(MemoryError::NotFound(missing))) => {
                self.inbox
                    .restore_status(id, ProposalStatus::Undone, proposal.status)?;
                let forgotten = super::inbox::ForgottenMemories {
                    ids: vec![missing],
                    ..Default::default()
                };
                self.inbox.forget_memories(&forgotten, self.now())?;
                Ok(self.inbox.get(id)?.into())
            }
            Err(e) => {
                self.inbox
                    .restore_status(id, ProposalStatus::Undone, proposal.status)?;
                self.inbox.record_error(id, &e.to_string())?;
                Err(e)
            }
        }
    }

    /// Marks a failed application: stale when the memory it changes moved on, failed
    /// otherwise.
    fn settle_failure(&self, id: &str, error: &LearnError) -> LearnResult<()> {
        let event = if is_stale(error) {
            StatusEvent::Stale
        } else {
            StatusEvent::Fail
        };
        if let Err(e) = self.inbox.transition(id, event, self.now()) {
            tracing::warn!(target: "shodh::memory", error = %e, "could not record the suggestion's failure");
        }
        self.inbox.record_error(id, &error.to_string())
    }

    /// Applies a suggestion. Returns what it did and, when edited, the edited action.
    async fn apply(
        &self,
        proposal: &Proposal,
        authority: Authority,
        edit: Option<MemoryContent>,
        actor: &Actor,
    ) -> LearnResult<(AppliedOutcome, Option<ProposalAction>)> {
        let origin_for = |source: &str, model: &str, edited: bool| -> Origin {
            let authority_value = match authority {
                Authority::User => WriteAuthority::UserApproval {
                    step_id: proposal.id.clone(),
                },
                Authority::Policy => WriteAuthority::LearnPolicy {
                    proposal_id: proposal.id.clone(),
                },
            };
            if edited {
                // The user rewrote it: the user formulated this memory.
                Origin {
                    source: source.to_string(),
                    extractor: ExtractorKind::User,
                    extractor_version: self.service.app_version().to_string(),
                    confidence: 1.0,
                    authority: authority_value,
                }
            } else {
                Origin {
                    source: source.to_string(),
                    extractor: ExtractorKind::Llm,
                    extractor_version: model.to_string(),
                    confidence: proposal.confidence,
                    authority: authority_value,
                }
            }
        };
        match &proposal.action {
            ProposalAction::Remember {
                content,
                decision,
                at,
                source,
                model,
                by_user,
                ..
            } => {
                let edited = edit.is_some();
                let content = edit.unwrap_or_else(|| content.clone());
                let content = with_valid_from(content, *at);
                // The user's own words are the user's formulation, as an edit is.
                let origin = origin_for(source, model, edited || *by_user);
                let statement = self
                    .service
                    .build_statement_at(content.clone(), &origin, *at)?;
                // An edit is a complete new fact: let the ontology decide unless a target
                // was named explicitly.
                let outcome = self
                    .service
                    .put_learned(
                        statement,
                        proposal.scope.clone(),
                        decision.intent(),
                        &origin,
                        actor,
                    )
                    .await?;
                if let PutOutcome::Conflict { existing, .. } = &outcome.outcome {
                    return Err(LearnError::Stale(format!(
                        "it now conflicts with memory `{existing}`"
                    )));
                }
                let edited_action = edited.then(|| {
                    let mut action = proposal.action.clone();
                    if let ProposalAction::Remember {
                        content: c, text, ..
                    } = &mut action
                    {
                        *c = content.clone();
                        if let Some(memory) = &outcome.memory {
                            *text = memory.text.clone();
                        }
                    }
                    action
                });
                Ok((applied(&outcome.outcome), edited_action))
            }
            ProposalAction::Revise {
                target,
                content,
                at,
                source,
                model,
                ..
            } => {
                let edited = edit.is_some();
                let content = with_valid_from(edit.unwrap_or_else(|| content.clone()), *at);
                let origin = origin_for(source, model, edited);
                let statement = self
                    .service
                    .build_statement_at(content.clone(), &origin, *at)?;
                let outcome = self
                    .service
                    .put_learned(
                        statement,
                        proposal.scope.clone(),
                        crate::statements::PutIntent::Supersede {
                            target: target.clone(),
                        },
                        &origin,
                        actor,
                    )
                    .await?;
                Ok((applied(&outcome.outcome), None))
            }
            ProposalAction::Link { a, b, reason, .. } => {
                if edit.is_some() {
                    return Err(LearnError::Memory(MemoryError::InvalidInput(
                        "a link suggestion cannot be edited".to_string(),
                    )));
                }
                // Both memories must still exist.
                self.service.memory_statement(a).await?;
                self.service.memory_statement(b).await?;
                let dynamics = self.service.store().dynamics();
                let previous = dynamics.link(a, b)?;
                let now = self.now();
                let mut importance = HashMap::new();
                importance.insert(a.clone(), 0.5);
                importance.insert(b.clone(), 0.5);
                dynamics.strengthen_links(
                    &[crate::statements::dynamics::ordered_pair(a, b)],
                    &importance,
                    now,
                )?;
                self.service.audit(
                    actor,
                    AuditEventType::MemoryWrite,
                    json!({
                        "action": "link",
                        "ids": [a, b],
                        "reason_chars": crate::user_memory::text_chars(reason),
                        "extractor": "llm",
                        "authority": match authority { Authority::User => "user_approval", Authority::Policy => "learn_policy" },
                        "approval": proposal.id,
                        "via": actor.via,
                    }),
                );
                Ok((
                    AppliedOutcome {
                        memory_id: Some(a.clone()),
                        put: None,
                        undo: Some(Undo::RestoreLink {
                            a: a.clone(),
                            b: b.clone(),
                            previous: previous.map(|l| (l.weight, l.co_activations, l.updated_at)),
                        }),
                    },
                    None,
                ))
            }
            ProposalAction::Resolve { keep, retire, .. } => {
                no_edit(&edit)?;
                self.service.retire(retire, keep, actor).await?;
                Ok((
                    AppliedOutcome {
                        memory_id: Some(keep.clone()),
                        put: None,
                        undo: Some(Undo::Unretire {
                            retire: retire.clone(),
                            keep: keep.clone(),
                        }),
                    },
                    None,
                ))
            }
            ProposalAction::Archive { target, .. } => {
                no_edit(&edit)?;
                self.service.archive(target, actor).await?;
                Ok((
                    AppliedOutcome {
                        memory_id: None,
                        put: None,
                        undo: Some(Undo::Unarchive { id: target.clone() }),
                    },
                    None,
                ))
            }
        }
    }

    /// The newer version written since memory `id` (an edit of it), if any: undoing the
    /// suggestion that stored `id` would then drop that edit, so it cannot be undone.
    async fn edited_since(&self, id: &str) -> LearnResult<Option<String>> {
        let memory = self.service.get(id).await?;
        let Some(successor) = memory.superseded_by else {
            return Ok(None);
        };
        // A historical write points at the older current memory; an edit at a newer one.
        Ok(match self.service.get(&successor).await {
            Ok(newer) if newer.created_at > memory.created_at => Some(successor),
            _ => None,
        })
    }

    /// `views` with `undoable` cleared for suggestions whose memory was edited since
    /// ([`Self::edited_since`]): Undo would be refused, so it is not offered. A memory
    /// that cannot be read leaves the view as it is (Undo then says what happened).
    pub async fn with_current_undo(&self, mut views: Vec<ProposalView>) -> Vec<ProposalView> {
        for view in &mut views {
            let Some(Undo::Revert { id }) = view.outcome.as_ref().and_then(|o| o.undo.as_ref())
            else {
                continue;
            };
            if !view.undoable {
                continue;
            }
            if matches!(self.edited_since(id).await, Ok(Some(_))) {
                view.undoable = false;
            }
        }
        views
    }

    /// Suggestions for the inbox: waiting ones, and recently decided ones.
    pub fn list(
        &self,
        statuses: &[ProposalStatus],
        limit: usize,
    ) -> LearnResult<Vec<ProposalView>> {
        Ok(self
            .inbox
            .list(statuses, limit)?
            .into_iter()
            .map(ProposalView::from)
            .collect())
    }

    /// The kill switch: rejects every waiting suggestion. Returns how many.
    pub fn discard_pending(&self) -> LearnResult<usize> {
        self.inbox.reject_all_pending(self.now())
    }

    /// Runs evolution for a memory the user accepted (by suggestion id).
    pub async fn evolve_accepted(&self, id: &str) -> LearnResult<usize> {
        let proposal = self.inbox.get(id)?;
        let Some(memory) = proposal
            .outcome
            .as_ref()
            .and_then(|o| evolve_target(&proposal, o))
        else {
            return Ok(0);
        };
        self.evolve(&memory, &proposal).await
    }
}

fn no_edit(edit: &Option<MemoryContent>) -> LearnResult<()> {
    match edit {
        Some(_) => Err(LearnError::Memory(MemoryError::InvalidInput(
            "this suggestion cannot be edited".to_string(),
        ))),
        None => Ok(()),
    }
}

fn applied(outcome: &PutOutcome) -> AppliedOutcome {
    AppliedOutcome {
        memory_id: outcome.current_id().map(str::to_string),
        put: Some(PutOutcomeView::from(outcome)),
        undo: outcome
            .stored_id()
            .map(|id| Undo::Revert { id: id.to_string() }),
    }
}

fn with_valid_from(content: MemoryContent, at: DateTime<Utc>) -> MemoryContent {
    match content {
        MemoryContent::Fact {
            class,
            subject,
            properties,
            valid_from,
        } => MemoryContent::Fact {
            class,
            subject,
            properties,
            valid_from: valid_from.or(Some(at)),
        },
        note => note,
    }
}

/// The memory to evolve after applying `proposal`: a remembered memory that was stored.
fn evolve_target(proposal: &Proposal, outcome: &AppliedOutcome) -> Option<String> {
    if !matches!(proposal.action, ProposalAction::Remember { .. }) {
        return None;
    }
    let put = outcome.put.as_ref()?;
    matches!(put.outcome.as_str(), "added" | "updated")
        .then(|| put.stored.clone())
        .flatten()
}

fn is_stale(error: &LearnError) -> bool {
    matches!(
        error,
        LearnError::Stale(_)
            | LearnError::Memory(MemoryError::NotFound(_))
            | LearnError::Memory(MemoryError::Statement(
                StatementError::InvalidSupersede { .. }
            ))
            | LearnError::Memory(MemoryError::Statement(StatementError::NotFound(_)))
    )
}

/// The origin of a suggestion the user formulated, applied on the user's Accept of
/// `proposal_id`: extractor `user` at `app_version`, confidence 1.
fn user_origin(source: &str, app_version: &str, proposal_id: &str) -> Origin {
    Origin {
        source: source.to_string(),
        extractor: ExtractorKind::User,
        extractor_version: app_version.to_string(),
        confidence: 1.0,
        authority: WriteAuthority::UserApproval {
            step_id: proposal_id.to_string(),
        },
    }
}

/// An LLM origin (for building statements that are only validated or rendered).
pub(crate) fn llm_origin(source: &str, model: &str, confidence: f64, step: &str) -> Origin {
    Origin {
        source: source.to_string(),
        extractor: ExtractorKind::Llm,
        extractor_version: model.to_string(),
        confidence: confidence.clamp(0.0, 1.0),
        authority: WriteAuthority::UserApproval {
            step_id: step.to_string(),
        },
    }
}

/// Whether `proposal` stores a note in the user's own words (they asked Shodh to remember
/// it): Undo erases it as a forget does.
fn is_users_note(proposal: &Proposal) -> bool {
    matches!(
        &proposal.action,
        ProposalAction::Remember {
            by_user: true,
            content: MemoryContent::Note { .. },
            decision: Decision::Add,
            ..
        }
    )
}

/// The actor of a note the user asked for in run `turn_id` of `conversation_id`.
fn request_actor(conversation_id: &str, turn_id: &str) -> Actor {
    Actor {
        via: "chat",
        principal: LOCAL_OWNER.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        profile_id: None,
        run_id: Some(turn_id.to_string()),
    }
}

/// The actor of an automatic application.
fn learn_actor(proposal: &Proposal) -> Actor {
    Actor {
        via: "learn",
        principal: LOCAL_OWNER.to_string(),
        conversation_id: proposal.conversation_id.clone(),
        profile_id: None,
        run_id: proposal.turn_id.clone(),
    }
}

/// Whether the automatic-learning policy may apply `proposal` without asking:
/// automatic mode; confidence at or above the threshold; nothing sensitive; decided by
/// the ontology's rules (an ADD, a reinforcement, an extension or a temporal supersede —
/// never a contradiction the model judged or a non-temporal value change); or a link or
/// an archive (reversible, no new content). A new memory consolidation found, revisions,
/// contradiction resolutions and anything undecided always ask.
pub fn auto_eligible(proposal: &Proposal, policy: &LearnPolicy) -> bool {
    if policy.mode != LearnMode::Auto
        || proposal.status != ProposalStatus::Pending
        || !proposal.sensitive.is_empty()
    {
        return false;
    }
    match &proposal.action {
        // What the user asked to remember is saved as their own approved note
        // (`file_user_note`), or waits for them when sensitive: never by the policy.
        ProposalAction::Remember { by_user: true, .. } => false,
        // A memory consolidation found is not from a turn the chat shows: applied it would
        // be new content saved out of sight, so it waits for the user. (Its archives and
        // links add nothing and are reversible: they apply as before.)
        ProposalAction::Remember { .. } if proposal.origin == ProposalOrigin::Consolidate => false,
        ProposalAction::Remember {
            decision,
            decided_by,
            ..
        } => {
            proposal.confidence >= policy.auto_min_confidence
                && *decided_by == DecidedBy::Rule
                && matches!(
                    decision,
                    Decision::Add
                        | Decision::Noop { .. }
                        | Decision::Extend { .. }
                        | Decision::Historical { .. }
                        | Decision::Supersede {
                            explicit: false,
                            ..
                        }
                )
        }
        ProposalAction::Link { .. } | ProposalAction::Archive { .. } => true,
        // A revision rewrites an existing memory from the model's reading of a new one;
        // a contradiction picks between two facts. Both always ask.
        ProposalAction::Revise { .. } | ProposalAction::Resolve { .. } => false,
    }
}

/// Duplicate-suppression key of a suggestion.
pub(crate) fn fingerprint(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(normalise_text(part).as_bytes());
        hasher.update([0x1f]);
    }
    hex::encode(&hasher.finalize()[..16])
}

/// Fingerprint of a "remember" suggestion. Global ones are unchanged from before
/// workspaces (so suggestions already rejected or waiting are not offered again); a
/// workspace's include its scope, so the same text in two workspaces is two suggestions.
pub(crate) fn remember_fingerprint(
    scope: &crate::statements::Scope,
    class: &str,
    text: &str,
) -> String {
    match scope {
        crate::statements::Scope::Global => fingerprint(&["remember", class, text]),
        crate::statements::Scope::Workspace(_) => {
            fingerprint(&["remember", &scope.as_key(), class, text])
        }
    }
}
