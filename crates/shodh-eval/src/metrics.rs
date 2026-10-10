//! Scoring: pure functions over what a run observed.
//!
//! Retrieval (no LLM): the first rank of an expected document (results are
//! chunks; a document counts once, at its first chunk), of a chunk on an
//! expected page, and of a chunk containing an expected passage.
//!
//! Answers: fact recall, citation correctness (the passages an answer cites
//! contain the facts it states), the grounding verifier's verdicts, and
//! refusal correctness for unanswerable questions.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::dataset::EvalCase;
use crate::text::{contains_span, is_refusal};

/// Cut-offs reported for retrieval.
pub const KS: [usize; 3] = [1, 5, 10];

/// One retrieved chunk, in rank order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievedChunk {
    /// Document key, `None` when the source is not a corpus file.
    pub key: Option<String>,
    /// First and last page the chunk covers.
    pub pages: Option<(u32, u32)>,
    pub text: String,
}

/// Where the expected evidence first appeared (1-based ranks).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseRanks {
    /// Rank among distinct documents.
    pub doc_rank: Option<usize>,
    /// Rank among chunks; `None` also when the case gives no pages.
    pub page_rank: Option<usize>,
    /// Rank among chunks; `None` also when the case gives no passage.
    pub passage_rank: Option<usize>,
    pub has_pages: bool,
    pub has_passage: bool,
}

/// `expected` maps each expected document key to its source spec (pages and
/// passage); keys must be spelled like [`RetrievedChunk::key`].
pub fn rank_case(
    expected: &BTreeMap<String, (Vec<u32>, Option<String>)>,
    chunks: &[RetrievedChunk],
) -> CaseRanks {
    let mut docs: Vec<&str> = Vec::new();
    let mut doc_rank = None;
    let mut page_rank = None;
    let mut passage_rank = None;
    for (i, chunk) in chunks.iter().enumerate() {
        let Some(key) = chunk.key.as_deref() else {
            continue;
        };
        if !docs.contains(&key) {
            docs.push(key);
            if doc_rank.is_none() && expected.contains_key(key) {
                doc_rank = Some(docs.len());
            }
        }
        let Some((pages, passage)) = expected.get(key) else {
            continue;
        };
        if page_rank.is_none() && !pages.is_empty() {
            if let Some((start, end)) = chunk.pages {
                if pages.iter().any(|p| (start..=end).contains(p)) {
                    page_rank = Some(i + 1);
                }
            }
        }
        if passage_rank.is_none() {
            if let Some(passage) = passage {
                if contains_span(&chunk.text, passage) {
                    passage_rank = Some(i + 1);
                }
            }
        }
    }
    CaseRanks {
        doc_rank,
        page_rank,
        passage_rank,
        has_pages: expected.values().any(|(p, _)| !p.is_empty()),
        has_passage: expected.values().any(|(_, p)| p.is_some()),
    }
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / n as f64)
}

fn hit(rank: Option<usize>, k: usize) -> f64 {
    if rank.is_some_and(|r| r <= k) {
        1.0
    } else {
        0.0
    }
}

/// Aggregate retrieval metrics (all higher-is-better, in 0..=1): `hit@k` and
/// `mrr` over documents, `page_hit@k` over cases with pages, `passage_hit@k`
/// over cases with a passage. Metrics without applicable cases are absent.
pub fn retrieval_metrics(cases: &[CaseRanks]) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for k in KS {
        if let Some(m) = mean(cases.iter().map(|c| hit(c.doc_rank, k))) {
            out.insert(format!("hit@{k}"), m);
        }
        let paged = cases.iter().filter(|c| c.has_pages);
        if let Some(m) = mean(paged.map(|c| hit(c.page_rank, k))) {
            out.insert(format!("page_hit@{k}"), m);
        }
        let spans = cases.iter().filter(|c| c.has_passage);
        if let Some(m) = mean(spans.map(|c| hit(c.passage_rank, k))) {
            out.insert(format!("passage_hit@{k}"), m);
        }
    }
    if let Some(m) = mean(
        cases
            .iter()
            .map(|c| c.doc_rank.map_or(0.0, |r| 1.0 / r as f64)),
    ) {
        out.insert("mrr".to_string(), m);
    }
    out
}

