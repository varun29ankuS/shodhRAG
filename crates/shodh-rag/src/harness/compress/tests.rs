use super::*;

/// Every line of `output` that tells of a problem, found in `text` word for
/// word: compression never drops or rewrites one.
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

const CARGO_TEST_FAILING: &str = "\
   Compiling libc v0.2.150
   Compiling serde v1.0.190
   Compiling shodh v0.1.0 (/work/shodh)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 12.40s
     Running unittests src/lib.rs (target/debug/deps/shodh-1a2b3c)

running 6 tests
test parse::reads_numbers ... ok
test parse::reads_words ... ok
test parse::error_messages_are_plain ... ok
test math::adds ... FAILED
test math::subtracts ... ok
test slow::big ... ignored

failures:

---- math::adds stdout ----
thread 'math::adds' panicked at src/math.rs:12:9:
assertion `left == right` failed
  left: 4
 right: 5
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    math::adds

test result: FAILED. 4 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s

error: test failed, to rerun pass `--lib`
";

#[test]
fn a_failing_cargo_test_keeps_every_failure_line() {
    let out = compress("cargo test", CARGO_TEST_FAILING);
    assert!(out.changed());
    assert_failures_kept(CARGO_TEST_FAILING, &out.text);
    for kept in [
        "test math::adds ... FAILED",
        "test slow::big ... ignored",
        "---- math::adds stdout ----",
        "thread 'math::adds' panicked at src/math.rs:12:9:",
        "  left: 4",
        " right: 5",
        "    math::adds",
        "     Running unittests src/lib.rs (target/debug/deps/shodh-1a2b3c)",
        "    Finished `test` profile [unoptimized + debuginfo] target(s) in 12.40s",
        "error: test failed, to rerun pass `--lib`",
    ] {
        assert!(
            out.text.lines().any(|l| l == kept),
            "{kept:?} missing:\n{}",
            out.text
        );
    }
    assert!(!out.text.contains("Compiling"));
    assert!(!out.text.contains("... ok"));
    assert_eq!(out.passing_tests, 4);
    assert_eq!(out.removed_lines, 7);
    // The count stands just above the suite's result line.
    let lines: Vec<&str> = out.text.lines().collect();
    let result = lines
        .iter()
        .position(|l| l.starts_with("test result: FAILED"))
        .unwrap();
    assert_eq!(lines[result - 1], "[4 passing tests not listed]");
    assert!(out.text.len() < CARGO_TEST_FAILING.len());
}

const CARGO_BUILD_ERROR: &str = "\
    Updating crates.io index
  Downloaded anyhow v1.0.75
   Compiling anyhow v1.0.75
   Compiling shodh v0.1.0 (/work/shodh)
error[E0308]: mismatched types
  --> src/main.rs:4:18
   |
4  |     let x: u32 = \"five\";
   |            ---   ^^^^^^ expected `u32`, found `&str`
   |            |
   |            expected due to this

warning: unused variable: `y`
 --> src/main.rs:5:9
  |
5 |     let y = 2;
  |         ^ help: if this is intentional, prefix it with an underscore: `_y`

