//! Answer evaluation through the app's answering path: an omp agent session
//! with the library's document tools (`search_documents`, `open_document`,
//! `list_sources`, `update_plan`) and the grounding verifier (reranker and,
//! when installed, the answer-checking NLI model; follow-up repair on), one
//! fresh session per question.
//!
//! Needs a configured language model ([`crate::provider`]) and sends
//! passages of the corpus to it: never run in CI, and only on folders whose
//! content may go to that provider.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;
use shodh_rag::embeddings::model_store::ANSWER_CHECK_DIR;
use shodh_rag::harness::events::{GroundingReport, RunStatus};
use shodh_rag::harness::grounding::citations::cited_numbers;
use shodh_rag::harness::grounding::{GroundingConfig, ScorerSet, SharedEntailment};
use shodh_rag::harness::profile::AgentProfile;
use shodh_rag::harness::tools::documents::{OpenDocumentTool, OPEN_DOCUMENT};
use shodh_rag::harness::tools::plan::UpdatePlanTool;
use shodh_rag::harness::tools::search::{SearchDocumentsTool, SEARCH_DOCUMENTS};
use shodh_rag::harness::tools::sources::ListSourcesTool;
use shodh_rag::harness::tools::ToolRegistry;
use shodh_rag::harness::web::relevance::SharedScorer;
use shodh_rag::harness::{
    fetch_omp, resolve_binary_path, select_model, AgentEvent, AgentHarness, LaunchSpec, OmpLayout,
    OmpModel, OmpSession, ProviderError, ProviderErrorKind, SessionConfig, Thinking,
};
use shodh_rag::reranking::NliModel;

use crate::corpus::{corpus_hash, relative_key};
use crate::dataset::{Dataset, EvalCase};
use crate::engine::{index_corpus, settings};
use crate::metrics::{
    answer_metrics, score_answer, AnswerScore, AnswerTranscript, GroundingCounts, SeenPassage,
};
use crate::provider::ProviderChoice;
use crate::report::{combine_runs, summarize_latencies, CaseRecord, RunKind, RunReport};
use crate::retrieval::expected_map;

/// Longest one answer may take (searches, answer, check and follow-ups).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Waits before asking again after a rate limit, when the provider gives no
/// time; one entry per retry, so a question is tried at most five times.
const RATE_LIMIT_BACKOFF_SECS: [u64; 4] = [20, 40, 80, 160];

/// Longest wait honoured from a provider's own retry hint.
const MAX_RETRY_AFTER_SECS: u64 = 300;

/// How long to wait before retrying a question whose run ended in `error`,
/// `retries` retries in: only rate limits are retried (a used-up daily quota,
/// a bad key or an unknown model do not recover by waiting); `None` when
/// the question is not to be retried.
fn rate_limit_wait(error: Option<&ProviderError>, retries: usize) -> Option<Duration> {
    let error = error.filter(|e| e.kind == ProviderErrorKind::RateLimited)?;
    let backoff = *RATE_LIMIT_BACKOFF_SECS.get(retries)?;
    let secs = error
        .retry_after_secs
        .map_or(backoff, |s| u64::from(s).clamp(1, MAX_RETRY_AFTER_SECS));
    Some(Duration::from_secs(secs))
}

/// The document tools an answer may use (what the app registers for them).
const TOOLS: [&str; 4] = [
    SEARCH_DOCUMENTS,
    OPEN_DOCUMENT,
    "list_sources",
    "update_plan",
];

pub struct AnswerOptions {
    pub corpus: PathBuf,
    pub dataset: Dataset,
    pub models: PathBuf,
    /// Holds the agent runtime (`bin/omp`) and its session files.
    pub runtime_dir: PathBuf,
    pub limit: Option<usize>,
    pub timeout: Duration,
}

