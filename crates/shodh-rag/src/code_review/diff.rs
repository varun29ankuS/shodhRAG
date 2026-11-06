//! Line differences between two versions of a file: the one diff Shodh shows,
//! in the Artifacts tab's versions, Code mode's edit and write approvals, the
//! GitHub push prompt and the code review pane.
//!
//! Myers' O(ND) algorithm on lines, grouped into hunks with a few lines of
//! context. Files past [`MAX_DIFF_LINES`] lines, or whose difference needs
//! more than [`MAX_EDIT_DISTANCE`] changed lines, are reported as too large
//! to compare rather than diffed slowly. Git's own unified diffs (a push)
//! are read into the same shape ([`parse_unified`]).

use serde::Serialize;

/// Lines of either side past which a file is not compared line by line.
pub const MAX_DIFF_LINES: usize = 5_000;
/// Changed lines past which the comparison stops (the file is "too large").
pub const MAX_EDIT_DISTANCE: usize = 1_000;
/// Unchanged lines shown around each change.
pub const CONTEXT_LINES: usize = 3;

/// One line of a hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    /// `same`, `added` or `removed`.
    pub kind: &'static str,
    pub text: String,
    /// Its number in the old version (not for an added line), from 1.
    pub old: Option<usize>,
    /// Its number in the new version (not for a removed line), from 1.
    pub new: Option<usize>,
}

/// A run of changes with its context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub lines: Vec<DiffLine>,
}

/// One edit of the script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    Same(usize, usize),
    Removed(usize),
    Added(usize),
}

/// The shortest edit script from `a` to `b`, or `None` when it needs more
/// than `max` changed lines.
fn edit_script(a: &[&str], b: &[&str], max: usize) -> Option<Vec<Edit>> {
    let (n, m) = (a.len(), b.len());
    let offset = n + m + 1;
    let mut v = vec![0usize; 2 * offset + 1];
    // Before round d, the furthest x of each diagonal k in -d..=d (what
    // round d - 1 reached): all the walk back needs, about d² values in all.
    let mut trace: Vec<Vec<usize>> = Vec::new();
    let limit = (n + m).min(max);
    let mut found = None;
    for d in 0..=limit {
        trace.push(v[offset - d..=offset + d].to_vec());
        let d = d as isize;
        let mut k = -d;
        while k <= d {
            let index = (k + offset as isize) as usize;
            let mut x = if k == -d || (k != d && v[index - 1] < v[index + 1]) {
                v[index + 1]
            } else {
                v[index - 1] + 1
            };
            let mut y = (x as isize - k) as usize;
            while x < n && y < m && a[x] == b[y] {
                x += 1;
                y += 1;
            }
            v[index] = x;
            if x >= n && y >= m {
                found = Some(d as usize);
                break;
            }
            k += 2;
        }
        if found.is_some() {
            break;
        }
    }
    let depth = found?;
    // Walk the trace back from the end.
    let mut edits = Vec::new();
    let (mut x, mut y) = (n as isize, m as isize);
    for d in (0..=depth).rev() {
        if d == 0 {
            // What is left from (0, 0) is one diagonal: lines both sides share.
            while x > 0 && y > 0 {
                x -= 1;
                y -= 1;
                edits.push(Edit::Same(x as usize, y as usize));
            }
            break;
        }
        let v = &trace[d];
        let d = d as isize;
        let k = x - y;
        let index = |k: isize| (k + d) as usize;
        let previous_k = if k == -d || (k != d && v[index(k - 1)] < v[index(k + 1)]) {
            k + 1
        } else {
            k - 1
        };
        let previous_x = v[index(previous_k)] as isize;
        let previous_y = previous_x - previous_k;
        while x > previous_x && y > previous_y {
            x -= 1;
            y -= 1;
            edits.push(Edit::Same(x as usize, y as usize));
        }
        if x == previous_x {
            y -= 1;
            edits.push(Edit::Added(y as usize));
        } else {
            x -= 1;
            edits.push(Edit::Removed(x as usize));
        }
    }
    edits.reverse();
    Some(edits)
}

