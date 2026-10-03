//! Relevance ranking for results fetched from the web (papers and web
//! search sources).
//!
//! Search APIs return whatever matched some of the query's words, and the
//! model cites what it is given, so every result is scored against the
//! query before the model sees it and weak matches are dropped:
//!
//! * With the local cross-encoder (the ms-marco MiniLM reranker used for
//!   document search) each `(query, text)` pair gets a logit; its sigmoid is
//!   the content score.
//! * Without it (first run, before the models are installed) a lexical
//!   score is used instead: IDF-weighted coverage of the query's significant
//!   terms by the result text (BM25's term weighting without length
//!   normalisation, which matters little for title + abstract snippets).
//!   Lexical scores are blunter, so their cutoff is stricter.
//!
//! The threshold applies to the content score only. A source's own rank
//! orders results but never rescues one below the threshold: an API's
//! second hit is often unrelated to the query.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use url::Url;

use crate::reranking::CrossEncoderReranker;

/// Scores passages against a query. Implemented by the cross-encoder; tests
/// replay recorded scores.
pub trait PassageScorer: Send + Sync {
    /// One raw relevance logit per passage, in the passages' order.
    fn score(&self, query: &str, passages: &[String]) -> Result<Vec<f32>, String>;
}

impl PassageScorer for CrossEncoderReranker {
    fn score(&self, query: &str, passages: &[String]) -> Result<Vec<f32>, String> {
        let candidates: Vec<(String, String)> = passages
            .iter()
            .enumerate()
            .map(|(i, p)| (i.to_string(), p.clone()))
            .collect();
        let scored = self
            .rerank_batch(query, &candidates, candidates.len())
            .map_err(|e| e.to_string())?;
        // A passage the tokenizer rejected gets no score; it counts as
        // irrelevant rather than failing the whole ranking.
        let mut out = vec![f32::NEG_INFINITY; passages.len()];
        for (id, score) in scored {
            if let Some(slot) = id.parse::<usize>().ok().and_then(|i| out.get_mut(i)) {
                *slot = score;
            }
        }
        Ok(out)
    }
}

/// How results were scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankMethod {
    CrossEncoder,
    Lexical,
}

impl RankMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::CrossEncoder => "cross-encoder",
            Self::Lexical => "keyword overlap",
        }
    }
}

/// Minimum content scores for a result to be kept; `None` keeps every
/// result scored that way (ordering only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// Minimum sigmoid of the cross-encoder logit.
    pub cross_encoder: Option<f32>,
    /// Minimum lexical coverage (0..=1).
    pub lexical: Option<f32>,
}

/// Paper cutoffs, calibrated on the recorded answers in
/// `harness/fixtures/papers/` (`papers_calibration.rs`; the table is printed
/// by its `record_cross_encoder_logits` test). Cross-encoder logits there:
/// * the named paper of a title query scores 9.4; papers on its topic
///   (biologically plausible credit assignment) 0.2 to 2.5; everything
///   else -6.0 or lower, the unrelated OpenAlex hits about -11;
/// * for "linear attention delta rule" every linear-attention paper scores
///   1.2 or more, while delta-rule papers from other fields (Bayesian
///   change-point inference, belief updating, robot memory) score -0.9 to
///   -2.9;
/// * for the vague "credit assignment" every hit scores 0.8 or more.
///
/// A cut at sigmoid 0.38 (logit -0.49) keeps all of the first groups and
/// drops all of the second, with margins of 0.7 and 0.4 logits. Lexical
/// coverage separates far less (the wrong-field delta-rule papers cover
/// 0.54 to 0.73 of "linear attention delta rule", two relevant ones only
/// 0.46), so without the model the cut is stricter, at 0.5: it removes the
/// clear junk (the unrelated title-query hits cover 0.36 at most) and favours
/// precision over recall.
pub const PAPER_THRESHOLDS: Thresholds = Thresholds {
    cross_encoder: Some(0.38),
    lexical: Some(0.5),
};

