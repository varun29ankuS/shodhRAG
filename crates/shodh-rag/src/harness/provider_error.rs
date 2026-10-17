//! Classify model-provider failures so the UI can offer the right next step.
//!
//! The agent runtime reports a failed turn as a single message string, usually
//! the provider's HTTP status followed by its JSON error body (OpenRouter,
//! Anthropic, OpenAI, Google). [`classify`] maps that text to a
//! [`ProviderErrorKind`] and, for rate limits, how long to wait before retrying.
//! Classification only reads the text; it never sees or needs credentials.

use serde::{Deserialize, Serialize};

/// What kind of provider failure ended a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    /// Too many requests for now (429); retrying later or another model helps.
    RateLimited,
    /// The account's credits or daily quota are used up; waiting minutes does not help.
    QuotaExhausted,
    /// The model is overloaded, unknown or has no provider serving it (404/5xx).
    ModelUnavailable,
    /// The API key was rejected (401/403).
    Auth,
    /// The provider did not answer in time (408, 504, "timed out").
    Timeout,
    /// The connection broke or never opened, or the stream ended mid-answer.
    Network,
    /// The provider refused the request itself (400/422 without a known reason).
    InvalidRequest,
    /// The question with its sources is longer than the model accepts.
    ContextTooLong,
    /// A content filter or moderation stopped the request or the answer.
    ContentFiltered,
    /// The provider answered with nothing: no text and no tool call.
    EmptyResponse,
    /// Anything else.
    Other,
}

/// A classified provider failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    /// HTTP status, when the message carries one.
    pub status: Option<u16>,
    /// Seconds to wait before retrying, when the provider said.
    pub retry_after_secs: Option<u32>,
    /// The provider behind a router that failed (OpenRouter's
    /// `metadata.provider_name`), when the message names one.
    #[serde(default)]
    pub upstream: Option<String>,
}

impl ProviderError {
    /// Whether another model could answer instead (rate limit, quota,
    /// unavailable, a provider too slow to answer or a broken connection
    /// to it).
    pub fn fallback_helps(&self) -> bool {
        matches!(
            self.kind,
            ProviderErrorKind::RateLimited
                | ProviderErrorKind::QuotaExhausted
                | ProviderErrorKind::ModelUnavailable
                | ProviderErrorKind::Timeout
                | ProviderErrorKind::Network
        )
    }
}

/// Longest wait a provider hint is trusted for; longer hints are capped.
pub const MAX_RETRY_AFTER_SECS: u32 = 3_600;

/// Classify a provider error message. `retryable` is the runtime's own hint,
/// when it gave one; `now_ms` (Unix milliseconds) turns reset timestamps
/// (OpenRouter's `X-RateLimit-Reset`) into a wait.
pub fn classify(message: &str, retryable: Option<bool>, now_ms: u64) -> ProviderError {
    let lower = message.to_ascii_lowercase();
    let status = http_status(&lower);
    let kind = kind_of(&lower, status, retryable);
    let retry_after_secs = match kind {
        ProviderErrorKind::RateLimited | ProviderErrorKind::ModelUnavailable => {
            retry_after(&lower, now_ms)
        }
        _ => None,
    };
    ProviderError {
        kind,
        status,
        retry_after_secs,
        upstream: upstream_provider(message),
    }
}

/// Longest upstream provider name kept.
const MAX_UPSTREAM_CHARS: usize = 64;

/// The upstream provider a router names in its error body
/// (`"provider_name":"Nvidia"`, also inside an escaped JSON string).
pub fn upstream_provider(message: &str) -> Option<String> {
    const KEY: &str = "provider_name";
    let at = message.find(KEY)?;
    let after = message[at + KEY.len()..]
        .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '"' | '\\' | ':'));
    let name: String = after
        .chars()
        .take_while(|c| !matches!(c, '"' | '\\' | ',' | '}') && !c.is_control())
        .take(MAX_UPSTREAM_CHARS + 1)
        .collect();
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_UPSTREAM_CHARS || name == "null" {
        return None;
    }
    Some(name.to_string())
}

