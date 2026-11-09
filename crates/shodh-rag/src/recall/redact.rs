//! Secret redaction for Recall, applied at index time and again whenever a turn is read.
//!
//! Every string Recall takes from a transcript passes through one [`Redactor`] before
//! it reaches anything else: the chunks that are embedded and keyword-indexed, the
//! entities, the catalog, the receipt viewer, the agent's tools and every error record.
//! Transcripts hold what the user pasted and what their tools printed, which includes
//! their own API keys, so what is redacted here is never indexed, shown or sent to a
//! model.
//!
//! What is redacted:
//! - private key blocks (PEM, OpenSSH, PGP, PuTTY), also cut short, JSON-escaped or
//!   base64-encoded (Kubernetes secrets, kubeconfig `*-key-data`);
//! - tokens with a provider's prefix (Anthropic, OpenAI, OpenRouter, GitHub, GitLab,
//!   Slack, Discord, Google, Hugging Face, Groq, xAI, Perplexity, npm, PyPI, Stripe,
//!   SendGrid, AWS ids, Telegram bots also inside their API URLs, JWTs, webhooks), also
//!   when a terminal wrapped one over several lines, even right after its prefix;
//! - credentials in headers (`Authorization`, `Bearer`, `x-api-key`, cookies), in URLs
//!   (`user:password@host`, `:password@host` with no user, secret query parameters), on
//!   command lines (`--password x`, `-u user:pass`, `docker login -p`, `sshpass -p`,
//!   `mysql -pX`, `gh secret set --body`, `aws configure set`, npm `_authToken`);
//! - the value of every assignment whose name says it holds a secret (`*_KEY`,
//!   `*_TOKEN`, `*_SECRET`, `*_PASS`, `*_PW`, `password`, `api_key`, `tls.key`…), quoted
//!   or not, whatever the value looks like;
//! - email addresses (unless the user keeps them);
//! - long random-looking strings (keys without a known prefix);
//! - the user's own words and patterns.
//!
//! A text that holds JSON escapes (`\n`, `\"`) is unescaped first, so a key after an
//! escaped line break is seen at the start of its line. Over-masking a hash costs a
//! little recall; leaking a key costs the user the key, so the rules err towards masking.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use sha2::{Digest, Sha256};

/// What a secret is replaced with.
pub const REDACTED: &str = "[REDACTED]";
/// What an email address is replaced with.
pub const EMAIL: &str = "[EMAIL]";

/// Version of the built-in rules. Raised whenever a rule changes, so every session is
/// indexed again under the new rules ([`Redactor::fingerprint`]), and what was indexed
/// under the old ones is not served meanwhile.
pub const RULES_VERSION: u32 = 2;

/// Longest user pattern accepted (characters).
pub const MAX_PATTERN_CHARS: usize = 300;
/// Most user patterns accepted.
pub const MAX_PATTERNS: usize = 50;

/// A user pattern Shodh cannot use. It names the line, never the pattern itself (the
/// pattern is what the user wants hidden).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    /// 1-based line of the patterns.
    pub line: usize,
    pub reason: String,
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Line {} of your hidden words: {}",
            self.line, self.reason
        )
    }
}

impl std::error::Error for PatternError {}

/// What the user chose about redaction beyond the built-in rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactOptions {
    /// Email addresses are kept (by default they are hidden).
    pub keep_emails: bool,
}

/// Redacts secrets from transcript text with the built-in rules and the user's patterns.
#[derive(Debug, Clone)]
pub struct Redactor {
    user: Vec<Regex>,
    options: RedactOptions,
    fingerprint: String,
}

impl Default for Redactor {
    fn default() -> Self {
        Self {
            user: Vec::new(),
            options: RedactOptions::default(),
            fingerprint: fingerprint(&[], &RedactOptions::default()),
        }
    }
}