/// Cutoff for web pages returned by a search engine as such (SearXNG): a
/// title and a page description. Not calibrated on recorded engine
/// answers (no provider was configured to record them); inferred from the
/// paper calibration, where off-topic items score logits of -2.9 and
/// below, and set lower than for papers because a page description says less
/// than an abstract: sigmoid 0.1 (logit -2.2). Without the cross-encoder the
/// engine's own order is kept: a keyword cut on short descriptions drops
/// good pages.
pub const WEB_PAGE_THRESHOLDS: Thresholds = Thresholds {
    cross_encoder: Some(0.1),
    lexical: None,
};

/// Grounded answers (OpenRouter's web plugin, Gemini with Google Search)
/// are not cut, only ordered. Their sources were chosen by the provider to
/// support an answer to this query, and their snippets are often fragments
/// of that answer ("This victory marks Spain's record fourth title.") or a
/// bare domain, which the cross-encoder cannot judge out of context: on the
/// recorded Gemini answer such a supporting source scores logit -4.9.
pub const GROUNDED_THRESHOLDS: Thresholds = Thresholds {
    cross_encoder: None,
    lexical: None,
};

/// One kept item with its scores.
#[derive(Debug, Clone)]
pub struct Scored<T> {
    pub item: T,
    /// Content relevance in 0..=1 (sigmoid logit or lexical coverage).
    pub relevance: f32,
    /// Exact or near-exact title match (always kept, listed first).
    pub title_match: bool,
}

/// The ranking result.
#[derive(Debug, Clone)]
pub struct Ranking<T> {
    pub kept: Vec<Scored<T>>,
    /// How many results scored below the threshold.
    pub dropped: usize,
    pub method: RankMethod,
}

/// One candidate for [`rank`].
#[derive(Debug, Clone)]
pub struct Candidate<T> {
    pub item: T,
    /// Text the query is scored against (title + abstract or snippet).
    pub text: String,
    /// The sources' own opinion in 0..=1 (from their rank); orders results,
    /// never rescues them.
    pub prior: f32,
    pub title_match: bool,
}

/// Words that carry no topic: English function words, query boilerplate
/// ("paper about …") and the arXiv boolean operators.
const STOPWORDS: &[&str] = &[
    "a",
    "about",
    "above",
    "after",
    "again",
    "against",
    "all",
    "also",
    "am",
    "an",
    "and",
    "andnot",
    "any",
    "are",
    "article",
    "articles",
    "as",
    "at",
    "be",
    "because",
    "been",
    "before",
    "being",
    "below",
    "between",
    "both",
    "but",
    "by",
    "can",
    "could",
    "did",
    "do",
    "does",
    "doing",
    "down",
    "during",
    "each",
    "explain",
    "few",
    "find",
    "for",
    "from",
    "further",
    "had",
    "has",
    "have",
    "having",
    "he",
    "her",
    "here",
    "hers",
    "him",
    "his",
    "how",
    "i",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "just",
    "me",
    "more",
    "most",
    "my",
    "no",
    "nor",
    "not",
    "of",
    "off",
    "on",
    "once",
    "only",
    "or",
    "other",
    "our",
    "ours",
    "out",
    "over",
    "own",
    "paper",
    "papers",
    "please",
    "same",
    "she",
    "should",
    "show",
    "so",
    "some",
    "such",
    "summarise",
    "summarize",
    "summary",
    "than",
    "that",
    "the",
    "their",
    "theirs",
    "them",
    "then",
    "there",
    "these",
    "they",
    "this",
    "those",
    "through",
    "to",
    "too",
    "under",
    "until",
    "up",
    "very",
    "was",
    "we",
    "were",
    "what",
    "when",
    "where",
    "which",
    "while",
    "who",
    "whom",
    "why",
    "will",
    "with",
    "would",
    "you",
    "your",
    "yours",
];

pub fn is_stopword(word: &str) -> bool {
    STOPWORDS.binary_search(&word).is_ok()
}

/// Lower-cased alphanumeric words of `text`.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// A light English stem so "networks" matches "network".
fn stem(word: &str) -> String {
    let n = word.chars().count();
    if n > 4 && word.ends_with("ies") {
        return format!("{}y", &word[..word.len() - 3]);
    }
    if n > 3 && word.ends_with('s') && !word.ends_with("ss") && !word.ends_with("us") {
        return word[..word.len() - 1].to_string();
    }
    word.to_string()
}

