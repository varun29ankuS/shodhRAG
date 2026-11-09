//! Secret redaction for Recall, applied at index time.
//!
//! Every string Recall takes from a transcript passes through one [`Redactor`] before
//! it reaches anything else: the chunks that are embedded and keyword-indexed, the
//! entities, the catalog, the receipt viewer, the agent's tools and every error record.
//! Transcripts hold what the user pasted and what their tools printed, which includes
//! their own API keys, so what is redacted here is never indexed, shown or sent to a
//! model.
//!
//! What is redacted:
//! - private key blocks (PEM, OpenSSH, PGP), including a block cut off before its end;
//! - tokens with a provider's prefix (Anthropic, OpenAI, OpenRouter, GitHub, GitLab,
//!   Slack, Google, Hugging Face, Groq, xAI, Perplexity, npm, PyPI, Stripe, SendGrid,
//!   AWS access key ids, Telegram bots, JWTs, Slack and Discord webhooks), also when a
//!   terminal wrapped one over several lines;
//! - credentials in headers (`Authorization`, `Bearer`, `x-api-key`, cookies), in URLs
//!   (`user:password@host`, `?token=`), on command lines (`--password x`);
//! - the values of `.env`-style lines and of assignments whose name says key, token,
//!   secret or password, quoted or not;
//! - long random-looking strings of mixed letters and digits (keys without a known
//!   prefix);
//! - the user's own patterns (regular expressions).
//!
//! Over-masking a hash costs a little recall; leaking a key costs the user the key, so
//! the rules err towards masking.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use sha2::{Digest, Sha256};

/// What a secret is replaced with.
pub const REDACTED: &str = "[REDACTED]";

/// Version of the built-in rules. Raised whenever a rule changes, so every session is
/// indexed again under the new rules ([`Redactor::fingerprint`]).
pub const RULES_VERSION: u32 = 1;

/// Longest user pattern accepted (characters).
pub const MAX_PATTERN_CHARS: usize = 300;
/// Most user patterns accepted.
pub const MAX_PATTERNS: usize = 50;

/// A user pattern that is not a valid regular expression, or too long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    pub pattern: String,
    pub reason: String,
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\"{}\" is not a pattern Shodh can use: {}",
            self.pattern, self.reason
        )
    }
}

impl std::error::Error for PatternError {}

/// Redacts secrets from transcript text with the built-in rules and the user's patterns.
#[derive(Debug, Clone)]
pub struct Redactor {
    user: Vec<Regex>,
    fingerprint: String,
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new(&[]).unwrap_or_else(|_| Self {
            user: Vec::new(),
            fingerprint: fingerprint(&[]),
        })
    }
}

