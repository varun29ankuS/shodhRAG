//! Web search providers, in the order they are tried:
//! 1. OpenRouter's web plugin, with the OpenRouter key the user configured;
//! 2. Gemini with Grounding with Google Search, when `GEMINI_API_KEY` is set;
//! 3. a SearXNG instance, when `SHODH_SEARXNG_URL` is set.
//!
//! No provider scrapes a search engine's result pages. Each response is
//! parsed into [`WebResult`]s; parsing is separate from the request so it is
//! tested against recorded response shapes.

use serde::Serialize;
use serde_json::{json, Value};
use url::Url;

use super::client::{SafeClient, WebError};

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
pub const GEMINI_URL_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
/// Model used for OpenRouter web searches unless `SHODH_WEB_SEARCH_MODEL`
/// names another. A small model keeps the per-search cost low; the web
/// plugin's results are what matter, not its prose.
pub const DEFAULT_OPENROUTER_SEARCH_MODEL: &str = "openai/gpt-4o-mini";
pub const DEFAULT_GEMINI_SEARCH_MODEL: &str = "gemini-2.5-flash";

/// Largest provider answer read.
const MAX_PROVIDER_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SNIPPET_CHARS: usize = 400;

/// A provider API key. Never printed.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(key: impl Into<String>) -> Option<Self> {
        let key = key.into().trim().to_string();
        (!key.is_empty()).then_some(Self(key))
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey([REDACTED])")
    }
}

/// Which providers are available for this search.
#[derive(Debug, Clone, Default)]
pub struct SearchConfig {
    pub openrouter_key: Option<ApiKey>,
    pub openrouter_model: Option<String>,
    pub gemini_key: Option<ApiKey>,
    pub gemini_model: Option<String>,
    pub searxng_url: Option<Url>,
}

impl SearchConfig {
    pub fn is_empty(&self) -> bool {
        self.openrouter_key.is_none() && self.gemini_key.is_none() && self.searxng_url.is_none()
    }
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// What a provider returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOutcome {
    pub provider: &'static str,
    pub results: Vec<WebResult>,
    /// The provider model's own summary, when it wrote one (untrusted).
    pub summary: Option<String>,
    /// HTML the provider requires to be shown with its results (Google's
    /// search suggestions). Rendered only in a sandboxed frame without
    /// scripts.
    pub attribution_html: Option<String>,
}

