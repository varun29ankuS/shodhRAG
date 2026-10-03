//! Scholarly search over free, open APIs only: arXiv, OpenAlex and
//! Semantic Scholar (keyless public endpoint). Nothing is scraped, and only
//! open-access PDF links the APIs report are passed on; paywalled or shadow
//! library copies are never looked up.
//!
//! Each API matches loosely (any of the query's words), so a search runs in
//! three stages:
//! 1. focused queries ([`paper_requests`]): arXiv gets a field-scoped AND of
//!    the significant terms, plus an exact-title clause when the query looks
//!    like a paper title; OpenAlex gets its relevance search and, for a
//!    title, an exact title filter; Semantic Scholar keeps its relevance
//!    order;
//! 2. merging ([`merge`]): records of the same paper (same DOI, same arXiv id
//!    or near-identical title) become one, keeping the richest fields and
//!    every source;
//! 3. ranking ([`rank_papers`]): each paper is scored against the query by
//!    the local cross-encoder (or lexically when it is not installed) and
//!    weak matches are dropped. An exact title match is always kept.

use serde::Serialize;
use serde_json::Value;
use url::Url;

use super::client::{SafeClient, WebError};
use super::relevance::{
    is_stopword, is_title_match, rank, rank_prior, title_jaccard, words, Candidate, RankMethod,
    SharedScorer, PAPER_THRESHOLDS,
};

pub const ARXIV_API: &str = "https://export.arxiv.org/api/query";
pub const OPENALEX_API: &str = "https://api.openalex.org/works";
pub const SEMANTIC_SCHOLAR_API: &str = "https://api.semanticscholar.org/graph/v1/paper/search";

const MAX_API_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ABSTRACT_CHARS: usize = 600;
const MAX_AUTHORS: usize = 8;
/// Most terms ANDed in an arXiv query; more makes a title query match
/// nothing when one word differs.
const MAX_ARXIV_TERMS: usize = 8;
/// Results asked of OpenAlex's exact-title filter.
const TITLE_FILTER_RESULTS: usize = 5;
/// Fields OpenAlex returns (keeps answers small).
const OPENALEX_SELECT: &str = "id,doi,display_name,publication_year,authorships,\
primary_location,best_oa_location,open_access,abstract_inverted_index,relevance_score";
const SEMANTIC_SCHOLAR_FIELDS: &str =
    "title,authors,year,venue,abstract,externalIds,openAccessPdf,url";
/// arXiv's DOI prefix: such a DOI names the preprint, not a journal version.
const ARXIV_DOI_PREFIX: &str = "10.48550/arxiv.";

pub const ARXIV: &str = "arXiv";
pub const OPENALEX: &str = "OpenAlex";
pub const SEMANTIC_SCHOLAR: &str = "Semantic Scholar";

/// One paper.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paper {
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
    pub abstract_snippet: Option<String>,
    pub doi: Option<String>,
    /// arXiv identifier without its version (`1706.03762`).
    pub arxiv_id: Option<String>,
    pub landing_url: Option<String>,
    /// An open-access PDF reported by the API.
    pub pdf_url: Option<String>,
    /// Which APIs found it.
    pub sources: Vec<&'static str>,
    /// Relevance to the query in 0..=1, once ranked.
    pub relevance: Option<f32>,
    /// The paper's title is (nearly) the title the query named.
    pub title_match: bool,
    /// The API's own relevance score (OpenAlex), for ordering only.
    #[serde(skip)]
    pub source_score: Option<f64>,
}

impl Paper {
    /// The text a query is scored against.
    pub fn relevance_text(&self) -> String {
        match &self.abstract_snippet {
            Some(a) => format!("{}. {a}", self.title),
            None => self.title.clone(),
        }
    }

    /// The best link to the paper.
    pub fn link(&self) -> String {
        self.landing_url
            .clone()
            .or_else(|| self.doi.as_ref().map(|d| format!("https://doi.org/{d}")))
            .or_else(|| {
                self.arxiv_id
                    .as_ref()
                    .map(|id| format!("https://arxiv.org/abs/{id}"))
            })
            .or_else(|| self.pdf_url.clone())
            .unwrap_or_default()
    }
}

fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn snippet(text: &str) -> Option<String> {
    let text = clean(text);
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= MAX_ABSTRACT_CHARS {
        return Some(text);
    }
    let mut out: String = text.chars().take(MAX_ABSTRACT_CHARS - 1).collect();
    out.push('…');
    Some(out)
}

/// `10.1234/abc` from a DOI or DOI URL, lower-cased.
pub fn normalize_doi(raw: &str) -> Option<String> {
    let lower = raw.trim().to_ascii_lowercase();
    let doi = [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
    ]
    .iter()
    .find_map(|p| lower.strip_prefix(p))
    .unwrap_or(&lower)
    .trim();
    doi.starts_with("10.").then(|| doi.to_string())
}

fn is_new_style_arxiv_id(id: &str) -> bool {
    let Some((yymm, number)) = id.split_once('.') else {
        return false;
    };
    yymm.len() == 4
        && yymm.chars().all(|c| c.is_ascii_digit())
        && (4..=5).contains(&number.len())
        && number.chars().all(|c| c.is_ascii_digit())
}

