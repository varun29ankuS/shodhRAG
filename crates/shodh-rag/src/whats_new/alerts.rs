//! One check of a paper alert: the words the user consented to, sent to arXiv, OpenAlex
//! and Semantic Scholar (one date-bounded request each,
//! [`crate::harness::web::papers::alert_requests`]), the answers merged, papers already
//! known (in the library, or found before, dismissed ones included) left out, and the
//! rest ranked by the local cross-encoder. No model call.
//!
//! - **Nothing but the words leaves.** Requests go through [`SafeClient`] with no header
//!   of their own: no email (OpenAlex's "polite pool" asks for one; it is never given), no
//!   name, no key. The user agent is the client's generic [`USER_AGENT`].
//! - **Each API fails alone.** A check reports every API's outcome; the others' papers
//!   still count.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use super::pace::ApiPacer;
use crate::harness::web::client::{SafeClient, WebError};
use crate::harness::web::papers::{
    alert_requests, merge, parse_arxiv, parse_openalex, parse_semantic_scholar, rank_papers,
    same_paper, Paper, PaperApi,
};
use crate::harness::web::relevance::SharedScorer;

/// The consent line: `{words}` is the exact query sent.
pub const CONSENT_TEMPLATE: &str = "Shodh sends these words: {words} to arXiv, OpenAlex and \
     Semantic Scholar to look for new papers. Your files, your library and who you are stay \
     on this computer.";
/// Longest words an alert sends.
pub const MAX_WORDS_CHARS: usize = 200;

/// How far back an alert's first check looks.
pub const FIRST_WINDOW_DAYS: i64 = 14;
/// Largest answer read from an API.
const MAX_ANSWER_BYTES: u64 = 4 * 1024 * 1024;

/// The consent line for `words` (trimmed): what the user agrees to before anything is
/// sent.
pub fn consent_line(words: &str) -> String {
    CONSENT_TEMPLATE.replace("{words}", words.trim())
}

/// The words an alert sends: trimmed, one space between words, and refused when empty
/// or longer than [`MAX_WORDS_CHARS`].
pub fn clean_words(words: &str) -> Result<String, String> {
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.is_empty() {
        return Err("Type the words to look for.".into());
    }
    if words.chars().count() > MAX_WORDS_CHARS {
        return Err(format!(
            "Keep the words under {MAX_WORDS_CHARS} characters; they are sent as they are."
        ));
    }
    Ok(words)
}

/// Days an API's date window reaches back before the day it was last checked through:
/// an index's late additions are not missed.
fn overlap_days(api: PaperApi) -> i64 {
    match api {
        PaperApi::Arxiv => 2,
        PaperApi::OpenAlex | PaperApi::SemanticScholar => 7,
    }
}

/// The first day each API's request asks for, from the day it was last checked through
/// (none yet: the last [`FIRST_WINDOW_DAYS`] days).
pub fn windows(
    checked_through: &BTreeMap<PaperApi, NaiveDate>,
    now: DateTime<Utc>,
) -> BTreeMap<PaperApi, NaiveDate> {
    PaperApi::ALL
        .iter()
        .map(|api| {
            let since = match checked_through.get(api) {
                Some(day) => *day - TimeDelta::days(overlap_days(*api)),
                None => (now - TimeDelta::days(FIRST_WINDOW_DAYS)).date_naive(),
            };
            (*api, since)
        })
        .collect()
}

/// Why an API gave no papers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Failure {
    /// No connection (the address did not resolve, the request timed out or broke).
    Offline,
    /// Too many requests; `retry_after_secs` when the API said when to come back.
    RateLimited { retry_after_secs: Option<u64> },
    /// Another HTTP status.
    Status { status: u16 },
    /// An answer that could not be read.
    Invalid,
}

/// How one API answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ApiOutcome {
    /// It answered with this many papers.
    Answered {
        papers: usize,
    },
    Failed {
        failure: Failure,
        detail: String,
    },
}

/// One check: the new papers, best first, and how each API asked answered.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertCheck {
    pub new: Vec<Paper>,
    /// Papers the APIs answered that are not known yet and too weak a match to
    /// show: kept as seen, so a later check (whose answers rank differently)
    /// never offers them as new.
    pub weak: Vec<Paper>,
    pub outcomes: BTreeMap<PaperApi, ApiOutcome>,
}

