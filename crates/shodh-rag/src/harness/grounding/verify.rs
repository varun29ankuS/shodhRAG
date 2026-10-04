//! The claim verifier: local and deterministic, no LLM judge.
//!
//! For every claim of an answer ([`super::claims::split_claims`]):
//! 1. citation numbers no passage of the run has → `invalid_citation`;
//! 2. no citation, in an answer built from passages, stating a fact →
//!    `uncited_factual`;
//! 3. otherwise the claim is checked against the text of the passages it
//!    cites (plus what the run read of the same document with
//!    `open_document`):
//!    * every number it states must appear in that text (spec §7.4),
//!    * with the entailment model installed, the text is cut into windows of
//!      three sentences, the windows sharing most words with the claim are
//!      classified (premise = window, hypothesis = claim) and the best
//!      entailment probability is the support score;
//!    * without it, the cross-encoder's best sigmoid over the cited
//!      passages is (it measures relevance, so it cannot see a reversed or
//!      swapped statement; `calibration.rs` shows the difference), and
//!      without either model the share of the claim's significant words
//!      found in the text is,
//!    * thresholds ([`Thresholds`], calibrated in `calibration.rs` on
//!      passages of the user's corpus) turn the score into `supported`,
//!      `weak` or `unsupported`.
//!
//! Flagged claims are also scored against every passage of the run, so the
//! transcript can offer the closest one.

use std::collections::HashSet;
use std::sync::Arc;

use super::claims::{split_claims, Claim};
use super::numbers::missing_numbers;
use crate::harness::events::{
    ClaimCheck, ClaimOutcome, CoverageState, GroundingSummary, NeedCheck, ScoringMethod,
};
use crate::harness::web::relevance::{
    query_terms, sigmoid, stem, words, PassageScorer, SharedScorer,
};
use crate::reranking::{Entailment, NliModel};

/// Classifies whether premises entail a hypothesis. Implemented by the NLI
/// model; tests replay recorded probabilities.
pub trait EntailmentScorer: Send + Sync {
    /// One result per premise, in order.
    fn entail(&self, hypothesis: &str, premises: &[String]) -> Result<Vec<Entailment>, String>;
}

impl EntailmentScorer for NliModel {
    fn entail(&self, hypothesis: &str, premises: &[String]) -> Result<Vec<Entailment>, String> {
        let pairs: Vec<(&str, &str)> = premises.iter().map(|p| (p.as_str(), hypothesis)).collect();
        self.classify(&pairs).map_err(|e| e.to_string())
    }
}

/// A shared entailment scorer handle.
pub type SharedEntailment = Arc<dyn EntailmentScorer>;

/// The local models available for one check.
#[derive(Clone, Default)]
pub struct ScorerSet {
    pub relevance: Option<SharedScorer>,
    pub entailment: Option<SharedEntailment>,
}

/// Borrowed view of a [`ScorerSet`].
#[derive(Clone, Copy, Default)]
pub struct Scorers<'a> {
    pub relevance: Option<&'a dyn PassageScorer>,
    pub entailment: Option<&'a dyn EntailmentScorer>,
}

impl ScorerSet {
    pub fn scorers(&self) -> Scorers<'_> {
        Scorers {
            relevance: self.relevance.as_deref(),
            entailment: self.entailment.as_deref(),
        }
    }
}

/// One numbered source of the run, as the model saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub n: u32,
    /// File path or URL; ties the passage to text opened from the same file.
    pub path: String,
    pub text: String,
    /// False for text the cross-encoder cannot judge (a search provider's
    /// answer fragments); such sources are only checked for numbers.
    pub checkable: bool,
}

/// One text block of the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerMessage {
    pub id: String,
    pub text: String,
}

