//! Which of the memories put in front of a question the answer used.
//!
//! A memory is reinforced (stronger, linked to the others used with it, audited as
//! `memory_use`) only when the answer used it, not whenever recall put it in the prompt:
//! otherwise a memory that matches many questions grows stronger whether or not it helps,
//! and the strong get stronger. The judgement is local (no model call), from two signals
//! measured inside one short window of the answer (one sentence, or two in a row):
//!
//! ```text
//! similarity(m, w) = cos(E(m), E(w))
//! overlap(m, w)    = share of m's content words that appear in w
//! score(m)         = max over windows w of (similarity(m, w) + overlap(m, w)) / 2
//! used             = score ≥ DEFAULT_USE_CUTOFF
//! ```
//!
//! Both signals are taken from the same window, so a long answer gains nothing from its
//! length: words scattered over many paragraphs do not add up, and a memory counts as used
//! only where one or two sentences rely on it. (A longer answer has more windows to take
//! the maximum over; the fixture's long answers measure what that costs.)
//!
//! Similarity alone does not separate use from topic: an answer about the same subject
//! scores almost as high as one that relies on the memory (see [`DEFAULT_USE_CUTOFF`]).
//! An answer that uses a memory usually repeats its specifics (a name, a date, a value),
//! which the word overlap sees; one that paraphrases it is caught by the similarity. The
//! two are weighted equally (not tuned), so only the cutoff is fitted.
//!
//! Both sides are embedded as queries: this is a comparison of two statements, not a
//! search of passages, and E5 asks for the same prefix on both sides of a symmetric task.

use std::collections::HashSet;

use crate::embeddings::EmbeddingModel;

/// Score at which an answer counts as using a memory (see the module docs). Measured with
/// multilingual-e5-base on `fixtures/memory_use.json`
/// (`the_use_cutoff_separates_used_from_unused_memories_with_e5`): 66 hand-labelled
/// memory–answer pairs, each memory once used (often paraphrased) and once in an answer on
/// the same topic that does not use it; 18 of them have long answers (166–187 words) that
/// use the memory in one sentence or not at all.
///
/// Chosen on the 42 tuning pairs only, as the best of a sweep in steps of 0.005: 40 of 42
/// right (95.2%); every cutoff from 0.590 to 0.625 is within one pair of that. Tuning
/// scores: used 0.406–0.942 (median 0.792), not used 0.357–0.633 (median 0.502).
///
/// The 24 held-out pairs (8 long), never used to choose it: 19 of 24 right (79.2%); used
/// 0.586–0.932 (median 0.858), not used 0.377–0.663 (median 0.579). Four of the five
/// misses are on-topic answers that do not use the memory but score above the cutoff
/// (two of them long), so the cutoff errs towards reinforcing a memory that did not help.
/// On all 18 long pairs: 16 right (88.9%). The test requires at least 75% held out (one
/// pair below the measurement). The fixture is small and written by hand: re-measure when
/// the embedding model or the scoring changes.
pub const DEFAULT_USE_CUTOFF: f64 = 0.635;

/// Words that say nothing about which memory an answer drew on, left out of
/// [`content_overlap`]: function words, and the labels facts are rendered with
/// ("Preference: preference holder the user; preference topic ...").
const NON_CONTENT_WORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "and",
    "or",
    "but",
    "of",
    "to",
    "in",
    "on",
    "at",
    "for",
    "with",
    "by",
    "from",
    "as",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "am",
    "i",
    "im",
    "i'm",
    "my",
    "me",
    "we",
    "our",
    "us",
    "you",
    "your",
    "it",
    "its",
    "this",
    "that",
    "these",
    "those",
    "there",
    "here",
    "so",
    "if",
    "then",
    "than",
    "not",
    "no",
    "do",
    "does",
    "did",
    "done",
    "have",
    "has",
    "had",
    "will",
    "would",
    "can",
    "could",
    "should",
    "about",
    "into",
    "over",
    "after",
    "before",
    "up",
    "down",
    "out",
    "off",
    "just",
    "also",
    "very",
    "more",
    "most",
    "some",
    "any",
    "all",
    "each",
    "every",
    "per",
    "via",
    "preference",
    "holder",
    "user",
    "topic",
    "value",
];

