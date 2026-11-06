//! The reviewer's reply, checked: ranked findings whose citations point at
//! lines the reviewer was actually shown, fixes whose replaced text is in the
//! code, verification commands that pass the command checks, and the
//! module diagram of an Architecture review.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::request::{LineKind, ReviewCheck, ReviewRequest};
use super::structure::StructureView;

/// Findings kept from one reply.
pub const MAX_FINDINGS: usize = 40;
/// Characters kept of each text field of a finding.
const MAX_FIELD_CHARS: usize = 4_000;

/// How bad a finding is, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "critical" => Some(Severity::Critical),
            "high" => Some(Severity::High),
            "medium" | "moderate" => Some(Severity::Medium),
            "low" | "minor" => Some(Severity::Low),
            _ => None,
        }
    }
}

/// Which version of a file a line number counts in: the new one (unchanged and
/// added lines, and every line of plain code) or the old one (removed lines).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    New,
    Old,
}

/// Lines of a file a finding cites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub path: String,
    pub line: usize,
    pub end_line: usize,
    pub side: Side,
}

/// A fix that replaces text found in the code reviewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Replacement {
    pub path: String,
    pub old: String,
    pub new: String,
}

/// What a check command should do when the finding is right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Expect {
    Fails,
    Passes,
}

/// A command the reviewer proposes to check a finding by running it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyStep {
    pub command: String,
    pub expect: Expect,
    pub output_contains: Option<String>,
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub severity: Severity,
    pub title: String,
    /// `None` when the reviewer named no line it was shown.
    pub citation: Option<Citation>,
    pub scenario: String,
    pub fix: String,
    pub replace: Option<Replacement>,
    /// Architecture: module imports the finding is about.
    pub edges: Vec<(String, String)>,
    pub verify: Option<VerifyStep>,
}

/// A checked review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewReport {
    pub check: ReviewCheck,
    pub summary: String,
    /// Worst first.
    pub findings: Vec<Finding>,
    /// Architecture: the module diagram (Mermaid), problem edges marked.
    pub diagram: Option<String>,
    /// Citations or fixes the reviewer gave that were dropped, as they do not
    /// point at the code it was shown.
    pub dropped: usize,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawReply {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    findings: Vec<Value>,
}

/// The JSON object of a reply (inside a code fence or prose, the first `{`
/// to the last `}`).
fn json_object(text: &str) -> Option<RawReply> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

