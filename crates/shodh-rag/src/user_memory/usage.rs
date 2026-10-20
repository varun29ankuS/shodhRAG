//! Which of the memories put in front of a question the answer used.
//!
//! A memory is reinforced (stronger, linked to the others used with it, audited as
//! `memory_use`) only when the answer used it, not whenever recall put it in the prompt:
//! otherwise a memory that matches many questions grows stronger whether or not it helps,
//! and the strong get stronger. The judgement is local (no model call): the memory's text
//! and each sentence of the answer are embedded with the search embedder, and a memory
//! counts as used when its best-matching sentence reaches [`DEFAULT_USE_SIMILARITY`].
//!
//! ```text
//! score(memory) = max over answer sentences s of cos(E(memory), E(s))
//! used          = score ≥ DEFAULT_USE_SIMILARITY
//! ```
//!
//! Both sides are embedded as queries: this is a comparison of two statements, not a
//! search of passages, and E5 asks for the same prefix on both sides of a symmetric task.

use crate::embeddings::EmbeddingModel;

/// Cosine similarity at which an answer sentence counts as using a memory. Measured with
/// multilingual-e5-base on `fixtures/memory_use.json` (48 hand-labelled memory–answer
/// pairs, half used, half on the same topic but not used; see
/// `the_use_cutoff_separates_used_from_unused_memories_with_e5`).
pub const DEFAULT_USE_SIMILARITY: f64 = 0.85;

/// Most sentences of an answer compared (the first ones): bounds the work at the end of a
/// long answer. Memories are used where an answer starts to address the question.
pub const MAX_ANSWER_SENTENCES: usize = 48;

/// Sentences shorter than this (in characters, after cleaning) are not compared: list
/// markers, headings of one word and stray citation numbers say nothing about a memory.
pub const MIN_SENTENCE_CHARS: usize = 12;

/// Longest sentence compared, in characters (the embedder truncates longer input anyway).
pub const MAX_SENTENCE_CHARS: usize = 600;

/// How one memory scored against the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct UseJudgement {
    /// The memory.
    pub id: String,
    /// Best cosine similarity with any answer sentence, in `[-1, 1]`.
    pub score: f64,
    /// `score` reached the cutoff.
    pub used: bool,
}

/// The sentences of `answer` that are compared with memories: Markdown markers, citation
/// numbers (`[3]`) and code blocks removed, split at line breaks and sentence ends, short
/// fragments dropped, at most [`MAX_ANSWER_SENTENCES`].
pub fn answer_sentences(answer: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_code = false;
    for line in answer.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        for sentence in split_sentences(&clean_line(trimmed)) {
            if out.len() == MAX_ANSWER_SENTENCES {
                return out;
            }
            let sentence = sentence.trim();
            if sentence.chars().count() < MIN_SENTENCE_CHARS {
                continue;
            }
            out.push(sentence.chars().take(MAX_SENTENCE_CHARS).collect());
        }
    }
    out
}

/// A line without Markdown markers (headings, quotes, list bullets, emphasis, table bars)
/// and citation numbers.
fn clean_line(line: &str) -> String {
    let line = line.trim_start_matches(['#', '>', ' ']);
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
        .unwrap_or(line);
    let line = match line.split_once(". ") {
        Some((number, rest))
            if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) =>
        {
            rest
        }
        _ => line,
    };
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '_' | '`' => {}
            '|' => out.push(' '),
            '[' => {
                // `[3]`, `[3, 4]`: citation numbers are dropped; other brackets stay.
                let rest: String = chars.clone().take_while(|c| *c != ']').collect();
                let is_citation = !rest.is_empty()
                    && rest.len() <= 24
                    && rest
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == ',' || c == ' ' || c == '-');
                if is_citation && chars.clone().nth(rest.chars().count()) == Some(']') {
                    for _ in 0..=rest.chars().count() {
                        chars.next();
                    }
                } else {
                    out.push(c);
                }
            }
            other => out.push(other),
        }
    }
    let mut joined = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // A citation dropped before punctuation leaves a space in front of it.
    for mark in [",", ".", ";", ":", "!", "?"] {
        joined = joined.replace(&format!(" {mark}"), mark);
    }
    joined
}

