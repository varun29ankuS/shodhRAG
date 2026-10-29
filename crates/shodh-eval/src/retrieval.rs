//! Retrieval evaluation: index the corpus into a fresh index, search every
//! answerable question the way the agent's `search_documents` tool does
//! (`search_comprehensive`, no filter), and rank the expected evidence.
//! No LLM is involved, so this runs in CI.
//!
//! Each question is searched twice: with `k` results (every metric but
//! `recall@20`) and with [`WIDE_K`], the most the agent's tool asks for
//! (`recall@20` and how many files the results come from).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};

use crate::corpus::{corpus_hash, expected_key, relative_key};
use crate::dataset::{Dataset, EvalCase};
use crate::engine::{index_corpus_with, settings_with, IndexedCorpus, Tweaks};
use crate::metrics::{rank_case, retrieval_metrics, unique_sources, RetrievedChunk};
use crate::report::{combine_runs, summarize_latencies, CaseRecord, RunKind, RunReport, TopResult};
use crate::text::contains_span;

/// Results fetched per question (the top of the agent tool's range).
pub const DEFAULT_K: usize = 10;

/// Results of the second search per question: the most `search_documents`
/// returns.
pub const WIDE_K: usize = 20;

pub struct RetrievalOptions {
    /// Absolute corpus folder.
    pub corpus: PathBuf,
    pub dataset: Dataset,
    pub models: PathBuf,
    /// Repetitions, each on a freshly built index.
    pub runs: usize,
    pub k: usize,
    /// OCR engine and render resolutions for scanned files.
    pub ocr: shodh_rag::config::OcrConfig,
    /// Engine settings changed from the app's defaults.
    pub tweaks: Tweaks,
}

/// Expected document key -> (pages, passage) for one case.
pub fn expected_map(
    corpus: &std::path::Path,
    case: &EvalCase,
) -> BTreeMap<String, (Vec<u32>, Option<String>)> {
    let mut map: BTreeMap<String, (Vec<u32>, Option<String>)> = BTreeMap::new();
    for source in &case.sources {
        let entry = map.entry(expected_key(corpus, &source.file)).or_default();
        entry.0.extend(source.pages.iter().copied());
        if entry.1.is_none() {
            entry.1 = source.passage.clone();
        }
    }
    map
}

