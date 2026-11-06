//! The reviewer's reply, checked: ranked findings whose citations point at
//! lines the reviewer was actually shown, fixes whose replaced text is in the
//! code, verification commands that pass the command checks, and the
//! module diagram of an Architecture review.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::request::{ReviewCheck, ReviewRequest};
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

/// Lines of a file a finding cites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub path: String,
    pub line: usize,
    pub end_line: usize,
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

fn field(value: &Value, key: &str) -> String {
    let text = value.get(key).and_then(Value::as_str).unwrap_or("").trim();
    if text.chars().count() > MAX_FIELD_CHARS {
        let kept: String = text.chars().take(MAX_FIELD_CHARS).collect();
        format!("{kept}…")
    } else {
        text.to_string()
    }
}

fn number(value: &Value, key: &str) -> Option<usize> {
    let v = value.get(key)?;
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .and_then(|n| usize::try_from(n).ok())
}

/// The lines the reviewer was shown, by path.
struct Shown<'a> {
    lines: HashMap<&'a str, BTreeSet<usize>>,
    text: HashMap<&'a str, String>,
    structure: Option<&'a StructureView>,
}

impl<'a> Shown<'a> {
    fn new(request: &'a ReviewRequest, structure: Option<&'a StructureView>) -> Self {
        let mut lines: HashMap<&str, BTreeSet<usize>> = HashMap::new();
        let mut text: HashMap<&str, String> = HashMap::new();
        for piece in &request.pieces {
            let entry = lines.entry(piece.path.as_str()).or_default();
            let body = text.entry(piece.path.as_str()).or_default();
            for line in &piece.lines {
                entry.insert(line.number);
                body.push_str(&line.text);
                body.push('\n');
            }
        }
        Shown {
            lines,
            text,
            structure,
        }
    }

    /// The citation, when its path and lines are ones the reviewer saw:
    /// lines of a piece, or, in an Architecture review, lines of a file of
    /// the structural view.
    fn citation(&self, path: &str, line: usize, end: usize) -> Option<Citation> {
        let path = path.trim().trim_start_matches("./").replace('\\', "/");
        let (start, end) = (line.min(end), line.max(end));
        if start == 0 {
            return None;
        }
        let ok = match self.lines.get(path.as_str()) {
            Some(shown) => shown.contains(&start) && shown.contains(&end),
            None => self
                .structure
                .and_then(|view| view.file_lines(&path))
                .is_some_and(|count| end <= count.max(1)),
        };
        ok.then_some(Citation {
            path,
            line: start,
            end_line: end,
        })
    }

    fn contains(&self, path: &str, old: &str) -> bool {
        !old.is_empty() && self.text.get(path).is_some_and(|text| text.contains(old))
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
            shown.citation(&path, line, end)
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
        .map(|s| s.trim().chars().take(MAX_FIELD_CHARS).collect::<String>())
        .unwrap_or_default();
    Some(ReviewReport {
        check: request.check,
        summary,
        findings,
        diagram,
        dropped,
    })
}