fn fingerprint(patterns: &[String]) -> String {
    let mut hash = Sha256::new();
    hash.update(RULES_VERSION.to_le_bytes());
    for pattern in patterns {
        hash.update((pattern.len() as u64).to_le_bytes());
        hash.update(pattern.as_bytes());
    }
    hash.finalize()
        .iter()
        .take(12)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Checks the user's patterns: each must be a regular expression of at most
/// [`MAX_PATTERN_CHARS`], at most [`MAX_PATTERNS`] of them, and none may match the empty
/// text (it would redact nothing, or everything, between every character).
pub fn check_patterns(patterns: &[String]) -> Result<Vec<Regex>, PatternError> {
    if patterns.len() > MAX_PATTERNS {
        return Err(PatternError {
            pattern: patterns[MAX_PATTERNS].clone(),
            reason: format!("at most {MAX_PATTERNS} patterns are kept"),
        });
    }
    patterns
        .iter()
        .map(|pattern| {
            let error = |reason: String| PatternError {
                pattern: pattern.clone(),
                reason,
            };
            if pattern.trim().is_empty() {
                return Err(error("it is empty".to_string()));
            }
            if pattern.chars().count() > MAX_PATTERN_CHARS {
                return Err(error(format!(
                    "it is longer than {MAX_PATTERN_CHARS} characters"
                )));
            }
            let regex = regex::RegexBuilder::new(pattern)
                .size_limit(1 << 20)
                .build()
                .map_err(|e| error(e.to_string()))?;
            if regex.is_match("") {
                return Err(error("it matches empty text".to_string()));
            }
            Ok(regex)
        })
        .collect()
}

impl Redactor {
    /// The built-in rules and `patterns` (the user's own, regular expressions).
    pub fn new(patterns: &[String]) -> Result<Self, PatternError> {
        Ok(Self {
            user: check_patterns(patterns)?,
            fingerprint: fingerprint(patterns),
        })
    }

    /// Identifies the rules and patterns: text indexed under another fingerprint is
    /// indexed again.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// `text` with every secret replaced by [`REDACTED`].
    pub fn redact(&self, text: &str) -> String {
        self.redact_counted(text).0
    }

    /// [`Self::redact`], and how many secrets were replaced.
    pub fn redact_counted(&self, text: &str) -> (String, usize) {
        let mut count = 0usize;
        let mut out = Cow::Borrowed(text);
        for rule in RULES.iter() {
            out = apply(out, rule, &mut count);
        }
        if let Some(wrapped) = redact_wrapped_tokens(&out, &mut count) {
            out = Cow::Owned(wrapped);
        }
        if let Some(random) = redact_random_strings(&out, &mut count) {
            out = Cow::Owned(random);
        }
        for regex in &self.user {
            if regex.is_match(&out) {
                let replaced = regex.replace_all(&out, |_: &Captures| {
                    count += 1;
                    REDACTED
                });
                out = Cow::Owned(replaced.into_owned());
            }
        }
        (out.into_owned(), count)
    }

    /// A line that could not be read as JSON, made fit to keep and show: its JSON escapes
    /// undone (so an escaped key is seen whole), redacted, and cut to `max_chars`.
    pub fn redact_raw_line(&self, raw: &[u8], max_chars: usize) -> String {
        let text = String::from_utf8_lossy(raw);
        let unescaped = unescape_json(&text);
        let redacted = self.redact(&unescaped);
        let mut cut: String = redacted.chars().take(max_chars).collect();
        if cut.len() < redacted.len() {
            cut.push('…');
        }
        cut
    }
}

/// One built-in rule: where it matches, the text is replaced by what `replace` makes of
/// the captures.
struct Rule {
    regex: Regex,
    replace: fn(&Captures) -> String,
}

fn whole(_: &Captures) -> String {
    REDACTED.to_string()
}

/// Keeps the first capture group (a name, a scheme), replaces the rest.
fn keep_first(caps: &Captures) -> String {
    format!("{}{REDACTED}", &caps[1])
}

/// An `.env`-style line: keeps the prefix, the name and the `=`, replaces the value.
fn keep_assignment(caps: &Captures) -> String {
    let value = caps.get(4).map_or("", |m| m.as_str());
    if is_placeholder(value) || (!env_style_name(&caps[2]) && looks_like_code(value)) {
        return caps[0].to_string();
    }
    format!("{}{}{}{REDACTED}", &caps[1], &caps[2], &caps[3])
}

/// A name written like an environment variable (`DB_PASSWORD`): no lower-case letter.
/// Its value is always a value, never code.
fn env_style_name(name: &str) -> bool {
    !name.chars().any(|c| c.is_ascii_lowercase())
}

/// A value that reads as code rather than a secret (`token = tokens.next()`,
/// `password = form.password`, `key = map[k]`): it starts with a lower-case identifier,
/// holds no digit, and is a field, a call, a path or an index. A plain word is not code:
/// `password: changeme` is a password.
fn looks_like_code(value: &str) -> bool {
    let v = value.trim().trim_end_matches(';').trim_end_matches(',');
    let starts_like_identifier = v
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_' || c == '&' || c == '*');
    starts_like_identifier
        && !v.chars().any(|c| c.is_ascii_digit())
        && v.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '_' | '.'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | ':'
                        | '&'
                        | '*'
                        | '!'
                        | '?'
                        | ' '
                        | ','
                        | '\''
                        | '"'
                )
        })
        && v.contains(['(', '.', '[', ':'])
}

