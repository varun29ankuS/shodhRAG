//! Resolving papers to OpenAlex works: identifier lookups, a strict title match, a cache in
//! `shodh.db` and a polite request pace.
//!
//! What is sent: only bibliographic identifiers (a DOI, or an arXiv id as its arXiv DOI)
//! and, for references without one, the reference's title with its year range. Document
//! text is never sent, and neither is a library paper's own title (a paper of the user's
//! may be unpublished): a library paper is looked up by its DOI or arXiv id only.
//!
//! Matching ([`strict_match`]): a title search result is accepted only when its title is at
//! least [`TITLE_THRESHOLD`] similar to the reference's, its year is within one of the
//! reference's and its first author has the reference's first-author surname. An
//! identifier lookup is accepted unless both titles are known and plainly disagree (a
//! mistyped DOI).
//!
//! Requests go through [`SafeClient`] (SSRF checks, the app's generic User-Agent, no
//! e-mail or other identifying header), at most one every [`MIN_INTERVAL`], at most
//! [`DEFAULT_BUDGET`] per build, and stop for the build when the API answers 429. Every
//! answer, including "not found", is cached in `scholarly_cache` ([`POSITIVE_TTL_DAYS`],
//! [`NEGATIVE_TTL_DAYS`]), so a rebuild sends nothing for what it already asked.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use url::Url;

use super::text::{normalize_surname, title_similarity};
use crate::harness::web::papers::{normalize_arxiv_id, normalize_doi, OPENALEX_API};
use crate::harness::web::{SafeClient, WebError};
use crate::research::db::ResearchDb;
use crate::research::{blocking, ResearchError, ResearchResult};

/// Lowest title similarity a title search result needs.
pub const TITLE_THRESHOLD: f64 = 0.9;
/// Below this similarity an identifier lookup's title disagrees with the reference.
pub const IDENTIFIER_TITLE_FLOOR: f64 = 0.5;
/// Pause between two requests.
pub const MIN_INTERVAL: Duration = Duration::from_millis(250);
/// Most requests one build sends.
pub const DEFAULT_BUDGET: usize = 400;
/// How long a found work is reused before it is asked again.
pub const POSITIVE_TTL_DAYS: i64 = 30;
/// How long "not found" is remembered.
pub const NEGATIVE_TTL_DAYS: i64 = 7;
/// Largest answer read.
const MAX_ANSWER_BYTES: u64 = 2 * 1024 * 1024;
/// Results asked of a title search.
const SEARCH_RESULTS: usize = 5;
/// Fields asked of OpenAlex (keeps answers small).
const SELECT: &str =
    "id,doi,display_name,publication_year,authorships,primary_location,cited_by_count,ids,type";
/// arXiv's DOI prefix.
const ARXIV_DOI_PREFIX: &str = "10.48550/arxiv.";

/// One OpenAlex work, reduced to what the graph uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAlexWork {
    /// `W…` id.
    pub id: String,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub title: String,
    pub year: Option<i32>,
    /// Display names, in author order.
    pub authors: Vec<String>,
    pub venue: Option<String>,
    pub cited_by_count: Option<i64>,
}