/// Longest provider detail shown behind "Details" and written to the log.
pub const MAX_DETAIL_CHARS: usize = 2_000;

/// Provider text fit to show and log: `secret` (the API key, when known)
/// and anything shaped like a credential (`Bearer …`, `sk-…`) replaced by
/// `[REDACTED]`, then cut to `max` characters.
pub fn safe_detail(text: &str, secret: Option<&str>, max: usize) -> String {
    let unkeyed = match secret.map(str::trim) {
        Some(key) if key.len() >= 8 => text.replace(key, "[REDACTED]"),
        _ => text.to_string(),
    };
    super::truncate_chars(mask_credentials(&unkeyed).trim(), max)
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// Replace bearer tokens and `sk-` keys. A masked key such as
/// `sk-proj-****abcd` (as OpenAI echoes it) stays: `*` ends the token.
fn mask_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after_bearer = false;
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if !is_token_char(c) {
            out.push(c);
            if !c.is_whitespace() {
                after_bearer = false;
            }
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(at, next)) = chars.peek() {
            if !is_token_char(next) {
                break;
            }
            end = at + next.len_utf8();
            chars.next();
        }
        let token = &text[start..end];
        let credential =
            (token.starts_with("sk-") && token.len() >= 20) || (after_bearer && token.len() >= 8);
        out.push_str(if credential { "[REDACTED]" } else { token });
        after_bearer = token.eq_ignore_ascii_case("bearer");
    }
    out
}

const AUTH_MARKERS: &[&str] = &[
    "authentication_error",
    "invalid_api_key",
    "invalid api key",
    "incorrect api key",
    "api key not valid",
    "api_key_invalid",
    "invalid x-api-key",
    "no auth credentials",
    "user not found",
    "unauthorized",
    "permission_error",
    "permission_denied",
    "invalid authentication",
];

const QUOTA_MARKERS: &[&str] = &[
    "insufficient_quota",
    "exceeded your current quota",
    "credit balance is too low",
    "insufficient credits",
    "requires more credits",
    "free-models-per-day",
    "add 10 credits",
    "billing_hard_limit",
    "quota exceeded",
    "out of credits",
    "payment required",
];

const RATE_MARKERS: &[&str] = &[
    "rate_limit",
    "rate limit",
    "rate-limit",
    "too many requests",
    "resource_exhausted",
    "free-models-per-min",
    "requests per minute",
    "tokens per minute",
];

const UNAVAILABLE_MARKERS: &[&str] = &[
    "overloaded",
    "no endpoints found",
    "no available providers",
    "no allowed providers",
    "model_not_found",
    "not_found_error",
    "does not exist",
    "is not a valid model",
    "model not found",
    "temporarily unavailable",
    "service unavailable",
    "\"unavailable\"",
    "bad gateway",
    "internal server error",
    "provider returned error",
    "upstream error",
    "provider_unavailable",
    "no endpoints available",
];

/// The provider's stream to the runtime was cut or stalled (omp's own
/// wording, and a `network_error` finish reason): a provider-side outage,
/// not the person's connection.
const PROVIDER_STREAM_MARKERS: &[&str] = &[
    "stream closed before",
    "stream ended before",
    "stream stalled",
    "finish_reason: network_error",
];

fn contains_any(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| text.contains(m))
}

/// Moderation and content filters (OpenRouter answers a moderation refusal
/// with 403; it is not a bad key).
const FILTER_MARKERS: &[&str] = &[
    "requires moderation",
    "was flagged",
    "content_policy",
    "content_filter",
    "content filter",
];

/// A prompt longer than the model accepts.
const CONTEXT_MARKERS: &[&str] = &[
    "context length",
    "context_length_exceeded",
    "context window",
    "prompt is too long",
    "input is too long",
    "maximum prompt length",
];

