//! git over the network, done by Shodh on this computer (never by a sandboxed command),
//! with the user's GitHub sign-in that never touches the project.
//!
//! **Where the network git runs.** Not in the project: its `.git/config` can be changed
//! by any command (`url.*.insteadOf`, `remote.<x>.uploadpack`, `protocol.ext.allow`,
//! `core.sshCommand`, `http.*.proxy`, `include.path`, a credential helper), and a git
//! transport that reads that file could run a program or send data elsewhere. Every
//! fetch and push to GitHub runs in a bare **mirror** Shodh owns
//! (`<data>/connectors/github/mirrors/<owner>/<repo>.git`), whose config Shodh writes
//! afresh before each use, with the system and global git config switched off.
//!
//! **How objects move between the project and the mirror.** Never by a git transport
//! (`fetch`, `push`, `clone` or anything that takes a URL or a path to another
//! repository): those apply the repository's own `insteadOf`, `remote.*`, `protocol.*`
//! and `core.sshCommand` settings to the path. Instead, by plumbing that reads no
//! transport setting: `pack-objects --revs --stdout` in one repository, piped into
//! `index-pack --stdin` in the other, then `update-ref`. Objects coming from the project
//! are checked as they enter the mirror (`index-pack --strict`).
//!
//! **How the sign-in is given.** As an `Authorization` header scoped to
//! `https://github.com/`, through git's `GIT_CONFIG_COUNT` environment of that one git
//! process: never on its command line, never in a file, never in the project's or the
//! mirror's config, never in a command's environment. Credential helpers, askpass
//! programs and prompts are off, inherited `GIT_*` settings are dropped, hooks come from
//! an empty folder, and only HTTPS may be used. Same-user programs can read a running
//! process's environment on every system (on Windows, where Shodh's commands are not
//! sandboxed yet, a command can also read the credential store directly); this keeps the
//! token away from commands' own environments and from files, not from such a program.
//!
//! **What is pushed.** Exactly the commit the user approved
//! (`<sha>:refs/heads/shodh/<topic>`), never `HEAD`, never with `+` or `--force`; a
//! branch that moved on GitHub rejects the push instead of being overwritten.
//!
//! Requires git 2.32 or newer (`GIT_CONFIG_COUNT`, `GIT_CONFIG_GLOBAL`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::Engine as _;

use super::remote::RepoRef;
use super::token::{scrub, UserToken};

/// Where Shodh's git connects: GitHub over HTTPS. Tests use a local folder (`file://`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitEndpoint {
    base: String,
}

impl GitEndpoint {
    pub fn github() -> Self {
        Self {
            base: "https://github.com/".to_string(),
        }
    }

    /// An endpoint at `base` (`https://…/` or `file:///…/`).
    pub fn at(base: &str) -> Self {
        let mut base = base.to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        Self { base }
    }

    fn scheme(&self) -> &str {
        self.base.split("://").next().unwrap_or("https")
    }

    pub fn url(&self, repo: &RepoRef) -> String {
        repo.git_url(&self.base)
    }
}

/// The oldest git with `GIT_CONFIG_COUNT` and `GIT_CONFIG_GLOBAL`.
pub const MIN_GIT: (u32, u32) = (2, 32);
/// Longest a network git command may take.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Longest a local git command may take.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Most commits listed in an approval.
pub const MAX_LISTED_COMMITS: usize = 30;
/// The mirror's ref namespace for commits on their way out (one ref per approval).
const OUTGOING: &str = "refs/shodh-outgoing/";

/// Why a git step failed. Never holds the token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error("git was not found on this computer. Install git, then try again.")]
    Missing,
    #[error("Shodh needs git 2.32 or newer to work with GitHub safely; this computer has {0}. Update git, then try again.")]
    TooOld(String),
    #[error(
        "GitHub did not accept Shodh's sign-in for git. Sign in again under Connectors → GitHub."
    )]
    AuthFailed,
    #[error("GitHub rejected the push: {0}")]
    Rejected(String),
    #[error("This repository moved on GitHub (it was renamed or transferred). Point the project's remote at its new name, then ask again.")]
    Moved,
    #[error("{0}")]
    Failed(String),
    #[error("git took too long and was stopped")]
    TimedOut,
}

/// A commit an approval lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitLine {
    pub title: String,
    pub files: u64,
    pub insertions: u64,
    pub deletions: u64,
    /// A merge commit (its numbers are against its first parent).
    pub merge: bool,
}

impl CommitLine {
    /// `“Fix the login” · 2 files, +10 −3`: the title is quoted, so a title cannot pass
    /// for the numbers Shodh adds.
    pub fn words(&self) -> String {
        let files = if self.files == 1 {
            "1 file".to_string()
        } else {
            format!("{} files", self.files)
        };
        let merge = if self.merge { "merge commit, " } else { "" };
        format!(
            "“{}” · {merge}{files}, +{} −{}",
            self.title, self.insertions, self.deletions
        )
    }
}

/// What a push would send, prepared before the approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushPlan {
    /// The exact commit to push.
    pub sha: String,
    /// The project's branch the commit was on (`None`: a detached HEAD).
    pub local_branch: Option<String>,
    /// The newest commits not yet on GitHub, newest first (at most
    /// [`MAX_LISTED_COMMITS`]).
    pub commits: Vec<CommitLine>,
    /// How many commits are not yet on GitHub.
    pub total: u64,
    /// Every file those commits change, merge commits diffed against each parent (so a
    /// change made in a merge's resolution is listed too).
    pub files: Vec<String>,
    /// The mirror ref holding the commit until it is pushed.
    pub outgoing: String,
    /// The repository's workflows (on its default branch) that run on a push to a new
    /// branch, by path.
    pub push_workflows: Vec<String>,
    /// The code the push brings, for the approval.
    pub diff: PushDiff,
}

/// The code a push brings, as its approval shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushDiff {
    /// Each changed file with its hunks (cut at [`MAX_PUSH_DIFF_LINES`] lines in all),
    /// against where the push leaves `against` (the default branch).
    Files {
        files: Vec<crate::code_review::diff::FileChange>,
        against: String,
    },
    /// More than the approval can show.
    TooLarge,
    /// git could not compare it.
    Unavailable,
}

/// Lines a push approval's diff shows at most.
pub const MAX_PUSH_DIFF_LINES: usize = 1_500;
/// Changed lines (over every commit) past which a push's diff is not read.
pub const MAX_PUSH_DIFF_SOURCE_LINES: u64 = 20_000;
/// Bytes of git's diff read at most.
const MAX_PUSH_DIFF_BYTES: usize = 4 * 1024 * 1024;
/// git's empty tree object.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Lines inserted and deleted, from `git diff --shortstat` ("3 files changed, 10
/// insertions(+), 2 deletions(-)").
fn shortstat_lines(text: &str) -> u64 {
    text.split(',')
        .filter(|part| part.contains("insertion") || part.contains("deletion"))
        .filter_map(|part| part.split_whitespace().next()?.parse::<u64>().ok())
        .sum()
}

