//! What Shodh may do on GitHub: the user's per-repository permissions and the hard limits
//! no setting changes. Pure, so every rule is tested on its own and checked before any
//! call reaches GitHub.
//!
//! **Permissions** (the user sets them per repository; nothing else is reachable):
//! - Read: fetch, read issues and pull requests. No approval.
//! - Write: push a branch, open a pull request, comment. Each one approved.
//! - Merge: off by default; only when GitHub says the pull request is ready
//!   (`mergeable_state` `clean`: required checks passed, required reviews given, no
//!   conflict, not behind a required base), never as an administrator's override, with
//!   a method the repository allows. Each one approved.
//! - Delete a branch: only Shodh's own `shodh/…` branch, only once its pull request was
//!   merged with exactly what the branch holds. Each one approved.
//!
//! **Hard limits:** no force push (no `+` refspec, no `--force`); pushes go only to
//! `shodh/…` branches, never to the default branch or a protected one; no other branch,
//! tag or repository is deleted; no change to `.github/workflows/`, repository settings,
//! secrets, webhooks, Actions or collaborators (the app has no permission for them, and
//! no tool exists for them).

use serde::{Deserialize, Serialize};

use super::api::{BranchInfo, PullRequest, RepoInfo};
use super::remote::RepoRef;

/// Shodh's own branches.
pub const SHODH_PREFIX: &str = "shodh/";
/// Longest branch name Shodh creates.
const MAX_BRANCH_CHARS: usize = 100;
/// Files Shodh never pushes a change to (and the app may not write).
pub const WORKFLOWS_DIR: &str = ".github/workflows/";

/// The user's permissions on one repository (the allowlist entry).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoGrant {
    pub owner: String,
    pub name: String,
    /// Push a `shodh/…` branch, open pull requests, comment.
    #[serde(default)]
    pub write: bool,
    /// Merge pull requests (needs `write`).
    #[serde(default)]
    pub merge: bool,
}

impl RepoGrant {
    pub fn repo(&self) -> Option<RepoRef> {
        RepoRef::new(&self.owner, &self.name).ok()
    }

    pub fn is(&self, repo: &RepoRef) -> bool {
        self.owner.eq_ignore_ascii_case(&repo.owner) && self.name.eq_ignore_ascii_case(&repo.name)
    }
}

/// What an action needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Read,
    Write,
    Merge,
}

/// Why Shodh will not do something. The text is what the model and the user are told.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("Shodh may not reach github.com/{0}: it is not on your list of repositories. Ask the user to add it under Connectors → GitHub → Repositories (or call github_request_access).")]
    NotAllowed(String),
    #[error("Shodh may only read github.com/{0}. Pushing, pull requests and comments need Write, which the user turns on under Connectors → GitHub → Repositories.")]
    ReadOnly(String),
    #[error("Merging is off for github.com/{0}. Only the user can turn it on, under Connectors → GitHub → Repositories.")]
    MergeOff(String),
    #[error("“{0}” is not a branch name Shodh can use: letters, digits, “-”, “_”, “.” and “/” only, no “..”, no spaces, at most 100 characters.")]
    BadBranch(String),
    #[error("Shodh never pushes to the default branch ({0}). It pushes to a shodh/… branch and opens a pull request.")]
    DefaultBranch(String),
    #[error("{0} is a protected branch. Shodh never pushes to or deletes a protected branch.")]
    Protected(String),
    #[error("Shodh only deletes its own shodh/… branches; {0} is not one.")]
    NotShodhBranch(String),
    #[error(
        "{0} has no merged pull request with exactly what the branch holds, so Shodh keeps it."
    )]
    NotMerged(String),
    #[error("These commits change {0}. Shodh never changes GitHub Actions workflows; push this yourself if you mean to.")]
    Workflows(String),
    #[error("Pull request #{0} is {1}; only an open pull request can be merged.")]
    NotOpen(u64, String),
    #[error("Pull request #{0} is a draft. Mark it ready for review on GitHub first.")]
    Draft(u64),
    #[error("Pull request #{number} is not ready to merge: {why}. Shodh merges only when GitHub says every required check passed and every required review is given, and never overrides branch protection.")]
    NotReady { number: u64, why: String },
    #[error("This repository does not allow {0} merges.")]
    MethodNotAllowed(String),
    #[error("“{0}” is not a merge method: use merge, squash or rebase.")]
    UnknownMethod(String),
}