const TIMEOUT_MARKERS: &[&str] = &[
    "timed out",
    "timeout",
    "time out",
    "deadline exceeded",
    "deadline_exceeded",
    "did not answer in time",
];

const EMPTY_MARKERS: &[&str] = &[
    "empty response",
    "empty answer",
    "no choices returned",
    "returned no choices",
    "no content returned",
];

const NETWORK_MARKERS: &[&str] = &[
    "connection reset",
    "connection refused",
    "connection closed",
    "connection aborted",
    "connection error",
    "econnreset",
    "econnrefused",
    "enotfound",
    "network error",
    "fetch failed",
    "socket hang up",
    "dns error",
    "error sending request",
    "broken pipe",
    "unexpected eof",
];

const INVALID_REQUEST_MARKERS: &[&str] = &["invalid_request", "bad request", "invalid request"];

fn kind_of(lower: &str, status: Option<u16>, retryable: Option<bool>) -> ProviderErrorKind {
    // Refusals of the request itself first: another try or a key change does
    // not help, whatever the status.
    if contains_any(lower, FILTER_MARKERS) {
        return ProviderErrorKind::ContentFiltered;
    }
    if contains_any(lower, CONTEXT_MARKERS) || status == Some(413) {
        return ProviderErrorKind::ContextTooLong;
    }
    // Quota before auth: some providers send quota failures as 403.
    if contains_any(lower, QUOTA_MARKERS) || status == Some(402) {
        return ProviderErrorKind::QuotaExhausted;
    }
    if contains_any(lower, AUTH_MARKERS) || matches!(status, Some(401) | Some(403)) {
        return ProviderErrorKind::Auth;
    }
    if contains_any(lower, RATE_MARKERS) || status == Some(429) {
        return ProviderErrorKind::RateLimited;
    }
    if contains_any(lower, PROVIDER_STREAM_MARKERS) {
        return ProviderErrorKind::ModelUnavailable;
    }
    if contains_any(lower, TIMEOUT_MARKERS) || matches!(status, Some(408) | Some(504)) {
        return ProviderErrorKind::Timeout;
    }
    if contains_any(lower, EMPTY_MARKERS) {
        return ProviderErrorKind::EmptyResponse;
    }
    if contains_any(lower, NETWORK_MARKERS) {
        return ProviderErrorKind::Network;
    }
    if contains_any(lower, UNAVAILABLE_MARKERS)
        || matches!(status, Some(404) | Some(500..=503) | Some(529))
    {
        return ProviderErrorKind::ModelUnavailable;
    }
    // An upstream "rate-limited" phrasing without the markers above.
    if lower.contains("rate-limited") || lower.contains("ratelimited") {
        return ProviderErrorKind::RateLimited;
    }
    if contains_any(lower, INVALID_REQUEST_MARKERS) || matches!(status, Some(400) | Some(422)) {
        return ProviderErrorKind::InvalidRequest;
    }
    if retryable == Some(true) && status.is_none() {
        return ProviderErrorKind::ModelUnavailable;
    }
    ProviderErrorKind::Other
}

/// A plausible HTTP error status in the message: a leading `429 …`, or
/// `"code": 429` / `status 429` / `http 429` / `error 429` in the body.
fn http_status(lower: &str) -> Option<u16> {
    let valid = |n: u16| (400..=599).contains(&n);
    let leading: String = lower
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if leading.len() == 3 {
        if let Ok(n) = leading.parse::<u16>() {
            if valid(n) {
                return Some(n);
            }
        }
    }
    for marker in [
        "\"code\":",
        "\"status\":",
        "status code",
        "status:",
        "status ",
        "http ",
        "error ",
    ] {
        let mut rest = lower;
        while let Some(at) = rest.find(marker) {
            let after = rest[at + marker.len()..].trim_start();
            let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
            if digits.len() == 3 {
                if let Ok(n) = digits.parse::<u16>() {
                    if valid(n) {
                        return Some(n);
                    }
                }
            }
            rest = &rest[at + marker.len()..];
        }
    }
    None
}

