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
use shodh_rag::harness::tools::documents::OpenDocumentTool;
use shodh_rag::harness::tools::plan::UpdatePlanTool;
use shodh_rag::harness::tools::search::{SearchDocumentsTool, SEARCH_DOCUMENTS};
use shodh_rag::harness::tools::sources::ListSourcesTool;
use shodh_rag::harness::tools::ToolRegistry;
use shodh_rag::harness::web::relevance::SharedScorer;
use shodh_rag::harness::{
    fetch_omp, resolve_binary_path, select_model, AgentEvent, AgentHarness, LaunchSpec, OmpLayout,
    OmpModel, OmpSession, SessionConfig,
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

/// The document tools an answer may use (what the app registers for them).
const TOOLS: [&str; 4] = [
    SEARCH_DOCUMENTS,
    "open_document",
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
    report: Option<GroundingReport>,
    superseded: BTreeSet<String>,
    status: Option<(RunStatus, Option<String>)>,
}

impl Collector {
    pub fn new(root_key: &str) -> Self {
        Self {
            root_key: root_key.to_string(),
            ..Self::default()
        }
    }

    /// Take one event; true when the run has finished.
    pub fn observe(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::StepStarted { step_id, tool, .. } => {
                self.tools.insert(step_id.clone(), tool.clone());
            }
            AgentEvent::StepFinished {
                step_id,
                ok: true,
                detail: Some(detail),
                ..
            } if self.tools.get(step_id).map(String::as_str) == Some(SEARCH_DOCUMENTS) => {
                self.add_passages(detail);
            }
            AgentEvent::TextDelta {
                message_id, delta, ..
            } => match self.messages.iter_mut().find(|(id, _)| id == message_id) {
                Some((_, text)) => text.push_str(delta),
                None => self.messages.push((message_id.clone(), delta.clone())),
            },
            AgentEvent::Grounding { report, .. } => {
                self.superseded
                    .extend(report.superseded_message_ids.iter().cloned());
                if report.is_final {
                    self.report = Some(report.clone());
                }
            }
            AgentEvent::RunFinished { status, error, .. } => {
                self.status = Some((*status, error.clone()));
                return true;
            }
            _ => {}
        }
        false
    }

    fn add_passages(&mut self, detail: &Value) {
        let Some(list) = detail.get("passages").and_then(Value::as_array) else {
            return;
        };
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
        }
    }

    pub fn transcript(&self) -> AnswerTranscript {
        // The answer is the text blocks the final check covered; without a
        // check, every block that no revision replaced.
        let answer_ids: Vec<&str> = match &self.report {
            Some(r) if !r.message_ids.is_empty() => {
                r.message_ids.iter().map(String::as_str).collect()
            }
            _ => self
                .messages
                .iter()
                .map(|(id, _)| id.as_str())
                .filter(|id| !self.superseded.contains(*id))
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
            cited,
            grounding,
            error,
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
                        if collector.observe(&event) {
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
    let mut records = Vec::with_capacity(cases.len());
    for (i, case) in cases.iter().enumerate() {
        eprintln!("[{}/{}] {}", i + 1, cases.len(), case.id);
        let started = Instant::now();
        let transcript = answerer
            .answer(case, &index.root_key)
            .await
            .unwrap_or_else(|e| AnswerTranscript {
                error: Some(format!("{e:#}")),
                ..AnswerTranscript::default()
            });
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let expected: BTreeSet<String> = expected_map(&index.root, case).into_keys().collect();
        let score = score_answer(case, &expected, &transcript);
        if transcript.error.is_some() {
            errors += 1;
        } else {
            latencies.push(ms);
            scores.push(score.clone());
        }
        records.push(CaseRecord {
            id: case.id.clone(),
            question: case.question.clone(),
            latency_ms: Some(ms),
            answer: Some(transcript.answer.clone()),
            score: Some(score),
            error: transcript.error.clone(),
            ..CaseRecord::default()
        });
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
        latency: summarize_latencies(&latencies),
        ingest: vec![index.ingest.clone()],
        cases: records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use shodh_rag::harness::events::{
        ClaimCheck, ClaimKind, ClaimOutcome, GroundingSummary, ScoringMethod,
    };

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
            },
            delta("m1", "Draft answer."),
            delta("m2", "The total is INR 7,96,500"),
            delta("m2", " [1]."),
            AgentEvent::Grounding {
                run_id: "r".into(),
                report: report(&["m2"], &["m1"], &[2]),
            },
        ];
        for e in &events {
            assert!(!c.observe(e));
        }
        assert!(c.observe(&finished(RunStatus::Completed)));
        let t = c.transcript();
        assert_eq!(t.answer, "The total is INR 7,96,500 [1].");
        assert_eq!(t.cited, [1, 2].into());
        assert_eq!(t.passages[&1].key.as_deref(), Some("invoices/a.pdf"));
        assert_eq!(t.passages[&1].page.as_deref(), Some("2"));
        assert_eq!(t.passages[&2].key, None);
        assert_eq!(t.grounding.unwrap().unsupported, 1);
        assert!(t.error.is_none());
    }

    #[test]
    fn unfinished_or_failed_runs_are_errors() {
        let mut c = Collector::new("c:/corpus");
        c.observe(&delta("m1", "partial"));
        assert!(c.transcript().error.is_some());
        assert!(c.observe(&finished(RunStatus::Error)));
        let t = c.transcript();
        assert!(t.error.unwrap().contains("Error"));
        assert_eq!(t.answer, "partial");
    }

    #[test]
    fn session_ids_are_safe() {
        assert_eq!(uuid_like("MSA payment/1"), "msa-payment-1");
    }
}
