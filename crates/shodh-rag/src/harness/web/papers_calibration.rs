//! Relevance cut calibration on recorded API answers (offline).
//!
//! `fixtures/papers/<name>/` holds real arXiv, OpenAlex and Semantic Scholar
//! answers recorded once for the requests [`paper_requests`] builds, plus,
//! for the "Diffusing Blame" title, the answer of the old `all:` arXiv
//! query that let unrelated papers through in production.
//! `cross-encoder-logits.json` holds the ms-marco MiniLM cross-encoder's
//! logits for every candidate, so the threshold tests replay the real
//! model's scores without the model; `recorded_logits_match_the_model`
//! checks them against the live model when `SHODH_TEST_MODELS` is set, and
//! `record_cross_encoder_logits` (ignored) rewrites them after a fixture or
//! the candidate text changes.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use super::*;
use crate::harness::web::client::testing::*;
use crate::harness::web::relevance::PassageScorer;

struct Fixture {
    name: &'static str,
    query: &'static str,
    arxiv: &'static str,
    openalex_title: Option<&'static str>,
    openalex: &'static str,
    semantic_scholar: &'static str,
    /// Earlier, unfocused arXiv answer (kept to prove its junk is cut).
    legacy_arxiv: Option<&'static str>,
}

const DIFFUSING_BLAME_TITLE: &str = "Diffusing Blame: Task-Dependent Credit Assignment in \
                                     Biologically Plausible Dual-Stream Networks";

const DIFFUSING_BLAME: Fixture = Fixture {
    name: "diffusing-blame",
    query: DIFFUSING_BLAME_TITLE,
    arxiv: include_str!("../fixtures/papers/diffusing-blame/arxiv.xml"),
    openalex_title: Some(include_str!(
        "../fixtures/papers/diffusing-blame/openalex-title.json"
    )),
    openalex: include_str!("../fixtures/papers/diffusing-blame/openalex.json"),
    semantic_scholar: include_str!("../fixtures/papers/diffusing-blame/semantic-scholar.json"),
    legacy_arxiv: Some(include_str!(
        "../fixtures/papers/diffusing-blame/arxiv-all-legacy.xml"
    )),
};

const LINEAR_ATTENTION: Fixture = Fixture {
    name: "linear-attention",
    query: "linear attention delta rule",
    arxiv: include_str!("../fixtures/papers/linear-attention/arxiv.xml"),
    openalex_title: None,
    openalex: include_str!("../fixtures/papers/linear-attention/openalex.json"),
    semantic_scholar: include_str!("../fixtures/papers/linear-attention/semantic-scholar.json"),
    legacy_arxiv: None,
};

const CREDIT_ASSIGNMENT: Fixture = Fixture {
    name: "credit-assignment",
    query: "credit assignment",
    arxiv: include_str!("../fixtures/papers/credit-assignment/arxiv.xml"),
    openalex_title: None,
    openalex: include_str!("../fixtures/papers/credit-assignment/openalex.json"),
    semantic_scholar: include_str!("../fixtures/papers/credit-assignment/semantic-scholar.json"),
    legacy_arxiv: None,
};

const FIXTURES: [&Fixture; 3] = [&DIFFUSING_BLAME, &LINEAR_ATTENTION, &CREDIT_ASSIGNMENT];

const LOGITS: &str = include_str!("../fixtures/papers/cross-encoder-logits.json");
const LOGITS_PATH: &str = "src/harness/fixtures/papers/cross-encoder-logits.json";

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// The candidate pool for a fixture, merged as `search_papers` merges it
/// (arXiv, OpenAlex title filter, OpenAlex, Semantic Scholar), plus the
/// legacy arXiv answer when there is one.
fn pool(f: &Fixture) -> Vec<MergedPaper> {
    let mut lists = vec![parse_arxiv(f.arxiv).unwrap()];
    if let Some(t) = f.openalex_title {
        lists.push(parse_openalex(&json(t)).unwrap());
    }
    lists.push(parse_openalex(&json(f.openalex)).unwrap());
    lists.push(parse_semantic_scholar(&json(f.semantic_scholar)).unwrap());
    if let Some(legacy) = f.legacy_arxiv {
        lists.push(parse_arxiv(legacy).unwrap());
    }
    merge(lists)
}

/// Replays recorded cross-encoder logits; an unrecorded passage is an
/// error, which `rank` turns into the lexical fallback (asserted against).
struct Replay(HashMap<String, f32>);

impl PassageScorer for Replay {
    fn score(&self, _query: &str, passages: &[String]) -> Result<Vec<f32>, String> {
        passages
            .iter()
            .map(|p| {
                self.0.get(p).copied().ok_or_else(|| {
                    format!(
                        "no recorded logit for {:?}",
                        p.chars().take(80).collect::<String>()
                    )
                })
            })
            .collect()
    }
}

fn replay(f: &Fixture) -> SharedScorer {
    let all: HashMap<String, HashMap<String, f32>> = serde_json::from_str(LOGITS).unwrap();
    Arc::new(Replay(all.get(f.name).cloned().unwrap_or_default()))
}