/// Cutoffs on the support score (0..=1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// Entailment probability at or above which a claim is supported.
    pub entail_support: f32,
    /// Entailment probability at or above which it is weakly supported.
    pub entail_weak: f32,
    /// Cross-encoder sigmoid at or above which a claim is supported.
    pub support: f32,
    /// Cross-encoder sigmoid at or above which it is weakly supported.
    pub weak: f32,
    /// Word coverage at or above which a claim is supported (no model).
    pub lexical_support: f32,
    /// Word coverage at or above which it is weakly supported (no model).
    pub lexical_weak: f32,
    /// Lowest score at which a passage is offered as the closest one.
    pub closest: f32,
    /// Cross-encoder sigmoid at or above which a passage covers an
    /// information need.
    pub need: f32,
    /// Word coverage at or above which a passage covers a need (no model).
    pub lexical_need: f32,
}

/// Calibrated on the 88 labeled claims and 20 needs of
/// `fixtures/grounding/claims.json` (`calibration.rs` sweeps every cut and
/// asserts each shipped one is among the most accurate):
/// * entailment: 93% of claims judged right at any support cut from 0.10 to
///   0.35 (the top of that range is shipped) and any weak cut from 0.10 up;
///   every reversed statement and 6 of 7 swapped ones are flagged;
/// * cross-encoder fallback: 75% at best (support 0.45 to 0.80, weak band
///   below that adds nothing for flagging); it flags 1 of 10 reversed and 1
///   of 7 swapped statements, because relevance is not support;
/// * word overlap: 78% at 0.6, with no useful weak band;
/// * needs: 18 of 20 at every cut; no missing need is ever judged covered.
pub const THRESHOLDS: Thresholds = Thresholds {
    entail_support: 0.35,
    entail_weak: 0.2,
    support: 0.6,
    weak: 0.45,
    lexical_support: 0.6,
    lexical_weak: 0.6,
    closest: 0.1,
    need: 0.5,
    lexical_need: 0.6,
};

/// Characters of opened document text per scored window.
const WINDOW_CHARS: usize = 1_200;
const WINDOW_OVERLAP: usize = 200;
/// Opened-text windows scored per cited document.
const MAX_WINDOWS_PER_PATH: usize = 12;
/// Passages scored by the cross-encoder when looking for the closest one
/// (pre-selected by word overlap).
const CLOSEST_CANDIDATES: usize = 8;
/// Sentences per window classified by the entailment model.
const SENTENCES_PER_WINDOW: usize = 3;
/// Windows classified per claim (those sharing most words with it).
const ENTAILMENT_WINDOWS: usize = 4;

/// What a document tool showed the model from a file, beyond numbered passages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedText {
    pub path: String,
    pub text: String,
}

/// Inputs of one verification.
#[derive(Debug, Clone, Copy)]
pub struct VerifyInput<'a> {
    pub messages: &'a [AnswerMessage],
    pub passages: &'a [Evidence],
    pub opened: &'a [OpenedText],
}

/// Share of the claim's significant words (stemmed) found in `evidence`.
pub fn word_coverage(claim: &str, evidence: &str) -> f32 {
    let terms = query_terms(claim);
    if terms.is_empty() {
        return 0.0;
    }
    let found: HashSet<String> = words(evidence).iter().map(|w| stem(w)).collect();
    let hit = terms.iter().filter(|t| found.contains(*t)).count();
    hit as f32 / terms.len() as f32
}

fn windows(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= WINDOW_CHARS {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() && out.len() < MAX_WINDOWS_PER_PATH {
        let end = (start + WINDOW_CHARS).min(chars.len());
        out.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start = end - WINDOW_OVERLAP;
    }
    out
}

/// Sentences of `text`: split after `.`, `!` or `?` followed by whitespace.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(c, '.' | '!' | '?') {
            if let Some(&(j, next)) = chars.peek() {
                if next.is_whitespace() {
                    let piece = text[start..=i].trim();
                    if !piece.is_empty() {
                        out.push(piece);
                    }
                    start = j;
                }
            }
        }
    }
    let rest = text[start..].trim();
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// Overlapping windows of [`SENTENCES_PER_WINDOW`] sentences.
pub(crate) fn sentence_windows(text: &str) -> Vec<String> {
    let parts = sentences(text);
    if parts.len() <= SENTENCES_PER_WINDOW {
        return vec![text.trim().to_string()];
    }
    parts
        .windows(SENTENCES_PER_WINDOW)
        .map(|w| w.join(" "))
        .collect()
}