/// Significant query terms, stemmed and de-duplicated, in order.
pub fn query_terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    words(query)
        .into_iter()
        .filter(|w| !is_stopword(w))
        .map(|w| stem(&w))
        .filter(|w| seen.insert(w.clone()))
        .collect()
}

/// IDF-weighted share of the query's significant terms that appear in each
/// text, in 0..=1. IDF is computed over the candidate pool with BM25's
/// smoothing, so a term every result shares weighs less than a rare one.
pub fn lexical_scores(query: &str, texts: &[String]) -> Vec<f32> {
    let terms = query_terms(query);
    if terms.is_empty() {
        return vec![0.0; texts.len()];
    }
    let docs: Vec<HashSet<String>> = texts
        .iter()
        .map(|t| words(t).iter().map(|w| stem(w)).collect())
        .collect();
    let n = docs.len() as f32;
    let idf: HashMap<&str, f32> = terms
        .iter()
        .map(|t| {
            let df = docs.iter().filter(|d| d.contains(t)).count() as f32;
            (t.as_str(), (1.0 + (n - df + 0.5) / (df + 0.5)).ln())
        })
        .collect();
    let total: f32 = idf.values().sum();
    docs.iter()
        .map(|d| {
            let hit: f32 = terms
                .iter()
                .filter(|t| d.contains(t.as_str()))
                .map(|t| idf.get(t.as_str()).copied().unwrap_or(0.0))
                .sum();
            if total > 0.0 {
                hit / total
            } else {
                0.0
            }
        })
        .collect()
}

/// Word set used for title comparisons (stopwords kept: they are part of a
/// title).
fn title_words(text: &str) -> HashSet<String> {
    words(text).into_iter().collect()
}

/// Whether `title` is (nearly) the title the query names:
/// * at least 90% of the title's words appear in the query, and the title
///   has at least four words, so a short title cannot match by accident;
/// * the title covers at least 60% of the query's significant terms, so a
///   short title made of a long query's words ("Credit Assignment in
///   Networks" for a query naming a longer title) is not taken for it.
///
/// Measuring the title's words in the query rather than a symmetric overlap
/// lets a query such as "summarize the paper <title>" still match.
pub fn is_title_match(query: &str, title: &str) -> bool {
    let title_set = title_words(title);
    if title_set.len() < 4 {
        return false;
    }
    let query_set = title_words(query);
    let found = title_set.iter().filter(|w| query_set.contains(*w)).count();
    if (found as f32) < 0.9 * title_set.len() as f32 {
        return false;
    }
    let terms = query_terms(query);
    if terms.is_empty() {
        return false;
    }
    let stemmed: HashSet<String> = title_set.iter().map(|w| stem(w)).collect();
    let covered = terms.iter().filter(|t| stemmed.contains(*t)).count();
    covered as f32 >= 0.6 * terms.len() as f32
}

/// Jaccard similarity of two titles' word sets.
pub fn title_jaccard(a: &str, b: &str) -> f32 {
    let a = title_words(a);
    let b = title_words(b);
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(&b).count() as f32;
    let union = a.union(&b).count() as f32;
    inter / union
}

fn sigmoid(x: f32) -> f32 {
    if x.is_nan() || x == f32::NEG_INFINITY {
        return 0.0;
    }
    1.0 / (1.0 + (-x).exp())
}

/// Weight of the content score in the ordering (the rest is the prior).
const CONTENT_WEIGHT: f32 = 0.85;

