//! Threshold calibration on labeled claims (offline).
//!
//! `fixtures/grounding/claims.json` holds passages of the user's indexed
//! arXiv papers (verbatim, cut to the 1 500 characters `search_documents`
//! gives the model) and 88 hand-labeled claims about them: 47 the passage
//! states, and 41 it does not, by category (wrong number, swapped roles,
//! reversed statement, on topic but not stated, about another document),
//! plus 20 information needs paired with a passage that covers them or not.
//!
//! `fixtures/grounding/model-scores.json` records what the pinned models
//! (ms-marco MiniLM cross-encoder, nli-deberta-v3-xsmall) return for every
//! pair the verifier scores here, so these tests replay real model output
//! without the models. `recorded_scores_match_the_models` checks the
//! recording against the live models when `SHODH_TEST_MODELS` is set;
//! `record_model_scores` (ignored) rewrites it.
//!
//! Each test sweeps its cutoff and asserts that the shipped threshold is one
//! of the cutoffs with the best accuracy: the thresholds are calibrated, not
//! picked. Run with `--nocapture` to see the tables.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::numbers::missing_numbers;
use super::verify::{
    clean_evidence, need_coverage, outcome_for, support, EntailmentScorer, Scorers, Thresholds,
    THRESHOLDS,
};
use crate::harness::events::{ClaimOutcome, ScoringMethod};
use crate::harness::web::relevance::PassageScorer;
use crate::reranking::Entailment;

const FIXTURE: &str = include_str!("../fixtures/grounding/claims.json");
const SCORES: &str = include_str!("../fixtures/grounding/model-scores.json");
const SCORES_PATH: &str = "src/harness/fixtures/grounding/model-scores.json";
/// Separates query and passage in recorded keys.
const SEP: char = '\u{1f}';

#[derive(Debug, Deserialize)]
struct PassageEntry {
    text: String,
}

#[derive(Debug, Deserialize)]
struct LabeledClaim {
    passage: String,
    label: String,
    category: String,
    claim: String,
}

#[derive(Debug, Deserialize)]
struct LabeledNeed {
    need: String,
    passage: String,
    covered: bool,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    passages: BTreeMap<String, PassageEntry>,
    claims: Vec<LabeledClaim>,
    needs: Vec<LabeledNeed>,
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).expect("claims.json parses")
}

/// Recorded model output: cross-encoder logits and NLI probabilities
/// (contradiction, entailment, neutral), keyed by `query SEP passage`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Recorded {
    relevance: BTreeMap<String, f32>,
    entailment: BTreeMap<String, [f32; 3]>,
}

fn key(query: &str, passage: &str) -> String {
    format!("{query}{SEP}{passage}")
}

/// Replays recorded scores; an unrecorded pair is an error (the tests
/// assert the expected scoring method, so a fallback cannot pass silently).
struct Replay(Recorded);

impl PassageScorer for Replay {
    fn score(&self, query: &str, passages: &[String]) -> Result<Vec<f32>, String> {
        passages
            .iter()
            .map(|p| {
                self.0
                    .relevance
                    .get(&key(query, p))
                    .copied()
                    .ok_or_else(|| format!("no recorded logit for {query:?}"))
            })
            .collect()
    }
}

impl EntailmentScorer for Replay {
    fn entail(&self, hypothesis: &str, premises: &[String]) -> Result<Vec<Entailment>, String> {
        premises
            .iter()
            .map(|p| {
                self.0
                    .entailment
                    .get(&key(hypothesis, p))
                    .map(|[c, e, n]| Entailment {
                        contradiction: *c,
                        entailment: *e,
                        neutral: *n,
                    })
                    .ok_or_else(|| format!("no recorded entailment for {hypothesis:?}"))
            })
            .collect()
    }
}

fn replay() -> Replay {
    Replay(serde_json::from_str(SCORES).expect("model-scores.json parses"))
}

