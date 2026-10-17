//! Typed failures of the app's own model calls (memory learning, one-shot
//! extraction, evals) and the parsing of OpenAI-compatible and Anthropic
//! streams that reports them.
//!
//! A provider can fail after it answered `200 OK`: OpenRouter sends a chunk
//! with a top-level `error` object (and `finish_reason: "error"`) once tokens
//! are flowing, a non-streaming body can be `{"error": …}`, and a stream can
//! break or end with nothing in it. Each of these is a [`ProviderFailure`],
//! never answer text and never a finished stream. The SSE parsers are pure
//! (one line in, items out) so every case is tested without HTTP.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value;

use super::ToolCall;
use crate::harness::provider_error::{classify, safe_detail, upstream_provider, ProviderErrorKind};

/// Longest provider detail kept on a failure (the log and error text).
pub const MAX_FAILURE_DETAIL_CHARS: usize = 600;

/// A model call that failed at, or on the way to, the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    pub kind: ProviderErrorKind,
    /// The provider called (`OpenRouter`, `Anthropic`, …).
    pub provider: String,
    /// The provider behind a router that failed, when it said.
    pub upstream: Option<String>,
    pub model: String,
    /// HTTP status, when the failure carried one.
    pub status: Option<u16>,
    /// The provider's own words, cut and without credentials.
    pub detail: String,
}

impl ProviderFailure {
    /// Plain words for the kind of failure.
    pub fn summary(&self) -> &'static str {
        match self.kind {
            ProviderErrorKind::RateLimited => "the provider is rate-limiting requests",
            ProviderErrorKind::QuotaExhausted => "the provider credits or daily limit are used up",
            ProviderErrorKind::ModelUnavailable => "the model is unavailable at the provider",
            ProviderErrorKind::Auth => "the provider rejected the API key",
            ProviderErrorKind::Timeout => "the provider did not answer in time",
            ProviderErrorKind::Network => "the connection to the provider broke",
            ProviderErrorKind::InvalidRequest => "the provider refused the request",
            ProviderErrorKind::ContextTooLong => "the request is too long for the model",
            ProviderErrorKind::ContentFiltered => {
                "the provider's content filter stopped the answer"
            }
            ProviderErrorKind::EmptyResponse => "the provider sent back an empty answer",
            ProviderErrorKind::Other => "the provider could not answer",
        }
    }

    /// Write the failure to the log (kind, provider, upstream, model,
    /// status and the cut detail; never the key or the request).
    pub fn log(&self) {
        tracing::warn!(
            target: "shodh::llm",
            kind = ?self.kind,
            provider = %self.provider,
            upstream = self.upstream.as_deref().unwrap_or("-"),
            model = %self.model,
            status = ?self.status,
            detail = %self.detail,
            "model call failed"
        );
    }
}

