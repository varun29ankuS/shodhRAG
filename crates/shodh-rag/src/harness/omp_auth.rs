//! Signing in to subscriptions (Claude Pro/Max, ChatGPT Plus/Pro, GitHub
//! Copilot, Google Gemini) through the agent runtime's own login, in Shodh's
//! isolated accounts directory ([`OmpLayout::accounts_dir`]).
//!
//! omp 18.4.10 commands used (verified against the binary's `--help` and
//! source; `login` itself is never run by the tests):
//! - `omp auth-broker login <provider>`: the provider's OAuth flow. It prints
//!   the page to open (and, for Copilot, the device code), waits for the
//!   browser to finish (a callback on localhost, or device polling) and saves
//!   the account in the local credential store. It never opens a browser
//!   itself, so the app opens the (checked) page. Copilot first asks for a
//!   GitHub Enterprise domain on stdin; an empty line means github.com.
//! - `omp token <provider> --list`: the signed-in accounts, one
//!   `N. identity` line each; exit 1 when there are none.
//! - `omp token <provider>`: exit 0 when a usable token exists (refreshing
//!   an expired one). It prints the token, so its stdout is discarded.
//! - `omp auth-broker logout <provider>`: removes the stored account.
//!
//! Every command runs with [`child_env`]'s cleared, isolated environment.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;

use super::error::HarnessError;
use super::model::OmpModel;
use super::sidecar::{child_env, OmpLayout};
use crate::llm::SubscriptionService;

/// omp's callback wait is 300 s; the app gives up a little later.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(330);

/// Status commands answer from the local store (a refresh may contact the provider).
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(30);

/// The provider id for a subscription, from a fixed list (never user text).
pub fn omp_id(service: SubscriptionService) -> &'static str {
    service.omp_provider()
}

pub fn login_args(service: SubscriptionService) -> Vec<String> {
    vec!["auth-broker".into(), "login".into(), omp_id(service).into()]
}

pub fn logout_args(service: SubscriptionService) -> Vec<String> {
    vec![
        "auth-broker".into(),
        "logout".into(),
        omp_id(service).into(),
    ]
}

pub fn accounts_args(service: SubscriptionService) -> Vec<String> {
    vec!["token".into(), omp_id(service).into(), "--list".into()]
}

pub fn token_check_args(service: SubscriptionService) -> Vec<String> {
    vec!["token".into(), omp_id(service).into()]
}

/// Copilot's first question (no newline follows it while it waits).
const ENTERPRISE_PROMPT: &str = "GitHub Enterprise URL/domain";

/// The answer to write to the login's stdin when `pending` (output not yet
/// ended by a newline) is a question the app answers: Copilot asks for a
/// GitHub Enterprise domain first, and an empty line means github.com. The
/// answer is written only once the question is asked: a line sent earlier
/// would be read before the question and lost.
pub fn prompt_answer(service: SubscriptionService, pending: &str) -> Option<&'static str> {
    (service == SubscriptionService::Copilot && pending.contains(ENTERPRISE_PROMPT)).then_some("\n")
}

/// Splits a child's output into lines, keeping the unfinished last line
/// (a prompt waiting for input) visible.
#[derive(Debug, Default)]
pub struct LineSplitter {
    pending: String,
}

impl LineSplitter {
    /// Add output; returns the lines it completed.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.pending.push_str(chunk);
        let mut lines = Vec::new();
        while let Some(at) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=at).collect();
            lines.push(line.trim_end_matches(['\r', '\n']).to_string());
        }
        // A runaway line without a newline is cut, not buffered forever.
        if self.pending.len() > 64 * 1024 {
            lines.push(std::mem::take(&mut self.pending));
        }
        lines
    }

    /// Output after the last newline.
    pub fn pending(&self) -> &str {
        &self.pending
    }

    /// The unfinished last line, at the end of the output.
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.pending);
        (!rest.trim().is_empty()).then_some(rest)
    }
}

/// Hosts a sign-in page may be on. The app opens only pages on these hosts.
fn allowed_hosts(service: SubscriptionService) -> &'static [&'static str] {
    match service {
        SubscriptionService::Claude => &["claude.ai", "console.anthropic.com"],
        SubscriptionService::ChatGpt => &["auth.openai.com"],
        SubscriptionService::Copilot => &["github.com"],
        SubscriptionService::Gemini => &["accounts.google.com"],
    }
}

/// Whether `url` is an https page of the subscription's provider.
pub fn is_sign_in_page(service: SubscriptionService, url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.port().is_none()
        && parsed
            .host_str()
            .is_some_and(|h| allowed_hosts(service).contains(&h))
}

/// Whether a subscription can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInState {
    Connected,
    /// Signed in, but the token is no longer usable (sign in again).
    Expired,
    SignedOut,
}

