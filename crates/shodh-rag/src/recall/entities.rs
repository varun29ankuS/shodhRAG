//! L2: exact ids found in a session's (redacted) text by pattern, with no model: pull
//! request and issue numbers, commit SHAs, file paths, branches, model ids, error codes
//! and URLs. They are looked up exactly ("the session that touched #120", "where did
//! E0502 come up") and filter a search.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// The kinds of entity Recall extracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// A pull request or issue number (`#120`).
    Issue,
    /// A commit SHA (7 to 40 hex digits).
    Commit,
    /// A file path.
    File,
    /// A git branch.
    Branch,
    /// A model id (`claude-opus-4`, `gpt-5`).
    Model,
    /// An error code or error type (`E0502`, `TS2345`, `ENOENT`, `TypeError`).
    Error,
    /// A URL.
    Url,
}

impl EntityKind {
    pub const ALL: [EntityKind; 7] = [
        EntityKind::Issue,
        EntityKind::Commit,
        EntityKind::File,
        EntityKind::Branch,
        EntityKind::Model,
        EntityKind::Error,
        EntityKind::Url,
    ];

    pub fn id(self) -> &'static str {
        match self {
            EntityKind::Issue => "issue",
            EntityKind::Commit => "commit",
            EntityKind::File => "file",
            EntityKind::Branch => "branch",
            EntityKind::Model => "model",
            EntityKind::Error => "error",
            EntityKind::Url => "url",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }
}

/// One entity: its kind and its value as written, and the value it is looked up by.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entity {
    pub kind: EntityKind,
    /// As written (a path keeps its spelling).
    pub value: String,
}

impl Entity {
    /// The value it is stored and looked up by: lower case, `/` for path separators, no
    /// trailing punctuation.
    pub fn key(&self) -> String {
        normalize(self.kind, &self.value)
    }
}

/// The lookup form of `value` as an entity of `kind`.
pub fn normalize(kind: EntityKind, value: &str) -> String {
    let v = value
        .trim()
        .trim_end_matches(['.', ',', ';', ':', ')', ']', '\'', '"']);
    match kind {
        EntityKind::File => v.replace('\\', "/").to_lowercase(),
        EntityKind::Issue => format!("#{}", v.trim_start_matches('#')),
        _ => v.to_lowercase(),
    }
}

/// Entities kept per turn at most.
pub const MAX_PER_TURN: usize = 40;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("recall entity pattern {pattern}: {e}"))
}

static ISSUE: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"(?i)(?:^|[\s(\[,])#(\d{1,6})\b|\b(?:pr|pull request|issue|pull)\s*#?\s*(\d{1,6})\b|/(?:pull|issues)/(\d{1,6})\b",
    )
});
static COMMIT: LazyLock<Regex> = LazyLock::new(|| regex(r"\b[0-9a-f]{7,40}\b"));
static FILE: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"(?:[A-Za-z]:)?(?:[\w.@~-]+[/\\])+[\w@-][\w.@-]*\.[A-Za-z][A-Za-z0-9]{0,7}\b|\b[\w-]+\.(?:rs|ts|tsx|js|jsx|mjs|py|go|java|kt|swift|rb|php|cs|cpp|cc|c|h|hpp|toml|json|yaml|yml|md|sql|sh|ps1|css|scss|html|vue|svelte|lock)\b",
    )
});
static BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"\b(?:feat|feature|fix|bugfix|hotfix|chore|docs|refactor|release|test|ci|perf|build|style|exp|wip)/[A-Za-z0-9][A-Za-z0-9._/-]*[A-Za-z0-9]",
    )
});
static BRANCH_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"\bgit (?:checkout -b|switch -c|checkout|switch|branch|push -u origin|push origin)\s+([A-Za-z0-9][A-Za-z0-9._/-]{1,80})",
    )
});
static MODEL: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"(?i)\b(?:claude-(?:opus|sonnet|haiku|fable|instant)[a-z0-9.-]*|claude-[0-9][a-z0-9.-]*|gpt-[0-9][a-z0-9.-]*|gemini-[0-9][a-z0-9.-]*|llama-?[0-9][a-z0-9.-]*|mistral-[a-z0-9.-]+|qwen[0-9][a-z0-9.-]*|deepseek-[a-z0-9.-]+|o[134](?:-mini|-pro)?(?:-\d{4}-\d{2}-\d{2})?\b|codex-[a-z0-9.-]+)",
    )
});
static ERROR: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"\berror\[(E\d{4})\]|\b(E\d{4})\b|\b(TS\d{4,5})\b|\b(E(?:NOENT|ACCES|ADDRINUSE|CONNREFUSED|CONNRESET|PERM|PIPE|TIMEDOUT|EXIST|NOTDIR|ISDIR|MFILE|NOSPC))\b|\b([A-Z][A-Za-z]*(?:Error|Exception))\b|\b(SIG(?:SEGV|ABRT|KILL|TERM|BUS))\b",
    )
});
static URL: LazyLock<Regex> = LazyLock::new(|| regex(r#"\bhttps?://[^\s<>"'`)\]}]+"#));

/// The entities of `text`, once each, at most [`MAX_PER_TURN`].
pub fn extract(text: &str) -> Vec<Entity> {
    let mut found: BTreeSet<Entity> = BTreeSet::new();
    let mut add = |kind: EntityKind, value: &str| {
        let value = value
            .trim()
            .trim_end_matches(['.', ',', ';', ':', ')', ']', '\'', '"', '`']);
        if !value.is_empty() && value.len() <= 300 {
            found.insert(Entity {
                kind,
                value: value.to_string(),
            });
        }
    };
    for caps in ISSUE.captures_iter(text) {
        if let Some(n) = caps.get(1).or_else(|| caps.get(2)).or_else(|| caps.get(3)) {
            add(EntityKind::Issue, &format!("#{}", n.as_str()));
        }
    }
    for m in COMMIT.find_iter(text) {
        let sha = m.as_str();
        let has_letter = sha.chars().any(|c| c.is_ascii_alphabetic());
        let has_digit = sha.chars().any(|c| c.is_ascii_digit());
        // Part of a UUID or a longer hex run is no commit.
        let before = text[..m.start()].chars().next_back();
        let after = text[m.end()..].chars().next();
        let in_uuid = before == Some('-') || after == Some('-');
        if has_letter && has_digit && !in_uuid {
            add(EntityKind::Commit, sha);
        }
    }
    for m in URL.find_iter(text) {
        add(EntityKind::Url, m.as_str());
    }
    for m in FILE.find_iter(text) {
        let path = m.as_str();
        // A URL's path is not a file of the project.
        let before = &text[..m.start()];
        let in_url = before
            .rsplit(char::is_whitespace)
            .next()
            .is_some_and(|word| word.contains("://"));
        if in_url {
            continue;
        }
        if path.contains("://") || path.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue;
        }
        add(EntityKind::File, path);
    }
    for m in BRANCH.find_iter(text) {
        add(EntityKind::Branch, m.as_str());
    }
    for caps in BRANCH_COMMAND.captures_iter(text) {
        let name = &caps[1];
        if !name.starts_with('-') && (name.contains('/') || (name != "main" && name != "master")) {
            add(EntityKind::Branch, name);
        }
    }
    for m in MODEL.find_iter(text) {
        add(EntityKind::Model, m.as_str());
    }
    for caps in ERROR.captures_iter(text) {
        if let Some(code) = (1..=6).find_map(|i| caps.get(i)) {
            add(EntityKind::Error, code.as_str());
        }
    }
    found.into_iter().take(MAX_PER_TURN).collect()
}
