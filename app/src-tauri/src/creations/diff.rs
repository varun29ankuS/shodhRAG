//! Line differences between two versions of a file, for the Artifacts tab.
//!
//! Myers' O(ND) algorithm on lines, grouped into hunks with a few lines of
//! context. Files past [`MAX_DIFF_LINES`] lines, or whose difference needs
//! more than [`MAX_EDIT_DISTANCE`] changed lines, are reported as too large
//! to compare rather than diffed slowly.

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

#[cfg(test)]
mod tests {
    use super::*;

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