/// Whether a secret-bearing name is about a password or a secret (any value counts) rather
/// than a key or token (only key-shaped values count: `max_tokens` is no secret).
fn password_like_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "passw",
        "pwd",
        "passphrase",
        "secret",
        "credential",
        "cookie",
        "dsn",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// A value shaped like a key or token: a digit in it, or long.
fn key_shaped(value: &str) -> bool {
    let v = value.trim();
    v.len() >= 8 && !v.contains(' ') && (v.chars().any(|c| c.is_ascii_digit()) || v.len() >= 20)
}

/// A value that only names where a secret comes from (`${TOKEN}`, `<your key>`,
/// `process.env.KEY`), or is empty: kept, it holds nothing secret and tells what the
/// session did.
fn is_placeholder(value: &str) -> bool {
    let v = value.trim().trim_matches(|c| c == '"' || c == '\'');
    v.is_empty()
        || v.starts_with('$')
        || v.starts_with('<')
        || v.starts_with('{')
        || v.starts_with("process.env")
        || v.starts_with("os.environ")
        || v.starts_with("env(")
        || v.starts_with("std::env")
        || v.eq_ignore_ascii_case("null")
        || v.eq_ignore_ascii_case("none")
        || v.eq_ignore_ascii_case("true")
        || v.eq_ignore_ascii_case("false")
        || v.chars()
            .all(|c| c == '*' || c == 'x' || c == 'X' || c == '.')
        || v == REDACTED
}

