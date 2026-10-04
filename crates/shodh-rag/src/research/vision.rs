//! Equation transcription with a vision-capable model.
//!
//! Whether a model reads images is taken only from metadata its provider publishes, never
//! guessed from its name:
//! - OpenRouter lists every model's `architecture.input_modalities` at `/api/v1/models`
//!   (a public endpoint; no key or user data is sent);
//! - Ollama reports a local model's `capabilities` (including `vision`) at `/api/show`.
//!
//! Other providers publish no input metadata through their APIs, so for them (and for the
//! in-process llama.cpp runtime, which loads no vision projector) transcription is not
//! offered, with the reason.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};

use super::{ResearchError, ResearchResult};

/// OpenRouter's model list.
pub const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const OPENROUTER_CHAT_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
/// The local Ollama server (the address the app's Ollama provider uses).
pub const OLLAMA_URL: &str = "http://localhost:11434";
/// How long OpenRouter's model list is reused.
const MODEL_LIST_TTL: Duration = Duration::from_secs(3600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest transcription read.
const MAX_OUTPUT_TOKENS: u32 = 1_200;

/// The instruction sent with the image.
pub const TRANSCRIBE_PROMPT: &str =
    "Transcribe the mathematical content of this image into LaTeX. \
Reply with the LaTeX only: no explanation, no surrounding $ or \\[ \\] delimiters, no code fence. \
Keep the notation exactly as printed (symbols, sub- and superscripts, equation structure). If \
there are several equations, put each on its own line. If the image contains no mathematics, \
reply with NONE.";

/// A model that may be asked to transcribe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisionModel {
    OpenRouter { model: String },
    Ollama { model: String },
}

impl VisionModel {
    pub fn model_id(&self) -> &str {
        match self {
            VisionModel::OpenRouter { model } | VisionModel::Ollama { model } => model,
        }
    }
}

static OPENROUTER_LIST: LazyLock<Mutex<Option<(Instant, HashMap<String, Vec<String>>)>>> =
    LazyLock::new(|| Mutex::new(None));

fn client(timeout: Duration) -> ResearchResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| ResearchError::Model(format!("HTTP client could not be built: {e}")))
}

/// Input modalities by model id from an OpenRouter `/models` response.
pub fn parse_openrouter_models(body: &Value) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    for model in body
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            continue;
        };
        let modalities: Vec<String> = model
            .pointer("/architecture/input_modalities")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_ascii_lowercase)
            .collect();
        out.insert(id.to_string(), modalities);
    }
    out
}

/// What OpenRouter says `model` accepts: `Ok(None)` when it does not list the model.
pub async fn openrouter_modalities(model: &str) -> ResearchResult<Option<Vec<String>>> {
    {
        let cached = OPENROUTER_LIST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, list)) = cached.as_ref() {
            if at.elapsed() < MODEL_LIST_TTL {
                return Ok(list.get(model).cloned());
            }
        }
    }
    let body: Value = client(PROBE_TIMEOUT)?
        .get(OPENROUTER_MODELS_URL)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| ResearchError::Model(format!("OpenRouter's model list is unreachable: {e}")))?
        .json()
        .await
        .map_err(|e| ResearchError::Model(format!("OpenRouter's model list is unreadable: {e}")))?;
    let list = parse_openrouter_models(&body);
    let found = list.get(model).cloned();
    *OPENROUTER_LIST.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list));
    Ok(found)
}

/// Capabilities from an Ollama `/api/show` response.
pub fn parse_ollama_capabilities(body: &Value) -> Vec<String> {
    body.get("capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_ascii_lowercase)
        .collect()
}

/// What the local Ollama server says `model` can do.
pub async fn ollama_capabilities(model: &str) -> ResearchResult<Vec<String>> {
    let body: Value = client(PROBE_TIMEOUT)?
        .post(format!("{OLLAMA_URL}/api/show"))
        .json(&json!({ "model": model }))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| ResearchError::Model(format!("Ollama did not describe {model}: {e}")))?
        .json()
        .await
        .map_err(|e| {
            ResearchError::Model(format!("Ollama's model description is unreadable: {e}"))
        })?;
    Ok(parse_ollama_capabilities(&body))
}