/// The hunks between `old` and `new` (text split into lines), or `None`
/// when the files are too large to compare ([`MAX_DIFF_LINES`],
/// [`MAX_EDIT_DISTANCE`]). Equal texts give no hunk.
pub fn hunks(old: &str, new: &str) -> Option<Vec<Hunk>> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len() > MAX_DIFF_LINES || b.len() > MAX_DIFF_LINES {
        return None;
    }
    let edits = edit_script(&a, &b, MAX_EDIT_DISTANCE)?;
    let changed: Vec<usize> = edits
        .iter()
        .enumerate()
        .filter(|(_, edit)| !matches!(edit, Edit::Same(..)))
        .map(|(at, _)| at)
        .collect();
    let mut out: Vec<Hunk> = Vec::new();
    let mut at = 0;
    while at < changed.len() {
        let start = changed[at].saturating_sub(CONTEXT_LINES);
        let mut end = changed[at];
        // Changes closer than twice the context share one hunk.
        while at + 1 < changed.len() && changed[at + 1] <= end + 2 * CONTEXT_LINES + 1 {
            at += 1;
            end = changed[at];
        }
        let end = (end + CONTEXT_LINES).min(edits.len() - 1);
        let lines = edits[start..=end]
            .iter()
            .map(|edit| match *edit {
                Edit::Same(x, y) => DiffLine {
                    kind: "same",
                    text: a[x].to_string(),
                    old: Some(x + 1),
                    new: Some(y + 1),
                },
                Edit::Removed(x) => DiffLine {
                    kind: "removed",
                    text: a[x].to_string(),
                    old: Some(x + 1),
                    new: None,
                },
                Edit::Added(y) => DiffLine {
                    kind: "added",
                    text: b[y].to_string(),
                    old: None,
                    new: Some(y + 1),
                },
            })
            .collect();
        out.push(Hunk { lines });
        at += 1;
    }
    Some(out)
}

/// How one file differs between two versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    /// `added`, `removed` or `changed`.
    pub status: &'static str,
    pub binary: bool,
    /// Too large to compare line by line ([`hunks`]), or to show where it is
    /// shown ([`capped`]).
    pub too_large: bool,
    pub hunks: Vec<Hunk>,
}

impl FileChange {
    /// `path` from `old` (`None`: the file is new) to `new` (`None`: it was removed).
    pub fn between(path: &str, old: Option<&str>, new: Option<&str>) -> Self {
        let status = match (old, new) {
            (None, Some(_)) => "added",
            (Some(_), None) => "removed",
            _ => "changed",
        };
        let (hunks, too_large) = match hunks(old.unwrap_or(""), new.unwrap_or("")) {
            Some(found) => (found, false),
            None => (Vec::new(), true),
        };
        FileChange {
            path: path.to_string(),
            status,
            binary: false,
            too_large,
            hunks,
        }
    }

    /// The lines its hunks show.
    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|hunk| hunk.lines.len()).sum()
    }
}

/// `changes` with at most `max_lines` lines shown in all: a file whose hunks
/// would pass that is kept by name, marked too large to show here.
pub fn capped(changes: Vec<FileChange>, max_lines: usize) -> Vec<FileChange> {
    let mut used = 0usize;
    changes
        .into_iter()
        .map(|mut change| {
            let lines = change.line_count();
            if used + lines > max_lines {
                change.hunks.clear();
                change.too_large = true;
            } else {
                used += lines;
            }
            change
        })
        .collect()
}