fn titles(papers: &[Paper]) -> Vec<String> {
    papers.iter().map(|p| p.title.to_lowercase()).collect()
}

fn pool_has(f: &Fixture, needle: &str) -> bool {
    pool(f)
        .iter()
        .any(|m| m.paper.title.to_lowercase().contains(needle))
}

fn kept_has(papers: &[Paper], needle: &str) -> bool {
    titles(papers).iter().any(|t| t.contains(needle))
}

/// Rank a fixture's pool with the replayed cross-encoder.
fn ranked(f: &Fixture) -> (Vec<Paper>, usize) {
    let (papers, dropped, method) = rank_papers(f.query, pool(f), Some(&replay(f)), 20);
    assert_eq!(
        method,
        RankMethod::CrossEncoder,
        "{}: recorded logits are stale; run record_cross_encoder_logits",
        f.name
    );
    (papers, dropped)
}

fn ranked_lexically(f: &Fixture) -> (Vec<Paper>, usize) {
    let (papers, dropped, method) = rank_papers(f.query, pool(f), None, 20);
    assert_eq!(method, RankMethod::Lexical);
    (papers, dropped)
}

/// Junk the APIs returned for the Diffusing Blame title (OpenAlex search and
/// the old `all:` arXiv query).
const DIFFUSING_BLAME_JUNK: &[&str] = &[
    "transboundary networked forest governance",
    "so what if chatgpt wrote it",
    "environmental realpolitik",
    "open learning agency",
    "planning for the management of technological risk",
    "patterns of multiplex layer entanglement",
];

/// Work on the same topic (biologically plausible credit assignment).
const DIFFUSING_BLAME_RELATED: &[&str] = &[
    "counter-current learning",
    "kickback cuts backprop",
    "biological credit assignment through dynamic inversion",
    "diffusion of neuromodulators",
    "top-down credit assignment network",
];

#[test]
fn diffusing_blame_keeps_the_named_paper_first_and_drops_the_junk() {
    for junk in DIFFUSING_BLAME_JUNK {
        assert!(pool_has(&DIFFUSING_BLAME, junk), "fixture lacks {junk}");
    }
    let (papers, dropped) = ranked(&DIFFUSING_BLAME);
    assert!(papers[0].title.starts_with("Diffusing Blame"));
    assert!(papers[0].title_match);
    assert_eq!(
        papers[0].sources,
        vec![ARXIV, OPENALEX, SEMANTIC_SCHOLAR],
        "one record across every API"
    );
    for junk in DIFFUSING_BLAME_JUNK {
        assert!(!kept_has(&papers, junk), "kept junk {junk}");
    }
    for related in DIFFUSING_BLAME_RELATED {
        assert!(kept_has(&papers, related), "dropped {related}");
    }
    assert_eq!(papers.len(), 1 + DIFFUSING_BLAME_RELATED.len());
    assert_eq!(dropped, 17);
}

#[test]
fn diffusing_blame_lexical_fallback_keeps_only_the_named_paper() {
    let (papers, dropped) = ranked_lexically(&DIFFUSING_BLAME);
    assert_eq!(titles(&papers).len(), 1, "{:?}", titles(&papers));
    assert!(papers[0].title.starts_with("Diffusing Blame"));
    assert_eq!(dropped, 22);
}

/// Delta-rule papers from other fields.
const WRONG_FIELD_DELTA_RULE: &[&str] = &[
    "mixture of delta-rules approximation to bayesian inference",
    "approximately bayesian delta-rule model",
    "dram: delta-rule recurrent associative memory",
];

#[test]
fn linear_attention_keeps_linear_attention_and_drops_other_fields() {
    for other in WRONG_FIELD_DELTA_RULE {
        assert!(pool_has(&LINEAR_ATTENTION, other), "fixture lacks {other}");
    }
    let (papers, dropped) = ranked(&LINEAR_ATTENTION);
    for relevant in [
        "parallelizing linear transformers with the delta rule",
        "gated delta networks",
        "kernelized linear attention",
        "enhancing linear attention with residual learning",
    ] {
        assert!(kept_has(&papers, relevant), "dropped {relevant}");
    }
    for other in WRONG_FIELD_DELTA_RULE {
        assert!(!kept_has(&papers, other), "kept {other}");
    }
    assert_eq!(dropped, WRONG_FIELD_DELTA_RULE.len());

    let (lexical, _) = ranked_lexically(&LINEAR_ATTENTION);
    assert!(kept_has(
        &lexical,
        "parallelizing linear transformers with the delta rule"
    ));
}

#[test]
fn a_vague_query_is_not_over_pruned() {
    let (papers, dropped) = ranked(&CREDIT_ASSIGNMENT);
    assert_eq!(dropped, 0);
    assert_eq!(papers.len(), 20, "cut to the limit, not by relevance");
    let (_, dropped) = ranked_lexically(&CREDIT_ASSIGNMENT);
    assert_eq!(dropped, 0);
}

