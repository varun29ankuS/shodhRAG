//! Line diff of two instruction versions (longest common subsequence), for the history
//! view and for the approval of an instruction edit proposed by the assistant.

use serde::Serialize;

/// What happened to a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffOp {
    Same,
    Added,
    Removed,
}

/// One line of a diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    pub op: DiffOp,
    pub text: String,
}

/// Lines of `before` and `after` as kept, removed and added, in order (removals before
/// additions where a block changed). Instructions are capped at a few thousand
/// characters, so the quadratic table stays small.
pub fn line_diff(before: &str, after: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = if before.is_empty() {
        Vec::new()
    } else {
        before.split('\n').collect()
    };
    let b: Vec<&str> = if after.is_empty() {
        Vec::new()
    } else {
        after.split('\n').collect()
    };
    // lcs[i][j]: length of the LCS of a[i..] and b[j..].
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let line = |op, text: &str| DiffLine {
        op,
        text: text.to_string(),
    };
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(a.len().max(b.len()));
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push(line(DiffOp::Same, a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(line(DiffOp::Removed, a[i]));
            i += 1;
        } else {
            out.push(line(DiffOp::Added, b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|t| line(DiffOp::Removed, t)));
    out.extend(b[j..].iter().map(|t| line(DiffOp::Added, t)));
    out
}

/// The diff as unified-style text (`+ `, `- `, `  ` prefixes), for an approval preview.
pub fn diff_text(lines: &[DiffLine]) -> String {
    lines
        .iter()
        .map(|l| {
            let mark = match l.op {
                DiffOp::Same => "  ",
                DiffOp::Added => "+ ",
                DiffOp::Removed => "- ",
            };
            format!("{mark}{}", l.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