fn fingerprint(patterns: &[String], options: &RedactOptions) -> String {
    let mut hash = Sha256::new();
    hash.update(RULES_VERSION.to_le_bytes());
    hash.update([u8::from(options.keep_emails)]);
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

/// Checks the user's hidden words, one a line: a line is a literal word or phrase
/// (any case), or a regular expression written between slashes (`/ACME-[0-9]{6}/`, also
/// any case). At most [`MAX_PATTERNS`] lines of at most [`MAX_PATTERN_CHARS`]; a pattern
/// may not match empty text.
pub fn check_patterns(patterns: &[String]) -> Result<Vec<Regex>, PatternError> {
    if patterns.len() > MAX_PATTERNS {
        return Err(PatternError {
            line: MAX_PATTERNS + 1,
            reason: format!("at most {MAX_PATTERNS} lines are kept"),
        });
    }
    patterns
        .iter()
        .enumerate()
        .map(|(i, pattern)| {
            let error = |reason: String| PatternError {
                line: i + 1,
                reason,
            };
            let trimmed = pattern.trim();
            if trimmed.is_empty() {
                return Err(error("it is empty".to_string()));
            }
            if trimmed.chars().count() > MAX_PATTERN_CHARS {
                return Err(error(format!(
                    "it is longer than {MAX_PATTERN_CHARS} characters"
                )));
            }
            let source = match trimmed
                .strip_prefix('/')
                .and_then(|r| r.strip_suffix('/'))
                .filter(|r| !r.is_empty())
            {
                Some(regex) => regex.to_string(),
                None => regex::escape(trimmed),
            };
            let regex = regex::RegexBuilder::new(&source)
                .case_insensitive(true)
                .size_limit(1 << 20)
                .build()
                .map_err(|_| error("it is not a pattern Shodh can read".to_string()))?;
            if regex.is_match("") {
                return Err(error("it matches empty text".to_string()));
            }
            Ok(regex)
        })
        .collect()
}

impl Redactor {
    /// The built-in rules and `patterns` (the user's own hidden words).
    pub fn new(patterns: &[String]) -> Result<Self, PatternError> {
        Self::with_options(patterns, RedactOptions::default())
    }

    /// [`Self::new`] with the user's other choices.
    pub fn with_options(patterns: &[String], options: RedactOptions) -> Result<Self, PatternError> {
        Ok(Self {
            user: check_patterns(patterns)?,
            fingerprint: fingerprint(patterns, &options),
            options,
        })
    }

    /// Identifies the rules, patterns and options: text indexed under another fingerprint
    /// is indexed again, and not served before it is.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// `text` with every secret replaced by [`REDACTED`].
    pub fn redact(&self, text: &str) -> String {
        self.redact_counted(text).0
    }

    /// [`Self::redact`], and how many secrets were replaced.
    pub fn redact_counted(&self, text: &str) -> (String, usize) {
        let (out, count) = self.redact_core(text);
        // Text with JSON escapes is also read with them undone: `\nAKIA…` is a key at the
        // start of a line. That reading is kept only when it finds more secrets, so a
        // Windows path (`C:\Users\runner`) is not turned into a line break needlessly.
        if has_json_escapes(text) {
            let (unescaped, more) = self.redact_core(&unescape_json(text));
            if more > count {
                return (unescaped, more);
            }
        }
        (out, count)
    }

    fn redact_core(&self, text: &str) -> (String, usize) {
        let mut count = 0usize;
        let mut out = Cow::Borrowed(text);
        if let Some(split) = redact_split_prefixes(&out, &mut count) {
            out = Cow::Owned(split);
        }
        for rule in RULES.iter() {
            out = apply(out, rule, &mut count);
        }
        if !self.options.keep_emails {
            out = apply(out, &EMAIL_RULE, &mut count);
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

/// Whether `text` holds JSON string escapes that hide line starts and word boundaries.
fn has_json_escapes(text: &str) -> bool {
    text.contains("\\n") || text.contains("\\t") || text.contains("\\r") || text.contains("\\\"")
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

/// A name written like an environment variable (`DB_PASSWORD`): no lower-case letter.
/// Its value is always a value, never code.
fn env_style_name(name: &str) -> bool {
    !name.chars().any(|c| c.is_ascii_lowercase())
}

/// A value that reads as code rather than a secret (`token = tokens.next()`,
/// `password = form.password`, `key = map[k]`): it starts with a lower-case identifier,
/// holds no digit, and is a field, a call, a path or an index. A plain word is not code:
/// `password: changeme` is a password. Only unquoted values are ever read as code: a
/// quoted one is a literal.
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
                    '_' | '.' | '(' | ')' | '[' | ']' | ':' | '&' | '*' | '!' | '?'
                )
        })
        && (v.contains(['(', '[']) || v.contains("::") || v.matches('.').count() == 1)
}

/// Type names that follow a name in code (`token: string`), never secrets.
const TYPE_WORDS: [&str; 20] = [
    "string", "str", "number", "boolean", "bool", "int", "integer", "any", "unknown", "object",
    "bytes", "text", "varchar", "uuid", "option", "string?", "vec", "char", "float", "double",
];

/// A value that only names where a secret comes from (`${TOKEN}`, `<your key>`,
/// `process.env.KEY`, `$TOKEN`), a type, or nothing. Only a value that is wholly such a
/// reference counts: `{noop}hunter2` and `$ecret1` are passwords.
fn is_placeholder(value: &str) -> bool {
    static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(?:\$\{[^}]*\}|\{\{[^}]*\}\}|<[^<>]*>|\$[A-Za-z_][A-Za-z0-9_]*|%[A-Za-z_][A-Za-z0-9_]*%|process\.env\.[A-Za-z0-9_]+|os\.environ(?:\.get)?[\[(].*|env\(.*\)|std::env::var\(.*\)|\[REDACTED\]|\[EMAIL\])$",
        )
        .unwrap_or_else(|e| panic!("{e}"))
    });
    let v = value.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    v.is_empty()
        || REFERENCE.is_match(v)
        // The first word of `<your key here>` once a value stops at a space.
        || (v.starts_with('<') && !v.chars().any(|c| c.is_ascii_digit()))
        || ["null", "none", "nil", "true", "false", "undefined", "required", "optional"]
            .iter()
            .any(|w| v.eq_ignore_ascii_case(w))
        || TYPE_WORDS.iter().any(|w| v.eq_ignore_ascii_case(w))
        || v.chars().all(|c| c == '*' || c == 'x' || c == 'X' || c == '.' || c == '•')
}