/// The grant of `repo`, if it is on the list.
pub fn grant_of<'a>(grants: &'a [RepoGrant], repo: &RepoRef) -> Option<&'a RepoGrant> {
    grants.iter().find(|g| g.is(repo))
}

/// Whether the allowlist lets Shodh do `need` on `repo`.
pub fn require(grants: &[RepoGrant], repo: &RepoRef, need: Need) -> Result<(), Refusal> {
    let name = repo.full_name();
    let grant = grant_of(grants, repo).ok_or_else(|| Refusal::NotAllowed(name.clone()))?;
    match need {
        Need::Read => Ok(()),
        Need::Write if grant.write => Ok(()),
        Need::Write => Err(Refusal::ReadOnly(name)),
        Need::Merge if grant.write && grant.merge => Ok(()),
        Need::Merge if !grant.write => Err(Refusal::ReadOnly(name)),
        Need::Merge => Err(Refusal::MergeOff(name)),
    }
}

/// Whether `name` passes git's ref-name rules and Shodh's own stricter ones.
fn valid_branch_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_BRANCH_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
        && !name.contains("..")
        && !name.contains("//")
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && name.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.starts_with('-')
                && !part.ends_with(".lock")
        })
}

/// The `shodh/<topic>` branch a push goes to. `topic` may already start with `shodh/`.
pub fn shodh_branch(topic: &str) -> Result<String, Refusal> {
    let topic = topic.trim();
    let shown: String = topic.chars().take(110).collect();
    let rest = topic.strip_prefix(SHODH_PREFIX).unwrap_or(topic);
    if rest.starts_with("refs/") || rest.contains(':') || rest.contains('+') {
        return Err(Refusal::BadBranch(shown));
    }
    let branch = format!("{SHODH_PREFIX}{rest}");
    if !valid_branch_name(rest) || !valid_branch_name(&branch) {
        return Err(Refusal::BadBranch(shown));
    }
    Ok(branch)
}

/// Whether `name` is one of Shodh's own branches.
pub fn is_shodh_branch(name: &str) -> bool {
    name.strip_prefix(SHODH_PREFIX)
        .is_some_and(|rest| valid_branch_name(rest))
}

/// Whether a push to `branch` (already a `shodh/…` name) may go ahead: never the default
/// branch, never a protected branch.
pub fn push_target_ok(
    branch: &str,
    repo: &RepoInfo,
    existing: Option<&BranchInfo>,
) -> Result<(), Refusal> {
    if !is_shodh_branch(branch) {
        return Err(Refusal::BadBranch(branch.to_string()));
    }
    if branch.eq_ignore_ascii_case(&repo.default_branch) {
        return Err(Refusal::DefaultBranch(repo.default_branch.clone()));
    }
    if existing.is_some_and(|b| b.protected) {
        return Err(Refusal::Protected(branch.to_string()));
    }
    Ok(())
}

/// The first workflow file among `paths` (files the pushed commits change).
pub fn workflow_change<'a>(paths: impl IntoIterator<Item = &'a str>) -> Option<String> {
    paths
        .into_iter()
        .map(|p| p.trim().trim_start_matches("./").replace('\\', "/"))
        .find(|p| p.to_ascii_lowercase().starts_with(WORKFLOWS_DIR))
}

/// A merge method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }

    /// The words the approval shows.
    pub fn words(self) -> &'static str {
        match self {
            MergeMethod::Merge => "Create a merge commit",
            MergeMethod::Squash => "Squash and merge",
            MergeMethod::Rebase => "Rebase and merge",
        }
    }

    fn allowed(self, repo: &RepoInfo) -> Option<bool> {
        match self {
            MergeMethod::Merge => repo.allow_merge_commit,
            MergeMethod::Squash => repo.allow_squash_merge,
            MergeMethod::Rebase => repo.allow_rebase_merge,
        }
    }

    pub fn parse(raw: &str) -> Result<Self, Refusal> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "merge" => Ok(MergeMethod::Merge),
            "squash" => Ok(MergeMethod::Squash),
            "rebase" => Ok(MergeMethod::Rebase),
            other => Err(Refusal::UnknownMethod(other.chars().take(40).collect())),
        }
    }
}