/// Builds an [`AnswerTranscript`] from a run's event stream.
#[derive(Debug, Default)]
pub struct Collector {
    root_key: String,
    tools: HashMap<String, String>,
    messages: Vec<(String, String)>,
    passages: BTreeMap<u32, SeenPassage>,
    searches: Vec<Vec<u32>>,
    first_token_ms: Option<f64>,
    report: Option<GroundingReport>,
    superseded: BTreeSet<String>,
    /// Text blocks that are working notes before tool calls, not answer.
    interim: BTreeSet<String>,
    status: Option<(RunStatus, Option<String>)>,
    provider_error: Option<ProviderError>,
}

impl Collector {
    pub fn new(root_key: &str) -> Self {
        Self {
            root_key: root_key.to_string(),
            ..Self::default()
        }
    }

    /// Take one event, `elapsed` after the question was sent; true when
    /// the run has finished.
    pub fn observe(&mut self, event: &AgentEvent, elapsed: Duration) -> bool {
        match event {
            AgentEvent::StepStarted { step_id, tool, .. } => {
                self.tools.insert(step_id.clone(), tool.clone());
            }
            AgentEvent::StepFinished {
                step_id,
                ok: true,
                detail: Some(detail),
                ..
            } => match self.tools.get(step_id).map(String::as_str) {
                // Both number passages from one sequence; an answer may cite
                // what it read as well as what it searched.
                Some(SEARCH_DOCUMENTS) => {
                    let numbers = self.add_passages(detail);
                    self.searches.push(numbers);
                }
                Some(OPEN_DOCUMENT) => {
                    self.add_passages(detail);
                }
                _ => {}
            },
            AgentEvent::TextDelta {
                message_id, delta, ..
            } => {
                if self.first_token_ms.is_none() && !delta.trim().is_empty() {
                    self.first_token_ms = Some(elapsed.as_secs_f64() * 1000.0);
                }
                match self.messages.iter_mut().find(|(id, _)| id == message_id) {
                    Some((_, text)) => text.push_str(delta),
                    None => self.messages.push((message_id.clone(), delta.clone())),
                }
            }
            AgentEvent::TextInterim {
                message_id,
                interim,
                ..
            } => {
                if *interim {
                    self.interim.insert(message_id.clone());
                } else {
                    self.interim.remove(message_id);
                }
            }
            AgentEvent::Grounding { report, .. } => {
                self.superseded
                    .extend(report.superseded_message_ids.iter().cloned());
                if report.is_final {
                    self.report = Some(report.clone());
                }
            }
            AgentEvent::RunFinished {
                status,
                error,
                provider_error,
                ..
            } => {
                self.status = Some((*status, error.clone()));
                self.provider_error = provider_error.clone();
                return true;
            }
            _ => {}
        }
        false
    }

    /// Record a step's numbered passages; their numbers in the step's order.
    fn add_passages(&mut self, detail: &Value) -> Vec<u32> {
        let Some(list) = detail.get("passages").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut numbers = Vec::with_capacity(list.len());
        for p in list {
            let Some(n) = p
                .get("n")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
            else {
                continue;
            };
            let text = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
            self.passages.insert(
                n,
                SeenPassage {
                    key: text("path").and_then(|path| relative_key(&self.root_key, &path)),
                    page: text("page"),
                    text: text("text").unwrap_or_default(),
                },
            );
            numbers.push(n);
        }
        numbers
    }