/// How strongly a name says its value is secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Secrecy {
    /// Nothing secret about it.
    None,
    /// `*_KEY`, `*_TOKEN`, `password`, `client_secret`: any value is a secret.
    Strong,
    /// `token_count`, `keymap`: only a value shaped like a key is.
    Weak,
}

/// The words of a name, lower case: `DB_PASS` → `db`, `pass`; `authToken` → `auth`,
/// `token`; `tls.key` → `tls`, `key`.
fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() && previous_lower && !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
            word.push(c.to_ascii_lowercase());
        } else {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            previous_lower = false;
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Words that make a name secret wherever they are in it.
const SECRET_ANYWHERE: [&str; 13] = [
    "password",
    "passwd",
    "passphrase",
    "pwd",
    "secret",
    "secrets",
    "credential",
    "credentials",
    "apikey",
    "privatekey",
    "authorization",
    "dsn",
    "cookie",
];
/// Words that make a name secret when they end it (`SMTP_PASS`, `ADMIN_PW`, `tls.key`).
const SECRET_LAST: [&str; 9] = [
    "pass",
    "pw",
    "key",
    "token",
    "secret",
    "auth",
    "sig",
    "signature",
    "salt",
];
/// Pairs of words that make a name secret together (`api_key`, `client_secret`).
const SECRET_PAIRS: [(&str, &str); 12] = [
    ("api", "key"),
    ("access", "key"),
    ("private", "key"),
    ("secret", "key"),
    ("client", "secret"),
    ("auth", "token"),
    ("access", "token"),
    ("refresh", "token"),
    ("session", "key"),
    ("signing", "key"),
    ("key", "data"),
    ("webhook", "url"),
];

fn secrecy(name: &str) -> Secrecy {
    let words = name_words(name.trim_matches(|c| c == '"' || c == '\''));
    if words.is_empty() {
        return Secrecy::None;
    }
    let last = words.last().map(String::as_str).unwrap_or("");
    let strong = words.iter().any(|w| SECRET_ANYWHERE.contains(&w.as_str()))
        || SECRET_LAST.contains(&last)
        || words
            .windows(2)
            .any(|p| SECRET_PAIRS.contains(&(p[0].as_str(), p[1].as_str())));
    if strong {
        return Secrecy::Strong;
    }
    if words
        .iter()
        .any(|w| matches!(w.as_str(), "token" | "key" | "auth" | "pass" | "pw"))
    {
        return Secrecy::Weak;
    }
    Secrecy::None
}

/// A value shaped like a key or token: a digit in it, or long.
fn key_shaped(value: &str) -> bool {
    let v = value.trim();
    v.len() >= 8 && !v.contains(' ') && (v.chars().any(|c| c.is_ascii_digit()) || v.len() >= 20)
}

/// Whether the value of an assignment to `name` is a secret. `quoted`: the value was
/// written in quotes (a literal, never code).
fn secret_value(name: &str, value: &str, quoted: bool) -> bool {
    if is_placeholder(value) {
        return false;
    }
    match secrecy(name) {
        Secrecy::None => false,
        Secrecy::Strong => quoted || env_style_name(name) || !looks_like_code(value),
        Secrecy::Weak => key_shaped(value) && !looks_like_code(value),
    }
}

/// `NAME=value` at the start of a line (`.env`, shell exports, YAML `tls.key: …`, a
/// diff's `+NAME=value`): the rest of the line is the value.
fn line_assignment(c: &Captures) -> String {
    let (prefix, name, sep, value) = (&c[1], &c[2], &c[3], &c[4]);
    let trimmed = value.trim();
    let quoted = trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')));
    if !secret_value(name, trimmed, quoted) {
        return c[0].to_string();
    }
    format!("{prefix}{name}{sep}{REDACTED}")
}

/// `name = value` or `"name": "value"` anywhere in a line.
fn inline_assignment(c: &Captures) -> String {
    let name = &c[2];
    let value = &c[5];
    let quoted = value.starts_with('"') || value.starts_with('\'');
    let inner = if quoted && value.len() >= 2 {
        &value[1..value.len() - 1]
    } else {
        value
    };
    if !secret_value(name, inner, quoted) {
        return c[0].to_string();
    }
    let replaced = if quoted {
        let q = &value[..1];
        format!("{q}{REDACTED}{q}")
    } else {
        REDACTED.to_string()
    };
    format!("{}{}{}{}{replaced}", &c[1], name, &c[3], &c[4])
}

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
        // A PEM block base64-encoded ("-----BEGIN" → LS0tLS1CRUdJTi): Kubernetes secrets,
        // kubeconfig client keys.
        rule(r"LS0tLS1CRUdJT[A-Za-z0-9+/=\r\n]*", whole),
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
        // Telegram bot tokens, also inside an API URL (`/bot<id>:<secret>/getMe`).
        rule(r"(bot|\b)\d{8,12}:[A-Za-z0-9_-]{35}", |c| {
            format!("{}{REDACTED}", &c[1])
        }),
        // Credentials in a URL: scheme://user:password@host, also with no user
        // (redis://:password@host).
        rule(
            r#"(?i)\b([a-z][a-z0-9+.-]*://[^\s:/@"'<>]*:)[^\s@/"'<>]+@"#,
            |c| format!("{}{REDACTED}@", &c[1]),
        ),
        // Secret query parameters (any name that says secret: X-Amz-Signature, sig, key).
        rule(
            r#"(?i)([?&;](?:[a-z0-9_.-]*(?:token|secret|password|passwd|pwd|signature|credential|api[_-]?key|apikey|auth)[a-z0-9_.-]*|key|sig|code_verifier|code)=)[^&\s"'#<>]+"#,
            keep_first,
        ),
        // Authorization headers (any scheme).
        rule(
            r#"(?i)\b((?:proxy-)?authorization\s*[:=]\s*["']?\s*)(?:(bearer|basic|token|bot|digest|apikey)\s+)?[A-Za-z0-9._~+/=:%-]{6,}"#,
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
            r#"(?i)\b((?:x-)?(?:api-key|apikey|auth-token|access-token|goog-api-key|github-token|gitlab-token|private-token|subscription-key|functions-key|amz-security-token)\s*:\s*["']?)[^\s"',;]{6,}"#,
            keep_first,
        ),
        rule(r#"(?i)\b((?:set-)?cookie\s*:\s*)[^\r\n"]+"#, keep_first),
        // Tokens with a provider's prefix.
        rule(r"\bsk-ant-[A-Za-z0-9_-]{16,}", whole),
        rule(r"\bsk-[A-Za-z0-9_-]{20,}", whole),
        rule(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}", whole),
        rule(r"\bgithub_pat_[A-Za-z0-9_]{20,}", whole),
        rule(r"\bglpat-[A-Za-z0-9_-]{16,}", whole),
        rule(r"\bxox[abposre]-[A-Za-z0-9-]{10,}", whole),
        rule(r"\bxapp-[A-Za-z0-9-]{10,}", whole),
        // Discord bot tokens: three dot-separated base64url parts.
        rule(
            r"\b[MNO][A-Za-z0-9_-]{23,27}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,40}\b",
            whole,
        ),
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
        rule(r"\bkey-[a-z0-9]{32}\b", whole),
        rule(
            r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
            whole,
        ),
        // AWS secret keys and session tokens by name, after `=`, `:` or a space
        // (`aws configure set aws_secret_access_key …`), URL-encoded or not.
        rule(
            r#"(?i)\b((?:aws_)?(?:secret_access_key|session_token|secret_key)["']?\s*[:=\s]\s*["']?)[A-Za-z0-9/+=%]{16,}"#,
            keep_first,
        ),
        // `aws configure set <secret name> <value>`.
        rule(
            r#"(?i)\b(aws\s+configure\s+set\s+\S*(?:secret|token|key)\S*\s+)("[^"]*"|'[^']*'|\S+)"#,
            keep_first,
        ),
        // Secrets on a command line: --password x, --token=x, --body for gh secrets.
        rule(
            r#"(?i)(--?(?:password|passwd|pass|pw|token|api-key|apikey|secret|auth-token|access-token|client-secret|secret-access-key)(?:=|\s+))("[^"]*"|'[^']*'|[^\s"']+)"#,
            keep_first,
        ),
        rule(
            r#"(?i)(\bgh\s+secret\s+set\b[^\n]*?\s(?:--body|-b)(?:=|\s+))("[^"]*"|'[^']*'|\S+)"#,
            keep_first,
        ),
        // curl -u user:password, --user user:password (the user is kept).
        rule(
            r#"(\s(?:-u|--user|--proxy-user)(?:=|\s*)["']?[^\s:"']*:)([^\s"']+)"#,
            keep_first,
        ),
        // docker login -p x, sshpass -p x, mysql -px (and mysqldump, mysqladmin).
        rule(
            r#"(?i)(\bdocker\s+login\b[^\n]*?\s-p(?:=|\s+))("[^"]*"|'[^']*'|\S+)"#,
            keep_first,
        ),
        rule(r#"(\bsshpass\s+-p\s*)("[^"]*"|'[^']*'|\S+)"#, keep_first),
        rule(
            r#"(\bmysql(?:dump|admin|sh)?\b[^\n]*?\s-p)("[^"]*"|'[^']*'|[^\s"']+)"#,
            keep_first,
        ),
        // npm's auth (`//registry.npmjs.org/:_authToken abc`, `_auth=…`).
        rule(
            r#"(?i)(_auth(?:token)?["']?\s*[:=\s]\s*["']?)([^\s"']+)"#,
            keep_first,
        ),
        // Assignments at the start of a line: .env, shell exports, YAML, diff lines.
        rule(
            r#"(?m)^(\s*[+-]?\s*(?:export\s+|set\s+|setx\s+|\$env:|-\s+)?)([A-Za-z_][A-Za-z0-9_.-]*)(\s*[:=]\s*)(\S.*?)\s*$"#,
            line_assignment,
        ),
        // Assignments anywhere: "api_key": "value", password='value', DB_PASS=x.
        rule(
            r#"(^|[^A-Za-z0-9_.-])(["']?[A-Za-z_][A-Za-z0-9_.-]*)(["']?)(\s*[:=]\s*)("[^"\r\n]*"|'[^'\r\n]*'|[^\s"',;)\]]+)"#,
            inline_assignment,
        ),
    ]
});

