//! Output compression for Code mode: the model's copy of a `run_command`
//! output, with the noise that carries no information for it removed.
//!
//! The user always sees the command's full output (the live panel and the
//! step, from [`super::run_command::CommandOutcome::output`]); only the text
//! the model gets back is shortened ([`compress`]). The rules:
//!
//! - **Failures are kept whole.** Filters remove only lines they recognise
//!   exactly as success noise: cargo's own progress (`   Compiling foo v1.0`,
//!   right-aligned as cargo writes it, never inside a test's captured output
//!   or a program `cargo run` started), passing test lines (`test a::b ...
//!   ok`, `✓ renders`, `--- PASS: TestX`), git's transfer progress and its
//!   `(use "git add"...)` hints, and a blank line their removal left next to
//!   another. Every other line, and so every error, warning, failing test,
//!   panic and stack trace, stays word for word. Nothing is cut to a length
//!   or a count.
//! - **Reversible regrouping.** Search and file-list output is regrouped by
//!   file or folder (the path written once), and a run of identical lines is
//!   written once with how many more there were: no information is lost.
//! - **Never worse.** When the result would not be shorter, the output is
//!   returned unchanged.
//! - **Known programs only.** Command-specific filters apply only to a single
//!   recognised program (optionally after `cd <dir> &&` and environment
//!   assignments, with `2>&1`); any other command, pipes, lists and
//!   redirections included, gets only the lossless repeat step, since
//!   `cargo test | tail` is not cargo's output and a diff's blank and
//!   look-alike lines are content.
//! - **Honest.** The model is told what was removed and that the user sees
//!   the full output ([`note`]).
//!
//! The command-aware design (one filter per program, success noise dropped,
//! a never-worse guard) follows the Apache-2.0 command filters of rtk
//! (<https://github.com/rtk-ai/rtk>, v1.1.0, `src/cmds/**`): `cargo_cmd.rs`,
//! `git_cmd.rs`, `pytest_cmd.rs`, `vitest_cmd.rs`, `go_cmd.rs`, `grep` and
//! `find` grouping. Modified for Shodh: rewritten in this module without
//! regular expressions, and rtk's caps and truncation of failures (at most N
//! failures, 200 characters each, a tail of 5 lines) are left out, so that
//! failures are never shortened. rtk's BUSL-1.1 compression engine
//! (`engine/`) is not used. See NOTICE.

pub mod savings;

#[cfg(test)]
mod tests;

/// The program a command runs, as far as output compression cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    /// `cargo build`, `check`, `clippy`, `run`, `doc`, `install`...
    Cargo,
    /// `cargo test`, `cargo nextest`.
    CargoTest,
    /// `git status` in its long format.
    GitStatus,
    /// `git push`, `pull`, `fetch`, `clone`.
    GitTransfer,
    /// `pytest`, `python -m pytest`.
    Pytest,
    /// `jest`, `vitest`, `npm test` and the like.
    JsTest,
    /// `go test`.
    GoTest,
    /// `grep`, `rg`, `git grep`.
    Search,
    /// `find`, `fd`.
    Find,
    /// Anything else, or a command Shodh does not read as one program.
    Other,
}

/// What the model gets of an output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compressed {
    /// The model's copy (the output itself when nothing changed).
    pub text: String,
    /// Noise lines removed.
    pub removed_lines: usize,
    /// Passing tests whose lines were removed (counted in `removed_lines`).
    pub passing_tests: usize,
    /// The steps that changed the output, in order.
    pub steps: Vec<&'static str>,
}

impl Compressed {
    fn unchanged(output: &str) -> Self {
        Compressed {
            text: output.to_string(),
            removed_lines: 0,
            passing_tests: 0,
            steps: Vec::new(),
        }
    }

    /// Whether the model's copy differs from the output.
    pub fn changed(&self) -> bool {
        !self.steps.is_empty()
    }
}

/// Fewest consecutive identical lines written once with a count.
const MIN_REPEAT_RUN: usize = 3;
/// Fewest consecutive lines of one file or folder regrouped under it.
const MIN_GROUP: usize = 2;

/// The model's copy of `output`, the output of `command`.
pub fn compress(command: &str, output: &str) -> Compressed {
    compress_as(program_of(command), output)
}