/// One labeled claim with what the verifier measured.
#[derive(Debug)]
struct Row {
    category: String,
    supported: bool,
    numbers_missing: bool,
    score: f32,
}

fn rows(scorers: Scorers<'_>, expected: ScoringMethod) -> Vec<Row> {
    let f = fixture();
    f.claims
        .iter()
        .map(|c| {
            let text = &f.passages[&c.passage].text;
            let found = support(scorers, &c.claim, std::slice::from_ref(text));
            assert_eq!(
                found.method, expected,
                "{:?} was not scored as expected",
                c.claim
            );
            Row {
                category: c.category.clone(),
                supported: c.label == "supported",
                numbers_missing: !missing_numbers(&c.claim, &[text]).is_empty(),
                score: found.score,
            }
        })
        .collect()
}

/// Whether the claim is flagged (`unsupported`) under these cutoffs.
fn flagged(row: &Row, method: ScoringMethod, t: &Thresholds) -> bool {
    row.numbers_missing || outcome_for(row.score, method, t) == ClaimOutcome::Unsupported
}

/// Whether the claim counts as supported (not weak, not flagged).
fn accepted(row: &Row, method: ScoringMethod, t: &Thresholds) -> bool {
    !row.numbers_missing && outcome_for(row.score, method, t) == ClaimOutcome::Supported
}

fn accuracy(rows: &[Row], predict: impl Fn(&Row) -> bool, positive: impl Fn(&Row) -> bool) -> f32 {
    let correct = rows.iter().filter(|r| predict(r) == positive(r)).count();
    correct as f32 / rows.len() as f32
}

const CUTS: [f32; 19] = [
    0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85,
    0.9, 0.95,
];

/// Accuracy of each cut up to `max_cut`; asserts `chosen` is one of the best.
fn assert_best_cut(name: &str, chosen: f32, max_cut: f32, accuracy_at: impl Fn(f32) -> f32) -> f32 {
    let table: Vec<(f32, f32)> = CUTS
        .iter()
        .filter(|c| **c <= max_cut + 1e-6)
        .map(|&c| (c, accuracy_at(c)))
        .collect();
    let best = table.iter().map(|(_, a)| *a).fold(0.0_f32, f32::max);
    println!("{name}:");
    for (c, a) in &table {
        let mark = if (c - chosen).abs() < 1e-6 {
            "  <- shipped"
        } else {
            ""
        };
        println!("  cut {c:.2}: accuracy {a:.3}{mark}");
    }
    let shipped = accuracy_at(chosen);
    assert!(
        (shipped - best).abs() < 1e-6,
        "{name}: the shipped cut {chosen} scores {shipped}, the best cut scores {best}"
    );
    shipped
}

fn per_category(rows: &[Row], wrong: impl Fn(&Row) -> bool) -> BTreeMap<String, (usize, usize)> {
    let mut out: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for r in rows {
        let entry = out.entry(r.category.clone()).or_default();
        entry.1 += 1;
        if !wrong(r) {
            entry.0 += 1;
        }
    }
    out
}

fn calibrate(
    scorers: Scorers<'_>,
    method: ScoringMethod,
    support_cut: impl Fn(&mut Thresholds, f32),
    weak_cut: impl Fn(&mut Thresholds, f32),
    shipped: (f32, f32),
) -> (f32, f32, BTreeMap<String, (usize, usize)>) {
    let rows = rows(scorers, method);
    let support_accuracy =
        assert_best_cut(&format!("{method:?} support cut"), shipped.0, 1.0, |c| {
            let mut t = THRESHOLDS;
            support_cut(&mut t, c);
            accuracy(&rows, |r| accepted(r, method, &t), |r| r.supported)
        });
    // The weak band lies below the support cut.
    let flag_accuracy = assert_best_cut(
        &format!("{method:?} weak cut (flagging)"),
        shipped.1,
        shipped.0,
        |c| {
            let mut t = THRESHOLDS;
            weak_cut(&mut t, c);
            accuracy(&rows, |r| flagged(r, method, &t), |r| !r.supported)
        },
    );
    let categories = per_category(&rows, |r| flagged(r, method, &THRESHOLDS) == r.supported);
    println!("{method:?} flagging correct per category: {categories:?}");
    (support_accuracy, flag_accuracy, categories)
}