static EMAIL_RULE: LazyLock<Rule> = LazyLock::new(|| Rule {
    regex: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}\b")
        .unwrap_or_else(|e| panic!("{e}")),
    replace: |_| EMAIL.to_string(),
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

/// Provider prefixes that begin a key.
const KEY_PREFIXES: [&str; 14] = [
    "sk-ant-",
    "sk-proj-",
    "sk-or-",
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "AIza",
    "hf_",
    "xoxb-",
];

/// A key a terminal wrapped right after its prefix (`sk-ant-api03-ab` then the rest on
/// the next line): too short on its line for its own rule, it is redacted with the
/// random-looking line that continues it.
fn redact_split_prefixes(text: &str, count: &mut usize) -> Option<String> {
    static DANGLING: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)(?:\b|_)(?:sk-ant-|sk-proj-|sk-or-|sk-|ghp_|gho_|ghu_|ghs_|ghr_|github_pat_|glpat-|AIza|hf_|xox[abposre]-)[A-Za-z0-9_-]*(\r?\n|\\n)([A-Za-z0-9_-]{8,})")
            .unwrap_or_else(|e| panic!("{e}"))
    });
    if !KEY_PREFIXES.iter().any(|p| text.contains(p)) && !text.contains("xox") {
        return None;
    }
    let mut changed = false;
    let replaced = DANGLING.replace_all(text, |c: &Captures| {
        if looks_random(&c[2]) {
            changed = true;
            *count += 1;
            format!("{REDACTED}{}{REDACTED}", &c[1])
        } else {
            c[0].to_string()
        }
    });
    changed.then(|| replaced.into_owned())
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

