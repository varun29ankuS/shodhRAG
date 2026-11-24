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
    same_paper, Paper, PaperApi, ALERT_RESULTS_PER_API,
};
use crate::harness::web::relevance::SharedScorer;

/// The consent line: `{words}` is the exact query sent.
pub const CONSENT_TEMPLATE: &str = "Shodh sends these words: {words} to arXiv, OpenAlex and \
     Semantic Scholar to look for new papers. Your files, your library and who you are stay \
     on this computer.";
/// Most new papers one check shows; the rest are kept as seen.
pub const NEW_PAPERS_PER_RUN: usize = 10;
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
    /// No connection could be made: the name did not resolve, or the connection was
    /// refused or had no route. The computer (or the API) is offline.
    Offline,
    /// The API was reached but gave no answer: the request timed out or broke off.
    Unanswered,
    /// Too many requests; `retry_after_secs` when the API said when to come back.
    RateLimited { retry_after_secs: Option<u64> },
    /// Another HTTP status (`retry_after_secs` when a 503 said when to come back).
    Status {
        status: u16,
        retry_after_secs: Option<u64>,
    },
    /// An answer that could not be read.
    Invalid,
}

impl Failure {
    /// When the API asked to be asked again, in seconds.
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited { retry_after_secs }
            | Self::Status {
                retry_after_secs, ..
            } => *retry_after_secs,
            _ => None,
        }
    }
}

/// The longest wait a `Retry-After` is honoured for.
pub const MAX_RETRY_AFTER_SECS: u64 = 24 * 60 * 60;

/// A `Retry-After` header: seconds, or an HTTP date (`Wed, 21 Oct 2026 07:28:00 GMT`),
/// as seconds from `now`, at most [`MAX_RETRY_AFTER_SECS`]. `None` when it cannot be
/// read.
pub fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<u64> {
    let value = value.trim();
    let secs = if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        // Digits only: a huge number saturates instead of overflowing.
        value.parse::<u64>().unwrap_or(u64::MAX)
    } else {
        let at = DateTime::parse_from_rfc2822(value)
            .ok()?
            .with_timezone(&Utc);
        u64::try_from((at - now).num_seconds()).unwrap_or(0)
    };
    Some(secs.min(MAX_RETRY_AFTER_SECS))
}

/// How one API answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ApiOutcome {
    /// It answered with this many papers. `full`: as many as were asked for, so
    /// older ones in the window may be missing; `oldest`: the earliest day among them,
    /// when the API dates its papers.
    Answered {
        papers: usize,
        full: bool,
        oldest: Option<NaiveDate>,
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
    /// Papers the APIs answered that are not known yet and are not shown: too weak a
    /// match, or past the [`NEW_PAPERS_PER_RUN`] best. They are kept as seen, so a
    /// later check (whose answers rank differently) never offers them as new.
    pub set_aside: Vec<Paper>,
    pub outcomes: BTreeMap<PaperApi, ApiOutcome>,
}