/// The accounts in `omp token <p> --list` output (`N. identity` lines).
pub fn parse_accounts(success: bool, stdout: &str) -> Vec<String> {
    if !success {
        return Vec::new();
    }
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (number, rest) = line.split_once(". ")?;
            if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let identity = rest.trim();
            (!identity.is_empty()).then(|| identity.chars().take(200).collect())
        })
        .collect()
}

pub fn classify(accounts: &[String], token_ok: bool) -> SignInState {
    match (accounts.is_empty(), token_ok) {
        (true, _) => SignInState::SignedOut,
        (false, true) => SignInState::Connected,
        (false, false) => SignInState::Expired,
    }
}

/// One step of a running sign-in, from the login's output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoginStep {
    /// The provider's page to open.
    OpenPage { url: String },
    /// Copilot: the code to enter on GitHub's page.
    DeviceCode { code: String },
    /// A progress message from the runtime.
    Progress { message: String },
    /// The account was saved.
    Saved,
}

/// Reads a sign-in's stdout line by line.
#[derive(Debug, Default)]
pub struct LoginParser {
    expect_url: bool,
}

const OPEN_URL: &str = "Open this URL in your browser:";
const DEVICE_CODE: &str = "Enter code:";
const SAVED: &str = "Credentials saved to";
/// Prompt text that can precede other output on the same line (no newline).
const PROMPTS: [&str; 2] = [
    "Paste the authorization code (or full redirect URL):",
    "GitHub Enterprise URL/domain (blank for github.com) (company.ghe.com):",
];

impl LoginParser {
    pub fn feed(&mut self, raw: &str) -> Option<LoginStep> {
        let mut line = raw.trim();
        for prompt in PROMPTS {
            if let Some(rest) = line.strip_prefix(prompt) {
                line = rest.trim();
            }
        }
        if line.is_empty() {
            return None;
        }
        if self.expect_url {
            self.expect_url = false;
            if line.starts_with("https://") {
                return Some(LoginStep::OpenPage {
                    url: line.to_string(),
                });
            }
        }
        if line.starts_with(OPEN_URL) {
            self.expect_url = true;
            return None;
        }
        if let Some(code) = line.strip_prefix(DEVICE_CODE) {
            let code = code.trim();
            if !code.is_empty()
                && code.len() <= 32
                && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                return Some(LoginStep::DeviceCode {
                    code: code.to_string(),
                });
            }
        }
        if line.starts_with(SAVED) {
            return Some(LoginStep::Saved);
        }
        if line.starts_with("Local shortcut") || line.starts_with("http") {
            return None;
        }
        Some(LoginStep::Progress {
            message: line.chars().take(300).collect(),
        })
    }
}

/// The last meaningful line of a failed command's stderr, without ANSI codes
/// and local paths' noise, for the UI.
pub fn failure_message(stderr: &str) -> String {
    let clean: String = strip_ansi(stderr);
    let line = clean
        .lines()
        .map(str::trim)
        .rev()
        .find(|l| !l.is_empty() && !l.starts_with("at "))
        .unwrap_or("The sign-in did not finish.");
    let line = line
        .strip_prefix("Login failed: ")
        .or_else(|| line.strip_prefix("Error: "))
        .unwrap_or(line);
    line.chars().take(300).collect()
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The isolated environment for sign-in commands: the accounts directory,
/// no model, no credentials.
pub fn accounts_env(
    layout: &OmpLayout,
    parent: impl Fn(&str) -> Option<String>,
) -> Vec<(String, super::model::EnvValue)> {
    let accounts = OmpModel {
        model_arg: String::new(),
        provider_label: "none",
        is_local: false,
        env: Vec::new(),
        warning: None,
        uses_accounts: true,
    };
    child_env(layout, &accounts, parent)
}

/// A command against the accounts directory, ready to spawn.
pub fn command(
    binary: &Path,
    layout: &OmpLayout,
    service: SubscriptionService,
    args: &[String],
) -> Result<Command, HarnessError> {
    std::fs::create_dir_all(&layout.accounts_dir)?;
    std::fs::create_dir_all(&layout.home)?;
    std::fs::create_dir_all(&layout.temp)?;
    let mut command = Command::new(binary);
    command
        .args(args)
        .env_clear()
        .current_dir(&layout.temp)
        .kill_on_drop(true);
    for (name, value) in accounts_env(layout, |n| std::env::var(n).ok()) {
        command.env(name, value.as_str());
    }
    if service == SubscriptionService::Gemini {
        for var in super::model::GEMINI_PROJECT_VARS {
            if let Some(value) = std::env::var(var).ok().filter(|v| !v.trim().is_empty()) {
                command.env(var, value.trim());
            }
        }
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command)
}

/// The subscription's state, from omp's own `token` command.
pub async fn status(
    binary: &Path,
    layout: &OmpLayout,
    service: SubscriptionService,
) -> Result<(SignInState, Vec<String>), HarnessError> {
    let mut list = command(binary, layout, service, &accounts_args(service))?;
    list.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = tokio::time::timeout(STATUS_TIMEOUT, list.output())
        .await
        .map_err(|_| HarnessError::Spawn("omp token --list did not answer in time".into()))?
        .map_err(|e| HarnessError::Spawn(e.to_string()))?;
    let accounts = parse_accounts(
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
    );
    if accounts.is_empty() {
        return Ok((SignInState::SignedOut, accounts));
    }
    // The token is printed on stdout: never read it.
    let mut check = command(binary, layout, service, &token_check_args(service))?;
    check
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let token_ok = match tokio::time::timeout(STATUS_TIMEOUT, check.status()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(e)) => return Err(HarnessError::Spawn(e.to_string())),
        Err(_) => false,
    };
    Ok((classify(&accounts, token_ok), accounts))
}

