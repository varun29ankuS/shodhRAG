//! Fetching the model lists the picker shows: OpenRouter's public model list
//! (no key is sent), the models installed in Ollama and loaded in LM Studio,
//! and the one request that checks a pasted API key with its provider.
//!
//! Both requests are bounded (time and size) and identify the app with its
//! generic User-Agent only. Nothing about the user is sent.

use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;

use super::model::is_valid_model_id;
use super::model_catalog::{parse_openrouter, CatalogError, CatalogModel};

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
    let body = get_bounded(OPENROUTER_MODELS_URL, OPENROUTER_TIMEOUT).await?;
    Ok(parse_openrouter(&body)?)
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

/// Check `key` with one request to `provider` only. The key is sent in a
/// header and never appears in an error.
pub async fn verify_key(
    provider: super::model_catalog::ProviderId,
    key: &str,
) -> Result<(), KeyCheckError> {
    let label = provider.label();
    let check = key_check(provider).ok_or(KeyCheckError::NoKey(label))?;
    let value = reqwest::header::HeaderValue::from_str(&format!("{}{}", check.prefix, key.trim()))
        .map_err(|_| KeyCheckError::Rejected(label))?;
    let client =
        client(KEY_CHECK_TIMEOUT).map_err(|e| KeyCheckError::Unreachable(label, e.to_string()))?;
    let mut request = client
        .get(check.url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(check.header, value);
    for (name, v) in check.extra {
        request = request.header(*name, *v);
    }
    let response = request
        .send()
        .await
        .map_err(|e| KeyCheckError::Unreachable(label, e.without_url().to_string()))?;
    match response.status().as_u16() {
        200..=299 => Ok(()),
        400 | 401 | 403 => Err(KeyCheckError::Rejected(label)),
        status => Err(KeyCheckError::Unreachable(label, format!("HTTP {status}"))),
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

    #[test]
    fn the_user_agent_is_generic() {
        assert!(CATALOG_USER_AGENT.starts_with("shodh/"));
        assert!(!CATALOG_USER_AGENT.contains('@'));
    }
}
