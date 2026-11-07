use super::*;

/// Every line of `output` that tells of a problem is in `text` word for word.
fn assert_failures_kept(output: &str, text: &str) {
    for line in output.lines() {
        // A passing test line may name an error (`test errors_are_plain ... ok`).
        if line.trim_start().starts_with("test ") && line.ends_with(" ... ok") {
            continue;
        }
        let lowered = line.to_ascii_lowercase();
        let trouble = [
            "error",
            "fail",
            "panic",
            "warning",
            "assert",
            "expected",
            "traceback",
            "exception",
            "left:",
            "right:",
            "-->",
            "^",
            "blocking",
        ]
        .iter()
        .any(|w| lowered.contains(w));
        if trouble {
            assert!(
                text.lines().any(|kept| kept == line),
                "dropped or changed: {line:?}\n--- model copy ---\n{text}"
            );
        }
    }
}

/// Real output: `cargo test --workspace --all-targets` of this repository on
/// CI (Windows, Rust 1.97.1, run 42382635564), colour codes removed as
/// run_command's output filter removes them, and the lines between the
/// excerpts left out.
const REAL_CARGO_TEST_FAILING: &str = include_str!("fixtures/cargo-test-failing.txt");

#[test]
fn a_real_failing_cargo_test_keeps_its_failures_and_progress() {
    let out = compress(
        "cargo test --workspace --all-targets",
        REAL_CARGO_TEST_FAILING,
        false,
    );
    assert!(out.changed());
    assert_failures_kept(REAL_CARGO_TEST_FAILING, &out.text);
    for kept in [
        "   Compiling shodh-rag v0.0.10 (D:\\a\\shodhRAG\\shodhRAG\\crates\\shodh-rag)",
        "test harness::compress::tests::compound_commands_get_only_the_lossless_steps ... FAILED",
        "---- harness::compress::tests::compound_commands_get_only_the_lossless_steps stdout ----",
        "assertion failed: out.text.contains(\"   Compiling libc v0.2.150\")",
        "    harness::compress::tests::the_note_says_what_was_removed_and_that_the_user_sees_everything",
    ] {
        assert!(out.text.lines().any(|l| l == kept), "{kept:?} missing:\n{}", out.text);
    }
    let passing = REAL_CARGO_TEST_FAILING
        .lines()
        .filter(|l| l.starts_with("test ") && l.ends_with(" ... ok"))
        .count();
    assert!(passing > 50);
    assert_eq!(out.passing_tests, passing);
    assert_eq!(
        out.removed_lines, passing,
        "a failed run loses only passing tests"
    );
    let lines: Vec<&str> = out.text.lines().collect();
    let result = lines
        .iter()
        .position(|l| l.starts_with("test result: FAILED"))
        .unwrap();
    assert_eq!(
        lines[result - 1],
        format!("[{passing} passing tests not listed]")
    );
}

#[test]
fn a_successful_cargo_run_loses_its_progress() {
    let mut output = String::from("   Compiling shodh v0.1.0\n    Finished `test` profile in 3.2s\n     Running unittests src/lib.rs\n\nrunning 200 tests\n");
    for i in 0..200 {
        output.push_str(&format!("test module_{i}::does_the_thing_{i} ... ok\n"));
    }
    output.push_str("\ntest result: ok. 200 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.40s\n");
    let out = compress("cargo test", &output, true);
    assert_eq!(out.passing_tests, 200);
    assert_eq!(out.removed_lines, 201);
    assert!(!out.text.contains("Compiling"));
    assert!(out
        .text
        .contains("[200 passing tests not listed]\ntest result: ok. 200 passed"));
    let saved = savings::estimate_tokens(&output) - savings::estimate_tokens(&out.text);
    assert!(saved * 10 > savings::estimate_tokens(&output) * 9);
}