fn truncate(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn host_of(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// Characters `start..end` of `text` (indices are in characters).
fn char_slice(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// Add `result` unless its URL is already present.
fn push_unique(results: &mut Vec<WebResult>, result: WebResult) {
    if !result.url.is_empty() && !results.iter().any(|r| r.url == result.url) {
        results.push(result);
    }
}

/// Parse an OpenRouter chat completion made with the `web` plugin: each
/// `url_citation` annotation becomes a result; its snippet is the cited
/// content, else the answer text the citation covers.
pub fn parse_openrouter(response: &Value) -> Result<SearchOutcome, WebError> {
    let message = response
        .pointer("/choices/0/message")
        .ok_or_else(|| WebError::Http("OpenRouter returned no message".to_string()))?;
    let text = message.get("content").and_then(Value::as_str).unwrap_or("");
    let mut results = Vec::new();
    for annotation in message
        .get("annotations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if annotation.get("type").and_then(Value::as_str) != Some("url_citation") {
            continue;
        }
        let Some(citation) = annotation.get("url_citation") else {
            continue;
        };
        let url = citation.get("url").and_then(Value::as_str).unwrap_or("");
        let title = citation
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| host_of(url));
        let snippet = match citation.get("content").and_then(Value::as_str) {
            Some(content) if !content.trim().is_empty() => content.to_string(),
            _ => {
                let start = citation.get("start_index").and_then(Value::as_u64);
                let end = citation.get("end_index").and_then(Value::as_u64);
                match (start, end) {
                    (Some(s), Some(e)) => char_slice(
                        text,
                        usize::try_from(s).unwrap_or(0),
                        usize::try_from(e).unwrap_or(0),
                    ),
                    _ => String::new(),
                }
            }
        };
        push_unique(
            &mut results,
            WebResult {
                title: truncate(&title, 200),
                url: url.to_string(),
                snippet: truncate(&snippet, MAX_SNIPPET_CHARS),
            },
        );
    }
    Ok(SearchOutcome {
        provider: "OpenRouter web search",
        results,
        summary: (!text.trim().is_empty()).then(|| truncate(text, 2_000)),
        attribution_html: None,
    })
}

/// Parse a Gemini `generateContent` answer grounded with Google Search.
/// Grounding chunk URIs are Google redirect links and their titles are the
/// source domains; both are shown as Google provides them. The snippet of a
/// chunk is the answer text its grounding supports cover.
pub fn parse_gemini(response: &Value) -> Result<SearchOutcome, WebError> {
    let candidate = response
        .pointer("/candidates/0")
        .ok_or_else(|| WebError::Http("Gemini returned no candidate".to_string()))?;
    let text: String = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let grounding = candidate.get("groundingMetadata");
    let chunks: Vec<&Value> = grounding
        .and_then(|g| g.get("groundingChunks"))
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let mut snippets: Vec<Vec<String>> = vec![Vec::new(); chunks.len()];
    for support in grounding
        .and_then(|g| g.get("groundingSupports"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let segment = support
            .pointer("/segment/text")
            .and_then(Value::as_str)
            .unwrap_or("");
        for index in support
            .get("groundingChunkIndices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
        {
            if let Some(list) = usize::try_from(index)
                .ok()
                .and_then(|i| snippets.get_mut(i))
            {
                if !segment.is_empty() {
                    list.push(segment.to_string());
                }
            }
        }
    }
    let mut results = Vec::new();
    for (chunk, snippet) in chunks.iter().zip(snippets) {
        let Some(web) = chunk.get("web") else {
            continue;
        };
        let url = web.get("uri").and_then(Value::as_str).unwrap_or("");
        let title = web
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| host_of(url));
        push_unique(
            &mut results,
            WebResult {
                title: truncate(&title, 200),
                url: url.to_string(),
                snippet: truncate(&snippet.join(" "), MAX_SNIPPET_CHARS),
            },
        );
    }
    let attribution_html = grounding
        .and_then(|g| g.pointer("/searchEntryPoint/renderedContent"))
        .and_then(Value::as_str)
        .filter(|h| !h.trim().is_empty())
        .map(str::to_string);
    Ok(SearchOutcome {
        provider: "Google Search (via Gemini)",
        results,
        summary: (!text.trim().is_empty()).then(|| truncate(&text, 2_000)),
        attribution_html,
    })
}

/// Parse a SearXNG `format=json` answer.
pub fn parse_searxng(response: &Value) -> Result<SearchOutcome, WebError> {
    let items = response
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| WebError::Http("SearXNG returned no results list".to_string()))?;
    let mut results = Vec::new();
    for item in items {
        let url = item.get("url").and_then(Value::as_str).unwrap_or("");
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| host_of(url));
        let snippet = item.get("content").and_then(Value::as_str).unwrap_or("");
        push_unique(
            &mut results,
            WebResult {
                title: truncate(&title, 200),
                url: url.to_string(),
                snippet: truncate(snippet, MAX_SNIPPET_CHARS),
            },
        );
    }
    Ok(SearchOutcome {
        provider: "SearXNG",
        results,
        summary: None,
        attribution_html: None,
    })
}

/// The message shown when no provider is configured.
pub const NO_PROVIDER: &str = "Web search is not set up. To enable it, add an OpenRouter API key \
    in Settings → Models (web search then uses OpenRouter's web plugin), or set GEMINI_API_KEY \
    (Google Search grounding), or point SHODH_SEARXNG_URL at a SearXNG instance, and restart Shodh.";

