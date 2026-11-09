//! Design check: a deterministic check of UI source files (HTML, CSS, JSX/TSX,
//! Vue, Svelte and the Tailwind classes in them) for generated-UI tells and
//! design and accessibility flaws: purple-blue gradient defaults, gradient
//! text, side-tab borders, low contrast, text under 12px, missing alt text and
//! labels, clickable divs, removed focus outlines, emoji as icons, Lorem ipsum,
//! inconsistent radius and type scales, and the rest of [`rules::RULES`].
//!
//! No model is called: the same files always give the same findings, in the
//! same order ([`check_source`] is pure). Each finding carries its rule, a
//! severity, the file and line, what is wrong in plain words, and a
//! suggested fix; a fix that is mechanical (one value or class replaced on one
//! line) also carries the exact replacement, which is only ever *proposed*:
//! Shodh sends it to the conversation for an approved edit, never writes it.
//!
//! Most rules are ported from pbakaus/impeccable (Apache-2.0); see
//! [`rules`] and `NOTICE`.
//!
//! Bounded: at most [`MAX_FILES`] files, [`MAX_FILE_BYTES`] each and
//! [`MAX_TOTAL_BYTES`] in all, and [`MAX_PER_RULE`] findings of one rule per
//! file; what is left out is counted in the report.

pub mod color;
pub mod rules;
pub mod scan;
pub mod source;

#[cfg(test)]
mod tests;

use std::path::Path;

use serde::Serialize;

pub use rules::{rule, DesignSeverity, Rule, RuleOrigin, RULES};
pub use source::UiKind;

use crate::code_review::secrets::secret_path;
use crate::code_review::structure::SKIPPED_DIRS;

/// Files one check reads at most.
pub const MAX_FILES: usize = 400;
/// Largest file read; a larger one is counted as skipped.
pub const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Bytes one check reads at most.
pub const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024;
/// Findings of one rule kept per file (the rest are counted).
pub const MAX_PER_RULE: usize = 20;
/// Findings one report keeps (worst first; the rest are counted).
pub const MAX_FINDINGS: usize = 500;
/// Longest line a mechanical fix is offered on (a longer one is minified).
const MAX_FIX_LINE_CHARS: usize = 400;

/// A mechanical fix: in `path`, replace `old` (a whole line, found exactly
/// once in the file) with `new`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignReplacement {
    pub path: String,
    pub old: String,
    pub new: String,
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignFinding {
    pub rule: &'static str,
    pub severity: DesignSeverity,
    pub origin: RuleOrigin,
    /// The rule's name.
    pub title: &'static str,
    pub path: String,
    pub line: usize,
    pub end_line: usize,
    /// What is wrong, in plain words (the rule's description).
    pub explanation: &'static str,
    /// What was found there (`font-size: 10px`).
    pub evidence: String,
    /// What to do instead.
    pub fix: &'static str,
    pub replace: Option<DesignReplacement>,
}

/// A file the check could not read, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

/// Counts by severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeverityCounts {
    pub error: usize,
    pub warning: usize,
    pub advisory: usize,
}

/// A finished check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignReport {
    /// Worst first, then by path, line and rule.
    pub findings: Vec<DesignFinding>,
    /// Counted over every finding, also those past [`MAX_FINDINGS`].
    pub counts: SeverityCounts,
    pub files_checked: usize,
    pub skipped: Vec<SkippedFile>,
    /// Findings past the per-rule and report limits, counted, not listed.
    pub left_out: usize,
    /// A limit stopped the check before every UI file was read.
    pub partial: bool,
    pub rules: usize,
}

impl SeverityCounts {
    pub fn add(&mut self, severity: DesignSeverity) {
        match severity {
            DesignSeverity::Error => self.error += 1,
            DesignSeverity::Warning => self.warning += 1,
            DesignSeverity::Advisory => self.advisory += 1,
        }
    }

    pub fn merge(&mut self, other: &SeverityCounts) {
        self.error += other.error;
        self.warning += other.warning;
        self.advisory += other.advisory;
    }

    pub fn total(&self) -> usize {
        self.error + self.warning + self.advisory
    }
}

impl DesignReport {
    /// Every finding, counted and listed or not.
    pub fn total(&self) -> usize {
        self.counts.total()
    }
}