For more information about this error, try `rustc --explain E0308`.
warning: `shodh` (bin \"shodh\") generated 1 warning
error: could not compile `shodh` (bin \"shodh\") due to 1 previous error; 1 warning emitted
";

#[test]
fn a_failing_cargo_build_keeps_errors_and_warnings_whole() {
    let out = compress("cd app && cargo build --release 2>&1", CARGO_BUILD_ERROR);
    assert!(out.changed());
    assert_failures_kept(CARGO_BUILD_ERROR, &out.text);
    // The diagnostic block is kept exactly, blank lines and all.
    let block = "error[E0308]: mismatched types\n  --> src/main.rs:4:18\n   |\n4  |     let x: u32 = \"five\";";
    assert!(out.text.contains(block), "{}", out.text);
    assert!(out
        .text
        .contains("|            expected due to this\n\nwarning: unused variable"));
    assert!(!out.text.contains("Compiling") && !out.text.contains("Downloaded"));
    assert_eq!(out.removed_lines, 4);
    assert_eq!(out.passing_tests, 0);
}

#[test]
fn a_progress_word_that_names_a_problem_is_kept() {
    let output = "   Compiling shodh v0.1.0\n    Blocking waiting for file lock on build directory\nUpdating failed: network unreachable\n";
    let out = compress("cargo build", output);
    assert!(out.text.contains("Updating failed: network unreachable"));
    assert!(!out.text.contains("Blocking waiting for file lock"));
    assert!(!out.text.contains("Compiling"));
}

const PYTEST_FAILING: &str = "\
============================= test session starts ==============================
platform linux -- Python 3.12.1, pytest-8.0.0
collected 7 items

tests/test_parse.py ....                                                  [ 57%]
tests/test_math.py .F.                                                    [100%]

=================================== FAILURES ===================================
__________________________________ test_add ____________________________________

    def test_add():
>       assert add(2, 2) == 5
E       assert 4 == 5
E        +  where 4 = add(2, 2)

tests/test_math.py:4: AssertionError
=========================== short test summary info ============================
FAILED tests/test_math.py::test_add - assert 4 == 5
========================= 1 failed, 6 passed in 0.05s ==========================
";

#[test]
fn a_failing_pytest_keeps_its_failure_and_drops_only_dot_progress() {
    let out = compress("python -m pytest -q", PYTEST_FAILING);
    assert!(out.changed());
    assert_failures_kept(PYTEST_FAILING, &out.text);
    assert!(!out.text.contains("tests/test_parse.py ...."));
    // A progress line with a failure mark is kept.
    assert!(out.text.contains("tests/test_math.py .F."));
    assert!(out.text.contains("E       assert 4 == 5"));
    assert!(out
        .text
        .contains("FAILED tests/test_math.py::test_add - assert 4 == 5"));
}

#[test]
fn verbose_pytest_passes_are_counted() {
    let output = "tests/a.py::test_one PASSED  [ 33%]\ntests/a.py::test_two PASSED  [ 66%]\ntests/a.py::test_three FAILED [100%]\n====== 1 failed, 2 passed in 0.01s ======\n";
    let out = compress("pytest -v", output);
    assert_eq!(out.passing_tests, 2);
    assert!(out.text.contains("tests/a.py::test_three FAILED [100%]"));
    assert!(out
        .text
        .contains("[2 passing tests not listed]\n====== 1 failed, 2 passed"));
}

const JEST_FAILING: &str = "\
PASS src/utils/format.test.ts
PASS src/utils/parse.test.ts
FAIL src/cart/total.test.ts
  cart total
    ✓ sums items (3 ms)
    ✓ applies discounts (1 ms)
    ✕ rounds to cents (4 ms)

  ● cart total › rounds to cents

    expect(received).toBe(expected) // Object.is equality

    Expected: 10.01
    Received: 10.009999

      12 |   it('rounds to cents', () => {
    > 13 |     expect(total([5.005, 5.005])).toBe(10.01);
         |                                   ^

      at Object.<anonymous> (src/cart/total.test.ts:13:35)

Test Suites: 1 failed, 2 passed, 3 total
Tests:       1 failed, 2 passed, 3 total
";

#[test]
fn a_failing_jest_run_keeps_the_failure() {
    let out = compress("npm test", JEST_FAILING);
    assert!(out.changed());
    assert_failures_kept(JEST_FAILING, &out.text);
    assert!(out.text.contains("FAIL src/cart/total.test.ts"));
    assert!(out.text.contains("    ✕ rounds to cents (4 ms)"));
    assert!(out
        .text
        .contains("    > 13 |     expect(total([5.005, 5.005])).toBe(10.01);"));
    assert!(!out.text.contains("PASS src/utils"));
    assert!(!out.text.contains("✓ sums items"));
    assert_eq!(out.passing_tests, 2);
}

const GO_FAILING: &str = "\
=== RUN   TestAdd
--- PASS: TestAdd (0.00s)
=== RUN   TestDivide
    calc_test.go:21: Divide(1, 0) = 0, want error
--- FAIL: TestDivide (0.00s)
=== RUN   TestSub
--- PASS: TestSub (0.00s)
FAIL
FAIL\texample.com/calc\t0.004s
FAIL
";

#[test]
fn a_failing_go_test_keeps_the_failure() {
    let out = compress("go test -v ./...", GO_FAILING);
    assert!(out.changed());
    assert_failures_kept(GO_FAILING, &out.text);
    assert!(out
        .text
        .contains("    calc_test.go:21: Divide(1, 0) = 0, want error"));
    assert!(out.text.contains("--- FAIL: TestDivide (0.00s)"));
    assert!(!out.text.contains("=== RUN"));
    assert!(!out.text.contains("--- PASS"));
    assert_eq!(out.passing_tests, 2);
}

#[test]
fn git_status_hints_and_transfer_progress_are_dropped() {
    let status = "\
On branch main
Changes not staged for commit:
  (use \"git add <file>...\" to update what will be committed)
  (use \"git restore <file>...\" to discard changes in working directory)
\tmodified:   src/lib.rs

Untracked files:
  (use \"git add <file>...\" to include in what will be committed)
\tnotes.md

no changes added to commit (use \"git add\" and/or \"git commit -a\")
";
    let out = compress("git status", status);
    assert_eq!(out.removed_lines, 3);
    assert!(out.text.contains("\tmodified:   src/lib.rs"));
    assert!(out.text.contains("\tnotes.md"));
    assert!(out.text.contains("no changes added to commit"));
    // The short format has no hints and is left alone.
    assert_eq!(program_of("git status --porcelain"), Program::Other);

    let push = "\
Enumerating objects: 9, done.
Counting objects: 100% (9/9), done.
Delta compression using up to 8 threads
Compressing objects: 100% (5/5), done.
Writing objects: 100% (5/5), 1.02 KiB | 1.02 MiB/s, done.
Total 5 (delta 4), reused 0 (delta 0), pack-reused 0
remote: Resolving deltas: 100% (4/4), completed with 4 local objects.
To github.com:me/repo.git
 ! [rejected]        main -> main (fetch first)
error: failed to push some refs to 'github.com:me/repo.git'
hint: Updates were rejected because the remote contains work that you do not
";
    let out = compress("git push origin main", push);
    assert_failures_kept(push, &out.text);
    assert_eq!(out.removed_lines, 7);
    assert!(out
        .text
        .starts_with("To github.com:me/repo.git\n ! [rejected]"));
}

#[test]
fn search_matches_are_grouped_by_file_without_losing_any() {
    let output = "\
src/harness/session.rs:120:    let x = error_count();
src/harness/session.rs:144:    // error handling
src/harness/session.rs:200:    return Err(error);
src/lib.rs:7:pub mod error;
README.md:3:See the error guide.
README.md:9:Errors are plain.
";
    let out = compress("rg -n error", output);
    assert!(out.changed());
    assert_eq!(
        out.text,
        "src/harness/session.rs\n  120:     let x = error_count();\n  144:     // error handling\n  200:     return Err(error);\nsrc/lib.rs:7:pub mod error;\nREADME.md\n  3: See the error guide.\n  9: Errors are plain.\n"
    );
    assert_eq!(out.removed_lines, 0);
}

#[test]
fn file_lists_are_grouped_by_folder() {
    let output = "./src/a.rs\n./src/b.rs\n./src/c.rs\n./docs/readme.md\n./Cargo.toml\n";
    let out = compress("find . -name '*.rs'", output);
    assert_eq!(
        out.text,
        "./src/\n  a.rs\n  b.rs\n  c.rs\n./docs/readme.md\n./Cargo.toml\n"
    );
}

#[test]
fn compound_commands_get_only_the_lossless_steps() {
    for command in [
        "cargo test | tail -20",
        "cargo test; echo done",
        "cargo test > out.txt",
        "cargo test && cargo build",
        "echo $(cargo test)",
        "cargo test &",
    ] {
        assert_eq!(program_of(command), Program::Other, "{command}");
    }
    // Progress words survive when the output may not be cargo's.
    let out = compress("cargo test | tee log.txt", CARGO_TEST_FAILING);
    assert!(out.text.contains("   Compiling libc v0.2.150"));
    assert!(out.text.contains("test parse::reads_numbers ... ok"));
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
    assert_eq!(
        program_of("/usr/bin/git -C repo push"),
        Program::GitTransfer
    );
    assert_eq!(program_of("git grep -n foo"), Program::Search);
    assert_eq!(program_of("npx vitest run"), Program::JsTest);
    assert_eq!(program_of("pnpm run test"), Program::JsTest);
    assert_eq!(program_of("uv run pytest tests"), Program::Pytest);
    assert_eq!(program_of("go test ./..."), Program::GoTest);
    assert_eq!(program_of("fd -e rs"), Program::Find);
    assert_eq!(program_of("cargo fmt"), Program::Other);
    assert_eq!(program_of("npm install"), Program::Other);
    assert_eq!(program_of("cd a b && cargo test"), Program::Other);
    assert_eq!(program_of(""), Program::Other);
}

#[test]
fn repeated_lines_are_written_once_with_a_count() {
    let output = "start\nretrying connection\nretrying connection\nretrying connection\nretrying connection\nerror: gave up\n";
    let out = compress("./run.sh", output);
    assert_eq!(
        out.text,
        "start\nretrying connection\n[the line above repeated 3 more times]\nerror: gave up\n"
    );
    // Two in a row are left alone.
    assert!(!compress("x", "a\na\nb\n").changed());
}

#[test]
fn redrawn_progress_keeps_its_last_state_and_never_an_error() {
    let output = "Downloading model  10% (1/10)\nDownloading model  50% (5/10)\nDownloading model 100% (10/10)\nerror at 40% (4/10)\nerror at 50% (5/10)\ndone\n";
    let out = compress("./fetch.sh", output);
    assert_eq!(
        out.text,
        "Downloading model 100% (10/10)\nerror at 40% (4/10)\nerror at 50% (5/10)\ndone\n"
    );
    assert_eq!(out.removed_lines, 2);
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
        let out = compress(command, output);
        assert!(!out.changed(), "{command}: {:?}", out.steps);
        assert_eq!(out.text, output);
    }
}

#[test]
fn a_passing_cargo_test_run_is_mostly_its_summary() {
    let mut output = String::from("   Compiling shodh v0.1.0\n    Finished `test` profile in 3.2s\n     Running unittests src/lib.rs\n\nrunning 200 tests\n");
    for i in 0..200 {
        output.push_str(&format!("test module_{i}::does_the_thing_{i} ... ok\n"));
    }
    output.push_str("\ntest result: ok. 200 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.40s\n");
    let out = compress("cargo test", &output);
    assert_eq!(out.passing_tests, 200);
    assert!(out
        .text
        .contains("[200 passing tests not listed]\ntest result: ok. 200 passed"));
    let saved = savings::estimate_tokens(&output) - savings::estimate_tokens(&out.text);
    assert!(
        saved * 10 > savings::estimate_tokens(&output) * 9,
        "saved {saved} of {}",
        savings::estimate_tokens(&output)
    );
}

#[test]
fn the_note_says_what_was_removed_and_that_the_user_sees_everything() {
    let out = compress("cargo test", CARGO_TEST_FAILING);
    let said = note(&out);
    assert!(said.contains("4 passing test lines not listed"), "{said}");
    assert!(
        said.contains("3 progress or status lines removed"),
        "{said}"
    );
    assert!(said.contains("The user sees the full output."));
    let grouped = compress("rg -n x", "a.rs:1:x\na.rs:2:x\n");
    assert!(note(&grouped).contains("grouped (nothing dropped)"));
}