fn failure_of(error: &WebError) -> Failure {
    match error {
        WebError::Unresolvable(_) | WebError::Unreachable(_) => Failure::Offline,
        WebError::Timeout | WebError::Http(_) => Failure::Unanswered,
        WebError::Status { status: 429 } => Failure::RateLimited {
            retry_after_secs: None,
        },
        WebError::Status { status } => Failure::Status {
            status: *status,
            retry_after_secs: None,
        },
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
    now: DateTime<Utc>,
) -> Result<Vec<Paper>, ApiOutcome> {
    // The turn is held until the answer is read: the spacing counts from its end.
    let _turn = pacer.wait_turn(api).await;
    let fetched = match client.get(url.as_str(), vec![]).await {
        Ok(fetched) => fetched,
        Err(e) => return failed(failure_of(&e), e.to_string()),
    };
    let status = fetched.status;
    let retry_after_secs = fetched
        .header("retry-after")
        .and_then(|v| parse_retry_after(v, now));
    if status == 429 {
        return failed(
            Failure::RateLimited { retry_after_secs },
            format!("{} asked Shodh to wait", api.name()),
        );
    }
    if !(200..300).contains(&status) {
        return failed(
            Failure::Status {
                status,
                retry_after_secs: retry_after_secs.filter(|_| status == 503),
            },
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
                set_aside: Vec::new(),
                outcomes,
            };
        }
    };
    // Each request is one hop (paced as one): a redirect is not followed.
    let client = client.clone().without_redirects();
    let mut lists = Vec::new();
    for api in apis {
        let Some(url) = requests.get(api) else {
            // No significant word for its query syntax: nothing to ask.
            outcomes.insert(
                *api,
                ApiOutcome::Answered {
                    papers: 0,
                    full: false,
                    oldest: None,
                },
            );
            continue;
        };
        match ask(&client, pacer, *api, url, now).await {
            Ok(papers) => {
                outcomes.insert(
                    *api,
                    ApiOutcome::Answered {
                        papers: papers.len(),
                        full: papers.len() >= ALERT_RESULTS_PER_API,
                        oldest: papers.iter().filter_map(|p| p.published).min(),
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
    let mut set_aside: Vec<Paper> = answered
        .into_iter()
        .filter(|p| !relevant.iter().any(|r| same_paper(r, p)) && !is_known(p))
        .collect();
    let mut new: Vec<Paper> = relevant.into_iter().filter(|p| !is_known(p)).collect();
    // The best few are shown; the rest are kept as seen, so they never come back as new.
    if new.len() > NEW_PAPERS_PER_RUN {
        set_aside.extend(new.split_off(NEW_PAPERS_PER_RUN));
    }
    AlertCheck {
        new,
        set_aside,
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
        let sent = transport.sent.lock().unwrap_or_else(|e| e.into_inner());
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
            .chain(&first.set_aside)
            .map(|p| known_paper(&p.title, p.doi.as_deref(), p.arxiv_id.as_deref()))
            .collect();
        let (again, _) = check(transport(&[]), &all(), &known).await;
        assert!(
            again.new.is_empty(),
            "every relevant paper was kept the first time: {:?}",
            again.new.iter().map(|p| &p.title).collect::<Vec<_>>()
        );
        assert!(again.set_aside.is_empty());
        // One API's answers alone rank differently; still nothing is new.
        let only = BTreeSet::from([PaperApi::SemanticScholar]);
        let (alone, _) = check(transport(&[]), &only, &known).await;
        assert!(
            alone.new.is_empty(),
            "{:?}",
            alone.new.iter().map(|p| &p.title).collect::<Vec<_>>()
        );
        for p in &first.set_aside {
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
                failure: Failure::Status { status: 503, .. },
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
        assert_eq!(
            transport
                .sent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len(),
            2
        );
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
    fn retry_after_reads_seconds_and_dates_and_is_capped() {
        let now = Utc.with_ymd_and_hms(2026, 11, 23, 9, 0, 0).unwrap();
        assert_eq!(parse_retry_after("120", now), Some(120));
        assert_eq!(parse_retry_after(" 0 ", now), Some(0));
        assert_eq!(
            parse_retry_after("Mon, 23 Nov 2026 09:30:00 GMT", now),
            Some(1800),
            "an HTTP date"
        );
        assert_eq!(
            parse_retry_after("Mon, 23 Nov 2026 08:00:00 GMT", now),
            Some(0),
            "a date already past"
        );
        assert_eq!(
            parse_retry_after("99999999999999999999999999", now),
            Some(MAX_RETRY_AFTER_SECS),
            "a huge number is capped, never overflows"
        );
        assert_eq!(parse_retry_after("172800", now), Some(MAX_RETRY_AFTER_SECS));
        assert_eq!(
            parse_retry_after("Fri, 01 Jan 2100 00:00:00 GMT", now),
            Some(MAX_RETRY_AFTER_SECS)
        );
        assert_eq!(parse_retry_after("soon", now), None);
        assert_eq!(parse_retry_after("", now), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_503_with_retry_after_is_honoured() {
        let urls = urls();
        let transport = transport(&[]).route(
            urls[&PaperApi::SemanticScholar].as_str(),
            Canned {
                status: 503,
                headers: vec![("retry-after".into(), "Mon, 23 Nov 2026 10:00:00 GMT".into())],
                body: Vec::new(),
            },
        );
        let (found, _) = check(transport, &all(), &[]).await;
        match &found.outcomes[&PaperApi::SemanticScholar] {
            ApiOutcome::Failed { failure, .. } => {
                assert_eq!(failure.retry_after_secs(), Some(3600));
                assert!(matches!(failure, Failure::Status { status: 503, .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_reached_api_that_fails_is_not_offline() {
        let urls = urls();
        // OpenAlex resolves but has no answer for the request.
        let mut transport = transport(&[]);
        transport.routes.remove(urls[&PaperApi::OpenAlex].as_str());
        let (found, _) = check(transport, &all(), &[]).await;
        // The fake answers an unrouted request 404: a reached API that did not answer.
        assert!(matches!(
            found.outcomes[&PaperApi::OpenAlex],
            ApiOutcome::Failed {
                failure: Failure::Status { status: 404, .. },
                ..
            }
        ));
        assert_eq!(failure_of(&WebError::Timeout), Failure::Unanswered);
        assert_eq!(
            failure_of(&WebError::Http("reset".into())),
            Failure::Unanswered
        );
        assert_eq!(
            failure_of(&WebError::Unreachable("refused".into())),
            Failure::Offline
        );
        assert_eq!(
            failure_of(&WebError::Unresolvable("x".into())),
            Failure::Offline
        );
    }

    #[tokio::test(start_paused = true)]
    async fn at_most_ten_new_papers_are_shown_and_the_rest_kept_as_seen() {
        let (found, _) = check(transport(&[]), &all(), &[]).await;
        assert!(found.new.len() <= NEW_PAPERS_PER_RUN);
        let total = found.new.len() + found.set_aside.len();
        for (i, a) in found.new.iter().chain(&found.set_aside).enumerate() {
            for b in found.new.iter().chain(&found.set_aside).skip(i + 1) {
                assert!(!same_paper(a, b), "{} twice", a.title);
            }
        }
        assert!(total >= found.new.len());
        if found.new.len() == NEW_PAPERS_PER_RUN {
            // What was over the cap is set aside, so the next check offers none of it.
            let known: Vec<Paper> = found
                .new
                .iter()
                .chain(&found.set_aside)
                .map(|p| known_paper(&p.title, p.doi.as_deref(), p.arxiv_id.as_deref()))
                .collect();
            let (again, _) = check(transport(&[]), &all(), &known).await;
            assert!(again.new.is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_arxiv_page_reports_its_oldest_day() {
        let (found, _) = check(transport(&[]), &all(), &[]).await;
        match found.outcomes[&PaperApi::Arxiv] {
            ApiOutcome::Answered {
                papers,
                full,
                oldest,
            } => {
                assert_eq!(full, papers >= ALERT_RESULTS_PER_API);
                assert!(oldest.is_some(), "arXiv dates its papers");
            }
            ref other => panic!("{other:?}"),
        }
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