/// [`compress`] for output already known to come from `program`.
pub fn compress_as(program: Program, output: &str) -> Compressed {
    if output.trim().is_empty() {
        return Compressed::unchanged(output);
    }
    let mut steps = Vec::new();
    // Lines as they are (trailing spaces too: a diff's whitespace is content).
    let mut lines: Vec<String> = output.lines().map(str::to_string).collect();

    let mut removed = 0;
    let mut passing = 0;
    let filtered = match program {
        Program::Cargo | Program::CargoTest => {
            let mut cargo = CargoFilter::new(program == Program::CargoTest);
            Some(drop_noise(&lines, &mut |line| cargo.classify(line)))
        }
        Program::GitStatus => Some(drop_noise(&lines, &mut git_status_line)),
        Program::GitTransfer => Some(drop_noise(&lines, &mut git_transfer_line)),
        Program::Pytest => Some(drop_noise(&lines, &mut pytest_line)),
        Program::JsTest => Some(drop_noise(&lines, &mut js_test_line)),
        Program::GoTest => Some(drop_noise(&lines, &mut go_test_line)),
        Program::Search | Program::Find | Program::Other => None,
    };
    if let Some(filtered) = filtered {
        if filtered.removed > 0 {
            removed += filtered.removed;
            passing += filtered.passing;
            lines = filtered.lines;
            steps.push(program_step(program));
        }
    }
    let regrouped = match program {
        Program::Search => group_search(&lines),
        Program::Find => group_paths(&lines),
        _ => None,
    };
    if let Some(regrouped) = regrouped {
        lines = regrouped;
        steps.push(if program == Program::Search {
            "matches grouped by file"
        } else {
            "paths grouped by folder"
        });
    }

    // The one generic step, for every command: it loses nothing (the line
    // and how many times it repeated are both kept).
    if let Some(collapsed) = collapse_repeats(&lines) {
        lines = collapsed;
        steps.push("repeated lines");
    }

    if steps.is_empty() {
        return Compressed::unchanged(output);
    }
    let mut text = lines.join("\n");
    if output.ends_with('\n') {
        text.push('\n');
    }
    // Never worse: an output this cannot shorten goes as it is.
    if text.chars().count() >= output.chars().count() {
        return Compressed::unchanged(output);
    }
    Compressed {
        text,
        removed_lines: removed,
        passing_tests: passing,
        steps,
    }
}

/// What the model is told about its shortened copy (appended after it).
pub fn note(compressed: &Compressed) -> String {
    let mut parts = Vec::new();
    if compressed.passing_tests > 0 {
        parts.push(format!(
            "{} passing test line{} not listed",
            compressed.passing_tests,
            plural(compressed.passing_tests)
        ));
    }
    let other = compressed
        .removed_lines
        .saturating_sub(compressed.passing_tests);
    if other > 0 {
        parts.push(format!(
            "{other} progress or status line{} removed",
            plural(other)
        ));
    }
    let regrouped = compressed.steps.iter().any(|s| {
        matches!(
            *s,
            "matches grouped by file" | "paths grouped by folder" | "repeated lines"
        )
    });
    if regrouped {
        parts.push("matching lines grouped (nothing dropped)".to_string());
    }
    if parts.is_empty() {
        parts.push("nothing dropped".to_string());
    }
    format!(
        "[Shodh shortened this output for you: {}. Every error, warning and failing test is \
         kept word for word. The user sees the full output.]",
        parts.join("; ")
    )
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn program_step(program: Program) -> &'static str {
    match program {
        Program::Cargo => "cargo progress",
        Program::CargoTest => "cargo progress and passing tests",
        Program::GitStatus => "git hints",
        Program::GitTransfer => "git transfer progress",
        Program::Pytest | Program::JsTest | Program::GoTest => "passing tests",
        Program::Search | Program::Find | Program::Other => "noise",
    }
}

// ── Recognising the program ───────────────────────────────────────────────────

/// Characters that make a command more than one program (pipes, lists,
/// redirections, substitutions, background jobs).
const COMPOUND: [&str; 9] = ["|", ";", "&", ">", "<", "`", "$(", "\n", "\r"];

