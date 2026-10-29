//! Run reports, baselines and the regression gate.
//!
//! A report holds every metric as statistics over the run's repetitions
//! (fresh index each time). A baseline is a report's metrics frozen with the
//! dataset and corpus hashes. The gate fails when a metric is worse than the
//! baseline mean by more than the measured noise:
//! `max(NOISE_SIGMA * stddev, EPSILON)`. Retrieval is deterministic, so its
//! measured stddev is 0 and any drop beyond `EPSILON` fails. Latency and
//! the diagnostics (how many files the results come from, per search stage)
//! are reported, never gated: more files is not better for a question one
//! document answers.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::metrics::{lower_is_better, AnswerScore, CaseRanks, RetrievedChunk};

pub const NOISE_SIGMA: f64 = 3.0;
/// Smallest change treated as real (float noise in means of 0/1 scores).
pub const EPSILON: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Retrieval,
    Answers,
}

impl RunKind {
    pub fn name(self) -> &'static str {
        match self {
            RunKind::Retrieval => "retrieval",
            RunKind::Answers => "answers",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricStats {
    pub mean: f64,
    /// Sample standard deviation (n - 1); 0 for one run.
    pub stddev: f64,
    pub min: f64,
    pub max: f64,
    pub runs: usize,
}

impl MetricStats {
    pub fn from_values(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let n = values.len() as f64;
        let mean = values.iter().sum::<f64>() / n;
        let stddev = if values.len() > 1 {
            (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
        } else {
            0.0
        };
        Some(Self {
            mean,
            stddev,
            min: values.iter().copied().fold(f64::INFINITY, f64::min),
            max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            runs: values.len(),
        })
    }
}

/// Statistics per metric over several runs' metric maps. A metric missing
/// from any run is dropped (it would otherwise look stable when it is not).
pub fn combine_runs(runs: &[BTreeMap<String, f64>]) -> BTreeMap<String, MetricStats> {
    let Some(first) = runs.first() else {
        return BTreeMap::new();
    };
    first
        .keys()
        .filter_map(|name| {
            let values: Option<Vec<f64>> = runs.iter().map(|r| r.get(name).copied()).collect();
            Some((name.clone(), MetricStats::from_values(&values?)?))
        })
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LatencySummary {
    pub count: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

/// Nearest-rank percentiles.
pub fn summarize_latencies(ms: &[f64]) -> LatencySummary {
    if ms.is_empty() {
        return LatencySummary::default();
    }
    let mut sorted = ms.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = |p: f64| {
        let idx = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
        sorted[idx.clamp(1, sorted.len()) - 1]
    };
    LatencySummary {
        count: sorted.len(),
        p50_ms: rank(50.0),
        p95_ms: rank(95.0),
        max_ms: sorted[sorted.len() - 1],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IngestSummary {
    pub files_seen: usize,
    pub files_indexed: usize,
    pub failures: Vec<String>,
    pub chunks: usize,
    pub seconds: f64,
}

/// One case of the last run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CaseRecord {
    pub id: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranks: Option<CaseRanks>,
    /// Document keys of the results, in rank order (distinct).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retrieved: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    /// Answers: from sending the question to the first streamed text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<f64>,
    /// Answers: times the question was asked again after a rate limit.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retries: usize,
    /// Retrieval: the first result (document, pages, start of its text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<TopResult>,
    /// Retrieval: distinct documents among the first n results, by n
    /// (`"10"`, `"20"`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unique_sources: BTreeMap<String, usize>,
    /// Retrieval: whether the index holds every expected passage in a chunk
    /// of its document (`false`: lost before ranking, in parsing or chunking).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passage_indexed: Option<bool>,
    /// Retrieval: what each stage of the search kept (results and files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<shodh_rag::rag_engine::SearchTrace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<AnswerScore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// The first search result of a case, to see what outranked the evidence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TopResult {
    pub key: Option<String>,
    pub pages: Option<(u32, u32)>,
    /// The first [`TOP_TEXT_CHARS`] characters of the chunk.
    pub text: String,
}

/// Characters of the first result's text kept in a report.
pub const TOP_TEXT_CHARS: usize = 160;

impl TopResult {
    pub fn of(chunk: &RetrievedChunk) -> Self {
        Self {
            key: chunk.key.clone(),
            pages: chunk.pages,
            text: chunk.text.chars().take(TOP_TEXT_CHARS).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    pub kind: RunKind,
    pub dataset_id: String,
    pub dataset_hash: String,
    pub corpus_hash: String,
    pub created_at: String,
    /// What was run (k, models, provider): recorded for reproducibility.
    pub settings: BTreeMap<String, String>,
    pub metrics: BTreeMap<String, MetricStats>,
    /// Reported, never gated (see the module docs).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub diagnostics: BTreeMap<String, MetricStats>,
    /// Per search (retrieval) or per answer (answers), across all runs.
    pub latency: LatencySummary,
    /// Answers: from sending the question to the first streamed text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token: Option<LatencySummary>,
    /// Answers: provider failures by kind (`rate_limited`, …), every attempt
    /// counted, retried ones too.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_errors: BTreeMap<String, usize>,
    /// Why the run ended before its last question, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    /// One entry per run.
    pub ingest: Vec<IngestSummary>,
    /// Cases of the last run.
    pub cases: Vec<CaseRecord>,
}

fn write_json<T: Serialize>(value: &T, path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    std::fs::write(path, format!("{json}\n")).with_context(|| format!("writing {}", path.display()))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

impl RunReport {
    pub fn write(&self, path: &Path) -> Result<()> {
        write_json(self, path)
    }

    pub fn read(path: &Path) -> Result<Self> {
        read_json(path)
    }

    /// A short Markdown summary: metrics, latency, ingestion and the cases
    /// that missed.
    pub fn markdown(&self) -> String {
        let mut md = String::new();
        let _ = writeln!(md, "# shodh-eval {} report\n", self.kind.name());
        let _ = writeln!(
            md,
            "Dataset `{}` ({} cases, hash `{}`), corpus hash `{}`, {}.\n",
            self.dataset_id,
            self.cases.len(),
            short(&self.dataset_hash),
            short(&self.corpus_hash),
            self.created_at
        );
        if !self.settings.is_empty() {
            let settings: Vec<String> = self
                .settings
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            let _ = writeln!(md, "Settings: {}\n", settings.join(", "));
        }
        let _ = writeln!(md, "| Metric | Mean | Std | Min | Max | Runs |");
        let _ = writeln!(md, "|---|---|---|---|---|---|");
        for (name, s) in &self.metrics {
            let _ = writeln!(
                md,
                "| {name} | {:.4} | {:.4} | {:.4} | {:.4} | {} |",
                s.mean, s.stddev, s.min, s.max, s.runs
            );
        }
        if !self.diagnostics.is_empty() {
            let _ = writeln!(
                md,
                "\n| Diagnostic (not gated) | Mean | Std | Min | Max | Runs |"
            );
            let _ = writeln!(md, "|---|---|---|---|---|---|");
            for (name, s) in &self.diagnostics {
                let _ = writeln!(
                    md,
                    "| {name} | {:.4} | {:.4} | {:.4} | {:.4} | {} |",
                    s.mean, s.stddev, s.min, s.max, s.runs
                );
            }
        }
        let l = &self.latency;
        let what = match self.kind {
            RunKind::Retrieval => "search",
            RunKind::Answers => "answer",
        };
        let _ = writeln!(
            md,
            "\nLatency per {what} (not gated): p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms over {}.",
            l.p50_ms, l.p95_ms, l.max_ms, l.count
        );
        if let Some(f) = &self.first_token {
            let _ = writeln!(
                md,
                "Time to first token (question sent to first streamed text): p50 {:.1} ms, \
                 p95 {:.1} ms, max {:.1} ms over {}.",
                f.p50_ms, f.p95_ms, f.max_ms, f.count
            );
        }
        if !self.provider_errors.is_empty() {
            let errors: Vec<String> = self
                .provider_errors
                .iter()
                .map(|(kind, n)| format!("{kind} {n}"))
                .collect();
            let _ = writeln!(
                md,
                "Provider errors (every attempt): {}.",
                errors.join(", ")
            );
        }
        if let Some(reason) = &self.stopped {
            let _ = writeln!(md, "Stopped early: {reason}.");
        }
        for (i, ingest) in self.ingest.iter().enumerate() {
            let _ = writeln!(
                md,
                "Ingest run {}: {}/{} files, {} chunks, {:.1} s.",
                i + 1,
                ingest.files_indexed,
                ingest.files_seen,
                ingest.chunks,
                ingest.seconds
            );
            for failure in &ingest.failures {
                let _ = writeln!(md, "- failed: {failure}");
            }
        }
        let misses: Vec<&CaseRecord> = self
            .cases
            .iter()
            .filter(|c| match (&c.ranks, &c.score) {
                (Some(r), _) => r.doc_rank.is_none_or(|rank| rank > 5),
                (None, Some(s)) => {
                    s.fact_recall.is_some_and(|f| f < 1.0)
                        || s.refusal_correct == Some(false)
                        || s.flagged.is_some_and(|f| f > 0)
                }
                (None, None) => c.error.is_some(),
            })
            .collect();
        if !misses.is_empty() {
            let title = match self.kind {
                RunKind::Retrieval => "Expected document not in the top 5",
                RunKind::Answers => {
                    "Answers with missing facts, flags, a wrong refusal or an error"
                }
            };
            let _ = writeln!(md, "\n## {title}\n");
            for case in misses {
                let detail = match (&case.ranks, &case.error) {
                    (_, Some(e)) => format!("error: {e}"),
                    (Some(r), None) => format!(
                        "document rank {}",
                        r.doc_rank.map_or("none".to_string(), |d| d.to_string())
                    ),
                    (None, None) => case
                        .score
                        .as_ref()
                        .map(|s| {
                            format!(
                                "fact recall {}, flagged {}",
                                s.fact_recall.map_or("-".to_string(), |f| format!("{f:.2}")),
                                s.flagged.map_or("-".to_string(), |f| f.to_string())
                            )
                        })
                        .unwrap_or_default(),
                };
                let _ = writeln!(md, "- `{}` {} ({detail})", case.id, case.question);
            }
        }
        md
    }
}

fn short(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub kind: RunKind,
    pub dataset_id: String,
    pub dataset_hash: String,
    pub corpus_hash: String,
    pub recorded_at: String,
    pub settings: BTreeMap<String, String>,
    pub metrics: BTreeMap<String, MetricStats>,
}

impl Baseline {
    pub fn from_report(report: &RunReport) -> Self {
        Self {
            kind: report.kind,
            dataset_id: report.dataset_id.clone(),
            dataset_hash: report.dataset_hash.clone(),
            corpus_hash: report.corpus_hash.clone(),
            recorded_at: report.created_at.clone(),
            settings: report.settings.clone(),
            metrics: report.metrics.clone(),
        }
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        write_json(self, path)
    }

    pub fn read(path: &Path) -> Result<Self> {
        read_json(path)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompareRow {
    pub metric: String,
    pub baseline: f64,
    /// The worst value that still passes.
    pub limit: f64,
    pub current: Option<f64>,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub rows: Vec<CompareRow>,
    /// Reasons the two runs are not comparable at all.
    pub problems: Vec<String>,
}

impl Comparison {
    pub fn passed(&self) -> bool {
        self.problems.is_empty() && self.rows.iter().all(|r| r.passed)
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for problem in &self.problems {
            let _ = writeln!(out, "FAIL {problem}");
        }
        let _ = writeln!(out, "| Metric | Baseline | Limit | Current | |");
        let _ = writeln!(out, "|---|---|---|---|---|");
        for row in &self.rows {
            let current = row
                .current
                .map_or("missing".to_string(), |c| format!("{c:.4}"));
            let verdict = if row.passed { "ok" } else { "FAIL" };
            let _ = writeln!(
                out,
                "| {} | {:.4} | {:.4} | {current} | {verdict} |",
                row.metric, row.baseline, row.limit
            );
        }
        out
    }
}

/// Gate `report` against `baseline`: same kind, dataset and corpus, every
/// baseline metric present and not worse than its noise allows.
pub fn compare(baseline: &Baseline, report: &RunReport) -> Comparison {
    let mut problems = Vec::new();
    if baseline.kind != report.kind {
        problems.push(format!(
            "baseline is a {} run, report is a {} run",
            baseline.kind.name(),
            report.kind.name()
        ));
    }
    if baseline.dataset_hash != report.dataset_hash {
        problems.push(format!(
            "dataset changed (baseline {}, report {}): record a new baseline",
            short(&baseline.dataset_hash),
            short(&report.dataset_hash)
        ));
    }
    if baseline.corpus_hash != report.corpus_hash {
        problems.push(format!(
            "corpus changed (baseline {}, report {}): record a new baseline",
            short(&baseline.corpus_hash),
            short(&report.corpus_hash)
        ));
    }
    let rows = baseline
        .metrics
        .iter()
        .map(|(name, stats)| {
            let tolerance = (NOISE_SIGMA * stats.stddev).max(EPSILON);
            let lower = lower_is_better(name);
            let limit = if lower {
                stats.mean + tolerance
            } else {
                stats.mean - tolerance
            };
            let current = report.metrics.get(name).map(|s| s.mean);
            let passed = current.is_some_and(|c| if lower { c <= limit } else { c >= limit });
            CompareRow {
                metric: name.clone(),
                baseline: stats.mean,
                limit,
                current,
                passed,
            }
        })
        .collect();
    Comparison { rows, problems }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(metrics: &[(&str, &[f64])]) -> RunReport {
        RunReport {
            kind: RunKind::Retrieval,
            dataset_id: "d".into(),
            dataset_hash: "dh".into(),
            corpus_hash: "ch".into(),
            created_at: "t".into(),
            settings: BTreeMap::new(),
            metrics: metrics
                .iter()
                .map(|(k, v)| (k.to_string(), MetricStats::from_values(v).unwrap()))
                .collect(),
            diagnostics: BTreeMap::new(),
            latency: LatencySummary::default(),
            first_token: None,
            provider_errors: BTreeMap::new(),
            stopped: None,
            ingest: vec![],
            cases: vec![],
        }
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let s = summarize_latencies(&[5.0, 1.0, 3.0, 2.0, 4.0]);
        assert_eq!((s.count, s.p50_ms, s.p95_ms, s.max_ms), (5, 3.0, 5.0, 5.0));
        assert_eq!(summarize_latencies(&[]).count, 0);
    }

    #[test]
    fn stats_use_the_sample_deviation() {
        let s = MetricStats::from_values(&[0.5, 0.7]).unwrap();
        assert!((s.mean - 0.6).abs() < 1e-12);
        assert!((s.stddev - 0.141_421_356_237).abs() < 1e-9);
        assert_eq!((s.min, s.max, s.runs), (0.5, 0.7, 2));
        assert!(MetricStats::from_values(&[]).is_none());
    }

    #[test]
    fn runs_combine_only_metrics_every_run_has() {
        let a: BTreeMap<String, f64> = [("x".to_string(), 1.0), ("y".to_string(), 0.5)].into();
        let b: BTreeMap<String, f64> = [("x".to_string(), 0.0)].into();
        let combined = combine_runs(&[a, b]);
        assert_eq!(combined.len(), 1);
        assert_eq!(combined["x"].mean, 0.5);
    }

    #[test]
    fn zero_noise_fails_any_drop_and_passes_equal_or_better() {
        let base = Baseline::from_report(&report(&[("hit@5", &[0.8, 0.8, 0.8])]));
        assert!(!compare(&base, &report(&[("hit@5", &[0.7999])])).passed());
        assert!(compare(&base, &report(&[("hit@5", &[0.8])])).passed());
        assert!(compare(&base, &report(&[("hit@5", &[0.9])])).passed());
    }

    #[test]
    fn measured_noise_widens_the_limit_by_three_sigma() {
        let base = Baseline::from_report(&report(&[("m", &[0.50, 0.52, 0.54])]));
        // mean 0.52, stddev 0.02 -> limit 0.46
        assert!(compare(&base, &report(&[("m", &[0.47])])).passed());
        assert!(!compare(&base, &report(&[("m", &[0.45])])).passed());
    }

    #[test]
    fn lower_is_better_metrics_fail_on_a_rise() {
        let base = Baseline::from_report(&report(&[("flagged_per_answer", &[1.0])]));
        assert!(!compare(&base, &report(&[("flagged_per_answer", &[1.5])])).passed());
        assert!(compare(&base, &report(&[("flagged_per_answer", &[0.5])])).passed());
    }

    #[test]
    fn missing_metrics_and_changed_inputs_fail() {
        let base = Baseline::from_report(&report(&[("a", &[1.0]), ("b", &[1.0])]));
        let missing = compare(&base, &report(&[("a", &[1.0])]));
        assert!(!missing.passed());
        assert!(missing
            .render()
            .contains("| b | 1.0000 | 1.0000 | missing | FAIL |"));
        let mut changed = report(&[("a", &[1.0]), ("b", &[1.0])]);
        changed.corpus_hash = "other".into();
        let outcome = compare(&base, &changed);
        assert!(!outcome.passed());
        assert!(outcome.problems[0].contains("corpus changed"));
        changed.corpus_hash = "ch".into();
        changed.dataset_hash = "other".into();
        assert!(!compare(&base, &changed).passed());
    }

    #[test]
    fn reports_round_trip_and_render() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.json");
        let mut r = report(&[("hit@5", &[1.0])]);
        r.cases.push(CaseRecord {
            id: "c1".into(),
            question: "q?".into(),
            ranks: Some(CaseRanks::default()),
            ..CaseRecord::default()
        });
        r.write(&path).unwrap();
        assert_eq!(RunReport::read(&path).unwrap(), r);
        let md = r.markdown();
        assert!(md.contains("| hit@5 | 1.0000 |"));
        assert!(md.contains("`c1` q? (document rank none)"));
        assert!(!md.contains("Time to first token"));
        assert!(!md.contains("Diagnostic (not gated)"));
        r.diagnostics.insert(
            "unique_sources@10".into(),
            MetricStats::from_values(&[3.5]).unwrap(),
        );
        r.write(&path).unwrap();
        assert_eq!(RunReport::read(&path).unwrap(), r);
        assert!(r.markdown().contains("| unique_sources@10 | 3.5000 |"));
        r.first_token = Some(summarize_latencies(&[300.0, 900.0]));
        r.provider_errors.insert("rate_limited".into(), 2);
        r.stopped = Some("quota used up".into());
        let md = r.markdown();
        assert!(md.contains(
            "Time to first token (question sent to first streamed text): p50 300.0 ms, \
             p95 900.0 ms, max 900.0 ms over 2."
        ));
        assert!(md.contains("Provider errors (every attempt): rate_limited 2."));
        assert!(md.contains("Stopped early: quota used up."));
    }
}