/// Runs `command` and reads its stdout up to `cap` bytes: `Ok(None)` when there is
/// more (the process is then stopped), `Err` when it fails.
async fn run_capped(
    command: Command,
    timeout: Duration,
    cap: usize,
) -> Result<Option<String>, GitError> {
    use tokio::io::AsyncReadExt;
    let mut command = tokio::process::Command::from(command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(GitError::Missing),
        Err(e) => return Err(GitError::Failed(format!("git could not be started: {e}"))),
    };
    let Some(stdout) = child.stdout.take() else {
        return Err(GitError::Failed("git gave no output".to_string()));
    };
    let read = async {
        let mut bytes = Vec::new();
        // One byte past the cap tells that there is more.
        let mut limited = stdout.take(cap as u64 + 1);
        limited
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| GitError::Failed(format!("git's output could not be read: {e}")))?;
        Ok::<_, GitError>(bytes)
    };
    let bytes = match tokio::time::timeout(timeout, read).await {
        Ok(result) => result?,
        Err(_) => return Err(GitError::TimedOut),
    };
    if bytes.len() > cap {
        let _ = child.kill().await;
        return Ok(None);
    }
    let status = child
        .wait()
        .await
        .map_err(|e| GitError::Failed(format!("git failed: {e}")))?;
    if !status.success() {
        return Err(GitError::Failed(
            "git could not compare the code".to_string(),
        ));
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Shodh's git for one data folder.
#[derive(Debug, Clone)]
pub struct HostGit {
    root: PathBuf,
    endpoint: GitEndpoint,
}

/// The variables of Shodh's own environment a network git drops: every `GIT_*` (a
/// `GIT_CONFIG_*`, `GIT_SSH*`, `GIT_ASKPASS`, `GIT_TRACE*` could redirect, prompt or log
/// the sign-in), askpass programs and the credential manager's settings.
fn dropped(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("GIT_")
        || upper.starts_with("GCM_")
        || upper == "SSH_ASKPASS"
        || upper == "SSH_ASKPASS_REQUIRE"
        || upper == "GH_TOKEN"
        || upper == "GITHUB_TOKEN"
}

/// The base64 of `x-access-token:<token>`, as the header carries it.
fn encoded(secret: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{secret}"))
}

/// `text` without the token, raw or in the header's base64 form.
fn scrub_all(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    scrub(&scrub(text, secret), &encoded(secret))
}

/// The `Authorization` header value git sends (`basic`, user `x-access-token`).
fn auth_header(token: &UserToken) -> String {
    format!("AUTHORIZATION: basic {}", encoded(token.expose()))
}

/// The `GIT_CONFIG_*` settings of a network git: what it may use and, for HTTPS, the
/// sign-in scoped to the endpoint.
fn network_settings(
    endpoint: &GitEndpoint,
    hooks: &Path,
    token: Option<&UserToken>,
) -> Vec<(String, String)> {
    let scheme = endpoint.scheme().to_ascii_lowercase();
    let mut settings = vec![
        ("credential.helper".to_string(), String::new()),
        ("core.askPass".to_string(), String::new()),
        ("core.hooksPath".to_string(), hooks.display().to_string()),
        ("core.fsmonitor".to_string(), "false".to_string()),
        ("protocol.allow".to_string(), "never".to_string()),
        (format!("protocol.{scheme}.allow"), "always".to_string()),
        ("http.sslVerify".to_string(), "true".to_string()),
        ("http.followRedirects".to_string(), "false".to_string()),
        ("http.lowSpeedLimit".to_string(), "1000".to_string()),
        ("http.lowSpeedTime".to_string(), "120".to_string()),
        ("fetch.recurseSubmodules".to_string(), "false".to_string()),
        ("push.recurseSubmodules".to_string(), "no".to_string()),
        ("push.followTags".to_string(), "false".to_string()),
    ];
    if scheme == "https" {
        if let Some(token) = token {
            settings.push((
                format!("http.{}.extraHeader", endpoint.base),
                auth_header(token),
            ));
        }
    }
    settings
}

/// git's messages in English, whatever the user's language: Shodh reads them.
fn english(command: &mut Command) {
    command.env("LC_ALL", "C").env("LANGUAGE", "");
}

fn no_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// A network git in `mirror`, built from Shodh's environment `parent` (pure, so tests see
/// exactly its arguments and environment).
pub fn network_command(
    mirror: &Path,
    endpoint: &GitEndpoint,
    hooks: &Path,
    empty_config: &Path,
    token: Option<&UserToken>,
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    args: &[&str],
) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(mirror).args(args);
    for (name, _) in parent {
        if dropped(&name.to_string_lossy()) {
            command.env_remove(&name);
        }
    }
    let settings = network_settings(endpoint, hooks, token);
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", empty_config)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_CONFIG_COUNT", settings.len().to_string());
    for (i, (key, value)) in settings.iter().enumerate() {
        command
            .env(format!("GIT_CONFIG_KEY_{i}"), key)
            .env(format!("GIT_CONFIG_VALUE_{i}"), value);
    }
    english(&mut command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut command);
    command
}

/// The mirror's whole config, written before every use: hooks from the empty folder
/// whatever runs in it.
fn mirror_config(hooks: &Path) -> String {
    let hooks = hooks
        .display()
        .to_string()
        .replace('\\', "/")
        .replace('"', "");
    format!(
        "[core]\n\trepositoryformatversion = 0\n\tbare = true\n\tlogallrefupdates = false\n\thooksPath = \"{hooks}\"\n\tfsmonitor = false\n[gc]\n\tauto = 0\n"
    )
}

/// What a git command printed.
struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
}

async fn run_full(
    mut command: Command,
    timeout: Duration,
    secret: &str,
) -> Result<Output, GitError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(GitError::Missing),
        Err(e) => return Err(GitError::Failed(format!("git could not be started: {e}"))),
    };
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(GitError::Failed(format!("git failed: {e}"))),
        Err(_) => return Err(GitError::TimedOut),
    };
    Ok(Output {
        ok: output.status.success(),
        stdout: scrub_all(&String::from_utf8_lossy(&output.stdout), secret),
        stderr: scrub_all(&String::from_utf8_lossy(&output.stderr), secret),
    })
}

async fn run(command: Command, timeout: Duration, secret: &str) -> Result<String, GitError> {
    let output = run_full(command, timeout, secret).await?;
    if output.ok {
        return Ok(output.stdout);
    }
    failure(&output)
}

/// Runs `command` with `input` on its stdin (local commands only).
async fn run_input(mut command: Command, input: String) -> Result<String, GitError> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(GitError::Missing),
        Err(e) => return Err(GitError::Failed(format!("git could not be started: {e}"))),
    };
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|e| GitError::Failed(format!("git could not be given its input: {e}")))?;
    }
    let output = match tokio::time::timeout(LOCAL_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(GitError::Failed(format!("git failed: {e}"))),
        Err(_) => return Err(GitError::TimedOut),
    };
    let output = Output {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    };
    if output.ok {
        Ok(output.stdout)
    } else {
        failure(&output)
    }
}

/// `producer`'s stdout (a pack) into `consumer`'s stdin, `revs` on the producer's stdin;
/// both local. Blocking: run on a blocking thread.
fn pipe_blocking(
    mut producer: Command,
    revs: String,
    mut consumer: Command,
) -> Result<String, GitError> {
    let started = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::Missing
        } else {
            GitError::Failed(format!("git could not be started: {e}"))
        }
    };
    producer
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut make = producer.spawn().map_err(started)?;
    let (Some(mut stdin), Some(pack), Some(mut make_err)) =
        (make.stdin.take(), make.stdout.take(), make.stderr.take())
    else {
        let _ = make.kill();
        return Err(GitError::Failed("git's pipes could not be opened".into()));
    };
    let writer = std::thread::spawn(move || {
        let written = stdin.write_all(revs.as_bytes());
        drop(stdin);
        written
    });
    let make_errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = make_err.read_to_string(&mut text);
        text
    });
    consumer
        .stdin(Stdio::from(pack))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let take = match consumer.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = make.kill();
            let _ = make.wait();
            return Err(started(e));
        }
    };
    let taken = take
        .wait_with_output()
        .map_err(|e| GitError::Failed(format!("git failed: {e}")))?;
    let made = make
        .wait()
        .map_err(|e| GitError::Failed(format!("git failed: {e}")))?;
    let write_result = writer.join().unwrap_or(Ok(()));
    let make_stderr = make_errors.join().unwrap_or_default();
    if !made.success() {
        return failure(&Output {
            ok: false,
            stdout: String::new(),
            stderr: make_stderr,
        });
    }
    if let Err(e) = write_result {
        return Err(GitError::Failed(format!(
            "git could not be given its input: {e}"
        )));
    }
    let output = Output {
        ok: taken.status.success(),
        stdout: String::from_utf8_lossy(&taken.stdout).to_string(),
        stderr: String::from_utf8_lossy(&taken.stderr).to_string(),
    };
    if output.ok {
        Ok(output.stdout)
    } else {
        failure(&output)
    }
}

async fn pipe(producer: Command, revs: String, consumer: Command) -> Result<String, GitError> {
    let task = tokio::task::spawn_blocking(move || pipe_blocking(producer, revs, consumer));
    match tokio::time::timeout(LOCAL_TIMEOUT, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(GitError::Failed(format!("git failed: {e}"))),
        Err(_) => Err(GitError::TimedOut),
    }
}

/// The error of a failed git command.
fn failure<T>(output: &Output) -> Result<T, GitError> {
    let stdout = output.stdout.as_str();
    let stderr = output.stderr.trim();
    let lower = stderr.to_ascii_lowercase();
    // Only a refused sign-in is one: a 403 (the user may only read, or the app is not
    // installed there) leaves the sign-in good and is said as a refusal.
    if lower.contains("authentication failed")
        || lower.contains("could not read username")
        || lower.contains("invalid username or password")
        || lower.contains("http 401")
        || lower.contains("error: 401")
    {
        return Err(GitError::AuthFailed);
    }
    if lower.contains("error: 403")
        || lower.contains("http 403")
        || (lower.contains("permission to ") && lower.contains("denied"))
    {
        return Err(GitError::Failed(
            "GitHub refused this: you, or the Shodh app on this repository, may not write to it. Check your access and where the app is installed.".to_string(),
        ));
    }
    // Redirects are not followed (a renamed or transferred repository answers 301).
    if lower.contains("error: 301") || lower.contains("http 301") || lower.contains("redirect") {
        return Err(GitError::Moved);
    }
    let message: String = if stderr.is_empty() {
        stdout.trim().chars().take(600).collect()
    } else {
        stderr.chars().take(600).collect()
    };
    Err(GitError::Failed(if message.is_empty() {
        "git failed without saying why".to_string()
    } else {
        message
    }))
}

/// A local git in the project, hardened like Code mode's own git (no hooks, no file
/// system monitor), with no transport allowed at all: Shodh never runs a fetch, push or
/// clone in the project, and this makes any that slipped in fail.
fn project_git(project: &Path, args: &[&str]) -> Result<Command, GitError> {
    let mut command = crate::harness::code_mode::git(project).map_err(GitError::Failed)?;
    command.args(["-c", "protocol.allow=never"]);
    command.args(args);
    english(&mut command);
    Ok(command)
}

