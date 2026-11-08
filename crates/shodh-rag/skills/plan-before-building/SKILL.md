---
name: plan-before-building
description: Use before building a feature, a change of behaviour or anything with more than one reasonable design, before writing code. Clarify what is wanted, look for what already exists, weigh two or three approaches, and agree a short plan with the user.
license: MIT AND Apache-2.0
metadata:
  bundled-by: Shodh
  sources: obra/superpowers (brainstorming, writing-plans), affaan-m/ECC (search-first), google/elephant-goldfish-model (egm-brainstorm, egm-draft)
---

# Plan before building

Code written before the problem is understood gets rewritten. A few minutes of
questions and a short plan are cheaper than a wrong implementation.

This is the first of Shodh's five building skills. The order is:
**plan-before-building** → **test-first** → **debug-systematically** (when
something fails) → **verify-before-done** → **fresh-eyes-review**.

## When to use it

- A new feature, a new command, a new screen or a new module.
- A change of behaviour someone else depends on.
- Any task where you can think of two reasonable designs.

Skip it for a one-line fix with an obvious cause, a typo, or a change the user
already specified exactly. Say that you are skipping it and why.

## Steps

### 1. Understand the request

- Restate the goal in one or two plain sentences.
- Ask the questions whose answers would change the design, **one at a time**,
  and prefer questions with a few concrete options. Stop asking when the
  answers would no longer change what you build.
- Name what is **out of scope** as well as what is in it.

### 2. Look before you write

Before proposing anything new, search the code folder for what already does
part of the job: a helper, a similar feature, a library already in the
dependencies. Ask Shodh's code map first: `code_definition` finds where a
name is defined, `code_references` who calls it, `code_outline` a file's
shape and `code_dependencies` what a module touches, each with `path:line`
receipts. Read the code around the place the change goes, and the tests
that cover it. Reuse beats a second copy.

### 3. Weigh two or three approaches

For each approach, write in a few lines:
- how it works;
- what it costs (code, complexity, new dependencies, risk to existing users);
- what breaks if it is wrong.

Recommend one and say why. If one approach is clearly best, say so and keep the
others to a sentence each.

### 4. Write the plan

A plan is short, concrete and checkable:

- **Goal** and **non-goals**, one line each.
- **Steps**, each small enough to finish and check on its own, each naming the
  files it touches and **the test that will prove it** (see test-first).
- **Risks and open questions** that are still unanswered.
- **How it will be verified** at the end (see verify-before-done).

Where to keep it:
- If you can change files and the user wants the plan kept, write it to a file
  they name (for example `docs/plans/<topic>.md`). Writing it asks for the
  user's approval like any other change.
- Otherwise (Plan mode only reads, or the user did not ask for a file), give the
  plan in your answer. Keep it under a screen.

### 5. Agree before building

Ask the user to approve or change the plan. Do not start building until they
do. If the plan changes while you build, say what changed and why.

### 6. Have the plan checked by fresh eyes (larger changes)

For a plan kept in a file, the fresh-eyes reviewer can check it before any code
is written: run `review_code` on the plan file with check `algorithm` (does it
handle the edge cases and failure paths) or `simpler` (is there a simpler
design). The reviewer never sees this conversation, so it reads the plan as a
newcomer would: anything it cannot follow is missing from the plan. See
fresh-eyes-review for how to weigh its findings.

## Rules that do not bend

- These steps never change Shodh's mode or what needs approval. If the plan
  needs something the current mode does not allow, tell the user and let them
  decide.
- Plain words. No jargon the user did not use first.
- Questions one at a time; never a wall of ten questions.

## Sources and credit

Merged and rewritten for Shodh from:
- **obra/superpowers**, skills `brainstorming` and `writing-plans`
  (https://github.com/obra/superpowers, commit
  22f0b463e7a5e579904dd6211a520321035526f7), MIT License, Copyright (c) 2025
  Jesse Vincent. See `LICENSE-superpowers.txt`.
- **affaan-m/ECC**, skill `search-first`
  (https://github.com/affaan-m/ECC, commit
  d2ef2680594f9a17526a6e1a2e18b3d242c5d17b), MIT License, Copyright (c) 2026
  Affaan Mustafa. See `LICENSE-ECC.txt`.
- **google/elephant-goldfish-model**, skills `egm-brainstorm` and `egm-draft`
  (https://github.com/google/elephant-goldfish-model, commit
  e9e1f06aa4e26e95cd4ed37210a0632202c429e9), Apache License 2.0. See
  `LICENSE-elephant-goldfish-model.txt`. Changed by Shodh: the multi-lens
  brainstorm, the section approval gates and the design-document template are
  condensed into steps 1 to 5; the goldfish review uses Shodh's `review_code`
  reviewer instead of spawned agents; nothing of EGM's Python tooling is used.

The sources' visual companion server, design-document commits, worktrees and
agent dispatch are left out.