/// Whether `path` is a UI file the check reads.
pub fn is_ui_file(path: &str) -> bool {
    UiKind::of_path(path).is_some()
}

/// The findings of one file (`path` as shown, `/`-separated). Pure: the same
/// text always gives the same findings in the same order. Also returns how
/// many findings the per-rule limit left out, by severity.
pub fn check_source(path: &str, text: &str) -> (Vec<DesignFinding>, SeverityCounts) {
    let Some(kind) = UiKind::of_path(path) else {
        return (Vec::new(), SeverityCounts::default());
    };
    let source = source::Source::new(path, text, kind);
    let mut hits = scan::run(&source);
    hits.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then(rules::rank(a.rule).cmp(&rules::rank(b.rule)))
            .then(a.end.cmp(&b.end))
    });
    hits.dedup_by(|a, b| a.rule == b.rule && a.start == b.start);
    let mut per_rule: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut findings = Vec::new();
    let mut left_out = SeverityCounts::default();
    for hit in hits {
        let Some(rule) = rules::rule(hit.rule) else {
            continue;
        };
        let seen = per_rule.entry(hit.rule).or_default();
        *seen += 1;
        if *seen > MAX_PER_RULE {
            left_out.add(rule.severity);
            continue;
        }
        let line = source.line_of(hit.start);
        let end_line = source.line_of(hit.end.saturating_sub(1).max(hit.start));
        let replace = hit
            .fix
            .as_ref()
            .and_then(|(start, end, new)| line_fix(&source, path, *start, *end, new));
        findings.push(DesignFinding {
            rule: rule.id,
            severity: rule.severity,
            origin: rule.origin,
            title: rule.name,
            path: path.to_string(),
            line,
            end_line,
            explanation: rule.description,
            evidence: hit.evidence,
            fix: rule.fix,
            replace,
        });
    }
    (findings, left_out)
}

/// `start..end` of the file replaced with `new`, as a whole-line edit: only
/// when the span is on one line, the line is not minified, and the line is
/// found exactly once in the file (so the edit lands where it is meant to).
fn line_fix(
    source: &source::Source,
    path: &str,
    start: usize,
    end: usize,
    new: &str,
) -> Option<DesignReplacement> {
    let line = source.line_of(start);
    if source.line_of(end.saturating_sub(1).max(start)) != line {
        return None;
    }
    let old = source.line_text(line);
    if old.trim().is_empty() || old.chars().count() > MAX_FIX_LINE_CHARS {
        return None;
    }
    if source.raw.matches(old).count() != 1 {
        return None;
    }
    let line_start = source.line_start(line);
    let (from, to) = (start.checked_sub(line_start)?, end.checked_sub(line_start)?);
    if to > old.len() || !old.is_char_boundary(from) || !old.is_char_boundary(to) {
        return None;
    }
    let replaced = format!("{}{}{}", &old[..from], new, &old[to..]);
    (replaced != old).then(|| DesignReplacement {
        path: path.to_string(),
        old: old.to_string(),
        new: replaced,
    })
}

/// Sort worst first, then by path, line and rule: the report's one order.
fn rank(findings: &mut [DesignFinding]) {
    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
            .then(rules::rank(a.rule).cmp(&rules::rank(b.rule)))
            .then_with(|| a.evidence.cmp(&b.evidence))
    });
}