fn failure_of(error: &WebError) -> Failure {
    match error {
        WebError::Unresolvable(_) | WebError::Timeout | WebError::Http(_) => Failure::Offline,
        WebError::Status { status: 429 } => Failure::RateLimited {
            retry_after_secs: None,
        },
        WebError::Status { status } => Failure::Status { status: *status },
        _ => Failure::Invalid,
    }
}

fn failed(failure: Failure, detail: String) -> Result<Vec<Paper>, ApiOutcome> {
    Err(ApiOutcome::Failed { failure, detail })
}

/// Asks `api` at `url`, in its turn.
async fn ask(
    client: &SafeClient,
    pacer: &ApiPacer,
    api: PaperApi,
    url: &url::Url,
) -> Result<Vec<Paper>, ApiOutcome> {
    pacer.wait_turn(api).await;
    let fetched = match client.get(url.as_str(), vec![]).await {
        Ok(fetched) => fetched,
        Err(e) => return failed(failure_of(&e), e.to_string()),
    };
    let status = fetched.status;
    if status == 429 {
        let retry_after_secs = fetched
            .header("retry-after")
            .and_then(|v| v.trim().parse::<u64>().ok());
        return failed(
            Failure::RateLimited { retry_after_secs },
            format!("{} asked Shodh to wait", api.name()),
        );
    }
    if !(200..300).contains(&status) {
        return failed(
            Failure::Status { status },
            format!("{} answered {status}", api.name()),
        );
    }
    let bytes = match fetched.read_capped(MAX_ANSWER_BYTES).await {
        Ok(bytes) => bytes,
        Err(e) => return failed(failure_of(&e), e.to_string()),
    };
    let parsed = match api {
        PaperApi::Arxiv => parse_arxiv(&String::from_utf8_lossy(&bytes)),
        PaperApi::OpenAlex | PaperApi::SemanticScholar => {
            match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(value) if api == PaperApi::OpenAlex => parse_openalex(&value),
                Ok(value) => parse_semantic_scholar(&value),
                Err(e) => Err(WebError::Http(e.to_string())),
            }
        }
    };
    parsed.map_err(|e| ApiOutcome::Failed {
        failure: Failure::Invalid,
        detail: format!("{} sent an answer Shodh could not read: {e}", api.name()),
    })
}

