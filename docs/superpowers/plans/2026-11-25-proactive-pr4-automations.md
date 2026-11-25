# Proactive assistant, PR 4 (Automations) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Saved instructions that Shodh runs on a schedule, when something happens, or when the user clicks **Run now**. Each run:
- shows its call estimate first, and never goes past it;
- yields to the user's chat;
- never changes anything without the user's OK;
- leaves a readable record and a conversation.

The user can create an automation by describing it in plain words: the model drafts the name, the schedule and the instruction, and the user confirms them in a prefilled form. Automations can use a separate model from chat. The view always says whether that model shares chat's daily limit.

**Architecture:**
- **One call ledger** (`engine::ledger`, migration 21's `model_calls`) replaces `table_calls`. Chat calls are counted (never refused). Tables keep their cap through the same ledger. Each row carries a quota *pool* (`openrouter:free`, `lmstudio`, ...), so the counter and the chat reserve can tell whether automations and chat compete.
- **Playbooks are `playbook` engine jobs.** The playbook lives in the job's `input_json`: name, instruction, scope, steps and revision. They run on their own loop in `engine_commands.rs`, so a minutes-long agent step never delays a repeating task or a folder watch.
- **An `agent` step is a headless agent session.** It starts through the existing `agent_start`/`agent_send` path, as the phone's are, with three settings fixed by the engine:
  - the built-in `automation` profile: reads only, no execution or outward tools, a small tool limit;
  - `AgentMode::Ask`, whatever the user's default mode is;
  - the automations model, through `ModelOverride` and `check_selection`.
- **Writes are deferred, never run.** The session's approval gate is in *deferring* mode. A write the agent asks for:
  - is recorded with its own `ApprovalPreview`;
  - is answered to the model as "saved for the user's OK, not run".

  When the run ends, the deferred writes make one approval card with a checkbox per write. Approving runs exactly the recorded calls through the registry. (See decision Q5; this replaces §5.5's stop-and-resume.)
- **Calls are counted as they happen.** Each `Usage` event of the session is one model call. It goes into the ledger (`automations`) and into `engine_run.used_calls`. At `planned_calls` the session is aborted and the run ends with what it has.

**Tech stack:** Rust (rusqlite, chrono, the existing harness, engine and model picker; no new crate), React and TypeScript. Tests:
- shodh-rag unit tests, in CI "Tests";
- app tests, in CI "App tests and doc tests";
- Node strip-types tests, in CI "Frontend Typecheck + Tests";
- Clippy, in CI "Check + Clippy".

**Spec:** `docs/superpowers/specs/2026-11-22-proactive-assistant-design.md`, these parts:
- §5.2–5.6;
- §8, with its tools table;
- §12 items 1–3 and 5;
- §13 row 4;
- §14;
- §15 decision 2 (no automation cap).

The UI rules come from the memory "UI: sturdy, unambiguous" and from `docs/superpowers/specs/2026-11-23-ui-structure-audit.md`.

The branch is `feat/proactive-automations`, from `origin/master` `6695be40` (PRs #199, #202, #203, #204 and #207 merged). The next free migration is **21**. The only open PR, #208, touches neither `audit/store.rs`, `engine/`, `engine_commands.rs`, `agent_session_commands.rs`, `navigate.rs`, `viewTabs.ts` nor `agent-coverage.json`. It does touch `app_settings.rs` and `SettingsView.tsx`, so whichever lands second rebases (checked 2026-11-25).

## Proposed split (decision Q1)

| PR | Scope | Ships alone because |
|---|---|---|
| **4a. One ledger** | Migration 21's `model_calls` (copies and drops `table_calls`); `engine::ledger`; chat counted from `Usage`; `CallCounter` on the ledger; `model_usage_today` and the counter line in Settings → Model and `system_status` | The counts become honest before anything spends from them. Small, with no new UI surface. |
| **4b. Automations** | `engine_step`, `engine_approval` and `engine_cache` (also in migration 21, created in 4a and unused until 4b); the engine changes (E1–E5); the `automation` profile and the deferring gate; playbooks with `agent` and `notify` steps on a schedule or by hand; Run now; the automations model; yielding, the reserve and cooldowns; durable approvals; the Automations view and run page; tools, coverage and navigation | The core and the view. A user can write an automation, run it, and approve what it wanted. |
| **4c. Describe, templates, events** | Describe-to-create; event triggers (`files_added`, `paper_found`) with one-minute coalescing; the two templates; `engine_cache`; Show me what it did; Save as automation from an installed skill | Each one builds on 4b's runs. |

Migration 21 is written whole in 4a, so 4b and 4c add no migration. Every new table is `CREATE TABLE IF NOT EXISTS` and nothing uses `ALTER`, so re-applying it in the rollback tests is safe.

## Global Constraints

- **No local builds.** cargo, `npm install`, `npm test` and running the app happen in CI only. Each "Run" step names the CI job. `rustfmt` on changed files and `npx tsc --noEmit` are allowed when cheap. Rust only for any tooling.
- **Production code only:**
  - no TODO, stub, mock or placeholder;
  - Mutex locks use `.unwrap_or_else(|e| e.into_inner())`;
  - never weaken a test, threshold or gate.
- **Commits** have plain prose subjects and no footers: no Co-Authored-By, no "Generated with".
- **Same-task registration.** Every new command lands in one task, together with:
  - its `lib.rs` registration;
  - its `permissions/main-window.json` grant;
  - its `agent-coverage.json` entry;
  - a literal-string frontend use (the ACL and coverage gates).
- **`shodh.db` never enables foreign keys.** Deleting a playbook job deletes its runs, steps, approvals and cache rows explicitly, in one transaction.
- **A background run never moves the screen.** Only Run now, a user's click, opens the run's conversation, and it goes through `followNavigation`'s gate.
- **Free quota only.** Tests use recorded model replies only. No eval calls a live model in CI.
- **The agent never approves its own steps or changes spending.** `engine_approval_decide` and the automations-model setting are user-only.

## Spec ↔ code drift (code wins; each is handled in this plan)

1. **The migration number is 21, not 19.** PR 1 took 19 (engine and Inbox) and PR 3 took 20 (`feed_item`).
2. **Engine schema as built:**
   - `engine_run`'s column is `trigger_kind`, not `trigger`.
   - `planned_calls`, `used_calls` and `conversation_id` exist as columns, but `Run` does not read them.
   - `trigger_json` exists but is never read or written.
   - `engine_step`, `engine_approval` and `engine_cache` were deferred to PR 4 (PR 1's D6).
3. **`Schedule` has no time zone.** Schedules use the computer's zone (PR 1's D4 said the zone would come with PR 4). The plan keeps the computer's zone: a pinned zone would make an automation's "09:00" disagree with the clock in the title bar after travel. There is no IANA field.
4. **Deferrals cost attempts.** `Handled::RetryAt` and `Transient` raise `attempt`. Three attempts fail the run, and three failed runs hold the job. Yielding to chat, waiting for a quota reset and the two-minute wait after a wake would therefore put a busy user's automations on hold. PR 3's `paper_alerts` already spends an attempt on `AFTER_WAKE`. **E1** adds `Handled::Defer`.
5. **Waiting runs have no timer.** `Handled::Waiting` stores `next_attempt_at = None`, and only `serve_event` (hard-wired to `WaitReason::Decision`) and `settle` touch waiting runs. Approvals need a resume path (**E2**).
6. **Events carry nothing.** `Engine::event(job_id)` has no payload and runs have no input column. `files_added` and paper findings are direct calls inside `whats_new.rs` and `paper_alerts.rs`, not generic events. **E3** (4c) stores triggers in `trigger_json`, dispatches from those two call sites, and keeps a run's event subjects as its step 0 input (no `ALTER`).
7. **One loop ticks every kind inline** (`engine_commands::spawn`). A playbook's agent step, minutes long, would delay a repeating task's question. **E4** gives playbooks their own loop on the same `Engine`.
8. **`RunnerSource`/`RunEvents` stayed in `research::tables`** (PR 1's D1). A `table_add_rows_and_fill` step needs them made generic, or the fill job's state polled. It is out of v1 (Q9).
9. **The ledger today:**
   - Only tables count, in `table_calls`.
   - Slack and Telegram count *answers*, not calls (UTC days, their own stores).
   - Learning counts in `memory_learn_usage`.
   - Slack answers go through `AnswerService`/`TextModel`, not agent sessions, so counting chat at the session layer double-counts nothing.
   - Two rollback tests run `DROP TABLE table_calls` (`jobs_tests.rs:1218`, `audit/store.rs:2658`). After migration 21 that table no longer exists, so those two lines become `DROP TABLE IF EXISTS table_calls; DROP TABLE IF EXISTS model_calls;`. This edits fixture setup only; no assertion changes.
10. **The provider almost never reports what is left.** OpenRouter's `X-RateLimit-*` values reach Rust only inside error bodies (`llm/failure.rs::rate_limit_headers`), and omp passes no headers on success. §5.4's "Your provider says 27 are left" and its chat-reserve rule therefore have no input. §14 already allows this degraded form; Q3 settles what replaces it.
11. **A cheaper free model on the same account saves no quota.** `model_fallback/policy.rs` records that OpenRouter limits free models *per account* (20 a minute, 50 a day, or 1000 once credits were bought), and that more keys do not help. Only another provider, a paid model or LM Studio is outside chat's limit. The automations model therefore carries its pool, and the yield and reserve rules apply only when the pools match (Q2).
12. **Model calls happen inside the omp sidecar.** Rust sees one `Usage` event per answered message, with running totals. Calls are counted from those, and the ceiling is enforced by aborting the session. "Re-check before each call" (§5.4) is impossible, so the yield check happens before each step. A refused call (a 429) emits no `Usage` and is not counted.
13. **The step limit is 24 tool calls** (`DEFAULT_MAX_TOOL_CALLS`). That is about 25 model calls against a 50-a-day quota, plus up to 1 repair, 2 coverage and 1 retrieve rounds (`grounding/followup.rs`).
    - The `automation` profile has `max_tool_calls = 3`.
    - Its sessions start with repair and coverage rounds off.
    - The estimate is therefore `max_tool_calls + 1` = **4 calls** per agent step, the spec mock's figure.
14. **Only the `assistant` built-in profile exists.** `AgentProfile { allowed_tools, mode, max_tool_calls }` already holds what `automation` needs.
15. **Approvals are live and session-bound.** `ApprovalGate` is a oneshot per step with a timeout. `InboxStore::recover` drops session approvals at launch. Host tools can be dispatched later through the registry (`ToolRegistry::dispatch`), which is what the deferred card uses.
16. **"Chat in flight" has no global check.** `AgentSessions::answering(conversation_id)` is per conversation. A new `any_user_answer_running()` excludes automation sessions. Automation sessions also count against `MAX_LIVE_SESSIONS` (4), and are evicted first and closed when their run ends.
17. **Navigation drifted.**
    - `VIEWS` lives in `crates/shodh-rag/src/harness/tools/navigate.rs`, not in the app's `navigate.rs`.
    - Master's bar is ask, artifacts, library, workspaces, connectors, tasks, activity, settings: Artifacts and Connectors are still there, and Library comes before Workspaces, unlike the approved navbar.
    - This PR only inserts `automations` between `tasks` and `activity`. Reconciling the rest with the approved navbar is a separate change.
    - `NavigationTarget::KINDS` has 14 kinds; `automation` makes 15.
18. **Paper alerts already exist as their own job kind** (PR 3). §8's *Weekly literature check* with its own `find_papers` step would make a second path to the same APIs, with a second consent line. The template instead runs on an existing alert's finds (`paper_found`) and only adds the summary call.
19. **No `playbook` table.** The job row already has the title, schedule, trigger, `created_by`, provenance and `input_json`. The playbook (name, instruction, scope, steps, revision) lives in `input_json`, so a playbook and its job can never disagree, and deleting one is a single delete.
20. **State copy.** The spec's full-sentence states ("Paused so your chat keeps working: ...") become 1–2-word chips with a one-line tooltip, as PR 1's D15 did.
21. **The Inbox is ready.** PR 2 already has `'automation'` in `KINDS`, the hold item's `fix: 'resume'`, and `AUTOMATION_YIELD_ID = 'automation:yield'` (an Info item; `recover` marks it failed after a restart).

## Data model: migration 21 (written in 4a)

```
model_calls     utc_day TEXT, spender TEXT CHECK IN ('chat','tables','automations','review',
                'slack','telegram','learning'), pool TEXT NOT NULL DEFAULT '',
                calls INTEGER NOT NULL CHECK (calls >= 0), PRIMARY KEY (utc_day, spender, pool)
                -- copied from table_calls as ('tables', ''), ON CONFLICT keeps the larger count
                -- (safe to re-apply); then DROP TABLE table_calls
engine_step     run_id, idx, kind CHECK IN ('input','agent','notify','approval','find_papers',
                'create_task'), state CHECK IN ('pending','running','done','failed','skipped'),
                input_json, output_json, calls, started_at, ended_at, PRIMARY KEY (run_id, idx)
engine_approval id PK, job_id, run_id, step_idx, title, writes_json (each: tool, args, tier,
                preview, chosen), created_at, expires_at, decision NULL CHECK IN ('approved',
                'denied','expired','partly'), decided_at, result_json
                INDEX engine_approval_open ON engine_approval(decision) WHERE decision IS NULL
engine_cache    job_id, input_key, output_json, created_at, PRIMARY KEY (job_id, input_key)
```

- **The pool** is `<provider>:<free|paid>` for OpenRouter, `<provider>` for other keys, and `lmstudio` for local. `''` means unknown: old rows, and spenders that do not say.
- **A run's event input** is its `engine_step` row at index 0, kind `input` (4c).
- **The cache key** is `sha256(revision ‖ step idx ‖ canonical JSON of the step's inputs)`. A scheduled agent step's input includes:
  - the occurrence's date;
  - the ids and modification times of the scope's sources changed since the last successful run.

  So "unchanged inputs make no call" holds, but a weekly digest is never cached forever. An agent step with no data input (a pure instruction) is never cached.
- **Retention** follows `engine_run`: steps, approvals and cache rows of pruned runs are pruned in the same transaction. Cache rows are kept 90 days.

## Engine changes (shodh-rag, 4b unless marked)

- **E1. `Handled::Defer { at, reason, detail }`.** The run waits as `reason` until `at` (it sets `next_attempt_at`), and `attempt` does not change. It is used for `yield`, `quota`, `offline`, `model_unavailable` and the two-minute wait after a wake. `tick` already resumes waiting runs whose `next_attempt_at` has passed (`resumable`). PR 3's `paper_alerts` switches its after-wake wait from `RetryAt` to `Defer`, a one-line behaviour fix with a test (`a_wake_wait_is_not_an_attempt`).
- **E2. `Engine::resume(handler, job_id, run_id)`** claims a run that waits as `approval` or `decision` and calls `handler.run`. The handler reads the decision from the store. `serve_event` keeps its `Decision` case and calls this.
- **E3 (4c). Event triggers.**
  - `Trigger` JSON in `trigger_json`, either `{kind: "files_added", sourceId}` or `{kind: "paper_found", alertId}`.
  - `Engine::event_with(job_id, subjects)` queues one run deferred 60 s. A second event within that minute adds its subjects to the queued run's input instead of making a run.
  - `EngineStore::jobs_triggered_by(kind, ref)` lists the jobs an event should reach.
- **E4. A second loop** in `engine_commands.rs`: one `tokio` task for `JobKind::Playbook` with its own `Notify`. It runs one playbook run at a time (no stampede), on the shared `Engine`. `wake_automations(app)` wakes it.
- **E5. The fold extends to deferred waits.** When an occurrence passes while a run waits as `yield`, `quota`, `offline` or `model_unavailable`, the run moves to the newer occurrence (`missed` + 1) and no row is added, as it does for `decision` (PR 1's D8). Without this, an offline week would write a "previous run was still going" row for every occurrence.
- **E6. `Run` reads** `planned_calls`, `used_calls` and `conversation_id`. `EngineStore` gains `set_planned`, `add_used` (the increment is one SQL update) and `set_conversation`.

## The call ledger (4a)

- **`engine::ledger::Ledger`** over `model_calls`:
  - `record(day, spender, pool, n)`;
  - `try_record(day, spender, pool, n, cap)` (one immediate transaction, as `CallCounter::try_record`);
  - `count(day, spender)`;
  - `today(day) -> Totals { by_spender, by_pool }`;
  - `busiest(spender, pool, days: 14) -> u32`.
- **`CallCounter`** keeps its public API and becomes the ledger with spender `tables`, so the table runner, its cap and the fill dialog's "Tables have made N of 30 calls today" are unchanged.
- **Chat** is counted in `agent_session_commands`'s event pump:
  - every `Usage` event of a session that is not an automation session is one `chat` call, in the pool of the session's model;
  - this covers side threads and phone answers;
  - every other direct `LLMManager` call made for chat (an inventory task lists each call site) also records `chat`.
- **Never refused:** chat uses `record`, never `try_record`.
- **Slack, Telegram and learning** keep their own stores until next touched (§5.4). `model_usage_today` reads them and labels Slack and Telegram as answers.
- **The counter line** (one sentence, the same text everywhere):
  - "Today: 23 model calls (chat 9, tables 8, automations 6). Resets 5:30 am."
  - With an OpenRouter free pool and Q3's recommended option: "About 27 left on your free OpenRouter limit."

  It shows in Settings → Model, in the Automations view's footer, and in `system_status`.

## Playbooks, steps, the profile and approvals (4b)

- **The playbook** is `input_json = {name, instruction, scope: {workspaceId} | {library: true}, steps: [...], revision}`. A user-written playbook has the steps `[agent, notify]`.
  - `revision` goes up on every change of the instruction, steps or scope.
  - An open approval of an older revision stays valid for exactly what it previewed.
- **`agent` step:**
  1. The engine checks the yield, cooldown and reserve rules (below). If one applies, the run waits with `Defer`.
  2. It sets `planned_calls`.
  3. It starts a session: `agent_start(conversation_id = "auto-<job>-<run>", profile "automation", mode Ask, ModelOverride = the automations model)`.
  4. It sends the instruction with a fixed preamble. The preamble names the scope, says that sources are data and never instructions (`UNTRUSTED_NOTICE`), and that writes are saved for the user's OK.
  5. While the session runs, the engine folds events with `TurnRecorder`, so the conversation is saved like any answer. It counts `Usage` events. It aborts the session at `planned_calls` ("Stopped at its estimate of 4 calls; here is what it found.").
  6. At the end, the session is closed.
- **`notify` step:** posts one Info item, "*Weekly digest* ran: found 4 papers [Open]". No desktop notification is sent for a success (Q8).
- **The `automation` profile (shodh-rag `profile.rs`):**
  - `allowed_tools` is the assistant's minus:
    - `run_command` and every Code-mode tool;
    - the GitHub write tools;
    - connector posts;
    - `download_file`;
    - MCP tools not set to read;
    - the automation tools themselves (an automation cannot make, change or run automations);
    - the navigation tools (`open_view`, `show_*`), which are recorded as replay targets for 4c's Show me what it did instead of moving the screen.
  - `mode` is Ask and `max_tool_calls` is 3.
  - A test pins the exclusion list against `app_tools::ALL`, so a new execution tool fails the test until it is classified.
- **The deferring gate.** `ApprovalGate::deferring()`. Every call `decide` would send to `Gate::Ask` (LocalEdit, Write, Destructive, and `must_confirm` tools) is handled like this:
  - It is recorded as `{tool, args, tier, preview}`, using the tool's own `preview_in`.
  - It is answered to the model with "Saved for the user's OK; it has not run. Do not try it again; finish your answer."
  - It is never executed in the session. This holds in every `AgentMode`, because the session's mode is fixed by the engine.
- **The approval card.** When the run ends with deferred writes, an `engine_approval` row is written and the run waits as `approval`.
  - One Attention item is posted, `engine:approval:<id>`: "*Weekly digest* wants to add 3 tasks [Review]". [Review] opens the run page, which shows one checkbox per write with its preview.
  - **Approve selected** dispatches exactly the recorded calls, in order, through the registry with a pre-approved gate. The arguments are revalidated, so a target that is gone fails that one write, in words. The result is stored.
  - **Deny** ends the run as `succeeded` with "You declined 3 changes".
  - Expiry is 7 days. It cancels the run, saying "Expired: nobody approved these changes in 7 days."
  - At launch the engine re-posts every open approval, after `InboxStore::recover` has run.
- **Run now** (`automation_run_now`):
  - It inserts a `Manual` run and wakes the playbook loop.
  - From the UI it also opens the run's conversation in Ask, through the gate.
  - When a run is already active the button reads **Running** and is disabled, with the same width.
  - Q7 says whether it skips yielding.

### Yielding, cooldowns and the reserve (4b)

They are checked before each agent step, in this order. Each one waits with `Defer`, so none costs an attempt.
1. **Cooldown.** After any spender's rate-limit or quota error in the automations model's pool, `PoolCooldowns` (in memory, set from the classified `ProviderFailure` and the existing `CoolingModel` path) holds that pool until:
   - the reported reset;
   - else 60 s for a short window;
   - else the next UTC midnight for a daily one.

   The run waits as `quota` until then. Chat keeps trying as it does today.
2. **Chat in flight**, only when the automations model's pool equals chat's: while `any_user_answer_running()`, the run waits as `yield` and is checked again 30 s later. A chat that starts during a step does not stop it: the step finishes within its ceiling of 4 calls (default; see Q7).
3. **The reserve**, only when the pools match and the pool has a known daily limit (Q3).
   - `left = limit − today's ledger count for the pool`.
   - When `left − planned_calls < max(1, busiest chat day of the last 14)`, the run waits as `quota` until the pool's reset.
4. The pause posts and updates one Info item, `automation:yield`: "Automations paused so your chat keeps working. They go on after 5:30 am."

With LM Studio as the automations model, rules 1 (for LM Studio's own errors) and 2 (only if chat also uses LM Studio, which has a single slot) apply, and rule 3 never applies.

## The automations model (4b; Q2)

- **The setting:** `AutomationPrefs { model: Option<ModelRef> }` in `app_settings.rs`.
  - `None` means "Same as chat".
  - It is user-only: an `AGENT_DENIED` entry, and coverage `excluded` ("which model automations spend on is the user's decision").
  - It is checked with `check_selection` (key, Local-only, stealth consent) when saved and again before every run.
- **The picker** reuses the model picker's list. Each option carries a pool tag in one word: **Shares chat's limit**, **Own limit** or **Local**.
- **No silent fallback to chat's model.** When the automations model fails as unavailable:
  - the run waits as `model_unavailable` (Defer, 15 min);
  - after 3 runs it is held as `model_unavailable` with an Attention item "Automations' model isn't answering [Check model]".
  - Automations never spend chat's model because their own failed.

## Describe to create (4c; Q6)

- The New automation dialog opens on one box: "What should Shodh do, and when?". Under it are [Draft] and "1 call".
- **Draft** makes one one-shot call (`automation_draft`, the automations model, spender `automations`), with a JSON reply schema: `{name, instruction, when: Schedule | {event} | "manual", scope}`.
  - The reply is parsed with serde and `Schedule::validate`, and scope ids are checked against the workspaces.
  - On success it prefills the same form the user would fill by hand, and that form is the preview. Nothing is saved until **Save**.
  - An invalid or failed draft keeps the user's text as the instruction, leaves When for the user, and says "Couldn't draft it; fill in When."
- **With no model**, [Draft] is disabled with the tooltip "Set up a model first", and the form below still works.
- The text drafted is the user's own, so it is trusted input. A drafted event trigger is offered only for sources and alerts that exist.

## UI (4b; describe and templates in 4c)

```
 Automations                               Model [Same as chat v] Shares chat's limit   [+ New automation]
 ┌──────────────────────────────────────────────────────────────────────────────────────────────┐
 │ Weekly digest       Mon 09:00 · next Mon 1 Dec   [Scheduled]  Last: 4 papers, 3 calls  [Run now] [Pause] [⋯] │
 │ Thesis check-in     Only when you start it       [Needs you]  Wants to add 3 tasks      [Run now] [Pause] [⋯] │
 │ Monthly summary     1st of month 08:00           [On hold]    Model not answering ×3    [Run now] [Resume] [⋯] │
 └──────────────────────────────────────────────────────────────────────────────────────────────┘
 Today: 23 model calls (chat 9, tables 8, automations 6). Resets 5:30 am.
```

- **Fixed slots:**
  - Columns have fixed widths (name truncates, When truncates, Last truncates; the full text is in a tooltip).
  - The state chip has one width for every state.
  - The buttons have fixed widths. Pause and Resume share one slot. Run now becomes **Running** with a spinner and keeps its width.
  - The footer counter is always one line.
- **States** (chip, then tooltip):
  - Scheduled · "Runs Mon 09:00".
  - Running · "Step 1 of 2, 2 of 4 calls".
  - Waiting · "Waiting while you chat".
  - Paused · "Your provider's limit; goes on after 5:30 am".
  - Offline · "Tries again when you're online".
  - Needs you · "Wants to make 3 changes".
  - On hold · "Failed 3 times: <reason>".
  - Off · "Paused by you".
- **The run page** opens in the view's right pane, which has a fixed width.
  - Header: when it ran and why ("Ran when Shodh opened; Monday's was missed"), and calls used of planned.
  - Steps in words, one fixed-height row each.
  - [Open conversation]; [Show me what it did] (4c).
  - Pending writes with checkboxes, [Approve selected] and [Deny].
  - Runs list: the latest 20 runs, each with a state word and a summary.
- **The New automation dialog:** Describe (4c), then:
  - Name;
  - What should Shodh do;
  - When: Every day / Weekdays / Weekly on [days] / Monthly on [day] / Every [N] hours / Only when I start it, plus (4c) When a file arrives in [folder] and When an alert finds papers [alert];
  - At [time];
  - Where [Library | workspace];
  - one line: "Each run: up to 4 model calls. Changes wait for your OK."
  - [Cancel] [Save].
- **Empty view:** the template cards (4c), each with [Set up]. In 4b, before templates exist, the empty view shows only [+ New automation] centred, with "No automations yet".
- **Errors** sit in the slot they belong to: a failed save under the form's button row; a failed run in the run's row tooltip and its run page.

## Agent tools, coverage and navigation (4b)

| Command | Tool(s) | Tier |
|---|---|---|
| `automations_list`, `automation_get`, `automation_runs`, `automation_run_get` | `list_automations`, `show_automation` | Read; `show_automation` opens Automations and highlights the automation or run |
| `automation_create`, `automation_update` | `create_automation`, `update_automation` | Write, `must_confirm`; the preview shows the schedule and "up to 4 calls a run (about 16 a month)" |
| `automation_delete` | `delete_automation` | Destructive |
| `automation_pause`, `automation_resume` | `pause_automation`, `resume_automation` | Write, local edit |
| `automation_run_now` | `run_automation` | Write, `must_confirm`, with the estimate; it never opens the screen from the agent |
| `automation_draft` (4c) | maps to `create_automation` | The agent drafts in its own answer |
| `engine_approval_decide` | excluded | "The assistant never approves its own steps." |
| `model_usage_today` | `system_status` (its report gains the counter) | Read |
| the automations-model setting (`settings_update` key `automations.model`) | excluded via `AGENT_DENIED` | Spending is the user's decision |

- **Views.** `VIEWS` (shodh-rag `navigate.rs`) and `VIEW_TABS` (`viewTabs.ts`) gain `automations` between `tasks` and `activity`, with:
  - its label, description and keywords ("automations automation scheduled playbook routine recurring run");
  - the `navigation.views` entry.
- **Target kind `automation` `{automationId, runId?}`:**
  - `NavigationTarget::KINDS` (15) with a round-trip test;
  - a `kind_owner` arm (`features/automations/AutomationsView.tsx`);
  - `case 'automation':` in `navigation.ts`;
  - `useNavigationTarget('automation'` in the view;
  - an `expect_navigation` test per tool.
- **Write tools highlight what they saved** and never open a dialog (§12 item 5).

## Notifications (Q8)

- Approval waiting: `notify(NeedsYou)`.
- Hold: `notify(Problems)`.
- Success: an Info item only.
- Yield and quota pause: the one updated Info item, never a desktop notification.

## How it degrades

| Situation | What happens | What the user sees |
|---|---|---|
| No model configured | A scheduled run defers as `model_unavailable` and holds after 3 runs; Run now refuses at once | Chip **On hold** · "No model set up" [Set up model]; Run now disabled, tooltip "Set up a model first"; Draft disabled |
| Automations model refused by Local-only mode | The run is recorded `skipped`, never failed (like paper alerts) | **Off** · "Local-only: pick a local model" |
| Rate-limited or quota used up (any spender, same pool) | `Defer(quota)` until the reset; no attempt spent | **Paused** · "goes on after 5:30 am"; one updated Info item |
| User chatting (same pool) | `Defer(yield)`, re-checked every 30 s | **Waiting** · "Waiting while you chat" |
| Offline (connection-class failure on the first call) | `Defer(offline)` on the ladder's times; never holds; folds into the next occurrence (E5) | **Offline** |
| App closed at the scheduled time | Per Q4: one catch-up run after launch, deferred 2 min and yielding to chat | Run page: "Ran when Shodh opened; 2 earlier times were skipped" |
| Asleep mid-run | The lease expires; the agent step restarts from its beginning (its calls may count twice, §14) | Run page: "Restarted after the computer slept" |
| Estimate reached | The session is aborted; the run succeeds with what it has | "Stopped at its estimate of 4 calls" |
| Approval ignored for 7 days | The run is cancelled | "Expired: nobody approved these changes in 7 days" |
| `shodh.db` unavailable | The view shows the database error in its body; no runs | The same text as the audit page |

## Tasks

### 4a

**Task A1: Migration 21 and the ledger.**
- Files: `audit/store.rs` (migration and doc comment), `engine/ledger.rs` (new), `engine/mod.rs`, `research/tables/calls.rs` (on the ledger, same API), `jobs_tests.rs:1218` and `audit/store.rs:2658` (`DROP TABLE IF EXISTS`; drift 9).
- Tests:
  - `table_calls_are_copied_into_the_ledger_and_dropped`;
  - `reapplying_21_keeps_the_larger_count`;
  - `the_table_cap_holds_on_the_ledger` (the existing `try_record` tests unchanged and green);
  - `spenders_and_pools_add_up_per_utc_day`;
  - `busiest_chat_day_of_the_last_14_is_at_least_one`;
  - `engine_step_approval_and_cache_exist_and_reapply_safely`.

**Task A2: Count chat.**
- Files: `agent_session_commands.rs` (the `Usage` → ledger hook; automation sessions excluded), and each direct chat `LLMManager` call site from the inventory.
- Tests:
  - `each_usage_event_of_a_chat_session_is_one_chat_call`;
  - `a_refused_call_without_usage_is_not_counted`;
  - `chat_is_never_refused_by_the_ledger`;
  - `phone_and_side_thread_answers_count_as_chat`.

**Task A3: The counter.**
- Files:
  - `model_usage_today` (command, ACL, coverage → `system_status`);
  - `agent_tools` `system_status` text;
  - the Settings → Model line;
  - `features/modelPicker/usage.ts` (pure text) and its test.
- Tests: `the_counter_reads_one_line_for_each_mix`, including Slack and Telegram as answers and the reset in local time; `system_status_reports_the_counter`.

### 4b

**Task B1: Engine E1, E2, E4, E5 and E6.**
- Tests:
  - `a_defer_never_counts_an_attempt_or_holds`;
  - `a_deferred_run_resumes_at_its_time`;
  - `a_wake_wait_is_not_an_attempt` (paper alerts);
  - `resume_takes_up_an_approval_wait`;
  - `a_deferred_wait_folds_into_the_next_occurrence`;
  - `a_long_playbook_run_does_not_delay_a_repeat` (two loops);
  - `used_calls_add_up_and_planned_is_read_back`.

**Task B2: The `automation` profile and the deferring gate (shodh-rag harness).**
- Tests:
  - `the_automation_profile_has_no_execution_outward_or_automation_tools`, pinned against `app_tools::ALL`;
  - `every_write_is_deferred_in_every_agent_mode`, an exhaustive loop over `AgentMode`;
  - `a_deferred_write_never_executes_and_tells_the_model`;
  - `navigation_tools_are_recorded_not_given`;
  - `the_estimate_is_tool_limit_plus_one_with_revisions_off`.

**Task B3: Playbooks: the handler, the steps, the session runner and counting.**
- Files: `app/src-tauri/src/automations.rs` (new: the handler, commands and the store adapter), `engine_commands.rs`, `agent_session_commands.rs` (`any_user_answer_running`, automation sessions first to evict, closed at the end).
- Tests, with recorded model replies only (`harness` fake sidecar transcripts):
  - `a_run_never_exceeds_its_estimate`;
  - `an_automation_call_waits_while_a_chat_answer_runs` (same pool) and `does_not_wait_on_another_pool`;
  - `after_a_429_from_chat_automations_wait_for_the_reset_and_chat_does_not`;
  - `at_the_reserve_automations_pause_and_resume_after_the_reset_with_one_updated_item`;
  - `the_run_saves_its_conversation`;
  - `a_scheduled_run_never_navigates`;
  - `catch_up_once_runs_after_two_minutes_and_records_missed`;
  - `no_model_holds_after_three_and_never_falls_back_to_chats_model`;
  - `local_only_with_a_remote_model_skips`.

**Task B4: Durable approvals.**
- Tests:
  - `deferred_writes_make_one_card_with_a_checkbox_each`;
  - `approving_runs_exactly_the_recorded_calls`;
  - `a_gone_target_fails_that_write_alone`;
  - `deny_records_declined`;
  - `approvals_survive_restart_and_are_reposted_after_recover`;
  - `expiry_after_seven_days_cancels_and_says_so`;
  - `engine_approval_decide_is_refused_to_the_agent` (coverage).

**Task B5: The automations model setting.**
- Tests:
  - `the_setting_is_agent_denied`;
  - `check_selection_runs_on_save_and_before_each_run`;
  - `pool_tags_same_free_openrouter_account_shares_chats_limit`;
  - `lm_studio_is_local`;
  - `a_paid_model_needs_allow_paid` (the existing rule).

**Task B6: Commands, coverage, ACL, tools and navigation.**
- Files: `lib.rs`, `permissions/main-window.json`, `agent-coverage.json`, `agent_tools/automations.rs` (new), `navigate.rs`, `events.rs` (`KINDS`), `agent_coverage.rs` (`kind_owner`), `viewTabs.ts`, `navigation.ts`.
- Tests: the previews of the create, update, run and delete tools (schedule and estimate text); `expect_navigation` per tool; the `KINDS` round-trip; `viewTabs.test.ts`.

**Task B7: The Automations view and run page.**
- Files: `features/automations/{AutomationsView.tsx, RunPanel.tsx, NewAutomationDialog.tsx, model.ts, api.ts}`, `app/tests/automations.test.ts`.
- Tests (pure `model.ts`):
  - the state chip and tooltip per state;
  - the When label per schedule;
  - the estimate line;
  - the run page's "why it ran" line (schedule, catch-up with missed, manual, event);
  - Run now disabled while active;
  - fixed-width slot text never empty.
- Screenshot verification happens through the CI screenshot tour if it covers the view; otherwise by the user.

### 4c

**Task C1: Describe to create.**
- Tests, with recorded replies: `a_valid_draft_prefills_the_form_and_saves_nothing`; `an_invalid_schedule_in_the_draft_is_refused_and_the_text_kept`; `the_draft_is_one_automations_call`; `draft_is_disabled_without_a_model`.

**Task C2: Event triggers (E3) and their dispatch.**
- Files: `engine/runner.rs`, `whats_new.rs`, `paper_alerts.rs`.
- Tests: `events_within_a_minute_make_one_run_with_every_subject`; `a_folder_watch_post_reaches_its_automations`; `an_alert_find_reaches_its_automations`; `a_deleted_source_or_alert_holds_the_automation_with_a_reason`.

**Task C3: Templates.**
- *Summarise new papers*: on an alert's finds, one agent step that cites each paper (1 call a run).
- *Weekly workspace digest*: Mondays 09:00, an agent step over the workspace's new and changed sources (up to 4 calls).
- Tests: `templates_save_with_their_estimate`; `the_paper_summary_template_sends_nothing_new_off_the_computer`.

**Task C4: Cache, Show me what it did, Save as automation from a skill.**
- Tests: `a_cache_hit_makes_zero_calls`; `a_new_occurrence_or_changed_source_misses_the_cache`; `show_me_replays_targets_through_the_gate_one_by_one`; `a_skill_becomes_an_agent_step_and_installing_stays_user_only`.

## Acceptance tests (§13 row 4) → tasks

| Acceptance test | Task |
|---|---|
| Table counts survive and the table cap holds | A1 |
| Chat calls are counted and never refused | A2 |
| A run never exceeds its estimate | B3 |
| An automation call waits while a chat answer runs | B3 |
| After a 429 from any spender, automations wait for the reported reset; chat does not | B3 |
| At the chat reserve, automations pause and resume after the reset, with one updated Info item | B3 (needs Q3) |
| The reserve equals the busiest chat day of the last 14 (at least 1) | A1 |
| An agent Write becomes an approval and never runs, in every `AgentMode`; execution tools are absent | B2 |
| Approvals survive restart and are re-posted; expiry cancels | B4 |
| A cache hit makes 0 calls | C4 |
| Estimate and counter text | A3, B6, B7 |
| Recorded model replies only | B3, C1 |
| `automation` coverage and navigation | B6 |

## Decisions only the user can make

These are in the report; the answers are recorded here before Phase 2.

- **Q1.** Split into 4a, 4b and 4c, as above.
- **Q2.** Where the automations model lives, its default, and whether it falls back.
- **Q3.** What replaces the provider-reported remainder for the chat reserve.
- **Q4.** Missed runs.
- **Q5.** How agent writes wait for approval.
- **Q6.** How describe-to-create drafts.
- **Q7.** Whether Run now yields to chat, and what happens when chat starts mid-step.
- **Q8.** Desktop notifications.
- **Q9.** Which templates and step kinds ship in v1.
- **Q10.** Where an automation run's conversation lives.