/// A check of the given files (paths relative to `root`, `/`-separated).
/// Each is read inside `root` (a link is not followed), at most
/// [`MAX_FILE_BYTES`]; files that are not UI files are ignored. Blocking.
pub fn check_files(root: &Path, paths: &[String]) -> DesignReport {
    let mut report = DesignReport {
        findings: Vec::new(),
        counts: SeverityCounts::default(),
        files_checked: 0,
        skipped: Vec::new(),
        left_out: 0,
        partial: false,
        rules: RULES.len(),
    };
    let mut paths: Vec<&String> = paths.iter().filter(|p| is_ui_file(p)).collect();
    paths.sort();
    paths.dedup();
    let mut total: u64 = 0;
    for path in paths {
        if report.files_checked >= MAX_FILES || total >= MAX_TOTAL_BYTES {
            report.partial = true;
            break;
        }
        if let Some(why) = secret_path(path) {
            report.skipped.push(SkippedFile {
                path: path.clone(),
                reason: why.to_string(),
            });
            continue;
        }
        let full = root.join(path);
        let meta = match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_file() => meta,
            // Deleted since it was listed, or a link: nothing to check.
            _ => continue,
        };
        if meta.len() > MAX_FILE_BYTES {
            report.skipped.push(SkippedFile {
                path: path.clone(),
                reason: "larger than 512 KB".to_string(),
            });
            continue;
        }
        let text = match std::fs::read(&full).map(String::from_utf8) {
            Ok(Ok(text)) => text,
            _ => {
                report.skipped.push(SkippedFile {
                    path: path.clone(),
                    reason: "not text".to_string(),
                });
                continue;
            }
        };
        total += meta.len();
        report.files_checked += 1;
        let (findings, left_out) = check_source(path, &text);
        report.left_out += left_out.total();
        report.counts.merge(&left_out);
        report.findings.extend(findings);
    }
    for finding in &report.findings {
        report.counts.add(finding.severity);
    }
    rank(&mut report.findings);
    if report.findings.len() > MAX_FINDINGS {
        report.left_out += report.findings.len() - MAX_FINDINGS;
        report.findings.truncate(MAX_FINDINGS);
    }
    report
}

/// The UI files under `within` (a folder or file of `root`, `.` for all of
/// it), relative to `root`: in a repository the files git tracks or would
/// add, elsewhere a walk honouring `.gitignore`; never dependency or build
/// folders, files that may hold secrets, or links. Sorted; at most
/// [`MAX_FILES`] + 1 (one more than a check reads, so it can say it stopped).
/// Blocking.
pub fn ui_files(root: &Path, within: &str) -> Vec<String> {
    let within = within.trim().trim_start_matches("./").trim_end_matches('/');
    let within = if within.is_empty() { "." } else { within };
    let full = if within == "." {
        root.to_path_buf()
    } else {
        root.join(within)
    };
    if std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_file()) {
        return if is_ui_file(within) {
            vec![within.to_string()]
        } else {
            Vec::new()
        };
    }
    let keep = |relative: &str| {
        is_ui_file(relative)
            && secret_path(relative).is_none()
            && !relative
                .split('/')
                .any(|part| SKIPPED_DIRS.contains(&part) || part == "..")
    };
    let mut files: Vec<String> = match crate::harness::code_mode::listed_files(root, within) {
        Some(listed) => listed.into_iter().filter(|p| keep(p)).collect(),
        None => {
            let mut found = Vec::new();
            let walker = ignore::WalkBuilder::new(&full)
                .hidden(false)
                .follow_links(false)
                .git_ignore(true)
                .git_exclude(true)
                .git_global(false)
                .require_git(false)
                .parents(false)
                .sort_by_file_name(|a, b| a.cmp(b))
                .filter_entry(|entry| {
                    let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                    !(is_dir
                        && entry.depth() > 0
                        && entry.file_name().to_str().is_some_and(|name| {
                            SKIPPED_DIRS.contains(&name) || secret_path(name).is_some()
                        }))
                })
                .build();
            for entry in walker.filter_map(Result::ok).take(50_000) {
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                let Ok(relative) = entry.path().strip_prefix(root) else {
                    continue;
                };
                let relative = relative.to_string_lossy().replace('\\', "/");
                if keep(&relative) {
                    found.push(relative);
                }
                if found.len() > MAX_FILES {
                    break;
                }
            }
            found
        }
    };
    files.sort();
    files.truncate(MAX_FILES + 1);
    files
}

/// The UI files of `root` changed against `base` (the last commit when
/// `None`) or not tracked yet: what Code mode's edits wrote. `None` outside a
/// repository. Blocking.
pub fn changed_ui_files(root: &Path, base: Option<&str>) -> Option<Vec<String>> {
    let changed = crate::harness::code_mode::changed_paths(root, base, 20_000)?;
    Some(
        changed
            .into_iter()
            .filter(|p| is_ui_file(p) && secret_path(p).is_none())
            .take(MAX_FILES + 1)
            .collect(),
    )
}