fn is_old_style_arxiv_id(id: &str) -> bool {
    let Some((archive, number)) = id.split_once('/') else {
        return false;
    };
    !archive.is_empty()
        && archive
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-' || c == '.')
        && number.len() == 7
        && number.chars().all(|c| c.is_ascii_digit())
}

/// The arXiv id (lower-case, version stripped) in an id, `arXiv:` id,
/// arxiv.org abs/pdf URL or arXiv DOI.
pub fn normalize_arxiv_id(raw: &str) -> Option<String> {
    let lower = raw.trim().to_ascii_lowercase();
    let rest = ["arxiv.org/abs/", "arxiv.org/pdf/", ARXIV_DOI_PREFIX]
        .iter()
        .find_map(|marker| lower.find(marker).map(|i| &lower[i + marker.len()..]))
        .or_else(|| lower.strip_prefix("arxiv:"))
        .unwrap_or(&lower);
    let rest = rest
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .trim_end_matches(".pdf");
    let id = match rest.rfind('v') {
        Some(i)
            if i + 1 < rest.len()
                && rest[i + 1..].chars().all(|c| c.is_ascii_digit())
                && rest[..i].ends_with(|c: char| c.is_ascii_digit()) =>
        {
            &rest[..i]
        }
        _ => rest,
    };
    (is_new_style_arxiv_id(id) || is_old_style_arxiv_id(id)).then(|| id.to_string())
}

fn https_only(url: &str) -> Option<String> {
    let url = url.trim();
    Url::parse(url)
        .ok()
        .filter(|u| matches!(u.scheme(), "https" | "http"))
        .map(|u| u.to_string())
}

fn new_paper(title: String, source: &'static str) -> Paper {
    Paper {
        title,
        authors: Vec::new(),
        year: None,
        venue: None,
        abstract_snippet: None,
        doi: None,
        arxiv_id: None,
        landing_url: None,
        pdf_url: None,
        sources: vec![source],
        relevance: None,
        title_match: false,
        source_score: None,
    }
}

// ── query construction ────────────────────────────────────────────────────

/// Quoted phrases in `query` (`"..."`), as lower-case words.
fn quoted_phrases(query: &str) -> Vec<String> {
    query
        .split('"')
        .skip(1)
        .step_by(2)
        .map(|p| words(p).join(" "))
        .filter(|p| !p.is_empty())
        .collect()
}

/// `query` with its quoted phrases removed.
fn unquoted(query: &str) -> String {
    query.split('"').step_by(2).collect::<Vec<_>>().join(" ")
}

/// The paper title `query` names, if it looks like one: a quoted phrase of
/// at least four words, or at least four words in title case once a
/// lower-case lead-in ("summarize the paper …") is dropped. Returned as
/// lower-case words without punctuation, ready for a phrase search.
pub fn title_in_query(query: &str) -> Option<String> {
    if let Some(phrase) = quoted_phrases(query)
        .into_iter()
        .find(|p| p.split(' ').count() >= 4)
    {
        return Some(phrase);
    }
    let tokens: Vec<&str> = query.split_whitespace().collect();
    let start = tokens.iter().position(|t| {
        t.chars()
            .find(|c| c.is_alphanumeric())
            .is_some_and(char::is_uppercase)
    })?;
    let candidate = tokens[start..].join(" ");
    let all_words: Vec<&str> = candidate
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if all_words.len() < 4 {
        return None;
    }
    let significant: Vec<&&str> = all_words
        .iter()
        .filter(|w| !is_stopword(&w.to_lowercase()))
        .collect();
    let capitalised = significant
        .iter()
        .filter(|w| w.starts_with(char::is_uppercase))
        .count();
    if significant.is_empty() || (capitalised as f32) < 0.7 * significant.len() as f32 {
        return None;
    }
    Some(words(&candidate).join(" "))
}

/// The arXiv `search_query` for `query`: every significant term must occur
/// in the title or abstract (`(ti:x OR abs:x) AND …`), quoted phrases stay
/// phrases, and a title-like query also matches its exact title
/// (`ti:"…" OR (…)`). Only letters, digits and spaces of the query reach
/// the search, so arXiv's syntax (quotes, parentheses, field prefixes) cannot
/// be injected, and its boolean words are stopwords. `None` when the query
/// has no significant term.
pub fn arxiv_search_query(query: &str) -> Option<String> {
    let phrases = quoted_phrases(query);
    let mut terms: Vec<String> = Vec::new();
    for word in words(&unquoted(query)) {
        if !is_stopword(&word) && !terms.contains(&word) {
            terms.push(word);
        }
    }
    if terms.len() > MAX_ARXIV_TERMS {
        // Keep the longest (most specific) terms, in query order.
        let mut by_len: Vec<(usize, usize)> = terms
            .iter()
            .enumerate()
            .map(|(i, t)| (i, t.len()))
            .collect();
        by_len.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut keep: Vec<usize> = by_len
            .into_iter()
            .take(MAX_ARXIV_TERMS)
            .map(|(i, _)| i)
            .collect();
        keep.sort_unstable();
        terms = keep
            .into_iter()
            .filter_map(|i| terms.get(i).cloned())
            .collect();
    }
    let clauses: Vec<String> = phrases
        .iter()
        .map(|p| format!("(ti:\"{p}\" OR abs:\"{p}\")"))
        .chain(terms.iter().map(|t| format!("(ti:{t} OR abs:{t})")))
        .collect();
    if clauses.is_empty() {
        return None;
    }
    let and = clauses.join(" AND ");
    Some(match title_in_query(query) {
        Some(title) => format!("ti:\"{title}\" OR ({and})"),
        None => and,
    })
}