/// The program `command` runs, [`Program::Other`] for anything Shodh does
/// not read as a single recognised program.
pub fn program_of(command: &str) -> Program {
    let mut command = command.trim();
    // `2>&1` only merges stderr into the output the tool already merges.
    for suffix in ["2>&1", "2> &1"] {
        if let Some(rest) = command.strip_suffix(suffix) {
            command = rest.trim_end();
        }
    }
    // Leading `cd <dir> &&` steps print nothing on success.
    let mut segments: Vec<&str> = command.split("&&").map(str::trim).collect();
    let Some(last) = segments.pop() else {
        return Program::Other;
    };
    if segments
        .iter()
        .any(|segment| !is_plain_cd(segment) || COMPOUND.iter().any(|c| segment.contains(c)))
    {
        return Program::Other;
    }
    if COMPOUND.iter().any(|c| last.contains(c)) {
        return Program::Other;
    }
    let words: Vec<&str> = last
        .split_whitespace()
        .skip_while(|w| is_assignment(w))
        .collect();
    classify(&words)
}

/// `cd <one argument>`.
fn is_plain_cd(segment: &str) -> bool {
    let words: Vec<&str> = segment.split_whitespace().collect();
    matches!(words.as_slice(), ["cd", _])
}

/// `NAME=value` before a command.
fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

/// The program's name without a folder or `.exe`.
fn program_name(word: &str) -> &str {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word);
    name.strip_suffix(".exe").unwrap_or(name)
}

fn classify(words: &[&str]) -> Program {
    let Some(first) = words.first() else {
        return Program::Other;
    };
    let rest = &words[1..];
    match program_name(first) {
        "cargo" => cargo_program(rest),
        "git" => git_program(rest),
        "pytest" | "py.test" => Program::Pytest,
        "python" | "python3" | "py" => match rest {
            ["-m", "pytest", ..] => Program::Pytest,
            _ => Program::Other,
        },
        "uv" | "poetry" | "pipenv" => match rest {
            ["run", tool, ..] if matches!(program_name(tool), "pytest" | "py.test") => {
                Program::Pytest
            }
            _ => Program::Other,
        },
        "jest" | "vitest" => Program::JsTest,
        "npx" | "bunx" | "pnpx" => match rest.iter().find(|w| !w.starts_with('-')) {
            Some(tool) if matches!(program_name(tool), "jest" | "vitest") => Program::JsTest,
            _ => Program::Other,
        },
        "npm" | "pnpm" | "yarn" | "bun" => match rest {
            ["test", ..] | ["t", ..] | ["run", "test", ..] => Program::JsTest,
            [tool, ..] if matches!(*tool, "jest" | "vitest") => Program::JsTest,
            _ => Program::Other,
        },
        "go" => match rest {
            ["test", ..] => Program::GoTest,
            _ => Program::Other,
        },
        "grep" | "egrep" | "fgrep" | "rg" => Program::Search,
        "find" | "fd" | "fdfind" => Program::Find,
        _ => Program::Other,
    }
}

fn cargo_program(rest: &[&str]) -> Program {
    // `cargo +nightly test`: the toolchain comes first.
    let rest = match rest.first() {
        Some(word) if word.starts_with('+') => &rest[1..],
        _ => rest,
    };
    match rest.first().copied() {
        Some("test" | "t" | "nextest") => Program::CargoTest,
        Some(
            "build" | "b" | "check" | "c" | "clippy" | "run" | "r" | "doc" | "install" | "bench"
            | "fetch" | "update",
        ) => Program::Cargo,
        _ => Program::Other,
    }
}

fn git_program(rest: &[&str]) -> Program {
    // Global options before the subcommand: `-C <dir>`, `-c <name=value>`.
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "-C" | "-c" => i += 2,
            word if word.starts_with("--") || word == "-P" => i += 1,
            _ => break,
        }
    }
    let Some(sub) = rest.get(i) else {
        return Program::Other;
    };
    let args = &rest[i + 1..];
    match *sub {
        // The short and machine formats have no hints to drop.
        "status"
            if !args.iter().any(|a| {
                matches!(*a, "-s" | "--short" | "--porcelain" | "-z")
                    || a.starts_with("--porcelain=")
            }) =>
        {
            Program::GitStatus
        }
        "push" | "pull" | "fetch" | "clone" => Program::GitTransfer,
        "grep" => Program::Search,
        _ => Program::Other,
    }
}

