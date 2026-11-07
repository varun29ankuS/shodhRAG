---
name: verify-before-done
description: Use before saying work is done, fixed or passing. Run the commands that prove it now, read their full output, and report the evidence, including anything that failed or was not checked.
license: MIT
metadata:
  bundled-by: Shodh
  sources: obra/superpowers (verification-before-completion), affaan-m/ECC (verification-loop)
---

# Verify before done

"It should work" is not evidence. Claim only what a command you ran just now
shows.

Fourth of Shodh's five building skills (plan-before-building → test-first →
debug-systematically → **verify-before-done** → fresh-eyes-review).

**No claim of success without fresh evidence from this conversation.**

## The check

Before you say done, fixed, passing or ready:

1. **Name the command** that proves the claim.
2. **Run it in full**, with `run_command`. Do not pipe it through `tail`,
   `head` or `grep` to shorten it: Shodh already shortens what you read of a
   command's output, keeping every error and failing test, and a cut output can
   hide the one line that matters.
3. **Read the result**: the exit code, the counts of passed and failed tests,
   every error and warning.
4. **Then state the result, with the evidence**: "cargo test: 214 passed, 0
   failed" or "the build fails: error E0308 in src/main.rs:4".

## What to run

Use what the project already has, found in its build files, scripts and CI
configuration. In order, stopping to fix whatever fails:

| Step | Examples |
|---|---|
| Build | `cargo build`, `npm run build`, `go build ./...` |
| Types and lint | `cargo clippy`, `npx tsc --noEmit`, `ruff check .` |
| Tests | `cargo test`, `npm test`, `pytest`, `go test ./...` |
| The change itself | The command or case the user reported, run again |
| The diff | `git diff --stat`, then read every changed file |

Reading the diff, check for: changes you did not mean to make, debugging prints
left behind, secrets or keys, missing error handling, and edge cases the tests
do not cover.

## What each claim needs

| Claim | Needs | Not enough |
|---|---|---|
| Tests pass | Test output with 0 failed, just now | An earlier run, "should pass" |
| Build succeeds | The build command exiting 0 | Lint passing |
| Bug fixed | The original failing case passing | The code changed |
| Requirement met | Each requirement checked one by one | Tests passing |

## The report

End with a short, honest report:

```
Build:  passed (cargo build)
Lint:   passed (cargo clippy, 0 warnings)
Tests:  failed: 213 passed, 1 failed (parse::rounds_cents)
Diff:   4 files changed, reviewed
Not checked: the release build (it needs the signing key)
```

Say plainly what failed and what you could not check, and why. A command the
user declined to run is "not checked", never "passed".

## Words to avoid until the check is done

"Should", "probably", "seems to", "done", "fixed", "all good". If you catch
yourself writing one, run the check first.

## Sources and credit

Merged and rewritten for Shodh from:
- **obra/superpowers**, skill `verification-before-completion`
  (https://github.com/obra/superpowers, commit
  22f0b463e7a5e579904dd6211a520321035526f7), MIT License, Copyright (c) 2025
  Jesse Vincent. See `LICENSE-superpowers.txt`.
- **affaan-m/ECC**, skill `verification-loop`
  (https://github.com/affaan-m/ECC, commit
  d2ef2680594f9a17526a6e1a2e18b3d242c5d17b), MIT License, Copyright (c) 2026
  Affaan Mustafa. See `LICENSE-ECC.txt`.

Where the two disagreed, Shodh chose: commands run in full instead of through
`tail` or `sed` (ECC's phases cut output; Shodh's output compression keeps
failures whole instead); no coverage threshold; no periodic re-verification
loop. The phase order and the report format come from ECC, the evidence rule
from superpowers.