/// The requests one paper search sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaperRequests {
    /// `None` when the query has no significant term for arXiv.
    pub arxiv: Option<Url>,
    pub openalex: Url,
    /// OpenAlex exact-title filter, for title-like queries.
    pub openalex_title: Option<Url>,
    pub semantic_scholar: Url,
}

fn api_url(base: &str) -> Result<Url, WebError> {
    Url::parse(base).map_err(|e| WebError::Http(e.to_string()))
}

/// Build the requests for `query`, asking each API for `per_source` results.
pub fn paper_requests(query: &str, per_source: usize) -> Result<PaperRequests, WebError> {
    let per_source = per_source.to_string();
    let arxiv = match arxiv_search_query(query) {
        Some(q) => {
            let mut url = api_url(ARXIV_API)?;
            url.query_pairs_mut()
                .append_pair("search_query", &q)
                .append_pair("start", "0")
                .append_pair("max_results", &per_source);
            Some(url)
        }
        None => None,
    };
    let mut openalex = api_url(OPENALEX_API)?;
    openalex
        .query_pairs_mut()
        .append_pair("search", &clean(&query.replace('"', " ")))
        .append_pair("per-page", &per_source)
        .append_pair("select", OPENALEX_SELECT);
    let openalex_title = match title_in_query(query) {
        Some(title) => {
            // Filter values are split on ',' and '|'; `title` holds only
            // letters, digits and spaces.
            let mut url = api_url(OPENALEX_API)?;
            url.query_pairs_mut()
                .append_pair("filter", &format!("title.search:{title}"))
                .append_pair("per-page", &TITLE_FILTER_RESULTS.to_string())
                .append_pair("select", OPENALEX_SELECT);
            Some(url)
        }
        None => None,
    };
    // Semantic Scholar has no query syntax and matches nothing for
    // hyphenated terms, so it gets plain words.
    let mut semantic_scholar = api_url(SEMANTIC_SCHOLAR_API)?;
    semantic_scholar
        .query_pairs_mut()
        .append_pair("query", &words(query).join(" "))
        .append_pair("limit", &per_source)
        .append_pair("fields", SEMANTIC_SCHOLAR_FIELDS);
    Ok(PaperRequests {
        arxiv,
        openalex,
        openalex_title,
        semantic_scholar,
    })
}

// ── parsing ───────────────────────────────────────────────────────────────

/// Parse an arXiv API Atom feed.
pub fn parse_arxiv(xml: &str) -> Result<Vec<Paper>, WebError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| WebError::Http(format!("arXiv returned invalid XML: {e}")))?;
    let child_text = |node: roxmltree::Node, name: &str| -> Option<String> {
        node.children()
            .find(|c| c.is_element() && c.tag_name().name() == name)
            .and_then(|c| c.text())
            .map(clean)
            .filter(|t| !t.is_empty())
    };
    let mut papers = Vec::new();
    for entry in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "entry")
    {
        let Some(title) = child_text(entry, "title") else {
            continue;
        };
        let mut p = new_paper(title, ARXIV);
        p.authors = entry
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "author")
            .filter_map(|a| child_text(a, "name"))
            .take(MAX_AUTHORS)
            .collect();
        p.year =
            child_text(entry, "published").and_then(|d| d.get(0..4).and_then(|y| y.parse().ok()));
        for link in entry
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "link")
        {
            let href = link.attribute("href").and_then(https_only);
            match (link.attribute("title"), link.attribute("rel")) {
                (Some("pdf"), _) => p.pdf_url = href,
                (_, Some("alternate")) => p.landing_url = href,
                _ => {}
            }
        }
        let id = child_text(entry, "id");
        if p.landing_url.is_none() {
            p.landing_url = id.as_deref().and_then(https_only);
        }
        p.arxiv_id = id.as_deref().and_then(normalize_arxiv_id);
        p.venue = child_text(entry, "journal_ref").or_else(|| Some(ARXIV.to_string()));
        p.abstract_snippet = child_text(entry, "summary").and_then(|s| snippet(&s));
        p.doi = child_text(entry, "doi").and_then(|d| normalize_doi(&d));
        papers.push(p);
    }
    Ok(papers)
}