impl fmt::Display for ProviderFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.provider)?;
        if let Some(upstream) = &self.upstream {
            write!(f, " via {upstream}")?;
        }
        write!(f, " · {}: {}", self.model, self.summary())?;
        if let Some(status) = self.status {
            write!(f, " (HTTP {status})")?;
        }
        if !self.detail.is_empty() {
            write!(f, ": {}", self.detail)?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderFailure {}

/// Who a call goes to, and the key to keep out of every failure detail.
#[derive(Clone)]
pub struct CallContext {
    provider: String,
    model: String,
    secret: String,
}

/// Never prints the key.
impl fmt::Debug for CallContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallContext")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// An HTTP error status in a provider's error `code` (number or digits).
fn error_status(code: &Value) -> Option<u16> {
    let n = match code {
        Value::Number(n) => n.as_u64()?,
        Value::String(s) => s.trim().parse::<u64>().ok()?,
        _ => return None,
    };
    u16::try_from(n).ok().filter(|n| (400..=599).contains(n))
}

impl CallContext {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        secret: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            secret: secret.into(),
        }
    }

    fn detail(&self, text: &str) -> String {
        safe_detail(text, Some(&self.secret), MAX_FAILURE_DETAIL_CHARS)
    }

    /// A failure of a known `kind`.
    pub fn failure(
        &self,
        kind: ProviderErrorKind,
        status: Option<u16>,
        detail: &str,
    ) -> ProviderFailure {
        ProviderFailure {
            kind,
            provider: self.provider.clone(),
            upstream: upstream_provider(detail),
            model: self.model.clone(),
            status,
            detail: self.detail(detail),
        }
    }

    /// An HTTP error response, classified from its status and body.
    pub fn http(&self, status: u16, body: &str) -> ProviderFailure {
        let classified = classify(&format!("{status} {body}"), None, now_ms());
        ProviderFailure {
            kind: classified.kind,
            provider: self.provider.clone(),
            upstream: classified.upstream,
            model: self.model.clone(),
            status: Some(status),
            detail: self.detail(body),
        }
    }

    /// An error the provider put inside a `200 OK` body or stream chunk
    /// (`{"error": {"code", "message", "metadata": {"provider_name", "raw"}}}`).
    /// `chunk_provider` is the chunk's own `provider` field, when it has one.
    pub fn in_band(&self, error: &Value, chunk_provider: Option<&str>) -> ProviderFailure {
        let status = error_status(&error["code"]).or_else(|| error_status(&error["status"]));
        let message = match error {
            Value::String(s) => s.clone(),
            _ => error["message"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| error.to_string()),
        };
        let raw = match &error["metadata"]["raw"] {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        };
        let full = error.to_string();
        let prefix = status.map(|s| format!("{s} ")).unwrap_or_default();
        let classified = classify(&format!("{prefix}{full}"), None, now_ms());
        let detail = match raw {
            Some(raw) if !raw.trim().is_empty() && raw != message => format!("{message} ({raw})"),
            _ => message,
        };
        ProviderFailure {
            kind: classified.kind,
            provider: self.provider.clone(),
            upstream: classified.upstream.or_else(|| {
                chunk_provider
                    .map(str::trim)
                    .filter(|p| !p.is_empty() && p.chars().count() <= 64)
                    .map(str::to_string)
            }),
            model: self.model.clone(),
            status,
            detail: self.detail(&detail),
        }
    }

    /// The request got no answer: it timed out or never connected.
    pub fn transport(&self, error: &reqwest::Error) -> ProviderFailure {
        let kind = if error.is_timeout() {
            ProviderErrorKind::Timeout
        } else {
            ProviderErrorKind::Network
        };
        self.failure(kind, None, &error.to_string())
    }

    /// A stream broke after it started (`detail` is the transport's error).
    pub fn interrupted(&self, detail: &str) -> ProviderFailure {
        let classified = classify(detail, None, now_ms());
        let kind = match classified.kind {
            ProviderErrorKind::Timeout => ProviderErrorKind::Timeout,
            _ => ProviderErrorKind::Network,
        };
        self.failure(kind, None, detail)
    }

    /// A `200 OK` non-streaming body: the error it carries, if any.
    pub fn body_error(&self, body: &Value) -> Option<ProviderFailure> {
        if let Some(error) = body.get("error").filter(|e| !e.is_null()) {
            return Some(self.in_band(error, body["provider"].as_str()));
        }
        match body["choices"][0]["finish_reason"].as_str() {
            Some("error") => Some(self.failure(
                ProviderErrorKind::ModelUnavailable,
                None,
                "The provider ended the answer with finish_reason \"error\"",
            )),
            Some("content_filter") => Some(self.failure(
                ProviderErrorKind::ContentFiltered,
                None,
                "The provider ended the answer with finish_reason \"content_filter\"",
            )),
            _ => None,
        }
    }

    /// An answer with nothing in it.
    pub fn empty(&self) -> ProviderFailure {
        self.failure(
            ProviderErrorKind::EmptyResponse,
            None,
            "The provider returned an empty response",
        )
    }
}

/// What a stream produced.
#[derive(Debug, Clone)]
pub enum StreamItem {
    Content(String),
    ToolCall(ToolCall),
    /// The answer is complete.
    Done,
    /// The stream failed; nothing follows.
    Failed(ProviderFailure),
}

/// The SSE `data:` payload of a line (`data: x` or `data:x`); comments
/// (`: OPENROUTER PROCESSING`) and other fields are not data.
fn sse_data(line: &str) -> Option<&str> {
    line.trim().strip_prefix("data:").map(str::trim)
}

/// One OpenAI-compatible chat completion stream (OpenAI, OpenRouter,
/// Together, Ollama, LM Studio, …), fed line by line.
#[derive(Debug)]
pub struct OpenAiSse {
    ctx: CallContext,
    /// index → (id, name, arguments) of streamed tool calls.
    tool_calls: BTreeMap<u64, (String, String, String)>,
    produced: bool,
    finish_reason: Option<String>,
    over: bool,
}

