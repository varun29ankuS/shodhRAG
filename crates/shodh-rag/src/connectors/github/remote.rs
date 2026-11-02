//! A project's git remotes as GitHub repositories: which remote is which `owner/repo`,
//! and, for a fork, where branches are pushed and where pull requests are opened.
//!
//! Only `github.com` itself counts. Its HTTPS and SSH forms are accepted
//! (`https://github.com/o/r(.git)`, `git@github.com:o/r.git`,
//! `ssh://git@github.com/o/r.git`); every other host is "not GitHub", including
//! enterprise or look-alike hosts (`github.example.com`, `github.com.evil.test`), so
//! Shodh never sends the sign-in anywhere else. Shodh never connects through the
//! remote's own URL: it builds `https://github.com/<owner>/<repo>.git` from the parsed
//! name ([`RepoRef::https_url`]).

use std::fmt;

use serde::{Deserialize, Serialize};

/// The one host the connector talks to.
pub const GITHUB_HOST: &str = "github.com";

/// A GitHub repository, `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

/// Why a repository name was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepoError {
    #[error("name the repository as owner/name, for example octo-org/hello-world")]
    Shape,
    #[error("“{0}” is not a GitHub account name")]
    Owner(String),
    #[error("“{0}” is not a GitHub repository name")]
    Name(String),
}

/// GitHub account names: 1 to 39 letters, digits or single hyphens, not at the ends.
fn valid_owner(owner: &str) -> bool {
    (1..=39).contains(&owner.len())
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !owner.starts_with('-')
        && !owner.ends_with('-')
}

/// GitHub repository names: 1 to 100 of letters, digits, `.`, `-`, `_`; never `.` or `..`.
fn valid_name(name: &str) -> bool {
    (1..=100).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && name != "."
        && name != ".."
}

impl RepoRef {
    pub fn new(owner: &str, name: &str) -> Result<Self, RepoError> {
        if !valid_owner(owner) {
            return Err(RepoError::Owner(owner.chars().take(60).collect()));
        }
        if !valid_name(name) {
            return Err(RepoError::Name(name.chars().take(110).collect()));
        }
        Ok(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    /// `owner/name` as typed (a trailing `.git` and spaces are ignored).
    pub fn parse(full: &str) -> Result<Self, RepoError> {
        let full = full.trim();
        let full = full.strip_prefix("https://github.com/").unwrap_or(full);
        let full = full.trim_end_matches('/');
        let (owner, name) = full.split_once('/').ok_or(RepoError::Shape)?;
        if name.contains('/') {
            return Err(RepoError::Shape);
        }
        let name = name.strip_suffix(".git").unwrap_or(name);
        Self::new(owner, name)
    }

    /// `owner/name`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// Whether this names the same repository as `other` (GitHub names ignore case).
    pub fn same(&self, other: &RepoRef) -> bool {
        self.owner.eq_ignore_ascii_case(&other.owner) && self.name.eq_ignore_ascii_case(&other.name)
    }

    /// The URL Shodh's git connects to: `<base><owner>/<name>.git`, `base` being
    /// `https://github.com/` (tests use a local folder).
    pub fn git_url(&self, base: &str) -> String {
        let base = base.trim_end_matches('/');
        format!("{base}/{}/{}.git", self.owner, self.name)
    }

    /// `https://github.com/<owner>/<name>.git`.
    pub fn https_url(&self) -> String {
        self.git_url("https://github.com")
    }

    /// The repository's page.
    pub fn web_url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.name)
    }
}

impl fmt::Display for RepoRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// What a remote URL is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteKind {
    GitHub(RepoRef),
    /// Another host (GitLab, Bitbucket, a self-hosted or enterprise server). Holds the
    /// host only, never the URL (it may carry a user name or a password).
    Other {
        host: String,
    },
    /// A local path or a URL Shodh cannot read.
    Unknown,
}