/// The windows of `texts` most likely to hold the claim: ranked by word
/// coverage, the best [`ENTAILMENT_WINDOWS`].
pub fn entailment_windows(claim: &str, texts: &[String]) -> Vec<String> {
    let mut windows: Vec<(String, f32)> = texts
        .iter()
        .flat_map(|t| sentence_windows(t))
        .map(|w| {
            let score = word_coverage(claim, &w);
            (w, score)
        })
        .collect();
    windows.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut seen = HashSet::new();
    windows
        .into_iter()
        .map(|(w, _)| w)
        .filter(|w| seen.insert(w.clone()))
        .take(ENTAILMENT_WINDOWS)
        .collect()
}

/// Scores `query` against `texts`: cross-encoder sigmoids, or `None` when
/// the scorer is absent or failed (which is logged).
fn neural_scores(
    scorer: Option<&dyn PassageScorer>,
    query: &str,
    texts: &[String],
) -> Option<Vec<f32>> {
    let scorer = scorer?;
    if texts.is_empty() {
        return Some(Vec::new());
    }
    match scorer.score(query, texts) {
        Ok(logits) if logits.len() == texts.len() => {
            Some(logits.into_iter().map(sigmoid).collect())
        }
        Ok(logits) => {
            tracing::warn!(target: "shodh::grounding", expected = texts.len(), got = logits.len(), "scorer returned the wrong number of scores; using word overlap");
            None
        }
        Err(e) => {
            tracing::warn!(target: "shodh::grounding", error = %e, "scorer failed; using word overlap");
            None
        }
    }
}

/// How well `texts` support `claim`: the best score, and how it was scored
/// (entailment model, else cross-encoder, else word overlap; a model that
/// fails is logged and the next one is used).
pub fn support(scorers: Scorers<'_>, claim: &str, texts: &[String]) -> (f32, ScoringMethod) {
    if let Some(nli) = scorers.entailment {
        let windows = entailment_windows(claim, texts);
        if !windows.is_empty() {
            match nli.entail(claim, &windows) {
                Ok(results) if results.len() == windows.len() => {
                    let best = results.iter().map(|r| r.entailment).fold(0.0_f32, f32::max);
                    return (best, ScoringMethod::Entailment);
                }
                Ok(results) => {
                    tracing::warn!(target: "shodh::grounding", expected = windows.len(), got = results.len(), "entailment model returned the wrong number of results; using the reranker");
                }
                Err(e) => {
                    tracing::warn!(target: "shodh::grounding", error = %e, "entailment model failed; using the reranker");
                }
            }
        }
    }
    match neural_scores(scorers.relevance, claim, texts) {
        Some(scores) => (
            scores.into_iter().fold(0.0_f32, f32::max),
            ScoringMethod::CrossEncoder,
        ),
        None => {
            let joined = texts.join("\n");
            (word_coverage(claim, &joined), ScoringMethod::Lexical)
        }
    }
}

/// The checkable passage that best supports `claim`, with its score, when
/// it reaches the `closest` cutoff.
fn closest_passage(
    scorer: Option<&dyn PassageScorer>,
    claim: &str,
    passages: &[Evidence],
    thresholds: &Thresholds,
) -> Option<(u32, f32)> {
    let mut candidates: Vec<(&Evidence, f32)> = passages
        .iter()
        .filter(|p| p.checkable && !p.text.trim().is_empty())
        .map(|p| (p, word_coverage(claim, &p.text)))
        .collect();
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
    candidates.truncate(CLOSEST_CANDIDATES);
    let texts: Vec<String> = candidates.iter().map(|(p, _)| p.text.clone()).collect();
    let (scores, cutoff) = match neural_scores(scorer, claim, &texts) {
        Some(scores) => (scores, thresholds.closest),
        None => (
            candidates.iter().map(|(_, s)| *s).collect(),
            thresholds.lexical_weak,
        ),
    };
    candidates
        .iter()
        .zip(scores)
        .filter(|(_, s)| *s >= cutoff)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|((p, _), s)| (p.n, s))
}