/// The version of git on this computer, as `(major, minor)`.
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let version = text.trim().strip_prefix("git version ")?;
    let mut parts = version.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Parses `git log --format=%x1e%P%x1f%s --shortstat` output.
fn parse_commits(text: &str) -> Vec<CommitLine> {
    text.split('\u{1e}')
        .filter(|record| !record.trim().is_empty())
        .map(|record| {
            let mut lines = record.lines();
            let first = lines.next().unwrap_or_default();
            let (parents, title) = first.split_once('\u{1f}').unwrap_or(("", first));
            let title = title.trim();
            let mut line = CommitLine {
                title: if title.is_empty() {
                    "(no title)".to_string()
                } else {
                    title.chars().take(200).collect()
                },
                files: 0,
                insertions: 0,
                deletions: 0,
                merge: parents.split_whitespace().count() > 1,
            };
            for stat in lines.filter(|l| l.contains("changed")) {
                for part in stat.split(',') {
                    let part = part.trim();
                    let n = part
                        .split_whitespace()
                        .next()
                        .and_then(|n| n.parse::<u64>().ok())
                        .unwrap_or(0);
                    if part.contains("file") {
                        line.files = n;
                    } else if part.contains("insertion") {
                        line.insertions = n;
                    } else if part.contains("deletion") {
                        line.deletions = n;
                    }
                }
            }
            line
        })
        .collect()
}

/// What `git push --porcelain` said about a rejected ref.
fn rejection(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let mut fields = line.split('\t');
        let flag = fields.next()?;
        if flag != "!" {
            return None;
        }
        let _refs = fields.next()?;
        let summary = fields.next().unwrap_or("rejected").trim();
        Some(if summary.contains("stale info") {
            "the branch moved on GitHub since you approved, so it was kept.".to_string()
        } else if summary.contains("non-fast-forward") || summary.contains("fetch first") {
            "the branch on GitHub has commits this one does not. Fetch, merge them in, then push again.".to_string()
        } else {
            summary.chars().take(200).collect()
        })
    })
}

/// Whether a workflow file's text runs it on a push to a new `shodh/…` branch: a `push`
/// trigger with no `branches:` list, or one that does not exclude such a branch. Read
/// conservatively: when unsure, it is taken to run.
pub fn runs_on_push(text: &str) -> bool {
    let lines: Vec<(usize, String)> = text
        .lines()
        .map(|raw| {
            raw.split(" #")
                .next()
                .unwrap_or_default()
                .trim_end()
                .to_string()
        })
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| (l.len() - l.trim_start().len(), l.trim().to_string()))
        .collect();
    let is_on = |t: &str| {
        ["on:", "\"on\":", "'on':", "true:"]
            .iter()
            .any(|p| t.starts_with(p))
    };
    let mentions_push = |t: &str| {
        t.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|w| w == "push")
    };
    let mut i = 0;
    while i < lines.len() {
        let (indent, line) = &lines[i];
        if *indent != 0 || !is_on(line) {
            i += 1;
            continue;
        }
        // `on: push` / `on: [push, pull_request]`: every branch.
        let inline = line
            .split_once(':')
            .map(|(_, r)| r.trim())
            .unwrap_or_default();
        if !inline.is_empty() {
            return mentions_push(inline);
        }
        let mut j = i + 1;
        while j < lines.len() && lines[j].0 > 0 {
            let (level, entry) = &lines[j];
            // `- push` in a list of events: every branch.
            if entry == "- push" {
                return true;
            }
            let key = entry
                .split(':')
                .next()
                .unwrap_or_default()
                .trim()
                .trim_matches(['"', '\'']);
            if key == "push" {
                let rest = entry
                    .split_once(':')
                    .map(|(_, r)| r.trim())
                    .unwrap_or_default();
                let mut body = rest.to_string();
                let mut k = j + 1;
                while k < lines.len() && lines[k].0 > *level {
                    body.push(' ');
                    body.push_str(&lines[k].1);
                    k += 1;
                }
                return push_body_runs(&body);
            }
            j += 1;
        }
        return false;
    }
    false
}

/// Whether a `push:` trigger's settings run it for a new `shodh/…` branch.
fn push_body_runs(body: &str) -> bool {
    let b = body.trim();
    if b.is_empty() || b == "{}" || b == "null" || b == "~" {
        return true;
    }
    let has = |k: &str| b.contains(&format!("{k}:"));
    if has("branches-ignore") {
        // Runs for every branch it does not ignore.
        return !b.contains("shodh");
    }
    if has("branches") {
        return b.contains("**")
            || b.contains("'*'")
            || b.contains("\"*\"")
            || b.contains("- *")
            || b.contains("shodh");
    }
    // `tags:` without `branches:` runs only for tags; `paths:` alone runs for every branch.
    !(has("tags") || has("tags-ignore"))
}

impl HostGit {
    /// Shodh's git, with its mirrors under `<data_dir>/connectors/github/`.
    pub fn new(data_dir: &Path) -> Self {
        Self::with_endpoint(data_dir, GitEndpoint::github())
    }

    pub fn with_endpoint(data_dir: &Path, endpoint: GitEndpoint) -> Self {
        Self {
            root: data_dir.join("connectors").join("github"),
            endpoint,
        }
    }

    pub fn endpoint(&self) -> &GitEndpoint {
        &self.endpoint
    }

    fn mirror_dir(&self, repo: &RepoRef) -> PathBuf {
        self.root
            .join("mirrors")
            .join(repo.owner.to_ascii_lowercase())
            .join(format!("{}.git", repo.name.to_ascii_lowercase()))
    }

    fn empty_config(&self) -> Result<PathBuf, GitError> {
        let path = self.root.join("empty.gitconfig");
        std::fs::create_dir_all(&self.root)
            .and_then(|_| std::fs::write(&path, b""))
            .map_err(|e| {
                GitError::Failed(format!("Shodh's git settings could not be written: {e}"))
            })?;
        Ok(path)
    }

    fn network(
        &self,
        mirror: &Path,
        token: Option<&UserToken>,
        args: &[&str],
    ) -> Result<Command, GitError> {
        let hooks = crate::harness::code_mode::empty_hooks_dir().map_err(GitError::Failed)?;
        let empty = self.empty_config()?;
        Ok(network_command(
            mirror,
            &self.endpoint,
            &hooks,
            &empty,
            token,
            std::env::vars_os(),
            args,
        ))
    }

    /// Checks git's version once per call site.
    pub async fn check_version(&self) -> Result<(), GitError> {
        let mut command = Command::new("git");
        command.arg("--version");
        english(&mut command);
        no_window(&mut command);
        let text = run(command, LOCAL_TIMEOUT, "").await?;
        match parse_version(&text) {
            Some(version) if version >= MIN_GIT => Ok(()),
            Some((major, minor)) => Err(GitError::TooOld(format!("git {major}.{minor}"))),
            None => Err(GitError::TooOld(text.trim().chars().take(40).collect())),
        }
    }

    /// The mirror of `repo`, created when missing, with its config written afresh.
    async fn mirror(&self, repo: &RepoRef) -> Result<PathBuf, GitError> {
        let dir = self.mirror_dir(repo);
        if !dir.join("objects").is_dir() {
            std::fs::create_dir_all(&dir).map_err(|e| {
                GitError::Failed(format!("Shodh's copy of {repo} could not be created: {e}"))
            })?;
            let init = self.network(&dir, None, &["init", "--bare", "--quiet", "--template="])?;
            run(init, LOCAL_TIMEOUT, "").await?;
        }
        let hooks = crate::harness::code_mode::empty_hooks_dir().map_err(GitError::Failed)?;
        std::fs::write(dir.join("config"), mirror_config(&hooks)).map_err(|e| {
            GitError::Failed(format!("Shodh's copy of {repo} could not be set up: {e}"))
        })?;
        Ok(dir)
    }