#[test]
fn a_command_that_did_not_succeed_keeps_its_progress() {
    // Blocked on a lock and stopped at its time limit: the lock line is the diagnosis.
    let blocked =
        "    Updating crates.io index\n    Blocking waiting for file lock on package cache\n";
    assert_eq!(compress("cargo build", blocked, false).text, blocked);
    // Killed mid-build: the last Compiling line names what hung.
    let hung = "   Compiling libc v0.2.150\n   Compiling openssl-sys v0.9.103\n";
    let out = compress("cargo build --release", hung, false);
    assert!(!out.changed());
    assert_eq!(out.text, hung);
    // A lock line is never progress, even when the command succeeded.
    let waited = "    Blocking waiting for file lock on build directory\n   Compiling shodh v0.1.0\n    Finished `dev` profile in 9s\n";
    let out = compress("cargo build", waited, true);
    assert_eq!(
        out.text,
        "    Blocking waiting for file lock on build directory\n    Finished `dev` profile in 9s\n"
    );
}

const CARGO_BUILD_ERROR: &str = "    Updating crates.io index
  Downloaded anyhow v1.0.75
   Compiling anyhow v1.0.75
error[E0308]: mismatched types
  --> src/main.rs:4:18
   |
4  |     let x: u32 = \"five\";
   |            ---   ^^^^^^ expected `u32`, found `&str`