/// The host and path of a URL in any form git accepts, without credentials.
fn host_and_path(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    if let Some((scheme, rest)) = url.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(
            scheme.as_str(),
            "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git"
        ) {
            return None;
        }
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host_port = authority.rsplit('@').next().unwrap_or(authority);
        let host = match host_port.strip_prefix('[') {
            Some(v6) => v6.split(']').next().unwrap_or_default().to_string(),
            None => host_port.split(':').next().unwrap_or_default().to_string(),
        };
        return Some((host.to_ascii_lowercase(), path.to_string()));
    }
    // scp-like: `[user@]host:path`, never a Windows drive (`C:\…`, `C:/…`) or a path.
    let (before, path) = url.split_once(':')?;
    if before.contains('/') || before.contains('\\') || before.len() <= 1 {
        return None;
    }
    let host = before.rsplit('@').next().unwrap_or(before);
    if host.is_empty() {
        return None;
    }
    Some((host.to_ascii_lowercase(), path.to_string()))
}

/// What `url` (a remote's URL) is.
pub fn classify(url: &str) -> RemoteKind {
    let Some((host, path)) = host_and_path(url) else {
        return RemoteKind::Unknown;
    };
    if host.is_empty() {
        return RemoteKind::Unknown;
    }
    if host != GITHUB_HOST && host != "www.github.com" {
        return RemoteKind::Other { host };
    }
    let path = path.trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(name), None) => match RepoRef::new(owner, name) {
            Ok(repo) => RemoteKind::GitHub(repo),
            Err(_) => RemoteKind::Unknown,
        },
        _ => RemoteKind::Unknown,
    }
}

/// One remote of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub kind: RemoteKind,
}

/// Remote names Shodh accepts in a refspec (`refs/remotes/<name>/…`).
pub fn valid_remote_name(name: &str) -> bool {
    (1..=100).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !name.starts_with('.')
        && !name.starts_with('-')
        && !name.ends_with(".lock")
        && !name.contains("..")
}

/// The remotes of `git config --get-regexp '^remote\..*\.url$'` output (one
/// `remote.<name>.url <url>` per line; a remote with several URLs keeps its first).
/// Remotes whose names Shodh cannot use in a refspec are left out.
pub fn parse_remotes(config: &str) -> Vec<Remote> {
    let mut out: Vec<Remote> = Vec::new();
    for line in config.lines() {
        let Some((key, url)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|rest| rest.strip_suffix(".url"))
        else {
            continue;
        };
        if !valid_remote_name(name) || out.iter().any(|r| r.name == name) {
            continue;
        }
        out.push(Remote {
            name: name.to_string(),
            kind: classify(url),
        });
    }
    out
}

/// Where a project's GitHub work goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The remote branches are pushed to, and its repository.
    pub push_remote: String,
    pub push_repo: RepoRef,
    /// The repository pull requests are opened against (the original of a fork).
    pub pr_repo: RepoRef,
    /// Whether `push_repo` is a fork of `pr_repo` (an `upstream` remote).
    pub fork: bool,
}

/// Why a project has no route to GitHub.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    #[error("This project has no git remote. Add one on github.com and link it with your own git first.")]
    NoRemote,
    #[error("This project's remote “{remote}” is on {host}, not GitHub. Shodh can't push to {host} yet: push this branch yourself.")]
    NotGitHub { remote: String, host: String },
    #[error("This project has no remote named “{0}”.")]
    NoSuchRemote(String),
    #[error("Shodh can't tell where the remote “{0}” points. Push this branch yourself.")]
    Unreadable(String),
    #[error("This project has several GitHub remotes ({0}). Say which one to use.")]
    Ambiguous(String),
}

fn github_of(remote: &Remote) -> Result<RepoRef, RouteError> {
    match &remote.kind {
        RemoteKind::GitHub(repo) => Ok(repo.clone()),
        RemoteKind::Other { host } => Err(RouteError::NotGitHub {
            remote: remote.name.clone(),
            host: host.clone(),
        }),
        RemoteKind::Unknown => Err(RouteError::Unreadable(remote.name.clone())),
    }
}

