//! git over the network, done by Shodh on this computer (never by a sandboxed command),
//! with the user's GitHub sign-in that never touches the project.
//!
//! **Where the network git runs.** Not in the project: its `.git/config` can be changed
//! by any command (`url.*.insteadOf`, `http.*.proxy`, `http.sslVerify=false`,
//! `include.path`, a credential helper), and a git that sends the sign-in while reading
//! that file could send it elsewhere. Every fetch and push to GitHub runs in a bare
//! **mirror** Shodh owns (`<data>/connectors/github/mirrors/<owner>/<repo>.git`), whose
//! config Shodh writes afresh before each use, with the system and global git config
//! switched off. Objects move between the mirror and the project only by local git
//! (`fetch`/`push` between two folders on this computer), which needs no sign-in.
//!
//! **How the sign-in is given.** As an `Authorization` header scoped to
//! `https://github.com/`, through git's `GIT_CONFIG_COUNT` environment of that one git
//! process: never on its command line (other programs can list command lines), never in
//! a file, never in the project's or the mirror's config, never in a command's
//! environment. Credential helpers, askpass programs and prompts are off, inherited
//! `GIT_*` settings are dropped, hooks come from an empty folder, and only HTTPS may be
//! used.
//!
//! **What is pushed.** Exactly the commit the user approved
//! (`<sha>:refs/heads/shodh/<topic>`), never `HEAD`, never with `+` or `--force`; a
//! branch that moved on GitHub rejects the push instead of being overwritten.
//!
//! Requires git 2.32 or newer (`GIT_CONFIG_COUNT`, `GIT_CONFIG_GLOBAL`).

use std::collections::BTreeMap;
use std::ffi::OsString;
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
}