/// The outcome a support score gives (numbers already checked).
pub fn outcome_for(score: f32, method: ScoringMethod, t: &Thresholds) -> ClaimOutcome {
    let (support, weak) = match method {
        ScoringMethod::Entailment => (t.entail_support, t.entail_weak),
        ScoringMethod::CrossEncoder => (t.support, t.weak),
        ScoringMethod::Lexical => (t.lexical_support, t.lexical_weak),
    };
    if score >= support {
        ClaimOutcome::Supported
    } else if score >= weak {
        ClaimOutcome::Weak
    } else {
        ClaimOutcome::Unsupported
    }
}

/// Check one claim. `None` when the claim is not checked at all (no
/// citation and not a factual statement in a sourced answer).
fn check_claim(
    message_id: &str,
    claim: &Claim,
    input: &VerifyInput<'_>,
    scorers: Scorers<'_>,
    thresholds: &Thresholds,
    methods: &mut HashSet<ScoringMethod>,
) -> Option<ClaimCheck> {
    let sourced_answer = !input.passages.is_empty();
    let mut check = ClaimCheck {
        message_id: message_id.to_string(),
        text: claim.text.clone(),
        anchor: claim.anchor.clone(),
        kind: claim.kind,
        outcome: ClaimOutcome::Unchecked,
        cited: claim.citations.clone(),
        invalid: Vec::new(),
        support: None,
        missing_numbers: Vec::new(),
        closest: None,
        closest_score: None,
    };
    if claim.citations.is_empty() {
        if !(claim.factual && sourced_answer) {
            return None;
        }
        check.outcome = ClaimOutcome::UncitedFactual;
    } else {
        let cited: Vec<&Evidence> = claim
            .citations
            .iter()
            .filter_map(|n| input.passages.iter().find(|p| p.n == *n))
            .collect();
        check.invalid = claim
            .citations
            .iter()
            .copied()
            .filter(|n| !input.passages.iter().any(|p| p.n == *n))
            .collect();
        if !check.invalid.is_empty() {
            check.outcome = ClaimOutcome::InvalidCitation;
        } else if !claim.statement {
            // "See [3] for the derivation": a pointer, not a claim.
            return None;
        } else {
            // The cited passages, plus text the run opened from their files.
            let mut texts: Vec<String> = Vec::new();
            let mut all_texts: Vec<&str> = Vec::new();
            let mut paths: Vec<&str> = Vec::new();
            for p in &cited {
                all_texts.push(&p.text);
                if p.checkable {
                    texts.push(p.text.clone());
                }
                if !paths.contains(&p.path.as_str()) {
                    paths.push(&p.path);
                }
            }
            for opened in input
                .opened
                .iter()
                .filter(|o| paths.contains(&o.path.as_str()))
            {
                all_texts.push(&opened.text);
                texts.extend(windows(&opened.text));
            }
            check.missing_numbers = missing_numbers(&claim.text, &all_texts);
            if texts.is_empty() {
                check.outcome = if check.missing_numbers.is_empty() {
                    ClaimOutcome::Unchecked
                } else {
                    ClaimOutcome::Unsupported
                };
            } else {
                let (score, method) = support(scorers, &claim.text, &texts);
                methods.insert(method);
                check.support = Some(score);
                check.outcome = if check.missing_numbers.is_empty() {
                    outcome_for(score, method, thresholds)
                } else {
                    ClaimOutcome::Unsupported
                };
            }
        }
    }
    if matches!(
        check.outcome,
        ClaimOutcome::Weak
            | ClaimOutcome::Unsupported
            | ClaimOutcome::UncitedFactual
            | ClaimOutcome::InvalidCitation
    ) {
        if let Some((n, score)) =
            closest_passage(scorers.relevance, &claim.text, input.passages, thresholds)
        {
            // Offering the passage the claim already cites adds nothing.
            if !claim.citations.contains(&n) {
                check.closest = Some(n);
                check.closest_score = Some(score);
            }
        }
    }
    Some(check)
}

