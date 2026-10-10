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
    /// Anything else (bad request, context too long, network, …).
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
}

impl ProviderError {
    /// Whether another model could answer instead (rate limit, quota, unavailable).
    pub fn fallback_helps(&self) -> bool {
        matches!(
            self.kind,
            ProviderErrorKind::RateLimited
                | ProviderErrorKind::QuotaExhausted
                | ProviderErrorKind::ModelUnavailable
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
    }
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
    "gateway timeout",
    "internal server error",
    "provider returned error",
    "upstream error",
];

fn contains_any(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| text.contains(m))
}

/// Refusals of the request itself (moderation, a prompt too long for the
/// model): another try or a key change does not help, whatever the status.
const REQUEST_MARKERS: &[&str] = &[
    "requires moderation",
    "was flagged",
    "content_policy",
    "maximum context length",
    "context_length_exceeded",
    "prompt is too long",
];

fn kind_of(lower: &str, status: Option<u16>, retryable: Option<bool>) -> ProviderErrorKind {
    // OpenRouter answers a moderation refusal with 403; it is not a bad key.
    if contains_any(lower, REQUEST_MARKERS) {
        return ProviderErrorKind::Other;
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
    if contains_any(lower, UNAVAILABLE_MARKERS)
        || matches!(status, Some(404) | Some(500..=504) | Some(529))
    {
        return ProviderErrorKind::ModelUnavailable;
    }
    // An upstream "rate-limited" phrasing without the markers above.
    if lower.contains("rate-limited") || lower.contains("ratelimited") {
        return ProviderErrorKind::RateLimited;
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
    let reset_marker = "x-ratelimit-reset";
    if let Some(at) = lower.find(reset_marker) {
        let after = &lower[at + reset_marker.len()..];
        let digits: String = after
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(reset) = digits.parse::<u64>() {
            // Milliseconds since the epoch (13 digits) or seconds (10 digits).
            let reset_ms = if digits.len() >= 13 {
                reset
            } else {
                reset.saturating_mul(1_000)
            };
            if reset_ms > now_ms {
                let secs = (reset_ms - now_ms).div_ceil(1_000);
                return Some(
                    u32::try_from(secs)
                        .unwrap_or(u32::MAX)
                        .min(MAX_RETRY_AFTER_SECS),
                );
            }
        }
    }
    None
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
        assert_eq!(
            kind(
                r#"400 {"error":{"message":"This model's maximum context length is 128000 tokens.","code":400}}"#
            ),
            ProviderErrorKind::Other
        );
        assert_eq!(kind("omp exited"), ProviderErrorKind::Other);
        assert_eq!(
            classify("connection reset", Some(true), NOW_MS).kind,
            ProviderErrorKind::ModelUnavailable
        );
    }

    #[test]
    fn refusals_of_the_request_are_not_key_or_rate_problems() {
        let moderation = r#"403 {"error":{"message":"meta-llama/llama-3-70b requires moderation on OpenRouter. Your input was flagged for \"harassment\".","code":403,"metadata":{"reasons":["harassment"],"flagged_input":"..."}}}"#;
        let e = classify(moderation, None, NOW_MS);
        assert_eq!(e.kind, ProviderErrorKind::Other);
        assert!(!e.fallback_helps());
        let too_long = r#"400 {"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#;
        assert_eq!(kind(too_long), ProviderErrorKind::Other);
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
    }
}