/// A numbered passage an answer could cite (from `search_documents`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenPassage {
    /// Document key, `None` when not a corpus file.
    pub key: Option<String>,
    pub page: Option<String>,
    pub text: String,
}

/// The grounding verifier's counts for the final answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct GroundingCounts {
    pub checked: u32,
    pub supported: u32,
    pub weak: u32,
    pub unsupported: u32,
    pub uncited: u32,
    pub invalid: u32,
    pub unchecked: u32,
}

/// Everything an answer run produced that scoring needs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnswerTranscript {
    pub answer: String,
    pub passages: BTreeMap<u32, SeenPassage>,
    pub cited: BTreeSet<u32>,
    pub grounding: Option<GroundingCounts>,
    /// The run ended without an answer (provider error, timeout, abort).
    pub error: Option<String>,
}

/// Per-case answer scores; `None` where a score does not apply.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnswerScore {
    /// Expected facts the answer states / expected facts.
    pub fact_recall: Option<f64>,
    /// Of the facts the answer states, the share found in a passage it cites.
    pub citation_correct: Option<f64>,
    /// Whether any cited passage is from an expected document.
    pub cited_expected_source: Option<bool>,
    /// Verified claims supported by their citations (weak counts half),
    /// over checkable claims.
    pub claims_supported: Option<f64>,
    /// Claims the verifier flagged: unsupported, uncited or citing a
    /// passage that does not exist.
    pub flagged: Option<u32>,
    /// Unanswerable: the answer declined. Answerable: `None`.
    pub refusal_correct: Option<bool>,
    /// Answerable: the answer declined although the corpus answers it.
    pub false_refusal: Option<bool>,
}

pub fn score_answer(
    case: &EvalCase,
    expected_keys: &BTreeSet<String>,
    t: &AnswerTranscript,
) -> AnswerScore {
    if t.error.is_some() {
        return AnswerScore::default();
    }
    let refused = is_refusal(&t.answer);
    let grounding = t.grounding.map(|g| {
        let checkable = g.checked.saturating_sub(g.unchecked);
        let supported = (checkable > 0)
            .then(|| (f64::from(g.supported) + f64::from(g.weak) / 2.0) / f64::from(checkable));
        (supported, g.unsupported + g.uncited + g.invalid)
    });
    let (claims_supported, flagged) = match grounding {
        Some((s, f)) => (s, Some(f)),
        None => (None, None),
    };
    if !case.answerable {
        return AnswerScore {
            refusal_correct: Some(refused),
            claims_supported,
            flagged,
            ..AnswerScore::default()
        };
    }
    let cited: Vec<&SeenPassage> = t.cited.iter().filter_map(|n| t.passages.get(n)).collect();
    let stated: Vec<&String> = case
        .facts
        .iter()
        .filter(|f| contains_span(&t.answer, f))
        .collect();
    let fact_recall =
        (!case.facts.is_empty()).then(|| stated.len() as f64 / case.facts.len() as f64);
    let citation_correct = (!stated.is_empty()).then(|| {
        let backed = stated
            .iter()
            .filter(|f| cited.iter().any(|p| contains_span(&p.text, f)))
            .count();
        backed as f64 / stated.len() as f64
    });
    let cited_expected_source = Some(
        cited
            .iter()
            .any(|p| p.key.as_ref().is_some_and(|k| expected_keys.contains(k))),
    );
    AnswerScore {
        fact_recall,
        citation_correct,
        cited_expected_source,
        claims_supported,
        flagged,
        refusal_correct: None,
        false_refusal: Some(refused),
    }
}