error: could not compile `shodh` (bin \"shodh\") due to 1 previous error
";

#[test]
fn a_failing_build_is_passed_whole() {
    let out = compress("cargo build", CARGO_BUILD_ERROR, false);
    assert!(!out.changed());
    assert_eq!(out.text, CARGO_BUILD_ERROR);
    // Succeeding, only cargo's own aligned progress goes.
    let out = compress(
        "cargo check",
        "Checking 4 files\n   Compiling app v0.1.0\n",
        true,
    );
    assert_eq!(out.text, "Checking 4 files\n");
}

#[test]
fn cargo_update_and_fetch_keep_their_results() {
    let update = "    Updating crates.io index\n     Locking 1 package to latest compatible version\n    Updating serde v1.0.190 -> v1.0.210\n      Adding serde_derive v1.0.210\n";
    for command in ["cargo update -p serde", "cargo fetch"] {
        assert_eq!(program_of(command), Program::Other, "{command}");
        assert_eq!(compress(command, update, true).text, update);
    }
}

#[test]
fn a_failing_tests_captured_output_is_kept_even_when_it_looks_like_cargo() {
    let mut output = String::from("running 6 tests\n");
    for name in ["saves", "lists", "counts", "sorts", "merges"] {
        output.push_str(&format!("test store::{name} ... ok\n"));
    }
    output.push_str(
        "test store::loads ... FAILED\n\nfailures:\n\n---- store::loads stdout ----\n    Checking invariants of 3 records\n   Compiling the index\ntest store::inner ... ok\nthread 'store::loads' panicked at src/store.rs:40:9:\nindex out of date\n\nfailures:\n    store::loads\n\ntest result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n",
    );
    // As if it had succeeded: the captured block is protected by itself, not by the exit code.
    let out = compress("cargo test", &output, true);
    for kept in [
        "    Checking invariants of 3 records",
        "   Compiling the index",
        "test store::inner ... ok",
    ] {
        assert!(
            out.text.lines().any(|l| l == kept),
            "{kept:?} dropped:\n{}",
            out.text
        );
    }
    assert_eq!(out.passing_tests, 5);
}

/// nextest's format, from nextest-runner's displayer (`{:>12}` status words,
/// the failing test's output after its status line under `stdout ───`).
const NEXTEST_FAILING: &str = "    Starting 4 tests across 1 binary
        PASS [   0.004s] store store::saves
        PASS [   0.005s] store store::lists
        FAIL [   0.010s] store store::loads
  stdout ───
    Checking invariants of 3 records
    test inner ... ok
  stderr ───
    thread 'store::loads' panicked at src/store.rs:40:9:
    index out of date

        PASS [   0.004s] store store::counts
────────────
     Summary [   0.020s] 4 tests run: 3 passed, 1 failed, 0 skipped
        FAIL [   0.010s] store store::loads
error: test run failed
";

#[test]
fn nextest_keeps_a_failing_tests_output() {
    assert_eq!(program_of("cargo nextest run"), Program::CargoTest);
    let out = compress("cargo nextest run", NEXTEST_FAILING, true);
    assert_failures_kept(NEXTEST_FAILING, &out.text);
    for kept in [
        "    Checking invariants of 3 records",
        "    test inner ... ok",
        "  stdout ───",
    ] {
        assert!(
            out.text.lines().any(|l| l == kept),
            "{kept:?} dropped:\n{}",
            out.text
        );
    }
    assert_eq!(out.passing_tests, 3);
    assert!(out
        .text
        .contains("[3 passing tests not listed]\n     Summary [   0.020s] 4 tests run"));
}

#[test]
fn streamed_test_output_keeps_its_cargo_looking_lines() {
    let output = "   Compiling shodh v0.1.0\n    Finished `test` profile in 1s\n     Running unittests src/lib.rs\n\nrunning 2 tests\n    Checking the cache\n   Compiling report\ntest a ... ok\ntest b ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
    let streamed = compress("cargo test -- --nocapture", output, true);
    assert_eq!(
        streamed.text,
        "    Finished `test` profile in 1s\n     Running unittests src/lib.rs\n\nrunning 2 tests\n    Checking the cache\n   Compiling report\n\n[2 passing tests not listed]\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
    );
    // Captured (the default), such lines can only be cargo's.
    let captured = compress("cargo test", output, true);
    assert!(!captured.text.contains("Checking the cache"));
}

#[test]
fn the_output_of_a_program_cargo_run_started_is_kept() {
    let output = "   Compiling app v0.1.0 (/work/app)\n    Finished `dev` profile [unoptimized] target(s) in 0.52s\n     Running `target/debug/app`\n    Updating the cache\n   Compiling report for 3 users\n";
    let out = compress("cargo run -- --report", output, true);
    assert_eq!(out.removed_lines, 1);
    assert_eq!(
        out.text,
        "    Finished `dev` profile [unoptimized] target(s) in 0.52s\n     Running `target/debug/app`\n    Updating the cache\n   Compiling report for 3 users\n"
    );
    // `-q` prints no Running line to tell the program's output from cargo's.
    assert_eq!(program_of("cargo run -q"), Program::Other);
    assert_eq!(program_of("cargo run -- -q"), Program::Cargo);
}

/// pytest `-q`: bare progress, no session header, a bare last line.
const PYTEST_Q_FAILING: &str = "..F.                                                                     [100%]
=================================== FAILURES ===================================
___________________________________ test_add ___________________________________

    def test_add():
        status = build()
>       assert status == \"pkg::build PASSED\"
E       AssertionError: assert 'pkg::build FAILED' == 'pkg::build PASSED'

tests/test_math.py:5: AssertionError
=========================== short test summary info ============================
FAILED tests/test_math.py::test_add - AssertionError: assert 'pkg::build FAILED' == 'pkg::build PASSED'
1 failed, 3 passed in 0.05s
";

#[test]
fn pytest_q_failures_pass_whole() {
    let out = compress("python -m pytest -q", PYTEST_Q_FAILING, false);
    assert!(!out.changed(), "{:?}", out.steps);
    // Passing: the dot line goes, the result stays.
    let passed =
        "........                                                                 [100%]\n8 passed in 0.02s\n";
    assert_eq!(
        compress("pytest -q", passed, true).text,
        "8 passed in 0.02s\n"
    );
}

const PYTEST_V_FAILING: &str =
    "tests/test_math.py::test_one PASSED                                      [ 25%]
tests/test_math.py::test_two PASSED                                      [ 50%]
tests/test_math.py::test_three PASSED                                    [ 75%]
tests/test_math.py::test_add FAILED                                      [100%]

=================================== FAILURES ===================================
___________________________________ test_add ___________________________________

    def test_add():
>       assert status == \"pkg::build PASSED\"
----------------------------- Captured stdout call -----------------------------
tests/test_inner.py::helper PASSED
E       AssertionError

tests/test_math.py:5: AssertionError
=========================== short test summary info ============================
FAILED tests/test_math.py::test_add - AssertionError
========================= 1 failed, 3 passed in 0.05s ==========================
";

#[test]
fn pytest_v_drops_passes_but_never_inside_its_failures() {
    let out = compress("pytest -v", PYTEST_V_FAILING, false);
    assert_failures_kept(PYTEST_V_FAILING, &out.text);
    assert_eq!(out.passing_tests, 3);
    for kept in [
        ">       assert status == \"pkg::build PASSED\"",
        "tests/test_inner.py::helper PASSED",
        "tests/test_math.py::test_add FAILED                                      [100%]",
    ] {
        assert!(
            out.text.lines().any(|l| l == kept),
            "{kept:?} dropped:\n{}",
            out.text
        );
    }
    assert!(out
        .text
        .contains("[3 passing tests not listed]\n========================= 1 failed"));
}

const JEST_FAILING: &str = "PASS src/utils/format.test.ts
FAIL src/cart/total.test.ts
  cart total
    ✓ sums items (3 ms)
    ✓ applies discounts (1 ms)
    ✕ rounds to cents (4 ms)

  ● cart total › rounds to cents

    expect(received).toBe(expected) // Object.is equality

    Expected: 10.01
    Received: 10.009999

  console.log
    ✓ step done

Test Suites: 1 failed, 1 passed, 2 total
Tests:       1 failed, 2 passed, 3 total
";

#[test]
fn jest_drops_passes_but_never_inside_a_block() {
    let out = compress("npm test", JEST_FAILING, false);
    assert_failures_kept(JEST_FAILING, &out.text);
    assert_eq!(out.passing_tests, 2);
    for kept in [
        "PASS src/utils/format.test.ts",
        "    ✕ rounds to cents (4 ms)",
        "    ✓ step done",
    ] {
        assert!(
            out.text.lines().any(|l| l == kept),
            "{kept:?} dropped:\n{}",
            out.text
        );
    }
    // Passing: a file's PASS line goes unless console output under it needs it.
    let passing = "PASS src/a.test.ts\n  ✓ one (1 ms)\nPASS src/b.test.ts\n  console.warn\n    act() warning\n\nTest Suites: 2 passed, 2 total\nTests:       2 passed, 2 total\n";
    let out = compress("npx jest", passing, true);
    assert!(!out.text.contains("PASS src/a.test.ts"));
    assert!(out
        .text
        .contains("PASS src/b.test.ts\n  console.warn\n    act() warning"));
}

const GO_FAILING: &str = "=== RUN   TestDivide
=== RUN   TestDivide/by_zero
    calc_test.go:21: got 0, want error
=== RUN   TestDivide/by_one
--- FAIL: TestDivide (0.00s)
    --- FAIL: TestDivide/by_zero (0.00s)
    --- PASS: TestDivide/by_one (0.00s)
FAIL
FAIL\texample.com/calc\t0.004s
FAIL
";

#[test]
fn go_keeps_its_run_lines_when_a_test_fails() {
    let out = compress("go test -v ./...", GO_FAILING, false);
    assert_failures_kept(GO_FAILING, &out.text);
    assert!(out
        .text
        .contains("=== RUN   TestDivide/by_zero\n    calc_test.go:21: got 0, want error"));
    assert_eq!(out.passing_tests, 1);
    let passing = "=== RUN   TestAdd\n--- PASS: TestAdd (0.00s)\n=== RUN   TestSub\n--- PASS: TestSub (0.00s)\nPASS\nok  \texample.com/calc\t0.004s\n";
    assert_eq!(
        compress("go test -v", passing, true).text,
        "[2 passing tests not listed]\nPASS\nok  \texample.com/calc\t0.004s\n"
    );
}

#[test]
fn git_status_hints_go_and_git_transfers_pass_whole() {
    let status = "On branch main
Changes not staged for commit:
  (use \"git add <file>...\" to update what will be committed)
  (use \"git restore <file>...\" to discard changes in working directory)
\tmodified:   src/lib.rs

no changes added to commit (use \"git add\" and/or \"git commit -a\")
";
    let out = compress("git status", status, true);
    assert_eq!(out.removed_lines, 2);
    assert!(out.text.contains("\tmodified:   src/lib.rs"));
    assert_eq!(program_of("git status --porcelain"), Program::Other);
    // Through a pipe git writes no transfer progress; a push is left alone.
    let push = "To github.com:me/repo.git\n ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs to 'github.com:me/repo.git'\nremote: Total size of new objects exceeds the 100 MB limit\n";
    assert_eq!(program_of("git push origin main"), Program::Other);
    assert_eq!(compress("git push origin main", push, false).text, push);
}

#[test]
fn search_matches_with_line_numbers_are_grouped_exactly() {
    let output = "src/harness/session.rs:120:    let x = error_count();\nsrc/harness/session.rs:144:    // error handling\nsrc/lib.rs:7:pub mod error;\n";
    let out = compress("rg -n error", output, true);
    assert_eq!(
        out.text,
        "src/harness/session.rs\n  120:    let x = error_count();\n  144:    // error handling\nsrc/lib.rs:7:pub mod error;\n"
    );
    // Without line numbers there is nothing to group by.
    assert_eq!(program_of("rg error"), Program::Other);
    assert_eq!(program_of("grep -rn error src"), Program::Search);
    // One file's matches have no path: a timestamp is not one.
    let log = "12:2026-11-07 10:15:32 ERROR db timeout\n13:2026-11-07 10:15:40 ERROR db timeout\n";
    assert!(!compress("grep -n ERROR app.log", log, true).changed());
    let stamps = "2026-11-07 10:15:32 ERROR db timeout\n2026-11-07 10:15:40 ERROR db timeout\n";
    assert!(!compress("grep -n ERROR app.log", stamps, true).changed());
}

#[test]
fn file_lists_are_grouped_by_folder() {
    let output = "./src/a.rs\n./src/b.rs\n./src/c.rs\n./docs/readme.md\n./Cargo.toml\n";
    assert_eq!(
        compress("find . -name '*.rs'", output, true).text,
        "./src/\n  a.rs\n  b.rs\n  c.rs\n./docs/readme.md\n./Cargo.toml\n"
    );
}

#[test]
fn anything_unrecognised_passes_unchanged() {
    let diff = "diff --git a/a.css b/a.css\n@@ -1,6 +1,6 @@\n .box {\n-  opacity: 10%;\n+  opacity: 20%;\n+  opacity: 30%;\n \n \n }\n }\n }\n\n\n";
    for command in [
        "git diff",
        "git diff | cat",
        "cat a.css.diff",
        "cargo test | tail -20",
        "cargo test; echo done",
        "cargo test > out.txt",
        "cargo test && cargo build",
        "echo $(cargo test)",
        "cargo test &",
    ] {
        let out = compress(command, diff, true);
        assert!(!out.changed(), "{command}: {:?}", out.steps);
        assert_eq!(out.text, diff);
    }
    // Identical lines are not merged either.
    let repeated = "retrying\nretrying\nretrying\nretrying\n";
    assert_eq!(compress("./run.sh", repeated, true).text, repeated);
}

#[test]
fn programs_are_recognised_behind_cd_and_environment_assignments() {
    assert_eq!(
        program_of("RUST_BACKTRACE=1 cargo test -p x"),
        Program::CargoTest
    );
    assert_eq!(
        program_of("cd crates/a && cargo +nightly t"),
        Program::CargoTest
    );
    assert_eq!(
        program_of("cargo clippy --all-targets 2>&1"),
        Program::Cargo
    );
    assert_eq!(program_of("git grep -n foo"), Program::Search);
    assert_eq!(program_of("npx vitest run"), Program::JsTest);
    assert_eq!(program_of("pnpm run test"), Program::JsTest);
    assert_eq!(program_of("uv run pytest tests"), Program::Pytest);
    assert_eq!(program_of("go test ./..."), Program::GoTest);
    assert_eq!(program_of("fd -e rs"), Program::Find);
    assert_eq!(program_of("cargo fmt"), Program::Other);
    assert_eq!(program_of("cargo test --quiet"), Program::Other);
    assert_eq!(program_of("npm install"), Program::Other);
    assert_eq!(program_of("cd a b && cargo test"), Program::Other);
    assert_eq!(program_of(""), Program::Other);
    assert!(command_of("cargo test -- --nocapture").streams_output);
    assert!(command_of("cargo nextest run --no-capture").streams_output);
    assert!(!command_of("cargo test").streams_output);
}

#[test]
fn only_blank_lines_left_together_by_removed_lines_are_joined() {
    let output = "running 4 tests\n\ntest a ... ok\n\ntest b ... ok\n\ntest c ... ok\n\ntest d ... FAILED\n\n\ndone\n";
    assert_eq!(
        compress("cargo test", output, false).text,
        "running 4 tests\n\ntest d ... FAILED\n\n\ndone\n[3 passing tests not listed]\n"
    );
}

#[test]
fn output_that_cannot_be_shortened_goes_unchanged() {
    for (command, output) in [
        ("ls", "a\nb\nc\n"),
        (
            "cargo test",
            "running 0 tests\n\ntest result: ok. 0 passed\n",
        ),
        ("echo", ""),
        ("echo", "   \n"),
    ] {
        let out = compress(command, output, true);
        assert!(!out.changed(), "{command}: {:?}", out.steps);
        assert_eq!(out.text, output);
    }
}

#[test]
fn the_note_says_what_was_removed_and_how_to_read_the_rest() {
    let out = compress("cargo test", REAL_CARGO_TEST_FAILING, false);
    let said = note(&out, "abc123");
    assert!(
        said.contains(&format!("{} passing-test lines removed", out.passing_tests)),
        "{said}"
    );
    assert!(
        !said.contains("progress"),
        "a failed run keeps its progress: {said}"
    );
    assert!(said.contains("No other line was removed."));
    assert!(said.contains("command abc123: read or search it with read_command_output"));
    let grouped = compress(
        "rg -n x",
        "src/harness/a.rs:1:x\nsrc/harness/a.rs:2:x\n",
        true,
    );
    assert!(grouped.changed());
    assert!(note(&grouped, "g").contains("grouped under their file or folder (none removed)"));
}

#[test]
fn a_cut_keeps_the_failure_lines_of_the_middle_and_says_so() {
    let mut text = String::new();
    for i in 0..400 {
        text.push_str(&format!("line {i} fine\n"));
    }
    text.push_str("error[E0425]: cannot find value `x`\n  --> src/a.rs:3:5\n\n");
    for i in 400..800 {
        text.push_str(&format!("line {i} fine\n"));
    }
    let cut = cut_keeping_failures(&text, 500, 500, Some("c7"));
    assert!(cut.contains("characters cut from the middle of the output"));
    assert!(cut.contains("[2 lines from the cut part that tell of an error, a failure or a warning, in order:]\nerror[E0425]: cannot find value `x`\n  --> src/a.rs:3:5\n"));
    assert!(cut
        .contains("[The whole output is command c7: read or search it with read_command_output.]"));
    assert!(!cut.contains("line 300 fine"));
    // Short enough: untouched.
    assert_eq!(cut_keeping_failures("short\n", 10, 10, None), "short\n");
}

#[test]
fn a_cut_says_when_failure_lines_did_not_fit() {
    let mut text = "start\n".repeat(200);
    for i in 0..2_000 {
        text.push_str(&format!("error: problem number {i} in the build\n"));
    }
    text.push_str(&"end\n".repeat(200));
    let cut = cut_keeping_failures(&text, 400, 400, None);
    assert!(cut.contains("more failure lines of the cut part did not fit here."));
    assert!(cut.chars().count() < 400 + 400 + MIDDLE_FAILURE_CHARS + 1_000);
}
