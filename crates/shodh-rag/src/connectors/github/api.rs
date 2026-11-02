//! The GitHub REST calls the connector makes (<https://docs.github.com/en/rest>), with the
//! user's token of the Shodh GitHub App.
//!
//! What the app's permissions (Contents RW, Pull requests RW, Issues RW, Metadata R)
//! allow, and so all the connector calls: the user and the app's installations and
//! their repositories; a repository, a branch (`protected`); issues, pull requests,
//! their comments and files; creating a pull request or a comment; merging a pull
//! request; deleting a branch. The app has no Administration, Checks or Commit statuses
//! permission, so branch-protection settings, check runs and statuses are not read: a
//! pull request's own `mergeable_state` (GitHub's verdict on its required checks and
//! reviews) is what merging is decided on ([`super::limits`]).
//!
//! The token goes in the `Authorization` header only. Errors never carry a URL with
//! query parameters or the token.

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use reqwest::Method;
use serde_json::{json, Value};

use super::remote::RepoRef;
use super::token::{scrub, UserToken};

/// GitHub's REST API.
pub const GITHUB_API: &str = "https://api.github.com/";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Most repositories read from one installation (100 per page).
const MAX_REPO_PAGES: usize = 20;

/// Why a GitHub call failed. No variant holds the token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("GitHub no longer accepts Shodh's sign-in (it was revoked or expired). Sign in again under Connectors → GitHub.")]
    Unauthorized,
    #[error("GitHub is rate limiting Shodh{}", until(.reset_at))]
    RateLimited { reset_at: Option<DateTime<Utc>> },
    #[error("GitHub refused this: {0}")]
    Forbidden(String),
    #[error("GitHub has no such {0}, or the Shodh app is not installed on it")]
    NotFound(String),
    #[error("GitHub refused this ({status}): {message}")]
    Refused { status: u16, message: String },
    #[error("GitHub could not be reached: {0}")]
    Network(String),
    #[error("GitHub's answer could not be read: {0}")]
    BadResponse(String),
}

/// " until 14:05 UTC" for a known reset time.
fn until(reset_at: &Option<DateTime<Utc>>) -> String {
    reset_at
        .map(|at| format!(" until {}", at.format("%H:%M UTC")))
        .unwrap_or_default()
}

/// The signed-in user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubUser {
    pub id: u64,
    pub login: String,
    pub name: Option<String>,
}

/// A repository the app can reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub id: u64,
    pub repo: RepoRef,
    pub private: bool,
    pub default_branch: String,
    /// Whether the user may push (`permissions.push`), when GitHub says.
    pub can_push: Option<bool>,
    pub allow_merge_commit: Option<bool>,
    pub allow_squash_merge: Option<bool>,
    pub allow_rebase_merge: Option<bool>,
    /// The repository this one is a fork of.
    pub parent: Option<RepoRef>,
}

/// A branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub sha: String,
    pub protected: bool,
}

/// An issue or a pull request's conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub author: String,
    pub body: String,
    pub url: String,
    pub is_pull_request: bool,
    pub comments: u64,
}

/// A comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// A pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub draft: bool,
    pub merged: bool,
    pub author: String,
    pub url: String,
    /// The head's repository and branch.
    pub head_repo: Option<RepoRef>,
    pub head_ref: String,
    pub head_sha: String,
    pub base_ref: String,
    /// `clean`, `blocked`, `behind`, `dirty`, `unstable`, `has_hooks`, `draft`,
    /// `unknown` (GitHub is still working it out).
    pub mergeable_state: String,
    pub mergeable: Option<bool>,
    pub commits: u64,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
}

/// One commit of a comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitTitle {
    pub title: String,
}

/// What a branch would bring into another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub ahead_by: u64,
    pub commits: Vec<CommitTitle>,
    pub files: u64,
    pub additions: u64,
    pub deletions: u64,
}

/// The REST client.
#[derive(Clone)]
pub struct GitHubApi {
    client: reqwest::Client,
    base: String,
}