// ── Dropping success noise ────────────────────────────────────────────────────

/// How a filter sees one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Keep,
    /// Progress or a hint: removed.
    Noise,
    /// A passing test: removed and counted.
    Passed,
    /// The end of a test suite's results: the count of the passing tests
    /// removed before it is written just above it.
    SuiteEnd,
}

struct Filtered {
    lines: Vec<String>,
    removed: usize,
    passing: usize,
}

/// `lines` without the ones `classify` calls noise or passing tests. The
/// passing tests removed since the last suite end are counted in a line just
/// before the next one (or at the end). A blank line that only became next
/// to another blank line because the lines between them were removed is
/// removed too; blank lines the output itself has next to each other stay.
fn drop_noise(lines: &[String], classify: &mut dyn FnMut(&str) -> Line) -> Filtered {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut removed = 0;
    let mut passing = 0;
    let mut pending = 0;
    // A line was removed since the last one kept.
    let mut gap = false;
    let flush = |out: &mut Vec<String>, pending: &mut usize| {
        if *pending > 0 {
            out.push(format!(
                "[{} passing test{} not listed]",
                *pending,
                plural(*pending)
            ));
            *pending = 0;
        }
    };
    for line in lines {
        match classify(line) {
            Line::Keep => {
                let blank = line.trim().is_empty();
                let after_blank = out.last().is_some_and(|l| l.trim().is_empty());
                if !(blank && after_blank && gap) {
                    out.push(line.clone());
                }
                gap = false;
            }
            Line::Noise => {
                removed += 1;
                gap = true;
            }
            Line::Passed => {
                removed += 1;
                passing += 1;
                pending += 1;
                gap = true;
            }
            Line::SuiteEnd => {
                flush(&mut out, &mut pending);
                out.push(line.clone());
                gap = false;
            }
        }
    }
    flush(&mut out, &mut pending);
    Filtered {
        lines: out,
        removed,
        passing,
    }
}

/// Cargo's status words for progress (the right-aligned first word).
const CARGO_PROGRESS: [&str; 16] = [
    "Compiling",
    "Checking",
    "Downloading",
    "Downloaded",
    "Updating",
    "Locking",
    "Adding",
    "Fresh",
    "Packaging",
    "Verifying",
    "Archiving",
    "Unpacking",
    "Documenting",
    "Blocking",
    "Removing",
    "Unchanged",
];

/// Width of the field cargo right-aligns its status word in
/// (`   Compiling`, `  Downloaded`).
const CARGO_STATUS_WIDTH: usize = 12;

/// A cargo progress line exactly as cargo writes one: one of
/// [`CARGO_PROGRESS`] right-aligned in [`CARGO_STATUS_WIDTH`] columns, then a
/// space and what it is about, with no word of trouble. A program's own line
/// that merely starts with one of those words is not one.
fn is_cargo_progress(line: &str) -> bool {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let text = &line[indent..];
    let Some((word, rest)) = text.split_once(' ') else {
        return false;
    };
    indent + word.len() == CARGO_STATUS_WIDTH
        && CARGO_PROGRESS.contains(&word)
        && !rest.trim().is_empty()
        && !mentions_trouble(rest)
}

/// cargo's output, read line by line. Progress is dropped only where cargo
/// itself writes: never inside a test's captured output (`---- name stdout
/// ----` up to the `failures:` list), and never after `cargo run` starts the
/// program (`Running \`target/...\``), whose own output follows. `test`
/// also drops passing test lines.
struct CargoFilter {
    test: bool,
    /// Inside a failing test's captured output.
    captured: bool,
    /// The program `cargo run` started is writing.
    program: bool,
}

impl CargoFilter {
    fn new(test: bool) -> Self {
        CargoFilter {
            test,
            captured: false,
            program: false,
        }
    }