/// Remove the signed-in account.
pub async fn sign_out(
    binary: &Path,
    layout: &OmpLayout,
    service: SubscriptionService,
) -> Result<(), HarnessError> {
    let mut logout = command(binary, layout, service, &logout_args(service))?;
    logout
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(STATUS_TIMEOUT, logout.output())
        .await
        .map_err(|_| HarnessError::Spawn("omp auth-broker logout did not answer in time".into()))?
        .map_err(|e| HarnessError::Spawn(e.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(HarnessError::Spawn(failure_message(
            &String::from_utf8_lossy(&output.stderr),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_name_one_provider_from_the_fixed_list() {
        assert_eq!(
            login_args(SubscriptionService::Claude),
            ["auth-broker", "login", "anthropic"]
        );
        assert_eq!(
            login_args(SubscriptionService::ChatGpt),
            ["auth-broker", "login", "openai-codex"]
        );
        assert_eq!(
            login_args(SubscriptionService::Copilot),
            ["auth-broker", "login", "github-copilot"]
        );
        assert_eq!(
            login_args(SubscriptionService::Gemini),
            ["auth-broker", "login", "google-gemini-cli"]
        );
        assert_eq!(
            logout_args(SubscriptionService::Gemini),
            ["auth-broker", "logout", "google-gemini-cli"]
        );
        assert_eq!(
            accounts_args(SubscriptionService::Copilot),
            ["token", "github-copilot", "--list"]
        );
        assert_eq!(
            token_check_args(SubscriptionService::Claude),
            ["token", "anthropic"]
        );
        for service in SubscriptionService::ALL {
            // A provider id is always given: without one omp opens an interactive picker.
            assert_eq!(login_args(service).len(), 3);
            assert!(!login_args(service)[2].starts_with('-'));
        }
        let asked = "GitHub Enterprise URL/domain (blank for github.com) (company.ghe.com): ";
        assert_eq!(
            prompt_answer(SubscriptionService::Copilot, asked),
            Some("\n")
        );
        assert_eq!(
            prompt_answer(SubscriptionService::Copilot, ""),
            None,
            "only once asked"
        );
        assert_eq!(prompt_answer(SubscriptionService::Claude, asked), None);
        assert_eq!(
            prompt_answer(
                SubscriptionService::Claude,
                "Paste the authorization code (or full redirect URL): "
            ),
            None,
            "the browser callback answers Claude, never the app"
        );
    }

    #[test]
    fn status_is_parsed_from_omps_own_output() {
        // `omp token anthropic --list` with no account (captured from omp 18.4.10):
        // stderr "No OAuth accounts found for provider "anthropic"." and exit 1.
        assert!(parse_accounts(false, "").is_empty());
        assert_eq!(
            classify(&parse_accounts(false, ""), false),
            SignInState::SignedOut
        );
        // The source's format: `${position + 1}. ${identity}`.
        let listed = "1. ada@example.com (Example Org)\n2. credential #7\n";
        let accounts = parse_accounts(true, listed);
        assert_eq!(accounts, ["ada@example.com (Example Org)", "credential #7"]);
        assert_eq!(classify(&accounts, true), SignInState::Connected);
        assert_eq!(classify(&accounts, false), SignInState::Expired);
        assert!(parse_accounts(true, "Warning: something\n").is_empty());
        assert!(parse_accounts(true, "x. y\n").is_empty());
    }

    #[test]
    fn login_output_becomes_steps() {
        let mut p = LoginParser::default();
        // oauth-code providers (Claude, ChatGPT, Gemini).
        assert_eq!(p.feed(""), None);
        assert_eq!(p.feed("Open this URL in your browser:"), None);
        assert_eq!(
            p.feed("https://claude.ai/oauth/authorize?code=true&client_id=x&state=abc"),
            Some(LoginStep::OpenPage {
                url: "https://claude.ai/oauth/authorize?code=true&client_id=x&state=abc".into()
            })
        );
        assert_eq!(
            p.feed("Local shortcut (this machine only): http://localhost:54545/launch"),
            None
        );
        assert_eq!(
            p.feed("Waiting for browser authentication..."),
            Some(LoginStep::Progress {
                message: "Waiting for browser authentication...".into()
            })
        );
        assert_eq!(
            p.feed("Paste the authorization code (or full redirect URL): Exchanging authorization code for tokens..."),
            Some(LoginStep::Progress {
                message: "Exchanging authorization code for tokens...".into()
            })
        );
        assert_eq!(
            p.feed("Credentials saved to C:\\data\\omp\\accounts\\agent.db"),
            Some(LoginStep::Saved)
        );

        // Copilot's device flow, after the enterprise prompt was answered.
        let mut c = LoginParser::default();
        assert_eq!(
            c.feed("GitHub Enterprise URL/domain (blank for github.com) (company.ghe.com): "),
            None
        );
        assert_eq!(c.feed("Open this URL in your browser:"), None);
        assert_eq!(
            c.feed("https://github.com/login/device"),
            Some(LoginStep::OpenPage {
                url: "https://github.com/login/device".into()
            })
        );
        assert_eq!(
            c.feed("Enter code: ABCD-1234"),
            Some(LoginStep::DeviceCode {
                code: "ABCD-1234".into()
            })
        );
        assert!(matches!(
            c.feed("Enter code: <script>"),
            Some(LoginStep::Progress { .. })
        ));
    }

    #[test]
    fn output_is_split_into_lines_and_a_waiting_prompt() {
        let mut split = LineSplitter::default();
        assert!(split.push("Open this URL").is_empty());
        assert_eq!(split.pending(), "Open this URL");
        assert_eq!(
            split.push(" in your browser:\r\nhttps://github.com/login/device\nGitHub Enterprise"),
            [
                "Open this URL in your browser:",
                "https://github.com/login/device"
            ]
        );
        assert_eq!(split.pending(), "GitHub Enterprise");
        assert_eq!(split.finish().as_deref(), Some("GitHub Enterprise"));
        assert_eq!(split.finish(), None);
        let mut long = LineSplitter::default();
        assert_eq!(long.push(&"x".repeat(70 * 1024)).len(), 1);
        assert_eq!(long.pending(), "");
    }

    #[test]
    fn only_the_providers_https_pages_are_opened() {
        use SubscriptionService::*;
        assert!(is_sign_in_page(
            Claude,
            "https://claude.ai/oauth/authorize?x=1"
        ));
        assert!(is_sign_in_page(
            ChatGpt,
            "https://auth.openai.com/oauth/authorize?x=1"
        ));
        assert!(is_sign_in_page(Copilot, "https://github.com/login/device"));
        assert!(is_sign_in_page(
            Gemini,
            "https://accounts.google.com/o/oauth2/v2/auth?x"
        ));
        assert!(!is_sign_in_page(Claude, "http://claude.ai/oauth/authorize"));
        assert!(!is_sign_in_page(
            Claude,
            "https://claude.ai.evil.example/oauth"
        ));
        assert!(!is_sign_in_page(
            Claude,
            "https://auth.openai.com/oauth/authorize"
        ));
        assert!(!is_sign_in_page(
            Copilot,
            "https://user@github.com/login/device"
        ));
        assert!(!is_sign_in_page(
            Copilot,
            "https://github.com:8443/login/device"
        ));
        assert!(!is_sign_in_page(
            Gemini,
            "file:///C:/Windows/system32/calc.exe"
        ));
        assert!(!is_sign_in_page(Gemini, "not a url"));
    }

    #[test]
    fn failures_are_short_and_plain() {
        assert_eq!(
            failure_message(
                "\u{1b}[31mLogin failed: OAuth callback port 1455 is in use.\u{1b}[39m\n"
            ),
            "OAuth callback port 1455 is in use."
        );
        assert_eq!(failure_message(""), "The sign-in did not finish.");
        assert_eq!(failure_message(&"x".repeat(1000)).len(), 300);
    }

    #[test]
    fn sign_in_commands_use_the_accounts_directory_and_no_keys() {
        let layout = OmpLayout::new(Path::new("/data"));
        let env = accounts_env(&layout, |name| match name {
            "ANTHROPIC_API_KEY" => Some("sk-ant-leak".into()),
            "PATH" => Some("/usr/bin".into()),
            _ => None,
        });
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str().to_string())
        };
        assert_eq!(
            get("PI_CODING_AGENT_DIR"),
            Some(layout.accounts_dir.display().to_string())
        );
        assert_eq!(get("ANTHROPIC_API_KEY"), None);
        assert_eq!(get("PATH").as_deref(), Some("/usr/bin"));
    }
}