/// Aggregate answer metrics. Higher-is-better ones are means in 0..=1;
/// `flagged_per_answer` and `false_refusal` are lower-is-better and named so.
pub fn answer_metrics(scores: &[AnswerScore], errors: usize) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    let mut put = |name: &str, value: Option<f64>| {
        if let Some(v) = value {
            out.insert(name.to_string(), v);
        }
    };
    let flag = |b: bool| if b { 1.0 } else { 0.0 };
    put(
        "fact_recall",
        mean(scores.iter().filter_map(|s| s.fact_recall)),
    );
    put(
        "citation_correct",
        mean(scores.iter().filter_map(|s| s.citation_correct)),
    );
    put(
        "cited_expected_source",
        mean(
            scores
                .iter()
                .filter_map(|s| s.cited_expected_source.map(flag)),
        ),
    );
    put(
        "claims_supported",
        mean(scores.iter().filter_map(|s| s.claims_supported)),
    );
    put(
        "flagged_per_answer",
        mean(scores.iter().filter_map(|s| s.flagged.map(f64::from))),
    );
    put(
        "refusal_correct",
        mean(scores.iter().filter_map(|s| s.refusal_correct.map(flag))),
    );
    put(
        "false_refusal",
        mean(scores.iter().filter_map(|s| s.false_refusal.map(flag))),
    );
    let total = scores.len() + errors;
    if total > 0 {
        put("answered", Some((total - errors) as f64 / total as f64));
    }
    out
}

