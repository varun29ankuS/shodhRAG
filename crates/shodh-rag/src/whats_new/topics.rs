//! Topics a paper alert could follow, suggested from the user's library: computed on this
//! computer, deterministically, with no model call and no request.
//!
//! - **Phrases from titles.** Two or three consecutive words of a library paper's title
//!   (from the paper graph's scans, else a file name that reads as a title), with no
//!   stopword at either end. A phrase counts the titles it occurs in, and needs two.
//!   A phrase inside a longer one found in as many titles is dropped: the longer one
//!   says more.
//! - **Methods.** The paper graph's methods count twice the library papers using them.
//! - Ordered by count, then longer first, then alphabetically; topics already followed
//!   are left out; at most [`MAX_SUGGESTIONS`].

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::harness::web::relevance::{is_stopword, words};

/// Most topics suggested at once.
pub const MAX_SUGGESTIONS: usize = 8;
/// Titles a phrase must occur in to be suggested.
const MIN_TITLES: usize = 2;

/// Where a suggestion came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicSource {
    Titles,
    Method,
}

/// One suggested topic: the exact words an alert on it would send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicSuggestion {
    pub words: String,
    pub from: TopicSource,
    /// Library papers it comes from.
    pub papers: usize,
}

/// What the library offers to suggest from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TopicInputs {
    /// Titles of library papers (one per paper).
    pub titles: Vec<String>,
    /// The paper graph's methods, with the library papers that use each.
    pub methods: Vec<(String, usize)>,
}