/// The method to merge with: the one asked for, if the repository allows it; else the
/// first the repository allows (merge, squash, rebase). When GitHub does not say what a
/// repository allows, the asked one (or a merge commit) is tried and GitHub decides.
pub fn merge_method(repo: &RepoInfo, asked: Option<&str>) -> Result<MergeMethod, Refusal> {
    if let Some(asked) = asked {
        let method = MergeMethod::parse(asked)?;
        return match method.allowed(repo) {
            Some(false) => Err(Refusal::MethodNotAllowed(method.as_str().to_string())),
            _ => Ok(method),
        };
    }
    [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase]
        .into_iter()
        .find(|m| m.allowed(repo) == Some(true))
        .or_else(|| {
            [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase]
                .into_iter()
                .find(|m| m.allowed(repo).is_none())
        })
        .ok_or_else(|| Refusal::MethodNotAllowed("any".to_string()))
}

/// What a `mergeable_state` means, in words, when it is not `clean`.
fn not_ready_reason(state: &str) -> String {
    match state {
        "blocked" => "a required check has not passed or a required review is missing".into(),
        "behind" => {
            "it is behind its base branch, which the repository requires to be up to date".into()
        }
        "dirty" => "it has conflicts with its base branch".into(),
        "unstable" => "some checks are failing".into(),
        "draft" => "it is a draft".into(),
        "has_hooks" => "GitHub has not confirmed its checks".into(),
        "unknown" | "" => "GitHub is still checking it; ask again in a moment".into(),
        other => format!(
            "GitHub says it is “{}”",
            other.chars().take(40).collect::<String>()
        ),
    }
}

/// Whether pull request `pr` may be merged now (merging already allowed for its
/// repository). Only `clean` is ready: an administrator's token can merge a blocked pull
/// request through the API, which would override branch protection.
pub fn merge_ready(pr: &PullRequest) -> Result<(), Refusal> {
    if pr.merged {
        return Err(Refusal::NotOpen(pr.number, "already merged".into()));
    }
    if pr.state != "open" {
        return Err(Refusal::NotOpen(pr.number, pr.state.clone()));
    }
    if pr.draft {
        return Err(Refusal::Draft(pr.number));
    }
    if pr.mergeable_state != "clean" || pr.mergeable == Some(false) {
        return Err(Refusal::NotReady {
            number: pr.number,
            why: not_ready_reason(&pr.mergeable_state),
        });
    }
    Ok(())
}