#[test]
fn every_kept_paper_clears_the_threshold_or_is_a_title_match() {
    for f in FIXTURES {
        let (papers, _) = ranked(f);
        for p in &papers {
            let r = p.relevance.unwrap();
            assert!(
                p.title_match || PAPER_THRESHOLDS.cross_encoder.is_some_and(|t| r >= t),
                "{}: {} at {r}",
                f.name,
                p.title
            );
        }
        let (papers, _) = ranked_lexically(f);
        for p in &papers {
            let r = p.relevance.unwrap();
            assert!(
                p.title_match || PAPER_THRESHOLDS.lexical.is_some_and(|t| r >= t),
                "{}",
                p.title
            );
        }
    }
}

/// The real search path, offline: requests are answered from the fixtures
/// by URL, so a change in the query builder that the fixtures no longer
/// match fails here.
#[tokio::test]
async fn search_papers_runs_offline_on_the_recorded_answers() {
    let f = &DIFFUSING_BLAME;
    let requests = paper_requests(f.query, 12).unwrap();
    let mut transport = FakeTransport::default()
        .route(
            requests.arxiv.as_ref().unwrap().as_str(),
            Canned::ok("application/atom+xml", f.arxiv),
        )
        .route(
            requests.openalex.as_str(),
            Canned::ok("application/json", f.openalex),
        )
        .route(
            requests.semantic_scholar.as_str(),
            Canned::ok("application/json", f.semantic_scholar),
        );
    if let (Some(url), Some(body)) = (&requests.openalex_title, f.openalex_title) {
        transport = transport.route(url.as_str(), Canned::ok("application/json", body));
    }
    let (client, sent) = client(
        FakeResolver::default()
            .with("export.arxiv.org", &["128.84.21.199"])
            .with("api.openalex.org", &["104.20.10.20"])
            .with("api.semanticscholar.org", &["13.32.150.10"]),
        transport,
    );
    // limit 8 asks each API for 12, as recorded.
    let found = search_papers(&client, f.query, 8, Some(replay(f)))
        .await
        .unwrap();
    assert!(found.failures.is_empty(), "{:?}", found.failures);
    assert_eq!(sent.sent.lock().unwrap().len(), 4);
    assert_eq!(found.method, RankMethod::CrossEncoder);
    assert!(found.papers[0].title.starts_with("Diffusing Blame"));
    assert!(found.dropped > 0);
}

fn live_logits() -> BTreeMap<String, BTreeMap<String, f32>> {
    let dir = std::path::PathBuf::from(
        std::env::var("SHODH_TEST_MODELS").expect("SHODH_TEST_MODELS must name the models dir"),
    )
    .join("ms-marco-MiniLM-L6-v2");
    let model = crate::reranking::CrossEncoderReranker::new(&dir).expect("load reranker");
    let mut out = BTreeMap::new();
    for f in FIXTURES {
        let texts: Vec<String> = pool(f).iter().map(|m| m.paper.relevance_text()).collect();
        let logits = PassageScorer::score(&model, f.query, &texts).unwrap();
        out.insert(f.name.to_string(), texts.into_iter().zip(logits).collect());
    }
    out
}

#[test]
#[cfg_attr(
    not(shodh_test_models),
    ignore = "requires SHODH_TEST_MODELS containing ms-marco-MiniLM-L6-v2/"
)]
fn recorded_logits_match_the_model() {
    let recorded: BTreeMap<String, BTreeMap<String, f32>> = serde_json::from_str(LOGITS).unwrap();
    let live = live_logits();
    assert_eq!(
        recorded.keys().collect::<Vec<_>>(),
        live.keys().collect::<Vec<_>>()
    );
    for (name, scores) in &live {
        for (text, logit) in scores {
            let r = recorded[name].get(text).copied();
            assert!(
                r.is_some_and(|r| (r - logit).abs() < 1e-3),
                "{name}: {r:?} vs live {logit} for {text:.80}"
            );
        }
    }
}

/// Rewrites `cross-encoder-logits.json` and prints each candidate's score
/// (the calibration table). Run with
/// `SHODH_TEST_MODELS=<models dir> cargo test ... record_cross_encoder_logits -- --ignored --nocapture`.
#[test]
#[ignore = "writes the logits fixture; needs the reranker model"]
fn record_cross_encoder_logits() {
    let live = live_logits();
    for f in FIXTURES {
        println!("== {} ({})   logit  sigmoid  lexical", f.name, f.query);
        let texts: Vec<String> = pool(f).iter().map(|m| m.paper.relevance_text()).collect();
        let lexical = crate::harness::web::relevance::lexical_scores(f.query, &texts);
        let mut rows: Vec<(f32, f32, String)> = texts
            .iter()
            .zip(lexical)
            .map(|(t, lex)| (live[f.name][t], lex, t.chars().take(80).collect()))
            .collect();
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (l, lex, t) in rows {
            let sig = 1.0 / (1.0 + (-l).exp());
            println!("{l:8.3} {sig:6.3} {lex:6.3}  {t}");
        }
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(LOGITS_PATH);
    let body = serde_json::to_string_pretty(&live).unwrap();
    std::fs::write(&path, body + "\n").unwrap();
}