/// The remote to use: `wanted` when named, else `origin`, else the only remote.
pub fn pick_remote<'a>(
    remotes: &'a [Remote],
    wanted: Option<&str>,
) -> Result<&'a Remote, RouteError> {
    if let Some(name) = wanted {
        return remotes
            .iter()
            .find(|r| r.name == name)
            .ok_or_else(|| RouteError::NoSuchRemote(name.chars().take(100).collect()));
    }
    if let Some(origin) = remotes.iter().find(|r| r.name == "origin") {
        return Ok(origin);
    }
    match remotes {
        [] => Err(RouteError::NoRemote),
        [only] => Ok(only),
        many => {
            let github: Vec<&Remote> = many
                .iter()
                .filter(|r| matches!(r.kind, RemoteKind::GitHub(_)))
                .collect();
            match github.as_slice() {
                [only] => Ok(only),
                _ => Err(RouteError::Ambiguous(
                    many.iter()
                        .map(|r| r.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                )),
            }
        }
    }
}

/// Where pushes and pull requests go. A fork (an `origin` and an `upstream` on GitHub
/// that are different repositories): branches go to `origin`, pull requests to
/// `upstream`. Otherwise both go to the chosen remote.
pub fn route(remotes: &[Remote], wanted: Option<&str>) -> Result<Route, RouteError> {
    let push = pick_remote(remotes, wanted)?;
    let push_repo = github_of(push)?;
    let upstream = remotes
        .iter()
        .find(|r| r.name == "upstream" && r.name != push.name)
        .and_then(|r| match &r.kind {
            RemoteKind::GitHub(repo) if !repo.same(&push_repo) => Some(repo.clone()),
            _ => None,
        });
    Ok(match upstream {
        Some(pr_repo) => Route {
            push_remote: push.name.clone(),
            push_repo,
            pr_repo,
            fork: true,
        },
        None => Route {
            push_remote: push.name.clone(),
            push_repo: push_repo.clone(),
            pr_repo: push_repo,
            fork: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(owner: &str, name: &str) -> RepoRef {
        RepoRef::new(owner, name).unwrap()
    }

    #[test]
    fn github_remotes_in_every_form() {
        for url in [
            "https://github.com/octo-org/hello-world.git",
            "https://github.com/octo-org/hello-world",
            "https://github.com/octo-org/hello-world/",
            "https://ann@github.com/octo-org/hello-world.git",
            "https://ann:secret@github.com/octo-org/hello-world.git",
            "HTTPS://GitHub.com/octo-org/hello-world.git",
            "git@github.com:octo-org/hello-world.git",
            "git@github.com:octo-org/hello-world",
            "ssh://git@github.com/octo-org/hello-world.git",
            "ssh://git@github.com:22/octo-org/hello-world.git",
        ] {
            assert_eq!(
                classify(url),
                RemoteKind::GitHub(repo("octo-org", "hello-world")),
                "{url}"
            );
        }
    }

    #[test]
    fn other_hosts_are_never_github() {
        for (url, host) in [
            ("https://gitlab.com/a/b.git", "gitlab.com"),
            ("git@gitlab.com:a/b.git", "gitlab.com"),
            ("https://github.example.com/a/b.git", "github.example.com"),
            (
                "https://github.com.evil.test/a/b.git",
                "github.com.evil.test",
            ),
            (
                "git@github.mycorp.internal:a/b.git",
                "github.mycorp.internal",
            ),
            ("https://evil.test/github.com/a/b.git", "evil.test"),
            ("ssh://git@bitbucket.org/a/b.git", "bitbucket.org"),
        ] {
            assert_eq!(
                classify(url),
                RemoteKind::Other { host: host.into() },
                "{url}"
            );
        }
        // A password in the URL never reaches what is kept.
        let kept = format!("{:?}", classify("https://ann:hunter2@gitlab.com/a/b.git"));
        assert!(!kept.contains("hunter2"), "{kept}");
    }

    #[test]
    fn paths_and_odd_urls_are_unknown() {
        for url in [
            "C:\\code\\repo",
            "C:/code/repo",
            "/srv/git/repo.git",
            "../repo",
            "file:///srv/git/repo.git",
            "https://github.com/only-owner",
            "https://github.com/a/b/c",
            "https://github.com/-bad/repo",
            "https://github.com/a/..",
            "",
        ] {
            assert_eq!(classify(url), RemoteKind::Unknown, "{url}");
        }
    }

    #[test]
    fn repo_names_are_checked() {
        assert_eq!(
            RepoRef::parse("octo-org/hello.world").unwrap(),
            repo("octo-org", "hello.world")
        );
        assert_eq!(
            RepoRef::parse("https://github.com/a/b.git").unwrap(),
            repo("a", "b")
        );
        assert_eq!(RepoRef::parse("a"), Err(RepoError::Shape));
        assert_eq!(RepoRef::parse("a/b/c"), Err(RepoError::Shape));
        assert!(matches!(RepoRef::parse("a b/c"), Err(RepoError::Owner(_))));
        assert!(matches!(RepoRef::parse("a/b c"), Err(RepoError::Name(_))));
        assert!(matches!(RepoRef::parse("a/.."), Err(RepoError::Name(_))));
        assert!(repo("A", "B").same(&repo("a", "b")));
        assert_eq!(repo("a", "b").https_url(), "https://github.com/a/b.git");
    }

    #[test]
    fn remotes_are_read_from_git_config() {
        let config = "remote.origin.url git@github.com:ann/hello.git\n\
                      remote.origin.url https://github.com/ann/other.git\n\
                      remote.upstream.url https://github.com/octo-org/hello.git\n\
                      remote.lab.url git@gitlab.com:ann/hello.git\n\
                      remote.bad..name.url https://github.com/x/y.git\n";
        let remotes = parse_remotes(config);
        assert_eq!(
            remotes,
            vec![
                Remote {
                    name: "origin".into(),
                    kind: RemoteKind::GitHub(repo("ann", "hello"))
                },
                Remote {
                    name: "upstream".into(),
                    kind: RemoteKind::GitHub(repo("octo-org", "hello"))
                },
                Remote {
                    name: "lab".into(),
                    kind: RemoteKind::Other {
                        host: "gitlab.com".into()
                    }
                },
            ]
        );
    }

    #[test]
    fn a_fork_pushes_to_origin_and_opens_prs_upstream() {
        let remotes = parse_remotes(
            "remote.origin.url git@github.com:ann/hello.git\nremote.upstream.url https://github.com/octo-org/hello.git\n",
        );
        let route = route(&remotes, None).unwrap();
        assert_eq!(route.push_remote, "origin");
        assert_eq!(route.push_repo, repo("ann", "hello"));
        assert_eq!(route.pr_repo, repo("octo-org", "hello"));
        assert!(route.fork);
        // Pushing to upstream itself is not a fork route.
        let direct = super::route(&remotes, Some("upstream")).unwrap();
        assert_eq!(direct.push_repo, repo("octo-org", "hello"));
        assert_eq!(direct.pr_repo, repo("octo-org", "hello"));
        assert!(!direct.fork);
    }

    #[test]
    fn a_single_repo_routes_to_itself_and_others_are_refused_plainly() {
        let remotes = parse_remotes("remote.origin.url https://github.com/octo-org/hello.git\n");
        let r = route(&remotes, None).unwrap();
        assert_eq!(r.pr_repo, r.push_repo);
        assert!(!r.fork);
        assert_eq!(route(&[], None), Err(RouteError::NoRemote));
        let lab = parse_remotes("remote.origin.url git@gitlab.com:ann/hello.git\n");
        let error = route(&lab, None).unwrap_err();
        assert!(
            error.to_string().contains("can't push to gitlab.com yet"),
            "{error}"
        );
        assert_eq!(
            route(&remotes, Some("nope")),
            Err(RouteError::NoSuchRemote("nope".into()))
        );
        let two = parse_remotes(
            "remote.a.url https://github.com/x/one.git\nremote.b.url https://github.com/x/two.git\n",
        );
        assert!(matches!(route(&two, None), Err(RouteError::Ambiguous(_))));
    }

    #[test]
    fn remote_names_safe_in_a_refspec() {
        for good in ["origin", "upstream", "my-fork", "fork_2", "a.b"] {
            assert!(valid_remote_name(good), "{good}");
        }
        for bad in [
            "", ".hidden", "-x", "a..b", "x.lock", "a/b", "a b", "a*", "a:b", "+a",
        ] {
            assert!(!valid_remote_name(bad), "{bad}");
        }
    }
}