/// The LaTeX in a model reply: code fences and display delimiters removed; `None` when
/// the model said the image has no mathematics or replied with nothing.
pub fn latex_from_reply(reply: &str) -> Option<String> {
    let mut text = reply.trim();
    if let Some(rest) = text.strip_prefix("```") {
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_alphabetic());
        text = rest.trim_end().strip_suffix("```").unwrap_or(rest).trim();
    }
    for (open, close) in [("$$", "$$"), ("\\[", "\\]"), ("$", "$")] {
        if let Some(inner) = text.strip_prefix(open).and_then(|t| t.strip_suffix(close)) {
            text = inner.trim();
            break;
        }
    }
    if text.is_empty() || text.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(text.to_string())
    }
}

/// Asks `model` to transcribe the PNG. `key` is the OpenRouter key (unused for Ollama).
pub async fn transcribe(
    model: &VisionModel,
    key: Option<&str>,
    png: &[u8],
) -> ResearchResult<String> {
    let image = base64::engine::general_purpose::STANDARD.encode(png);
    let http = client(REQUEST_TIMEOUT)?;
    let reply = match model {
        VisionModel::OpenRouter { model } => {
            let key = key.ok_or_else(|| {
                ResearchError::Model("No OpenRouter API key is configured.".to_string())
            })?;
            let body = json!({
                "model": model,
                "temperature": 0,
                "max_tokens": MAX_OUTPUT_TOKENS,
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": TRANSCRIBE_PROMPT },
                        { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{image}") } }
                    ]
                }]
            });
            let value: Value = http
                .post(OPENROUTER_CHAT_URL)
                .bearer_auth(key)
                .json(&body)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| ResearchError::Model(format!("{model} could not be reached: {e}")))?
                .json()
                .await
                .map_err(|e| {
                    ResearchError::Model(format!("{model} replied with something unreadable: {e}"))
                })?;
            value
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        }
        VisionModel::Ollama { model } => {
            let body = json!({
                "model": model,
                "stream": false,
                "options": { "temperature": 0, "num_predict": MAX_OUTPUT_TOKENS },
                "messages": [{ "role": "user", "content": TRANSCRIBE_PROMPT, "images": [image] }]
            });
            let value: Value = http
                .post(format!("{OLLAMA_URL}/api/chat"))
                .json(&body)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| ResearchError::Model(format!("Ollama could not run {model}: {e}")))?
                .json()
                .await
                .map_err(|e| {
                    ResearchError::Model(format!("Ollama replied with something unreadable: {e}"))
                })?;
            value
                .pointer("/message/content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        }
    };
    latex_from_reply(&reply).ok_or_else(|| {
        ResearchError::Model(format!(
            "{} found no mathematics to transcribe in this snippet.",
            model.model_id()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_metadata_is_read_not_guessed() {
        let list = parse_openrouter_models(&json!({
            "data": [
                { "id": "qwen/qwen2.5-vl-72b-instruct", "architecture": { "input_modalities": ["text", "image"] } },
                { "id": "meta-llama/llama-3.1-8b-instruct", "architecture": { "input_modalities": ["text"] } },
                { "id": "no-architecture" }
            ]
        }));
        assert!(list["qwen/qwen2.5-vl-72b-instruct"].contains(&"image".to_string()));
        assert!(!list["meta-llama/llama-3.1-8b-instruct"].contains(&"image".to_string()));
        assert!(list["no-architecture"].is_empty());
        assert_eq!(
            parse_ollama_capabilities(&json!({ "capabilities": ["completion", "Vision"] })),
            vec!["completion".to_string(), "vision".to_string()]
        );
        assert!(parse_ollama_capabilities(&json!({ "modelfile": "..." })).is_empty());
    }

    #[test]
    fn replies_are_unwrapped_to_latex() {
        assert_eq!(
            latex_from_reply("```latex\nE = mc^2\n```").as_deref(),
            Some("E = mc^2")
        );
        assert_eq!(
            latex_from_reply("$$\\frac{a}{b}$$").as_deref(),
            Some("\\frac{a}{b}")
        );
        assert_eq!(latex_from_reply("\\[ x^2 \\]").as_deref(), Some("x^2"));
        assert_eq!(
            latex_from_reply("  y = ax + b ").as_deref(),
            Some("y = ax + b")
        );
        assert_eq!(latex_from_reply("NONE"), None);
        assert_eq!(latex_from_reply(""), None);
    }
}