    pub fn transcript(&self) -> AnswerTranscript {
        // The answer is the text blocks the final check covered; without a
        // check, every block that no revision replaced and that is not a
        // working note before tool calls.
        let answer_ids: Vec<&str> = match &self.report {
            Some(r) if !r.message_ids.is_empty() => {
                r.message_ids.iter().map(String::as_str).collect()
            }
            _ => self
                .messages
                .iter()
                .map(|(id, _)| id.as_str())
                .filter(|id| !self.superseded.contains(*id) && !self.interim.contains(*id))
                .collect(),
        };
        let answer = self
            .messages
            .iter()
            .filter(|(id, _)| answer_ids.contains(&id.as_str()))
            .map(|(_, text)| text.trim())
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut cited = cited_numbers(&answer);
        if let Some(report) = &self.report {
            cited.extend(report.claims.iter().flat_map(|c| c.cited.iter().copied()));
        }
        let grounding = self.report.as_ref().map(|r| GroundingCounts {
            checked: r.summary.checked,
            supported: r.summary.supported,
            weak: r.summary.weak,
            unsupported: r.summary.unsupported,
            uncited: r.summary.uncited,
            invalid: r.summary.invalid,
            unchecked: r.summary.unchecked,
        });
        let error = match &self.status {
            Some((RunStatus::Completed, _)) => None,
            Some((status, error)) => Some(format!(
                "run {status:?}: {}",
                error.as_deref().unwrap_or("no reason given")
            )),
            None => Some("the run did not finish".to_string()),
        };
        AnswerTranscript {
            answer,
            passages: self.passages.clone(),
            searches: self.searches.clone(),
            cited,
            grounding,
            first_token_ms: self.first_token_ms,
            error,
            provider_error: self.provider_error.clone(),
        }
    }
}

fn grounding_config(
    nli: Option<SharedEntailment>,
    reranker: shodh_rag::rag_engine::SharedReranker,
) -> GroundingConfig {
    GroundingConfig {
        scorers: Arc::new(move || ScorerSet {
            relevance: reranker.get().map(|r| r as SharedScorer),
            entailment: nli.clone(),
        }),
        auto_repair: Arc::new(|| true),
        follow_ups: true,
    }
}

async fn load_nli(models: &Path) -> Result<Option<SharedEntailment>> {
    let dir = models.join(ANSWER_CHECK_DIR);
    if !dir.is_dir() {
        return Ok(None);
    }
    let model = tokio::task::spawn_blocking(move || NliModel::new(&dir))
        .await
        .context("loading the answer-checking model")?
        .context("loading the answer-checking model")?;
    Ok(Some(Arc::new(model) as SharedEntailment))
}

async fn runtime(dir: &Path) -> Result<PathBuf> {
    let binary = resolve_binary_path(dir);
    if binary.exists() {
        return Ok(binary);
    }
    eprintln!(
        "Downloading the pinned agent runtime into {}",
        dir.display()
    );
    let installed = fetch_omp(dir, &|_, _| {})
        .await
        .context("downloading the agent runtime")?;
    Ok(installed.path)
}

struct Answerer {
    binary: PathBuf,
    layout_dir: PathBuf,
    model: OmpModel,
    profile: AgentProfile,
    registry: Arc<ToolRegistry>,
    system_prompt: String,
    grounding: GroundingConfig,
    timeout: Duration,
}

impl Answerer {
    async fn answer(&self, case: &EvalCase, root_key: &str) -> Result<AnswerTranscript> {
        let launch = LaunchSpec {
            binary: self.binary.clone(),
            layout: OmpLayout::new(&self.layout_dir),
            model: self.model.clone(),
            system_prompt: self.system_prompt.clone(),
            session_id: format!("eval-{}", uuid_like(&case.id)),
            code: None,
            host_tools: Vec::new(),
            // As the app answers by default ("Think before answering" off).
            thinking: Thinking::default(),
        };
        let (session, mut events) = OmpSession::start(SessionConfig {
            launch,
            profile: self.profile.clone(),
            registry: self.registry.clone(),
            audit: None,
            grounding: Some(self.grounding.clone()),
            code: None,
        })
        .await
        .context("starting the agent session")?;
        let mut collector = Collector::new(root_key);
        let sent = Instant::now();
        let outcome = async {
            session.prompt(&case.question, None).await?;
            let deadline = tokio::time::Instant::now() + self.timeout;
            loop {
                match tokio::time::timeout_at(deadline, events.recv()).await {
                    Ok(Some(event)) => {
                        // Document tools never write; refuse anything that asks.
                        if let AgentEvent::ApprovalRequested { step_id, .. } = &event {
                            let _ = session.approve(step_id, false);
                        }
                        if collector.observe(&event, sent.elapsed()) {
                            return Ok(());
                        }
                    }
                    Ok(None) => bail!("the agent session closed before the answer finished"),
                    Err(_) => {
                        let _ = session.abort().await;
                        bail!("no answer within {} s", self.timeout.as_secs());
                    }
                }
            }
        }
        .await;
        session.shutdown().await;
        outcome.map(|()| collector.transcript())
    }
}