/// Rebuild an OpenAlex abstract from its inverted index.
fn openalex_abstract(index: &Value) -> Option<String> {
    let map = index.as_object()?;
    let mut words: Vec<(u64, &str)> = Vec::new();
    for (word, positions) in map {
        for p in positions.as_array()?.iter().filter_map(Value::as_u64) {
            words.push((p, word.as_str()));
        }
    }
    words.sort_unstable();
    let text = words
        .into_iter()
        .map(|(_, w)| w)
        .collect::<Vec<_>>()
        .join(" ");
    snippet(&text)
}

/// Parse an OpenAlex `/works` search answer.
pub fn parse_openalex(response: &Value) -> Result<Vec<Paper>, WebError> {
    let results = response
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| WebError::Http("OpenAlex returned no results list".to_string()))?;
    let s = |v: &Value, ptr: &str| {
        v.pointer(ptr)
            .and_then(Value::as_str)
            .map(clean)
            .filter(|t| !t.is_empty())
    };
    Ok(results
        .iter()
        .filter_map(|w| {
            let title = s(w, "/display_name").or_else(|| s(w, "/title"))?;
            let mut p = new_paper(title, OPENALEX);
            p.authors = w
                .get("authorships")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| s(x, "/author/display_name"))
                        .take(MAX_AUTHORS)
                        .collect()
                })
                .unwrap_or_default();
            p.pdf_url = s(w, "/best_oa_location/pdf_url")
                .or_else(|| s(w, "/open_access/oa_url"))
                .and_then(|u| https_only(&u));
            p.year = w
                .get("publication_year")
                .and_then(Value::as_i64)
                .and_then(|y| i32::try_from(y).ok());
            p.venue = s(w, "/primary_location/source/display_name");
            p.abstract_snippet = w.get("abstract_inverted_index").and_then(openalex_abstract);
            p.doi = s(w, "/doi").and_then(|d| normalize_doi(&d));
            p.landing_url = s(w, "/primary_location/landing_page_url")
                .or_else(|| s(w, "/doi"))
                .and_then(|u| https_only(&u));
            p.arxiv_id = p
                .doi
                .as_deref()
                .and_then(normalize_arxiv_id)
                .or_else(|| p.landing_url.as_deref().and_then(normalize_arxiv_id))
                .or_else(|| p.pdf_url.as_deref().and_then(normalize_arxiv_id));
            p.source_score = w.get("relevance_score").and_then(Value::as_f64);
            Some(p)
        })
        .collect())
}

/// Parse a Semantic Scholar paper search answer.
pub fn parse_semantic_scholar(response: &Value) -> Result<Vec<Paper>, WebError> {
    let data = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| WebError::Http("Semantic Scholar returned no data list".to_string()))?;
    let s = |v: &Value, ptr: &str| {
        v.pointer(ptr)
            .and_then(Value::as_str)
            .map(clean)
            .filter(|t| !t.is_empty())
    };
    Ok(data
        .iter()
        .filter_map(|item| {
            let title = s(item, "/title")?;
            let mut p = new_paper(title, SEMANTIC_SCHOLAR);
            p.authors = item
                .get("authors")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| s(x, "/name"))
                        .take(MAX_AUTHORS)
                        .collect()
                })
                .unwrap_or_default();
            p.arxiv_id = s(item, "/externalIds/ArXiv").and_then(|id| normalize_arxiv_id(&id));
            let arxiv_pdf = p
                .arxiv_id
                .as_ref()
                .map(|id| format!("https://arxiv.org/pdf/{id}"));
            p.year = item
                .get("year")
                .and_then(Value::as_i64)
                .and_then(|y| i32::try_from(y).ok());
            p.venue = s(item, "/venue");
            p.abstract_snippet = s(item, "/abstract").and_then(|a| snippet(&a));
            p.doi = s(item, "/externalIds/DOI").and_then(|d| normalize_doi(&d));
            p.landing_url = s(item, "/url").and_then(|u| https_only(&u));
            p.pdf_url = s(item, "/openAccessPdf/url")
                .or(arxiv_pdf)
                .and_then(|u| https_only(&u));
            Some(p)
        })
        .collect())
}

// ── merging ───────────────────────────────────────────────────────────────

/// Word-set Jaccard at which two titles name the same paper.
const SAME_TITLE_JACCARD: f32 = 0.9;

fn journal_doi(p: &Paper) -> Option<&str> {
    p.doi
        .as_deref()
        .filter(|d| !d.starts_with(ARXIV_DOI_PREFIX))
}

/// Whether two records describe the same paper. Different arXiv ids or
/// different journal DOIs mean different papers even under one title. A
/// preprint and its journal version share a title and may carry an arXiv
/// DOI and a journal DOI, so an arXiv DOI never blocks a match.
fn same_paper(a: &Paper, b: &Paper) -> bool {
    if let (Some(x), Some(y)) = (&a.arxiv_id, &b.arxiv_id) {
        return x == y;
    }
    if a.doi.is_some() && a.doi == b.doi {
        return true;
    }
    if let (Some(x), Some(y)) = (journal_doi(a), journal_doi(b)) {
        if x != y {
            return false;
        }
    }
    title_jaccard(&a.title, &b.title) >= SAME_TITLE_JACCARD
}