/// Words that make a name secret-bearing (`API_KEY`, `db_password`, `authToken`).
const SECRET_NAME: &str = r"(?:api[_-]?key|apikey|secret|token|passw(?:or)?d|passphrase|pwd|private[_-]?key|client[_-]?secret|access[_-]?key|credential|auth[_-]?(?:token|key|header)|authorization|session[_-]?key|cookie|dsn|signing[_-]?key|webhook[_-]?url)";

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let rule = |pattern: &str, replace: fn(&Captures) -> String| Rule {
        regex: Regex::new(pattern)
            .unwrap_or_else(|e| panic!("recall redaction rule {pattern}: {e}")),
        replace,
    };
    vec![
        // Private key blocks, to their end or to the end of the text (an output cut short).
        rule(
            r"(?s)-----BEGIN[ A-Z0-9]*PRIVATE KEY[ A-Z0-9]*-----.*?(?:-----END[ A-Z0-9]*PRIVATE KEY[ A-Z0-9]*-----|\z)",
            whole,
        ),
        // PuTTY key files.
        rule(
            r"(?s)PuTTY-User-Key-File-\d+:.*?(?:Private-MAC: *[0-9a-fA-F]+|\z)",
            whole,
        ),
        // Webhook URLs carry their secret in the path.
        rule(
            r"https://hooks\.slack\.com/(?:services|workflows|triggers)/[A-Za-z0-9/_-]+",
            whole,
        ),
        rule(
            r"https://(?:ptb\.|canary\.)?discord(?:app)?\.com/api/webhooks/\d+/[A-Za-z0-9_-]+",
            whole,
        ),
        // Credentials in a URL: scheme://user:password@host.
        rule(
            r#"(?i)\b([a-z][a-z0-9+.-]*://[^\s:/@"'<>]+:)[^\s@/"'<>]+@"#,
            |c| format!("{}{REDACTED}@", &c[1]),
        ),
        // Secret query parameters.
        rule(
            r#"(?i)([?&](?:access_token|refresh_token|id_token|token|api_key|apikey|api-key|key|secret|client_secret|password|passwd|pwd|sig|signature|auth|code_verifier)=)[^&\s"'#<>]+"#,
            keep_first,
        ),
        // Authorization headers (any scheme).
        rule(
            r#"(?i)\b((?:proxy-)?authorization\s*[:=]\s*["']?\s*)(?:(bearer|basic|token|bot|digest|apikey)\s+)?[A-Za-z0-9._~+/=:-]{6,}"#,
            |c| match c.get(2) {
                Some(scheme) => format!("{}{} {REDACTED}", &c[1], scheme.as_str()),
                None => format!("{}{REDACTED}", &c[1]),
            },
        ),
        rule(r"(?i)\b(bearer\s+)([A-Za-z0-9._~+/=-]{8,})", |c| {
            if key_shaped(&c[2]) {
                format!("{}{REDACTED}", &c[1])
            } else {
                c[0].to_string()
            }
        }),
        // API key and cookie headers.
        rule(
            r#"(?i)\b((?:x-)?(?:api-key|apikey|auth-token|access-token|goog-api-key|github-token|gitlab-token|private-token|subscription-key|functions-key)\s*:\s*["']?)[^\s"',;]{6,}"#,
            keep_first,
        ),
        rule(r#"(?i)\b((?:set-)?cookie\s*:\s*)[^\r\n"]+"#, keep_first),
        // Tokens with a provider's prefix.
        rule(r"\bsk-ant-[A-Za-z0-9_-]{16,}", whole),
        rule(r"\bsk-[A-Za-z0-9_-]{20,}", whole),
        rule(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}", whole),
        rule(r"\bgithub_pat_[A-Za-z0-9_]{20,}", whole),
        rule(r"\bglpat-[A-Za-z0-9_-]{16,}", whole),
        rule(r"\bxox[abposr]-[A-Za-z0-9-]{10,}", whole),
        rule(r"\bxapp-[A-Za-z0-9-]{10,}", whole),
        rule(r"\bAIza[0-9A-Za-z_-]{30,}", whole),
        rule(r"\bya29\.[0-9A-Za-z_-]{20,}", whole),
        rule(r"\b(?:hf|gsk|npm)_[A-Za-z0-9]{20,}", whole),
        rule(r"\b(?:xai|pplx)-[A-Za-z0-9]{20,}", whole),
        rule(r"\bpypi-[A-Za-z0-9_-]{20,}", whole),
        rule(r"\b(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{16,}", whole),
        rule(r"\bwhsec_[A-Za-z0-9+/=]{20,}", whole),
        rule(r"\bSG\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}", whole),
        rule(
            r"\b(?:AKIA|ASIA|AGPA|AIDA|AROA|ANPA|ANVA|AIPA)[0-9A-Z]{16}\b",
            whole,
        ),
        rule(r"\b(?:dop|doo|dor)_v1_[a-f0-9]{40,}", whole),
        rule(r"\bshp(?:at|ss|ca|pa)_[a-fA-F0-9]{32}", whole),
        rule(r"\blin_(?:api|oauth)_[A-Za-z0-9]{20,}", whole),
        rule(r"\b\d{8,10}:AA[A-Za-z0-9_-]{30,}", whole),
        rule(
            r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
            whole,
        ),
        // AWS secret keys and session tokens by name.
        rule(
            r#"(?i)\b((?:aws_)?(?:secret_access_key|session_token|secret_key)["']?\s*[:=]\s*["']?)[A-Za-z0-9/+=]{16,}"#,
            keep_first,
        ),
        // Secrets on a command line: --password x, --token=x, -p'x' is left (too common).
        rule(
            r#"(?i)(--?(?:password|passwd|pass|token|api-key|apikey|secret|auth-token|access-token|client-secret)(?:=|\s+))("[^"]*"|'[^']*'|[^\s"']+)"#,
            |c| format!("{}{REDACTED}", &c[1]),
        ),
        // `.env`-style lines and shell exports: NAME=value, where the name is secret-bearing.
        rule(
            &format!(
                r#"(?im)^(\s*(?:export\s+|set\s+|setx\s+|\$env:)?)([A-Za-z_][A-Za-z0-9_.-]*{SECRET_NAME}[A-Za-z0-9_.-]*|{SECRET_NAME}[A-Za-z0-9_.-]*)(\s*[:=]\s*)(.+)$"#
            ),
            keep_assignment,
        ),
        // Quoted assignments anywhere: "api_key": "value", password = 'value'.
        rule(
            &format!(
                r#"(?i)((?:["']?)[A-Za-z0-9_.-]*{SECRET_NAME}[A-Za-z0-9_.-]*["']?)(\s*[:=]\s*)(["'])([^"'\\\r\n]{{3,}})["']"#
            ),
            |c| {
                let value = &c[4];
                let secret = if password_like_name(&c[1]) {
                    !looks_like_code(value) || env_style_name(&c[1])
                } else {
                    key_shaped(value)
                };
                if is_placeholder(value) || !secret {
                    c[0].to_string()
                } else {
                    format!("{}{}{}{REDACTED}{}", &c[1], &c[2], &c[3], &c[3])
                }
            },
        ),
        // Unquoted assignments of passwords and secrets anywhere (`Password=x;`).
        rule(
            r#"(?i)\b([A-Za-z0-9_.-]*(?:passw(?:or)?d|pwd|passphrase|secret)[A-Za-z0-9_.-]*)(\s*[:=]\s*)([^\s"',;)}\]]{3,})"#,
            |c| {
                let code = !env_style_name(&c[1]) && looks_like_code(&c[3]);
                if is_placeholder(&c[3]) || c[3].starts_with('(') || code {
                    c[0].to_string()
                } else {
                    format!("{}{}{REDACTED}", &c[1], &c[2])
                }
            },
        ),
        // Unquoted assignments of keys and tokens whose value looks like one (a digit in it).
        rule(
            r#"(?i)\b([A-Za-z0-9_.-]*(?:api[_-]?key|apikey|token|access[_-]?key|private[_-]?key|client[_-]?secret|auth[_-]?key)[A-Za-z0-9_.-]*)(\s*[:=]\s*)([A-Za-z0-9_+/=~-]{8,})"#,
            |c| {
                let value = &c[3];
                if is_placeholder(value) || !value.chars().any(|ch| ch.is_ascii_digit()) {
                    c[0].to_string()
                } else {
                    format!("{}{}{REDACTED}", &c[1], &c[2])
                }
            },
        ),
    ]
});