/// Session ids are passed to the runtime; keep them to `[a-z0-9-]`.
fn uuid_like(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

pub async fn run(opts: &AnswerOptions) -> Result<RunReport> {
    let choice = ProviderChoice::from_env()?;
    let model = select_model(&choice.mode(), |_| None).context("choosing the model")?;
    let cases: Vec<&EvalCase> = opts
        .dataset
        .active_cases()
        .take(opts.limit.unwrap_or(usize::MAX))
        .collect();
    if cases.is_empty() {
        bail!("no cases to answer");
    }
    eprintln!(
        "Answering {} questions with {}: passages of {} are sent to this model.",
        cases.len(),
        choice.describe(),
        opts.corpus.display()
    );
    let corpus_hash = corpus_hash(&opts.corpus)?;
    let binary = runtime(&opts.runtime_dir).await?;
    let index = index_corpus(&opts.corpus, &opts.models).await?;
    let nli = load_nli(&opts.models).await?;
    let method = if nli.is_some() {
        "entailment"
    } else {
        "cross_encoder"
    };
    let reranker = index.rag.read().await.reranker_handle();

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(SearchDocumentsTool::new(index.rag.clone())))?;
    registry.register(Arc::new(OpenDocumentTool::new(index.rag.clone())))?;
    registry.register(Arc::new(ListSourcesTool::new(index.rag.clone())))?;
    registry.register(Arc::new(UpdatePlanTool))?;
    let profile = AgentProfile {
        allowed_tools: TOOLS.iter().map(|t| t.to_string()).collect(),
        ..AgentProfile::assistant()
    };
    let system_prompt = profile.system_prompt(&registry.capability_manifest(&profile, &[]));
    let answerer = Answerer {
        binary,
        layout_dir: opts.runtime_dir.clone(),
        model,
        profile,
        registry: Arc::new(registry),
        system_prompt,
        grounding: grounding_config(nli, reranker),
        timeout: opts.timeout,
    };

    let mut scores: Vec<AnswerScore> = Vec::new();
    let mut errors = 0usize;
    let mut latencies = Vec::new();
    let mut first_tokens = Vec::new();
    let mut provider_errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut stopped = None;
    let mut records = Vec::with_capacity(cases.len());
    for (i, case) in cases.iter().enumerate() {
        eprintln!("[{}/{}] {}", i + 1, cases.len(), case.id);
        let mut retries = 0usize;
        // Time of the last attempt only: waits for a rate limit are not
        // the answer's latency.
        let (transcript, ms) = loop {
            let started = Instant::now();
            let transcript = answerer
                .answer(case, &index.root_key)
                .await
                .unwrap_or_else(|e| AnswerTranscript {
                    error: Some(format!("{e:#}")),
                    ..AnswerTranscript::default()
                });
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            if let Some(e) = &transcript.provider_error {
                *provider_errors.entry(kind_name(e.kind)).or_default() += 1;
            }
            match rate_limit_wait(transcript.provider_error.as_ref(), retries) {
                Some(wait) => {
                    retries += 1;
                    eprintln!(
                        "  rate limited; asking again in {} s (retry {retries})",
                        wait.as_secs()
                    );
                    tokio::time::sleep(wait).await;
                }
                None => break (transcript, ms),
            }
        };
        let expected = expected_map(&index.root, case);
        let score = score_answer(case, &expected, &transcript);
        let quota_used_up = transcript
            .provider_error
            .as_ref()
            .is_some_and(|e| e.kind == ProviderErrorKind::QuotaExhausted);
        // A used-up quota is not an answer the model failed: the question is
        // left out of the metrics.
        if transcript.error.is_some() {
            errors += usize::from(!quota_used_up);
        } else {
            latencies.push(ms);
            first_tokens.extend(transcript.first_token_ms);
            scores.push(score.clone());
        }
        records.push(CaseRecord {
            id: case.id.clone(),
            question: case.question.clone(),
            latency_ms: Some(ms),
            first_token_ms: transcript.first_token_ms,
            retries,
            answer: Some(transcript.answer.clone()),
            score: Some(score),
            error: transcript.error.clone(),
            ..CaseRecord::default()
        });
        if quota_used_up {
            // Every later question would fail the same way and count as
            // unanswered; stop and say so instead.
            let reason = format!(
                "the provider's quota is used up: stopped after {} of {} questions",
                i + 1,
                cases.len()
            );
            eprintln!("{reason}");
            stopped = Some(reason);
            break;
        }
    }

    let mut settings = settings();
    settings.insert("model".to_string(), choice.describe());
    settings.insert("grounding".to_string(), method.to_string());
    settings.insert("tools".to_string(), TOOLS.join(","));
    Ok(RunReport {
        kind: RunKind::Answers,
        dataset_id: opts.dataset.id.clone(),
        dataset_hash: opts.dataset.content_hash(),
        corpus_hash,
        created_at: chrono::Utc::now().to_rfc3339(),
        settings,
        metrics: combine_runs(&[answer_metrics(&scores, errors)]),
        diagnostics: BTreeMap::new(),
        latency: summarize_latencies(&latencies),
        first_token: Some(summarize_latencies(&first_tokens)),
        provider_errors,
        stopped,
        ingest: vec![index.ingest.clone()],
        cases: records,
    })
}