/// Fold `other` into `into`, keeping the richest value of each field.
fn absorb(into: &mut Paper, other: Paper) {
    let other_has_journal_doi = journal_doi(&other).is_some();
    if other.authors.len() > into.authors.len() {
        into.authors = other.authors;
    }
    into.year = into.year.or(other.year);
    let real_venue = |v: &Option<String>| v.as_deref().is_some_and(|v| v != ARXIV);
    if !real_venue(&into.venue) && (real_venue(&other.venue) || into.venue.is_none()) {
        into.venue = other.venue;
    }
    let len = |a: &Option<String>| a.as_deref().map_or(0, |s| s.chars().count());
    if len(&other.abstract_snippet) > len(&into.abstract_snippet) {
        into.abstract_snippet = other.abstract_snippet;
    }
    if journal_doi(into).is_none() && (other_has_journal_doi || into.doi.is_none()) {
        into.doi = other.doi.or(into.doi.take());
    }
    into.arxiv_id = into.arxiv_id.take().or(other.arxiv_id);
    into.landing_url = into.landing_url.take().or(other.landing_url);
    into.pdf_url = into.pdf_url.take().or(other.pdf_url);
    for source in other.sources {
        if !into.sources.contains(&source) {
            into.sources.push(source);
        }
    }
    into.source_score = match (into.source_score, other.source_score) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
}

fn find_root(parent: &mut [usize], i: usize) -> usize {
    let mut root = i;
    while parent[root] != root {
        root = parent[root];
    }
    let mut node = i;
    while parent[node] != root {
        let next = parent[node];
        parent[node] = root;
        node = next;
    }
    root
}

/// A merged paper with the sources' best opinion of it (0..=1).
#[derive(Debug, Clone, PartialEq)]
pub struct MergedPaper {
    pub paper: Paper,
    pub prior: f32,
}

/// Merge result lists (each in its API's relevance order) into one record
/// per paper. Records match by DOI, arXiv id or near-identical title,
/// transitively. The lists are interleaved so each API's best hits come
/// first; a record's prior is its best rank across the lists, for OpenAlex
/// blended with its relevance score relative to the list's top hit.
pub fn merge(lists: Vec<Vec<Paper>>) -> Vec<MergedPaper> {
    let max_len = lists.iter().map(Vec::len).max().unwrap_or(0);
    let tops: Vec<Option<f64>> = lists
        .iter()
        .map(|l| l.iter().filter_map(|p| p.source_score).reduce(f64::max))
        .collect();
    let mut items: Vec<(Paper, f32)> = Vec::new();
    for i in 0..max_len {
        for (list, top) in lists.iter().zip(&tops) {
            let Some(p) = list.get(i) else { continue };
            let mut prior = rank_prior(i);
            if let (Some(score), Some(top)) = (p.source_score, top) {
                if *top > 0.0 {
                    prior = 0.5 * prior + 0.5 * (score / top) as f32;
                }
            }
            items.push((p.clone(), prior));
        }
    }
    let mut parent: Vec<usize> = (0..items.len()).collect();
    // Identifiers per group (indexed by root), so a chain of matches cannot
    // join two records with different arXiv ids or journal DOIs.
    let mut ids: Vec<(Option<String>, Option<String>)> = items
        .iter()
        .map(|(p, _)| (p.arxiv_id.clone(), journal_doi(p).map(str::to_string)))
        .collect();
    let conflict =
        |x: &Option<String>, y: &Option<String>| matches!((x, y), (Some(a), Some(b)) if a != b);
    for i in 0..items.len() {
        for j in (i + 1)..items.len() {
            if !same_paper(&items[i].0, &items[j].0) {
                continue;
            }
            let (a, b) = (find_root(&mut parent, i), find_root(&mut parent, j));
            if a == b || conflict(&ids[a].0, &ids[b].0) || conflict(&ids[a].1, &ids[b].1) {
                continue;
            }
            // The earlier record is the root, so it stays first.
            let (root, child) = (a.min(b), a.max(b));
            parent[child] = root;
            let (arxiv, doi) = std::mem::take(&mut ids[child]);
            let group = &mut ids[root];
            group.0 = group.0.take().or(arxiv);
            group.1 = group.1.take().or(doi);
        }
    }
    let mut out: Vec<MergedPaper> = Vec::new();
    let mut slot: Vec<Option<usize>> = vec![None; items.len()];
    for (i, (paper, prior)) in items.into_iter().enumerate() {
        let root = find_root(&mut parent, i);
        match slot[root].and_then(|k| out.get_mut(k)) {
            Some(merged) => {
                merged.prior = merged.prior.max(prior);
                absorb(&mut merged.paper, paper);
            }
            None => {
                slot[root] = Some(out.len());
                out.push(MergedPaper { paper, prior });
            }
        }
    }
    out
}

// ── ranking and search ────────────────────────────────────────────────────

/// What a paper search found.
#[derive(Debug, Clone)]
pub struct PaperSearch {
    /// Relevant papers, best first.
    pub papers: Vec<Paper>,
    /// Papers the APIs returned that were dropped as irrelevant.
    pub dropped: usize,
    pub method: RankMethod,
    /// APIs that failed, with why.
    pub failures: Vec<String>,
}