impl std::fmt::Debug for GitHubApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubApi")
            .field("base", &self.base)
            .finish()
    }
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn login_of(value: &Value) -> String {
    value
        .get("user")
        .map(|u| text(u, "login"))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "someone".to_string())
}

fn repo_of(value: &Value) -> Option<RepoRef> {
    let owner = value.get("owner").map(|o| text(o, "login"))?;
    RepoRef::new(&owner, &text(value, "name")).ok()
}

/// Percent-encodes one path segment (a branch name keeps its `/`: GitHub accepts it raw).
fn segment(raw: &str, keep_slash: bool) -> String {
    let mut out = String::new();
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' | b':' if keep_slash => out.push(b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

impl GitHubApi {
    pub fn new() -> Result<Self, ApiError> {
        Self::with_base(GITHUB_API)
    }

    /// A client for the API at `base` (tests use a local server).
    pub fn with_base(base: &str) -> Result<Self, ApiError> {
        let mut base = base.to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("Shodh/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ApiError::Network(e.without_url().to_string()))?;
        Ok(Self { client, base })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    async fn call(
        &self,
        token: &UserToken,
        method: Method,
        path: &str,
        body: Option<Value>,
        what: &str,
    ) -> Result<Option<Value>, ApiError> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token.expose())
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let secret = token.expose();
        let response = request
            .send()
            .await
            .map_err(|e| ApiError::Network(scrub(&e.without_url().to_string(), secret)))?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let raw = response
            .text()
            .await
            .map_err(|e| ApiError::Network(scrub(&e.without_url().to_string(), secret)))?;
        let value: Option<Value> = if raw.trim().is_empty() {
            None
        } else {
            serde_json::from_str(&raw).ok()
        };
        if (200..300).contains(&status) {
            if status != 204 && value.is_none() && !raw.trim().is_empty() {
                return Err(ApiError::BadResponse(format!("HTTP {status}, not JSON")));
            }
            return Ok(value);
        }
        let message = value
            .as_ref()
            .map(|v| text(v, "message"))
            .filter(|m| !m.is_empty())
            .map(|m| scrub(&m, secret).chars().take(300).collect::<String>())
            .unwrap_or_else(|| format!("HTTP {status}"));
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let reset_at = header("x-ratelimit-reset")
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|s| Utc.timestamp_opt(s, 0).single());
        let rate_limited = status == 429
            || (status == 403
                && (header("x-ratelimit-remaining").as_deref() == Some("0")
                    || header("retry-after").is_some()
                    || message.to_ascii_lowercase().contains("rate limit")));
        Err(match status {
            401 => ApiError::Unauthorized,
            _ if rate_limited => ApiError::RateLimited { reset_at },
            403 => ApiError::Forbidden(message),
            404 => ApiError::NotFound(what.to_string()),
            _ if status >= 500 => ApiError::Network(format!("GitHub answered HTTP {status}")),
            _ => ApiError::Refused { status, message },
        })
    }

    async fn get(&self, token: &UserToken, path: &str, what: &str) -> Result<Value, ApiError> {
        self.call(token, Method::GET, path, None, what)
            .await?
            .ok_or_else(|| ApiError::BadResponse("an empty answer".into()))
    }

    /// Who is signed in.
    pub async fn user(&self, token: &UserToken) -> Result<GitHubUser, ApiError> {
        let v = self.get(token, "user", "user").await?;
        let id = v
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| ApiError::BadResponse("no user id".into()))?;
        let login = text(&v, "login");
        if login.is_empty() {
            return Err(ApiError::BadResponse("no login".into()));
        }
        Ok(GitHubUser {
            id,
            login,
            name: v
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|n| !n.is_empty()),
        })
    }

    /// Every repository the app is installed on that the user can reach (all their
    /// installations, page by page).
    pub async fn installation_repos(&self, token: &UserToken) -> Result<Vec<RepoInfo>, ApiError> {
        let installations = self
            .get(token, "user/installations?per_page=100", "installation")
            .await?;
        let ids: Vec<u64> = installations
            .get("installations")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|i| i.get("id").and_then(Value::as_u64))
                    .collect()
            })
            .unwrap_or_default();
        let mut out: Vec<RepoInfo> = Vec::new();
        for id in ids {
            for page in 1..=MAX_REPO_PAGES {
                let v = self
                    .get(
                        token,
                        &format!("user/installations/{id}/repositories?per_page=100&page={page}"),
                        "installation",
                    )
                    .await?;
                let list = v
                    .get("repositories")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let count = list.len();
                for repo in list.iter().filter_map(repo_info) {
                    if !out.iter().any(|r| r.repo.same(&repo.repo)) {
                        out.push(repo);
                    }
                }
                if count < 100 {
                    break;
                }
            }
        }
        out.sort_by(|a, b| {
            a.repo
                .full_name()
                .to_ascii_lowercase()
                .cmp(&b.repo.full_name().to_ascii_lowercase())
        });
        Ok(out)
    }

    /// A repository.
    pub async fn repo(&self, token: &UserToken, repo: &RepoRef) -> Result<RepoInfo, ApiError> {
        let v = self
            .get(
                token,
                &format!("repos/{}/{}", repo.owner, repo.name),
                "repository",
            )
            .await?;
        repo_info(&v).ok_or_else(|| ApiError::BadResponse("no repository".into()))
    }

    /// A branch, or `None` when it does not exist.
    pub async fn branch(
        &self,
        token: &UserToken,
        repo: &RepoRef,
        branch: &str,
    ) -> Result<Option<BranchInfo>, ApiError> {
        let path = format!(
            "repos/{}/{}/branches/{}",
            repo.owner,
            repo.name,
            segment(branch, true)
        );
        match self.get(token, &path, "branch").await {
            Ok(v) => Ok(Some(BranchInfo {
                name: text(&v, "name"),
                sha: v.get("commit").map(|c| text(c, "sha")).unwrap_or_default(),
                protected: v.get("protected").and_then(Value::as_bool).unwrap_or(true),
            })),
            Err(ApiError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// An issue (or a pull request's conversation).
    pub async fn issue(
        &self,
        token: &UserToken,
        repo: &RepoRef,
        n: u64,
    ) -> Result<Issue, ApiError> {
        let v = self
            .get(
                token,
                &format!("repos/{}/{}/issues/{n}", repo.owner, repo.name),
                "issue",
            )
            .await?;
        Ok(Issue {
            number: number(&v, "number"),
            title: text(&v, "title"),
            state: text(&v, "state"),
            author: login_of(&v),
            body: text(&v, "body"),
            url: text(&v, "html_url"),
            is_pull_request: v.get("pull_request").is_some(),
            comments: number(&v, "comments"),
        })
    }

    /// The latest `limit` comments of an issue or pull request, oldest first.
    pub async fn comments(
        &self,
        token: &UserToken,
        repo: &RepoRef,
        n: u64,
        total: u64,
        limit: usize,
    ) -> Result<Vec<Comment>, ApiError> {
        if total == 0 || limit == 0 {
            return Ok(Vec::new());
        }
        let per_page = 100u64;
        let last_page = total.div_ceil(per_page).max(1);
        let v = self
            .get(
                token,
                &format!(
                    "repos/{}/{}/issues/{n}/comments?per_page={per_page}&page={last_page}",
                    repo.owner, repo.name
                ),
                "issue",
            )
            .await?;
        let list = v.as_array().cloned().unwrap_or_default();
        let skip = list.len().saturating_sub(limit);
        Ok(list
            .iter()
            .skip(skip)
            .map(|c| Comment {
                author: login_of(c),
                body: text(c, "body"),
                created_at: text(c, "created_at"),
            })
            .collect())
    }

    /// A pull request.
    pub async fn pull(
        &self,
        token: &UserToken,
        repo: &RepoRef,
        n: u64,
    ) -> Result<PullRequest, ApiError> {
        let v = self
            .get(
                token,
                &format!("repos/{}/{}/pulls/{n}", repo.owner, repo.name),
                "pull request",
            )
            .await?;
        Ok(pull_of(&v))
    }

    /// Pull requests whose head is `head_owner:branch`, in any state, newest first.
    pub async fn pulls_from(
        &self,
        token: &UserToken,
        repo: &RepoRef,
        head_owner: &str,
        branch: &str,
    ) -> Result<Vec<PullRequest>, ApiError> {
        let v = self
            .get(
                token,
                &format!(
                    "repos/{}/{}/pulls?state=all&per_page=30&head={}",
                    repo.owner,
                    repo.name,
                    segment(&format!("{head_owner}:{branch}"), false)
                ),
                "repository",
            )
            .await?;
        Ok(v.as_array()
            .map(|l| l.iter().map(pull_of).collect())
            .unwrap_or_default())
    }

    /// What `head` (`branch` or `owner:branch`) brings into `base`.
    pub async fn compare(
        &self,
        token: &UserToken,
        repo: &RepoRef,
        base: &str,
        head: &str,
    ) -> Result<Comparison, ApiError> {
        let v = self
            .get(
                token,
                &format!(
                    "repos/{}/{}/compare/{}...{}?per_page=100",
                    repo.owner,
                    repo.name,
                    segment(base, true),
                    segment(head, true)
                ),
                "branch",
            )
            .await?;
        let files = v
            .get("files")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(Comparison {
            ahead_by: number(&v, "ahead_by"),
            commits: v
                .get("commits")
                .and_then(Value::as_array)
                .map(|l| {
                    l.iter()
                        .map(|c| CommitTitle {
                            title: c
                                .get("commit")
                                .map(|c| text(c, "message"))
                                .unwrap_or_default()
                                .lines()
                                .next()
                                .unwrap_or_default()
                                .to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            files: files.len() as u64,
            additions: files.iter().map(|f| number(f, "additions")).sum(),
            deletions: files.iter().map(|f| number(f, "deletions")).sum(),
        })
    }

    /// Opens a pull request; returns it.
    pub async fn create_pull(
        &self,
        token: &UserToken,
        repo: &RepoRef,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<PullRequest, ApiError> {
        let v = self
            .call(
                token,
                Method::POST,
                &format!("repos/{}/{}/pulls", repo.owner, repo.name),
                Some(json!({ "title": title, "body": body, "head": head, "base": base, "maintainer_can_modify": true })),
                "repository",
            )
            .await?
            .ok_or_else(|| ApiError::BadResponse("an empty answer".into()))?;
        Ok(pull_of(&v))
    }

    /// Posts `body` as a comment on issue or pull request `n`; returns its page.
    pub async fn comment(
        &self,
        token: &UserToken,
        repo: &RepoRef,
        n: u64,
        body: &str,
    ) -> Result<String, ApiError> {
        let v = self
            .call(
                token,
                Method::POST,
                &format!("repos/{}/{}/issues/{n}/comments", repo.owner, repo.name),
                Some(json!({ "body": body })),
                "issue",
            )
            .await?
            .ok_or_else(|| ApiError::BadResponse("an empty answer".into()))?;
        Ok(text(&v, "html_url"))
    }

    /// Merges pull request `n` if its head is still `sha`, with `method`
    /// (`merge`, `squash` or `rebase`).
    pub async fn merge(
        &self,
        token: &UserToken,
        repo: &RepoRef,
        n: u64,
        sha: &str,
        method: &str,
    ) -> Result<(), ApiError> {
        self.call(
            token,
            Method::PUT,
            &format!("repos/{}/{}/pulls/{n}/merge", repo.owner, repo.name),
            Some(json!({ "sha": sha, "merge_method": method })),
            "pull request",
        )
        .await
        .map(|_| ())
    }

    /// Deletes branch `branch`.
    pub async fn delete_branch(
        &self,
        token: &UserToken,
        repo: &RepoRef,
        branch: &str,
    ) -> Result<(), ApiError> {
        self.call(
            token,
            Method::DELETE,
            &format!(
                "repos/{}/{}/git/refs/heads/{}",
                repo.owner,
                repo.name,
                segment(branch, true)
            ),
            None,
            "branch",
        )
        .await
        .map(|_| ())
    }
}

fn repo_info(v: &Value) -> Option<RepoInfo> {
    let repo = repo_of(v)?;
    let flag = |key: &str| v.get(key).and_then(Value::as_bool);
    Some(RepoInfo {
        id: number(v, "id"),
        repo,
        private: flag("private").unwrap_or(true),
        default_branch: text(v, "default_branch"),
        can_push: v
            .get("permissions")
            .and_then(|p| p.get("push"))
            .and_then(Value::as_bool),
        allow_merge_commit: flag("allow_merge_commit"),
        allow_squash_merge: flag("allow_squash_merge"),
        allow_rebase_merge: flag("allow_rebase_merge"),
        parent: v.get("parent").and_then(repo_of),
    })
}

fn pull_of(v: &Value) -> PullRequest {
    let head = v.get("head").cloned().unwrap_or(Value::Null);
    let base = v.get("base").cloned().unwrap_or(Value::Null);
    PullRequest {
        number: number(v, "number"),
        title: text(v, "title"),
        body: text(v, "body"),
        state: text(v, "state"),
        draft: v.get("draft").and_then(Value::as_bool).unwrap_or(false),
        merged: v.get("merged").and_then(Value::as_bool).unwrap_or(false)
            || v.get("merged_at").is_some_and(|m| !m.is_null()),
        author: login_of(v),
        url: text(v, "html_url"),
        head_repo: head.get("repo").and_then(repo_of),
        head_ref: text(&head, "ref"),
        head_sha: text(&head, "sha"),
        base_ref: text(&base, "ref"),
        mergeable_state: v
            .get("mergeable_state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        mergeable: v.get("mergeable").and_then(Value::as_bool),
        commits: number(v, "commits"),
        additions: number(v, "additions"),
        deletions: number(v, "deletions"),
        changed_files: number(v, "changed_files"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::fake::{serve, Reply};
    use super::super::token::tests::TOKEN;
    use super::*;

    pub fn token() -> UserToken {
        UserToken::parse(TOKEN).unwrap()
    }

    pub fn repo_json(owner: &str, name: &str, default_branch: &str) -> Value {
        json!({
            "id": 1296269,
            "name": name,
            "owner": {"login": owner},
            "private": false,
            "default_branch": default_branch,
            "permissions": {"push": true},
            "allow_merge_commit": true,
            "allow_squash_merge": true,
            "allow_rebase_merge": false
        })
    }

    #[tokio::test]
    async fn calls_send_the_token_only_in_the_header() {
        let (base, log) = serve(vec![Reply::json(
            200,
            json!({"id": 1, "login": "ann", "name": "Ann"}),
        )])
        .await;
        let api = GitHubApi::with_base(&base).unwrap();
        let user = api.user(&token()).await.unwrap();
        assert_eq!(user.login, "ann");
        let seen = log.lock().unwrap();
        assert_eq!(seen[0].path, "/user");
        assert!(
            seen[0]
                .head
                .contains(&format!("authorization: Bearer {TOKEN}"))
                || seen[0]
                    .head
                    .contains(&format!("Authorization: Bearer {TOKEN}"))
        );
        assert!(!seen[0].path.contains("ghu_"));
        assert!(
            seen[0].head.contains("X-GitHub-Api-Version")
                || seen[0].head.contains("x-github-api-version")
        );
    }

    #[tokio::test]
    async fn errors_are_named_and_never_quote_the_token() {
        let (base, _) = serve(vec![
            Reply::json(401, json!({"message": "Bad credentials"})),
            Reply::json(403, json!({"message": "API rate limit exceeded"}))
                .header("x-ratelimit-remaining", "0")
                .header("x-ratelimit-reset", "1798761600"),
            Reply::json(
                403,
                json!({"message": format!("Resource not accessible by integration {TOKEN}")}),
            ),
            Reply::json(404, json!({"message": "Not Found"})),
            Reply::json(422, json!({"message": "Validation Failed"})),
            Reply::json(502, json!({"message": "Bad gateway"})),
        ])
        .await;
        let api = GitHubApi::with_base(&base).unwrap();
        let t = token();
        assert_eq!(api.user(&t).await.unwrap_err(), ApiError::Unauthorized);
        assert!(matches!(
            api.user(&t).await.unwrap_err(),
            ApiError::RateLimited { reset_at: Some(_) }
        ));
        let forbidden = api.user(&t).await.unwrap_err();
        assert!(matches!(forbidden, ApiError::Forbidden(_)));
        assert!(!forbidden.to_string().contains("ghu_"), "{forbidden}");
        assert!(matches!(
            api.user(&t).await.unwrap_err(),
            ApiError::NotFound(_)
        ));
        assert!(matches!(
            api.user(&t).await.unwrap_err(),
            ApiError::Refused { status: 422, .. }
        ));
        assert!(matches!(
            api.user(&t).await.unwrap_err(),
            ApiError::Network(_)
        ));
    }

    #[tokio::test]
    async fn installation_repositories_are_read_from_every_installation() {
        let (base, log) = serve(vec![
            Reply::json(200, json!({"total_count": 2, "installations": [{"id": 7}, {"id": 8}]})),
            Reply::json(200, json!({"repositories": [repo_json("ann", "notes", "main")]})),
            Reply::json(200, json!({"repositories": [repo_json("octo-org", "Hello", "trunk"), repo_json("ann", "notes", "main")]})),
        ])
        .await;
        let api = GitHubApi::with_base(&base).unwrap();
        let repos = api.installation_repos(&token()).await.unwrap();
        let names: Vec<String> = repos.iter().map(|r| r.repo.full_name()).collect();
        assert_eq!(names, vec!["ann/notes", "octo-org/Hello"]);
        assert_eq!(repos[1].default_branch, "trunk");
        let seen = log.lock().unwrap();
        assert_eq!(
            seen[1].path,
            "/user/installations/7/repositories?per_page=100&page=1"
        );
        assert_eq!(
            seen[2].path,
            "/user/installations/8/repositories?per_page=100&page=1"
        );
    }

    #[tokio::test]
    async fn a_missing_branch_is_none_and_branch_names_keep_their_slash() {
        let (base, log) = serve(vec![
            Reply::json(404, json!({"message": "Branch not found"})),
            Reply::json(
                200,
                json!({"name": "main", "commit": {"sha": "abc"}, "protected": true}),
            ),
        ])
        .await;
        let api = GitHubApi::with_base(&base).unwrap();
        let repo = RepoRef::new("ann", "notes").unwrap();
        assert_eq!(
            api.branch(&token(), &repo, "shodh/fix login")
                .await
                .unwrap(),
            None
        );
        let main = api.branch(&token(), &repo, "main").await.unwrap().unwrap();
        assert!(main.protected);
        assert_eq!(
            log.lock().unwrap()[0].path,
            "/repos/ann/notes/branches/shodh/fix%20login"
        );
    }

    #[test]
    fn a_pull_request_is_read_from_its_json() {
        let pr = pull_of(&json!({
            "number": 12, "title": "Fix", "body": null, "state": "closed", "draft": false,
            "merged_at": "2026-11-01T10:00:00Z", "user": {"login": "ann"},
            "html_url": "https://github.com/octo-org/hello/pull/12",
            "head": {"ref": "shodh/fix", "sha": "abc", "repo": {"name": "hello", "owner": {"login": "ann"}}},
            "base": {"ref": "main"}, "mergeable_state": "clean", "mergeable": true,
            "commits": 2, "additions": 10, "deletions": 3, "changed_files": 1
        }));
        assert!(pr.merged);
        assert_eq!(pr.head_repo, Some(RepoRef::new("ann", "hello").unwrap()));
        assert_eq!(pr.body, "");
        assert_eq!(pr.mergeable_state, "clean");
    }
}