/// A provider error kind as reports name it (`rate_limited`, …).
fn kind_name(kind: ProviderErrorKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{kind:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use shodh_rag::harness::events::{
        ClaimCheck, ClaimKind, ClaimOutcome, GroundingSummary, ScoringMethod,
    };

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn finished(status: RunStatus) -> AgentEvent {
        AgentEvent::RunFinished {
            run_id: "r".into(),
            status,
            duration_ms: 1,
            error: None,
            provider_error: None,
        }
    }

    fn delta(id: &str, text: &str) -> AgentEvent {
        AgentEvent::TextDelta {
            run_id: "r".into(),
            message_id: id.into(),
            delta: text.into(),
        }
    }

    fn report(message_ids: &[&str], superseded: &[&str], cited: &[u32]) -> GroundingReport {
        GroundingReport {
            round: 1,
            is_final: true,
            method: ScoringMethod::CrossEncoder,
            summary: GroundingSummary {
                checked: 2,
                supported: 1,
                unsupported: 1,
                ..GroundingSummary::default()
            },
            claims: vec![ClaimCheck {
                message_id: "m2".into(),
                text: "claim".into(),
                anchor: "claim".into(),
                kind: ClaimKind::Sentence,
                outcome: ClaimOutcome::Supported,
                cited: cited.to_vec(),
                invalid: vec![],
                support: Some(0.9),
                contradiction: None,
                missing_numbers: vec![],
                closest: None,
                closest_score: None,
                suggested: None,
                suggested_score: None,
                suggested_contradiction: None,
            }],
            needs: vec![],
            message_ids: message_ids.iter().map(|s| s.to_string()).collect(),
            superseded_message_ids: superseded.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn transcripts_keep_the_final_answer_its_passages_and_citations() {
        let root = "c:/corpus";
        let mut c = Collector::new(root);
        let events = vec![
            AgentEvent::StepStarted {
                run_id: "r".into(),
                step_id: "s1".into(),
                parent_step_id: None,
                tool: SEARCH_DOCUMENTS.into(),
                label: "Search".into(),
                args: json!({}),
                tier: shodh_rag::harness::RiskTier::Read,
                at_ms: 0,
            },
            AgentEvent::StepFinished {
                run_id: "r".into(),
                step_id: "s1".into(),
                ok: true,
                summary: "2 passages".into(),
                detail: Some(json!({"passages": [
                    {"n": 1, "path": "c:/corpus/invoices/a.pdf", "page": "2", "text": "Total INR 7,96,500.00", "file": "a.pdf", "score": 0.5},
                    {"n": 2, "path": "c:/elsewhere/b.pdf", "text": "other", "file": "b.pdf", "score": 0.1}
                ]})),
                duration_ms: 3,
                invalid_arguments: false,
            },
            delta("m1", "Draft answer."),
            delta("m2", "The total is INR 7,96,500"),
            delta("m2", " [1]."),
            AgentEvent::Grounding {
                run_id: "r".into(),
                report: report(&["m2"], &["m1"], &[2]),
            },
        ];
        for (i, e) in events.iter().enumerate() {
            assert!(!c.observe(e, ms(10 * i as u64)));
        }
        assert!(c.observe(&finished(RunStatus::Completed), ms(100)));
        let t = c.transcript();
        assert_eq!(t.answer, "The total is INR 7,96,500 [1].");
        assert_eq!(t.searches, vec![vec![1, 2]]);
        assert_eq!(
            t.first_token_ms,
            Some(20.0),
            "the first text, a draft included"
        );
        assert_eq!(t.cited, [1, 2].into());
        assert_eq!(t.passages[&1].key.as_deref(), Some("invoices/a.pdf"));
        assert_eq!(t.passages[&1].page.as_deref(), Some("2"));
        assert_eq!(t.passages[&2].key, None);
        assert_eq!(t.grounding.unwrap().unsupported, 1);
        assert!(t.error.is_none());
    }

    #[test]
    fn notes_before_tool_calls_are_not_the_answer_without_a_check() {
        let mut c = Collector::new("c:/corpus");
        for e in [
            delta("m1", "I'll search for the monthly retainer."),
            AgentEvent::TextInterim {
                run_id: "r".into(),
                message_id: "m1".into(),
                interim: true,
            },
            delta("m2", "The retainer is INR 2,75,000 [11]."),
        ] {
            c.observe(&e, ms(1));
        }
        c.observe(&finished(RunStatus::Completed), ms(2));
        assert_eq!(c.transcript().answer, "The retainer is INR 2,75,000 [11].");
    }

    #[test]
    fn unfinished_or_failed_runs_are_errors() {
        let mut c = Collector::new("c:/corpus");
        c.observe(&delta("m1", "partial"), ms(5));
        assert!(c.transcript().error.is_some());
        assert!(c.observe(&finished(RunStatus::Error), ms(6)));
        let t = c.transcript();
        assert!(t.error.unwrap().contains("Error"));
        assert_eq!(t.answer, "partial");
        assert_eq!(t.provider_error, None);
    }

    fn step(id: &str, tool: &str, passages: Value) -> [AgentEvent; 2] {
        [
            AgentEvent::StepStarted {
                run_id: "r".into(),
                step_id: id.into(),
                parent_step_id: None,
                tool: tool.into(),
                label: tool.into(),
                args: json!({}),
                tier: shodh_rag::harness::RiskTier::Read,
                at_ms: 0,
            },
            AgentEvent::StepFinished {
                run_id: "r".into(),
                step_id: id.into(),
                ok: true,
                summary: String::new(),
                detail: Some(json!({ "passages": passages })),
                duration_ms: 1,
                invalid_arguments: false,
            },
        ]
    }

    #[test]
    fn passages_read_with_open_document_can_be_cited() {
        let mut c = Collector::new("c:/corpus");
        let events = [
            step(
                "s1",
                SEARCH_DOCUMENTS,
                json!([{"n": 1, "path": "c:/corpus/msa.pdf", "page": "1", "text": "Master Services Agreement between Northwind and Bluepeak"}]),
            ),
            step(
                "s2",
                OPEN_DOCUMENT,
                json!([{"n": 2, "path": "c:/corpus/msa.pdf", "page": "3", "text": "within forty-five (45) days of receipt"}]),
            ),
            step(
                "s3",
                "list_sources",
                json!([{"n": 3, "path": "c:/corpus/other.pdf", "text": "not a passage"}]),
            ),
        ];
        for e in events.iter().flatten() {
            c.observe(e, ms(1));
        }
        c.observe(&delta("m1", "Payment is due within 45 days [2]."), ms(2));
        c.observe(&finished(RunStatus::Completed), ms(3));
        let t = c.transcript();
        assert_eq!(t.cited, [2].into());
        assert_eq!(t.passages[&2].key.as_deref(), Some("msa.pdf"));
        assert_eq!(t.passages[&2].page.as_deref(), Some("3"));
        assert!(
            !t.passages.contains_key(&3),
            "only document tools number passages"
        );
        assert_eq!(t.searches, vec![vec![1]], "reads are not searches");
    }

    #[test]
    fn provider_failures_are_kept_with_the_transcript() {
        let mut c = Collector::new("c:/corpus");
        let limit = ProviderError {
            kind: ProviderErrorKind::RateLimited,
            status: Some(429),
            retry_after_secs: Some(12),
            upstream: None,
            resets_at_ms: None,
        };
        c.observe(
            &AgentEvent::RunFinished {
                run_id: "r".into(),
                status: RunStatus::Error,
                duration_ms: 1,
                error: Some("429 rate limit".into()),
                provider_error: Some(limit.clone()),
            },
            ms(1),
        );
        let t = c.transcript();
        assert_eq!(t.provider_error, Some(limit));
        assert!(t.error.is_some());
        assert_eq!(t.first_token_ms, None);
    }

    fn provider(kind: ProviderErrorKind, retry_after_secs: Option<u32>) -> ProviderError {
        ProviderError {
            kind,
            status: None,
            retry_after_secs,
            upstream: None,
            resets_at_ms: None,
        }
    }

    #[test]
    fn only_rate_limits_are_retried_with_backoff() {
        let limited = provider(ProviderErrorKind::RateLimited, None);
        let waits: Vec<Option<u64>> = (0..5)
            .map(|r| rate_limit_wait(Some(&limited), r).map(|d| d.as_secs()))
            .collect();
        assert_eq!(waits, [Some(20), Some(40), Some(80), Some(160), None]);
        let hinted = provider(ProviderErrorKind::RateLimited, Some(12));
        assert_eq!(
            rate_limit_wait(Some(&hinted), 0),
            Some(Duration::from_secs(12))
        );
        let huge = provider(ProviderErrorKind::RateLimited, Some(86_400));
        assert_eq!(
            rate_limit_wait(Some(&huge), 0),
            Some(Duration::from_secs(MAX_RETRY_AFTER_SECS))
        );
        for kind in [
            ProviderErrorKind::QuotaExhausted,
            ProviderErrorKind::Auth,
            ProviderErrorKind::ModelUnavailable,
            ProviderErrorKind::Other,
        ] {
            assert_eq!(rate_limit_wait(Some(&provider(kind, Some(5))), 0), None);
        }
        assert_eq!(rate_limit_wait(None, 0), None);
        assert_eq!(
            kind_name(ProviderErrorKind::QuotaExhausted),
            "quota_exhausted"
        );
    }

    #[test]
    fn session_ids_are_safe() {
        assert_eq!(uuid_like("MSA payment/1"), "msa-payment-1");
    }
}