/// `text` with every hidden character ([`crate::harness::run_command::hidden_character`])
/// shown as ⟨U+XXXX⟩: the reviewer's words are shown to the user and may be sent on, and
/// an invisible instruction must not ride along.
pub fn visible_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if crate::harness::run_command::hidden_character(c) {
            out.push_str(&format!("⟨U+{:04X}⟩", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

fn field(value: &Value, key: &str) -> String {
    let text = value.get(key).and_then(Value::as_str).unwrap_or("").trim();
    let text = if text.chars().count() > MAX_FIELD_CHARS {
        let kept: String = text.chars().take(MAX_FIELD_CHARS).collect();
        format!("{kept}…")
    } else {
        text.to_string()
    };
    visible_text(&text)
}

fn number(value: &Value, key: &str) -> Option<usize> {
    let v = value.get(key)?;
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .and_then(|n| usize::try_from(n).ok())
}

/// The lines the reviewer was shown, by path and side, and the runs of new-side
/// lines whose numbers follow each other (the text a fix may replace).
struct Shown<'a> {
    new: HashMap<&'a str, BTreeSet<usize>>,
    old: HashMap<&'a str, BTreeSet<usize>>,
    runs: HashMap<&'a str, Vec<String>>,
    structure: Option<&'a StructureView>,
}

impl<'a> Shown<'a> {
    fn new(request: &'a ReviewRequest, structure: Option<&'a StructureView>) -> Self {
        let mut new: HashMap<&str, BTreeSet<usize>> = HashMap::new();
        let mut old: HashMap<&str, BTreeSet<usize>> = HashMap::new();
        let mut runs: HashMap<&str, Vec<String>> = HashMap::new();
        for piece in &request.pieces {
            let path = piece.path.as_str();
            let mut run = String::new();
            let mut last: Option<usize> = None;
            for line in &piece.lines {
                if line.kind == LineKind::Removed {
                    old.entry(path).or_default().insert(line.number);
                    continue;
                }
                new.entry(path).or_default().insert(line.number);
                if last.is_some_and(|n| n + 1 != line.number) && !run.is_empty() {
                    runs.entry(path).or_default().push(std::mem::take(&mut run));
                }
                run.push_str(&line.text);
                run.push('\n');
                last = Some(line.number);
            }
            if !run.is_empty() {
                runs.entry(path).or_default().push(run);
            }
        }
        Shown {
            new,
            old,
            runs,
            structure,
        }
    }

    /// The citation, when its path and lines are ones the reviewer saw on that
    /// side: lines of a piece, or, in an Architecture review, lines of a file
    /// of the structural view (new side).
    fn citation(&self, path: &str, line: usize, end: usize, side: Side) -> Option<Citation> {
        let path = path.trim().trim_start_matches("./").replace('\\', "/");
        let (start, end) = (line.min(end), line.max(end));
        if start == 0 {
            return None;
        }
        let lines = match side {
            Side::New => self.new.get(path.as_str()),
            Side::Old => self.old.get(path.as_str()),
        };
        let shown_anywhere =
            self.new.contains_key(path.as_str()) || self.old.contains_key(path.as_str());
        let ok = match lines {
            Some(shown) => shown.contains(&start) && shown.contains(&end),
            None if shown_anywhere => false,
            None => {
                side == Side::New
                    && self
                        .structure
                        .and_then(|view| view.file_lines(&path))
                        .is_some_and(|count| end <= count.max(1))
            }
        };
        ok.then_some(Citation {
            path,
            line: start,
            end_line: end,
            side,
        })
    }

    /// Whether `old` is text of `path` as it is now, within one run of
    /// consecutive lines: never removed lines, never text stitched across a gap.
    fn contains(&self, path: &str, old: &str) -> bool {
        !old.is_empty()
            && self
                .runs
                .get(path)
                .is_some_and(|runs| runs.iter().any(|run| run.contains(old)))
    }
}

fn verify_step(value: &Value) -> Option<VerifyStep> {
    let command = value.get("command")?.as_str()?.trim().to_string();
    // The same checks every command gets: length, hidden characters, blank runs.
    crate::harness::run_command::parse_args(&serde_json::json!({ "command": command })).ok()?;
    let expect = match value.get("expect")?.as_str()?.trim() {
        "fails" | "fail" => Expect::Fails,
        "passes" | "pass" => Expect::Passes,
        _ => return None,
    };
    let output_contains = value
        .get("outputContains")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().count() <= 400)
        .map(str::to_string);
    // A failure confirms nothing unless it is the failure the finding predicts.
    if expect == Expect::Fails && output_contains.is_none() {
        return None;
    }
    Some(VerifyStep {
        command,
        expect,
        output_contains,
    })
}

/// The reviewer's `reply` to `request`, checked. `None` when it holds no
/// JSON object of the asked shape.
pub fn parse_reply(
    request: &ReviewRequest,
    structure: Option<&StructureView>,
    reply: &str,
) -> Option<ReviewReport> {
    let raw = json_object(reply)?;
    let shown = Shown::new(request, structure);
    let mut dropped = 0usize;
    let mut findings = Vec::new();
    for value in raw.findings.iter().take(MAX_FINDINGS) {
        let Some(severity) = value
            .get("severity")
            .and_then(Value::as_str)
            .and_then(Severity::parse)
        else {
            dropped += 1;
            continue;
        };
        let title = field(value, "title");
        if title.is_empty() {
            dropped += 1;
            continue;
        }
        let path = field(value, "path");
        let citation = number(value, "line").and_then(|line| {
            let end = number(value, "endLine").unwrap_or(line);
            let side = match value.get("side").and_then(Value::as_str) {
                Some("old") => Side::Old,
                _ => Side::New,
            };
            shown.citation(&path, line, end, side)
        });
        if citation.is_none() && (!path.is_empty() || number(value, "line").is_some()) {
            dropped += 1;
        }
        let replace = value
            .get("replace")
            .filter(|r| r.is_object())
            .and_then(|r| {
                let old = r.get("old")?.as_str()?.to_string();
                let new = r.get("new")?.as_str()?.to_string();
                let path = citation.as_ref()?.path.clone();
                if shown.contains(&path, &old) && old != new {
                    Some(Replacement { path, old, new })
                } else {
                    None
                }
            });
        if value.get("replace").is_some_and(Value::is_object) && replace.is_none() {
            dropped += 1;
        }
        let edges: Vec<(String, String)> = match (request.check, structure) {
            (ReviewCheck::Architecture, Some(view)) => value
                .get("edges")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|pair| {
                            let pair = pair.as_array()?;
                            let from = pair.first()?.as_str()?.trim().to_string();
                            let to = pair.get(1)?.as_str()?.trim().to_string();
                            view.has_edge(&from, &to).then_some((from, to))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let verify = if request.can_verify {
            value
                .get("verify")
                .filter(|v| v.is_object())
                .and_then(verify_step)
        } else {
            None
        };
        findings.push(Finding {
            severity,
            title,
            citation,
            scenario: field(value, "scenario"),
            fix: field(value, "fix"),
            replace,
            edges,
            verify,
        });
    }
    // Worst first; the reviewer's order within a severity.
    findings.sort_by_key(|f| f.severity);
    let diagram = match (request.check, structure) {
        (ReviewCheck::Architecture, Some(view)) => {
            let flagged: Vec<(String, String)> = findings
                .iter()
                .flat_map(|f| f.edges.iter().cloned())
                .collect();
            Some(view.diagram(&flagged))
        }
        _ => None,
    };
    let summary = raw
        .summary
        .map(|s| visible_text(&s.trim().chars().take(MAX_FIELD_CHARS).collect::<String>()))
        .unwrap_or_default();
    Some(ReviewReport {
        check: request.check,
        summary,
        findings,
        diagram,
        dropped,
    })
}