#[test]
fn entailment_cuts_are_calibrated() {
    let replay = replay();
    let scorers = Scorers {
        relevance: Some(&replay),
        entailment: Some(&replay),
    };
    let (support, flag, categories) = calibrate(
        scorers,
        ScoringMethod::Entailment,
        |t, c| t.entail_support = c,
        |t, c| t.entail_weak = c,
        (THRESHOLDS.entail_support, THRESHOLDS.entail_weak),
    );
    // The entailment model sees reversed and swapped statements, which the
    // relevance model cannot (see the next test).
    assert_eq!(categories["negation"], (10, 10));
    assert!(categories["swap"].0 >= 6, "{categories:?}");
    assert!(
        support >= 0.9 && flag >= 0.9,
        "support {support}, flag {flag}"
    );
}

#[test]
fn cross_encoder_fallback_cuts_are_calibrated() {
    let replay = replay();
    let scorers = Scorers {
        relevance: Some(&replay),
        entailment: None,
    };
    let (support, flag, categories) = calibrate(
        scorers,
        ScoringMethod::CrossEncoder,
        |t, c| t.support = c,
        |t, c| t.weak = c,
        (THRESHOLDS.support, THRESHOLDS.weak),
    );
    // Relevance is not support: a reversed statement is as relevant as the
    // original. Numbers and topic are still caught.
    assert!(categories["negation"].0 < 5, "{categories:?}");
    assert_eq!(categories["unrelated"], (2, 2));
    assert!(
        support >= 0.7 && flag >= 0.7,
        "support {support}, flag {flag}"
    );
}

#[test]
fn lexical_fallback_cuts_are_calibrated() {
    let (support, flag, _) = calibrate(
        Scorers::default(),
        ScoringMethod::Lexical,
        |t, c| t.lexical_support = c,
        |t, c| t.lexical_weak = c,
        (THRESHOLDS.lexical_support, THRESHOLDS.lexical_weak),
    );
    assert!(
        support >= 0.6 && flag >= 0.6,
        "support {support}, flag {flag}"
    );
}

#[test]
fn need_cut_is_calibrated() {
    let f = fixture();
    let replay = replay();
    let scored: Vec<(bool, f32)> = f
        .needs
        .iter()
        .map(|n| {
            let text = clean_evidence(&f.passages[&n.passage].text);
            let score = need_coverage(&replay, &n.need, &text, true)
                .unwrap_or_else(|| panic!("no recorded score for need {:?}", n.need));
            (n.covered, score)
        })
        .collect();
    let shipped = assert_best_cut("need cut", THRESHOLDS.need, 1.0, |c| {
        let correct = scored
            .iter()
            .filter(|(covered, s)| (*s >= c) == *covered)
            .count();
        correct as f32 / scored.len() as f32
    });
    // Missing needs are never reported covered at the shipped cut: a
    // covered need judged missing costs one more search, a missing need
    // judged covered would hide a gap.
    assert!(scored
        .iter()
        .all(|(covered, s)| *covered || *s < THRESHOLDS.need));
    assert!(shipped >= 0.85, "{shipped}");
}

/// Every pair the tests above score, with the live models.
fn score_everything(relevance: &dyn PassageScorer, nli: &dyn EntailmentScorer) {
    let f = fixture();
    for c in &f.claims {
        let text = std::slice::from_ref(&f.passages[&c.passage].text);
        support(
            Scorers {
                relevance: Some(relevance),
                entailment: Some(nli),
            },
            &c.claim,
            text,
        );
        support(
            Scorers {
                relevance: Some(relevance),
                entailment: None,
            },
            &c.claim,
            text,
        );
    }
    for n in &f.needs {
        need_coverage(
            relevance,
            &n.need,
            &clean_evidence(&f.passages[&n.passage].text),
            true,
        );
    }
}

