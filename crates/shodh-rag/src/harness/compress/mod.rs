//! Output compression for Code mode: the model's copy of a `run_command`
//! output, with lines that carry no information for it removed.
//!
//! The user always sees the command's full output (the live panel and the
//! step, from [`super::run_command::CommandOutcome::output`]), and the model
//! can read or search it too ([`super::command_output`]); only the text the
//! tool returns is shortened ([`compress`]). The rules:
//!
//! - **Only exact success noise goes.** Filters remove lines they recognise
//!   exactly: cargo's own progress (`   Compiling foo v1.0`, the status word
//!   right-aligned in twelve columns as cargo writes it), passing-test lines
//!   (`test a::b ... ok`, nextest's `PASS [`, pytest's `<node id> PASSED`,
//!   jest/vitest `✓`, go's `--- PASS:`), pytest's all-pass dot lines, go's
//!   `=== RUN` lines and git status's `(use "git add"...)` hints, and a blank
//!   line their removal left next to another. Every other line stays word for
//!   word; nothing is cut to a length or a count.
//! - **A command that did not succeed keeps its progress.** Its last
//!   `Compiling` line or `Blocking waiting for file lock` may be the only
//!   clue to a hang or a lock, so after a non-zero exit, a kill or a time
//!   limit only passing-test lines are removed ([`compress`]'s `succeeded`).
//! - **Failure sections are never filtered.** A failing test's captured
//!   output (libtest's `---- name stdout ----`, nextest's output after a
//!   `FAIL` line), output streamed with `--nocapture`, pytest's `FAILURES`
//!   and `ERRORS` sections and jest's `●` blocks keep every line.
//! - **Reversible regrouping.** `grep -n`-style matches are written under
//!   their file once (`path` then `  12:text`, nothing else changed), and
//!   file lists under their folder.
//! - **Known programs only.** Filters apply to a single recognised program
//!   (optionally after `cd <dir> &&`, environment assignments and `2>&1`).
//!   Anything else, a diff, a file, a pipe or a list, is returned unchanged.
//! - **Never worse.** When the result would not be shorter, the output is
//!   returned unchanged.
//! - **Honest.** The model is told what was removed and how to read the full
//!   output ([`note`]).
//!
//! The approach (filtering by the program that ran, dropping its success
//! lines and keeping failures, and grouping `grep` matches by file) follows
//! rtk's Apache-2.0 command filters (<https://github.com/rtk-ai/rtk>, v1.1.0,
//! `src/cmds/`). This module is Shodh's own implementation of that approach;
//! no rtk code is copied, and nothing of rtk's BUSL-1.1 engine (`engine/`)
//! is used. See NOTICE.

pub mod savings;

#[cfg(test)]
mod tests;

/// The program a command runs, as far as output compression cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    /// `cargo build`, `check`, `clippy`, `run`, `doc`, `install`, `bench`.
    Cargo,
    /// `cargo test`, `cargo nextest`.
    CargoTest,
    /// `git status` in its long format.
    GitStatus,
    /// `pytest`, `python -m pytest`.
    Pytest,
    /// `jest`, `vitest`, `npm test` and the like.
    JsTest,
    /// `go test`.
    GoTest,
    /// `grep -n`, `rg -n`, `git grep -n`: matches with line numbers.
    Search,
    /// `find`, `fd`.
    Find,
    /// Anything else, or a command Shodh does not read as one program.
    Other,
}

/// A command as compression reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    pub program: Program,
    /// Test output is streamed inline (`--nocapture`, `--show-output`,
    /// nextest's `--no-capture`): nothing but passing-test lines is removed
    /// after the tests start.
    pub streams_output: bool,
}

/// What the model gets of an output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compressed {
    /// The model's copy (the output itself when nothing changed).
    pub text: String,
    /// Lines removed.
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

/// Fewest consecutive lines of one file or folder regrouped under it.
const MIN_GROUP: usize = 2;