impl OpenAiSse {
    pub fn new(ctx: CallContext) -> Self {
        Self {
            ctx,
            tool_calls: BTreeMap::new(),
            produced: false,
            finish_reason: None,
            over: false,
        }
    }

    /// Whether the stream ended (done or failed); later lines are ignored.
    pub fn is_over(&self) -> bool {
        self.over
    }

    fn fail(&mut self, failure: ProviderFailure) -> StreamItem {
        self.over = true;
        self.tool_calls.clear();
        StreamItem::Failed(failure)
    }

    fn finish(&mut self) -> Vec<StreamItem> {
        if !self.produced {
            let empty = self.ctx.empty();
            return vec![self.fail(empty)];
        }
        self.over = true;
        let mut items: Vec<StreamItem> = std::mem::take(&mut self.tool_calls)
            .into_values()
            .map(|(id, name, arguments)| {
                StreamItem::ToolCall(ToolCall {
                    id,
                    name,
                    arguments,
                })
            })
            .collect();
        items.push(StreamItem::Done);
        items
    }

    /// One line of the stream (without its newline).
    pub fn line(&mut self, line: &str) -> Vec<StreamItem> {
        if self.over {
            return Vec::new();
        }
        let Some(data) = sse_data(line) else {
            return Vec::new();
        };
        if data == "[DONE]" {
            return self.finish();
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        if let Some(error) = chunk.get("error").filter(|e| !e.is_null()) {
            let failure = self.ctx.in_band(error, chunk["provider"].as_str());
            return vec![self.fail(failure)];
        }
        let choice = &chunk["choices"][0];
        let delta = &choice["delta"];
        let mut items = Vec::new();
        if let Some(content) = delta["content"].as_str() {
            if !content.is_empty() {
                self.produced = true;
                items.push(StreamItem::Content(content.to_string()));
            }
        }
        if let Some(deltas) = delta["tool_calls"].as_array() {
            for tc in deltas {
                self.produced = true;
                let index = tc["index"].as_u64().unwrap_or(0);
                let entry = self.tool_calls.entry(index).or_default();
                if let Some(id) = tc["id"].as_str().filter(|s| !s.is_empty()) {
                    entry.0 = id.to_string();
                }
                if let Some(name) = tc["function"]["name"].as_str().filter(|s| !s.is_empty()) {
                    entry.1 = name.to_string();
                }
                if let Some(args) = tc["function"]["arguments"].as_str() {
                    entry.2.push_str(args);
                }
            }
        }
        match choice["finish_reason"].as_str() {
            Some("error") => {
                let failure = self.ctx.failure(
                    ProviderErrorKind::ModelUnavailable,
                    None,
                    "The provider ended the answer with finish_reason \"error\"",
                );
                items.push(self.fail(failure));
            }
            Some("content_filter") => {
                let failure = self.ctx.failure(
                    ProviderErrorKind::ContentFiltered,
                    None,
                    "The provider ended the answer with finish_reason \"content_filter\"",
                );
                items.push(self.fail(failure));
            }
            Some(reason) => self.finish_reason = Some(reason.to_string()),
            None => {}
        }
        items
    }

    /// The byte stream ended: cleanly (`broken` is `None`) or with a
    /// transport error. Ending without `[DONE]` is complete only after a
    /// finish reason; with nothing received it is an empty response, and
    /// with part of an answer it is a broken stream.
    pub fn end(&mut self, broken: Option<&str>) -> Vec<StreamItem> {
        if self.over {
            return Vec::new();
        }
        if let Some(detail) = broken {
            let failure = self.ctx.interrupted(detail);
            return vec![self.fail(failure)];
        }
        if self.finish_reason.is_some() || !self.produced {
            return self.finish();
        }
        let failure = self.ctx.failure(
            ProviderErrorKind::Network,
            None,
            "The stream ended before the answer was complete",
        );
        vec![self.fail(failure)]
    }
}

/// One Anthropic Messages stream, fed line by line.
#[derive(Debug)]
pub struct AnthropicSse {
    ctx: CallContext,
    /// The tool-use block being streamed: (id, name, arguments).
    tool: Option<(String, String, String)>,
    produced: bool,
    over: bool,
}

impl AnthropicSse {
    pub fn new(ctx: CallContext) -> Self {
        Self {
            ctx,
            tool: None,
            produced: false,
            over: false,
        }
    }