/// Lower-cased words separated by single spaces, for comparing topics.
pub fn normalize_topic(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn usable(word: &str) -> bool {
    word.chars().count() >= 2 && !word.chars().all(|c| c.is_ascii_digit())
}

/// The phrases of one title (distinct).
fn phrases_of(title: &str) -> BTreeSet<Vec<String>> {
    let ws = words(title);
    let mut out = BTreeSet::new();
    for n in 2..=3 {
        for window in ws.windows(n) {
            let first = &window[0];
            let last = &window[n - 1];
            if is_stopword(first) || is_stopword(last) || !window.iter().all(|w| usable(w)) {
                continue;
            }
            out.insert(window.to_vec());
        }
    }
    out
}

/// Whether `inner` occurs in `outer` as consecutive words.
fn contains_phrase(outer: &[String], inner: &[String]) -> bool {
    outer.len() > inner.len() && outer.windows(inner.len()).any(|w| w == inner)
}

/// The suggestions for `inputs`, leaving out `existing_alerts` (their words).
pub fn suggest_topics(inputs: &TopicInputs, existing_alerts: &[String]) -> Vec<TopicSuggestion> {
    let mut counts: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    let distinct_titles: BTreeSet<String> =
        inputs.titles.iter().map(|t| normalize_topic(t)).collect();
    for title in &distinct_titles {
        for phrase in phrases_of(title) {
            *counts.entry(phrase).or_default() += 1;
        }
    }
    let frequent: Vec<(Vec<String>, usize)> = counts
        .into_iter()
        .filter(|(_, n)| *n >= MIN_TITLES)
        .collect();
    let mut candidates: Vec<(String, TopicSource, usize, usize)> = frequent
        .iter()
        .filter(|(phrase, n)| {
            !frequent
                .iter()
                .any(|(longer, m)| m == n && contains_phrase(longer, phrase))
        })
        .map(|(phrase, n)| (phrase.join(" "), TopicSource::Titles, *n, *n))
        .collect();
    for (label, papers) in &inputs.methods {
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
        if label.is_empty() || *papers == 0 {
            continue;
        }
        candidates.push((
            label,
            TopicSource::Method,
            papers.saturating_mul(2),
            *papers,
        ));
    }
    let existing: BTreeSet<String> = existing_alerts.iter().map(|a| normalize_topic(a)).collect();
    candidates.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| b.0.split(' ').count().cmp(&a.0.split(' ').count()))
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (words, from, _, papers) in candidates {
        let key = normalize_topic(&words);
        if existing.contains(&key) || !seen.insert(key) {
            continue;
        }
        out.push(TopicSuggestion {
            words,
            from,
            papers,
        });
        if out.len() == MAX_SUGGESTIONS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whats_new::library::title_from_file_name;

    /// A small library: titles from scans, PDFs whose names are titles, PDFs whose names
    /// are ids or junk, and the graph's methods.
    fn fixture() -> TopicInputs {
        let library: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/topic-library.json")).unwrap();
        let mut titles: Vec<String> = library["scanTitles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().to_string())
            .collect();
        titles.extend(
            library["fileNames"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|f| title_from_file_name(f.as_str().unwrap())),
        );
        let methods = library["methods"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m[0].as_str().unwrap().to_string(),
                    m[1].as_u64().unwrap() as usize,
                )
            })
            .collect();
        TopicInputs { titles, methods }
    }

    #[test]
    fn suggestions_from_a_fixture_library_rank_by_shared_titles() {
        let got: Vec<(String, TopicSource, usize)> = suggest_topics(&fixture(), &[])
            .into_iter()
            .map(|s| (s.words, s.from, s.papers))
            .collect();
        assert_eq!(
            got,
            vec![
                // 4 titles; DeltaNet scores 4 too (2 papers), and the longer comes first.
                ("linear attention".to_string(), TopicSource::Titles, 4),
                ("DeltaNet".to_string(), TopicSource::Method, 2),
                ("kv cache".to_string(), TopicSource::Titles, 3),
                // "state space" and "space models" are inside it, in as many titles.
                ("state space models".to_string(), TopicSource::Titles, 2),
                // One from a scan, one from a file name that reads as a title.
                ("vector search".to_string(), TopicSource::Titles, 2),
                ("Mamba".to_string(), TopicSource::Method, 1),
            ]
        );
    }

    #[test]
    fn junk_file_names_add_nothing() {
        let all = suggest_topics(&fixture(), &[]);
        for s in &all {
            for junk in [
                "download", "final", "2301", "10 1145", "paper", "untitled", "3f9a",
            ] {
                assert!(!s.words.contains(junk), "{} came from a junk name", s.words);
            }
        }
    }

    #[test]
    fn a_phrase_inside_a_longer_one_with_the_same_count_is_dropped() {
        let inputs = TopicInputs {
            titles: vec![
                "Selective state space models for audio".into(),
                "Structured state space models at scale".into(),
            ],
            methods: vec![],
        };
        let got: Vec<String> = suggest_topics(&inputs, &[])
            .into_iter()
            .map(|s| s.words)
            .collect();
        assert_eq!(got, ["state space models"]);
    }

    #[test]
    fn methods_rank_above_single_title_phrases() {
        let inputs = TopicInputs {
            titles: vec![
                "Kernel methods for graphs".into(),
                "Kernel methods at scale".into(),
            ],
            methods: vec![("RWKV".into(), 2)],
        };
        let got: Vec<String> = suggest_topics(&inputs, &[])
            .into_iter()
            .map(|s| s.words)
            .collect();
        assert_eq!(got, ["RWKV", "kernel methods"]);
    }

    #[test]
    fn existing_alerts_are_not_suggested() {
        let got: Vec<String> = suggest_topics(
            &fixture(),
            &["Linear   Attention".into(), "deltanet".into()],
        )
        .into_iter()
        .map(|s| s.words)
        .collect();
        assert!(
            !got.iter()
                .any(|w| w == "linear attention" || w == "DeltaNet"),
            "{got:?}"
        );
        assert!(got.contains(&"kv cache".to_string()));
    }

    #[test]
    fn an_empty_library_suggests_nothing() {
        assert!(suggest_topics(&TopicInputs::default(), &[]).is_empty());
        let one = TopicInputs {
            titles: vec!["Linear attention transformers".into()],
            methods: vec![],
        };
        assert!(
            suggest_topics(&one, &[]).is_empty(),
            "a phrase needs two titles"
        );
    }

    #[test]
    fn suggestions_are_deterministic() {
        let inputs = fixture();
        let mut shuffled = inputs.clone();
        shuffled.titles.reverse();
        shuffled.methods.reverse();
        assert_eq!(suggest_topics(&inputs, &[]), suggest_topics(&shuffled, &[]));
    }

    #[test]
    fn at_most_eight_are_suggested() {
        let methods = (0..20).map(|i| (format!("Method {i:02}"), 1)).collect();
        let inputs = TopicInputs {
            titles: vec![],
            methods,
        };
        assert_eq!(suggest_topics(&inputs, &[]).len(), MAX_SUGGESTIONS);
    }
}