/// Checks `query` on `apis` for papers new since each API's window (`since`), leaving
/// out every paper of `known` (the library's and those found before).
#[allow(clippy::too_many_arguments)]
pub async fn check_alert(
    client: &SafeClient,
    pacer: &ApiPacer,
    query: &str,
    since: &BTreeMap<PaperApi, NaiveDate>,
    apis: &BTreeSet<PaperApi>,
    now: DateTime<Utc>,
    known: &[Paper],
    scorer: Option<SharedScorer>,
) -> AlertCheck {
    let mut outcomes = BTreeMap::new();
    let requests = match alert_requests(query, since, now) {
        Ok(requests) => requests,
        Err(e) => {
            for api in apis {
                outcomes.insert(
                    *api,
                    ApiOutcome::Failed {
                        failure: Failure::Invalid,
                        detail: e.to_string(),
                    },
                );
            }
            return AlertCheck {
                new: Vec::new(),
                weak: Vec::new(),
                outcomes,
            };
        }
    };
    let mut lists = Vec::new();
    for api in apis {
        let Some(url) = requests.get(api) else {
            // No significant word for its query syntax: nothing to ask.
            outcomes.insert(*api, ApiOutcome::Answered { papers: 0 });
            continue;
        };
        match ask(client, pacer, *api, url).await {
            Ok(papers) => {
                outcomes.insert(
                    *api,
                    ApiOutcome::Answered {
                        papers: papers.len(),
                    },
                );
                lists.push(papers);
            }
            Err(outcome) => {
                outcomes.insert(*api, outcome);
            }
        }
    }
    // Ranked over everything the APIs answered, then the known papers left out: a
    // paper's relevance depends on the whole answer (the lexical score weighs terms
    // by how rare they are in it), so ranking only what is left would let papers
    // dropped as weak before pass now. Every relevant paper is kept, not only the
    // best few: one left out would be "new" on the next check though it was there
    // all along.
    let merged = merge(lists);
    let answered: Vec<Paper> = merged.iter().map(|m| m.paper.clone()).collect();
    let limit = merged.len().max(1);
    let owned_query = query.to_string();
    let fallback = merged.clone();
    let ranked = tokio::task::spawn_blocking(move || {
        rank_papers(&owned_query, merged, scorer.as_ref(), limit)
    })
    .await;
    let relevant = match ranked {
        Ok((papers, _, _)) => papers,
        Err(e) => {
            tracing::warn!(error = %e, "paper alert ranking failed; using keyword overlap");
            rank_papers(query, fallback, None, limit).0
        }
    };
    let is_known = |p: &Paper| known.iter().any(|k| same_paper(k, p));
    let weak = answered
        .into_iter()
        .filter(|p| !relevant.iter().any(|r| same_paper(r, p)) && !is_known(p))
        .collect();
    let new = relevant.into_iter().filter(|p| !is_known(p)).collect();
    AlertCheck {
        new,
        weak,
        outcomes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::web::client::testing::{client, Canned, FakeResolver, FakeTransport};
    use crate::harness::web::client::USER_AGENT;
    use crate::whats_new::library::known_paper;
    use chrono::TimeZone;

    const ARXIV: &str = include_str!("../harness/fixtures/papers/linear-attention/arxiv.xml");
    const OPENALEX: &str =
        include_str!("../harness/fixtures/papers/linear-attention/openalex.json");
    const SEMANTIC: &str =
        include_str!("../harness/fixtures/papers/linear-attention/semantic-scholar.json");
    /// The query the fixtures were recorded for.
    const WORDS: &str = "linear attention delta rule";

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, 9, 0, 0).unwrap()
    }

    fn since() -> BTreeMap<PaperApi, NaiveDate> {
        windows(&BTreeMap::new(), now())
    }

    fn urls() -> BTreeMap<PaperApi, url::Url> {
        alert_requests(WORDS, &since(), now()).unwrap()
    }

    fn resolver() -> FakeResolver {
        FakeResolver::default()
            .with("export.arxiv.org", &["128.84.21.199"])
            .with("api.openalex.org", &["104.26.12.155"])
            .with("api.semanticscholar.org", &["52.85.151.33"])
    }

    /// Every API answering with its fixture, except those `failing` with a status.
    fn transport(failing: &[(PaperApi, u16)]) -> FakeTransport {
        let urls = urls();
        let mut transport = FakeTransport::default();
        for (api, body, kind) in [
            (PaperApi::Arxiv, ARXIV, "application/atom+xml"),
            (PaperApi::OpenAlex, OPENALEX, "application/json"),
            (PaperApi::SemanticScholar, SEMANTIC, "application/json"),
        ] {
            let canned = match failing.iter().find(|(a, _)| *a == api) {
                Some((_, status)) => Canned {
                    status: *status,
                    headers: vec![("retry-after".into(), "120".into())],
                    body: Vec::new(),
                },
                None => Canned::ok(kind, body),
            };
            transport = transport.route(urls[&api].as_str(), canned);
        }
        transport
    }

    fn all() -> BTreeSet<PaperApi> {
        PaperApi::ALL.into_iter().collect()
    }

    async fn check(
        transport: FakeTransport,
        apis: &BTreeSet<PaperApi>,
        known: &[Paper],
    ) -> (AlertCheck, std::sync::Arc<FakeTransport>) {
        let (client, transport) = client(resolver(), transport);
        let pacer = ApiPacer::default();
        let found = check_alert(&client, &pacer, WORDS, &since(), apis, now(), known, None).await;
        (found, transport)
    }

    #[test]
    fn alert_requests_are_one_per_api_bounded_by_date() {
        let urls = urls();
        assert_eq!(
            urls.len(),
            3,
            "one request per API, no OpenAlex title request"
        );
        let arxiv = urls[&PaperApi::Arxiv].as_str();
        assert!(
            arxiv.starts_with("https://export.arxiv.org/api/query?search_query=%28"),
            "{arxiv}"
        );
        for part in [
            "+AND+submittedDate%3A%5B202611090000+TO+202611230900%5D&",
            "&sortBy=submittedDate&sortOrder=descending&start=0&max_results=25",
        ] {
            assert!(arxiv.contains(part), "{part} in {arxiv}");
        }
        let openalex = urls[&PaperApi::OpenAlex].as_str();
        assert!(openalex.starts_with(
            "https://api.openalex.org/works?search=linear+attention+delta+rule&filter=from_publication_date%3A2026-11-09&per-page=25&select="
        ), "{openalex}");
        let semantic = urls[&PaperApi::SemanticScholar].as_str();
        assert!(semantic.starts_with(
            "https://api.semanticscholar.org/graph/v1/paper/search?query=linear+attention+delta+rule&publicationDateOrYear=2026-11-09%3A&limit=25&fields="
        ), "{semantic}");
    }

    #[test]
    fn windows_overlap_the_last_check_and_start_fourteen_days_back() {
        assert_eq!(
            since()[&PaperApi::Arxiv],
            NaiveDate::from_ymd_opt(2026, 11, 9).unwrap()
        );
        let checked = BTreeMap::from([
            (
                PaperApi::Arxiv,
                NaiveDate::from_ymd_opt(2026, 11, 16).unwrap(),
            ),
            (
                PaperApi::OpenAlex,
                NaiveDate::from_ymd_opt(2026, 11, 16).unwrap(),
            ),
        ]);
        let w = windows(&checked, now());
        assert_eq!(
            w[&PaperApi::Arxiv],
            NaiveDate::from_ymd_opt(2026, 11, 14).unwrap()
        );
        assert_eq!(
            w[&PaperApi::OpenAlex],
            NaiveDate::from_ymd_opt(2026, 11, 9).unwrap()
        );
        assert_eq!(
            w[&PaperApi::SemanticScholar],
            NaiveDate::from_ymd_opt(2026, 11, 9).unwrap(),
            "never checked: the first window"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn alert_requests_carry_no_identity() {
        let (found, transport) = check(transport(&[]), &all(), &[]).await;
        assert!(found
            .outcomes
            .values()
            .all(|o| matches!(o, ApiOutcome::Answered { .. })));
        let sent = transport.sent.lock().unwrap();
        assert_eq!(sent.len(), 3);
        for request in sent.iter() {
            assert!(
                request.headers.is_empty(),
                "no header of its own: {:?}",
                request.headers
            );
            let url = request.url.as_str();
            assert!(!url.contains("mailto"), "{url}");
            assert!(!url.contains('@') && !url.contains("%40"), "{url}");
        }
        assert!(!USER_AGENT.contains('@'));
    }

    #[tokio::test(start_paused = true)]
    async fn the_same_paper_from_three_apis_is_one() {
        let (found, _) = check(transport(&[]), &all(), &[]).await;
        assert!(!found.new.is_empty());

        for (i, a) in found.new.iter().enumerate() {
            for b in &found.new[i + 1..] {
                assert!(
                    !same_paper(a, b),
                    "{} and {} are one paper",
                    a.title,
                    b.title
                );
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_check_keeps_every_relevant_paper_so_the_same_answers_find_nothing_again() {
        let (first, _) = check(transport(&[]), &all(), &[]).await;
        assert!(!first.new.is_empty());
        // The same answers again, with what the first check kept as known.
        // What the first check kept: its new papers, and those it saw as weak.
        let known: Vec<Paper> = first
            .new
            .iter()
            .chain(&first.weak)
            .map(|p| known_paper(&p.title, p.doi.as_deref(), p.arxiv_id.as_deref()))
            .collect();
        let (again, _) = check(transport(&[]), &all(), &known).await;
        assert!(
            again.new.is_empty(),
            "every relevant paper was kept the first time: {:?}",
            again.new.iter().map(|p| &p.title).collect::<Vec<_>>()
        );
        assert!(again.weak.is_empty());
        // One API's answers alone rank differently; still nothing is new.
        let only = BTreeSet::from([PaperApi::SemanticScholar]);
        let (alone, _) = check(transport(&[]), &only, &known).await;
        assert!(
            alone.new.is_empty(),
            "{:?}",
            alone.new.iter().map(|p| &p.title).collect::<Vec<_>>()
        );
        for p in &first.weak {
            assert!(
                !first.new.iter().any(|n| same_paper(n, p)),
                "weak and new are apart"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn new_papers_skip_the_library_and_earlier_items() {
        let (first, _) = check(transport(&[]), &all(), &[]).await;
        assert!(first.new.len() >= 2);
        // One is in the library by its arXiv id, or else its title; one was found before.
        let a = &first.new[0];
        let b = &first.new[1];
        let in_library = known_paper(&a.title, None, a.arxiv_id.as_deref());
        let found_before = known_paper(&b.title, b.doi.as_deref(), b.arxiv_id.as_deref());
        let (again, _) = check(transport(&[]), &all(), &[in_library, found_before]).await;
        for p in &again.new {
            assert!(
                !same_paper(p, a) && !same_paper(p, b),
                "{} came back",
                p.title
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_paper_known_by_title_is_not_new_when_it_gains_a_doi() {
        let (first, _) = check(transport(&[]), &all(), &[]).await;
        let paper = first
            .new
            .iter()
            .find(|p| p.doi.is_some() || p.arxiv_id.is_some())
            .expect("a paper with an identifier");
        // Known from an earlier check by its title only.
        let by_title = known_paper(&paper.title, None, None);
        let (again, _) = check(transport(&[]), &all(), &[by_title]).await;
        assert!(
            !again.new.iter().any(|p| same_paper(p, paper)),
            "{} came back with its identifier",
            paper.title
        );
    }

    #[tokio::test(start_paused = true)]
    async fn one_api_failing_reports_which_and_keeps_the_others() {
        let (found, _) = check(transport(&[(PaperApi::SemanticScholar, 503)]), &all(), &[]).await;
        assert!(matches!(
            found.outcomes[&PaperApi::SemanticScholar],
            ApiOutcome::Failed {
                failure: Failure::Status { status: 503 },
                ..
            }
        ));
        assert!(matches!(
            found.outcomes[&PaperApi::Arxiv],
            ApiOutcome::Answered { .. }
        ));
        assert!(matches!(
            found.outcomes[&PaperApi::OpenAlex],
            ApiOutcome::Answered { .. }
        ));
        assert!(!found.new.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_reads_retry_after() {
        let (found, _) = check(transport(&[(PaperApi::OpenAlex, 429)]), &all(), &[]).await;
        assert_eq!(
            found.outcomes[&PaperApi::OpenAlex],
            ApiOutcome::Failed {
                failure: Failure::RateLimited {
                    retry_after_secs: Some(120)
                },
                detail: "OpenAlex asked Shodh to wait".into(),
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn apis_already_checked_this_run_are_not_asked_again() {
        let only = BTreeSet::from([PaperApi::Arxiv, PaperApi::SemanticScholar]);
        let (found, transport) = check(transport(&[]), &only, &[]).await;
        assert_eq!(transport.sent.lock().unwrap().len(), 2);
        assert!(!found.outcomes.contains_key(&PaperApi::OpenAlex));
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreachable_api_is_offline() {
        let (client, _) = client(FakeResolver::default(), FakeTransport::default());
        let pacer = ApiPacer::default();
        let found = check_alert(&client, &pacer, WORDS, &since(), &all(), now(), &[], None).await;
        for outcome in found.outcomes.values() {
            assert!(
                matches!(
                    outcome,
                    ApiOutcome::Failed {
                        failure: Failure::Offline,
                        ..
                    }
                ),
                "{outcome:?}"
            );
        }
        assert!(found.new.is_empty());
    }

    #[test]
    fn the_consent_line_names_all_three_apis_and_the_exact_words() {
        assert_eq!(
            consent_line("  linear attention KV cache "),
            "Shodh sends these words: linear attention KV cache to arXiv, OpenAlex and Semantic Scholar to look for new papers. Your files, your library and who you are stay on this computer."
        );
    }

    #[test]
    fn words_are_cleaned_and_bounded() {
        assert_eq!(
            clean_words("  linear   attention ").unwrap(),
            "linear attention"
        );
        assert!(clean_words("   ").is_err());
        assert!(clean_words(&"a".repeat(MAX_WORDS_CHARS + 1)).is_err());
        assert!(clean_words(&"a".repeat(MAX_WORDS_CHARS)).is_ok());
    }
}