/// Run `query` against the first configured provider. A provider that fails
/// is reported together with the next one's outcome, never silently.
pub async fn search(
    client: &SafeClient,
    config: &SearchConfig,
    query: &str,
    max_results: usize,
) -> Result<(SearchOutcome, Vec<String>), WebError> {
    let mut failures = Vec::new();
    if let Some(key) = &config.openrouter_key {
        let model = config
            .openrouter_model
            .clone()
            .unwrap_or_else(|| DEFAULT_OPENROUTER_SEARCH_MODEL.to_string());
        let body = json!({
            "model": model,
            "plugins": [{"id": "web", "max_results": max_results}],
            "messages": [{
                "role": "user",
                "content": format!("Search the web for: {query}\nList what the most relevant sources say, briefly.")
            }],
        });
        let headers = vec![
            (
                "authorization".to_string(),
                format!("Bearer {}", key.expose()),
            ),
            ("x-title".to_string(), "Shodh".to_string()),
        ];
        match client
            .post_json(OPENROUTER_URL, headers, &body, MAX_PROVIDER_BYTES)
            .await
            .and_then(|v| parse_openrouter(&v))
        {
            Ok(outcome) if !outcome.results.is_empty() => return Ok((outcome, failures)),
            Ok(_) => failures.push("OpenRouter web search returned no sources".to_string()),
            Err(e) => failures.push(format!("OpenRouter web search failed: {e}")),
        }
    }
    if let Some(key) = &config.gemini_key {
        let model = config
            .gemini_model
            .clone()
            .unwrap_or_else(|| DEFAULT_GEMINI_SEARCH_MODEL.to_string());
        let url = format!("{GEMINI_URL_BASE}/{model}:generateContent");
        let body = json!({
            "contents": [{"parts": [{"text": query}]}],
            "tools": [{"google_search": {}}],
        });
        let headers = vec![("x-goog-api-key".to_string(), key.expose().to_string())];
        match client
            .post_json(&url, headers, &body, MAX_PROVIDER_BYTES)
            .await
            .and_then(|v| parse_gemini(&v))
        {
            Ok(outcome) if !outcome.results.is_empty() => return Ok((outcome, failures)),
            Ok(_) => failures.push("Gemini search grounding returned no sources".to_string()),
            Err(e) => failures.push(format!("Gemini search grounding failed: {e}")),
        }
    }
    if let Some(base) = &config.searxng_url {
        let mut url = base.clone();
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("format", "json");
        let trusted = client.clone().trusting(base);
        match trusted
            .get_json(url.as_str(), vec![], MAX_PROVIDER_BYTES)
            .await
            .and_then(|v| parse_searxng(&v))
        {
            Ok(mut outcome) => {
                outcome.results.truncate(max_results);
                return Ok((outcome, failures));
            }
            Err(e) => failures.push(format!("SearXNG search failed: {e}")),
        }
    }
    if config.is_empty() {
        return Err(WebError::Http(NO_PROVIDER.to_string()));
    }
    Err(WebError::Http(failures.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::super::client::testing::*;
    use super::*;

    const OPENROUTER: &str = include_str!("../fixtures/openrouter-web-search.json");
    const GEMINI: &str = include_str!("../fixtures/gemini-google-search.json");
    const SEARXNG: &str = include_str!("../fixtures/searxng-search.json");

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn openrouter_url_citations_become_results() {
        let outcome = parse_openrouter(&json(OPENROUTER)).unwrap();
        assert_eq!(outcome.results.len(), 2, "duplicate URL collapsed");
        assert_eq!(
            outcome.results[0].url,
            "https://www.example.org/rust-2024-edition"
        );
        assert_eq!(outcome.results[0].title, "Rust 2024 edition announced");
        assert!(outcome.results[0]
            .snippet
            .starts_with("The Rust 2024 edition"));
        // No `content`: the snippet is the cited span of the answer.
        assert_eq!(outcome.results[1].snippet, "stabilised async closures");
        assert!(outcome.summary.unwrap().contains("Rust 2024"));
    }

    #[test]
    fn gemini_grounding_chunks_become_results_with_attribution() {
        let outcome = parse_gemini(&json(GEMINI)).unwrap();
        assert_eq!(outcome.results.len(), 2);
        assert_eq!(outcome.results[0].title, "uefa.com");
        assert!(outcome.results[0]
            .url
            .starts_with("https://vertexaisearch.cloud.google.com/grounding-api-redirect/"));
        assert!(outcome.results[0].snippet.contains("Spain won Euro 2024"));
        assert!(outcome.attribution_html.unwrap().contains("container"));
    }

    #[test]
    fn searxng_results_parse() {
        let outcome = parse_searxng(&json(SEARXNG)).unwrap();
        assert_eq!(outcome.results.len(), 2);
        assert_eq!(outcome.results[1].title, "docs.rs");
        assert!(parse_searxng(&json("{}")).is_err());
    }

    #[tokio::test]
    async fn providers_are_tried_in_order_and_failures_reported() {
        let (client, transport) = client(
            FakeResolver::default()
                .with("openrouter.ai", &["104.18.2.115"])
                .with("generativelanguage.googleapis.com", &["142.250.74.10"]),
            FakeTransport::default()
                .route(
                    OPENROUTER_URL,
                    Canned {
                        status: 402,
                        headers: vec![],
                        body: br#"{"error":{"message":"Insufficient credits"}}"#.to_vec(),
                    },
                )
                .route(
                    &format!("{GEMINI_URL_BASE}/{DEFAULT_GEMINI_SEARCH_MODEL}:generateContent"),
                    Canned::ok("application/json", GEMINI),
                ),
        );
        let config = SearchConfig {
            openrouter_key: ApiKey::new("sk-or-test"),
            gemini_key: ApiKey::new("g-test"),
            ..SearchConfig::default()
        };
        let (outcome, failures) = search(&client, &config, "euro 2024 winner", 5)
            .await
            .unwrap();
        assert_eq!(outcome.provider, "Google Search (via Gemini)");
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("402"));
        let sent = transport.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        let auth = sent[0]
            .headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .unwrap();
        assert_eq!(auth.1, "Bearer sk-or-test");
    }

    #[tokio::test]
    async fn no_provider_explains_how_to_enable_search() {
        let (client, _) = client(FakeResolver::default(), FakeTransport::default());
        let err = search(&client, &SearchConfig::default(), "x", 5)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("OpenRouter API key"));
        assert!(format!("{:?}", ApiKey::new("secret").unwrap()).contains("REDACTED"));
    }
}