/// Wraps the live models and records every result.
struct Recording<'a> {
    relevance: &'a dyn PassageScorer,
    nli: &'a dyn EntailmentScorer,
    out: Mutex<Recorded>,
}

impl PassageScorer for Recording<'_> {
    fn score(&self, query: &str, passages: &[String]) -> Result<Vec<f32>, String> {
        let scores = self.relevance.score(query, passages)?;
        let mut out = self.out.lock().unwrap();
        for (p, s) in passages.iter().zip(&scores) {
            out.relevance.insert(key(query, p), *s);
        }
        Ok(scores)
    }
}

impl EntailmentScorer for Recording<'_> {
    fn entail(&self, hypothesis: &str, premises: &[String]) -> Result<Vec<Entailment>, String> {
        let results = self.nli.entail(hypothesis, premises)?;
        let mut out = self.out.lock().unwrap();
        for (p, r) in premises.iter().zip(&results) {
            out.entailment.insert(
                key(hypothesis, p),
                [r.contradiction, r.entailment, r.neutral],
            );
        }
        Ok(results)
    }
}

fn live_models() -> (
    crate::reranking::CrossEncoderReranker,
    crate::reranking::NliModel,
) {
    let root = std::path::PathBuf::from(
        std::env::var_os("SHODH_TEST_MODELS").expect("SHODH_TEST_MODELS must point at the models"),
    );
    let reranker = crate::reranking::CrossEncoderReranker::new(&root.join("ms-marco-MiniLM-L6-v2"))
        .expect("load the reranker");
    let nli = crate::reranking::NliModel::new(&root.join("nli-deberta-v3-xsmall"))
        .expect("load the NLI model");
    (reranker, nli)
}

fn record_live() -> Recorded {
    let (reranker, nli) = live_models();
    let recording = Recording {
        relevance: &reranker,
        nli: &nli,
        out: Mutex::new(Recorded::default()),
    };
    score_everything(&recording, &recording);
    recording.out.into_inner().unwrap()
}

#[test]
#[cfg_attr(
    not(shodh_test_models),
    ignore = "requires SHODH_TEST_MODELS with ms-marco-MiniLM-L6-v2/ and nli-deberta-v3-xsmall/"
)]
fn recorded_scores_match_the_models() {
    let live = record_live();
    let recorded = replay().0;
    assert_eq!(
        live.relevance.len(),
        recorded.relevance.len(),
        "re-record: the scored pairs changed"
    );
    assert_eq!(
        live.entailment.len(),
        recorded.entailment.len(),
        "re-record: the scored pairs changed"
    );
    for (k, v) in &live.relevance {
        let r = recorded.relevance.get(k).expect("pair recorded");
        assert!((v - r).abs() < 0.05, "logit drifted: {v} vs {r}");
    }
    for (k, v) in &live.entailment {
        let r = recorded.entailment.get(k).expect("pair recorded");
        for i in 0..3 {
            assert!(
                (v[i] - r[i]).abs() < 0.02,
                "probability drifted: {v:?} vs {r:?}"
            );
        }
    }
}

#[test]
#[ignore = "rewrites fixtures/grounding/model-scores.json from the live models"]
fn record_model_scores() {
    let live = record_live();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(SCORES_PATH);
    let text = serde_json::to_string_pretty(&live).expect("encode");
    std::fs::write(&path, format!("{text}\n")).expect("write model-scores.json");
    println!(
        "recorded {} logits and {} entailment results to {}",
        live.relevance.len(),
        live.entailment.len(),
        path.display()
    );
}