fn apply<'a>(text: Cow<'a, str>, rule: &Rule, count: &mut usize) -> Cow<'a, str> {
    if !rule.regex.is_match(&text) {
        return text;
    }
    let replaced = rule.regex.replace_all(&text, |caps: &Captures| {
        let out = (rule.replace)(caps);
        if out != caps[0] {
            *count += 1;
        }
        out
    });
    Cow::Owned(replaced.into_owned())
}

/// A redacted token that a terminal or an editor wrapped onto the next line(s): each
/// following line made only of token characters (after a line break, raw or written as
/// `\n`) is part of the same token and is redacted too.
fn redact_wrapped_tokens(text: &str, count: &mut usize) -> Option<String> {
    if !text.contains(REDACTED) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest: &str = text;
    let mut changed = false;
    while let Some(at) = rest.find(REDACTED) {
        let end = at + REDACTED.len();
        out.push_str(&rest[..end]);
        rest = &rest[end..];
        loop {
            let after_break = if let Some(r) = rest.strip_prefix("\r\n") {
                r
            } else if let Some(r) = rest.strip_prefix('\n') {
                r
            } else if let Some(r) = rest.strip_prefix("\\n") {
                r
            } else {
                break;
            };
            let run = after_break
                .char_indices()
                .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-'))
                .map_or(after_break.len(), |(i, _)| i);
            let line_ends = after_break[run..].is_empty()
                || after_break[run..].starts_with(['\n', '\r', '"', '\'', ' '])
                || after_break[run..].starts_with("\\n");
            // A continuation is long and random: a word or a number on the next line is not.
            let piece = &after_break[..run];
            if run >= 8 && line_ends && looks_random(piece) {
                let consumed = rest.len() - after_break.len() + run;
                out.push_str(&rest[..rest.len() - after_break.len()]);
                out.push_str(REDACTED);
                rest = &rest[consumed..];
                *count += 1;
                changed = true;
            } else {
                break;
            }
        }
    }
    if !changed {
        return None;
    }
    out.push_str(rest);
    Some(out)
}