/// Score, cut and order `candidates`.
///
/// With a scorer the cutoff is `thresholds.cross_encoder` on the sigmoid of
/// its logit; without one (or when it fails, which is logged) it is
/// `thresholds.lexical` on the lexical score. With no cutoff every
/// candidate is kept, in prior order.
pub fn rank<T>(
    query: &str,
    candidates: Vec<Candidate<T>>,
    scorer: Option<&dyn PassageScorer>,
    thresholds: Thresholds,
) -> Ranking<T> {
    let texts: Vec<String> = candidates.iter().map(|c| c.text.clone()).collect();
    let neural = match scorer {
        Some(s) if !texts.is_empty() => match s.score(query, &texts) {
            Ok(logits) if logits.len() == texts.len() => Some(logits),
            Ok(logits) => {
                tracing::warn!(
                    target: "shodh::harness",
                    expected = texts.len(),
                    got = logits.len(),
                    "relevance scorer returned the wrong number of scores; using keyword overlap"
                );
                None
            }
            Err(e) => {
                tracing::warn!(target: "shodh::harness", error = %e, "relevance scorer failed; using keyword overlap");
                None
            }
        },
        _ => None,
    };
    let (method, scores, cutoff) = match neural {
        Some(logits) => (
            RankMethod::CrossEncoder,
            logits.into_iter().map(sigmoid).collect::<Vec<_>>(),
            thresholds.cross_encoder,
        ),
        None => (
            RankMethod::Lexical,
            lexical_scores(query, &texts),
            thresholds.lexical,
        ),
    };
    let mut kept = Vec::new();
    let mut dropped = 0;
    let mut order: Vec<(f32, Scored<T>)> = Vec::new();
    for (candidate, relevance) in candidates.into_iter().zip(scores) {
        let pass = candidate.title_match || cutoff.is_none_or(|c| relevance >= c);
        if !pass {
            dropped += 1;
            continue;
        }
        let key = match (cutoff, candidate.title_match) {
            (_, true) => 2.0 + relevance,
            // No cutoff: the provider's ranking is the order.
            (None, false) => candidate.prior,
            (Some(_), false) => {
                CONTENT_WEIGHT * relevance + (1.0 - CONTENT_WEIGHT) * candidate.prior
            }
        };
        order.push((
            key,
            Scored {
                item: candidate.item,
                relevance,
                title_match: candidate.title_match,
            },
        ));
    }
    // Stable sort: equal keys keep the incoming (merged) order.
    order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    kept.extend(order.into_iter().map(|(_, s)| s));
    Ranking {
        kept,
        dropped,
        method,
    }
}

/// Rank prior for position `index` (0-based) in a source's list: 1.0 for
/// the first hit, decaying like reciprocal-rank fusion.
pub fn rank_prior(index: usize) -> f32 {
    const K: f32 = 4.0;
    K / (K + index as f32)
}

/// A shared, swappable scorer handle.
pub type SharedScorer = Arc<dyn PassageScorer>;