/// Mixed classes and not word-like: the shape of a key's continuation, not of a word or
/// an identifier.
fn looks_random(piece: &str) -> bool {
    let digits = piece.chars().filter(char::is_ascii_digit).count();
    let upper = piece.chars().filter(char::is_ascii_uppercase).count();
    let lower = piece.chars().filter(char::is_ascii_lowercase).count();
    let classes = usize::from(digits > 0) + usize::from(upper > 0) + usize::from(lower > 0);
    classes >= 2 && word_share(piece) < 0.6 && (digits > 0 || piece.len() >= 16)
}

/// Share of `piece` in runs of four or more lower-case letters (words, identifier
/// parts). Random base64 has few such runs; text and identifiers are mostly made of them.
fn word_share(piece: &str) -> f64 {
    let total = piece.chars().count();
    if total == 0 {
        return 0.0;
    }
    let mut in_words = 0usize;
    let mut run = 0usize;
    for c in piece.chars() {
        if c.is_ascii_lowercase() {
            run += 1;
        } else {
            if run >= 4 {
                in_words += run;
            }
            run = 0;
        }
    }
    if run >= 4 {
        in_words += run;
    }
    in_words as f64 / total as f64
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
/// (and digits when short), high entropy, and not word-like. Hex hashes and UUIDs (no
/// upper case), file paths and identifiers are left. The longer a candidate, the less a
/// word-like run inside it can be chance, so long ones need only their entropy.
fn is_random_secret(token: &str) -> bool {
    let core = token.trim_matches(|c| c == '=' || c == '-' || c == '_');
    let length = core.chars().count();
    if length < 24 {
        return false;
    }
    // A path: two or more lower-case parts between slashes (`src`, `lumen-cache`,
    // `lru.rs`) holding most of it. A short lower-case run that chance puts next to a
    // slash in a base64 key does not exempt the key.
    if core.contains('/') {
        let wordy: Vec<&str> = core
            .split('/')
            .filter(|p| {
                p.len() >= 2
                    && p.chars().any(|c| c.is_ascii_lowercase())
                    && p.chars().all(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.')
                    })
            })
            .collect();
        let wordy_chars: usize = wordy.iter().map(|p| p.len()).sum();
        if wordy.len() >= 2 && wordy_chars * 2 >= length {
            return false;
        }
    }
    let digits = core.chars().filter(char::is_ascii_digit).count();
    let upper = core.chars().filter(char::is_ascii_uppercase).count();
    let lower = core.chars().filter(char::is_ascii_lowercase).count();
    if upper == 0 || lower == 0 {
        return false;
    }
    if length > 64 {
        return entropy(core) >= 4.5 && word_share(core) < 0.5;
    }
    if digits == 0 && length < 32 {
        return false;
    }
    if word_share(core) >= 0.5 {
        return false;
    }
    let threshold = if length >= 32 { 4.0 } else { 3.8 };
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