/// Characters compared of each word for a loose match ("prefer" and "preferred",
/// "deadline" and "deadlines").
const STEM_CHARS: usize = 5;

/// Most sentences of an answer compared (the first ones): bounds the work at the end of a
/// long answer. Memories are used where an answer starts to address the question.
pub const MAX_ANSWER_SENTENCES: usize = 48;

/// Sentences shorter than this (in characters, after cleaning) are not compared: list
/// markers, headings of one word and stray citation numbers say nothing about a memory.
pub const MIN_SENTENCE_CHARS: usize = 12;

/// Longest sentence compared, in characters (the embedder truncates longer input anyway).
pub const MAX_SENTENCE_CHARS: usize = 600;

/// Most consecutive sentences in one window: a memory is often used across a sentence and
/// the one that follows it ("Your accountant is Priya. Send her the invoices.").
pub const MAX_WINDOW_SENTENCES: usize = 2;

/// How one memory scored against the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct UseJudgement {
    /// The memory.
    pub id: String,
    /// Cosine similarity with the best window, in `[-1, 1]`.
    pub similarity: f64,
    /// Share of the memory's content words found in the best window, in `[0, 1]`
    /// ([`content_overlap`]).
    pub overlap: f64,
    /// `(similarity + overlap) / 2` of the best window.
    pub score: f64,
    /// `score` reached the cutoff.
    pub used: bool,
}

/// The content words of `text`, lower-cased: runs of letters, digits, `_`, `:` and `.`
/// (so "8:15" and "3.5" stay whole) with leading and trailing `:` and `.` removed, at least
/// two characters long and not in [`NON_CONTENT_WORDS`].
pub fn content_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':' || c == '.'))
        .map(|word| word.trim_matches(|c: char| c == ':' || c == '.'))
        .filter(|word| word.chars().count() >= 2 && !NON_CONTENT_WORDS.contains(word))
        .map(str::to_string)
        .collect()
}

fn stem(word: &str) -> String {
    word.chars().take(STEM_CHARS).collect()
}

/// Share of `memory`'s distinct content words ([`content_words`]) that `answer` contains,
/// as the same word or one with the same first [`STEM_CHARS`] characters. 0 for a memory
/// without content words.
pub fn content_overlap(memory: &str, answer: &str) -> f64 {
    let memory: HashSet<String> = content_words(memory).into_iter().collect();
    if memory.is_empty() {
        return 0.0;
    }
    let answer: HashSet<String> = content_words(answer).into_iter().collect();
    let stems: HashSet<String> = answer.iter().map(|w| stem(w)).collect();
    let found = memory
        .iter()
        .filter(|w| answer.contains(*w) || stems.contains(&stem(w)))
        .count();
    found as f64 / memory.len() as f64
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

/// Scores each of `memories` (`(id, text)`) against the windows of `answer` with
/// `embedder` (see the module docs) and judges it used at `cutoff` (normally [`DEFAULT_USE_CUTOFF`]). Blocking
/// (it runs the embedder); call it off the async executor. An answer with no sentence to
/// compare uses nothing.
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
                similarity: 0.0,
                overlap: 0.0,
                score: 0.0,
                used: false,
            })
            .collect());
    }
    let windows = answer_windows(&sentences);
    let window_vectors = windows
        .iter()
        .map(|w| embedder.embed_query(w))
        .collect::<anyhow::Result<Vec<_>>>()?;
    memories
        .iter()
        .map(|(id, text)| {
            let memory = embedder.embed_query(&text.replace(['\n', '\r'], " "))?;
            let (similarity, overlap) = windows
                .iter()
                .zip(&window_vectors)
                .map(|(window, vector)| (cosine(&memory, vector), content_overlap(text, window)))
                .max_by(|a, b| (a.0 + a.1).total_cmp(&(b.0 + b.1)))
                .unwrap_or((0.0, 0.0));
            let score = (similarity + overlap) / 2.0;
            Ok(UseJudgement {
                id: id.clone(),
                similarity,
                overlap,
                score,
                used: score >= cutoff,
            })
        })
        .collect()
}