/// Score `merged` against `query`, drop weak matches and keep the best
/// `limit`. Returns the papers, how many were dropped as irrelevant, and the
/// scoring method. Blocking when `scorer` is set (runs the cross-encoder).
pub fn rank_papers(
    query: &str,
    merged: Vec<MergedPaper>,
    scorer: Option<&SharedScorer>,
    limit: usize,
) -> (Vec<Paper>, usize, RankMethod) {
    let candidates: Vec<Candidate<Paper>> = merged
        .into_iter()
        .map(|m| Candidate {
            text: m.paper.relevance_text(),
            title_match: is_title_match(query, &m.paper.title),
            prior: m.prior,
            item: m.paper,
        })
        .collect();
    let ranking = rank(
        query,
        candidates,
        scorer.map(|s| s.as_ref()),
        PAPER_THRESHOLDS,
        true,
    );
    let papers = ranking
        .kept
        .into_iter()
        .take(limit)
        .map(|s| {
            let mut p = s.item;
            p.relevance = Some(s.relevance);
            p.title_match = s.title_match;
            p
        })
        .collect();
    (papers, ranking.dropped, ranking.method)
}

async fn fetch_arxiv(client: &SafeClient, url: Url) -> Result<Vec<Paper>, WebError> {
    let fetched = client.get(url.as_str(), vec![]).await?;
    let status = fetched.status;
    let bytes = fetched.read_capped(MAX_API_BYTES).await?;
    if !(200..300).contains(&status) {
        return Err(WebError::Status { status });
    }
    parse_arxiv(&String::from_utf8_lossy(&bytes))
}

async fn fetch_json(
    client: &SafeClient,
    url: Url,
    parse: fn(&Value) -> Result<Vec<Paper>, WebError>,
) -> Result<Vec<Paper>, WebError> {
    let value = client.get_json(url.as_str(), vec![], MAX_API_BYTES).await?;
    parse(&value)
}