/// Metrics where lower is better (the gate inverts the comparison).
pub fn lower_is_better(metric: &str) -> bool {
    matches!(metric, "flagged_per_answer" | "false_refusal")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::ExpectedSource;

    fn chunk(key: &str, pages: Option<(u32, u32)>, text: &str) -> RetrievedChunk {
        RetrievedChunk {
            key: Some(key.into()),
            pages,
            text: text.into(),
        }
    }

    fn expect(
        entries: &[(&str, &[u32], Option<&str>)],
    ) -> BTreeMap<String, (Vec<u32>, Option<String>)> {
        entries
            .iter()
            .map(|(k, p, s)| (k.to_string(), (p.to_vec(), s.map(str::to_string))))
            .collect()
    }

    #[test]
    fn documents_count_once_at_their_first_chunk() {
        let chunks = [
            chunk("a", Some((1, 1)), "x"),
            chunk("a", Some((2, 2)), "y"),
            chunk("b", Some((3, 3)), "Total due: INR 7,96,500.00"),
        ];
        let ranks = rank_case(&expect(&[("b", &[3], Some("7,96,500"))]), &chunks);
        assert_eq!(ranks.doc_rank, Some(2));
        assert_eq!(ranks.page_rank, Some(3));
        assert_eq!(ranks.passage_rank, Some(3));
    }

    #[test]
    fn page_and_passage_need_the_expected_document() {
        let chunks = [
            chunk("other", Some((2, 2)), "the passage"),
            RetrievedChunk {
                key: None,
                pages: None,
                text: "the passage".into(),
            },
            chunk("a", Some((1, 4)), "nothing"),
        ];
        let ranks = rank_case(&expect(&[("a", &[3], Some("the passage"))]), &chunks);
        assert_eq!(ranks.doc_rank, Some(2));
        assert_eq!(ranks.page_rank, Some(3), "page 3 lies in 1..=4");
        assert_eq!(ranks.passage_rank, None);
    }

    #[test]
    fn aggregates_skip_inapplicable_cases() {
        let cases = [
            CaseRanks {
                doc_rank: Some(1),
                page_rank: Some(2),
                passage_rank: None,
                has_pages: true,
                has_passage: false,
            },
            CaseRanks {
                doc_rank: Some(4),
                page_rank: None,
                passage_rank: Some(6),
                has_pages: false,
                has_passage: true,
            },
            CaseRanks {
                doc_rank: None,
                ..CaseRanks::default()
            },
        ];
        let m = retrieval_metrics(&cases);
        assert!((m["hit@1"] - 1.0 / 3.0).abs() < 1e-12);
        assert!((m["hit@5"] - 2.0 / 3.0).abs() < 1e-12);
        assert!((m["mrr"] - (1.0 + 0.25) / 3.0).abs() < 1e-12);
        assert_eq!(m["page_hit@1"], 0.0);
        assert_eq!(m["page_hit@5"], 1.0);
        assert_eq!(m["passage_hit@5"], 0.0);
        assert_eq!(m["passage_hit@10"], 1.0);
        assert!(retrieval_metrics(&[]).is_empty());
    }

    fn case(answerable: bool, facts: &[&str]) -> EvalCase {
        EvalCase {
            id: "c".into(),
            question: "q".into(),
            answerable,
            sources: if answerable {
                vec![ExpectedSource {
                    file: "inv.pdf".into(),
                    pages: vec![],
                    passage: None,
                }]
            } else {
                vec![]
            },
            facts: facts.iter().map(|f| f.to_string()).collect(),
            keep: true,
            context: None,
        }
    }

    fn passage(key: &str, text: &str) -> SeenPassage {
        SeenPassage {
            key: Some(key.into()),
            page: Some("1".into()),
            text: text.into(),
        }
    }

    #[test]
    fn citation_correctness_needs_the_fact_in_a_cited_passage() {
        let keys: BTreeSet<String> = ["inv.pdf".to_string()].into();
        let t = AnswerTranscript {
            answer: "The total is INR 7,96,500 [1], due 30 June 2025 [2].".into(),
            passages: [
                (1, passage("inv.pdf", "Total amount due: INR 7,96,500.00")),
                (2, passage("msa.pdf", "Payment within 45 days.")),
            ]
            .into(),
            cited: [1, 2].into(),
            grounding: Some(GroundingCounts {
                checked: 2,
                supported: 1,
                weak: 0,
                unsupported: 1,
                ..Default::default()
            }),
            error: None,
        };
        let s = score_answer(
            &case(true, &["7,96,500", "30 June 2025", "GSTIN"]),
            &keys,
            &t,
        );
        assert!((s.fact_recall.unwrap() - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!(s.citation_correct, Some(0.5));
        assert_eq!(s.cited_expected_source, Some(true));
        assert_eq!(s.claims_supported, Some(0.5));
        assert_eq!(s.flagged, Some(1));
        assert_eq!(s.false_refusal, Some(false));
        assert_eq!(s.refusal_correct, None);
    }

    #[test]
    fn uncited_passages_do_not_back_facts() {
        let keys: BTreeSet<String> = ["inv.pdf".to_string()].into();
        let t = AnswerTranscript {
            answer: "It is INR 7,96,500.".into(),
            passages: [(1, passage("inv.pdf", "INR 7,96,500.00"))].into(),
            cited: BTreeSet::new(),
            grounding: None,
            error: None,
        };
        let s = score_answer(&case(true, &["7,96,500"]), &keys, &t);
        assert_eq!(s.citation_correct, Some(0.0));
        assert_eq!(s.cited_expected_source, Some(false));
        assert_eq!(s.claims_supported, None);
    }

    #[test]
    fn unanswerable_cases_score_the_refusal() {
        let keys = BTreeSet::new();
        let declined = AnswerTranscript {
            answer: "Your documents don't mention a CEO.".into(),
            ..Default::default()
        };
        let invented = AnswerTranscript {
            answer: "The CEO is Jane Doe.".into(),
            ..Default::default()
        };
        assert_eq!(
            score_answer(&case(false, &[]), &keys, &declined).refusal_correct,
            Some(true)
        );
        assert_eq!(
            score_answer(&case(false, &[]), &keys, &invented).refusal_correct,
            Some(false)
        );
        let failed = AnswerTranscript {
            error: Some("rate limited".into()),
            ..Default::default()
        };
        assert_eq!(
            score_answer(&case(false, &[]), &keys, &failed),
            AnswerScore::default()
        );
    }

    #[test]
    fn answer_aggregates_count_errors_as_unanswered() {
        let scores = [
            AnswerScore {
                fact_recall: Some(1.0),
                flagged: Some(2),
                false_refusal: Some(false),
                ..Default::default()
            },
            AnswerScore {
                fact_recall: Some(0.0),
                flagged: Some(0),
                refusal_correct: Some(true),
                ..Default::default()
            },
        ];
        let m = answer_metrics(&scores, 2);
        assert_eq!(m["fact_recall"], 0.5);
        assert_eq!(m["flagged_per_answer"], 1.0);
        assert_eq!(m["refusal_correct"], 1.0);
        assert_eq!(m["answered"], 0.5);
        assert!(!m.contains_key("citation_correct"));
        assert!(lower_is_better("flagged_per_answer") && !lower_is_better("hit@5"));
    }
}