/// The windows of `sentences` a memory is compared with: each sentence, and each run of
/// up to [`MAX_WINDOW_SENTENCES`] consecutive sentences, joined by a space.
pub fn answer_windows(sentences: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for start in 0..sentences.len() {
        for len in 1..=MAX_WINDOW_SENTENCES {
            if start + len > sentences.len() {
                break;
            }
            out.push(sentences[start..start + len].join(" "));
        }
    }
    out
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
        /// `tuning` (the cutoff is chosen on these) or `held_out` (only checks it).
        split: String,
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

    /// Answers of at least this many words count as long.
    const LONG_ANSWER_WORDS: usize = 150;

    fn is_long(p: &Pair) -> bool {
        p.answer.split_whitespace().count() >= LONG_ANSWER_WORDS
    }

    #[test]
    fn the_fixture_is_balanced_split_and_has_long_answers() {
        let pairs = pairs();
        assert!(pairs.len() >= 40, "{} pairs", pairs.len());
        let used = pairs.iter().filter(|p| p.used).count();
        assert_eq!(used * 2, pairs.len(), "half used, half not");
        let long: Vec<&Pair> = pairs.iter().filter(|p| is_long(p)).collect();
        assert!(long.len() >= 16, "{} long answers", long.len());
        assert_eq!(long.iter().filter(|p| p.used).count() * 2, long.len());
        assert!(long
            .iter()
            .all(|p| p.answer.split_whitespace().count() <= 400));
        let held_out: Vec<&Pair> = pairs.iter().filter(|p| p.split == "held_out").collect();
        assert!(
            held_out.len() * 3 >= pairs.len(),
            "held out: {}",
            held_out.len()
        );
        assert!(held_out.iter().any(|p| is_long(p)));
        assert!(pairs
            .iter()
            .all(|p| p.split == "held_out" || p.split == "tuning"));
        // Each memory once used and once not, both in the same split.
        let mut by_memory: std::collections::BTreeMap<&str, (usize, usize, &str)> =
            std::collections::BTreeMap::new();
        for p in &pairs {
            assert!(!answer_sentences(&p.answer).is_empty(), "{}", p.answer);
            let entry = by_memory
                .entry(p.memory.as_str())
                .or_insert((0, 0, p.split.as_str()));
            assert_eq!(entry.2, p.split, "{}", p.memory);
            if p.used {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }
        for (memory, (used, unused, _)) in by_memory {
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

    fn describe(name: &str, values: &[(f64, bool)]) {
        let used = spread(values.iter().filter(|s| s.1).map(|s| s.0).collect());
        let unused = spread(values.iter().filter(|s| !s.1).map(|s| s.0).collect());
        eprintln!(
            "{name}: used min {:.4} median {:.4} max {:.4}; unused min {:.4} median {:.4} max {:.4}",
            used.0, used.1, used.2, unused.0, unused.1, unused.2
        );
    }

    /// The shipped cutoff against the real E5 model on the labelled fixture. The cutoff is
    /// chosen on the `tuning` pairs only: it must lie on their plateau (within one pair of
    /// the best tuning accuracy, cutoffs swept in steps of 0.005). The `held_out` pairs,
    /// never used to choose it, must be classified right at least [`HELD_OUT_FLOOR`] of
    /// the time. Run with `--nocapture` for every pair's score and the sweep.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing multilingual-e5-base/"
    )]
    fn the_use_cutoff_separates_used_from_unused_memories_with_e5() {
        /// Share of the held-out pairs the shipped cutoff must classify right.
        const HELD_OUT_FLOOR: f64 = 0.75;
        let root = std::env::var_os("SHODH_TEST_MODELS")
            .expect("SHODH_TEST_MODELS must point at the models directory to run this test");
        let config = crate::embeddings::e5::E5Config::auto_detect(std::path::Path::new(&root))
            .expect("multilingual-e5-base under SHODH_TEST_MODELS")
            .with_compute(crate::compute::provider::ComputePreference::Cpu);
        let e5 = crate::embeddings::e5::E5Embeddings::new(config).expect("load E5");
        let (mut tuning, mut held_out, mut long, mut short) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for p in pairs() {
            let judged = judge_use(
                &e5,
                &[("m".to_string(), p.memory.clone())],
                &p.answer,
                DEFAULT_USE_CUTOFF,
            )
            .expect("embed");
            let j = &judged[0];
            eprintln!(
                "{:>8} {:>6} {:>5} score {:.4} similarity {:.4} overlap {:.2} {:45.45}",
                p.split,
                if p.used { "used" } else { "unused" },
                if is_long(&p) { "long" } else { "short" },
                j.score,
                j.similarity,
                j.overlap,
                p.memory,
            );
            let point = (j.score, p.used);
            if p.split == "tuning" {
                tuning.push(point);
            } else {
                held_out.push(point);
            }
            if is_long(&p) {
                long.push(point);
            } else {
                short.push(point);
            }
        }
        describe("tuning", &tuning);
        describe("held out", &held_out);
        describe("long", &long);
        describe("short", &short);
        let sweep: Vec<(f64, f64)> = (0..=140)
            .map(|i| {
                let cutoff = 0.30 + 0.005 * f64::from(i);
                (cutoff, accuracy(&tuning, cutoff))
            })
            .collect();
        let best = sweep.iter().map(|s| s.1).fold(0.0, f64::max);
        let one_pair = 1.0 / tuning.len() as f64;
        for (cutoff, acc) in &sweep {
            eprintln!(
                "cutoff {cutoff:.3} tuning {acc:.3} held out {:.3} long {:.3}",
                accuracy(&held_out, *cutoff),
                accuracy(&long, *cutoff)
            );
        }
        let plateau: Vec<f64> = sweep
            .iter()
            .filter(|(_, acc)| *acc >= best - one_pair - 1e-9)
            .map(|(cutoff, _)| *cutoff)
            .collect();
        let shipped_tuning = accuracy(&tuning, DEFAULT_USE_CUTOFF);
        let shipped_held_out = accuracy(&held_out, DEFAULT_USE_CUTOFF);
        eprintln!(
            "best tuning {best:.3}; plateau {:.3}-{:.3}; shipped {DEFAULT_USE_CUTOFF:.3}: tuning {shipped_tuning:.3}, held out {shipped_held_out:.3}, long {:.3}",
            plateau.first().copied().unwrap_or(f64::NAN),
            plateau.last().copied().unwrap_or(f64::NAN),
            accuracy(&long, DEFAULT_USE_CUTOFF),
        );
        assert!(
            shipped_tuning >= best - one_pair - 1e-9,
            "the shipped cutoff is off the tuning plateau: {shipped_tuning:.3} vs best {best:.3}"
        );
        assert!(
            shipped_held_out >= HELD_OUT_FLOOR,
            "held-out accuracy {shipped_held_out:.3} below {HELD_OUT_FLOOR}"
        );
    }

    #[test]
    fn overlap_counts_the_memorys_specifics_not_its_labels_or_function_words() {
        let memory = "Preference: preference holder the user; preference topic report format; preference value PDF";
        assert_eq!(content_words(memory), vec!["report", "format", "pdf"]);
        assert_eq!(
            content_words("I take the 8:15 train."),
            vec!["take", "8:15", "train"]
        );
        let answer = "I exported the reports as a PDF.";
        // "report" matches "reports" by its first five characters; "format" is missing.
        assert!((content_overlap(memory, answer) - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!(content_overlap("I am", answer), 0.0);
        assert_eq!(content_overlap(memory, ""), 0.0);
    }
}