    pub fn is_over(&self) -> bool {
        self.over
    }

    fn fail(&mut self, failure: ProviderFailure) -> StreamItem {
        self.over = true;
        self.tool = None;
        StreamItem::Failed(failure)
    }

    /// One line of the stream (without its newline).
    pub fn line(&mut self, line: &str) -> Vec<StreamItem> {
        if self.over {
            return Vec::new();
        }
        let Some(data) = sse_data(line) else {
            return Vec::new();
        };
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        match event["type"].as_str() {
            // `{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}`
            Some("error") => {
                let failure = self.ctx.in_band(&event["error"], None);
                vec![self.fail(failure)]
            }
            Some("content_block_start") => {
                let block = &event["content_block"];
                if block["type"].as_str() == Some("tool_use") {
                    self.produced = true;
                    self.tool = Some((
                        block["id"].as_str().unwrap_or("").to_string(),
                        block["name"].as_str().unwrap_or("").to_string(),
                        String::new(),
                    ));
                }
                Vec::new()
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => match delta["text"].as_str() {
                        Some(text) if !text.is_empty() => {
                            self.produced = true;
                            vec![StreamItem::Content(text.to_string())]
                        }
                        _ => Vec::new(),
                    },
                    Some("input_json_delta") => {
                        if let (Some(tool), Some(partial)) =
                            (self.tool.as_mut(), delta["partial_json"].as_str())
                        {
                            tool.2.push_str(partial);
                        }
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            Some("content_block_stop") => match self.tool.take() {
                Some((id, name, arguments)) => vec![StreamItem::ToolCall(ToolCall {
                    id,
                    name,
                    arguments,
                })],
                None => Vec::new(),
            },
            Some("message_stop") => {
                if !self.produced {
                    let empty = self.ctx.empty();
                    return vec![self.fail(empty)];
                }
                self.over = true;
                vec![StreamItem::Done]
            }
            _ => Vec::new(),
        }
    }

    /// The byte stream ended before `message_stop` (cleanly or with a
    /// transport error): the answer is not complete.
    pub fn end(&mut self, broken: Option<&str>) -> Vec<StreamItem> {
        if self.over {
            return Vec::new();
        }
        let failure = match broken {
            Some(detail) => self.ctx.interrupted(detail),
            None if !self.produced => self.ctx.empty(),
            None => self.ctx.failure(
                ProviderErrorKind::Network,
                None,
                "The stream ended before the answer was complete",
            ),
        };
        vec![self.fail(failure)]
    }
}

/// A line-by-line stream parser ([`OpenAiSse`], [`AnthropicSse`]).
pub trait SseParser {
    /// One line of the stream (without its newline).
    fn line(&mut self, line: &str) -> Vec<StreamItem>;
    /// The byte stream ended, cleanly or with a transport error.
    fn end(&mut self, broken: Option<&str>) -> Vec<StreamItem>;
    /// Whether the stream ended (done or failed).
    fn is_over(&self) -> bool;
}

impl SseParser for OpenAiSse {
    fn line(&mut self, line: &str) -> Vec<StreamItem> {
        OpenAiSse::line(self, line)
    }
    fn end(&mut self, broken: Option<&str>) -> Vec<StreamItem> {
        OpenAiSse::end(self, broken)
    }
    fn is_over(&self) -> bool {
        OpenAiSse::is_over(self)
    }
}

impl SseParser for AnthropicSse {
    fn line(&mut self, line: &str) -> Vec<StreamItem> {
        AnthropicSse::line(self, line)
    }
    fn end(&mut self, broken: Option<&str>) -> Vec<StreamItem> {
        AnthropicSse::end(self, broken)
    }
    fn is_over(&self) -> bool {
        AnthropicSse::is_over(self)
    }
}

/// Read `response` through `parser` until the stream is over, sending what
/// `convert` keeps of each item to `tx` (failures are logged first). Stops
/// early when the receiver is gone.
pub async fn pump<P, T>(
    response: reqwest::Response,
    mut parser: P,
    tx: tokio::sync::mpsc::Sender<T>,
    convert: fn(StreamItem) -> Option<T>,
) where
    P: SseParser,
{
    use futures::StreamExt;

    let mut bytes = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    while !parser.is_over() {
        let items = match bytes.next().await {
            Some(Ok(chunk)) => {
                buffer.extend_from_slice(&chunk);
                let mut items = Vec::new();
                for line in take_lines(&mut buffer) {
                    items.extend(parser.line(&line));
                }
                items
            }
            Some(Err(e)) => parser.end(Some(&e.to_string())),
            None => {
                let rest = String::from_utf8_lossy(&std::mem::take(&mut buffer))
                    .trim()
                    .to_string();
                let mut items = parser.line(&rest);
                items.extend(parser.end(None));
                items
            }
        };
        for item in items {
            if let StreamItem::Failed(failure) = &item {
                failure.log();
            }
            if let Some(event) = convert(item) {
                if tx.send(event).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Split complete lines off `buffer` (bytes received so far), leaving any
/// partial line in it. Bytes are decoded per line, so a character split
/// across network chunks stays whole.
pub fn take_lines(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(at) = buffer.iter().position(|b| *b == b'\n') {
        let line: Vec<u8> = buffer.drain(..=at).collect();
        lines.push(String::from_utf8_lossy(&line).trim().to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "sk-or-v1-0123456789abcdef0123456789abcdef";

    fn ctx() -> CallContext {
        CallContext::new("OpenRouter", "vendor/model", KEY)
    }

    fn feed(sse: &mut OpenAiSse, lines: &[&str]) -> Vec<StreamItem> {
        lines.iter().flat_map(|l| sse.line(l)).collect()
    }

    fn content(items: &[StreamItem]) -> String {
        items
            .iter()
            .filter_map(|i| match i {
                StreamItem::Content(c) => Some(c.as_str()),
                _ => None,
            })
            .collect()
    }

    fn failure(items: &[StreamItem]) -> &ProviderFailure {
        match items.last() {
            Some(StreamItem::Failed(f)) => f,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_complete_stream_is_done() {
        let mut sse = OpenAiSse::new(ctx());
        let items = feed(
            &mut sse,
            &[
                ": OPENROUTER PROCESSING",
                r#"data: {"choices":[{"index":0,"delta":{"content":"Sixty "}}]}"#,
                r#"data:{"choices":[{"index":0,"delta":{"content":"days."},"finish_reason":"stop"}]}"#,
                r#"data: {"choices":[],"usage":{"prompt_tokens":3}}"#,
                "data: [DONE]",
            ],
        );
        assert_eq!(content(&items), "Sixty days.");
        assert!(matches!(items.last(), Some(StreamItem::Done)));
        assert!(sse.is_over());
        assert!(sse.end(None).is_empty(), "nothing after [DONE]");
    }

    #[test]
    fn openrouter_mid_stream_error_chunk_fails_the_stream() {
        // OpenRouter's documented mid-stream error (HTTP stays 200).
        let mut sse = OpenAiSse::new(ctx());
        let items = feed(
            &mut sse,
            &[
                r#"data: {"id":"gen-abc123","object":"chat.completion.chunk","created":1234567890,"model":"openai/gpt-4o","provider":"OpenAI","choices":[{"index":0,"delta":{"content":"Part of an"}}]}"#,
                r#"data: {"id":"gen-abc123","object":"chat.completion.chunk","created":1234567890,"model":"openai/gpt-4o","provider":"OpenAI","error":{"code":429,"message":"Rate limit exceeded","metadata":{"error_type":"rate_limit_exceeded"}},"choices":[{"index":0,"delta":{"content":""},"finish_reason":"error"}]}"#,
                "data: [DONE]",
            ],
        );
        assert_eq!(content(&items), "Part of an", "partial text is kept");
        let f = failure(&items);
        assert_eq!(f.kind, ProviderErrorKind::RateLimited);
        assert_eq!(f.status, Some(429));
        assert_eq!(f.upstream.as_deref(), Some("OpenAI"));
        assert_eq!(f.provider, "OpenRouter");
        assert_eq!(f.model, "vendor/model");
        assert!(sse.end(None).is_empty(), "nothing after the failure");
    }

    #[test]
    fn an_upstream_failure_names_the_upstream_and_keeps_its_raw_text() {
        let mut sse = OpenAiSse::new(ctx());
        let items = feed(
            &mut sse,
            &[
                r#"data: {"error":{"code":502,"message":"Provider returned error","metadata":{"provider_name":"Nvidia","raw":"[ERROR: Agent failed (Function process_single_item_agent timed out after 90.0 seconds)]"}},"choices":[{"index":0,"delta":{"content":""},"finish_reason":"error"}]}"#,
            ],
        );
        let f = failure(&items);
        assert_eq!(f.upstream.as_deref(), Some("Nvidia"));
        assert_eq!(f.status, Some(502));
        assert_eq!(f.kind, ProviderErrorKind::Timeout);
        assert!(f.detail.contains("timed out after 90.0 seconds"));
        let shown = f.to_string();
        assert!(shown.starts_with("OpenRouter via Nvidia · vendor/model: the provider did not answer in time (HTTP 502)"), "{shown}");
    }

    #[test]
    fn finish_reason_error_without_an_error_object_fails() {
        let mut sse = OpenAiSse::new(ctx());
        let items = feed(
            &mut sse,
            &[
                r#"data: {"choices":[{"index":0,"delta":{"content":"Half"}}]}"#,
                r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"error"}]}"#,
            ],
        );
        assert_eq!(content(&items), "Half");
        assert_eq!(failure(&items).kind, ProviderErrorKind::ModelUnavailable);
        let mut filtered = OpenAiSse::new(ctx());
        let items = feed(
            &mut filtered,
            &[r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"content_filter"}]}"#],
        );
        assert_eq!(failure(&items).kind, ProviderErrorKind::ContentFiltered);
    }

    #[test]
    fn a_transport_error_mid_stream_is_a_failure_not_done() {
        let mut sse = OpenAiSse::new(ctx());
        let mut items = feed(
            &mut sse,
            &[r#"data: {"choices":[{"index":0,"delta":{"content":"Partial"}}]}"#],
        );
        items.extend(sse.end(Some("error decoding response body: connection reset")));
        assert_eq!(content(&items), "Partial");
        assert_eq!(failure(&items).kind, ProviderErrorKind::Network);
        let mut slow = OpenAiSse::new(ctx());
        let items = slow.end(Some("operation timed out"));
        assert_eq!(failure(&items).kind, ProviderErrorKind::Timeout);
    }

    #[test]
    fn an_empty_or_cut_stream_is_a_failure() {
        let mut empty = OpenAiSse::new(ctx());
        let items = empty.end(None);
        assert_eq!(failure(&items).kind, ProviderErrorKind::EmptyResponse);
        let mut done_but_empty = OpenAiSse::new(ctx());
        let items = done_but_empty.line("data: [DONE]");
        assert_eq!(failure(&items).kind, ProviderErrorKind::EmptyResponse);
        let mut cut = OpenAiSse::new(ctx());
        let mut items = cut.line(r#"data: {"choices":[{"index":0,"delta":{"content":"Half"}}]}"#);
        items.extend(cut.end(None));
        assert_eq!(failure(&items).kind, ProviderErrorKind::Network);
        // A finish reason without [DONE] is complete (some servers omit it).
        let mut no_done = OpenAiSse::new(ctx());
        let mut items = no_done.line(
            r#"data: {"choices":[{"index":0,"delta":{"content":"Whole."},"finish_reason":"stop"}]}"#,
        );
        items.extend(no_done.end(None));
        assert!(matches!(items.last(), Some(StreamItem::Done)));
    }

    #[test]
    fn streamed_tool_calls_are_assembled_in_order() {
        let mut sse = OpenAiSse::new(ctx());
        let items = feed(
            &mut sse,
            &[
                r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"second","arguments":"{}"}}]}}]}"#,
                r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"first","arguments":"{\"q\":"}}]}}]}"#,
                r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":"tool_calls"}]}"#,
                "data: [DONE]",
            ],
        );
        let calls: Vec<&ToolCall> = items
            .iter()
            .filter_map(|i| match i {
                StreamItem::ToolCall(c) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            (calls[0].id.as_str(), calls[0].name.as_str()),
            ("a", "first")
        );
        assert_eq!(calls[0].arguments, r#"{"q":1}"#);
        assert_eq!(calls[1].name, "second");
        assert!(matches!(items.last(), Some(StreamItem::Done)));
    }

    #[test]
    fn a_200_body_with_an_error_is_a_failure() {
        let body: Value = serde_json::from_str(
            r#"{"error":{"code":503,"message":"No available model provider","metadata":{"provider_name":"Chutes"}}}"#,
        )
        .unwrap();
        let f = ctx().body_error(&body).expect("an error");
        assert_eq!(f.kind, ProviderErrorKind::ModelUnavailable);
        assert_eq!(f.status, Some(503));
        assert_eq!(f.upstream.as_deref(), Some("Chutes"));
        let finished: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"x"},"finish_reason":"error"}]}"#,
        )
        .unwrap();
        assert!(ctx().body_error(&finished).is_some());
        let fine: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"x"},"finish_reason":"stop"}]}"#,
        )
        .unwrap();
        assert!(ctx().body_error(&fine).is_none());
        let null_error: Value =
            serde_json::from_str(r#"{"error":null,"choices":[{"message":{"content":"x"}}]}"#)
                .unwrap();
        assert!(ctx().body_error(&null_error).is_none());
    }

    #[test]
    fn http_statuses_map_to_kinds() {
        let c = ctx();
        assert_eq!(
            c.http(
                401,
                r#"{"error":{"code":401,"message":"No auth credentials found"}}"#
            )
            .kind,
            ProviderErrorKind::Auth
        );
        assert_eq!(
            c.http(
                402,
                r#"{"error":{"code":402,"message":"Insufficient credits"}}"#
            )
            .kind,
            ProviderErrorKind::QuotaExhausted
        );
        assert_eq!(
            c.http(408, "Request Timeout").kind,
            ProviderErrorKind::Timeout
        );
        assert_eq!(
            c.http(429, "slow down").kind,
            ProviderErrorKind::RateLimited
        );
        assert_eq!(c.http(503, "").kind, ProviderErrorKind::ModelUnavailable);
        assert_eq!(
            c.http(
                400,
                r#"{"error":{"code":400,"message":"Invalid model parameters"}}"#
            )
            .kind,
            ProviderErrorKind::InvalidRequest
        );
    }

    #[test]
    fn failure_details_never_carry_the_key_and_are_cut() {
        let body = format!(
            "{{\"error\":{{\"message\":\"Authorization: Bearer {KEY} is not valid\",\"code\":401}}}} {}",
            "x".repeat(5_000)
        );
        let f = ctx().http(401, &body);
        assert!(!f.detail.contains(KEY), "{}", f.detail);
        assert!(!f.to_string().contains(KEY));
        assert!(!format!("{f:?}").contains(KEY));
        assert!(!format!("{:?}", ctx()).contains(KEY));
        assert!(f.detail.chars().count() <= MAX_FAILURE_DETAIL_CHARS);
        let mid = OpenAiSse::new(ctx()).line(&format!(
            r#"data: {{"error":{{"code":401,"message":"bad key {KEY}"}}}}"#
        ));
        assert!(!failure(&mid).detail.contains(KEY));
    }

    #[test]
    fn anthropic_stream_errors_and_cuts_fail() {
        let anthropic = || CallContext::new("Anthropic", "claude-x", "sk-ant-secret-key-000000");
        let mut sse = AnthropicSse::new(anthropic());
        let items: Vec<StreamItem> = [
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ]
        .iter()
        .flat_map(|l| sse.line(l))
        .collect();
        assert_eq!(content(&items), "Hello");
        assert_eq!(failure(&items).kind, ProviderErrorKind::ModelUnavailable);

        let mut cut = AnthropicSse::new(anthropic());
        let mut items = cut.line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}"#,
        );
        items.extend(cut.end(None));
        assert_eq!(failure(&items).kind, ProviderErrorKind::Network);
        assert_eq!(
            failure(&AnthropicSse::new(anthropic()).end(None)).kind,
            ProviderErrorKind::EmptyResponse
        );

        let mut done = AnthropicSse::new(anthropic());
        let mut items = done.line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
        );
        items.extend(done.line(r#"data: {"type":"message_stop"}"#));
        assert!(matches!(items.last(), Some(StreamItem::Done)));
        assert!(done.end(None).is_empty());
    }

    #[test]
    fn lines_are_split_on_whole_characters() {
        let mut buffer = "data: é".as_bytes().to_vec();
        let split = buffer.len() - 1;
        let tail = buffer.split_off(split);
        assert!(take_lines(&mut buffer).is_empty());
        buffer.extend_from_slice(&tail);
        buffer.extend_from_slice(b"\r\ndata: [DONE]\npartial");
        assert_eq!(take_lines(&mut buffer), vec!["data: é", "data: [DONE]"]);
        assert_eq!(buffer, b"partial");
    }
}