/// Verify an answer. Returns the checked claims in reading order and the
/// scoring method (cross-encoder only when every scored claim used it).
pub fn verify_answer(
    input: &VerifyInput<'_>,
    scorers: Scorers<'_>,
    thresholds: &Thresholds,
) -> (Vec<ClaimCheck>, ScoringMethod) {
    let mut methods = HashSet::new();
    let mut checks = Vec::new();
    for message in input.messages {
        for claim in split_claims(&message.text) {
            if let Some(check) = check_claim(
                &message.id,
                &claim,
                input,
                scorers,
                thresholds,
                &mut methods,
            ) {
                checks.push(check);
            }
        }
    }
    // The weakest method any claim was scored with; with nothing scored,
    // the best one available.
    let method = if methods.contains(&ScoringMethod::Lexical) {
        ScoringMethod::Lexical
    } else if methods.contains(&ScoringMethod::CrossEncoder) {
        ScoringMethod::CrossEncoder
    } else if methods.contains(&ScoringMethod::Entailment) || scorers.entailment.is_some() {
        ScoringMethod::Entailment
    } else if scorers.relevance.is_some() {
        ScoringMethod::CrossEncoder
    } else {
        ScoringMethod::Lexical
    };
    (checks, method)
}

/// Counts and the grounding score of a set of checked claims.
pub fn summarise(checks: &[ClaimCheck]) -> GroundingSummary {
    let mut s = GroundingSummary {
        checked: u32::try_from(checks.len()).unwrap_or(u32::MAX),
        ..GroundingSummary::default()
    };
    for c in checks {
        match c.outcome {
            ClaimOutcome::Supported => s.supported += 1,
            ClaimOutcome::Weak => s.weak += 1,
            ClaimOutcome::Unsupported => s.unsupported += 1,
            ClaimOutcome::UncitedFactual => s.uncited += 1,
            ClaimOutcome::InvalidCitation => s.invalid += 1,
            ClaimOutcome::Unchecked => s.unchecked += 1,
        }
    }
    let scored = s.checked - s.unchecked;
    s.score = (scored > 0).then(|| (s.supported as f32 + 0.5 * s.weak as f32) / scored as f32);
    s
}

/// Passages whose windows are scored for a need (the best by whole-passage
/// score).
const NEED_WINDOW_PASSAGES: usize = 3;

/// How well `passage` covers `need`: the best cross-encoder sigmoid of the
/// whole passage and, for the strongest candidates, of its sentence
/// windows (a long passage can bury the one sentence that answers).
pub fn need_coverage(
    scorer: &dyn PassageScorer,
    need: &str,
    passage: &str,
    with_windows: bool,
) -> Option<f32> {
    let whole = neural_scores(Some(scorer), need, &[passage.to_string()])?
        .first()
        .copied()
        .unwrap_or(0.0);
    if !with_windows {
        return Some(whole);
    }
    let windows = sentence_windows(passage);
    if windows.len() <= 1 {
        return Some(whole);
    }
    let best = neural_scores(Some(scorer), need, &windows)?
        .into_iter()
        .fold(whole, f32::max);
    Some(best)
}