/// Seconds to wait from hints like "try again in 20s", "retry after 30
/// seconds", `"retryDelay": "17s"`, `Retry-After: 12` or an
/// `X-RateLimit-Reset` epoch in milliseconds.
fn retry_after(lower: &str, now_ms: u64) -> Option<u32> {
    for marker in [
        "try again in",
        "retry in",
        "retry after",
        "retry-after",
        "retrydelay",
        "retry_after",
        "please wait",
    ] {
        if let Some(at) = lower.find(marker) {
            if let Some(secs) = duration_after(&lower[at + marker.len()..]) {
                return Some(secs);
            }
        }
    }
    let reset_ms = limit_reset_at_ms(lower, now_ms)?;
    let secs = (reset_ms - now_ms).div_ceil(1_000);
    Some(
        u32::try_from(secs)
            .unwrap_or(u32::MAX)
            .min(MAX_RETRY_AFTER_SECS),
    )
}

/// The moment an `X-RateLimit-Reset` value names, in Unix milliseconds: the
/// first number in `value`, read as milliseconds since the epoch (13
/// digits) or seconds (10 digits).
pub fn reset_epoch_ms(value: &str) -> Option<u64> {
    let digits: String = value
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    let reset = digits.parse::<u64>().ok()?;
    Some(if digits.len() >= 13 {
        reset
    } else {
        reset.saturating_mul(1_000)
    })
}

/// When the limit a provider message reports resets (Unix milliseconds):
/// the `X-RateLimit-Reset` it carries (OpenRouter puts the response headers
/// in its error body), when that is still ahead of `now_ms`. Unlike
/// `retry_after_secs` it is not capped, so a daily limit keeps its real
/// reset time.
pub fn limit_reset_at_ms(message: &str, now_ms: u64) -> Option<u64> {
    const MARKER: &str = "x-ratelimit-reset";
    let lower = message.to_ascii_lowercase();
    let at = lower.find(MARKER)?;
    reset_epoch_ms(&lower[at + MARKER.len()..]).filter(|reset| *reset > now_ms)
}

