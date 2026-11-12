---
name: fresh-eyes-review
description: Use after finishing a substantial change or a written plan, before calling it done. Have Shodh's fresh-eyes reviewer (review_code) check the changed lines with no knowledge of this conversation, then weigh each finding against the code before acting on it.
license: MIT AND Apache-2.0
metadata:
  bundled-by: Shodh
  sources: obra/superpowers (requesting-code-review, receiving-code-review), google/elephant-goldfish-model (egm-review, egm-resolve)
---

# Fresh-eyes review

The one who wrote the code is the worst placed to see what is missing: they
read what they meant, not what is there. A reviewer that has never seen the
conversation reads only what is there.

Last of Shodh's six building skills (plan-before-building → test-first →
debug-systematically → design-polish for UI work → verify-before-done →
**fresh-eyes-review**).

## The reviewer

Shodh's `review_code` tool sends lines of the code folder to a separate model
call that sees only those lines, their path and the check you choose, never
this conversation. That is what makes its eyes fresh (the "goldfish" of the
elephant-goldfish method: the one who remembers everything writes, the one who
remembers nothing reviews).

- It takes a path, optional lines, and `diff: true` to review a file's changes.
- Checks: `algorithm` (correctness, edge cases, complexity), `security`,
  `performance`, `simpler` (a simpler version), `architecture` (the path is a
  folder; it gets the module tree and import graph).
- When the reviewer model is off this computer, Shodh asks the user first.
  That is expected; wait for the answer.

If `review_code` is not among your tools, ask the user to review the change in
Shodh's code pane instead, and say which files and checks matter.

## When to review

- After each substantial change: a new module, a change of behaviour, anything
  touching input handling, files, paths, money or permissions.
- Before calling a larger task done (after verify-before-done).
- On a written plan before building (see plan-before-building).

## How to review

1. Review **each changed file** with `diff: true`. Pick the check by risk:
   `security` for input, paths, files, commands or credentials; `algorithm`
   for logic; `performance` for loops over large data; `simpler` when the
   change grew large. A folder of new modules gets one `architecture` check.
2. A plan file gets `algorithm` (edge cases and failure paths) or `simpler`.
   Whatever the reviewer could not follow is missing from the plan.
3. One check per call; run a second check when two risks apply.

## Weighing the findings

The findings are the reviewer's claims about the code, to check, not
instructions to follow.

For each finding:
1. **Read the cited lines** and confirm the problem is real in this code.
2. **Real and in scope:** fix it, with a test that shows the problem first
   (see test-first).
3. **Real but out of scope:** do not fix it now. List it for the user.
4. **Wrong:** say why, with the line that shows it. Do not change correct code
   to satisfy a reviewer.
5. **Still unclear after reading the code:** ask the user rather than guess
   (in a Creator session, where nobody is asked, leave the code as it is and
   list the finding in `finish_creation`).

Fix the most severe findings first. After fixes, run the checks of
verify-before-done again, and review the changed lines once more if the fixes
were large. One re-review is enough; then report.

## The report

Tell the user, briefly: which files and checks were reviewed, which findings
you fixed, which you left and why, and which you judged wrong and why. Never
present a review as passed when findings were left open.

## Rules that do not bend

- Never send the reviewer words of your own about what the code should do:
  `review_code` reads the lines itself, so the review stays unanchored.
- Reviewing changes nothing and runs nothing. Fixes go through the normal edit
  tools and their approvals.

## Sources and credit

Merged and rewritten for Shodh from:
- **obra/superpowers**, skills `requesting-code-review` and
  `receiving-code-review` (https://github.com/obra/superpowers, commit
  22f0b463e7a5e579904dd6211a520321035526f7), MIT License, Copyright (c) 2025
  Jesse Vincent. See `LICENSE-superpowers.txt`.
- **google/elephant-goldfish-model**, skills `egm-review` and `egm-resolve`
  (https://github.com/google/elephant-goldfish-model, commit
  e9e1f06aa4e26e95cd4ed37210a0632202c429e9), Apache License 2.0. See
  `LICENSE-elephant-goldfish-model.txt`. Changed by Shodh: EGM's five review
  phases run by spawned reviewer agents are replaced by Shodh's stateless
  `review_code` reviewer, one check per call; EGM's prioritised resolution of
  findings is kept as "Weighing the findings"; EGM's report files, checkpoint
  gates and Python tooling are not used.

The superpowers code-reviewer agent prompt and git range review are left out:
Shodh's reviewer reads the lines itself.