    fn classify(&mut self, line: &str) -> Line {
        let text = line.trim_start();
        if self.program {
            return Line::Keep;
        }
        if text.starts_with("---- ")
            && (text.ends_with(" stdout ----") || text.ends_with(" stderr ----"))
        {
            self.captured = true;
            return Line::Keep;
        }
        if self.captured {
            if line == "failures:" || text.starts_with("test result:") {
                self.captured = false;
            } else {
                return Line::Keep;
            }
        }
        if text.starts_with("Running `")
            && line.len() - text.len() + "Running".len() == CARGO_STATUS_WIDTH
        {
            self.program = true;
            return Line::Keep;
        }
        if is_cargo_progress(line) {
            return Line::Noise;
        }
        if !self.test {
            return Line::Keep;
        }
        // libtest: `test path::name ... ok` (also `- should panic ... ok`).
        if text.starts_with("test ") && text.ends_with(" ... ok") {
            return Line::Passed;
        }
        // nextest: `PASS [   0.004s] crate::test`.
        if text.starts_with("PASS [") {
            return Line::Passed;
        }
        if text.starts_with("test result:") || text.starts_with("Summary [") {
            return Line::SuiteEnd;
        }
        Line::Keep
    }
}

/// Words that mark a line as part of a problem; a line with one is never
/// removed as progress.
fn mentions_trouble(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    [
        "error",
        "fail",
        "panic",
        "warning",
        "denied",
        "fatal",
        "abort",
        "cannot",
        "could not",
        "unable",
        "not found",
        "exception",
        "traceback",
        "rejected",
        "conflict",
    ]
    .iter()
    .any(|word| lowered.contains(word))
}

fn git_status_line(line: &str) -> Line {
    let text = line.trim();
    if text.starts_with("(use \"git ") && text.ends_with(')') {
        Line::Noise
    } else {
        Line::Keep
    }
}

/// git's transfer progress (also with `remote: `).
const GIT_PROGRESS: [&str; 11] = [
    "Enumerating objects:",
    "Counting objects:",
    "Compressing objects:",
    "Writing objects:",
    "Receiving objects:",
    "Resolving deltas:",
    "Unpacking objects:",
    "Updating files:",
    "Checking out files:",
    "Delta compression using up to",
    "Total ",
];

fn git_transfer_line(line: &str) -> Line {
    let text = line.trim_start();
    let text = text
        .strip_prefix("remote:")
        .map(str::trim_start)
        .unwrap_or(text);
    if GIT_PROGRESS.iter().any(|p| text.starts_with(p)) && !mentions_trouble(text) {
        Line::Noise
    } else {
        Line::Keep
    }
}

fn pytest_line(line: &str) -> Line {
    let text = line.trim();
    // `-v`: `tests/test_a.py::test_x PASSED   [ 50%]`.
    if text.contains("::") && text.contains(" PASSED") && !mentions_trouble(text) {
        return Line::Passed;
    }
    // Progress: `tests/test_a.py ..s.   [ 40%]`, only dots and skips.
    if let Some((head, tail)) = text.rsplit_once('[') {
        if tail.ends_with("%]") {
            if let Some((_, marks)) = head.trim_end().rsplit_once(' ') {
                if !marks.is_empty() && marks.chars().all(|c| c == '.' || c == 's') {
                    return Line::Noise;
                }
            }
        }
    }
    if text.starts_with("===") && text.contains(" passed") {
        return Line::SuiteEnd;
    }
    Line::Keep
}

fn js_test_line(line: &str) -> Line {
    let text = line.trim_start();
    // Passing tests (jest, vitest; `√` where the console has no `✓`).
    if (text.starts_with("✓ ") || text.starts_with("√ ")) && !mentions_trouble(text) {
        return Line::Passed;
    }
    // jest: a test file whose tests all passed.
    if text.starts_with("PASS ") && !mentions_trouble(&text[5..]) {
        return Line::Noise;
    }
    if text.starts_with("Tests:") || text.starts_with("Tests ") || text.starts_with("Test Files ") {
        return Line::SuiteEnd;
    }
    Line::Keep
}