/// The model's copy of `output`, the output of `command`. `succeeded`: the
/// command ran and exited with 0; otherwise only passing-test lines are
/// removed.
pub fn compress(command: &str, output: &str, succeeded: bool) -> Compressed {
    compress_as(command_of(command), output, succeeded)
}

/// [`compress`] for output already known to come from `command`.
pub fn compress_as(command: Command, output: &str, succeeded: bool) -> Compressed {
    if output.trim().is_empty() {
        return Compressed::unchanged(output);
    }
    let program = command.program;
    let mut steps = Vec::new();
    let mut lines: Vec<String> = output.lines().map(str::to_string).collect();
    let mut removed = 0;
    let mut passing = 0;

    let filtered = {
        let all = lines.clone();
        match program {
            Program::Cargo | Program::CargoTest => {
                let mut cargo =
                    CargoFilter::new(program == Program::CargoTest, command.streams_output);
                Some(drop_noise(&all, succeeded, &mut |_: usize, line: &str| {
                    cargo.classify(line)
                }))
            }
            Program::GitStatus => Some(drop_noise(&all, succeeded, &mut |_: usize, line: &str| {
                git_status_line(line)
            })),
            Program::Pytest => {
                let mut pytest = PytestFilter::default();
                Some(drop_noise(&all, succeeded, &mut |_: usize, line: &str| {
                    pytest.classify(line)
                }))
            }
            Program::JsTest => {
                let mut js = JsFilter::default();
                Some(drop_noise(&all, succeeded, &mut |i: usize, line: &str| {
                    js.classify(&all, i, line)
                }))
            }
            Program::GoTest => Some(drop_noise(&all, succeeded, &mut |_: usize, line: &str| {
                go_test_line(line)
            })),
            Program::Search | Program::Find | Program::Other => None,
        }
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

/// What the model is told about its shortened copy (appended after it):
/// what was removed, that nothing else was, and how to read the full output
/// (`command_id` for [`super::command_output::READ_COMMAND_OUTPUT`]).
pub fn note(compressed: &Compressed, command_id: &str) -> String {
    let mut parts = Vec::new();
    if compressed.passing_tests > 0 {
        parts.push(format!(
            "{} passing-test line{} removed",
            compressed.passing_tests,
            plural(compressed.passing_tests)
        ));
    }
    let other = compressed
        .removed_lines
        .saturating_sub(compressed.passing_tests);
    if other > 0 {
        parts.push(format!(
            "{other} progress or hint line{} removed",
            plural(other)
        ));
    }
    if compressed
        .steps
        .iter()
        .any(|s| matches!(*s, "matches grouped by file" | "paths grouped by folder"))
    {
        parts.push("lines grouped under their file or folder (none removed)".to_string());
    }
    format!(
        "[Shodh shortened this output: {}. No other line was removed. The full output is \
         command {command_id}: read or search it with {}.]",
        parts.join("; "),
        super::command_output::READ_COMMAND_OUTPUT
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
    command_of(command).program
}

const OTHER: Command = Command {
    program: Program::Other,
    streams_output: false,
};

/// `command` as compression reads it.
pub fn command_of(command: &str) -> Command {
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
        return OTHER;
    };
    if segments
        .iter()
        .any(|segment| !is_plain_cd(segment) || COMPOUND.iter().any(|c| segment.contains(c)))
    {
        return OTHER;
    }
    if COMPOUND.iter().any(|c| last.contains(c)) {
        return OTHER;
    }
    let words: Vec<&str> = last
        .split_whitespace()
        .skip_while(|w| is_assignment(w))
        .collect();
    let program = classify(&words);
    let streams_output = program == Program::CargoTest
        && words
            .iter()
            .any(|w| matches!(*w, "--nocapture" | "--no-capture" | "--show-output"));
    Command {
        program,
        streams_output,
    }
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

/// Whether `words` ask for line numbers: `-n`, `--line-number`, or a cluster
/// of short options with `n` (`-rn`).
fn asks_line_numbers(words: &[&str]) -> bool {
    words.iter().take_while(|w| **w != "--").any(|w| {
        *w == "--line-number"
            || (w.starts_with('-')
                && !w.starts_with("--")
                && w.len() > 1
                && w[1..].chars().all(|c| c.is_ascii_alphabetic())
                && w[1..].contains('n'))
    })
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
        "grep" | "egrep" | "fgrep" | "rg" if asks_line_numbers(rest) => Program::Search,
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
    // Cargo's own options, before `--` (what follows is the program's or the
    // test binary's). `--quiet` prints no progress: a program `cargo run -q`
    // starts has no `Running` line to tell its output from cargo's.
    let own = || rest.iter().take_while(|w| **w != "--");
    if own().any(|w| matches!(*w, "-q" | "--quiet")) {
        return Program::Other;
    }
    match rest.first().copied() {
        Some("test" | "t" | "nextest") => Program::CargoTest,
        // `update` and `fetch` are left out: their `Updating`, `Locking`,
        // `Adding` and `Removing` lines are their result.
        Some(
            "build" | "b" | "check" | "c" | "clippy" | "run" | "r" | "doc" | "install" | "bench",
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
        "grep" if asks_line_numbers(args) => Program::Search,
        _ => Program::Other,
    }
}

// ── Dropping success noise ────────────────────────────────────────────────────

/// How a filter sees one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Keep,
    /// Progress or a hint: removed when the command succeeded.
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

/// `lines` without the ones `classify` (given each line's index) calls
/// passing tests, and, when the command `succeeded`, noise. The passing tests
/// removed since the last suite end are counted in a line just before the
/// next one (or at the end). A blank line that only became next to another
/// blank line because the lines between them were removed is removed too;
/// blank lines the output itself has next to each other stay.
fn drop_noise(
    lines: &[String],
    succeeded: bool,
    classify: &mut dyn FnMut(usize, &str) -> Line,
) -> Filtered {
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
    for (i, line) in lines.iter().enumerate() {
        match classify(i, line) {
            Line::Noise if succeeded => {
                removed += 1;
                gap = true;
            }
            Line::Keep | Line::Noise => {
                let blank = line.trim().is_empty();
                let after_blank = out.last().is_some_and(|l| l.trim().is_empty());
                if !(blank && after_blank && gap) {
                    out.push(line.clone());
                }
                gap = false;
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
/// `Blocking` is not one: `Blocking waiting for file lock` explains a wait.
const CARGO_PROGRESS: [&str; 10] = [
    "Compiling",
    "Checking",
    "Downloading",
    "Downloaded",
    "Updating",
    "Fresh",
    "Packaging",
    "Verifying",
    "Archiving",
    "Documenting",
];

/// Width of the field cargo (and nextest) right-align a status word in
/// (`   Compiling`, `        PASS`).
const STATUS_WIDTH: usize = 12;

/// Words that mark a line as part of a problem; a line with one is never
/// removed as progress or as a passing test.
pub(crate) fn mentions_trouble(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    [
        "error",
        "fail",
        "panic",
        "warning",
        "denied",
        "fatal",
        "abort",
        "assert",
        "cannot",
        "could not",
        "unable",
        "not found",
        "exception",
        "traceback",
        "rejected",
        "conflict",
        "timed out",
        "timeout",
    ]
    .iter()
    .any(|word| lowered.contains(word))
}

/// The status word of a line right-aligned in [`STATUS_WIDTH`] columns, and
/// the rest of the line.
fn aligned_status(line: &str) -> Option<(&str, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let (word, rest) = line[indent..].split_once(' ')?;
    (indent + word.len() == STATUS_WIDTH).then_some((word, rest))
}

/// A cargo progress line exactly as cargo writes one: one of
/// [`CARGO_PROGRESS`] right-aligned in [`STATUS_WIDTH`] columns, then what it
/// is about, with no word of trouble.
fn is_cargo_progress(line: &str) -> bool {
    aligned_status(line).is_some_and(|(word, rest)| {
        CARGO_PROGRESS.contains(&word) && !rest.trim().is_empty() && !mentions_trouble(rest)
    })
}

/// nextest's status line of a finished test (`        FAIL [   0.004s] a::b`,
/// `  TRY 2 FAIL [ ...`, `     SIGSEGV [ ...`): the word, right-aligned in
/// [`STATUS_WIDTH`] columns, before ` [`.
fn nextest_status(line: &str) -> Option<&str> {
    let (head, _) = line.split_once(" [")?;
    let indent = head.len() - head.trim_start_matches(' ').len();
    (head.len() == STATUS_WIDTH && indent > 0).then(|| head.trim_start())
}

/// cargo's output, line by line. Progress goes only where cargo itself
/// writes: never inside a failing test's captured output (libtest's `----
/// name stdout ----` up to the `failures:` list; nextest's output after a
/// failing test's status line up to the next status line), never once
/// streamed test output starts (`--nocapture`), and never after `cargo run`
/// starts the program (`Running \`target/...\``). `test` also drops passing
/// test lines.
struct CargoFilter {
    test: bool,
    streams_output: bool,
    /// Inside a failing test's captured output.
    captured: bool,
    /// Tests are running with their output streamed inline.
    streaming: bool,
    /// The program `cargo run` started is writing.
    program: bool,
}

impl CargoFilter {
    fn new(test: bool, streams_output: bool) -> Self {
        CargoFilter {
            test,
            streams_output,
            captured: false,
            streaming: false,
            program: false,
        }
    }

    fn classify(&mut self, line: &str) -> Line {
        let text = line.trim_start();
        if self.program {
            return Line::Keep;
        }
        if self.test {
            if let Some(status) = nextest_status(line) {
                // nextest: a test's output follows its status line.
                self.captured = !matches!(status, "PASS" | "START" | "SLOW" | "Summary")
                    && !status.ends_with(" PASS");
                return match status {
                    "PASS" => Line::Passed,
                    "Summary" => Line::SuiteEnd,
                    _ => Line::Keep,
                };
            }
            if text.starts_with("---- ")
                && (text.ends_with(" stdout ----") || text.ends_with(" stderr ----"))
            {
                self.captured = true;
                return Line::Keep;
            }
        }
        if self.captured {
            if line == "failures:" || text.starts_with("test result:") {
                self.captured = false;
            } else {
                return Line::Keep;
            }
        }
        if text.starts_with("Running `") && aligned_status(line).is_some() {
            self.program = true;
            return Line::Keep;
        }
        if !self.test {
            return if is_cargo_progress(line) {
                Line::Noise
            } else {
                Line::Keep
            };
        }
        if self.streams_output && text.starts_with("running ") && text.ends_with(" tests") {
            self.streaming = true;
            return Line::Keep;
        }
        // libtest: `test path::name ... ok` (also `- should panic ... ok`).
        if text.starts_with("test ") && text.ends_with(" ... ok") && !text.contains('\t') {
            return Line::Passed;
        }
        if text.starts_with("test result:") {
            self.streaming = false;
            return Line::SuiteEnd;
        }
        if !self.streaming && is_cargo_progress(line) {
            return Line::Noise;
        }
        Line::Keep
    }
}

fn git_status_line(line: &str) -> Line {
    let text = line.trim();
    if text.starts_with("(use \"git ") && text.ends_with(')') {
        Line::Noise
    } else {
        Line::Keep
    }
}

/// pytest's output, line by line. Nothing in the `FAILURES` or `ERRORS`
/// sections is touched (they run to the short test summary).
#[derive(Default)]
struct PytestFilter {
    in_failures: bool,
}

/// `<node id> PASSED` with nothing else but an optional `[ 40%]`.
fn is_pytest_pass(text: &str) -> bool {
    let mut words = text.split_whitespace();
    let (Some(node), Some("PASSED")) = (words.next(), words.next()) else {
        return false;
    };
    let rest: String = words.collect::<Vec<_>>().join(" ");
    node.contains("::") && (rest.is_empty() || is_percent_marker(&rest)) && !mentions_trouble(node)
}

/// `[ 40%]`, `[100%]`.
fn is_percent_marker(text: &str) -> bool {
    text.strip_prefix('[')
        .and_then(|t| t.strip_suffix("%]"))
        .is_some_and(|n| !n.trim().is_empty() && n.trim().chars().all(|c| c.is_ascii_digit()))
}

/// A progress line of passes and skips only: `....s.  [ 40%]` (`-q`) or
/// `tests/test_a.py ..s.  [ 40%]`.
fn is_pytest_progress(text: &str) -> bool {
    let Some(at) = text.rfind('[') else {
        return false;
    };
    if !is_percent_marker(text[at..].trim_end()) {
        return false;
    }
    let head = text[..at].trim_end();
    let marks = head.rsplit_once(' ').map_or(head, |(_, marks)| marks);
    !marks.is_empty() && marks.chars().all(|c| c == '.' || c == 's')
}

impl PytestFilter {
    fn classify(&mut self, line: &str) -> Line {
        let text = line.trim();
        let banner = text.starts_with("==") && text.ends_with("==");
        if banner && (text.contains(" FAILURES ") || text.contains(" ERRORS ")) {
            self.in_failures = true;
            return Line::Keep;
        }
        if self.in_failures {
            if banner && text.contains("short test summary info") {
                self.in_failures = false;
            }
            return Line::Keep;
        }
        if is_pytest_pass(text) {
            return Line::Passed;
        }
        if is_pytest_progress(text) {
            return Line::Noise;
        }
        // The last line: `==== 1 failed, 6 passed in 0.05s ====`, or with
        // `-q`, `1 failed, 6 passed in 0.05s`.
        let summary = text.trim_matches('=').trim();
        if (summary.contains(" passed") || summary.contains(" failed"))
            && summary.contains(" in ")
            && !summary.contains("::")
        {
            return Line::SuiteEnd;
        }
        Line::Keep
    }
}

/// jest's and vitest's output, line by line. Nothing inside a `●` block (a
/// failure, or a test file's console output) is touched; a `PASS <file>`
/// line before console output stays, so the output keeps its file.
#[derive(Default)]
struct JsFilter {
    in_block: bool,
}

impl JsFilter {
    fn classify(&mut self, lines: &[String], i: usize, line: &str) -> Line {
        let text = line.trim_start();
        let file_header = line.starts_with("PASS ") || line.starts_with("FAIL ");
        let summary = text.starts_with("Tests:")
            || text.starts_with("Tests ")
            || text.starts_with("Test Suites:")
            || text.starts_with("Test Files ");
        if file_header || summary {
            self.in_block = false;
        }
        if text.starts_with("● ") || text.starts_with("console.") {
            self.in_block = true;
        }
        if self.in_block {
            return Line::Keep;
        }
        if summary && !text.starts_with("Test Suites:") {
            return Line::SuiteEnd;
        }
        if (text.starts_with("✓ ") || text.starts_with("√ ")) && !mentions_trouble(text) {
            return Line::Passed;
        }
        if line.starts_with("PASS ") && !mentions_trouble(&line[5..]) {
            // A file whose console output follows keeps its header.
            let next = lines[i + 1..]
                .iter()
                .map(|l| l.trim_start())
                .find(|l| !l.is_empty());
            if next.is_some_and(|l| l.starts_with("● ") || l.starts_with("console.")) {
                return Line::Keep;
            }
            return Line::Noise;
        }
        Line::Keep
    }
}

fn go_test_line(line: &str) -> Line {
    let text = line.trim_start();
    // `=== RUN` gives the output under it its test; it goes only when the
    // run succeeded (a failing run keeps it).
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

/// Whether `path` looks like a file path: no blanks, a `/`, `\` or `.`, and
/// not only digits and dashes (a date).
fn looks_like_path(path: &str) -> bool {
    !path.is_empty()
        && !path.chars().any(char::is_whitespace)
        && path.contains(['/', '\\', '.'])
        && !path
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '.' | ':'))
}

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
            let path = &line[..colon];
            return looks_like_path(path)
                .then(|| (path, &line[colon + 1..digits_end], &line[digits_end + 1..]));
        }
        start = colon + 1;
    }
    None
}

/// Search output with each file's consecutive matches under its path once:
/// `path:12:text` lines become `path` then `  12:text` (the text exactly as
/// it was). `None` when no file has [`MIN_GROUP`] consecutive matches.
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
                    out.push(format!("  {number}:{text}"));
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

// ── The model's cut ───────────────────────────────────────────────────────────

/// Most characters of the cut middle kept for its failure lines.
pub const MIDDLE_FAILURE_CHARS: usize = 8_000;
/// Most lines kept after a failure line in the cut middle (its block).
const BLOCK_LINES: usize = 30;

/// A line that starts a failure block: one naming trouble, or a test's
/// captured-output header.
fn starts_failure_block(line: &str) -> bool {
    let text = line.trim_start();
    mentions_trouble(line)
        || (text.starts_with("---- ") && text.ends_with(" ----"))
        || nextest_status(line).is_some_and(|s| s != "PASS" && s != "START")
}

/// `text` with at most `head` characters from its start and `tail` from its
/// end, the middle cut. Unlike a plain cut, the lines of the middle that tell
/// of a problem (each failure line and the lines after it, up to a blank
/// line) are kept, up to [`MIDDLE_FAILURE_CHARS`]; the marker says how much
/// was cut, how many such lines were kept, how many did not fit, and, with
/// `command_id`, how to read the whole output.
pub fn cut_keeping_failures(
    text: &str,
    head: usize,
    tail: usize,
    command_id: Option<&str>,
) -> String {
    let total = text.chars().count();
    if total <= head + tail {
        return text.to_string();
    }
    let byte_at = |chars: usize| {
        text.char_indices()
            .nth(chars)
            .map_or(text.len(), |(i, _)| i)
    };
    let head_end = byte_at(head);
    let tail_start = byte_at(total - tail);
    let middle = &text[head_end..tail_start];

    let mut kept: Vec<&str> = Vec::new();
    let mut kept_chars = 0;
    let mut missed = 0;
    let mut block_left = 0;
    for line in middle.lines() {
        let starts = starts_failure_block(line);
        if starts {
            block_left = BLOCK_LINES;
        } else if line.trim().is_empty() {
            block_left = 0;
        }
        if starts || block_left > 0 {
            block_left = block_left.saturating_sub(1);
            let size = line.chars().count() + 1;
            if kept_chars + size <= MIDDLE_FAILURE_CHARS {
                kept.push(line);
                kept_chars += size;
            } else if starts {
                missed += 1;
            }
        }
    }
    let cut = total - head - tail;
    let mut out = String::with_capacity(head_end + (text.len() - tail_start) + kept_chars + 400);
    out.push_str(&text[..head_end]);
    out.push_str(&format!(
        "\n[… {cut} characters cut from the middle of the output …]\n"
    ));
    if !kept.is_empty() {
        out.push_str(&format!(
            "[{} line{} from the cut part that tell of an error, a failure or a warning, in order:]\n",
            kept.len(),
            plural(kept.len())
        ));
        for line in &kept {
            out.push_str(line);
            out.push('\n');
        }
    }
    if missed > 0 {
        out.push_str(&format!(
            "[{missed} more failure line{} of the cut part did not fit here.]\n",
            plural(missed)
        ));
    }
    if let Some(id) = command_id {
        out.push_str(&format!(
            "[The whole output is command {id}: read or search it with {}.]\n",
            super::command_output::READ_COMMAND_OUTPUT
        ));
    }
    out.push_str("[… end of the cut …]\n");
    out.push_str(&text[tail_start..]);
    out
}