impl OpenAlexWork {
    /// Comparable surnames of the first author's name (every word of it, so `Yang
    /// Songlin` and `Songlin Yang` both match `yang`).
    fn first_author_words(&self) -> Vec<String> {
        self.authors
            .first()
            .map(|name| {
                name.split([' ', '-'])
                    .map(normalize_surname)
                    .filter(|w| w.len() > 1)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// `W123` from `https://openalex.org/W123` or `W123`.
pub fn openalex_id(raw: &str) -> Option<String> {
    let id = raw.trim().rsplit('/').next()?.to_string();
    let digits = id.strip_prefix('W')?;
    (!digits.is_empty() && digits.len() <= 12 && digits.chars().all(|c| c.is_ascii_digit()))
        .then_some(id)
}

/// Reads one work object.
pub fn parse_work(work: &Value) -> Option<OpenAlexWork> {
    let text = |ptr: &str| {
        work.pointer(ptr)
            .and_then(Value::as_str)
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
    };
    let id = text("/id").and_then(|i| openalex_id(&i))?;
    let title = text("/display_name").or_else(|| text("/title"))?;
    let doi = text("/doi").and_then(|d| normalize_doi(&d));
    let arxiv_id = doi
        .as_deref()
        .and_then(normalize_arxiv_id)
        .or_else(|| text("/ids/arxiv").and_then(|a| normalize_arxiv_id(&a)))
        .or_else(|| {
            text("/primary_location/landing_page_url").and_then(|u| normalize_arxiv_id(&u))
        });
    let authors = work
        .get("authorships")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    a.pointer("/author/display_name")
                        .and_then(Value::as_str)
                        .or_else(|| a.get("raw_author_name").and_then(Value::as_str))
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                })
                .collect()
        })
        .unwrap_or_default();
    Some(OpenAlexWork {
        id,
        doi,
        arxiv_id,
        title,
        year: work
            .get("publication_year")
            .and_then(Value::as_i64)
            .and_then(|y| i32::try_from(y).ok()),
        authors,
        venue: text("/primary_location/source/display_name"),
        cited_by_count: work.get("cited_by_count").and_then(Value::as_i64),
    })
}

/// Reads the `results` of a `/works` list answer.
pub fn parse_work_list(answer: &Value) -> Vec<OpenAlexWork> {
    answer
        .get("results")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(parse_work).collect())
        .unwrap_or_default()
}

/// What is looked up.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkRequest {
    /// A DOI (an arXiv id is looked up as its arXiv DOI).
    Doi(String),
    /// A reference's title within a year range.
    Title { title: String, year: Option<i32> },
}

impl WorkRequest {
    /// The lookup of an arXiv id.
    pub fn arxiv(id: &str) -> Self {
        WorkRequest::Doi(format!("{ARXIV_DOI_PREFIX}{}", id.to_ascii_lowercase()))
    }

    /// The cache key: the request in a canonical spelling.
    pub fn cache_key(&self) -> String {
        match self {
            WorkRequest::Doi(doi) => format!("openalex:doi:{}", doi.to_ascii_lowercase()),
            WorkRequest::Title { title, year } => format!(
                "openalex:title:{}:{}",
                title_filter_text(title),
                year.map(|y| y.to_string()).unwrap_or_default()
            ),
        }
    }

    /// The OpenAlex URL of the request.
    pub fn url(&self) -> Result<Url, WebError> {
        let parse = |s: &str| Url::parse(s).map_err(|e| WebError::Http(e.to_string()));
        match self {
            WorkRequest::Doi(doi) => {
                let mut url = parse(&format!("{OPENALEX_API}/doi:{doi}"))?;
                url.query_pairs_mut().append_pair("select", SELECT);
                Ok(url)
            }
            WorkRequest::Title { title, year } => {
                let mut filter = format!("title.search:{}", title_filter_text(title));
                if let Some(y) = year {
                    filter.push_str(&format!(",publication_year:{}-{}", y - 1, y + 1));
                }
                let mut url = parse(OPENALEX_API)?;
                url.query_pairs_mut()
                    .append_pair("filter", &filter)
                    .append_pair("per-page", &SEARCH_RESULTS.to_string())
                    .append_pair("select", SELECT);
                Ok(url)
            }
        }
    }
}

/// A title as an OpenAlex filter value: filter values are split on `,` and `|` and `:`
/// separates a filter's name, so only letters, digits and spaces are kept.
fn title_filter_text(title: &str) -> String {
    title
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// What a reference says about the work it cites, for matching.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkQuery {
    pub title: Option<String>,
    pub year: Option<i32>,
    pub first_surname: Option<String>,
}

/// Whether a work matches.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchVerdict {
    Accepted { similarity: f64 },
    Rejected { reason: String },
}

impl MatchVerdict {
    pub fn accepted(&self) -> bool {
        matches!(self, MatchVerdict::Accepted { .. })
    }
}