/// Whether branch `branch` of the repository may be deleted: a `shodh/…` branch, not the
/// default or a protected one, whose pull request (head `head_owner:branch`) was merged
/// with exactly the branch's last commit.
pub fn delete_ok(
    branch: &str,
    repo: &RepoInfo,
    existing: &BranchInfo,
    head_owner: &str,
    pulls: &[PullRequest],
) -> Result<(), Refusal> {
    if !is_shodh_branch(branch) {
        return Err(Refusal::NotShodhBranch(branch.chars().take(110).collect()));
    }
    if branch.eq_ignore_ascii_case(&repo.default_branch) {
        return Err(Refusal::DefaultBranch(repo.default_branch.clone()));
    }
    if existing.protected {
        return Err(Refusal::Protected(branch.to_string()));
    }
    let merged = pulls.iter().any(|pr| {
        pr.merged
            && pr.head_ref == branch
            && pr.head_sha == existing.sha
            && pr
                .head_repo
                .as_ref()
                .is_none_or(|r| r.owner.eq_ignore_ascii_case(head_owner))
    });
    if !merged {
        return Err(Refusal::NotMerged(branch.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants() -> Vec<RepoGrant> {
        vec![
            RepoGrant {
                owner: "ann".into(),
                name: "notes".into(),
                write: false,
                merge: false,
            },
            RepoGrant {
                owner: "octo-org".into(),
                name: "hello".into(),
                write: true,
                merge: false,
            },
            RepoGrant {
                owner: "octo-org".into(),
                name: "site".into(),
                write: true,
                merge: true,
            },
        ]
    }

    fn repo(name: &str) -> RepoRef {
        RepoRef::new("octo-org", name).unwrap()
    }

    fn info() -> RepoInfo {
        RepoInfo {
            id: 1,
            repo: repo("hello"),
            private: false,
            default_branch: "main".into(),
            can_push: Some(true),
            allow_merge_commit: Some(false),
            allow_squash_merge: Some(true),
            allow_rebase_merge: Some(false),
            parent: None,
        }
    }

    fn pr(state: &str, mergeable_state: &str) -> PullRequest {
        PullRequest {
            number: 7,
            title: "Fix".into(),
            body: String::new(),
            state: state.into(),
            draft: false,
            merged: false,
            author: "ann".into(),
            url: String::new(),
            head_repo: Some(repo("hello")),
            head_ref: "shodh/fix".into(),
            head_sha: "abc".into(),
            base_ref: "main".into(),
            mergeable_state: mergeable_state.into(),
            mergeable: Some(true),
            commits: 1,
            additions: 1,
            deletions: 0,
            changed_files: 1,
        }
    }

    #[test]
    fn the_allowlist_is_enforced_for_every_need() {
        let g = grants();
        let notes = RepoRef::new("ANN", "Notes").unwrap();
        assert_eq!(require(&g, &notes, Need::Read), Ok(()));
        assert!(matches!(
            require(&g, &notes, Need::Write),
            Err(Refusal::ReadOnly(_))
        ));
        assert!(matches!(
            require(&g, &notes, Need::Merge),
            Err(Refusal::ReadOnly(_))
        ));
        assert_eq!(require(&g, &repo("hello"), Need::Write), Ok(()));
        assert!(matches!(
            require(&g, &repo("hello"), Need::Merge),
            Err(Refusal::MergeOff(_))
        ));
        assert_eq!(require(&g, &repo("site"), Need::Merge), Ok(()));
        for need in [Need::Read, Need::Write, Need::Merge] {
            assert!(matches!(
                require(&g, &repo("secret"), need),
                Err(Refusal::NotAllowed(_))
            ));
        }
    }

    #[test]
    fn pushes_go_only_to_shodh_branches() {
        assert_eq!(shodh_branch("fix-login").unwrap(), "shodh/fix-login");
        assert_eq!(shodh_branch("shodh/fix-login").unwrap(), "shodh/fix-login");
        assert_eq!(shodh_branch("feature/v2.1").unwrap(), "shodh/feature/v2.1");
        for bad in [
            "",
            "main:main",
            "+main",
            "refs/heads/main",
            "a..b",
            "a b",
            "a//b",
            "/a",
            "a/",
            "a.lock",
            ".hidden",
            "a/.b",
            "-x",
            "a~1",
            "a^",
            "a?",
            "a*",
            "a[",
            "a\\b",
            "a@{1}",
            "shodh/",
            "a.",
        ] {
            assert!(shodh_branch(bad).is_err(), "{bad:?}");
        }
        assert!(shodh_branch(&"a".repeat(101)).is_err());
        assert!(is_shodh_branch("shodh/fix"));
        assert!(!is_shodh_branch("main"));
        assert!(!is_shodh_branch("shodh/"));
        assert!(!is_shodh_branch("feature/shodh/x"));
    }

    #[test]
    fn never_the_default_or_a_protected_branch() {
        let mut repo = info();
        assert_eq!(push_target_ok("shodh/fix", &repo, None), Ok(()));
        let protected = BranchInfo {
            name: "shodh/fix".into(),
            sha: "abc".into(),
            protected: true,
        };
        assert!(matches!(
            push_target_ok("shodh/fix", &repo, Some(&protected)),
            Err(Refusal::Protected(_))
        ));
        assert!(matches!(
            push_target_ok("main", &repo, None),
            Err(Refusal::BadBranch(_))
        ));
        repo.default_branch = "shodh/fix".into();
        assert!(matches!(
            push_target_ok("shodh/fix", &repo, None),
            Err(Refusal::DefaultBranch(_))
        ));
    }

    #[test]
    fn workflow_changes_are_found() {
        assert_eq!(
            workflow_change(["src/a.rs", ".github/workflows/ci.yml"]),
            Some(".github/workflows/ci.yml".into())
        );
        assert_eq!(
            workflow_change([".GitHub/Workflows/x.yml"]),
            Some(".GitHub/Workflows/x.yml".into())
        );
        assert_eq!(
            workflow_change(["./.github/workflows/x.yml"]),
            Some(".github/workflows/x.yml".into())
        );
        assert_eq!(workflow_change(["src/a.rs", ".github/CODEOWNERS"]), None);
    }

    #[test]
    fn merge_only_when_github_says_clean() {
        assert_eq!(merge_ready(&pr("open", "clean")), Ok(()));
        for state in [
            "blocked",
            "behind",
            "dirty",
            "unstable",
            "unknown",
            "has_hooks",
            "",
        ] {
            assert!(
                matches!(
                    merge_ready(&pr("open", state)),
                    Err(Refusal::NotReady { .. })
                ),
                "{state}"
            );
        }
        assert!(matches!(
            merge_ready(&pr("closed", "clean")),
            Err(Refusal::NotOpen(..))
        ));
        let mut draft = pr("open", "clean");
        draft.draft = true;
        assert!(matches!(merge_ready(&draft), Err(Refusal::Draft(7))));
        let mut merged = pr("open", "clean");
        merged.merged = true;
        assert!(matches!(merge_ready(&merged), Err(Refusal::NotOpen(..))));
        let mut conflicted = pr("open", "clean");
        conflicted.mergeable = Some(false);
        assert!(matches!(
            merge_ready(&conflicted),
            Err(Refusal::NotReady { .. })
        ));
    }

    #[test]
    fn the_repositorys_merge_methods_are_respected() {
        let repo = info();
        assert_eq!(merge_method(&repo, None), Ok(MergeMethod::Squash));
        assert_eq!(merge_method(&repo, Some("squash")), Ok(MergeMethod::Squash));
        assert!(matches!(
            merge_method(&repo, Some("rebase")),
            Err(Refusal::MethodNotAllowed(_))
        ));
        assert!(matches!(
            merge_method(&repo, Some("octopus")),
            Err(Refusal::UnknownMethod(_))
        ));
        let mut unknown = info();
        unknown.allow_merge_commit = None;
        unknown.allow_squash_merge = None;
        unknown.allow_rebase_merge = None;
        assert_eq!(merge_method(&unknown, None), Ok(MergeMethod::Merge));
        let mut none = info();
        none.allow_squash_merge = Some(false);
        assert!(merge_method(&none, None).is_err());
    }

    #[test]
    fn only_merged_shodh_branches_are_deleted() {
        let repo = info();
        let branch = BranchInfo {
            name: "shodh/fix".into(),
            sha: "abc".into(),
            protected: false,
        };
        let mut merged = pr("closed", "unknown");
        merged.merged = true;
        assert_eq!(
            delete_ok("shodh/fix", &repo, &branch, "octo-org", &[merged.clone()]),
            Ok(())
        );
        // Not merged, or merged with other commits than the branch now holds.
        assert!(matches!(
            delete_ok(
                "shodh/fix",
                &repo,
                &branch,
                "octo-org",
                &[pr("closed", "x")]
            ),
            Err(Refusal::NotMerged(_))
        ));
        let mut older = merged.clone();
        older.head_sha = "old".into();
        assert!(matches!(
            delete_ok("shodh/fix", &repo, &branch, "octo-org", &[older]),
            Err(Refusal::NotMerged(_))
        ));
        assert!(matches!(
            delete_ok(
                "shodh/fix",
                &repo,
                &branch,
                "someone-else",
                &[merged.clone()]
            ),
            Err(Refusal::NotMerged(_))
        ));
        // Never another branch, the default one or a protected one, even if merged.
        for other in ["main", "feature/x", "release"] {
            assert!(matches!(
                delete_ok(other, &repo, &branch, "octo-org", &[merged.clone()]),
                Err(Refusal::NotShodhBranch(_))
            ));
        }
        let protected = BranchInfo {
            protected: true,
            ..branch.clone()
        };
        assert!(matches!(
            delete_ok("shodh/fix", &repo, &protected, "octo-org", &[merged]),
            Err(Refusal::Protected(_))
        ));
    }
}
