//! Scholarly search over free, open APIs only: arXiv, OpenAlex and
//! Semantic Scholar (keyless public endpoint). Nothing is scraped, and only
//! open-access PDF links the APIs report are passed on; paywalled or shadow
//! library copies are never looked up.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::Value;
use url::Url;

use super::client::{SafeClient, WebError};

pub const ARXIV_API: &str = "https://export.arxiv.org/api/query";
pub const OPENALEX_API: &str = "https://api.openalex.org/works";
pub const SEMANTIC_SCHOLAR_API: &str = "https://api.semanticscholar.org/graph/v1/paper/search";

const MAX_API_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ABSTRACT_CHARS: usize = 600;
const MAX_AUTHORS: usize = 8;

/// One paper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paper {
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
    pub abstract_snippet: Option<String>,
    pub doi: Option<String>,
    pub landing_url: Option<String>,
    /// An open-access PDF reported by the API.
    pub pdf_url: Option<String>,
    /// Which API found it.
    pub source: &'static str,
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
fn normalize_doi(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let doi = raw
        .strip_prefix("https://doi.org/")
        .or_else(|| raw.strip_prefix("http://doi.org/"))
        .or_else(|| raw.strip_prefix("doi:"))
        .unwrap_or(raw);
    doi.starts_with("10.").then(|| doi.to_ascii_lowercase())
}

fn https_only(url: &str) -> Option<String> {
    let url = url.trim();
    Url::parse(url)
        .ok()
        .filter(|u| matches!(u.scheme(), "https" | "http"))
        .map(|u| u.to_string())
}

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
        let authors = entry
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "author")
            .filter_map(|a| child_text(a, "name"))
            .take(MAX_AUTHORS)
            .collect();
        let year =
            child_text(entry, "published").and_then(|p| p.get(0..4).and_then(|y| y.parse().ok()));
        let mut landing = None;
        let mut pdf = None;
        for link in entry
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "link")
        {
            let href = link.attribute("href").and_then(https_only);
            match (link.attribute("title"), link.attribute("rel")) {
                (Some("pdf"), _) => pdf = href,
                (_, Some("alternate")) => landing = href,
                _ => {}
            }
        }
        let landing = landing.or_else(|| child_text(entry, "id").and_then(|id| https_only(&id)));
        papers.push(Paper {
            title,
            authors,
            year,
            venue: child_text(entry, "journal_ref").or_else(|| Some("arXiv".to_string())),
            abstract_snippet: child_text(entry, "summary").and_then(|s| snippet(&s)),
            doi: child_text(entry, "doi").and_then(|d| normalize_doi(&d)),
            landing_url: landing,
            pdf_url: pdf,
            source: "arXiv",
        });
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
            let authors = w
                .get("authorships")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| s(x, "/author/display_name"))
                        .take(MAX_AUTHORS)
                        .collect()
                })
                .unwrap_or_default();
            let pdf = s(w, "/best_oa_location/pdf_url")
                .or_else(|| s(w, "/open_access/oa_url"))
                .and_then(|u| https_only(&u));
            Some(Paper {
                title,
                authors,
                year: w
                    .get("publication_year")
                    .and_then(Value::as_i64)
                    .and_then(|y| i32::try_from(y).ok()),
                venue: s(w, "/primary_location/source/display_name"),
                abstract_snippet: w.get("abstract_inverted_index").and_then(openalex_abstract),
                doi: s(w, "/doi").and_then(|d| normalize_doi(&d)),
                landing_url: s(w, "/primary_location/landing_page_url")
                    .or_else(|| s(w, "/doi"))
                    .and_then(|u| https_only(&u)),
                pdf_url: pdf,
                source: "OpenAlex",
            })
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
        .filter_map(|p| {
            let title = s(p, "/title")?;
            let authors = p
                .get("authors")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| s(x, "/name"))
                        .take(MAX_AUTHORS)
                        .collect()
                })
                .unwrap_or_default();
            let arxiv_pdf =
                s(p, "/externalIds/ArXiv").map(|id| format!("https://arxiv.org/pdf/{id}"));
            Some(Paper {
                title,
                authors,
                year: p
                    .get("year")
                    .and_then(Value::as_i64)
                    .and_then(|y| i32::try_from(y).ok()),
                venue: s(p, "/venue"),
                abstract_snippet: s(p, "/abstract").and_then(|a| snippet(&a)),
                doi: s(p, "/externalIds/DOI").and_then(|d| normalize_doi(&d)),
                landing_url: s(p, "/url").and_then(|u| https_only(&u)),
                pdf_url: s(p, "/openAccessPdf/url")
                    .filter(|u| !u.is_empty())
                    .or(arxiv_pdf)
                    .and_then(|u| https_only(&u)),
                source: "Semantic Scholar",
            })
        })
        .collect())
}