/// The strict title-search rule (see the module documentation). All three of title, year
/// and first-author surname must be known on the reference side.
pub fn strict_match(query: &WorkQuery, work: &OpenAlexWork) -> MatchVerdict {
    let reject = |reason: String| MatchVerdict::Rejected { reason };
    let (Some(title), Some(year), Some(surname)) =
        (&query.title, query.year, query.first_surname.as_deref())
    else {
        return reject("the reference lacks a title, year or first author".into());
    };
    let similarity = title_similarity(title, &work.title);
    if similarity < TITLE_THRESHOLD {
        return reject(format!(
            "title similarity {similarity:.2} < {TITLE_THRESHOLD}"
        ));
    }
    match work.year {
        Some(y) if (y - year).abs() <= 1 => {}
        Some(y) => return reject(format!("year {y} is not within one of {year}")),
        None => return reject("the work has no year".into()),
    }
    let surname = normalize_surname(surname);
    if !work.first_author_words().contains(&surname) {
        return reject(format!(
            "first author {} is not {surname}",
            work.authors.first().map(String::as_str).unwrap_or("(none)")
        ));
    }
    MatchVerdict::Accepted { similarity }
}

/// The rule for identifier lookups: accepted unless both titles are known and their
/// similarity is below [`IDENTIFIER_TITLE_FLOOR`].
pub fn identifier_match(title: Option<&str>, work: &OpenAlexWork) -> MatchVerdict {
    match title {
        Some(t) => {
            let similarity = title_similarity(t, &work.title);
            if similarity < IDENTIFIER_TITLE_FLOOR {
                MatchVerdict::Rejected {
                    reason: format!(
                        "the identifier names \"{}\", whose title disagrees ({similarity:.2})",
                        work.title
                    ),
                }
            } else {
                MatchVerdict::Accepted { similarity }
            }
        }
        None => MatchVerdict::Accepted { similarity: 1.0 },
    }
}

/// Why nothing more is looked up in this build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum Halt {
    /// The resolver reads only cached answers (the web is not allowed): nothing is sent.
    Offline,
    /// The per-build request budget is spent.
    Budget,
    /// The API answered 429 (too many requests).
    RateLimited,
    /// The API could not be reached.
    Unreachable(String),
}

/// What one lookup gave.
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    /// Works the API returned (one for a DOI; up to five for a title).
    Works(Vec<OpenAlexWork>),
    /// The API knows no such work.
    NotFound,
    /// Not asked: the build halted.
    Halted(Halt),
}

/// Counters of one build's lookups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupStats {
    pub requests: usize,
    pub cache_hits: usize,
    pub not_found: usize,
    pub errors: usize,
}

/// Fetches the JSON answer of a URL. Production uses [`SafeClient`]; tests record or
/// replay answers.
#[async_trait::async_trait]
pub trait ScholarlyTransport: Send + Sync {
    /// The status and body of a GET.
    async fn get(&self, url: &Url) -> Result<(u16, Vec<u8>), WebError>;
}

#[async_trait::async_trait]
impl ScholarlyTransport for SafeClient {
    async fn get(&self, url: &Url) -> Result<(u16, Vec<u8>), WebError> {
        let fetched = SafeClient::get(self, url.as_str(), Vec::new()).await?;
        let status = fetched.status;
        let body = fetched.read_capped(MAX_ANSWER_BYTES).await?;
        Ok((status, body))
    }
}

struct Pace {
    last: Option<tokio::time::Instant>,
    halted: Option<Halt>,
    stats: LookupStats,
}

/// Cached, paced OpenAlex lookups for one build. [`Resolver::online`] is created only when
/// the user's policy allows the web (see the app's graph command); otherwise
/// [`Resolver::cache_only`] reads answers already cached on this computer and sends
/// nothing.
pub struct Resolver {
    transport: Option<Arc<dyn ScholarlyTransport>>,
    db: Arc<ResearchDb>,
    budget: usize,
    interval: Duration,
    pace: Mutex<Pace>,
}