/// `text` split after `.`, `!`, `?` (and `;`) followed by a space. Decimal numbers and
/// abbreviations without a following space stay whole.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        if matches!(c, '.' | '!' | '?' | ';') && chars.peek().is_some_and(|n| n.is_whitespace()) {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// Cosine similarity of two vectors (0 when either is zero or their sizes differ).
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Scores each of `memories` (`(id, text)`) against `answer` with `embedder` and judges it
/// used at `cutoff` (normally [`DEFAULT_USE_SIMILARITY`]). Blocking (it runs the
/// embedder); call it off the async executor. An answer with no sentence to compare uses
/// nothing.
pub fn judge_use(
    embedder: &dyn EmbeddingModel,
    memories: &[(String, String)],
    answer: &str,
    cutoff: f64,
) -> anyhow::Result<Vec<UseJudgement>> {
    let sentences = answer_sentences(answer);
    if sentences.is_empty() || memories.is_empty() {
        return Ok(memories
            .iter()
            .map(|(id, _)| UseJudgement {
                id: id.clone(),
                score: 0.0,
                used: false,
            })
            .collect());
    }
    let sentence_vectors = sentences
        .iter()
        .map(|s| embedder.embed_query(s))
        .collect::<anyhow::Result<Vec<_>>>()?;
    memories
        .iter()
        .map(|(id, text)| {
            let memory = embedder.embed_query(&text.replace(['\n', '\r'], " "))?;
            let score = sentence_vectors
                .iter()
                .map(|s| cosine(&memory, s))
                .fold(f64::MIN, f64::max);
            Ok(UseJudgement {
                id: id.clone(),
                score,
                used: score >= cutoff,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_drop_markup_citations_code_and_fragments() {
        let answer = "## Plan\n\nYou prefer reports as PDF [2], so I exported it. It is 3.5 MB!\n\
                      - Send it to Priya Sharma by Friday [1, 3]\n\
                      1. Ask whether [the draft] is final?\n\
                      ```\nlet x = 1; // not prose\n```\n\
                      | Owner | Due date |\nOk.";
        let sentences = answer_sentences(answer);
        assert_eq!(
            sentences,
            vec![
                "You prefer reports as PDF, so I exported it.",
                "It is 3.5 MB!",
                "Send it to Priya Sharma by Friday",
                "Ask whether [the draft] is final?",
                "Owner Due date",
            ]
        );
    }

    #[test]
    fn long_answers_compare_a_bounded_number_of_sentences() {
        let answer = (0..200)
            .map(|i| format!("Sentence number {i} of the answer."))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(answer_sentences(&answer).len(), MAX_ANSWER_SENTENCES);
        let long = "word ".repeat(1_000);
        assert_eq!(
            answer_sentences(&long)[0].chars().count(),
            MAX_SENTENCE_CHARS
        );
    }

    #[test]
    fn cosine_is_scale_free_and_zero_for_degenerate_vectors() {
        assert!((cosine(&[1.0, 0.0], &[3.0, 0.0]) - 1.0).abs() < 1e-12);
        assert!(cosine(&[1.0, 0.0], &[0.0, 2.0]).abs() < 1e-12);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn a_memory_is_used_when_one_sentence_restates_it() {
        let embedder = crate::statements::testing::WordEmbedder::default();
        let memories = vec![
            ("pdf".to_string(), "reports as PDF".to_string()),
            ("gym".to_string(), "gym on Tuesdays".to_string()),
        ];
        let answer = "The summary is ready. I saved the reports as PDF for you.";
        let judged = judge_use(&embedder, &memories, answer, 0.6).unwrap();
        assert_eq!(judged.len(), 2);
        assert!(judged[0].used, "{judged:?}");
        assert!(!judged[1].used, "{judged:?}");
        assert!(judged[0].score > judged[1].score);
        // Nothing to compare: nothing used.
        let empty = judge_use(&embedder, &memories, "```\ncode\n```", 0.6).unwrap();
        assert!(empty.iter().all(|j| !j.used && j.score == 0.0));
    }

    /// The labelled pairs the cutoff is measured on.
    const FIXTURE: &str = include_str!("fixtures/memory_use.json");

    #[derive(serde::Deserialize)]
    struct Pair {
        memory: String,
        answer: String,
        used: bool,
    }

    #[derive(serde::Deserialize)]
    struct Fixture {
        pairs: Vec<Pair>,
    }

    fn pairs() -> Vec<Pair> {
        serde_json::from_str::<Fixture>(FIXTURE)
            .expect("memory_use.json is valid")
            .pairs
    }

    #[test]
    fn the_fixture_is_balanced_and_each_memory_is_both_used_and_not() {
        let pairs = pairs();
        assert!(pairs.len() >= 40, "{} pairs", pairs.len());
        let used = pairs.iter().filter(|p| p.used).count();
        assert_eq!(used * 2, pairs.len(), "half used, half not");
        let mut by_memory: std::collections::BTreeMap<&str, (usize, usize)> =
            std::collections::BTreeMap::new();
        for p in &pairs {
            assert!(!answer_sentences(&p.answer).is_empty(), "{}", p.answer);
            let entry = by_memory.entry(p.memory.as_str()).or_default();
            if p.used {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }
        for (memory, (used, unused)) in by_memory {
            assert_eq!((used, unused), (1, 1), "{memory}");
        }
    }

    /// Share of `scores` (`(score, used)`) classified right at `cutoff`.
    fn accuracy(scores: &[(f64, bool)], cutoff: f64) -> f64 {
        let right = scores
            .iter()
            .filter(|(score, used)| (*score >= cutoff) == *used)
            .count();
        right as f64 / scores.len() as f64
    }

    /// Smallest, median and largest of `values`.
    fn spread(mut values: Vec<f64>) -> (f64, f64, f64) {
        values.sort_by(f64::total_cmp);
        (
            values[0],
            values[values.len() / 2],
            values[values.len() - 1],
        )
    }

    /// The shipped cutoff against the real E5 model on the labelled fixture: it must be
    /// one of the cutoffs with the best accuracy (swept in steps of 0.005), and classify
    /// at least [`REQUIRED_ACCURACY`] of the pairs right. Run with `--nocapture` for the
    /// scores of every pair and the sweep.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing multilingual-e5-base/"
    )]
    fn the_use_cutoff_separates_used_from_unused_memories_with_e5() {
        /// Share of the fixture the shipped cutoff must classify right.
        const REQUIRED_ACCURACY: f64 = 0.85;
        let root = std::env::var_os("SHODH_TEST_MODELS")
            .expect("SHODH_TEST_MODELS must point at the models directory to run this test");
        let config = crate::embeddings::e5::E5Config::auto_detect(std::path::Path::new(&root))
            .expect("multilingual-e5-base under SHODH_TEST_MODELS");
        let e5 = crate::embeddings::e5::E5Embeddings::new(config).expect("load E5");
        let mut scores = Vec::new();
        for p in pairs() {
            let judged = judge_use(
                &e5,
                &[("m".to_string(), p.memory.clone())],
                &p.answer,
                DEFAULT_USE_SIMILARITY,
            )
            .expect("embed");
            let score = judged[0].score;
            eprintln!(
                "{:>6} {score:.4} {:60.60} | {:.60}",
                if p.used { "used" } else { "unused" },
                p.memory,
                p.answer
            );
            scores.push((score, p.used));
        }
        let used = spread(scores.iter().filter(|s| s.1).map(|s| s.0).collect());
        let unused = spread(scores.iter().filter(|s| !s.1).map(|s| s.0).collect());
        eprintln!(
            "used   min {:.4} median {:.4} max {:.4}",
            used.0, used.1, used.2
        );
        eprintln!(
            "unused min {:.4} median {:.4} max {:.4}",
            unused.0, unused.1, unused.2
        );
        let sweep: Vec<(f64, f64)> = (0..=100)
            .map(|i| {
                let cutoff = 0.70 + 0.005 * f64::from(i);
                (cutoff, accuracy(&scores, cutoff))
            })
            .collect();
        let best = sweep.iter().map(|s| s.1).fold(0.0, f64::max);
        for (cutoff, acc) in &sweep {
            eprintln!("cutoff {cutoff:.3} accuracy {acc:.3}");
        }
        let shipped = accuracy(&scores, DEFAULT_USE_SIMILARITY);
        eprintln!("shipped {DEFAULT_USE_SIMILARITY:.3} accuracy {shipped:.3}, best {best:.3}");
        assert!(
            (shipped - best).abs() < 1e-9,
            "the shipped cutoff is not one of the best: {shipped:.3} vs {best:.3}"
        );
        assert!(
            shipped >= REQUIRED_ACCURACY,
            "accuracy {shipped:.3} below {REQUIRED_ACCURACY}"
        );
    }
}
