//! Retrieval evaluation: index the corpus into a fresh index, search every
//! answerable question the way the agent's `search_documents` tool does
//! (`search_comprehensive`, no filter), and rank the expected evidence.
//! No LLM is involved, so this runs in CI.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};

use crate::corpus::{corpus_hash, expected_key};
use crate::dataset::{Dataset, EvalCase};
use crate::engine::{index_corpus, settings, IndexedCorpus};
use crate::metrics::{rank_case, retrieval_metrics, RetrievedChunk};
use crate::report::{combine_runs, summarize_latencies, CaseRecord, RunKind, RunReport};

/// Results fetched per question (the top of the agent tool's range).
pub const DEFAULT_K: usize = 10;

pub struct RetrievalOptions {
    /// Absolute corpus folder.
    pub corpus: PathBuf,
    pub dataset: Dataset,
    pub models: PathBuf,
    /// Repetitions, each on a freshly built index.
    pub runs: usize,
    pub k: usize,
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

async fn search_all(
    index: &IndexedCorpus,
    cases: &[&EvalCase],
    k: usize,
    latencies: &mut Vec<f64>,
) -> Result<Vec<CaseRecord>> {
    // The first search loads the embedder and the reranker; keep that out
    // of the latency figures.
    index
        .rag
        .read()
        .await
        .search_comprehensive("warm up the search models", k, None)
        .await
        .context("warm-up search")?;
    let mut records = Vec::with_capacity(cases.len());
    for case in cases {
        let engine = index.rag.read().await;
        let started = Instant::now();
        let results = engine
            .search_comprehensive(&case.question, k, None)
            .await
            .with_context(|| format!("searching case '{}'", case.id))?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        drop(engine);
        latencies.push(ms);
        let chunks: Vec<RetrievedChunk> = results.iter().map(|r| index.chunk(r)).collect();
        let ranks = rank_case(&expected_map(&index.root, case), &chunks);
        records.push(CaseRecord {
            id: case.id.clone(),
            question: case.question.clone(),
            ranks: Some(ranks),
            retrieved: distinct_keys(&chunks),
            latency_ms: Some(ms),
            ..CaseRecord::default()
        });
    }
    Ok(records)
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
    let mut ingests = Vec::with_capacity(opts.runs);
    let mut latencies = Vec::new();
    let mut last = Vec::new();
    for run in 1..=opts.runs {
        eprintln!(
            "run {run}/{}: indexing {}",
            opts.runs,
            opts.corpus.display()
        );
        let index = index_corpus(&opts.corpus, &opts.models).await?;
        eprintln!(
            "run {run}/{}: {}/{} files, {} chunks in {:.1} s; searching {} questions",
            opts.runs,
            index.ingest.files_indexed,
            index.ingest.files_seen,
            index.ingest.chunks,
            index.ingest.seconds,
            cases.len()
        );
        let records = search_all(&index, &cases, opts.k, &mut latencies).await?;
        let ranks: Vec<_> = records.iter().filter_map(|r| r.ranks.clone()).collect();
        let mut metrics = retrieval_metrics(&ranks);
        metrics.insert(
            "ingest_ok".to_string(),
            index.ingest.files_indexed as f64 / index.ingest.files_seen as f64,
        );
        per_run.push(metrics);
        ingests.push(index.ingest.clone());
        last = records;
    }
    let mut settings = settings();
    settings.insert("k".to_string(), opts.k.to_string());
    settings.insert("runs".to_string(), opts.runs.to_string());
    Ok(RunReport {
        kind: RunKind::Retrieval,
        dataset_id: opts.dataset.id.clone(),
        dataset_hash: opts.dataset.content_hash(),
        corpus_hash,
        created_at: chrono::Utc::now().to_rfc3339(),
        settings,
        metrics: combine_runs(&per_run),
        latency: summarize_latencies(&latencies),
        ingest: ingests,
        cases: last,
    })
}