impl CommitLine {
    /// "Fix the login (2 files, +10 −3)".
    pub fn words(&self) -> String {
        let files = if self.files == 1 {
            "1 file".to_string()
        } else {
            format!("{} files", self.files)
        };
        format!(
            "{} ({files}, +{} −{})",
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
    /// Every file those commits change.
    pub files: Vec<String>,
    /// The mirror ref holding the commit until it is pushed.
    pub outgoing: String,
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

/// The `Authorization` header value git sends (`basic`, user `x-access-token`).
fn auth_header(token: &UserToken) -> String {
    let pair = format!("x-access-token:{}", token.expose());
    format!(
        "AUTHORIZATION: basic {}",
        base64::engine::general_purpose::STANDARD.encode(pair)
    )
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
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The mirror's whole config, written before every use.
const MIRROR_CONFIG: &str = "[core]\n\trepositoryformatversion = 0\n\tbare = true\n\tlogallrefupdates = false\n[gc]\n\tauto = 0\n";

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
        stdout: scrub(&String::from_utf8_lossy(&output.stdout), secret),
        stderr: scrub(&String::from_utf8_lossy(&output.stderr), secret),
    })
}

async fn run(command: Command, timeout: Duration, secret: &str) -> Result<String, GitError> {
    let output = run_full(command, timeout, secret).await?;
    if output.ok {
        return Ok(output.stdout);
    }
    failure(&output)
}

/// The error of a failed git command.
fn failure<T>(output: &Output) -> Result<T, GitError> {
    let stdout = output.stdout.as_str();
    let stderr = output.stderr.trim();
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("authentication failed")
        || lower.contains("could not read username")
        || lower.contains("invalid username or password")
        || lower.contains("http 401")
        || lower.contains("error: 403")
        || (lower.contains("permission to ") && lower.contains("denied"))
    {
        return Err(GitError::AuthFailed);
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
/// system monitor), without the project's ability to reach the network mattering: it
/// only talks to the mirror folder.
fn project_git(project: &Path, args: &[&str]) -> Result<Command, GitError> {
    let mut command = crate::harness::code_mode::git(project).map_err(GitError::Failed)?;
    command.args(args);
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

/// Parses `git log --format=%x1e%s --shortstat` output.
fn parse_commits(text: &str) -> Vec<CommitLine> {
    text.split('\u{1e}')
        .filter(|record| !record.trim().is_empty())
        .map(|record| {
            let mut lines = record.lines();
            let title = lines.next().unwrap_or_default().trim().to_string();
            let mut line = CommitLine {
                title: if title.is_empty() {
                    "(no title)".to_string()
                } else {
                    title.chars().take(200).collect()
                },
                files: 0,
                insertions: 0,
                deletions: 0,
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
        Some(if summary.contains("non-fast-forward") || summary.contains("fetch first") {
            "the branch on GitHub has commits this one does not. Fetch, merge them in, then push again.".to_string()
        } else {
            summary.chars().take(200).collect()
        })
    })
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
        std::fs::write(dir.join("config"), MIRROR_CONFIG).map_err(|e| {
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
        if !super::remote::valid_remote_name(remote) {
            return Err(GitError::Failed(format!(
                "“{remote}” is not a remote name Shodh can use"
            )));
        }
        let mirror = self.update_mirror(repo, token).await?;
        let mirror_path = mirror.display().to_string();
        let spec = format!("+refs/heads/*:refs/remotes/{remote}/*");
        let local = project_git(
            project,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "--no-recurse-submodules",
                "--no-write-fetch-head",
                &mirror_path,
                &spec,
            ],
        )?;
        run(local, LOCAL_TIMEOUT, "").await?;
        let list = self.network(
            &mirror,
            None,
            &["for-each-ref", "--format=%(refname)", "refs/heads"],
        )?;
        Ok(run(list, LOCAL_TIMEOUT, "")
            .await?
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count())
    }

    /// Prepares a push of the project's current commit to `repo`: brings GitHub's
    /// branches into the mirror, copies the commit there, and lists what GitHub does not
    /// have yet. Nothing is sent to GitHub.
    pub async fn prepare_push(
        &self,
        project: &Path,
        repo: &RepoRef,
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
        let mirror_path = mirror.display().to_string();
        let copy = project_git(
            project,
            &[
                "push",
                "--quiet",
                "--no-verify",
                "--no-recurse-submodules",
                &mirror_path,
                &format!("{sha}:{outgoing}"),
            ],
        )?;
        run(copy, LOCAL_TIMEOUT, "").await?;

        let log = self.network(
            &mirror,
            None,
            &[
                "log",
                "--no-color",
                "--format=%x1e%s",
                "--shortstat",
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
        let names = self.network(
            &mirror,
            None,
            &[
                "log",
                "--no-color",
                "--format=",
                "--name-only",
                "--no-renames",
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
        Ok(PushPlan {
            sha,
            local_branch,
            commits,
            total,
            files,
            outgoing,
        })
    }

    /// Pushes exactly `plan.sha` to `refs/heads/<branch>` of `repo` (no force), and
    /// records it in the project's `refs/remotes/<remote>/<branch>`.
    pub async fn push(
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

/// The project's remotes (`git config --get-regexp`), hardened like Code mode's git.
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

    #[test]
    fn versions_and_logs_are_read() {
        assert_eq!(
            parse_version("git version 2.45.1.windows.1\n"),
            Some((2, 45))
        );
        assert_eq!(parse_version("git version 2.31.0"), Some((2, 31)));
        assert!(parse_version("git version 2.31.0").unwrap() < MIN_GIT);
        assert_eq!(parse_version("nonsense"), None);
        let log = "\u{1e}Fix the login\n\n 2 files changed, 10 insertions(+), 3 deletions(-)\n\u{1e}Add a test\n\n 1 file changed, 4 insertions(+)\n\u{1e}Merge branch 'x'\n";
        let commits = parse_commits(log);
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].words(), "Fix the login (2 files, +10 −3)");
        assert_eq!(commits[1].words(), "Add a test (1 file, +4 −0)");
        assert_eq!(commits[2].files, 0);
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
            .prepare_push(&w.project, &w.repo, &token(), "call-1")
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
            .prepare_push(&w.project, &w.repo, &token(), "call-1")
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
            .prepare_push(&w.project, &w.repo, &token(), "call-2")
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
            .prepare_push(&w.project, &w.repo, &token(), "call-1")
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
}