fn title_key(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Merge result lists, dropping later duplicates (same DOI or same title)
/// but filling in an open-access PDF a duplicate knew about.
pub fn merge(lists: Vec<Vec<Paper>>, limit: usize) -> Vec<Paper> {
    let mut out: Vec<Paper> = Vec::new();
    let mut seen_titles: HashSet<String> = HashSet::new();
    let max_len = lists.iter().map(Vec::len).max().unwrap_or(0);
    // Interleave so every source contributes its best hits first.
    for i in 0..max_len {
        for list in &lists {
            let Some(paper) = list.get(i) else { continue };
            let key = title_key(&paper.title);
            let duplicate = out
                .iter_mut()
                .find(|p| (p.doi.is_some() && p.doi == paper.doi) || title_key(&p.title) == key);
            match duplicate {
                Some(existing) => {
                    if existing.pdf_url.is_none() {
                        existing.pdf_url = paper.pdf_url.clone();
                    }
                    if existing.doi.is_none() {
                        existing.doi = paper.doi.clone();
                    }
                }
                None if seen_titles.insert(key) => out.push(paper.clone()),
                None => {}
            }
        }
    }
    out.truncate(limit);
    out
}

/// Search every API concurrently. Returns the merged papers and the APIs
/// that failed (with why); fails only when all of them failed.
pub async fn search_papers(
    client: &SafeClient,
    query: &str,
    limit: usize,
) -> Result<(Vec<Paper>, Vec<String>), WebError> {
    let per_source = limit.clamp(1, 25);
    let arxiv = async {
        let mut url = Url::parse(ARXIV_API).map_err(|e| WebError::Http(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("search_query", &format!("all:{query}"))
            .append_pair("start", "0")
            .append_pair("max_results", &per_source.to_string());
        let fetched = client.get(url.as_str(), vec![]).await?;
        let status = fetched.status;
        let bytes = fetched.read_capped(MAX_API_BYTES).await?;
        if !(200..300).contains(&status) {
            return Err(WebError::Status { status });
        }
        parse_arxiv(&String::from_utf8_lossy(&bytes))
    };
    let openalex = async {
        let mut url = Url::parse(OPENALEX_API).map_err(|e| WebError::Http(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("search", query)
            .append_pair("per-page", &per_source.to_string());
        let value = client.get_json(url.as_str(), vec![], MAX_API_BYTES).await?;
        parse_openalex(&value)
    };
    let semantic = async {
        let mut url =
            Url::parse(SEMANTIC_SCHOLAR_API).map_err(|e| WebError::Http(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("query", query)
            .append_pair("limit", &per_source.to_string())
            .append_pair(
                "fields",
                "title,authors,year,venue,abstract,externalIds,openAccessPdf,url",
            );
        let value = client.get_json(url.as_str(), vec![], MAX_API_BYTES).await?;
        parse_semantic_scholar(&value)
    };
    let (a, o, s) = tokio::join!(arxiv, openalex, semantic);
    let mut lists = Vec::new();
    let mut failures = Vec::new();
    for (name, result) in [("arXiv", a), ("OpenAlex", o), ("Semantic Scholar", s)] {
        match result {
            Ok(list) => lists.push(list),
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    if lists.is_empty() {
        return Err(WebError::Http(format!(
            "Every scholarly search failed: {}",
            failures.join("; ")
        )));
    }
    Ok((merge(lists, limit), failures))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARXIV: &str = include_str!("../fixtures/arxiv-query.xml");
    const OPENALEX: &str = include_str!("../fixtures/openalex-works.json");
    const SEMANTIC: &str = include_str!("../fixtures/semantic-scholar-search.json");

    #[test]
    fn arxiv_entries_parse_with_pdf_links() {
        let papers = parse_arxiv(ARXIV).unwrap();
        assert_eq!(papers.len(), 1);
        let p = &papers[0];
        assert_eq!(p.title, "Attention Is All You Need");
        assert_eq!(p.authors[0], "Ashish Vaswani");
        assert_eq!(p.year, Some(2017));
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
        let papers = parse_openalex(&serde_json::from_str(OPENALEX).unwrap()).unwrap();
        assert_eq!(papers.len(), 2);
        assert_eq!(papers[0].doi.as_deref(), Some("10.48550/arxiv.1706.03762"));
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
        let papers = parse_semantic_scholar(&serde_json::from_str(SEMANTIC).unwrap()).unwrap();
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

    #[test]
    fn merging_dedupes_by_doi_and_title_and_keeps_open_pdfs() {
        let arxiv = parse_arxiv(ARXIV).unwrap();
        let openalex = parse_openalex(&serde_json::from_str(OPENALEX).unwrap()).unwrap();
        let semantic = parse_semantic_scholar(&serde_json::from_str(SEMANTIC).unwrap()).unwrap();
        let merged = merge(vec![arxiv, openalex, semantic], 10);
        let titles: Vec<&str> = merged.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(
            titles
                .iter()
                .filter(|t| t.starts_with("Attention Is All"))
                .count(),
            1
        );
        assert_eq!(merged.len(), 4);
        assert_eq!(merge(vec![parse_arxiv(ARXIV).unwrap()], 0).len(), 0);
    }
}