/// Search every API concurrently, merge, rank and cut to `limit`. Fails only
/// when every API failed.
pub async fn search_papers(
    client: &SafeClient,
    query: &str,
    limit: usize,
    scorer: Option<SharedScorer>,
) -> Result<PaperSearch, WebError> {
    let limit = limit.max(1);
    // Ask for more than the limit: ranking drops some.
    let per_source = (limit + limit / 2).clamp(5, 25);
    let requests = paper_requests(query, per_source)?;
    let arxiv = async {
        match requests.arxiv.clone() {
            Some(url) => fetch_arxiv(client, url).await.map(Some),
            None => Ok(None),
        }
    };
    let openalex_title = async {
        match requests.openalex_title.clone() {
            Some(url) => fetch_json(client, url, parse_openalex).await.map(Some),
            None => Ok(None),
        }
    };
    let openalex = fetch_json(client, requests.openalex.clone(), parse_openalex);
    let semantic = fetch_json(
        client,
        requests.semantic_scholar.clone(),
        parse_semantic_scholar,
    );
    let (a, ot, o, s) = tokio::join!(arxiv, openalex_title, openalex, semantic);
    let mut lists = Vec::new();
    let mut failures = Vec::new();
    for (name, result) in [
        (ARXIV, a),
        ("OpenAlex title search", ot),
        (OPENALEX, o.map(Some)),
        (SEMANTIC_SCHOLAR, s.map(Some)),
    ] {
        match result {
            Ok(Some(list)) => lists.push(list),
            Ok(None) => {}
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    if lists.is_empty() {
        return Err(WebError::Http(format!(
            "Every scholarly search failed: {}",
            failures.join("; ")
        )));
    }
    let merged = merge(lists);
    let fallback = merged.clone();
    let owned_query = query.to_string();
    let ranked = tokio::task::spawn_blocking(move || {
        rank_papers(&owned_query, merged, scorer.as_ref(), limit)
    })
    .await;
    let (papers, dropped, method) = match ranked {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(target: "shodh::harness", error = %e, "paper ranking failed; using keyword overlap");
            rank_papers(query, fallback, None, limit)
        }
    };
    Ok(PaperSearch {
        papers,
        dropped,
        method,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARXIV_FIXTURE: &str = include_str!("../fixtures/arxiv-query.xml");
    const OPENALEX_FIXTURE: &str = include_str!("../fixtures/openalex-works.json");
    const SEMANTIC_FIXTURE: &str = include_str!("../fixtures/semantic-scholar-search.json");

    const DIFFUSING_BLAME: &str = "Diffusing Blame: Task-Dependent Credit Assignment in \
                                   Biologically Plausible Dual-Stream Networks";

    fn query_param(url: &Url, key: &str) -> String {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
            .unwrap()
    }

    #[test]
    fn arxiv_query_ands_significant_terms_in_title_or_abstract() {
        assert_eq!(
            arxiv_search_query("papers about the linear attention delta rule").as_deref(),
            Some(
                "(ti:linear OR abs:linear) AND (ti:attention OR abs:attention) AND \
                 (ti:delta OR abs:delta) AND (ti:rule OR abs:rule)"
            )
        );
        assert_eq!(arxiv_search_query("the of and"), None);
    }

    #[test]
    fn arxiv_query_keeps_quoted_phrases_and_adds_an_exact_title_clause() {
        assert_eq!(
            arxiv_search_query("\"state space\" models").as_deref(),
            Some("(ti:\"state space\" OR abs:\"state space\") AND (ti:models OR abs:models)")
        );
        let q = arxiv_search_query(DIFFUSING_BLAME).unwrap();
        assert!(
            q.starts_with(
                "ti:\"diffusing blame task dependent credit assignment in biologically \
                 plausible dual stream networks\" OR (("
            ),
            "{q}"
        );
        // At most eight terms are ANDed: the longest, in query order.
        let and = q.split_once(" OR (").unwrap().1;
        assert_eq!(and.matches("(ti:").count(), MAX_ARXIV_TERMS);
        assert!(and.starts_with("(ti:diffusing OR abs:diffusing) AND (ti:dependent"));
    }

    #[test]
    fn arxiv_query_cannot_be_injected() {
        let q = arxiv_search_query("x) OR au:\"evil\" ANDNOT (ti:* cat:cs.AI").unwrap();
        assert!(!q.contains("au:") && !q.contains("cat:") && !q.contains('*'));
        assert!(!q.contains("ANDNOT"), "boolean words are dropped: {q}");
        assert_eq!(
            q,
            "(ti:\"evil\" OR abs:\"evil\") AND (ti:x OR abs:x) AND (ti:au OR abs:au) AND \
             (ti:ti OR abs:ti) AND (ti:cat OR abs:cat) AND (ti:cs OR abs:cs) AND (ti:ai OR abs:ai)"
        );
        // Every quote and parenthesis is one the builder wrote.
        assert_eq!(q.matches('(').count(), q.matches(')').count());
    }

    #[test]
    fn titles_are_recognised_with_or_without_a_lead_in() {
        let plain = "diffusing blame task dependent credit assignment in biologically plausible \
                     dual stream networks";
        assert_eq!(title_in_query(DIFFUSING_BLAME).as_deref(), Some(plain));
        assert_eq!(
            title_in_query(&format!("summarize the paper {DIFFUSING_BLAME}")).as_deref(),
            Some(plain)
        );
        assert_eq!(
            title_in_query("find \"attention is all you need\" please").as_deref(),
            Some("attention is all you need")
        );
        assert_eq!(title_in_query("linear attention delta rule"), None);
        assert_eq!(title_in_query("What is BERT?"), None, "too short");
    }

    #[test]
    fn openalex_gets_relevance_search_and_a_title_filter_for_titles() {
        let r = paper_requests(&format!("{DIFFUSING_BLAME}, part 2|x"), 12).unwrap();
        assert_eq!(query_param(&r.openalex, "per-page"), "12");
        assert!(query_param(&r.openalex, "select").contains("relevance_score"));
        let filter = query_param(r.openalex_title.as_ref().unwrap(), "filter");
        assert!(filter.starts_with("title.search:diffusing blame task"));
        assert!(!filter.contains(',') && !filter.contains('|'));
        assert_eq!(
            query_param(&r.semantic_scholar, "query"),
            "diffusing blame task dependent credit assignment in biologically plausible dual \
             stream networks part 2 x",
            "hyphens become spaces for Semantic Scholar"
        );
        let topic = paper_requests("linear attention delta rule", 5).unwrap();
        assert!(topic.openalex_title.is_none());
        assert!(topic.arxiv.is_some());
    }

    #[test]
    fn identifiers_normalise() {
        assert_eq!(
            normalize_doi("https://doi.org/10.1000/ABC").as_deref(),
            Some("10.1000/abc")
        );
        assert_eq!(normalize_doi("doi:10.1/x").as_deref(), Some("10.1/x"));
        assert_eq!(normalize_doi("not a doi"), None);
        for raw in [
            "2101.00001v3",
            "arXiv:2101.00001",
            "https://arxiv.org/abs/2101.00001v2",
            "http://arxiv.org/pdf/2101.00001v1.pdf",
            "10.48550/arXiv.2101.00001",
            "https://doi.org/10.48550/arxiv.2101.00001",
        ] {
            assert_eq!(
                normalize_arxiv_id(raw).as_deref(),
                Some("2101.00001"),
                "{raw}"
            );
        }
        assert_eq!(
            normalize_arxiv_id("http://arxiv.org/abs/hep-th/9901001v2").as_deref(),
            Some("hep-th/9901001")
        );
        assert_eq!(normalize_arxiv_id("https://example.org/abs/1"), None);
    }

    fn rec(title: &str, source: &'static str) -> Paper {
        new_paper(title.to_string(), source)
    }

    #[test]
    fn merge_joins_by_doi_arxiv_id_and_fuzzy_title_keeping_the_richest_fields() {
        let mut a = rec("Deep Residual Learning for Image Recognition", ARXIV);
        a.arxiv_id = Some("1512.03385".into());
        a.doi = Some("10.48550/arxiv.1512.03385".into());
        a.venue = Some(ARXIV.into());
        a.abstract_snippet = Some("short".into());
        a.pdf_url = Some("https://arxiv.org/pdf/1512.03385".into());
        // Same paper, journal version: arXiv DOI vs journal DOI must not
        // block the title match; punctuation and case differ.
        let mut b = rec("Deep residual learning for image recognition.", OPENALEX);
        b.doi = Some("10.1109/cvpr.2016.90".into());
        b.venue = Some("CVPR".into());
        b.abstract_snippet = Some("a much longer abstract".into());
        b.authors = vec!["Kaiming He".into(), "Xiangyu Zhang".into()];
        b.source_score = Some(50.0);
        // Joins `b` by DOI only (title differs), so transitively joins `a`.
        let mut c = rec("ResNet", SEMANTIC_SCHOLAR);
        c.doi = Some("10.1109/cvpr.2016.90".into());
        c.year = Some(2016);
        // Same title but a different arXiv id: a different paper.
        let mut d = rec(
            "Deep Residual Learning for Image Recognition",
            SEMANTIC_SCHOLAR,
        );
        d.arxiv_id = Some("9999.99999".into());

        let merged = merge(vec![vec![a], vec![b], vec![c, d]]);
        assert_eq!(merged.len(), 2, "{merged:#?}");
        let p = &merged[0].paper;
        assert_eq!(p.sources, vec![ARXIV, OPENALEX, SEMANTIC_SCHOLAR]);
        assert_eq!(p.doi.as_deref(), Some("10.1109/cvpr.2016.90"));
        assert_eq!(p.arxiv_id.as_deref(), Some("1512.03385"));
        assert_eq!(p.venue.as_deref(), Some("CVPR"));
        assert_eq!(
            p.abstract_snippet.as_deref(),
            Some("a much longer abstract")
        );
        assert_eq!(p.authors.len(), 2);
        assert_eq!(p.year, Some(2016));
        assert!(p.pdf_url.is_some());
        assert_eq!(merged[0].prior, 1.0, "first hit of its lists");
        assert_eq!(merged[1].paper.arxiv_id.as_deref(), Some("9999.99999"));
    }

    #[test]
    fn different_journal_dois_block_a_title_merge() {
        let mut a = rec("A Survey of Methods", OPENALEX);
        a.doi = Some("10.1/one".into());
        let mut b = rec("A Survey of Methods", SEMANTIC_SCHOLAR);
        b.doi = Some("10.2/two".into());
        assert_eq!(merge(vec![vec![a], vec![b]]).len(), 2);
    }

    #[test]
    fn arxiv_entries_parse_with_pdf_links() {
        let papers = parse_arxiv(ARXIV_FIXTURE).unwrap();
        assert_eq!(papers.len(), 1);
        let p = &papers[0];
        assert_eq!(p.title, "Attention Is All You Need");
        assert_eq!(p.authors[0], "Ashish Vaswani");
        assert_eq!(p.year, Some(2017));
        assert_eq!(p.arxiv_id.as_deref(), Some("1706.03762"));
        assert_eq!(
            p.pdf_url.as_deref(),
            Some("https://arxiv.org/pdf/1706.03762v7")
        );
        assert_eq!(
            p.landing_url.as_deref(),
            Some("https://arxiv.org/abs/1706.03762v7")
        );
        assert!(p
            .abstract_snippet
            .as_deref()
            .unwrap()
            .starts_with("The dominant sequence"));
        assert!(parse_arxiv("<not xml").is_err());
    }

    #[test]
    fn openalex_works_parse_and_rebuild_abstracts() {
        let papers = parse_openalex(&serde_json::from_str(OPENALEX_FIXTURE).unwrap()).unwrap();
        assert_eq!(papers.len(), 2);
        assert_eq!(papers[0].doi.as_deref(), Some("10.48550/arxiv.1706.03762"));
        assert_eq!(papers[0].arxiv_id.as_deref(), Some("1706.03762"));
        assert_eq!(
            papers[0].abstract_snippet.as_deref(),
            Some("The dominant sequence transduction models")
        );
        assert_eq!(
            papers[0].pdf_url.as_deref(),
            Some("https://arxiv.org/pdf/1706.03762")
        );
        assert_eq!(papers[1].pdf_url, None, "closed access has no PDF");
        assert_eq!(papers[1].venue.as_deref(), Some("Nature"));
    }

    #[test]
    fn semantic_scholar_parses_and_falls_back_to_arxiv_pdfs() {
        let papers =
            parse_semantic_scholar(&serde_json::from_str(SEMANTIC_FIXTURE).unwrap()).unwrap();
        assert_eq!(papers.len(), 2);
        assert_eq!(
            papers[0].pdf_url.as_deref(),
            Some("https://arxiv.org/pdf/1810.04805")
        );
        assert_eq!(
            papers[1].pdf_url.as_deref(),
            Some("https://www.example.edu/open/paper.pdf")
        );
    }
}

#[cfg(test)]
#[path = "papers_calibration.rs"]
mod calibration;