/// The first duration in `text` (skipping punctuation such as `": "`): a
/// number with an optional unit (`ms`, `s`, `sec`, `seconds`, `m`, `min`,
/// `minutes`); a bare number is seconds. Rounded up to whole seconds.
fn duration_after(text: &str) -> Option<u32> {
    let trimmed = text.trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, ':' | '"' | '\'' | '=' | '(')
    });
    let number: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if number.is_empty() {
        return None;
    }
    let value: f64 = number.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let unit: String = trimmed[number.len()..]
        .trim_start()
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    let secs = match unit.as_str() {
        "ms" | "msec" | "milliseconds" | "millisecond" => value / 1_000.0,
        "m" | "min" | "mins" | "minute" | "minutes" => value * 60.0,
        "h" | "hour" | "hours" => value * 3_600.0,
        _ => value,
    };
    let secs = secs.ceil().max(1.0);
    Some(if secs >= f64::from(MAX_RETRY_AFTER_SECS) {
        MAX_RETRY_AFTER_SECS
    } else {
        // In range: 1 ..= MAX_RETRY_AFTER_SECS.
        secs as u32
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: u64 = 1_759_750_000_000;

    fn kind(message: &str) -> ProviderErrorKind {
        classify(message, None, NOW_MS).kind
    }

    #[test]
    fn openrouter_rate_limits_with_reset_header() {
        let body = r#"429 {"error":{"message":"Rate limit exceeded: free-models-per-min. ","code":429,"metadata":{"headers":{"X-RateLimit-Limit":"20","X-RateLimit-Remaining":"0","X-RateLimit-Reset":"1759750012000"}}},"user_id":"user_x"}"#;
        let e = classify(body, Some(true), NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::RateLimited);
        assert_eq!(e.status, Some(429));
        assert_eq!(e.retry_after_secs, Some(12));
        assert!(e.fallback_helps());
    }

    #[test]
    fn openrouter_upstream_rate_limit_of_a_free_model() {
        let body = r#"429 Provider returned error {"error":{"message":"Provider returned error","code":429,"metadata":{"raw":"nvidia/nemotron-3-super-120b-a12b:free is temporarily rate-limited upstream. Please retry shortly, or add your own key to accumulate your rate limits: https://openrouter.ai/settings/integrations","provider_name":"Nvidia"}}}"#;
        let e = classify(body, None, NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::RateLimited);
        assert_eq!(e.retry_after_secs, None);
    }

    #[test]
    fn a_daily_limit_keeps_its_reset_time_uncapped() {
        let tomorrow = NOW_MS + 20 * 3_600_000;
        let body = format!(
            r#"429 {{"error":{{"message":"Rate limit exceeded: free-models-per-day.","code":429,"metadata":{{"headers":{{"X-RateLimit-Limit":"50","X-RateLimit-Remaining":"0","X-RateLimit-Reset":"{tomorrow}"}}}}}}}}"#
        );
        let e = classify(&body, None, NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::QuotaExhausted);
        assert_eq!(limit_reset_at_ms(&body, NOW_MS), Some(tomorrow));
        // Seconds since the epoch read as such; a reset already past is none.
        assert_eq!(reset_epoch_ms("1759750012"), Some(1_759_750_012_000));
        assert_eq!(
            limit_reset_at_ms(r#""X-RateLimit-Reset":"1759740000000""#, NOW_MS),
            None
        );
        assert_eq!(limit_reset_at_ms("no header here", NOW_MS), None);
    }

    #[test]
    fn openrouter_daily_free_quota_is_not_a_short_wait() {
        let body = r#"429 {"error":{"message":"Rate limit exceeded: free-models-per-day. Add 10 credits to unlock 1000 free model requests per day","code":429}}"#;
        let e = classify(body, None, NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::QuotaExhausted);
        assert_eq!(e.retry_after_secs, None);
        let credits = r#"402 {"error":{"message":"This request requires more credits, or fewer max_tokens. You requested up to 4096 tokens, but can only afford 1200.","code":402}}"#;
        assert_eq!(kind(credits), ProviderErrorKind::QuotaExhausted);
    }

    #[test]
    fn openrouter_model_unavailable() {
        assert_eq!(
            kind(
                r#"404 {"error":{"message":"No endpoints found for nvidia/nemotron-x:free.","code":404}}"#
            ),
            ProviderErrorKind::ModelUnavailable
        );
        assert_eq!(
            kind(r#"502 {"error":{"message":"Provider returned error","code":502}}"#),
            ProviderErrorKind::ModelUnavailable
        );
        assert_eq!(
            kind(r#"503 {"error":{"message":"No available providers for this model","code":503}}"#),
            ProviderErrorKind::ModelUnavailable
        );
    }

    #[test]
    fn openrouter_bad_key() {
        let e = classify(
            r#"401 {"error":{"message":"No auth credentials found","code":401}}"#,
            Some(false),
            NOW_MS,
        );
        assert_eq!(e.kind, ProviderErrorKind::Auth);
        assert!(!e.fallback_helps());
        assert_eq!(kind("401 Unauthorized"), ProviderErrorKind::Auth);
        assert_eq!(
            kind(r#"401 {"error":{"message":"User not found.","code":401}}"#),
            ProviderErrorKind::Auth
        );
    }

    #[test]
    fn anthropic_errors() {
        let rate = r#"429 {"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed the rate limit for your organization of 50,000 input tokens per minute. Please try again later."}}"#;
        assert_eq!(kind(rate), ProviderErrorKind::RateLimited);
        let overloaded =
            r#"529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let e = classify(overloaded, None, NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::ModelUnavailable);
        assert_eq!(e.status, Some(529));
        let auth = r#"401 {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(kind(auth), ProviderErrorKind::Auth);
        let credit = r#"400 {"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits."}}"#;
        assert_eq!(kind(credit), ProviderErrorKind::QuotaExhausted);
        let model = r#"404 {"type":"error","error":{"type":"not_found_error","message":"model: claude-nope"}}"#;
        assert_eq!(kind(model), ProviderErrorKind::ModelUnavailable);
    }

    #[test]
    fn openai_errors() {
        let rate = r#"429 {"error":{"message":"Rate limit reached for gpt-5-mini in organization org-x on tokens per min (TPM): Limit 200000, Used 199000, Requested 3000. Please try again in 1.2s.","type":"tokens","param":null,"code":"rate_limit_exceeded"}}"#;
        let e = classify(rate, None, NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::RateLimited);
        assert_eq!(e.retry_after_secs, Some(2));
        let quota = r#"429 {"error":{"message":"You exceeded your current quota, please check your plan and billing details.","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}"#;
        assert_eq!(kind(quota), ProviderErrorKind::QuotaExhausted);
        let auth = r#"401 {"error":{"message":"Incorrect API key provided: sk-proj-****abcd.","type":"invalid_request_error","param":null,"code":"invalid_api_key"}}"#;
        assert_eq!(kind(auth), ProviderErrorKind::Auth);
        let model = r#"404 {"error":{"message":"The model `gpt-9` does not exist or you do not have access to it.","type":"invalid_request_error","code":"model_not_found"}}"#;
        assert_eq!(kind(model), ProviderErrorKind::ModelUnavailable);
    }

    #[test]
    fn gemini_errors() {
        let rate = r#"429 {"error":{"code":429,"message":"Resource has been exhausted (e.g. check quota).","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"17s"}]}}"#;
        let e = classify(rate, None, NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::RateLimited);
        assert_eq!(e.retry_after_secs, Some(17));
        let auth = r#"400 {"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}"#;
        assert_eq!(kind(auth), ProviderErrorKind::Auth);
        let busy = r#"503 {"error":{"code":503,"message":"The model is overloaded. Please try again later.","status":"UNAVAILABLE"}}"#;
        assert_eq!(kind(busy), ProviderErrorKind::ModelUnavailable);
    }

    #[test]
    fn other_errors_stay_other() {
        assert_eq!(kind("omp exited"), ProviderErrorKind::Other);
        assert_eq!(
            classify("something odd", Some(true), NOW_MS).kind,
            ProviderErrorKind::ModelUnavailable,
            "the runtime's retry hint without a status reads as a passing outage"
        );
    }

    #[test]
    fn refusals_of_the_request_are_not_key_or_rate_problems() {
        let moderation = r#"403 {"error":{"message":"meta-llama/llama-3-70b requires moderation on OpenRouter. Your input was flagged for \"harassment\".","code":403,"metadata":{"reasons":["harassment"],"flagged_input":"..."}}}"#;
        let e = classify(moderation, None, NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::ContentFiltered);
        assert!(!e.fallback_helps());
        let too_long = r#"400 {"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#;
        assert_eq!(kind(too_long), ProviderErrorKind::ContextTooLong);
        assert_eq!(
            kind(
                r#"400 {"error":{"message":"This model's maximum context length is 128000 tokens.","code":400}}"#
            ),
            ProviderErrorKind::ContextTooLong
        );
        assert_eq!(
            kind("Provider finish_reason: content_filter"),
            ProviderErrorKind::ContentFiltered
        );
        // OpenRouter's privacy settings leave no provider (common for
        // :free models): the model is unavailable, not filtered.
        let privacy = r#"404 {"error":{"message":"No endpoints available matching your guardrail restrictions and data policy. Configure: https://openrouter.ai/settings/privacy","code":404}}"#;
        let e = classify(privacy, None, NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::ModelUnavailable);
        assert!(e.fallback_helps());
        assert_eq!(
            kind("No endpoints available matching your guardrail restrictions and data policy. Configure: https://openrouter.ai/settings/privacy"),
            ProviderErrorKind::ModelUnavailable,
            "also without a status"
        );
        assert_eq!(
            kind(r#"{"error":{"message":"x","metadata":{"error_type":"provider_unavailable"}}}"#),
            ProviderErrorKind::ModelUnavailable
        );
    }

    #[test]
    fn the_field_report_is_a_timeout() {
        // What the tester saw (an upstream provider's own failure text).
        let field = "[ERROR: Agent failed (Function process_single_item_agent timed out after 90.0 seconds), API failed (API request returned None after all retries)]";
        let e = classify(field, None, NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::Timeout);
        assert_eq!(e.status, None);
        assert!(e.fallback_helps(), "another model may answer in time");
        assert_eq!(e.retry_after_secs, None);
        assert_eq!(
            kind(r#"408 {"error":{"message":"Request timed out","code":408}}"#),
            ProviderErrorKind::Timeout
        );
        assert_eq!(kind("504 Gateway Timeout"), ProviderErrorKind::Timeout);
        assert_eq!(
            kind("OpenAI completions stream timed out while waiting for the first event"),
            ProviderErrorKind::Timeout
        );
    }

    #[test]
    fn openrouter_status_codes_map_to_kinds() {
        let body = |code: u16, message: &str| {
            format!(r#"{code} {{"error":{{"code":{code},"message":"{message}"}}}}"#)
        };
        let cases = [
            (
                400,
                "Invalid model parameters",
                ProviderErrorKind::InvalidRequest,
            ),
            (401, "Invalid credentials", ProviderErrorKind::Auth),
            (
                402,
                "Insufficient credits",
                ProviderErrorKind::QuotaExhausted,
            ),
            (
                403,
                "Your input was flagged",
                ProviderErrorKind::ContentFiltered,
            ),
            (408, "Your request timed out", ProviderErrorKind::Timeout),
            (
                429,
                "You are being rate limited",
                ProviderErrorKind::RateLimited,
            ),
            (
                502,
                "Your chosen model is down",
                ProviderErrorKind::ModelUnavailable,
            ),
            (
                503,
                "No available model provider",
                ProviderErrorKind::ModelUnavailable,
            ),
        ];
        for (code, message, expected) in cases {
            let e = classify(&body(code, message), None, NOW_MS);
            assert_eq!(e.kind, expected, "{code} {message}");
            assert_eq!(e.status, Some(code));
        }
    }

    #[test]
    fn network_and_empty_answers() {
        assert_eq!(
            classify("connection reset by peer", Some(true), NOW_MS).kind,
            ProviderErrorKind::Network
        );
        assert_eq!(
            kind("error sending request for url (https://openrouter.ai/api/v1/chat/completions)"),
            ProviderErrorKind::Network
        );
        assert!(classify("connection reset by peer", None, NOW_MS).fallback_helps());
        // Provider-side stream cuts (omp's wording) are outages, not the
        // person's connection.
        for cut in [
            "OpenAI completions stream closed before a finish_reason was received",
            "OpenAI completions stream stalled while waiting for the next event",
            "Provider finish_reason: network_error",
        ] {
            let e = classify(cut, None, NOW_MS);
            assert_eq!(e.kind, ProviderErrorKind::ModelUnavailable, "{cut}");
            assert!(e.fallback_helps(), "{cut}");
        }
        assert_eq!(
            kind("The provider sent an empty response"),
            ProviderErrorKind::EmptyResponse
        );
        assert_eq!(
            kind("Provider returned error finish_reason"),
            ProviderErrorKind::ModelUnavailable
        );
    }

    #[test]
    fn the_upstream_provider_is_named() {
        let body = r#"502 {"error":{"message":"Provider returned error","code":502,"metadata":{"raw":"upstream died","provider_name":"Nvidia"}}}"#;
        assert_eq!(
            classify(body, None, NOW_MS).upstream.as_deref(),
            Some("Nvidia")
        );
        // Inside an escaped JSON string.
        assert_eq!(
            upstream_provider(r#"{\"provider_name\": \"Chutes\"}"#).as_deref(),
            Some("Chutes")
        );
        assert_eq!(upstream_provider(r#"{"provider_name":null}"#), None);
        assert_eq!(upstream_provider("no name here"), None);
        let long = format!(r#"{{"provider_name":"{}"}}"#, "x".repeat(200));
        assert_eq!(upstream_provider(&long), None);
    }

    #[test]
    fn details_never_carry_credentials() {
        let key = "sk-or-v1-0123456789abcdef0123456789abcdef";
        let text =
            format!("401 Authorization: Bearer {key} rejected; key {key}; other sk-proj-****abcd");
        let safe = safe_detail(&text, Some(key), MAX_DETAIL_CHARS);
        assert!(!safe.contains(key), "{safe}");
        assert!(!safe.contains("0123456789abcdef"), "{safe}");
        assert!(safe.contains("Bearer [REDACTED]"), "{safe}");
        assert!(
            safe.contains("sk-proj-****abcd"),
            "a masked key stays: {safe}"
        );
        // Without the key: credential-shaped tokens are still masked.
        let unknown = safe_detail(
            "Authorization: Bearer abcdefgh12345678 and sk-ant-api03-ABCDEFGHIJKLMNOP",
            None,
            MAX_DETAIL_CHARS,
        );
        assert!(!unknown.contains("abcdefgh12345678"), "{unknown}");
        assert!(!unknown.contains("ABCDEFGHIJKLMNOP"), "{unknown}");
        // Cut on a character boundary.
        let long = "é".repeat(5_000);
        let cut = safe_detail(&long, None, 100);
        assert_eq!(cut.chars().count(), 100);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn retry_hints_are_parsed_and_capped() {
        assert_eq!(
            retry_after("please retry after 30 seconds", NOW_MS),
            Some(30)
        );
        assert_eq!(retry_after("try again in 250ms", NOW_MS), Some(1));
        assert_eq!(retry_after("try again in 2 minutes", NOW_MS), Some(120));
        assert_eq!(
            retry_after("retry-after: 99999", NOW_MS),
            Some(MAX_RETRY_AFTER_SECS)
        );
        assert_eq!(
            retry_after("x-ratelimit-reset\":\"1759749000000", NOW_MS),
            None,
            "a reset in the past is no hint"
        );
        assert_eq!(
            retry_after("x-ratelimit-reset: 1759750030", NOW_MS),
            Some(30),
            "epoch seconds"
        );
        assert_eq!(retry_after("no hint here", NOW_MS), None);
    }

    #[test]
    fn statuses_are_found_only_in_the_error_range() {
        assert_eq!(http_status("429 too many"), Some(429));
        assert_eq!(http_status("{\"code\": 503}"), Some(503));
        assert_eq!(http_status("status code 401"), Some(401));
        assert_eq!(http_status("200 tokens left"), None);
        assert_eq!(http_status("used 1234 tokens"), None);
    }

    #[test]
    fn serialises_for_the_ui() {
        let e = classify("429 rate limit; try again in 5s", None, NOW_MS);
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "rate_limited");
        assert_eq!(v["status"], 429);
        assert_eq!(v["retryAfterSecs"], 5);
        assert_eq!(v["upstream"], serde_json::Value::Null);
        assert_eq!(
            serde_json::to_value(ProviderErrorKind::ContextTooLong).unwrap(),
            "context_too_long"
        );
        // Stored before `upstream` existed.
        let old: ProviderError =
            serde_json::from_str(r#"{"kind":"auth","status":401,"retryAfterSecs":null}"#).unwrap();
        assert_eq!(old.upstream, None);
    }
}