/// A path as git writes it in a diff header: `"a/sp\303\251cial"` quoted with
/// C escapes when it has unusual characters, plain otherwise.
fn unquoted(path: &str) -> String {
    let Some(inner) = path
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return path.to_string();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buffer = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some('"') => bytes.push(b'"'),
            Some('\\') => bytes.push(b'\\'),
            Some(first @ '0'..='7') => {
                let mut value = first.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek().and_then(|d| d.to_digit(8)) {
                        Some(digit) => {
                            value = value * 8 + digit;
                            chars.next();
                        }
                        None => break,
                    }
                }
                bytes.push((value & 0xff) as u8);
            }
            Some(other) => {
                let mut buffer = [0u8; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
            }
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The path of a `--- a/x` or `+++ b/x` header, `None` for `/dev/null`.
fn header_path(rest: &str) -> Option<String> {
    let rest = rest.trim_end_matches('\t');
    if rest == "/dev/null" {
        return None;
    }
    let path = unquoted(rest);
    Some(
        path.strip_prefix("a/")
            .or_else(|| path.strip_prefix("b/"))
            .unwrap_or(&path)
            .to_string(),
    )
}

/// `-12,3` or `+12` of a hunk header: the first line and how many lines.
fn hunk_range(part: &str) -> Option<(usize, usize)> {
    let part = part.get(1..)?;
    let (start, count) = match part.split_once(',') {
        Some((start, count)) => (start.parse().ok()?, count.parse().ok()?),
        None => (part.parse().ok()?, 1),
    };
    Some((start, count))
}

/// One file being read from a unified diff.
struct Reading {
    change: FileChange,
    /// Lines of the current hunk still to come on each side.
    old_left: usize,
    new_left: usize,
    old_at: usize,
    new_at: usize,
}

impl Reading {
    fn new(header: &str) -> Self {
        // `a/x b/x`: the new side's name, for a file whose diff has no ---/+++
        // lines (binary, mode only). Replaced by the +++ line when it comes.
        let path = header
            .rsplit_once(" b/")
            .map(|(_, path)| path.to_string())
            .unwrap_or_else(|| header.to_string());
        Reading {
            change: FileChange {
                path: unquoted(&path),
                status: "changed",
                binary: false,
                too_large: false,
                hunks: Vec::new(),
            },
            old_left: 0,
            new_left: 0,
            old_at: 0,
            new_at: 0,
        }
    }

    fn in_hunk(&self) -> bool {
        self.old_left > 0 || self.new_left > 0
    }

    fn push(&mut self, kind: &'static str, text: &str) {
        let (old, new) = match kind {
            "added" => (None, Some(self.new_at)),
            "removed" => (Some(self.old_at), None),
            _ => (Some(self.old_at), Some(self.new_at)),
        };
        if kind != "added" {
            self.old_at += 1;
            self.old_left = self.old_left.saturating_sub(1);
        }
        if kind != "removed" {
            self.new_at += 1;
            self.new_left = self.new_left.saturating_sub(1);
        }
        if let Some(hunk) = self.change.hunks.last_mut() {
            hunk.lines.push(DiffLine {
                kind,
                text: text.to_string(),
                old,
                new,
            });
        }
    }
}

/// The files of git's unified diff (`git diff --no-color`), each with its
/// hunks and line numbers. Lines a hunk header does not count are headers of
/// the next file, so a removed line that reads `--- x` is never taken for one.
pub fn parse_unified(text: &str) -> Vec<FileChange> {
    let mut out = Vec::new();
    let mut current: Option<Reading> = None;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(reading) = current.as_mut().filter(|r| r.in_hunk()) {
            match line.as_bytes().first() {
                Some(b'+') => reading.push("added", &line[1..]),
                Some(b'-') => reading.push("removed", &line[1..]),
                Some(b' ') => reading.push("same", &line[1..]),
                Some(b'\\') => {}
                // A context line whose space was trimmed.
                None => reading.push("same", ""),
                Some(_) => {
                    reading.old_left = 0;
                    reading.new_left = 0;
                }
            }
            if current.as_ref().is_some_and(Reading::in_hunk) {
                continue;
            }
            if line.starts_with(['+', '-', ' ', '\\']) || line.is_empty() {
                continue;
            }
        }
        if let Some(header) = line.strip_prefix("diff --git ") {
            if let Some(done) = current.take() {
                out.push(done.change);
            }
            current = Some(Reading::new(header));
            continue;
        }
        let Some(reading) = current.as_mut() else {
            continue;
        };
        if line.starts_with("new file mode") {
            reading.change.status = "added";
        } else if line.starts_with("deleted file mode") {
            reading.change.status = "removed";
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            reading.change.binary = true;
        } else if let Some(rest) = line.strip_prefix("--- ") {
            if let Some(path) = header_path(rest) {
                reading.change.path = path;
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            if let Some(path) = header_path(rest) {
                reading.change.path = path;
            }
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            let mut parts = rest.split(' ');
            let old = parts.next().and_then(hunk_range);
            let new = parts.next().and_then(hunk_range);
            if let (Some((old_at, old_left)), Some((new_at, new_left))) = (old, new) {
                // An empty side's start is the line before it (0 for an empty file).
                reading.old_at = if old_left == 0 { old_at + 1 } else { old_at };
                reading.new_at = if new_left == 0 { new_at + 1 } else { new_at };
                reading.old_left = old_left;
                reading.new_left = new_left;
                reading.change.hunks.push(Hunk { lines: Vec::new() });
            }
        }
    }
    if let Some(done) = current.take() {
        out.push(done.change);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unified_diff_reads_into_files_hunks_and_line_numbers() {
        // Lines kept as written: a context line starts with a space.
        let text = [
            "diff --git a/src/lib.rs b/src/lib.rs",
            "index 1111111..2222222 100644",
            "--- a/src/lib.rs",
            "+++ b/src/lib.rs",
            "@@ -1,4 +1,4 @@",
            " fn main() {",
            "--- a removed line that looks like a header",
            "+    let x = 1;",
            " }",
            " ",
            "diff --git a/new.txt b/new.txt",
            "new file mode 100644",
            "--- /dev/null",
            "+++ b/new.txt",
            "@@ -0,0 +1,2 @@",
            "+one",
            "+two",
            "\\ No newline at end of file",
            "diff --git a/logo.png b/logo.png",
            "Binary files a/logo.png and b/logo.png differ",
            "diff --git a/gone.rs b/gone.rs",
            "deleted file mode 100644",
            "--- a/gone.rs",
            "+++ /dev/null",
            "@@ -1 +0,0 @@",
            "-bye",
            "",
        ]
        .join("\n");
        let files = parse_unified(&text);
        assert_eq!(files.len(), 4);
        let lib = &files[0];
        assert_eq!((lib.path.as_str(), lib.status), ("src/lib.rs", "changed"));
        let lines = &lib.hunks[0].lines;
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[1].kind, "removed");
        assert_eq!(lines[1].text, "-- a removed line that looks like a header");
        assert_eq!((lines[1].old, lines[1].new), (Some(2), None));
        assert_eq!((lines[2].old, lines[2].new), (None, Some(2)));
        assert_eq!((lines[3].old, lines[3].new), (Some(3), Some(3)));
        let added = &files[1];
        assert_eq!((added.path.as_str(), added.status), ("new.txt", "added"));
        assert_eq!(added.hunks[0].lines.len(), 2);
        assert_eq!(added.hunks[0].lines[0].new, Some(1));
        assert!(files[2].binary);
        assert_eq!(files[2].path, "logo.png");
        let gone = &files[3];
        assert_eq!((gone.path.as_str(), gone.status), ("gone.rs", "removed"));
        assert_eq!(gone.hunks[0].lines[0].old, Some(1));
    }

    #[test]
    fn quoted_paths_are_read_back() {
        assert_eq!(unquoted("\"a/caf\\303\\251.txt\""), "a/café.txt");
        assert_eq!(unquoted("\"a/tab\\there\""), "a/tab\there");
        assert_eq!(unquoted("plain/path"), "plain/path");
    }

    #[test]
    fn capped_diffs_keep_every_file_by_name() {
        let big: String = (0..30).map(|i| format!("{i}\n")).collect();
        let changes = vec![
            FileChange::between("a.txt", None, Some(&big)),
            FileChange::between("b.txt", None, Some("x\n")),
            FileChange::between("c.txt", None, Some(&big)),
        ];
        let shown = capped(changes, 32);
        assert_eq!(shown.len(), 3);
        assert_eq!(shown[0].line_count(), 30);
        assert_eq!(shown[1].line_count(), 1);
        assert!(shown[2].too_large && shown[2].hunks.is_empty());
    }

    fn kinds(hunk: &Hunk) -> String {
        hunk.lines
            .iter()
            .map(|line| match line.kind {
                "same" => ' ',
                "added" => '+',
                _ => '-',
            })
            .collect()
    }

    #[test]
    fn equal_texts_have_no_hunk() {
        assert_eq!(hunks("a\nb\n", "a\nb\n"), Some(Vec::new()));
        assert_eq!(hunks("", ""), Some(Vec::new()));
    }

    #[test]
    fn one_changed_line_shows_with_its_context() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let new = "1\n2\n3\n4\nfive\n6\n7\n8\n9\n";
        let found = hunks(old, new).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(kinds(&found[0]), "   -+   ");
        let removed = &found[0].lines[3];
        assert_eq!(
            (removed.text.as_str(), removed.old, removed.new),
            ("5", Some(5), None)
        );
        let added = &found[0].lines[4];
        assert_eq!(
            (added.text.as_str(), added.old, added.new),
            ("five", None, Some(5))
        );
    }

    #[test]
    fn far_apart_changes_are_separate_hunks_and_near_ones_merge() {
        let text = |changed: &[usize]| -> String {
            (1..=40)
                .map(|i| {
                    if changed.contains(&i) {
                        format!("changed {i}\n")
                    } else {
                        format!("{i}\n")
                    }
                })
                .collect()
        };
        let old = text(&[]);
        assert_eq!(hunks(&old, &text(&[5, 35])).unwrap().len(), 2);
        assert_eq!(hunks(&old, &text(&[5, 9])).unwrap().len(), 1);
    }

    #[test]
    fn a_new_file_is_all_added_and_a_removed_one_all_removed() {
        let added = hunks("", "a\nb\n").unwrap();
        assert_eq!(kinds(&added[0]), "++");
        let removed = hunks("a\nb\n", "").unwrap();
        assert_eq!(kinds(&removed[0]), "--");
    }

    #[test]
    fn the_script_turns_the_old_text_into_the_new_one() {
        let old = "a\nb\nc\nd\ne\nf\n";
        let new = "a\nx\nc\ne\nf\ng\n";
        let a: Vec<&str> = old.lines().collect();
        let b: Vec<&str> = new.lines().collect();
        let script = edit_script(&a, &b, MAX_EDIT_DISTANCE).unwrap();
        let rebuilt: Vec<&str> = script
            .iter()
            .filter_map(|edit| match *edit {
                Edit::Same(x, _) => Some(a[x]),
                Edit::Added(y) => Some(b[y]),
                Edit::Removed(_) => None,
            })
            .collect();
        assert_eq!(rebuilt, b);
        let changes = script
            .iter()
            .filter(|e| !matches!(e, Edit::Same(..)))
            .count();
        assert_eq!(changes, 4, "b→x is two edits, d removed and g added");
    }

    #[test]
    fn large_files_are_not_compared_line_by_line() {
        let big: String = (0..=MAX_DIFF_LINES).map(|i| format!("{i}\n")).collect();
        assert_eq!(hunks(&big, "x\n"), None);
        let a: String = (0..3_000).map(|i| format!("a{i}\n")).collect();
        let b: String = (0..3_000).map(|i| format!("b{i}\n")).collect();
        assert_eq!(hunks(&a, &b), None);
    }
}