/// Mixed case and digits, or long mixed alphanumerics, and few long runs of lowercase
/// letters (words): the shape of a key, not of a word or an identifier.
fn looks_random(piece: &str) -> bool {
    let digits = piece.chars().filter(char::is_ascii_digit).count();
    let upper = piece.chars().filter(char::is_ascii_uppercase).count();
    let lower = piece.chars().filter(char::is_ascii_lowercase).count();
    let classes = usize::from(digits > 0) + usize::from(upper > 0) + usize::from(lower > 0);
    classes >= 2 && longest_lower_run(piece) <= 6 && (digits > 0 || piece.len() >= 16)
}

fn longest_lower_run(piece: &str) -> usize {
    let mut best = 0;
    let mut run = 0;
    for c in piece.chars() {
        if c.is_ascii_lowercase() {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// Shannon entropy of `piece` in bits per character.
fn entropy(piece: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    let mut total = 0usize;
    for c in piece.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
        total += 1;
    }
    if total == 0 {
        return 0.0;
    }
    counts
        .values()
        .map(|&n| {
            let p = n as f64 / total as f64;
            -p * p.log2()
        })
        .sum()
}

static RANDOM_CANDIDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/=_-]{24,}").unwrap_or_else(|e| panic!("{e}")));

/// Whether `token` is a key without a known prefix: long, of upper and lower case letters
/// and digits, high entropy, and no word-like part. Hex hashes (no upper case), UUIDs,
/// file paths and identifiers are left.
fn is_random_secret(token: &str) -> bool {
    let core = token.trim_matches(|c| c == '=' || c == '-' || c == '_');
    if core.len() < 24 {
        return false;
    }
    // A path: a part of three or more lower-case letters between slashes.
    if core.contains('/')
        && core.split('/').any(|part| {
            part.len() >= 3
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c == '-')
        })
    {
        return false;
    }
    let digits = core.chars().filter(char::is_ascii_digit).count();
    let upper = core.chars().filter(char::is_ascii_uppercase).count();
    let lower = core.chars().filter(char::is_ascii_lowercase).count();
    if digits < 2 || upper < 2 || lower < 2 {
        return false;
    }
    if longest_lower_run(core) > 7 {
        return false;
    }
    let threshold = if core.len() >= 32 { 4.0 } else { 3.8 };
    entropy(core) >= threshold
}

fn redact_random_strings(text: &str, count: &mut usize) -> Option<String> {
    let mut changed = false;
    let replaced = RANDOM_CANDIDATE.replace_all(text, |caps: &Captures| {
        let token = &caps[0];
        if is_random_secret(token) {
            *count += 1;
            changed = true;
            REDACTED.to_string()
        } else {
            token.to_string()
        }
    });
    changed.then(|| replaced.into_owned())
}

/// Undoes JSON string escapes (`\n`, `\t`, `\"`, `\\`, `\/`, `\uXXXX`, surrogate pairs);
/// anything malformed is kept as it is.
pub fn unescape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('b') | Some('f') => out.push(' '),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&hex, 16) {
                    Ok(high) if (0xD800..0xDC00).contains(&high) => {
                        // A surrogate pair: \uD83D\uDE00.
                        let mut low_text = String::new();
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                            if chars.peek() == Some(&'u') {
                                chars.next();
                                low_text = chars.by_ref().take(4).collect();
                            }
                        }
                        match u32::from_str_radix(&low_text, 16) {
                            Ok(low) if (0xDC00..0xE000).contains(&low) => {
                                let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                                out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                            }
                            _ => out.push('\u{FFFD}'),
                        }
                    }
                    Ok(code) => out.push(char::from_u32(code).unwrap_or('\u{FFFD}')),
                    Err(_) => {
                        out.push_str("\\u");
                        out.push_str(&hex);
                    }
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