fn distinct_keys(chunks: &[RetrievedChunk]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for chunk in chunks {
        let key = chunk
            .key
            .clone()
            .unwrap_or_else(|| "(outside the corpus)".to_string());
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// One run's searches: the case records and the per-case diagnostics.
struct Searched {
    records: Vec<CaseRecord>,
    /// `recall@20` per case.
    wide_recall: Vec<f64>,
    diagnostics: Vec<BTreeMap<String, f64>>,
}

/// Per-case diagnostics: distinct files among the first 10 and 20 results
/// (all cases, and the cases expecting several documents), and the files
/// each stage of an undecomposed search kept.
fn case_diagnostics(
    narrow: &[RetrievedChunk],
    wide: &[RetrievedChunk],
    expected_docs: usize,
    trace: &shodh_rag::rag_engine::SearchTrace,
) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    let at10 = unique_sources(narrow, 10) as f64;
    let at20 = unique_sources(wide, WIDE_K) as f64;
    out.insert("unique_sources@10".to_string(), at10);
    out.insert(format!("unique_sources@{WIDE_K}"), at20);
    if expected_docs > 1 {
        out.insert("multi_unique_sources@10".to_string(), at10);
        out.insert(format!("multi_unique_sources@{WIDE_K}"), at20);
    }
    let decomposed = trace.sub_queries > 1;
    out.insert("decomposed".to_string(), if decomposed { 1.0 } else { 0.0 });
    if !decomposed {
        for stage in &trace.stages {
            out.insert(
                format!("stage_{}_results", stage.stage),
                stage.results as f64,
            );
            out.insert(
                format!("stage_{}_sources", stage.stage),
                stage.unique_sources as f64,
            );
        }
    }
    out
}

/// Mean of each diagnostic over the cases that have it.
fn mean_diagnostics(cases: &[BTreeMap<String, f64>]) -> BTreeMap<String, f64> {
    let mut sums: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    for case in cases {
        for (name, value) in case {
            let entry = sums.entry(name.clone()).or_default();
            entry.0 += value;
            entry.1 += 1;
        }
    }
    sums.into_iter()
        .map(|(name, (sum, n))| (name, sum / n as f64))
        .collect()
}

async fn search_all(
    index: &IndexedCorpus,
    cases: &[&EvalCase],
    k: usize,
    latencies: &mut Vec<f64>,
) -> Result<Searched> {
    // The first search loads the embedder and the reranker; keep that out
    // of the latency figures.
    index
        .rag
        .read()
        .await
        .search_comprehensive("warm up the search models", k, None)
        .await
        .context("warm-up search")?;
    let mut searched = Searched {
        records: Vec::with_capacity(cases.len()),
        wide_recall: Vec::with_capacity(cases.len()),
        diagnostics: Vec::with_capacity(cases.len()),
    };
    // Indexed source of each document key, to tell a passage the search did
    // not rank from one the index does not hold (lost in parsing or chunking).
    let sources: BTreeMap<String, String> = index
        .rag
        .read()
        .await
        .document_sources()
        .await
        .context("listing the indexed sources")?
        .into_iter()
        .filter_map(|row| relative_key(&index.root_key, &row.source).map(|key| (key, row.source)))
        .collect();
    for case in cases {
        let passage_indexed = passage_indexed(index, &sources, case).await?;
        let engine = index.rag.read().await;
        let started = Instant::now();
        let (results, trace) = engine
            .search_traced(&case.question, k, None)
            .await
            .with_context(|| format!("searching case '{}'", case.id))?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let wide_results = engine
            .search_comprehensive(&case.question, WIDE_K, None)
            .await
            .with_context(|| format!("searching case '{}' for {WIDE_K}", case.id))?;
        drop(engine);
        latencies.push(ms);
        let chunks: Vec<RetrievedChunk> = results.iter().map(|r| index.chunk(r)).collect();
        let wide: Vec<RetrievedChunk> = wide_results.iter().map(|r| index.chunk(r)).collect();
        let expected = expected_map(&index.root, case);
        let ranks = rank_case(&expected, &chunks);
        searched
            .wide_recall
            .push(rank_case(&expected, &wide).recall(WIDE_K));
        let mut diagnostics = case_diagnostics(&chunks, &wide, expected.len(), &trace);
        if let Some(held) = passage_indexed {
            diagnostics.insert(
                "passage_not_in_index".to_string(),
                if held { 0.0 } else { 1.0 },
            );
        }
        let spread: BTreeMap<String, usize> = [
            ("10".to_string(), unique_sources(&chunks, 10)),
            (WIDE_K.to_string(), unique_sources(&wide, WIDE_K)),
        ]
        .into_iter()
        .collect();
        searched.diagnostics.push(diagnostics);
        searched.records.push(CaseRecord {
            id: case.id.clone(),
            question: case.question.clone(),
            ranks: Some(ranks),
            retrieved: distinct_keys(&chunks),
            top: chunks.first().map(TopResult::of),
            latency_ms: Some(ms),
            unique_sources: spread,
            trace: Some(trace),
            passage_indexed,
            ..CaseRecord::default()
        });
    }
    Ok(searched)
}

/// Whether every expected passage of `case` is in a stored chunk of its
/// document (`None` when the case gives no passage).
async fn passage_indexed(
    index: &IndexedCorpus,
    sources: &BTreeMap<String, String>,
    case: &EvalCase,
) -> Result<Option<bool>> {
    let mut any = false;
    for (key, (_, passage)) in expected_map(&index.root, case) {
        let Some(passage) = passage else {
            continue;
        };
        any = true;
        let Some(source) = sources.get(&key) else {
            return Ok(Some(false));
        };
        let chunks = index
            .rag
            .read()
            .await
            .chunks_of_source(source)
            .await
            .with_context(|| format!("listing the chunks of {key}"))?;
        if !chunks.iter().any(|c| contains_span(&c.text, &passage)) {
            return Ok(Some(false));
        }
    }
    Ok(any.then_some(true))
}

/// The OCR choice as report settings.
pub fn ocr_settings(ocr: &shodh_rag::config::OcrConfig) -> BTreeMap<String, String> {
    use shodh_rag::config::OcrEngineChoice;
    let engine = match ocr.engine {
        OcrEngineChoice::Auto => "auto",
        OcrEngineChoice::System => "system",
        OcrEngineChoice::PpOcr => "ppocr",
    };
    let mut settings: BTreeMap<String, String> = [
        ("ocr".to_string(), engine.to_string()),
        (
            "system_ocr_render_dpi".to_string(),
            ocr.system_render_dpi
                .map_or_else(|| "96 (page size)".to_string(), |d| d.to_string()),
        ),
    ]
    .into_iter()
    .collect();
    // The PP-OCRv6 settings only where it can read (`auto` uses it where the
    // system OCR is missing).
    if ocr.engine != OcrEngineChoice::System {
        settings.insert(
            "ocr_model".to_string(),
            shodh_rag::processing::ocr::OCR_MODEL_ID.to_string(),
        );
        settings.insert("ocr_render_dpi".to_string(), ocr.render_dpi.to_string());
    }
    settings
}

pub async fn run(opts: &RetrievalOptions) -> Result<RunReport> {
    if opts.runs == 0 || opts.k == 0 {
        bail!("runs and k must be at least 1");
    }
    let corpus_hash = corpus_hash(&opts.corpus)?;
    let cases: Vec<&EvalCase> = opts
        .dataset
        .active_cases()
        .filter(|c| c.answerable)
        .collect();
    if cases.is_empty() {
        bail!("the dataset has no answerable cases to search for");
    }
    let mut per_run = Vec::with_capacity(opts.runs);
    let mut diagnostics_per_run = Vec::with_capacity(opts.runs);
    let mut ingests = Vec::with_capacity(opts.runs);
    let mut latencies = Vec::new();
    let mut last = Vec::new();
    for run in 1..=opts.runs {
        eprintln!(
            "run {run}/{}: indexing {}",
            opts.runs,
            opts.corpus.display()
        );
        let index = index_corpus_with(&opts.corpus, &opts.models, &opts.ocr, &opts.tweaks).await?;
        eprintln!(
            "run {run}/{}: {}/{} files, {} chunks in {:.1} s; searching {} questions",
            opts.runs,
            index.ingest.files_indexed,
            index.ingest.files_seen,
            index.ingest.chunks,
            index.ingest.seconds,
            cases.len()
        );
        let searched = search_all(&index, &cases, opts.k, &mut latencies).await?;
        let ranks: Vec<_> = searched
            .records
            .iter()
            .filter_map(|r| r.ranks.clone())
            .collect();
        let mut metrics = retrieval_metrics(&ranks);
        metrics.insert(
            "ingest_ok".to_string(),
            index.ingest.files_indexed as f64 / index.ingest.files_seen as f64,
        );
        metrics.insert(
            format!("recall@{WIDE_K}"),
            searched.wide_recall.iter().sum::<f64>() / searched.wide_recall.len() as f64,
        );
        per_run.push(metrics);
        let mut diagnostics = mean_diagnostics(&searched.diagnostics);
        diagnostics.insert("chunks".to_string(), index.ingest.chunks as f64);
        diagnostics_per_run.push(diagnostics);
        ingests.push(index.ingest.clone());
        last = searched.records;
    }
    let mut settings = settings_with(&opts.tweaks);
    settings.insert("k".to_string(), opts.k.to_string());
    settings.insert("runs".to_string(), opts.runs.to_string());
    settings.extend(ocr_settings(&opts.ocr));
    Ok(RunReport {
        kind: RunKind::Retrieval,
        dataset_id: opts.dataset.id.clone(),
        dataset_hash: opts.dataset.content_hash(),
        corpus_hash,
        created_at: chrono::Utc::now().to_rfc3339(),
        settings,
        metrics: combine_runs(&per_run),
        diagnostics: combine_runs(&diagnostics_per_run),
        latency: summarize_latencies(&latencies),
        first_token: None,
        provider_errors: BTreeMap::new(),
        stopped: None,
        ingest: ingests,
        cases: last,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::rag_engine::{SearchStage, SearchTrace};

    fn chunk(key: &str) -> RetrievedChunk {
        RetrievedChunk {
            key: Some(key.into()),
            pages: None,
            text: String::new(),
        }
    }

    #[test]
    fn diagnostics_count_files_and_skip_stages_of_decomposed_searches() {
        let narrow = [chunk("a"), chunk("a"), chunk("b")];
        let wide = [chunk("a"), chunk("b"), chunk("c"), chunk("a")];
        let stage = |stage: &str, results, unique_sources| SearchStage {
            sub_query: 0,
            stage: stage.to_string(),
            results,
            unique_sources,
        };
        let trace = SearchTrace {
            sub_queries: 1,
            stages: vec![stage("vector", 50, 2), stage("returned", 10, 2)],
        };
        let single = case_diagnostics(&narrow, &wide, 1, &trace);
        assert_eq!(single["unique_sources@10"], 2.0);
        assert_eq!(single["unique_sources@20"], 3.0);
        assert_eq!(single["stage_vector_sources"], 2.0);
        assert_eq!(single["stage_vector_results"], 50.0);
        assert_eq!(single["decomposed"], 0.0);
        assert!(!single.contains_key("multi_unique_sources@10"));
        let decomposed = SearchTrace {
            sub_queries: 2,
            ..trace
        };
        let multi = case_diagnostics(&narrow, &wide, 3, &decomposed);
        assert_eq!(multi["multi_unique_sources@20"], 3.0);
        assert_eq!(multi["decomposed"], 1.0);
        assert!(!multi.contains_key("stage_vector_sources"));
        let mean = mean_diagnostics(&[single, multi]);
        assert_eq!(mean["unique_sources@20"], 3.0);
        assert_eq!(mean["decomposed"], 0.5);
        assert_eq!(mean["stage_vector_sources"], 2.0);
        assert_eq!(mean["multi_unique_sources@10"], 2.0);
    }
}