fn go_test_line(line: &str) -> Line {
    let text = line.trim_start();
    if text.starts_with("=== RUN ")
        || text.starts_with("=== PAUSE ")
        || text.starts_with("=== CONT ")
    {
        return Line::Noise;
    }
    if text.starts_with("--- PASS: ") {
        return Line::Passed;
    }
    if text == "PASS" || text.starts_with("ok ") || text.starts_with("ok\t") {
        return Line::SuiteEnd;
    }
    Line::Keep
}

// ── Regrouping (lossless) ─────────────────────────────────────────────────────

/// `path:line:text` of a search match.
fn search_match(line: &str) -> Option<(&str, &str, &str)> {
    let bytes = line.as_bytes();
    let mut start = 0;
    while let Some(offset) = line[start..].find(':') {
        let colon = start + offset;
        let digits_end = line[colon + 1..]
            .find(|c: char| !c.is_ascii_digit())
            .map(|n| colon + 1 + n)?;
        if digits_end > colon + 1 && bytes.get(digits_end) == Some(&b':') && colon > 0 {
            return Some((
                &line[..colon],
                &line[colon + 1..digits_end],
                &line[digits_end + 1..],
            ));
        }
        start = colon + 1;
    }
    None
}

/// Search output with each file's consecutive matches under its path once:
/// `path:12:text` lines become `path` then `  12: text`. `None` when no file
/// has [`MIN_GROUP`] consecutive matches.
fn group_search(lines: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::with_capacity(lines.len());
    let mut grouped = false;
    let mut i = 0;
    while i < lines.len() {
        let Some((path, _, _)) = search_match(&lines[i]) else {
            out.push(lines[i].clone());
            i += 1;
            continue;
        };
        let mut end = i + 1;
        while end < lines.len() && search_match(&lines[end]).is_some_and(|(p, _, _)| p == path) {
            end += 1;
        }
        if end - i >= MIN_GROUP {
            grouped = true;
            out.push(path.to_string());
            for line in &lines[i..end] {
                if let Some((_, number, text)) = search_match(line) {
                    out.push(format!("  {number}: {text}"));
                }
            }
        } else {
            out.extend(lines[i..end].iter().cloned());
        }
        i = end;
    }
    grouped.then_some(out)
}

/// The folder and name of a listed path (`src/a.rs` → `src/`, `a.rs`).
fn split_path(line: &str) -> Option<(&str, &str)> {
    if line.trim() != line || line.is_empty() {
        return None;
    }
    let at = line.rfind('/')?;
    let (dir, name) = line.split_at(at + 1);
    (!name.is_empty()).then_some((dir, name))
}

/// A file list with each folder's consecutive entries under the folder once.
/// `None` when no folder has [`MIN_GROUP`] consecutive entries.
fn group_paths(lines: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::with_capacity(lines.len());
    let mut grouped = false;
    let mut i = 0;
    while i < lines.len() {
        let Some((dir, _)) = split_path(&lines[i]) else {
            out.push(lines[i].clone());
            i += 1;
            continue;
        };
        let mut end = i + 1;
        while end < lines.len() && split_path(&lines[end]).is_some_and(|(d, _)| d == dir) {
            end += 1;
        }
        if end - i >= MIN_GROUP {
            grouped = true;
            out.push(dir.to_string());
            for line in &lines[i..end] {
                if let Some((_, name)) = split_path(line) {
                    out.push(format!("  {name}"));
                }
            }
        } else {
            out.extend(lines[i..end].iter().cloned());
        }
        i = end;
    }
    grouped.then_some(out)
}

// ── Generic steps ─────────────────────────────────────────────────────────────

/// Runs of at least [`MIN_REPEAT_RUN`] identical non-blank lines, written
/// once with how many more there were. `None` when there is none.
fn collapse_repeats(lines: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::with_capacity(lines.len());
    let mut changed = false;
    let mut i = 0;
    while i < lines.len() {
        let mut end = i + 1;
        while end < lines.len() && lines[end] == lines[i] {
            end += 1;
        }
        let run = end - i;
        if run >= MIN_REPEAT_RUN && !lines[i].trim().is_empty() {
            changed = true;
            out.push(lines[i].clone());
            out.push(format!("[the line above repeated {} more times]", run - 1));
        } else {
            out.extend(lines[i..end].iter().cloned());
        }
        i = end;
    }
    changed.then_some(out)
}
