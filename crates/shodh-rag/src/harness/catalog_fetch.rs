//! Fetching the model lists the picker shows: OpenRouter's public model list
//! (no key is sent), the models installed in Ollama and loaded in LM Studio,
//! and the one request that checks an API key with its provider (a pasted
//! key before it is saved, and the key in use for the connection status).
//!
//! Both requests are bounded (time and size) and identify the app with its
//! generic User-Agent only. Nothing about the user is sent.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use serde::Deserialize;

use super::model::is_valid_model_id;
use super::model_catalog::{parse_openrouter, CatalogError, CatalogModel};
use super::provider_error::{classify, limit_reset_at_ms, reset_epoch_ms, ProviderErrorKind};

/// OpenRouter's public list of models with prices and supported parameters.
pub const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";

/// The app's generic User-Agent for these requests.
pub const CATALOG_USER_AGENT: &str = concat!("shodh/", env!("CARGO_PKG_VERSION"));

/// Largest model list read (OpenRouter's is a few hundred kilobytes).
pub const MAX_LIST_BYTES: usize = 16 * 1024 * 1024;
/// Time allowed for the OpenRouter list.
pub const OPENROUTER_TIMEOUT: Duration = Duration::from_secs(12);
/// Time allowed for Ollama's local list (it answers at once when running).
pub const OLLAMA_TIMEOUT: Duration = Duration::from_millis(1_500);

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("The model list could not be fetched: {0}")]
    Http(String),
    #[error("The model list request returned HTTP {0}")]
    Status(u16),
    #[error("The model list is larger than {MAX_LIST_BYTES} bytes")]
    TooLarge,
    #[error(transparent)]
    Parse(#[from] CatalogError),
}

fn client(timeout: Duration) -> Result<reqwest::Client, FetchError> {
    reqwest::Client::builder()
        .user_agent(CATALOG_USER_AGENT)
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| FetchError::Http(e.to_string()))
}