/// The canonical form of a web URL for de-duplication: scheme and host
/// lower-cased, `www.` dropped, default port, fragment and tracking
/// parameters removed, trailing slash trimmed. URLs that do not parse are
/// returned trimmed.
pub fn canonical_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw.trim()) else {
        return raw.trim().to_string();
    };
    url.set_fragment(None);
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            !(k.starts_with("utm_")
                || matches!(
                    k.as_str(),
                    "fbclid" | "gclid" | "msclkid" | "mc_cid" | "mc_eid" | "ref" | "ref_src"
                ))
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(kept);
    }
    let scheme = match url.scheme() {
        "http" | "https" => "https".to_string(),
        other => other.to_string(),
    };
    let host = url
        .host_str()
        .map(|h| h.trim_start_matches("www.").to_ascii_lowercase())
        .unwrap_or_default();
    let port = url
        .port()
        .filter(|p| *p != 80 && *p != 443)
        .map(|p| format!(":{p}"))
        .unwrap_or_default();
    let path = url.path().trim_end_matches('/');
    let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
    format!("{scheme}://{host}{port}{path}{query}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopword_list_is_sorted_for_binary_search() {
        let mut sorted = STOPWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, STOPWORDS);
        assert!(is_stopword("the") && is_stopword("andnot") && !is_stopword("attention"));
    }

    #[test]
    fn query_terms_drop_stopwords_and_stem() {
        assert_eq!(
            query_terms("Find papers about the Networks of Bees"),
            vec!["network", "bee"]
        );
        assert_eq!(
            query_terms("Task-Dependent credit"),
            vec!["task", "dependent", "credit"]
        );
    }

    #[test]
    fn lexical_scores_weight_rare_terms_and_cover_the_query() {
        let texts = vec![
            "Linear attention with the delta rule for sequence models".to_string(),
            "Attention in classrooms".to_string(),
            "Copepod egg production".to_string(),
        ];
        let s = lexical_scores("linear attention delta rule", &texts);
        assert!(s[0] > 0.99, "{s:?}");
        assert!(s[1] < 0.3, "{s:?}");
        assert_eq!(s[2], 0.0);
        assert_eq!(lexical_scores("the of", &texts), vec![0.0; 3]);
    }

    #[test]
    fn title_match_measures_the_titles_words_in_the_query() {
        let title = "Diffusing Blame: Task-Dependent Credit Assignment in Biologically Plausible Dual-Stream Networks";
        assert!(is_title_match(title, title));
        assert!(is_title_match(
            &format!("summarize the paper {title}"),
            title
        ));
        assert!(!is_title_match("credit assignment", title));
        assert!(
            !is_title_match(title, "Credit Assignment in Networks"),
            "a short title made of the query's words is not the named title"
        );
        assert!(
            !is_title_match("deep learning is fun", "Deep Learning"),
            "short titles never match"
        );
    }

    struct Fixed(Vec<f32>);
    impl PassageScorer for Fixed {
        fn score(&self, _: &str, passages: &[String]) -> Result<Vec<f32>, String> {
            Ok(self.0.iter().copied().take(passages.len()).collect())
        }
    }
    struct Broken;
    impl PassageScorer for Broken {
        fn score(&self, _: &str, _: &[String]) -> Result<Vec<f32>, String> {
            Err("model crashed".into())
        }
    }

    fn cand(text: &str, prior: f32, title_match: bool) -> Candidate<String> {
        Candidate {
            item: text.to_string(),
            text: text.to_string(),
            prior,
            title_match,
        }
    }

    #[test]
    fn prior_orders_but_never_rescues() {
        let ranking = rank(
            "q",
            vec![
                cand("junk ranked first by the api", 1.0, false),
                cand("good", 0.5, false),
                cand("better", 0.2, false),
                cand("exact title", 0.1, true),
            ],
            Some(&Fixed(vec![-8.0, 2.0, 4.0, -9.0])),
            PAPER_THRESHOLDS,
        );
        assert_eq!(ranking.method, RankMethod::CrossEncoder);
        assert_eq!(ranking.dropped, 1);
        let order: Vec<&str> = ranking.kept.iter().map(|s| s.item.as_str()).collect();
        assert_eq!(order, vec!["exact title", "better", "good"]);
    }

    #[test]
    fn scorer_failure_falls_back_to_lexical_with_the_strict_cut() {
        let ranking = rank(
            "linear attention delta rule",
            vec![
                cand("Copepod eggs", 1.0, false),
                cand("Delta rule linear attention", 0.5, false),
            ],
            Some(&Broken),
            PAPER_THRESHOLDS,
        );
        assert_eq!(ranking.method, RankMethod::Lexical);
        assert_eq!(ranking.dropped, 1);
        assert_eq!(ranking.kept[0].item, "Delta rule linear attention");
    }

    #[test]
    fn without_a_cut_the_prior_is_the_order() {
        let ranking = rank(
            "rust",
            vec![cand("b", 0.5, false), cand("a", 1.0, false)],
            None,
            GROUNDED_THRESHOLDS,
        );
        assert_eq!(ranking.dropped, 0);
        assert_eq!(ranking.kept[0].item, "a");
    }

    #[test]
    fn canonical_urls_collapse_trivial_differences() {
        let a = canonical_url("https://www.Example.org/post/?utm_source=x&id=3#top");
        let b = canonical_url("http://example.org:80/post?id=3");
        assert_eq!(a, b);
        assert_eq!(a, "https://example.org/post?id=3");
        assert_ne!(
            canonical_url("https://example.org/a"),
            canonical_url("https://example.org/b")
        );
        assert_eq!(canonical_url(" not a url "), "not a url");
    }
}