impl std::fmt::Debug for Resolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolver")
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl Resolver {
    /// Lookups that may go to OpenAlex.
    pub fn online(transport: Arc<dyn ScholarlyTransport>, db: Arc<ResearchDb>) -> Self {
        Self::with_transport(Some(transport), db)
    }

    /// Lookups from the cache only: a miss is [`Halt::Offline`] and nothing is sent.
    pub fn cache_only(db: Arc<ResearchDb>) -> Self {
        Self::with_transport(None, db)
    }

    fn with_transport(transport: Option<Arc<dyn ScholarlyTransport>>, db: Arc<ResearchDb>) -> Self {
        Self {
            transport,
            db,
            budget: DEFAULT_BUDGET,
            interval: MIN_INTERVAL,
            pace: Mutex::new(Pace {
                last: None,
                halted: None,
                stats: LookupStats::default(),
            }),
        }
    }

    /// Another request budget and pause (tests use a zero pause).
    pub fn with_limits(mut self, budget: usize, interval: Duration) -> Self {
        self.budget = budget;
        self.interval = interval;
        self
    }

    /// Whether lookups may go to the network.
    pub fn is_online(&self) -> bool {
        self.transport.is_some()
    }

    /// Counters so far.
    pub async fn stats(&self) -> LookupStats {
        self.pace.lock().await.stats
    }

    /// Why lookups stopped, if they did.
    pub async fn halted(&self) -> Option<Halt> {
        self.pace.lock().await.halted.clone()
    }

    /// One lookup: from the cache when fresh, else from the API (paced, within budget).
    pub async fn lookup(&self, request: &WorkRequest) -> ResearchResult<Lookup> {
        let key = request.cache_key();
        let db = self.db.clone();
        let cache_key = key.clone();
        if let Some((status, body)) = blocking(move || db.cached_answer(&cache_key)).await? {
            self.pace.lock().await.stats.cache_hits += 1;
            return Ok(decode(request, status, &body));
        }
        let Some(transport) = self.transport.as_ref() else {
            return Ok(Lookup::Halted(Halt::Offline));
        };
        let mut pace = self.pace.lock().await;
        if let Some(halt) = &pace.halted {
            return Ok(Lookup::Halted(halt.clone()));
        }
        if pace.stats.requests >= self.budget {
            pace.halted = Some(Halt::Budget);
            return Ok(Lookup::Halted(Halt::Budget));
        }
        if let Some(last) = pace.last {
            let wait = self.interval.saturating_sub(last.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        pace.last = Some(tokio::time::Instant::now());
        pace.stats.requests += 1;
        let url = request
            .url()
            .map_err(|e| ResearchError::Invalid(e.to_string()))?;
        let answer = transport.get(&url).await;
        let (status, body) = match answer {
            Ok(answer) => answer,
            Err(e) => {
                pace.stats.errors += 1;
                let halt = Halt::Unreachable(e.to_string());
                pace.halted = Some(halt.clone());
                return Ok(Lookup::Halted(halt));
            }
        };
        match status {
            200..=299 | 404 => {}
            429 => {
                pace.halted = Some(Halt::RateLimited);
                return Ok(Lookup::Halted(Halt::RateLimited));
            }
            other if other >= 500 => {
                // A server error halts the build and is not cached; the next build asks
                // again.
                pace.stats.errors += 1;
                let halt = Halt::Unreachable(format!("OpenAlex answered {other}"));
                pace.halted = Some(halt.clone());
                return Ok(Lookup::Halted(halt));
            }
            _ => {
                // Another client error (a malformed request): this lookup gives nothing,
                // is not cached, and the build goes on.
                pace.stats.errors += 1;
                return Ok(Lookup::NotFound);
            }
        }
        let lookup = decode(request, status, &body);
        if matches!(lookup, Lookup::NotFound) {
            pace.stats.not_found += 1;
        }
        drop(pace);
        let ttl = if matches!(lookup, Lookup::Works(_)) {
            POSITIVE_TTL_DAYS
        } else {
            NEGATIVE_TTL_DAYS
        };
        let db = self.db.clone();
        let stored_body = if status == 404 { Vec::new() } else { body };
        blocking(move || db.cache_answer(&key, status, &stored_body, ttl)).await?;
        Ok(lookup)
    }
}

fn decode(request: &WorkRequest, status: u16, body: &[u8]) -> Lookup {
    if status == 404 {
        return Lookup::NotFound;
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return Lookup::NotFound;
    };
    let works = match request {
        WorkRequest::Doi(_) => parse_work(&value).into_iter().collect(),
        WorkRequest::Title { .. } => parse_work_list(&value),
    };
    if works.is_empty() {
        Lookup::NotFound
    } else {
        Lookup::Works(works)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) const FIXTURE: &str = include_str!("fixtures/openalex.json");

    pub(crate) fn fixture(name: &str) -> Value {
        let all: Value = serde_json::from_str(FIXTURE).unwrap();
        all[name].clone()
    }

    /// Answers from recorded JSON by URL; anything else is 404. Counts requests.
    pub(crate) struct Replay {
        pub answers: HashMap<String, (u16, Vec<u8>)>,
        pub requests: AtomicUsize,
    }

    impl Replay {
        pub(crate) fn new(answers: Vec<(WorkRequest, u16, Value)>) -> Self {
            Self {
                answers: answers
                    .into_iter()
                    .map(|(r, s, v)| {
                        (
                            r.url().unwrap().to_string(),
                            (s, serde_json::to_vec(&v).unwrap()),
                        )
                    })
                    .collect(),
                requests: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ScholarlyTransport for Replay {
        async fn get(&self, url: &Url) -> Result<(u16, Vec<u8>), WebError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .answers
                .get(url.as_str())
                .cloned()
                .unwrap_or((404, b"<html>Not Found</html>".to_vec())))
        }
    }

    pub(crate) fn temp_db() -> (tempfile::TempDir, Arc<ResearchDb>) {
        let dir = tempfile::tempdir().unwrap();
        let db = ResearchDb::open(&dir.path().join("shodh.db"), None).unwrap();
        (dir, Arc::new(db))
    }

    #[test]
    fn works_parse_with_ids_authors_venue_and_counts() {
        let w = parse_work(&fixture("work_by_arxiv_doi")).unwrap();
        assert_eq!(w.id, "W4399554431");
        assert_eq!(w.arxiv_id.as_deref(), Some("2406.06484"));
        assert_eq!(w.doi.as_deref(), Some("10.48550/arxiv.2406.06484"));
        assert_eq!(w.year, Some(2024));
        assert_eq!(w.authors.first().map(String::as_str), Some("Songlin Yang"));
        let lstm = parse_work(&fixture("work_by_doi")).unwrap();
        assert_eq!(lstm.venue.as_deref(), Some("Neural Computation"));
        assert!(lstm.cited_by_count.unwrap_or(0) > 1000);
        assert_eq!(lstm.arxiv_id, None);
        let found = parse_work_list(&fixture("title_search"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].arxiv_id.as_deref(), Some("2102.11174"));
        assert_eq!(
            openalex_id("https://openalex.org/W123"),
            Some("W123".into())
        );
        assert_eq!(openalex_id("https://openalex.org/A123"), None);
    }

    #[test]
    fn requests_have_stable_keys_and_safe_filters() {
        let r = WorkRequest::arxiv("2406.06484");
        assert_eq!(r.cache_key(), "openalex:doi:10.48550/arxiv.2406.06484");
        assert!(r
            .url()
            .unwrap()
            .as_str()
            .starts_with("https://api.openalex.org/works/doi:10.48550/arxiv.2406.06484?select="));
        let t = WorkRequest::Title {
            title: "Linear transformers: secretly, fast | weight programmers".into(),
            year: Some(2021),
        };
        let url = t.url().unwrap();
        let filter = url
            .query_pairs()
            .find(|(k, _)| k == "filter")
            .map(|(_, v)| v.to_string())
            .unwrap();
        assert_eq!(
            filter,
            "title.search:linear transformers secretly fast weight programmers,publication_year:2020-2022"
        );
    }

    #[test]
    fn the_strict_rule_needs_title_year_and_first_author() {
        let work = parse_work_list(&fixture("title_search")).remove(0);
        let query = WorkQuery {
            title: Some("Linear transformers are secretly fast weight programmers".into()),
            year: Some(2021),
            first_surname: Some("schlag".into()),
        };
        assert!(strict_match(&query, &work).accepted());
        let off_by_two = WorkQuery {
            year: Some(2023),
            ..query.clone()
        };
        assert!(!strict_match(&off_by_two, &work).accepted());
        let other_author = WorkQuery {
            first_surname: Some("irie".into()),
            ..query.clone()
        };
        assert!(!strict_match(&other_author, &work).accepted());
        let other_title = WorkQuery {
            title: Some("Linear transformers are secretly kernel machines".into()),
            ..query.clone()
        };
        assert!(!strict_match(&other_title, &work).accepted());
        let no_year = WorkQuery {
            year: None,
            ..query
        };
        assert!(!strict_match(&no_year, &work).accepted());
        assert!(identifier_match(Some("Long short-term memory"), &work).accepted() == false);
        assert!(identifier_match(None, &work).accepted());
    }

    #[tokio::test]
    async fn answers_are_cached_including_not_found_and_429_halts_the_build() {
        let (_dir, db) = temp_db();
        let lstm = WorkRequest::Doi("10.1162/neco.1997.9.8.1735".into());
        let replay = Arc::new(Replay::new(vec![(
            lstm.clone(),
            200,
            fixture("work_by_doi"),
        )]));
        let resolver = Resolver::online(replay.clone(), db.clone()).with_limits(10, Duration::ZERO);
        assert!(matches!(resolver.lookup(&lstm).await.unwrap(), Lookup::Works(w) if w.len() == 1));
        let missing = WorkRequest::Doi("10.9999/none".into());
        assert_eq!(resolver.lookup(&missing).await.unwrap(), Lookup::NotFound);
        assert_eq!(replay.requests.load(Ordering::SeqCst), 2);
        // A second build reads both from the cache.
        let again = Resolver::online(replay.clone(), db.clone()).with_limits(10, Duration::ZERO);
        assert!(matches!(
            again.lookup(&lstm).await.unwrap(),
            Lookup::Works(_)
        ));
        assert_eq!(again.lookup(&missing).await.unwrap(), Lookup::NotFound);
        assert_eq!(replay.requests.load(Ordering::SeqCst), 2);
        assert_eq!(again.stats().await.cache_hits, 2);

        struct TooMany(AtomicUsize);
        #[async_trait::async_trait]
        impl ScholarlyTransport for TooMany {
            async fn get(&self, _url: &Url) -> Result<(u16, Vec<u8>), WebError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok((429, Vec::new()))
            }
        }
        let busy = Arc::new(TooMany(AtomicUsize::new(0)));
        let resolver = Resolver::online(busy.clone(), db).with_limits(10, Duration::ZERO);
        let a = WorkRequest::arxiv("2102.11174");
        let b = WorkRequest::arxiv("2411.12537");
        assert_eq!(
            resolver.lookup(&a).await.unwrap(),
            Lookup::Halted(Halt::RateLimited)
        );
        assert_eq!(
            resolver.lookup(&b).await.unwrap(),
            Lookup::Halted(Halt::RateLimited)
        );
        assert_eq!(busy.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_cache_only_resolver_sends_nothing_and_reads_the_cache() {
        let (_dir, db) = temp_db();
        let lstm = WorkRequest::Doi("10.1162/neco.1997.9.8.1735".into());
        let offline = Resolver::cache_only(db.clone());
        assert!(!offline.is_online());
        assert_eq!(
            offline.lookup(&lstm).await.unwrap(),
            Lookup::Halted(Halt::Offline)
        );
        db.cache_answer(
            &lstm.cache_key(),
            200,
            &serde_json::to_vec(&fixture("work_by_doi")).unwrap(),
            30,
        )
        .unwrap();
        assert!(matches!(
            offline.lookup(&lstm).await.unwrap(),
            Lookup::Works(_)
        ));
        assert_eq!(offline.stats().await.requests, 0);
    }

    #[tokio::test]
    async fn the_budget_caps_requests_per_build() {
        let (_dir, db) = temp_db();
        let replay = Arc::new(Replay::new(Vec::new()));
        let resolver = Resolver::online(replay.clone(), db).with_limits(1, Duration::ZERO);
        assert_eq!(
            resolver
                .lookup(&WorkRequest::arxiv("2102.11174"))
                .await
                .unwrap(),
            Lookup::NotFound
        );
        assert_eq!(
            resolver
                .lookup(&WorkRequest::arxiv("2411.12537"))
                .await
                .unwrap(),
            Lookup::Halted(Halt::Budget)
        );
        assert_eq!(replay.requests.load(Ordering::SeqCst), 1);
    }
}