/// GET `url` and read at most [`MAX_LIST_BYTES`] of its body.
async fn get_bounded(url: &str, timeout: Duration) -> Result<String, FetchError> {
    let response = client(timeout)?
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| FetchError::Http(e.without_url().to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(FetchError::Status(status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_LIST_BYTES as u64)
    {
        return Err(FetchError::TooLarge);
    }
    let mut body: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| FetchError::Http(e.without_url().to_string()))?;
        if body.len() + chunk.len() > MAX_LIST_BYTES {
            return Err(FetchError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| FetchError::Http("the response is not UTF-8".into()))
}

/// OpenRouter's model list, parsed.
pub async fn fetch_openrouter() -> Result<Vec<CatalogModel>, FetchError> {
    Ok(parse_openrouter(&fetch_openrouter_body().await?)?)
}

/// OpenRouter's model list as sent (JSON), for readers of fields the picker does not
/// keep (the evidence tables read each model's `reasoning` metadata).
pub async fn fetch_openrouter_body() -> Result<String, FetchError> {
    fetch_openrouter_body_within(OPENROUTER_TIMEOUT).await
}

/// [`fetch_openrouter_body`] allowed `timeout` (a caller that a user waits on allows
/// less).
pub async fn fetch_openrouter_body_within(timeout: Duration) -> Result<String, FetchError> {
    get_bounded(OPENROUTER_MODELS_URL, timeout).await
}

#[derive(Deserialize)]
struct OllamaTags {
    #[serde(default)]
    models: Vec<OllamaTag>,
}

#[derive(Deserialize)]
struct OllamaTag {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

/// Ollama's `GET /api/tags` response: installed model names, sorted, with
/// ids the runtime could not take skipped.
pub fn parse_ollama_tags(json: &str) -> Result<Vec<String>, CatalogError> {
    let tags: OllamaTags =
        serde_json::from_str(json).map_err(|e| CatalogError::Parse(e.to_string()))?;
    let mut names: Vec<String> = tags
        .models
        .into_iter()
        .filter_map(|t| t.name.or(t.model))
        .map(|n| n.trim().to_string())
        .filter(|n| is_valid_model_id(n))
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// The models installed in the Ollama at `host` (e.g. `http://127.0.0.1:11434`).
/// Only loopback hosts are asked: the list must not leave this computer.
pub async fn fetch_ollama(host: &str) -> Result<Vec<String>, FetchError> {
    let base = host.trim().trim_end_matches('/');
    if !is_loopback_url(base) {
        return Err(FetchError::Http(format!(
            "{base} is not on this computer; only a local Ollama is listed"
        )));
    }
    let body = get_bounded(&format!("{base}/api/tags"), OLLAMA_TIMEOUT).await?;
    Ok(parse_ollama_tags(&body)?)
}

/// `http(s)://localhost`, `127.x.x.x` or `[::1]`, with an optional port.
pub fn is_loopback_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Time allowed for LM Studio's local list.
pub const LMSTUDIO_TIMEOUT: Duration = Duration::from_millis(1_500);

#[derive(Deserialize)]
struct OpenAiModels {
    #[serde(default)]
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    #[serde(default)]
    id: Option<String>,
}

/// An OpenAI-compatible `GET /models` response (LM Studio): model ids in
/// the server's order, unsafe ids and duplicates skipped. Embedding models
/// (`text-embedding-*`) are left out: they cannot answer.
pub fn parse_openai_models(json: &str) -> Result<Vec<String>, CatalogError> {
    let list: OpenAiModels =
        serde_json::from_str(json).map_err(|e| CatalogError::Parse(e.to_string()))?;
    let mut out: Vec<String> = Vec::new();
    for id in list.data.into_iter().filter_map(|m| m.id) {
        let id = id.trim().to_string();
        if is_valid_model_id(&id) && !id.starts_with("text-embedding") && !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// The models loaded in LM Studio at `base` (e.g. `http://127.0.0.1:1234/v1`).
/// Only loopback addresses are asked.
pub async fn fetch_lmstudio(base: &str) -> Result<Vec<String>, FetchError> {
    let base = base.trim().trim_end_matches('/');
    if !is_loopback_url(base) {
        return Err(FetchError::Http(format!(
            "{base} is not on this computer; only a local LM Studio is listed"
        )));
    }
    let body = get_bounded(&format!("{base}/models"), LMSTUDIO_TIMEOUT).await?;
    Ok(parse_openai_models(&body)?)
}

/// Time allowed for checking an API key.
pub const KEY_CHECK_TIMEOUT: Duration = Duration::from_secs(12);

/// The cheap authenticated request that checks a key: a models list (or,
/// for OpenRouter, whose model list is public, its key endpoint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCheck {
    pub url: &'static str,
    /// The header carrying the key, and its value prefix.
    pub header: &'static str,
    pub prefix: &'static str,
    /// Extra fixed headers.
    pub extra: &'static [(&'static str, &'static str)],
}

/// The key check of a key provider (`None` for providers without a key).
pub fn key_check(provider: super::model_catalog::ProviderId) -> Option<KeyCheck> {
    use super::model_catalog::ProviderId as P;
    let bearer = |url| KeyCheck {
        url,
        header: "authorization",
        prefix: "Bearer ",
        extra: &[],
    };
    Some(match provider {
        P::OpenRouter => bearer("https://openrouter.ai/api/v1/key"),
        P::OpenAI => bearer("https://api.openai.com/v1/models"),
        P::Grok => bearer("https://api.x.ai/v1/models"),
        P::Anthropic => KeyCheck {
            url: "https://api.anthropic.com/v1/models?limit=1",
            header: "x-api-key",
            prefix: "",
            extra: &[("anthropic-version", "2023-06-01")],
        },
        // The key goes in a header, never in the URL (errors could show it).
        P::Google => KeyCheck {
            url: "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1",
            header: "x-goog-api-key",
            prefix: "",
            extra: &[],
        },
        _ => return None,
    })
}

/// Why a key was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyCheckError {
    #[error("{0} did not accept this key.")]
    Rejected(&'static str),
    #[error("{0} could not be reached to check the key: {1}")]
    Unreachable(&'static str, String),
    #[error("{0} does not use an API key.")]
    NoKey(&'static str),
}

/// Most of a key check's answer read: its error message, or the key's
/// details. Anything past it is not needed and not read.
pub const MAX_KEY_CHECK_BODY: usize = 16 * 1024;
/// Longest provider message kept for the UI.
pub const MAX_PROVIDER_MESSAGE_CHARS: usize = 300;

/// What one key check found. The check is a models list or, for
/// OpenRouter, its key endpoint: neither generates text nor counts as a
/// request against a free model allowance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyHealth {
    /// The provider accepted the key.
    Works,
    /// The provider refused the key (400/401/403), with its message when it
    /// gave one. `status` is `None` when the key could not even be sent.
    Rejected {
        status: Option<u16>,
        message: Option<String>,
    },
    /// The key is valid but limited: rate-limited for now (429), or out of
    /// credit or quota (`quota`: 402, a quota message, or a key whose credit
    /// limit is used up). `reset_at_ms` is when the provider said it resets.
    Limited {
        status: u16,
        quota: bool,
        message: Option<String>,
        reset_at_ms: Option<u64>,
    },
    /// No answer about the key: the provider could not be reached, or
    /// answered with a redirect or a status that says nothing about it.
    Unreachable { message: String },
}

impl KeyHealth {
    /// The provider could not be reached (a network error, a timeout).
    pub fn unreachable(error: impl std::fmt::Display) -> Self {
        KeyHealth::Unreachable {
            message: error.to_string(),
        }
    }
}

/// Response headers that say when a limit resets.
#[derive(Debug, Clone, Copy, Default)]
pub struct LimitHeaders<'a> {
    /// `Retry-After`, in seconds.
    pub retry_after: Option<&'a str>,
    /// `X-RateLimit-Reset`, in epoch milliseconds or seconds.
    pub ratelimit_reset: Option<&'a str>,
}

/// When a limit resets: the `X-RateLimit-Reset` header, else `Retry-After`
/// seconds from now, else a reset the body reports. Times already past are
/// ignored.
fn limit_reset(headers: LimitHeaders<'_>, body: &str, now_ms: u64) -> Option<u64> {
    let ahead = |at: u64| (at > now_ms).then_some(at);
    headers
        .ratelimit_reset
        .and_then(reset_epoch_ms)
        .and_then(ahead)
        .or_else(|| {
            headers
                .retry_after
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(|secs| now_ms.saturating_add(secs.saturating_mul(1_000)))
                .and_then(ahead)
        })
        .or_else(|| limit_reset_at_ms(body, now_ms))
}

/// Credit left on the key, from OpenRouter's key endpoint
/// (`{"data":{"limit_remaining": …}}`); `None` when the answer has no limit.
fn credit_left(body: &str) -> Option<f64> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value.get("data")?.get("limit_remaining")?.as_f64()
}

/// Whether a word of a provider message may echo part of the key (OpenAI
/// and xAI quote a masked key in their "incorrect API key" message).
fn echoes_key(word: &str, key: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    let key = key.trim();
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    word.contains('*')
        || word.contains('\u{2026}')
        || ["sk-", "sk_", "xai-", "xa-", "aiza", "or-v1"]
            .iter()
            .any(|p| lower.contains(p))
        || (key.chars().count() >= 8 && word.contains(tail.as_str()))
}

/// The provider's own message in an error body (`{"error":{"message":…}}`,
/// `{"error":"…"}`, `{"message":…}`, or short plain text), made safe to
/// show: any word that may echo the key is replaced and the text is cut to
/// [`MAX_PROVIDER_MESSAGE_CHARS`]. The key itself never appears in it.
pub fn provider_message(body: &str, key: &str) -> Option<String> {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let raw = match &parsed {
        Some(value) => {
            let error = value.get("error");
            error
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .or_else(|| error.and_then(|e| e.as_str()))
                .or_else(|| value.get("message").and_then(|m| m.as_str()))
                .map(str::to_string)
        }
        None => {
            let text = body.trim();
            (!text.is_empty() && !text.starts_with('<')).then(|| text.to_string())
        }
    }?;
    let key = key.trim();
    let raw = if key.is_empty() {
        raw
    } else {
        raw.replace(key, "[key]")
    };
    let cleaned: Vec<String> = raw
        .split_whitespace()
        .map(|word| {
            if echoes_key(word, key) {
                "[key]".to_string()
            } else {
                word.to_string()
            }
        })
        .collect();
    let text = cleaned.join(" ");
    if text.is_empty() {
        return None;
    }
    if text.chars().count() > MAX_PROVIDER_MESSAGE_CHARS {
        let cut: String = text.chars().take(MAX_PROVIDER_MESSAGE_CHARS).collect();
        return Some(format!("{}\u{2026}", cut.trim_end()));
    }
    Some(text)
}

/// The provider's message in a failed answer's error text (the runtime's
/// `"401 {json body}"`), made safe to show like [`provider_message`].
pub fn answer_error_message(text: &str, key: &str) -> Option<String> {
    text.find('{')
        .and_then(|at| provider_message(&text[at..], key))
        .or_else(|| provider_message(text, key))
}

/// Classify a key check's answer from its HTTP status, limit headers and
/// body. `key` is only used to keep it out of the message.
pub fn classify_key_response(
    status: u16,
    headers: LimitHeaders<'_>,
    body: &str,
    key: &str,
    now_ms: u64,
) -> KeyHealth {
    let message = || provider_message(body, key);
    // Some providers send quota failures as 400/403 (read like a chat error).
    let quota_text = || {
        classify(&format!("{status} {body}"), None, now_ms).kind
            == ProviderErrorKind::QuotaExhausted
    };
    match status {
        200..=299 => match credit_left(body) {
            Some(left) if left <= 0.0 => KeyHealth::Limited {
                status,
                quota: true,
                message: Some("The key's credit limit is used up.".to_string()),
                reset_at_ms: None,
            },
            _ => KeyHealth::Works,
        },
        402 => KeyHealth::Limited {
            status,
            quota: true,
            message: message(),
            reset_at_ms: limit_reset(headers, body, now_ms),
        },
        429 => KeyHealth::Limited {
            status,
            quota: quota_text(),
            message: message(),
            reset_at_ms: limit_reset(headers, body, now_ms),
        },
        400 | 403 if quota_text() => KeyHealth::Limited {
            status,
            quota: true,
            message: message(),
            reset_at_ms: limit_reset(headers, body, now_ms),
        },
        400 | 401 | 403 => KeyHealth::Rejected {
            status: Some(status),
            message: message(),
        },
        300..=399 => KeyHealth::Unreachable {
            message: "the provider answered with a redirect, which is not followed with a key"
                .to_string(),
        },
        other => KeyHealth::Unreachable {
            message: format!("HTTP {other}"),
        },
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Read at most [`MAX_KEY_CHECK_BODY`] bytes of a response (the rest is
/// dropped, not an error: only an error message or key details are needed).
async fn bounded_text(response: reqwest::Response) -> String {
    let mut body: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        let room = MAX_KEY_CHECK_BODY.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= MAX_KEY_CHECK_BODY {
            break;
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

/// Check `key` with one request to `provider` only (see [`key_check`]): no
/// text is generated and no chat request is used. The key is sent in a
/// header and never appears in the result.
pub async fn check_key(
    provider: super::model_catalog::ProviderId,
    key: &str,
) -> Result<KeyHealth, KeyCheckError> {
    let label = provider.label();
    let check = key_check(provider).ok_or(KeyCheckError::NoKey(label))?;
    let Ok(value) =
        reqwest::header::HeaderValue::from_str(&format!("{}{}", check.prefix, key.trim()))
    else {
        return Ok(KeyHealth::Rejected {
            status: None,
            message: Some("The key contains characters that cannot be sent.".to_string()),
        });
    };
    // No redirects: a key header (`x-api-key`, `x-goog-api-key`) must never
    // follow a redirect to another host.
    let client = match reqwest::Client::builder()
        .user_agent(CATALOG_USER_AGENT)
        .timeout(KEY_CHECK_TIMEOUT)
        .connect_timeout(KEY_CHECK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(e) => return Ok(KeyHealth::unreachable(e.without_url())),
    };
    let mut request = client
        .get(check.url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(check.header, value);
    for (name, v) in check.extra {
        request = request.header(*name, *v);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(e) => return Ok(KeyHealth::unreachable(e.without_url())),
    };
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let retry_after = header("retry-after");
    let ratelimit_reset = header("x-ratelimit-reset");
    let body = bounded_text(response).await;
    Ok(classify_key_response(
        status,
        LimitHeaders {
            retry_after: retry_after.as_deref(),
            ratelimit_reset: ratelimit_reset.as_deref(),
        },
        &body,
        key,
        now_ms(),
    ))
}

/// Check a pasted key before it is saved: `Ok` when the provider accepted
/// it (a valid key whose credit is used up is still the right key).
pub async fn verify_key(
    provider: super::model_catalog::ProviderId,
    key: &str,
) -> Result<(), KeyCheckError> {
    let label = provider.label();
    match check_key(provider, key).await? {
        KeyHealth::Works => Ok(()),
        KeyHealth::Limited {
            status: 200..=299, ..
        } => Ok(()),
        // As before the status check existed: a 400/403 refuses the key,
        // whatever the reason the provider gives.
        KeyHealth::Rejected { .. }
        | KeyHealth::Limited {
            status: 400 | 403, ..
        } => Err(KeyCheckError::Rejected(label)),
        KeyHealth::Limited { status, .. } => {
            Err(KeyCheckError::Unreachable(label, format!("HTTP {status}")))
        }
        KeyHealth::Unreachable { message } => Err(KeyCheckError::Unreachable(label, message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_tags_are_parsed_sorted_and_safe() {
        let json = r#"{"models":[
            {"name":"qwen3:4b","model":"qwen3:4b","size":2500000000},
            {"model":"llama3.2:3b"},
            {"name":"qwen3:4b"},
            {"name":"--bad"},
            {"size":1}
        ]}"#;
        assert_eq!(
            parse_ollama_tags(json).unwrap(),
            vec!["llama3.2:3b".to_string(), "qwen3:4b".to_string()]
        );
        assert_eq!(parse_ollama_tags("{}").unwrap(), Vec::<String>::new());
        assert!(parse_ollama_tags("not json").is_err());
    }

    #[test]
    fn only_loopback_ollama_hosts_are_listed() {
        assert!(is_loopback_url("http://127.0.0.1:11434"));
        assert!(is_loopback_url("http://localhost:11434"));
        assert!(is_loopback_url("http://[::1]:11434"));
        assert!(!is_loopback_url("http://10.0.0.5:11434"));
        assert!(!is_loopback_url("http://ollama.example.com"));
        assert!(!is_loopback_url("file:///etc/passwd"));
        assert!(!is_loopback_url("127.0.0.1:11434"));
    }

    #[test]
    fn lm_studio_models_are_parsed_in_order() {
        let json = r#"{"object":"list","data":[
            {"id":"qwen3-8b","object":"model"},
            {"id":"text-embedding-nomic-embed-text-v1.5"},
            {"id":"gemma-3-4b"},
            {"id":"qwen3-8b"},
            {"id":"--bad"},
            {"object":"model"}
        ]}"#;
        assert_eq!(
            parse_openai_models(json).unwrap(),
            vec!["qwen3-8b".to_string(), "gemma-3-4b".to_string()]
        );
        assert!(parse_openai_models("<html>").is_err());
    }

    #[test]
    fn keys_are_checked_with_one_authenticated_request_to_their_provider() {
        use crate::harness::model_catalog::ProviderId as P;
        let or = key_check(P::OpenRouter).unwrap();
        assert_eq!(
            or.url, "https://openrouter.ai/api/v1/key",
            "OpenRouter's model list is public"
        );
        assert_eq!((or.header, or.prefix), ("authorization", "Bearer "));
        let anthropic = key_check(P::Anthropic).unwrap();
        assert_eq!(anthropic.header, "x-api-key");
        assert!(anthropic
            .extra
            .contains(&("anthropic-version", "2023-06-01")));
        let google = key_check(P::Google).unwrap();
        assert_eq!(google.header, "x-goog-api-key");
        assert!(!google.url.contains("key="), "never in the URL");
        assert!(key_check(P::OpenAI)
            .unwrap()
            .url
            .starts_with("https://api.openai.com/"));
        assert!(key_check(P::Grok)
            .unwrap()
            .url
            .starts_with("https://api.x.ai/"));
        for p in [P::Ollama, P::LmStudio, P::ClaudeSub, P::CopilotSub] {
            assert!(key_check(p).is_none());
        }
        for p in P::KEYED {
            assert!(key_check(p).unwrap().url.starts_with("https://"));
        }
    }

    const NOW: u64 = 1_759_750_000_000;
    const KEY: &str = "sk-or-v1-0123456789abcdefa3f9";

    fn health(status: u16, headers: LimitHeaders<'_>, body: &str) -> KeyHealth {
        classify_key_response(status, headers, body, KEY, NOW)
    }

    #[test]
    fn a_key_check_answer_is_classified_from_status_headers_and_body() {
        let none = LimitHeaders::default();
        // 200: the key works; OpenRouter's key details may say its credit is used up.
        assert_eq!(
            health(
                200,
                none,
                r#"{"data":{"limit":null,"usage":0,"limit_remaining":null,"is_free_tier":true}}"#
            ),
            KeyHealth::Works
        );
        assert_eq!(
            health(200, none, r#"{"data":[{"id":"gpt-4o"}]}"#),
            KeyHealth::Works
        );
        assert!(matches!(
            health(
                200,
                none,
                r#"{"data":{"limit":5,"usage":5,"limit_remaining":0}}"#
            ),
            KeyHealth::Limited {
                status: 200,
                quota: true,
                ..
            }
        ));

        // 401 with the provider's message (the user's "User not found").
        assert_eq!(
            health(
                401,
                none,
                r#"{"error":{"message":"User not found.","code":401}}"#
            ),
            KeyHealth::Rejected {
                status: Some(401),
                message: Some("User not found.".to_string())
            }
        );
        // 403 and Google's 400 for a bad key are rejections too.
        assert!(matches!(
            health(
                403,
                none,
                r#"{"error":{"type":"permission_error","message":"Forbidden"}}"#
            ),
            KeyHealth::Rejected {
                status: Some(403),
                ..
            }
        ));
        assert!(matches!(
            health(
                400,
                none,
                r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT"}}"#
            ),
            KeyHealth::Rejected {
                status: Some(400),
                ..
            }
        ));
        // A 403 that is really a quota is not a bad key.
        assert!(matches!(
            health(
                403,
                none,
                r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota"}}"#
            ),
            KeyHealth::Limited {
                status: 403,
                quota: true,
                ..
            }
        ));

        // 402: out of credit.
        assert_eq!(
            health(
                402,
                none,
                r#"{"error":{"message":"Insufficient credits","code":402}}"#
            ),
            KeyHealth::Limited {
                status: 402,
                quota: true,
                message: Some("Insufficient credits".to_string()),
                reset_at_ms: None
            }
        );

        // 429 with reset headers: epoch milliseconds, epoch seconds, or Retry-After seconds.
        let reset = NOW + 90_000;
        let ms = reset.to_string();
        assert_eq!(
            health(
                429,
                LimitHeaders {
                    retry_after: None,
                    ratelimit_reset: Some(&ms)
                },
                r#"{"error":{"message":"Rate limit exceeded: free-models-per-min.","code":429}}"#
            ),
            KeyHealth::Limited {
                status: 429,
                quota: false,
                message: Some("Rate limit exceeded: free-models-per-min.".to_string()),
                reset_at_ms: Some(reset)
            }
        );
        let secs = (reset / 1_000).to_string();
        assert!(matches!(
            health(429, LimitHeaders { retry_after: None, ratelimit_reset: Some(&secs) }, "{}"),
            KeyHealth::Limited { reset_at_ms: Some(at), .. } if at == reset
        ));
        assert!(matches!(
            health(429, LimitHeaders { retry_after: Some("30"), ratelimit_reset: None }, "{}"),
            KeyHealth::Limited { quota: false, reset_at_ms: Some(at), .. } if at == NOW + 30_000
        ));
        // A daily free-model limit is a quota, with its reset from the body.
        let tomorrow = NOW + 20 * 3_600_000;
        let daily = format!(
            r#"{{"error":{{"message":"Rate limit exceeded: free-models-per-day.","code":429,"metadata":{{"headers":{{"X-RateLimit-Reset":"{tomorrow}"}}}}}}}}"#
        );
        assert!(matches!(
            health(429, none, &daily),
            KeyHealth::Limited { quota: true, reset_at_ms: Some(at), .. } if at == tomorrow
        ));
        // A reset already past is not reported.
        let past = (NOW - 1_000).to_string();
        assert!(matches!(
            health(
                429,
                LimitHeaders {
                    retry_after: None,
                    ratelimit_reset: Some(&past)
                },
                "{}"
            ),
            KeyHealth::Limited {
                reset_at_ms: None,
                ..
            }
        ));

        // A network error, a redirect, a server error: nothing known about the key.
        assert_eq!(
            KeyHealth::unreachable("connection refused"),
            KeyHealth::Unreachable {
                message: "connection refused".to_string()
            }
        );
        assert!(matches!(
            health(302, none, ""),
            KeyHealth::Unreachable { .. }
        ));
        assert_eq!(
            health(503, none, "<html>busy</html>"),
            KeyHealth::Unreachable {
                message: "HTTP 503".to_string()
            }
        );
    }

    #[test]
    fn provider_messages_never_carry_the_key() {
        let echo = r#"{"error":{"message":"Incorrect API key provided: sk-proj-********a3f9. You can find your API key at https://platform.openai.com/account/api-keys.","code":"invalid_api_key"}}"#;
        let message = provider_message(echo, KEY).unwrap();
        assert!(!message.contains("a3f9"), "{message}");
        assert!(!message.contains("sk-"), "{message}");
        assert!(
            message.starts_with("Incorrect API key provided: [key]"),
            "{message}"
        );
        let full = format!(r#"{{"error":"bad key {KEY}"}}"#);
        let message = provider_message(&full, KEY).unwrap();
        assert!(
            !message.contains(KEY) && !message.contains("0123"),
            "{message}"
        );
        let xai = r#"{"code":"Client specified an invalid argument","error":"Incorrect API key provided: xa***f9. You can obtain an API key from https://console.x.ai."}"#;
        assert!(!provider_message(xai, KEY).unwrap().contains("xa***"));
        // Long messages are cut; HTML pages are not messages.
        let long = format!(r#"{{"message":"{}"}}"#, "word ".repeat(200));
        assert!(
            provider_message(&long, KEY).unwrap().chars().count() <= MAX_PROVIDER_MESSAGE_CHARS + 1
        );
        assert_eq!(
            answer_error_message(
                r#"401 {"error":{"message":"User not found.","code":401}}"#,
                KEY
            )
            .as_deref(),
            Some("User not found.")
        );
        assert_eq!(
            answer_error_message("connection reset by peer", KEY).as_deref(),
            Some("connection reset by peer")
        );
        assert_eq!(provider_message("<html>502</html>", KEY), None);
        assert_eq!(provider_message("", KEY), None);
    }

    #[test]
    fn the_user_agent_is_generic() {
        assert!(CATALOG_USER_AGENT.starts_with("shodh/"));
        assert!(!CATALOG_USER_AGENT.contains('@'));
    }
}
