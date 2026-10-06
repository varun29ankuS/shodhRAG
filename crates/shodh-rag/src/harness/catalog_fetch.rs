//! Fetching the model lists the picker shows: OpenRouter's public model list
//! (no key is sent) and the models installed in Ollama.
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
    fn the_user_agent_is_generic() {
        assert!(CATALOG_USER_AGENT.starts_with("shodh/"));
        assert!(!CATALOG_USER_AGENT.contains('@'));
    }
}