/// Whether the retrieved passages cover each information need: a need is
/// covered when some passage reaches the need cutoff ([`need_coverage`];
/// without the cross-encoder, word coverage of the need).
pub fn check_needs(
    needs: &[(String, String)],
    passages: &[Evidence],
    scorer: Option<&dyn PassageScorer>,
    thresholds: &Thresholds,
) -> Vec<NeedCheck> {
    let checkable: Vec<&Evidence> = passages
        .iter()
        .filter(|p| p.checkable && !p.text.trim().is_empty())
        .collect();
    needs
        .iter()
        .map(|(id, text)| {
            let mut ranked: Vec<(&Evidence, f32)> = checkable
                .iter()
                .map(|p| (*p, word_coverage(text, &p.text)))
                .collect();
            let (scored, cutoff) = match scorer {
                Some(scorer) => {
                    // Pre-select by words, score whole passages, then the
                    // windows of the best few.
                    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
                    ranked.truncate(CLOSEST_CANDIDATES);
                    let mut whole: Vec<(&Evidence, f32)> = Vec::new();
                    let mut failed = false;
                    for (p, _) in &ranked {
                        match need_coverage(scorer, text, &p.text, false) {
                            Some(score) => whole.push((*p, score)),
                            None => {
                                failed = true;
                                break;
                            }
                        }
                    }
                    if failed {
                        (ranked, thresholds.lexical_need)
                    } else {
                        whole.sort_by(|a, b| b.1.total_cmp(&a.1));
                        for entry in whole.iter_mut().take(NEED_WINDOW_PASSAGES) {
                            if let Some(score) = need_coverage(scorer, text, &entry.0.text, true) {
                                entry.1 = entry.1.max(score);
                            }
                        }
                        (whole, thresholds.need)
                    }
                }
                None => (ranked, thresholds.lexical_need),
            };
            let mut covering: Vec<(u32, f32)> = scored
                .into_iter()
                .filter(|(_, s)| *s >= cutoff)
                .map(|(p, s)| (p.n, s))
                .collect();
            covering.sort_by(|a, b| b.1.total_cmp(&a.1));
            covering.truncate(3);
            NeedCheck {
                id: id.clone(),
                text: text.clone(),
                state: if covering.is_empty() {
                    CoverageState::Missing
                } else {
                    CoverageState::Covered
                },
                passages: covering.into_iter().map(|(n, _)| n).collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scores by word coverage scaled to a logit, so tests exercise the
    /// cross-encoder path deterministically.
    struct Overlap;
    impl PassageScorer for Overlap {
        fn score(&self, query: &str, passages: &[String]) -> Result<Vec<f32>, String> {
            Ok(passages
                .iter()
                .map(|p| 8.0 * word_coverage(query, p) - 4.0)
                .collect())
        }
    }

    fn passage(n: u32, text: &str) -> Evidence {
        Evidence {
            n,
            path: format!("c:/docs/{n}.pdf"),
            text: text.to_string(),
            checkable: true,
        }
    }

    fn message(text: &str) -> Vec<AnswerMessage> {
        vec![AnswerMessage {
            id: "m1".into(),
            text: text.into(),
        }]
    }

    fn passages() -> Vec<Evidence> {
        vec![
            passage(
                1,
                "Either party may terminate the agreement with sixty days written notice.",
            ),
            passage(
                2,
                "The renewal fee is 1,200 EUR per year, payable in advance.",
            ),
        ]
    }

    fn relevance(scorer: &dyn PassageScorer) -> Scorers<'_> {
        Scorers {
            relevance: Some(scorer),
            entailment: None,
        }
    }

    fn run(answer: &str, scorers: Scorers<'_>) -> Vec<ClaimCheck> {
        let messages = message(answer);
        let passages = passages();
        verify_answer(
            &VerifyInput {
                messages: &messages,
                passages: &passages,
                opened: &[],
            },
            scorers,
            &THRESHOLDS,
        )
        .0
    }

    #[test]
    fn outcomes_cover_every_case() {
        let checks = run(
            "Either party may terminate the agreement with 60 days written notice [1]. \
             The renewal fee is 900 EUR per year [2]. \
             Termination requires a court order [9]. \
             The agreement renews automatically every year with a fee payable in advance. \
             I searched the contracts folder.",
            relevance(&Overlap),
        );
        let outcomes: Vec<ClaimOutcome> = checks.iter().map(|c| c.outcome).collect();
        assert_eq!(
            outcomes,
            vec![
                ClaimOutcome::Supported,
                ClaimOutcome::Unsupported,
                ClaimOutcome::InvalidCitation,
                ClaimOutcome::UncitedFactual,
            ]
        );
        assert_eq!(checks[1].missing_numbers, vec!["900"]);
        assert_eq!(checks[2].invalid, vec![9]);
        assert_eq!(checks[3].closest, Some(2), "the closest passage is offered");
        let summary = summarise(&checks);
        assert_eq!((summary.checked, summary.supported), (4, 1));
        assert_eq!(summary.score, Some(0.25));
    }

    #[test]
    fn uncited_facts_are_not_flagged_without_sources() {
        let messages = message("Paris is the capital of France and has many famous museums.");
        let (checks, method) = verify_answer(
            &VerifyInput {
                messages: &messages,
                passages: &[],
                opened: &[],
            },
            Scorers::default(),
            &THRESHOLDS,
        );
        assert!(checks.is_empty());
        assert_eq!(method, ScoringMethod::Lexical);
        assert_eq!(summarise(&checks).score, None);
    }

    #[test]
    fn lexical_fallback_without_a_scorer() {
        let checks = run(
            "Either party may terminate with sixty days notice [1].",
            Scorers::default(),
        );
        assert_eq!(checks[0].outcome, ClaimOutcome::Supported);
        let off_topic = run(
            "The supplier must hold liability insurance [1].",
            Scorers::default(),
        );
        assert_eq!(off_topic[0].outcome, ClaimOutcome::Unsupported);
    }

    #[test]
    fn opened_text_of_the_cited_file_counts_as_evidence() {
        let messages = message("Disputes go to arbitration in Zurich under Swiss law [1].");
        let passages = passages();
        let opened = [OpenedText {
            path: "c:/docs/1.pdf".into(),
            text: format!(
                "{} Disputes go to arbitration in Zurich under Swiss law.",
                "Filler text. ".repeat(200)
            ),
        }];
        let (checks, _) = verify_answer(
            &VerifyInput {
                messages: &messages,
                passages: &passages,
                opened: &opened,
            },
            relevance(&Overlap),
            &THRESHOLDS,
        );
        assert_eq!(checks[0].outcome, ClaimOutcome::Supported);
    }

    #[test]
    fn provider_fragments_are_only_checked_for_numbers() {
        let messages = message(
            "Spain won a record fourth title in 2024 [1]. Spain won the final against England in Berlin last summer [1].",
        );
        let passages = vec![Evidence {
            n: 1,
            path: "https://example.org".into(),
            text: "This victory marks Spain's record fourth title.".into(),
            checkable: false,
        }];
        let (checks, _) = verify_answer(
            &VerifyInput {
                messages: &messages,
                passages: &passages,
                opened: &[],
            },
            relevance(&Overlap),
            &THRESHOLDS,
        );
        assert_eq!(
            checks[0].outcome,
            ClaimOutcome::Unsupported,
            "2024 is not in the source"
        );
        assert_eq!(checks[1].outcome, ClaimOutcome::Unchecked);
        assert_eq!(summarise(&checks).score, Some(0.0));
    }

    #[test]
    fn needs_are_covered_by_matching_passages() {
        let needs = vec![
            (
                "1".to_string(),
                "notice for terminating the agreement".to_string(),
            ),
            (
                "2".to_string(),
                "liability insurance requirements".to_string(),
            ),
        ];
        let checks = check_needs(&needs, &passages(), Some(&Overlap), &THRESHOLDS);
        assert_eq!(checks[0].state, CoverageState::Covered);
        assert_eq!(checks[0].passages, vec![1]);
        assert_eq!(checks[1].state, CoverageState::Missing);
        let lexical = check_needs(&needs, &passages(), None, &THRESHOLDS);
        assert_eq!(lexical[0].state, CoverageState::Covered);
        assert_eq!(lexical[1].state, CoverageState::Missing);
    }
}