    /// Brings `repo`'s branches from GitHub into its mirror.
    async fn update_mirror(&self, repo: &RepoRef, token: &UserToken) -> Result<PathBuf, GitError> {
        let mirror = self.mirror(repo).await?;
        let url = self.endpoint.url(repo);
        let fetch = self.network(
            &mirror,
            Some(token),
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "--prune",
                "--no-recurse-submodules",
                "--update-head-ok",
                &url,
                "+refs/heads/*:refs/heads/*",
            ],
        )?;
        run(fetch, NETWORK_TIMEOUT, token.expose()).await?;
        Ok(mirror)
    }

    /// The mirror's branches, `(sha, name)` with the name after `refs/heads/`.
    async fn mirror_heads(&self, mirror: &Path) -> Result<Vec<(String, String)>, GitError> {
        let list = self.network(
            mirror,
            None,
            &[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                "refs/heads",
            ],
        )?;
        Ok(run(list, LOCAL_TIMEOUT, "")
            .await?
            .lines()
            .filter_map(|l| {
                let (sha, name) = l.trim().split_once(' ')?;
                let name = name.strip_prefix("refs/heads/")?;
                Some((sha.to_string(), name.to_string()))
            })
            .collect())
    }

    /// Of `shas`, those the repository whose git `command` runs in has (`cat-file
    /// --batch-check`: no transport, no filter).
    async fn present(command: Command, shas: &[String]) -> Result<Vec<String>, GitError> {
        if shas.is_empty() {
            return Ok(Vec::new());
        }
        let input: String = shas.iter().map(|s| format!("{s}\n")).collect();
        let out = run_input(command, input).await?;
        Ok(out
            .lines()
            .filter(|l| !l.ends_with(" missing"))
            .filter_map(|l| l.split_whitespace().next().map(str::to_string))
            .collect())
    }

    /// The pack-objects input: `want`, then everything in `have` excluded.
    fn revs(want: &[String], have: &[String]) -> String {
        let mut text: String = want.iter().map(|s| format!("{s}\n")).collect();
        if !have.is_empty() {
            text.push_str("--not\n");
            for s in have {
                text.push_str(s);
                text.push('\n');
            }
        }
        text
    }

    /// Copies the mirror's branches into the project's `refs/remotes/<remote>/…`, by
    /// pack (no transport in the project). Returns how many branches there are.
    async fn copy_to_project(
        &self,
        mirror: &Path,
        project: &Path,
        remote: &str,
    ) -> Result<usize, GitError> {
        let heads = self.mirror_heads(mirror).await?;
        let shas: Vec<String> = heads.iter().map(|(s, _)| s.clone()).collect();
        let check = project_git(project, &["cat-file", "--batch-check"])?;
        let known = Self::present(check, &shas).await?;
        let want: Vec<String> = shas
            .iter()
            .filter(|s| !known.contains(s))
            .cloned()
            .collect();
        if !want.is_empty() {
            let make = self.network(
                mirror,
                None,
                &["pack-objects", "--revs", "--stdout", "--quiet"],
            )?;
            let take = project_git(project, &["index-pack", "--stdin"])?;
            pipe(make, Self::revs(&want, &known), take).await?;
        }
        let mut updates = String::new();
        for (sha, name) in &heads {
            if !super::remote::valid_remote_name(name.split('/').next().unwrap_or_default())
                || name.contains("..")
                || name.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                continue;
            }
            updates.push_str(&format!("update refs/remotes/{remote}/{name} {sha}\n"));
        }
        if !updates.is_empty() {
            let record = project_git(project, &["update-ref", "--stdin"])?;
            run_input(record, updates).await?;
        }
        Ok(heads.len())
    }

    /// Copies commit `sha` of the project into the mirror under `outgoing`, by pack, its
    /// objects checked as they enter (`index-pack --strict`).
    async fn copy_to_mirror(
        &self,
        project: &Path,
        mirror: &Path,
        sha: &str,
        outgoing: &str,
    ) -> Result<(), GitError> {
        let heads = self.mirror_heads(mirror).await?;
        let shas: Vec<String> = heads.into_iter().map(|(s, _)| s).collect();
        let check = project_git(project, &["cat-file", "--batch-check"])?;
        let known = Self::present(check, &shas).await?;
        let make = project_git(project, &["pack-objects", "--revs", "--stdout", "--quiet"])?;
        let take = self.network(mirror, None, &["index-pack", "--stdin", "--strict"])?;
        pipe(make, Self::revs(&[sha.to_string()], &known), take).await?;
        let record = self.network(mirror, None, &["update-ref", outgoing, sha])?;
        run(record, LOCAL_TIMEOUT, "").await?;
        Ok(())
    }

    /// Fetches `repo` from GitHub into the project's `refs/remotes/<remote>/…` (a fetch:
    /// the project's own branches and files do not change). Returns how many branches
    /// the remote has.
    async fn fetch_in(
        &self,
        project: &Path,
        remote: &str,
        repo: &RepoRef,
        token: &UserToken,
    ) -> Result<usize, GitError> {
        if !super::remote::valid_remote_name(remote) {
            return Err(GitError::Failed(format!(
                "“{remote}” is not a remote name Shodh can use"
            )));
        }
        let mirror = self.update_mirror(repo, token).await?;
        self.copy_to_project(&mirror, project, remote).await
    }

    /// The workflows of the mirror's `default` branch that run on a push to a new
    /// branch, by path.
    async fn push_workflows(&self, mirror: &Path, default: &str) -> Vec<String> {
        let Ok(list) = self.network(
            mirror,
            None,
            &[
                "ls-tree",
                "--name-only",
                &format!("refs/heads/{default}"),
                ".github/workflows/",
            ],
        ) else {
            return Vec::new();
        };
        let Ok(names) = run(list, LOCAL_TIMEOUT, "").await else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for path in names.lines().map(str::trim).filter(|p| {
            let p = p.to_ascii_lowercase();
            p.ends_with(".yml") || p.ends_with(".yaml")
        }) {
            let Ok(show) = self.network(
                mirror,
                None,
                &["cat-file", "blob", &format!("refs/heads/{default}:{path}")],
            ) else {
                continue;
            };
            match run(show, LOCAL_TIMEOUT, "").await {
                Ok(text) if runs_on_push(&text) => out.push(path.to_string()),
                Ok(_) => {}
                // Unreadable: taken to run.
                Err(_) => out.push(path.to_string()),
            }
        }
        out
    }

    /// Prepares a push of the project's current commit to `repo`: brings GitHub's
    /// branches into the mirror, copies the commit there, and lists what GitHub does not
    /// have yet. Nothing is sent to GitHub.
    async fn prepare_push_in(
        &self,
        project: &Path,
        repo: &RepoRef,
        default_branch: &str,
        token: &UserToken,
        call: &str,
    ) -> Result<PushPlan, GitError> {
        let head = project_git(
            project,
            &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
        )?;
        let sha = run(head, LOCAL_TIMEOUT, "").await.map_err(|_| {
            GitError::Failed(
                "This project has no commit to push yet. Commit the changes first.".to_string(),
            )
        })?;
        let sha = sha.trim().to_string();
        if sha.len() < 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(GitError::Failed(
                "git did not name the current commit".to_string(),
            ));
        }
        let branch = project_git(project, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        let local_branch = run(branch, LOCAL_TIMEOUT, "")
            .await
            .ok()
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty());

        let mirror = self.update_mirror(repo, token).await?;
        self.clear_outgoing(&mirror).await?;
        let id: String = call
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        let outgoing = format!(
            "{OUTGOING}{}",
            if id.is_empty() { "next" } else { id.as_str() }
        );
        self.copy_to_mirror(project, &mirror, &sha, &outgoing)
            .await?;

        // A merge commit's numbers are against its first parent (what it brings into
        // its branch, the resolution included).
        let log = self.network(
            &mirror,
            None,
            &[
                "log",
                "--no-color",
                "--format=%x1e%P%x1f%s",
                "--shortstat",
                "--diff-merges=first-parent",
                &format!("--max-count={MAX_LISTED_COMMITS}"),
                &sha,
                "--not",
                "--branches",
            ],
        )?;
        let commits = parse_commits(&run(log, LOCAL_TIMEOUT, "").await?);
        let count = self.network(
            &mirror,
            None,
            &["rev-list", "--count", &sha, "--not", "--branches"],
        )?;
        let total = run(count, LOCAL_TIMEOUT, "")
            .await?
            .trim()
            .parse::<u64>()
            .unwrap_or(commits.len() as u64);
        // Files: a merge commit against each of its parents, so nothing a merge's
        // resolution adds is left out.
        let names = self.network(
            &mirror,
            None,
            &[
                "log",
                "--no-color",
                "--format=",
                "--name-only",
                "--no-renames",
                "--diff-merges=separate",
                &sha,
                "--not",
                "--branches",
            ],
        )?;
        let mut files: Vec<String> = run(names, LOCAL_TIMEOUT, "")
            .await?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        files.sort();
        files.dedup();
        let push_workflows = self.push_workflows(&mirror, default_branch).await;
        let diff = self.push_diff(&mirror, default_branch, &sha).await;
        Ok(PushPlan {
            sha,
            local_branch,
            commits,
            total,
            files,
            outgoing,
            push_workflows,
            diff,
        })
    }

    /// The code a push brings, against where it leaves the repository's default branch
    /// (their merge base; the empty tree only for a history that shares nothing with it).
    /// Read in the mirror with external diff programs and text conversion off, so no
    /// setting can make git run a program. Its size is checked over every commit before
    /// it is read, and it is read only up to [`MAX_PUSH_DIFF_BYTES`]. `Unavailable` when git
    /// could not say, never a diff of the whole repository.
    async fn push_diff(&self, mirror: &Path, default_branch: &str, sha: &str) -> PushDiff {
        let default_ref = format!("refs/heads/{default_branch}");
        // An empty repository on GitHub has no default branch yet: every file is new.
        let Ok(check) = self.network(
            mirror,
            None,
            &["rev-parse", "--verify", "--quiet", &default_ref],
        ) else {
            return PushDiff::Unavailable;
        };
        let default_exists = match run_full(check, LOCAL_TIMEOUT, "").await {
            Ok(output) if output.ok => true,
            // `--quiet`: a missing branch fails without a word; anything else says why.
            Ok(output) if output.stderr.trim().is_empty() => false,
            Ok(_) => return PushDiff::Unavailable,
            Err(_) => return PushDiff::Unavailable,
        };
        let base = if !default_exists {
            EMPTY_TREE.to_string()
        } else {
            let Ok(command) = self.network(mirror, None, &["merge-base", sha, &default_ref]) else {
                return PushDiff::Unavailable;
            };
            match run_full(command, LOCAL_TIMEOUT, "").await {
                Ok(output) if output.ok => {
                    let found = output.stdout.trim().to_string();
                    if found.len() < 40 || !found.chars().all(|c| c.is_ascii_hexdigit()) {
                        return PushDiff::Unavailable;
                    }
                    found
                }
                // merge-base says nothing and fails when the histories share no commit:
                // then every file of the push is new.
                Ok(output)
                    if output.stdout.trim().is_empty() && output.stderr.trim().is_empty() =>
                {
                    EMPTY_TREE.to_string()
                }
                _ => return PushDiff::Unavailable,
            }
        };
        // Every commit counts: the listed ones are only the newest.
        let stat_args = [
            "diff",
            "--shortstat",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            base.as_str(),
            sha,
        ];
        let Ok(command) = self.network(mirror, None, &stat_args) else {
            return PushDiff::Unavailable;
        };
        match run(command, LOCAL_TIMEOUT, "").await {
            Ok(stat) if shortstat_lines(&stat) > MAX_PUSH_DIFF_SOURCE_LINES => {
                return PushDiff::TooLarge
            }
            Ok(_) => {}
            Err(_) => return PushDiff::Unavailable,
        }
        let args = [
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--unified=3",
            base.as_str(),
            sha,
        ];
        let Ok(command) = self.network(mirror, None, &args) else {
            return PushDiff::Unavailable;
        };
        match run_capped(command, LOCAL_TIMEOUT, MAX_PUSH_DIFF_BYTES).await {
            Ok(Some(text)) => PushDiff::Files {
                files: crate::code_review::diff::capped(
                    crate::code_review::diff::parse_unified(&text),
                    MAX_PUSH_DIFF_LINES,
                ),
                against: default_branch.to_string(),
            },
            Ok(None) => PushDiff::TooLarge,
            Err(_) => PushDiff::Unavailable,
        }
    }

    /// Pushes exactly `plan.sha` to `refs/heads/<branch>` of `repo` (no force), and
    /// records it in the project's `refs/remotes/<remote>/<branch>`.
    async fn push_in(
        &self,
        project: &Path,
        remote: &str,
        repo: &RepoRef,
        branch: &str,
        plan: &PushPlan,
        token: &UserToken,
    ) -> Result<(), GitError> {
        if !super::limits::is_shodh_branch(branch) {
            return Err(GitError::Failed(
                super::limits::Refusal::NotShodhBranch(branch.to_string()).to_string(),
            ));
        }
        if !super::remote::valid_remote_name(remote) {
            return Err(GitError::Failed(format!(
                "“{remote}” is not a remote name Shodh can use"
            )));
        }
        let mirror = self.mirror(repo).await?;
        let url = self.endpoint.url(repo);
        let refspec = format!("{}:refs/heads/{branch}", plan.sha);
        debug_assert!(!refspec.starts_with('+'));
        let push = self.network(
            &mirror,
            Some(token),
            &[
                "push",
                "--porcelain",
                "--no-verify",
                "--no-recurse-submodules",
                &url,
                &refspec,
            ],
        )?;
        let result = run_full(push, NETWORK_TIMEOUT, token.expose()).await;
        // The outgoing copy is no longer needed either way.
        self.clear_outgoing(&mirror).await?;
        let output = result?;
        if !output.ok {
            if let Some(reason) = rejection(&output.stdout) {
                return Err(GitError::Rejected(reason));
            }
            return failure(&output);
        }
        let record = project_git(
            project,
            &[
                "update-ref",
                &format!("refs/remotes/{remote}/{branch}"),
                &plan.sha,
            ],
        )?;
        if let Err(e) = run(record, LOCAL_TIMEOUT, "").await {
            tracing::warn!(target: "shodh::github", error = %e, "the pushed branch was not recorded in the project");
        }
        Ok(())
    }

    /// Deletes `branch` of `repo` only while it still points at `sha` (a push with a
    /// lease from the mirror: a branch that moved since the approval is kept).
    async fn delete_branch_in(
        &self,
        repo: &RepoRef,
        branch: &str,
        sha: &str,
        token: &UserToken,
    ) -> Result<(), GitError> {
        if !super::limits::is_shodh_branch(branch) {
            return Err(GitError::Failed(
                super::limits::Refusal::NotShodhBranch(branch.to_string()).to_string(),
            ));
        }
        if sha.len() < 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(GitError::Failed(
                "GitHub did not name the branch's commit".into(),
            ));
        }
        let mirror = self.mirror(repo).await?;
        let url = self.endpoint.url(repo);
        let lease = format!("--force-with-lease=refs/heads/{branch}:{sha}");
        let target = format!(":refs/heads/{branch}");
        let delete = self.network(
            &mirror,
            Some(token),
            &["push", "--porcelain", "--no-verify", &lease, &url, &target],
        )?;
        let output = run_full(delete, NETWORK_TIMEOUT, token.expose()).await?;
        if !output.ok {
            if let Some(reason) = rejection(&output.stdout) {
                return Err(GitError::Rejected(reason));
            }
            return failure(&output);
        }
        Ok(())
    }

    /// `error` without the mirror's folder (it holds the user's name): the model and the
    /// user are told "Shodh's copy of owner/repo" instead.
    fn hide(&self, repo: &RepoRef, error: GitError) -> GitError {
        let dir = self.mirror_dir(repo);
        let shown = format!("Shodh's copy of {repo}");
        let root = self.root.display().to_string();
        let scrub = |text: String| {
            let mut out = text;
            for form in [
                dir.display().to_string(),
                dir.display().to_string().replace('\\', "/"),
            ] {
                out = out.replace(&form, &shown);
            }
            for form in [root.clone(), root.replace('\\', "/")] {
                out = out.replace(&form, "Shodh's data folder");
            }
            out
        };
        match error {
            GitError::Failed(text) => GitError::Failed(scrub(text)),
            GitError::Rejected(text) => GitError::Rejected(scrub(text)),
            other => other,
        }
    }

    /// Fetches `repo` from GitHub into the project's `refs/remotes/<remote>/…` (a fetch:
    /// the project's own branches and files do not change). Returns how many branches
    /// the remote has.
    pub async fn fetch(
        &self,
        project: &Path,
        remote: &str,
        repo: &RepoRef,
        token: &UserToken,
    ) -> Result<usize, GitError> {
        self.fetch_in(project, remote, repo, token)
            .await
            .map_err(|e| self.hide(repo, e))
    }

    /// Prepares a push (see [`Self::prepare_push_in`]); nothing is sent to GitHub.
    /// `default_branch` is read for the workflows that a push to a new branch runs.
    pub async fn prepare_push(
        &self,
        project: &Path,
        repo: &RepoRef,
        default_branch: &str,
        token: &UserToken,
        call: &str,
    ) -> Result<PushPlan, GitError> {
        self.prepare_push_in(project, repo, default_branch, token, call)
            .await
            .map_err(|e| self.hide(repo, e))
    }

    /// Pushes exactly `plan.sha` (see [`Self::push_in`]).
    pub async fn push(
        &self,
        project: &Path,
        remote: &str,
        repo: &RepoRef,
        branch: &str,
        plan: &PushPlan,
        token: &UserToken,
    ) -> Result<(), GitError> {
        self.push_in(project, remote, repo, branch, plan, token)
            .await
            .map_err(|e| self.hide(repo, e))
    }

    /// Deletes `branch` while it still points at `sha` (see [`Self::delete_branch_in`]).
    pub async fn delete_branch(
        &self,
        repo: &RepoRef,
        branch: &str,
        sha: &str,
        token: &UserToken,
    ) -> Result<(), GitError> {
        self.delete_branch_in(repo, branch, sha, token)
            .await
            .map_err(|e| self.hide(repo, e))
    }

    /// Removes every outgoing ref of the mirror.
    async fn clear_outgoing(&self, mirror: &Path) -> Result<(), GitError> {
        let list = self.network(
            mirror,
            None,
            &["for-each-ref", "--format=%(refname)", OUTGOING],
        )?;
        let refs = run(list, LOCAL_TIMEOUT, "").await?;
        for name in refs
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with(OUTGOING))
        {
            let delete = self.network(mirror, None, &["update-ref", "-d", name])?;
            run(delete, LOCAL_TIMEOUT, "").await?;
        }
        Ok(())
    }
}

/// The project's remotes (`git config --get-regexp`: reading config runs no program).
pub async fn project_remotes(project: &Path) -> Result<Vec<super::remote::Remote>, GitError> {
    let command = project_git(project, &["config", "--get-regexp", r"^remote\..*\.url$"])?;
    match run(command, LOCAL_TIMEOUT, "").await {
        Ok(text) => Ok(super::remote::parse_remotes(&text)),
        // `git config --get-regexp` exits 1 when nothing matches.
        Err(GitError::Failed(text))
            if text.trim().is_empty() || text == "git failed without saying why" =>
        {
            Ok(Vec::new())
        }
        Err(e) => Err(e),
    }
}

/// The settings a network git runs with, by key (for tests and the report).
pub fn settings_of(command: &Command) -> BTreeMap<String, String> {
    let envs: BTreeMap<String, String> = command
        .get_envs()
        .filter_map(|(k, v)| {
            Some((
                k.to_string_lossy().to_string(),
                v?.to_string_lossy().to_string(),
            ))
        })
        .collect();
    let count: usize = envs
        .get("GIT_CONFIG_COUNT")
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (0..count)
        .filter_map(|i| {
            Some((
                envs.get(&format!("GIT_CONFIG_KEY_{i}"))?.clone(),
                envs.get(&format!("GIT_CONFIG_VALUE_{i}"))?.clone(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    use super::super::token::tests::TOKEN;
    use super::*;

    fn token() -> UserToken {
        UserToken::parse(TOKEN).unwrap()
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str, text: &str, title: &str) {
        std::fs::create_dir_all(dir.join(file).parent().unwrap()).unwrap();
        std::fs::write(dir.join(file), text).unwrap();
        git(dir, &["add", "-A"]);
        git(
            dir,
            &["-c", "commit.gpgsign=false", "commit", "-q", "-m", title],
        );
    }

    /// A "GitHub" of bare repositories in a folder, one `octo-org/hello` holding one
    /// commit on `main`, and a clone of it as the project.
    struct World {
        _dir: tempfile::TempDir,
        data: PathBuf,
        remote: PathBuf,
        project: PathBuf,
        host: HostGit,
        repo: RepoRef,
    }

    fn world() -> World {
        let dir = tempfile::tempdir().unwrap();
        let remotes = dir.path().join("remotes");
        let remote = remotes.join("octo-org").join("hello.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--bare", "-q", "-b", "main"]);
        let seed = dir.path().join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-q", "-b", "main"]);
        commit(&seed, "README.md", "hello\n", "First commit");
        git(
            &seed,
            &["push", "-q", remote.to_str().unwrap(), "main:main"],
        );
        let project = dir.path().join("project");
        let out = Command::new("git")
            .args(["clone", "-q"])
            .arg(&remote)
            .arg(&project)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success());
        // What a sandboxed command could have planted in the project's config.
        git(
            &project,
            &[
                "config",
                "url.https://evil.invalid/.insteadOf",
                "https://github.com/",
            ],
        );
        git(&project, &["config", "http.sslVerify", "false"]);
        git(&project, &["config", "http.proxy", "http://127.0.0.1:9"]);
        git(&project, &["config", "credential.helper", "!echo stolen"]);
        let base = url::Url::from_directory_path(&remotes).unwrap().to_string();
        let data = dir.path().join("data");
        World {
            host: HostGit::with_endpoint(&data, GitEndpoint::at(&base)),
            _dir: dir,
            data,
            remote,
            project,
            repo: RepoRef::new("octo-org", "hello").unwrap(),
        }
    }

    fn everything_in(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push((path.clone(), std::fs::read(&path).unwrap()));
                }
            }
        }
        out
    }

    fn holds_token(dir: &Path) -> Option<PathBuf> {
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{TOKEN}"));
        everything_in(dir).into_iter().find_map(|(path, bytes)| {
            let text = String::from_utf8_lossy(&bytes);
            (text.contains(TOKEN) || text.contains(&encoded)).then_some(path)
        })
    }

    #[test]
    fn the_sign_in_is_a_header_for_github_only_never_an_argument() {
        let dir = tempfile::tempdir().unwrap();
        let parent = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            (OsString::from("GIT_CONFIG_COUNT"), OsString::from("1")),
            (
                OsString::from("GIT_CONFIG_KEY_0"),
                OsString::from("url.https://evil.invalid/.insteadOf"),
            ),
            (OsString::from("GIT_SSH_COMMAND"), OsString::from("evil")),
            (OsString::from("GIT_TRACE_CURL"), OsString::from("1")),
            (OsString::from("GCM_TRACE"), OsString::from("1")),
            (OsString::from("GITHUB_TOKEN"), OsString::from("ghp_other")),
        ];
        let command = network_command(
            dir.path(),
            &GitEndpoint::github(),
            Path::new("/hooks"),
            Path::new("/empty"),
            Some(&token()),
            parent,
            &[
                "push",
                "--porcelain",
                "https://github.com/a/b.git",
                "abc:refs/heads/shodh/x",
            ],
        );
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(
            !args
                .iter()
                .any(|a| a.contains(TOKEN) || a.to_ascii_lowercase().contains("authorization")),
            "{args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with('+') || a == "--force" || a == "-f"),
            "{args:?}"
        );
        let settings = settings_of(&command);
        let header = settings
            .get("http.https://github.com/.extraHeader")
            .expect("the header, for github.com only");
        assert!(header.starts_with("AUTHORIZATION: basic "));
        assert_eq!(
            settings
                .keys()
                .filter(|k| k.to_ascii_lowercase().contains("extraheader"))
                .count(),
            1
        );
        assert_eq!(
            settings.get("credential.helper").map(String::as_str),
            Some("")
        );
        assert_eq!(
            settings.get("protocol.allow").map(String::as_str),
            Some("never")
        );
        assert_eq!(
            settings.get("protocol.https.allow").map(String::as_str),
            Some("always")
        );
        assert_eq!(
            settings.get("http.sslVerify").map(String::as_str),
            Some("true")
        );
        // Inherited settings that could redirect, trace or prompt are dropped.
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().to_string())
            .collect();
        for name in [
            "GIT_SSH_COMMAND",
            "GIT_TRACE_CURL",
            "GCM_TRACE",
            "GITHUB_TOKEN",
        ] {
            assert!(
                removed.iter().any(|r| r == name),
                "{name} dropped: {removed:?}"
            );
        }
        let envs: BTreeMap<String, Option<String>> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.map(|v| v.to_string_lossy().to_string()),
                )
            })
            .collect();
        assert_eq!(
            envs.get("GIT_CONFIG_NOSYSTEM"),
            Some(&Some("1".to_string()))
        );
        assert_eq!(
            envs.get("GIT_TERMINAL_PROMPT"),
            Some(&Some("0".to_string()))
        );
        assert_eq!(envs.get("GIT_ASKPASS"), Some(&Some(String::new())));
        // Without a token, and for a local endpoint, no header at all.
        let plain = network_command(
            dir.path(),
            &GitEndpoint::github(),
            Path::new("/h"),
            Path::new("/e"),
            None,
            Vec::new(),
            &["fetch"],
        );
        assert!(!settings_of(&plain)
            .keys()
            .any(|k| k.to_ascii_lowercase().contains("extraheader")));
        let local = network_command(
            dir.path(),
            &GitEndpoint::at("file:///srv/"),
            Path::new("/h"),
            Path::new("/e"),
            Some(&token()),
            Vec::new(),
            &["fetch"],
        );
        assert!(!settings_of(&local)
            .keys()
            .any(|k| k.to_ascii_lowercase().contains("extraheader")));
        assert_eq!(
            settings_of(&local)
                .get("protocol.file.allow")
                .map(String::as_str),
            Some("always")
        );
    }

    fn failed(stderr: &str) -> GitError {
        failure::<()>(&Output {
            ok: false,
            stdout: String::new(),
            stderr: stderr.to_string(),
        })
        .unwrap_err()
    }

    #[test]
    fn only_a_refused_sign_in_is_an_auth_failure_never_a_403() {
        for refused in [
            "fatal: Authentication failed for 'https://github.com/a/b.git/'",
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            "error: RPC failed; HTTP 401 curl 22 The requested URL returned error: 401",
        ] {
            assert_eq!(failed(refused), GitError::AuthFailed, "{refused}");
        }
        for forbidden in [
            "remote: Permission to octo-org/hello.git denied to ann.\nfatal: unable to access 'https://github.com/octo-org/hello.git/': The requested URL returned error: 403",
            "error: RPC failed; HTTP 403 curl 22",
        ] {
            let error = failed(forbidden);
            assert!(
                matches!(&error, GitError::Failed(text) if text.contains("may not write")),
                "{error:?}"
            );
        }
    }

    #[test]
    fn errors_never_show_the_mirrors_folder() {
        let dir = tempfile::tempdir().unwrap();
        let host = HostGit::new(dir.path());
        let repo = RepoRef::new("octo-org", "hello").unwrap();
        let mirror = host.mirror_dir(&repo);
        for path in [
            mirror.display().to_string(),
            mirror.display().to_string().replace('\\', "/"),
        ] {
            let error = host.hide(
                &repo,
                GitError::Failed(format!("fatal: '{path}' is broken")),
            );
            let text = error.to_string();
            assert!(!text.contains(&dir.path().display().to_string()), "{text}");
            assert!(text.contains("Shodh's copy of octo-org/hello"), "{text}");
        }
    }

    #[test]
    fn versions_and_logs_are_read() {
        assert_eq!(
            parse_version("git version 2.45.1.windows.1\n"),
            Some((2, 45))
        );
        assert_eq!(parse_version("git version 2.31.0"), Some((2, 31)));
        assert!(parse_version("git version 2.31.0").unwrap() < MIN_GIT);
        assert_eq!(parse_version("nonsense"), None);
        let log = "\u{1e}aaa\u{1f}Fix the login\n\n 2 files changed, 10 insertions(+), 3 deletions(-)\n\u{1e}bbb\u{1f}Add a test\n\n 1 file changed, 4 insertions(+)\n\u{1e}ccc ddd\u{1f}Merge branch 'x'\n\n 1 file changed, 1 insertion(+)\n";
        let commits = parse_commits(log);
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].words(), "“Fix the login” · 2 files, +10 −3");
        assert_eq!(commits[1].words(), "“Add a test” · 1 file, +4 −0");
        assert!(commits[2].merge);
        assert_eq!(
            commits[2].words(),
            "“Merge branch 'x'” · merge commit, 1 file, +1 −0"
        );
        // A title cannot pass for Shodh's own numbers: it is always quoted.
        let spoof = parse_commits(
            "\u{1e}aaa\u{1f}Fix typo (1 file, +1 −1)\n\n 9 files changed, 900 insertions(+)\n",
        );
        assert_eq!(
            spoof[0].words(),
            "“Fix typo (1 file, +1 −1)” · 9 files, +900 −0"
        );
        assert_eq!(
            rejection("!\tabc:refs/heads/shodh/x\t[rejected] (stale info)\n").unwrap(),
            "the branch moved on GitHub since you approved, so it was kept."
        );
        assert_eq!(
            failed("fatal: unable to update url base from redirection:\n  asked for: x"),
            GitError::Moved
        );
        assert_eq!(
            failed("error: RPC failed; HTTP 301 curl 22 The requested URL returned error: 301"),
            GitError::Moved
        );
        assert_eq!(
            rejection("To x\n!\tabc:refs/heads/shodh/x\t[rejected] (non-fast-forward)\nDone\n").unwrap(),
            "the branch on GitHub has commits this one does not. Fetch, merge them in, then push again."
        );
        assert_eq!(rejection("=\tabc:refs/heads/shodh/x\t[up to date]\n"), None);
    }

    #[tokio::test]
    async fn fetch_and_push_leave_the_projects_config_alone_and_write_the_sign_in_nowhere() {
        let w = world();
        w.host.check_version().await.unwrap();
        let config_before = std::fs::read(w.project.join(".git").join("config")).unwrap();

        // Someone else pushed to main.
        let other = w.project.parent().unwrap().join("other");
        let out = Command::new("git")
            .args(["clone", "-q"])
            .arg(&w.remote)
            .arg(&other)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success());
        commit(&other, "b.txt", "b\n", "Their change");
        git(&other, &["push", "-q", "origin", "main"]);

        let branches = w
            .host
            .fetch(&w.project, "origin", &w.repo, &token())
            .await
            .unwrap();
        assert_eq!(branches, 1);
        assert_eq!(
            git(
                &w.project,
                &["log", "-1", "--format=%s", "refs/remotes/origin/main"]
            ),
            "Their change"
        );
        // A fetch changes no file of the project and not its branch.
        assert!(!w.project.join("b.txt").exists());

        commit(
            &w.project,
            "src/login.rs",
            "fn login() {}\n",
            "Fix the login",
        );
        let plan = w
            .host
            .prepare_push(&w.project, &w.repo, "main", &token(), "call-1")
            .await
            .unwrap();
        assert_eq!(plan.total, 1);
        assert_eq!(plan.commits[0].title, "Fix the login");
        assert_eq!(plan.files, vec!["src/login.rs".to_string()]);
        assert_eq!(plan.local_branch.as_deref(), Some("main"));
        // HEAD moves after the approval: the approved commit is what is pushed.
        let approved = plan.sha.clone();
        commit(&w.project, "later.txt", "x\n", "Not approved");
        w.host
            .push(
                &w.project,
                "origin",
                &w.repo,
                "shodh/fix-login",
                &plan,
                &token(),
            )
            .await
            .unwrap();
        assert_eq!(
            git(&w.remote, &["rev-parse", "refs/heads/shodh/fix-login"]),
            approved
        );
        assert_eq!(
            git(&w.remote, &["log", "-1", "--format=%s", "main"]),
            "Their change",
            "main untouched"
        );
        assert_eq!(
            git(
                &w.project,
                &["rev-parse", "refs/remotes/origin/shodh/fix-login"]
            ),
            approved
        );

        // The project's config is byte for byte what it was; the token is in no file of
        // the project, the mirror or Shodh's data.
        assert_eq!(
            std::fs::read(w.project.join(".git").join("config")).unwrap(),
            config_before
        );
        assert_eq!(holds_token(&w.project), None);
        assert_eq!(holds_token(&w.data), None);
        assert_eq!(holds_token(&w.remote), None);
        // No outgoing copy is left behind.
        let mirror = w.host.mirror_dir(&w.repo);
        assert_eq!(git(&mirror, &["for-each-ref", OUTGOING]), "");
    }

    #[tokio::test]
    async fn a_branch_that_moved_on_github_rejects_the_push_never_overwritten() {
        let w = world();
        commit(&w.project, "a.txt", "a\n", "Mine");
        let plan = w
            .host
            .prepare_push(&w.project, &w.repo, "main", &token(), "call-1")
            .await
            .unwrap();
        w.host
            .push(&w.project, "origin", &w.repo, "shodh/x", &plan, &token())
            .await
            .unwrap();
        // Someone adds a commit to shodh/x on GitHub; the project rewrites its own.
        let other = w.project.parent().unwrap().join("other2");
        let out = Command::new("git")
            .args(["clone", "-q", "-b", "shodh/x"])
            .arg(&w.remote)
            .arg(&other)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success());
        commit(&other, "c.txt", "c\n", "Theirs on shodh/x");
        git(&other, &["push", "-q", "origin", "shodh/x"]);
        let theirs = git(&w.remote, &["rev-parse", "refs/heads/shodh/x"]);
        git(
            &w.project,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--amend",
                "-m",
                "Mine, rewritten",
            ],
        );
        let plan = w
            .host
            .prepare_push(&w.project, &w.repo, "main", &token(), "call-2")
            .await
            .unwrap();
        let error = w
            .host
            .push(&w.project, "origin", &w.repo, "shodh/x", &plan, &token())
            .await
            .unwrap_err();
        assert!(matches!(error, GitError::Rejected(_)), "{error:?}");
        assert_eq!(
            git(&w.remote, &["rev-parse", "refs/heads/shodh/x"]),
            theirs,
            "never overwritten"
        );
    }

    #[tokio::test]
    async fn only_shodh_branches_are_pushed() {
        let w = world();
        commit(&w.project, "a.txt", "a\n", "Mine");
        let plan = w
            .host
            .prepare_push(&w.project, &w.repo, "main", &token(), "call-1")
            .await
            .unwrap();
        for target in ["main", "refs/heads/main", "feature/x"] {
            assert!(
                w.host
                    .push(&w.project, "origin", &w.repo, target, &plan, &token())
                    .await
                    .is_err(),
                "{target}"
            );
        }
        assert_eq!(
            git(
                &w.remote,
                &["for-each-ref", "--format=%(refname)", "refs/heads"]
            ),
            "refs/heads/main"
        );
    }

    #[tokio::test]
    async fn remotes_are_read_from_the_project() {
        let w = world();
        git(
            &w.project,
            &[
                "remote",
                "set-url",
                "origin",
                "git@github.com:ann/hello.git",
            ],
        );
        git(
            &w.project,
            &[
                "remote",
                "add",
                "upstream",
                "https://github.com/octo-org/hello.git",
            ],
        );
        let remotes = project_remotes(&w.project).await.unwrap();
        let route = super::super::remote::route(&remotes, None).unwrap();
        assert!(route.fork);
        assert_eq!(route.push_repo.full_name(), "ann/hello");
        let bare = tempfile::tempdir().unwrap();
        git(bare.path(), &["init", "-q"]);
        assert_eq!(project_remotes(bare.path()).await.unwrap(), Vec::new());
    }

    fn git_input(dir: &Path, args: &[&str], input: &str) -> String {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A path as git config values and shell commands take it.
    fn slashed(path: &Path) -> String {
        path.display().to_string().replace('\\', "/")
    }

    /// What a sandboxed command could plant to make Shodh's own git, run in the project,
    /// start a program or send the project's commits elsewhere: every way a transport
    /// reads the repository's config for the mirror's path. None may take effect.
    #[tokio::test]
    async fn a_hostile_project_config_never_runs_anything_or_redirects_the_copy() {
        let w = world();
        let mirror = w.host.mirror_dir(&w.repo);
        let base = w.project.parent().unwrap().to_path_buf();
        let marker = base.join("ran.txt");
        let elsewhere = base.join("elsewhere.git");
        git(&base, &["init", "--bare", "-q", "elsewhere.git"]);
        let touch = format!("sh -c 'echo ran > \"{}\"'", slashed(&marker));
        for form in [mirror.display().to_string(), slashed(&mirror)] {
            // (a) the mirror's path rewritten to another repository;
            git(
                &w.project,
                &[
                    "config",
                    "--add",
                    &format!("url.{}.insteadOf", slashed(&elsewhere)),
                    &form,
                ],
            );
            // (b) a remote named like the mirror's path, with its own programs;
            git(
                &w.project,
                &["config", &format!("remote.{form}.uploadpack"), &touch],
            );
            git(
                &w.project,
                &["config", &format!("remote.{form}.receivepack"), &touch],
            );
        }
        // (c) the ext:: transport, allowed, behind a rewrite of the mirror's path;
        git(&w.project, &["config", "protocol.ext.allow", "always"]);
        git(
            &w.project,
            &[
                "config",
                "--add",
                &format!("url.ext::{touch} %S.insteadOf"),
                &mirror.display().to_string(),
            ],
        );
        // (d) an ssh command behind an ssh:// rewrite.
        git(&w.project, &["config", "core.sshCommand", &touch]);
        git(
            &w.project,
            &[
                "config",
                "--add",
                "url.ssh://x/.insteadOf",
                &slashed(&mirror),
            ],
        );
        let config_before = std::fs::read(w.project.join(".git").join("config")).unwrap();

        w.host
            .fetch(&w.project, "origin", &w.repo, &token())
            .await
            .unwrap();
        commit(&w.project, "a.txt", "a\n", "Mine");
        let plan = w
            .host
            .prepare_push(&w.project, &w.repo, "main", &token(), "call-1")
            .await
            .unwrap();
        w.host
            .push(&w.project, "origin", &w.repo, "shodh/x", &plan, &token())
            .await
            .unwrap();

        assert!(!marker.exists(), "a program from the project's config ran");
        assert_eq!(
            git(&elsewhere, &["for-each-ref"]),
            "",
            "the copy went to the rewritten repository"
        );
        assert_eq!(
            git(&w.remote, &["rev-parse", "refs/heads/shodh/x"]),
            plan.sha
        );
        assert_eq!(
            std::fs::read(w.project.join(".git").join("config")).unwrap(),
            config_before
        );
    }

    /// A crafted object from the project (here a commit without an author's email) is
    /// refused as it enters the mirror, before anything reaches GitHub.
    #[tokio::test]
    async fn objects_from_the_project_are_checked_as_they_enter_the_mirror() {
        let w = world();
        let tree = git_input(&w.project, &["mktree"], "");
        let parent = git(&w.project, &["rev-parse", "HEAD"]);
        let bad = git_input(
            &w.project,
            &["hash-object", "-t", "commit", "-w", "--literally", "--stdin"],
            &format!(
                "tree {tree}\nparent {parent}\nauthor nobody 1 +0000\ncommitter nobody 1 +0000\n\nBroken\n"
            ),
        );
        git(&w.project, &["update-ref", "refs/heads/main", &bad]);
        let error = w
            .host
            .prepare_push(&w.project, &w.repo, "main", &token(), "call-1")
            .await
            .unwrap_err();
        assert!(matches!(error, GitError::Failed(_)), "{error:?}");
        assert_eq!(
            git(
                &w.remote,
                &["for-each-ref", "--format=%(refname)", "refs/heads"]
            ),
            "refs/heads/main"
        );
    }

    /// A merge commit's own changes (an "evil merge") are listed: its numbers are against
    /// its first parent and its files against every parent.
    #[tokio::test]
    async fn a_merge_commits_own_changes_are_listed() {
        let w = world();
        git(&w.project, &["checkout", "-q", "-b", "side"]);
        commit(&w.project, "side.txt", "side\n", "Side work");
        git(&w.project, &["checkout", "-q", "main"]);
        commit(&w.project, "main.txt", "main\n", "Main work");
        git(
            &w.project,
            &[
                "-c",
                "commit.gpgsign=false",
                "merge",
                "-q",
                "--no-ff",
                "--no-commit",
                "side",
            ],
        );
        std::fs::write(
            w.project.join("package.json"),
            "{\"scripts\":{\"test\":\"curl evil\"}}\n",
        )
        .unwrap();
        git(&w.project, &["add", "-A"]);
        git(
            &w.project,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "Merge side",
            ],
        );
        let plan = w
            .host
            .prepare_push(&w.project, &w.repo, "main", &token(), "call-1")
            .await
            .unwrap();
        assert_eq!(plan.total, 3);
        let merge = plan
            .commits
            .iter()
            .find(|c| c.merge)
            .expect("the merge is listed");
        assert!(merge.files >= 2, "{merge:?}");
        for file in ["package.json", "side.txt", "main.txt"] {
            assert!(
                plan.files.iter().any(|f| f == file),
                "{file}: {:?}",
                plan.files
            );
        }
    }

    #[tokio::test]
    async fn workflows_that_run_on_new_branches_are_found() {
        let w = world();
        let seed = w.project.parent().unwrap().join("seed");
        std::fs::create_dir_all(seed.join(".github").join("workflows")).unwrap();
        std::fs::write(
            seed.join(".github/workflows/ci.yml"),
            "on:\n  push:\n  pull_request:\njobs: {}\n",
        )
        .unwrap();
        std::fs::write(
            seed.join(".github/workflows/release.yml"),
            "on:\n  push:\n    tags: ['v*']\njobs: {}\n",
        )
        .unwrap();
        git(&seed, &["add", "-A"]);
        git(
            &seed,
            &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "CI"],
        );
        git(
            &seed,
            &["push", "-q", w.remote.to_str().unwrap(), "main:main"],
        );
        commit(&w.project, "a.txt", "a\n", "Mine");
        let plan = w
            .host
            .prepare_push(&w.project, &w.repo, "main", &token(), "call-1")
            .await
            .unwrap();
        assert_eq!(
            plan.push_workflows,
            vec![".github/workflows/ci.yml".to_string()]
        );
    }

    #[test]
    fn push_triggers_are_read_conservatively() {
        for runs in [
            "on: push\n",
            "on: [push, pull_request]\n",
            "on:\n  - push\n",
            "on:\n  push:\n",
            "on:\n  push: {}\n",
            "on:\n  push:\n    branches: ['**']\n",
            "on:\n  push:\n    branches:\n      - 'shodh/**'\n",
            "on:\n  push:\n    branches-ignore: [main]\n",
            "on:\n  push:\n    paths: ['src/**']\n",
            "\"on\":\n  push:\n",
        ] {
            assert!(runs_on_push(runs), "{runs}");
        }
        for quiet in [
            "on: pull_request\n",
            "on:\n  push:\n    branches: [main]\n",
            "on:\n  push:\n    branches:\n      - main\n      - 'release/*'\n",
            "on:\n  push:\n    tags: ['v*']\n",
            "on:\n  workflow_dispatch:\n",
            "name: push\non: schedule\n",
        ] {
            assert!(!runs_on_push(quiet), "{quiet}");
        }
    }

    #[test]
    fn the_token_is_scrubbed_raw_and_as_the_header_carries_it() {
        let text = format!("x {TOKEN} y {}", encoded(TOKEN));
        let clean = scrub_all(&text, TOKEN);
        assert!(
            !clean.contains(TOKEN) && !clean.contains(&encoded(TOKEN)),
            "{clean}"
        );
    }

    /// A branch is deleted only while it still points at the commit the approval named.
    #[tokio::test]
    async fn a_branch_is_deleted_only_while_it_points_where_it_did() {
        let w = world();
        commit(&w.project, "a.txt", "a\n", "Mine");
        let plan = w
            .host
            .prepare_push(&w.project, &w.repo, "main", &token(), "call-1")
            .await
            .unwrap();
        w.host
            .push(&w.project, "origin", &w.repo, "shodh/x", &plan, &token())
            .await
            .unwrap();
        let main = git(&w.remote, &["rev-parse", "refs/heads/main"]);
        let stale = w
            .host
            .delete_branch(&w.repo, "shodh/x", &main, &token())
            .await
            .unwrap_err();
        assert!(matches!(stale, GitError::Rejected(_)), "{stale:?}");
        assert_eq!(
            git(&w.remote, &["rev-parse", "refs/heads/shodh/x"]),
            plan.sha
        );
        assert!(w
            .host
            .delete_branch(&w.repo, "main", &main, &token())
            .await
            .is_err());
        w.host
            .delete_branch(&w.repo, "shodh/x", &plan.sha, &token())
            .await
            .unwrap();
        assert_eq!(
            git(
                &w.remote,
                &["for-each-ref", "--format=%(refname)", "refs/heads"]
            ),
            "refs/heads/main"
        );
    }

    #[test]
    fn a_pushs_size_counts_every_line_git_reports() {
        assert_eq!(
            shortstat_lines(" 3 files changed, 10 insertions(+), 2 deletions(-)\n"),
            12
        );
        assert_eq!(shortstat_lines(" 1 file changed, 1 insertion(+)"), 1);
        assert_eq!(shortstat_lines(" 1 file changed, 4 deletions(-)"), 4);
        assert_eq!(shortstat_lines(""), 0);
    }

    #[tokio::test]
    async fn a_capped_read_stops_past_its_limit() {
        let mut big = Command::new("git");
        big.args(["--version"]);
        assert!(
            run_capped(big, LOCAL_TIMEOUT, 4).await.unwrap().is_none(),
            "more than 4 bytes"
        );
        let mut small = Command::new("git");
        small.args(["--version"]);
        let text = run_capped(small, LOCAL_TIMEOUT, 4096)
            .await
            .unwrap()
            .unwrap();
        assert!(text.starts_with("git version"), "{text}");
    }
}
