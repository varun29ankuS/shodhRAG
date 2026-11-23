# Proactive assistant, PR 1 (engine + repeating tasks) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One scheduler engine in `shodh.db` (typed local-time schedules, catch-up without a stampede, one active run per job, retries on a ladder, hold after repeated failure, readable run history). Its first visible use is repeating tasks. A series makes its next copy when the latest copy is done. When the next one falls due first, the series asks one question with three one-click answers.

**Architecture:**
- **`crates/shodh-rag/src/engine/`** holds:
  - the parts moved from the table runner (lease, backoff, clock);
  - the `Schedule` enum and its next-time maths;
  - `EngineStore`, the SQL over `engine_job`/`engine_run`;
  - the planner, which is pure: due occurrences, catch-up, overlap and the decision fold;
  - `Engine`, which ticks jobs through a `JobHandler` trait.
  Nothing in it knows the app or the calendar.
- **`app/src-tauri/src/task_repeat.rs`** is the `task_repeat` handler. It sits over `CalendarStore` and also holds the five `task_repeat_*` commands.
- **`app/src-tauri/src/engine_commands.rs`** opens the engine on the audit database, runs the loop and receives calendar changes.

Copies are created insert-if-absent under a deterministic id, so the engine never needs a second write path into the calendar.

**Tech stack:**
- Rust: rusqlite, chrono, and `chrono-tz` as a **dev**-dependency of `shodh-rag` (already in `Cargo.lock` at 0.10.4).
- React and TypeScript.
- Tests:
  - shodh-rag unit tests, run by CI "Tests" (`cargo test --workspace --exclude shodh-rag-app --all-targets`);
  - app unit tests, run by CI "App tests and doc tests" (`cargo test --package shodh-rag-app --all-targets`);
  - Node strip-types tests, run by CI "Frontend Typecheck + Tests" (`app/tests/*.test.ts`).
- Clippy runs in CI "Check + Clippy".

**Spec:** `docs/superpowers/specs/2026-11-22-proactive-assistant-design.md`: §4, §5.0–5.3, §5.6, §6, §12, the §13 row "1. Engine + repeating tasks", and the §15 decisions (decision 4 is load-bearing). Migration **19** is the next free number on master `023b5e92`. Open PRs #186, #196 and #198 do not touch `audit/store.rs`.

## Global Constraints

- **Builds and checks:**
  - No local builds. `cargo build/test/check/clippy`, `npm install` and running the app happen in CI only. Each "Run" step names the CI job whose log shows the result.
  - `rustfmt` on changed files and `npx tsc --noEmit` are allowed locally when cheap.
  - Rust only for any tooling. No new Python or shell scripts.
- **Code quality:**
  - No TODOs, placeholders, mocks in production code, or stubs.
  - Mutex locks use `.unwrap_or_else(|e| e.into_inner())`, never a bare `.unwrap()` outside tests.
  - Never weaken a test, threshold or gate to get green.
- **Git:**
  - Commit messages are plain prose subjects, like the repo history, with no generated footers (no Co-Authored-By).
  - All changes go through the PR workflow on `feat/proactive-engine`.
- **Pure-move commits (Tasks 1 and 2)** change no behaviour and no test file:
  - `git diff --stat HEAD~1` must list only the moved-from and moved-to files and `lib.rs`;
  - `jobs_tests.rs`, `fill_tests.rs`, `testkit.rs` and `reminders.rs`'s tests module must show **zero** changed lines.
- **The engine never moves the screen.** A scheduled run posts to the Inbox and rings the reminder alert. It never navigates.
- **`shodh.db` never enables `PRAGMA foreign_keys`** (`audit/store.rs::open_connection`). Every delete of a job deletes its runs explicitly in the same transaction, as `research/tables/store.rs` does for cells and captures. No schema relies on `ON DELETE CASCADE`.
- **Same-task registration.** Every new Tauri command lands in one task, together with:
  - its `lib.rs` registration;
  - its `permissions/main-window.json` entry;
  - its `agent-coverage.json` entry;
  - a literal-string frontend use.
  The ACL gate (`acl_manifest.rs`) and the coverage gate (`agent_coverage.rs`) fail otherwise.
- **The agent never decides for the user.** It never answers a repeat decision without approval (`update_task` with `repeat_next` is a Write-tier tool and asks), and it never resumes a held series without asking.

## Review Focus

1. **The pure move is provably pure.** Tasks 1 and 2 must leave every test file byte-identical. Old paths keep working through `pub use` re-exports:
   - `research::tables::calls::{Clock, SystemClock, utc_day, next_utc_midnight, day_key}`;
   - `research::tables::jobs::{LEASE_MINUTES, BACKOFF_LADDER_MINUTES, STALE_RESET_WAIT_SECS, MAX_PANICS}`;
   - `reminders::resolve_local`.

   `jobs_tests.rs` reaches `timestamp`, `lease_cutoff`, `BACKOFF_LADDER_MINUTES` and `MAX_PANICS` through `use super::*`, so `jobs.rs` must keep importing them. `testkit::ManualClock` implements `super::calls::Clock`, so the re-export must be the same trait, not a copy.
2. **One copy, ever, per occurrence.** A copy's id is `rep-<job>-<yyyymmddhhmm>` (the local occurrence). It is inserted only when absent. This holds through a crash mid-run, a lease takeover, a double event (UI and phone), and reconcile on launch. The tests are in Task 7.
3. **One pending decision per series.** It is the job's single active run: the partial unique index `engine_run_active`. When further occurrences pass, the planner **folds** them into that run: `scheduled_for` moves to the newest occurrence, `missed` grows, and no row is added. The fold is written before the alert rings, so a crash can lose an alert but never ring one twice, matching the "mark fired, then notify" rule in `reminders.rs`. The tests are in Tasks 6 and 7.
4. **Inbox traps.**
   - `inbox_commands::settled_reminders` drops every `reminder` item whose `data.taskId` is not an open task. The decision item therefore carries `data.taskId` = the open latest copy, so completing or deleting that copy settles the item. Its id is `repeat:<job_id>`, never `reminder:<task_id>`, so it cannot collide with the copy's own reminder item or `tauri_host::settled_reminder`. Make and Skip leave the old copy open, so the engine resolves `repeat:<job_id>` itself on every end of the decision.
   - `model.ts` `KINDS` silently skips unknown kinds, so `'automation'` must be added there in the same task as the Rust variant.
5. **Every completion path reaches the hook.** UI `update_task`, the reminder toast's Mark done (`update_task`), the agent's `complete_task`/`update_task` (`save_task_patch`) and the phone's `update_task` (`phone_link.rs:1174`, which calls `calendar_commands::update_task`) all end in `TauriEffects::calendar_changed`. The only calendar writes that bypass it are `reminders.rs` `mark_fired`/`snooze_reminder`, which never change `status`. Task 7 hooks there and nowhere else.
6. **The launch barrier must never swallow today's missed-reminder notice.** The engine's half reports through a drop guard on every exit path (Task 7, `a_failing_engine_still_lets_the_missed_reminder_summary_through`).
7. **An event never opens a decision.** A `task_completed` run that finds an open subject (a second completion after the copy was made) does nothing and leaves no History row. Only a passed occurrence opens a decision.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/shodh-rag/src/engine/mod.rs` (create) | Module root; re-exports. |
| `crates/shodh-rag/src/engine/lease.rs` (create, Task 1 move) | `LEASE_MINUTES`, `PROCESS_TOKEN`/`process_token`, `LiveRun` (the live-run slot), `timestamp`, `lease_cutoff`. |
| `crates/shodh-rag/src/engine/backoff.rs` (create, Task 1 move) | `BACKOFF_LADDER_MINUTES`, `STALE_RESET_WAIT_SECS`, `MAX_PANICS`. |
| `crates/shodh-rag/src/engine/clock.rs` (create, Task 1 move; Task 2 adds `resolve_local`) | `Clock`, `SystemClock`, `utc_day`, `next_utc_midnight`, `day_key`, `resolve_local`. |
| `crates/shodh-rag/src/engine/schedule.rs` (create) | `Schedule` (typed, serde-tagged), validation, `next_after`, `occurrences_in`, `describe`. |
| `crates/shodh-rag/src/engine/store.rs` (create) | `EngineStore`: jobs, runs, claim/beat/finish, wait, fold, hold, delete, retention. |
| `crates/shodh-rag/src/engine/plan.rs` (create) | Pure planner: what is due at `now`, catch-up, overlap, the decision fold, retries, hold, `sleep_for`. |
| `crates/shodh-rag/src/engine/runner.rs` (create) | `Engine` (tick, event, wake time) and the `JobHandler` trait. |
| `crates/shodh-rag/src/engine/testkit.rs` (create, `#[cfg(test)]`) | `ManualClock` and a recording `JobHandler` for engine tests. Test code only. |
| `crates/shodh-rag/src/research/tables/jobs.rs`, `calls.rs` (modify, Task 1) | Moved items deleted; `use`/`pub use` from `crate::engine`. |
| `crates/shodh-rag/src/lib.rs` (modify) | `pub mod engine;`. |
| `crates/shodh-rag/Cargo.toml` (modify, Task 3) | `[dev-dependencies] chrono-tz = "0.10"`. |
| `crates/shodh-rag/src/audit/store.rs` (modify, Task 4) | Migration 19 and its tests. |
| `crates/shodh-rag/src/inbox.rs` (modify, Task 4) | `InboxKind::Automation`. |
| `app/src-tauri/src/reminders.rs` (modify, Tasks 2 and 7) | `pub use` of `resolve_local`; the launch barrier for the missed summary (Task 7). |
| `app/src-tauri/src/calendar_store.rs` (modify, Task 7) | `CalendarData::insert_task_if_absent(id, NewTask, subtasks)`. |
| `app/src-tauri/src/task_repeat.rs` (create) | The `task_repeat` handler, the snapshot, copy ids, decision item and alert, the calendar-change hook, launch reconcile, and the five commands. |
| `app/src-tauri/src/engine_commands.rs` (create) | `EngineState` (opens `EngineStore` on the audit database like `InboxState`), the loop (sleeps at most `MAX_SLEEP`, `Notify` wake), `wake(app)`. |
| `app/src-tauri/src/agent_tools/tauri_host.rs` (modify, Task 7) | `calendar_changed` calls `task_repeat::on_calendar_change`. |
| `app/src-tauri/src/agent_tools/mod.rs`, `calendar.rs` (modify, Task 11) | `HostEffects::task_repeat` access; `repeat`/`repeat_next` arguments; `list_tasks` and `delete_task` text. |
| `app/src-tauri/src/lib.rs`, `permissions/main-window.json`, `agent-coverage.json` (modify, Task 8) | Commands, ACL and coverage. |
| `app/src/features/tasks/repeat.ts` (create) | Pure: schedule label, options from a due date, state of a series, history row view model. |
| `app/src/features/tasks/RepeatField.tsx`, `RepeatHistory.tsx` (create) | Repeat and History in the task sheet. |
| `app/src/features/tasks/TaskDetailSheet.tsx`, `TasksStore.tsx`, `types.ts` (modify) | Mounting; API wrappers with literal command names; wire types. |
| `app/src/features/tasks/ReminderAlerts.tsx` (modify, Task 10) | Decision variant of the reminder toast (three answers, no Snooze). |
| `app/src/features/inbox/model.ts`, `InboxButton.tsx` (modify, Tasks 4 and 10) | `'automation'` kind; `choices` for decision and hold items. |
| `app/tests/taskRepeat.test.ts` (create), `app/tests/inbox.test.ts` (modify) | Frontend model tests. |

**Decisions recorded here (the spec does not settle them):**
- **Date-only tasks** repeat on dates. Their occurrence instant is 00:00 local. A copy's due is `YYYY-MM-DD`.
- **Repeat needs a due date.** Without one, the field is disabled with the tooltip "Set a due date first".
- **The task UI offers** every day, every weekday, every week on chosen days, and every month on day N or the last day. "Every N hours" and "once" exist in `Schedule` for PR 4, but tasks do not offer them.
- **Monthly day 29–31** falls on the month's last day in shorter months (31 Jan → 28 Feb 2027, 29 Feb 2028).
- **Copies of the series:**
  - The series knows its copies by the id prefix `rep-<job>-`. It knows its first task through `input.origin_task_id`.
  - `task_repeat_get` on any of them returns the series.
  - "Changing Repeat on any copy changes the series" works through this lookup.
- **Planned-through.** The planner's lower bound is `max(job.created_at, max(engine_run.scheduled_for))`. Turning Repeat on writes one `succeeded` run (trigger `manual`, "Repeat turned on by you") at the latest copy's due occurrence. This anchors the plan and starts History. Retention keeps the latest 20 runs per job, so the maximum is never pruned.
- **Run summaries are written by Rust in plain words**, for example "Made when last week's was done (Fri 21 Nov)". The sheet, the agent's `list_tasks` and the Inbox show the same sentence. The copy a run made is derived from `(job_id, scheduled_for)`, so History's [Open] needs no extra column.
- **Launch reconcile** runs for each `task_repeat` job:
  - If the subject is missing, the series is re-pointed to the newest existing copy (`rep-<job>-…` or the origin). It is stopped only when none exists. A calendar restored from its backup therefore never deletes a series.
  - If the subject is completed and has no next copy, the copy is made.
- **`previous_label`** names the open copy relative to the occurrence. It reads "yesterday's", "last week's" or "last month's" when the copy is exactly one period before. Otherwise it reads "the one due Mon 10 Nov".

---

### Task 1: Pure move. Lease, backoff and clock leave the table runner for `shodh_rag::engine`

**Files:**
- Create: `crates/shodh-rag/src/engine/{mod.rs,lease.rs,backoff.rs,clock.rs}`
- Modify: `crates/shodh-rag/src/lib.rs` (`pub mod engine;`), `crates/shodh-rag/src/research/tables/jobs.rs:58-115,355-363` (items removed, `use crate::engine::...`; `pub use` for the public constants), `crates/shodh-rag/src/research/tables/calls.rs:85-118` (`Clock`, `SystemClock`, `utc_day`, `next_utc_midnight`, `day_key` removed; `pub use crate::engine::clock::{...}`)
- Test: none changed. `jobs_tests.rs`, `fill_tests.rs`, `calls.rs` tests module, `testkit.rs` and every external caller (11 files import `tables::calls::*` clock items) stay byte-identical.

**What moves (verbatim, doc comments included):**
- `lease.rs`: `LEASE_MINUTES`, `PROCESS_TOKEN`, `process_token` (from `pub(super)` to `pub(crate)`), `LIVE_RUNS`, `live_runs`, `LiveRun` (the struct, `enter` and `is_live` become `pub(crate)`; the bodies are unchanged), `timestamp`, `lease_cutoff`.

**Re-exports, so that no importer changes.** The only importers found are `tables/schedule.rs` (`super::jobs::{process_token, timestamp, ...}`), `app/src-tauri/src/table_commands.rs` and `notifications.rs` (which import only items that stay), and the 11 files that import clock items from `tables::calls`.
- In `jobs.rs`:
  - `pub use crate::engine::lease::LEASE_MINUTES;`
  - `pub use crate::engine::backoff::{BACKOFF_LADDER_MINUTES, STALE_RESET_WAIT_SECS, MAX_PANICS};`
  - `pub(super) use crate::engine::lease::{process_token, timestamp};` (keeps `schedule.rs` unchanged)
  - `use crate::engine::lease::{lease_cutoff, LiveRun, PROCESS_TOKEN};` (`jobs_tests.rs` reaches `lease_cutoff` and `timestamp` through `use super::*`)
- In `calls.rs`:
  - `pub use crate::engine::clock::{day_key, next_utc_midnight, utc_day, Clock, SystemClock};`
- `backoff.rs`: `BACKOFF_LADDER_MINUTES`, `STALE_RESET_WAIT_SECS`, `MAX_PANICS`.
- `clock.rs`: `Clock`, `SystemClock`, `utc_day`, `next_utc_midnight`, `day_key`.

**What stays, and why** (departure D1): these are typed on table structs or are `table_fill_job` SQL, so moving them would make `engine` depend on `research::tables`:
- `RunnerSource`, `RunEvents` and `RunnerError` (`schedule.rs`);
- `run_is_gone`, `hold_of` and `Hold` (they read `FillJob`);
- `next_rung` and `Streaks` (`JobRunner` internals);
- the claim, heartbeat, finish and panic SQL;
- `MAX_FAILING_PAPERS` (papers).

The engine writes its own SQL against `engine_run` using the moved constants.

- [ ] **Step 1:** Move the items, adjust visibility (`pub(super)` → `pub(crate)` only), and add the imports and re-exports.
- [ ] **Step 2:** Locally, `git diff --stat` must show changes only in `lib.rs`, `engine/*`, `jobs.rs` and `calls.rs`. `git diff -- '*_tests.rs' crates/shodh-rag/src/research/tables/testkit.rs` must be empty.
- [ ] **Step 3: Run** CI "Tests", "App tests and doc tests" and "Check + Clippy". Expected: green, with the same test count as master in the `research::tables` modules.
- [ ] **Step 4: Commit** "Engine: move the table runner's lease, backoff and clock into shodh_rag::engine".

### Task 2: Pure move. `resolve_local` moves from the app into `engine::clock`

**Files:**
- Modify: `app/src-tauri/src/reminders.rs:71-88` (the function moves out; `pub use shodh_rag::engine::clock::resolve_local;`), `crates/shodh-rag/src/engine/clock.rs`
- Test: `reminders.rs` tests module unchanged (`local_times_resolve_across_daylight_saving` keeps proving it through the re-export).

- [ ] **Step 1:** Move the function verbatim.
- [ ] **Step 2: Run** CI "App tests and doc tests". Expected: every `reminders::tests` test passes unchanged.
- [ ] **Step 3: Commit** "Engine: resolve_local moves into engine::clock for schedules".

### Task 3: Typed schedules and their next times

**Files:**
- Create: `crates/shodh-rag/src/engine/schedule.rs`
- Modify: `crates/shodh-rag/Cargo.toml` (`[dev-dependencies] chrono-tz = "0.10"`)
- Test: `schedule.rs` tests module

**Interfaces:**
- `#[serde(tag = "kind", rename_all = "snake_case")] pub enum Schedule`, with these variants:
  - `Once { at: NaiveDateTime }`
  - `Daily { at: NaiveTime }`
  - `Weekdays { at: NaiveTime }`
  - `Weekly { days: Vec<Weekday> /* non-empty, sorted, unique */, at: NaiveTime }`
  - `Monthly { day: MonthDay /* Day(1..=31) | Last */, at: NaiveTime }`
  - `EveryHours { hours: u32 /* 1..=24 */, from: DateTime<Utc> }`
- Its functions:
  - `pub fn validate(&self) -> Result<(), ScheduleError>`
  - `pub fn next_after<Tz: TimeZone>(&self, tz: &Tz, after: DateTime<Utc>) -> Option<DateTime<Utc>>`
  - `pub fn occurrences_in<Tz: TimeZone>(&self, tz: &Tz, after: DateTime<Utc>, upto: DateTime<Utc>, cap: usize) -> (Vec<DateTime<Utc>>, bool /* more */)`
  - `pub fn local_key<Tz>(&self, tz: &Tz, at: DateTime<Utc>) -> String` (`yyyymmddhhmm`, the copy-id suffix)
  - `pub fn describe(&self) -> String` ("Every week on Mon at 09:00")
- Calendar schedules walk local dates and resolve each with `resolve_local`: a gap goes to the first minute after it, and an ambiguous time to its first instance. `EveryHours` counts real hours from `from`, so daylight saving never shifts it. Production passes `chrono::Local`. Tests pass `chrono_tz` zones.

- [ ] **Step 1: Write the failing tests:**
  - `daily_times_across_dst_gaps_and_overlaps`:
    - Europe/London: 01:30 on 29 Mar 2026 → 02:00 BST; 01:30 on 25 Oct 2026 → the first (BST) instance only.
    - America/New_York: 02:30 on 8 Mar 2026 → 03:00 EDT; 01:30 on 1 Nov 2026 → once.
    - Asia/Kolkata: no DST, plain +05:30.
  - `every_hours_counts_real_hours_across_dst`: London, every 2 h through the fall-back night gives 2 h between UTC instants.
  - `weekdays_skip_the_weekend`.
  - `weekly_on_chosen_days_in_order`.
  - `monthly_day_31_falls_on_the_last_day_of_short_months`: Jan 31 → Feb 28 2027 → Mar 31.
  - `monthly_last_day_in_leap_and_common_years`: Feb 29 2028, Feb 28 2027, Feb 28 2100 (not a leap year).
  - `occurrences_in_counts_a_three_day_gap`: daily, 3 passed → 3, `more` false; every-1-h over a year with `cap` 1000 → 1000, `more` true.
  - `schedules_round_trip_as_tagged_json`: stable JSON per variant.
  - `invalid_schedules_are_refused`: empty days, day 0 or 32, hours 0 or 25.
  - `describe_reads_plainly`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** CI "Tests". Expected: the new tests pass.
- [ ] **Step 4: Commit** "Engine: typed schedules with local-time next occurrences".

### Task 4: Migration 19 (engine tables, Inbox kinds incl. the `model` fix) and `InboxKind::Automation`

**Files:**
- Modify: `crates/shodh-rag/src/audit/store.rs` (append migration 19; update the `MIGRATIONS` doc comment), `crates/shodh-rag/src/inbox.rs` (`Automation` variant, `as_str`/`parse`), `app/src/features/inbox/model.ts` (`KINDS` gains `'automation'`)
- Test: `audit/store.rs` tests, `inbox.rs` tests, `app/tests/inbox.test.ts`

**Schema (migration 19).** As §5.2, with these changes:
- `engine_job` gains `input_json TEXT NOT NULL DEFAULT '{}'` (departure D5).
- No `REFERENCES ... ON DELETE CASCADE`, because cascades are inert here (D5).
- `engine_step`, `engine_approval` and `engine_cache` are **not** created here (D6, ruled): they arrive with PR 4's migration.

```
engine_job(id PK, kind CHECK IN ('task_repeat','folder_watch','paper_alert','playbook','daily_review','system'),
  subject, title, schedule_json NULL, trigger_json NULL, input_json NOT NULL DEFAULT '{}', enabled 0|1,
  overlap CHECK IN ('skip','queue_one'), catch_up CHECK IN ('once','skip'), max_attempts CHECK 1..5,
  next_due_at NULL, hold_reason NULL CHECK IN ('user','failing','model_unavailable','offline'),
  failure_streak >= 0, created_by CHECK IN ('user','agent','system'), provenance_json, created_at, updated_at)
  INDEX engine_job_subject(kind, subject)
engine_run(id PK, job_id NOT NULL, trigger CHECK IN ('schedule','catch_up','manual','event','retry'),
  scheduled_for NULL, missed >= 0, state CHECK IN ('queued','running','waiting','succeeded','failed','skipped','cancelled'),
  wait_reason NULL CHECK IN ('approval','decision','yield','quota','table_job','model_unavailable','offline','backoff'),
  attempt >= 0, next_attempt_at NULL, run_token NULL, heartbeat_at NULL, planned_calls >= 0, used_calls >= 0,
  conversation_id NULL, summary NULL, error_code NULL, error_detail NULL, started_at NULL, ended_at NULL, created_at)
  INDEX engine_run_job(job_id, created_at)
  UNIQUE INDEX engine_run_active ON engine_run(job_id) WHERE state IN ('queued','running','waiting')
inbox_items rebuilt (as 18 did) with kinds + 'model', 'automation', 'whats_new', 'review'.
```

- [ ] **Step 1: Write the failing tests:**
  - `the_inbox_is_rebuilt_for_engine_items_with_its_rows`, modelled on `the_inbox_is_rebuilt_for_connector_items_with_its_rows`:
    - a version-18 database holds one row of every kind 18 allows;
    - after migrating, every row is kept field for field;
    - `'model'`, `'automation'`, `'whats_new'` and `'review'` insert;
    - an unknown kind is still refused.
  - `a_model_item_is_stored`, in `inbox.rs`: `InboxStore::put` of `InboxKind::Model` succeeds and lists back. This is the §4 bug regression; it fails on master.
  - `the_index_refuses_a_second_active_run`: a raw insert of a second `queued`/`running`/`waiting` run for one job fails, and a second `succeeded` run inserts.
  - In `inbox.test.ts`, an `'automation'` item parses and is not skipped.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** CI "Tests" and "Frontend Typecheck + Tests".
- [ ] **Step 4: Commit** "Engine: migration 19 adds jobs and runs; the Inbox stores model and automation items".

### Task 5: `EngineStore`

**Files:**
- Create: `crates/shodh-rag/src/engine/store.rs`
- Test: `store.rs` tests module (temp `shodh.db` through `open_shared_connection`)

**Interfaces** (each one write transaction):
- Jobs:
  - `create_job(NewJob) -> Job`
  - `job(id)`
  - `jobs_of_kind(kind)`
  - `job_by_subject(kind, subject)`
  - `update_job(id, JobPatch)`
  - `delete_job(id) -> bool` (deletes runs first)
  - `set_hold(id, Option<HoldReason>)`
  - `record_failure(id) -> streak`
  - `reset_failures(id)`
- Runs:
  - `insert_run(NewRun) -> Result<Run, ActiveRun(Run)>` (a unique-index conflict returns the active run)
  - `claim(run_id, token, now, cutoff) -> Option<Run>` (`queued`, due `waiting/backoff`, or `running` with a stale lease or this token with no live slot; `LiveRun` from Task 1)
  - `beat(run_id, token, now) -> bool`
  - `wait(run_id, token, WaitReason, next_attempt_at)`
  - `finish(run_id, token, End)`, where `End` is `Succeeded{summary}`, `Failed{code, detail}`, `Skipped{summary}` or `Cancelled{summary}`
  - `fold(run_id, scheduled_for, missed_add) -> Run`
  - `active_run(job_id)`
  - `runs(job_id, limit)`
  - `latest_scheduled_for(job_id)`
  - `prune(now)`: drops runs older than 90 days but keeps the latest 20 per job and every active run.

- [ ] **Step 1: Write the failing tests:**
  - `a_live_lease_is_never_taken_and_a_stale_one_is_after_ten_minutes`: token B is refused at 9:59 and claims at 10:00, using `LEASE_MINUTES`.
  - `a_second_active_run_returns_the_first`.
  - `deleting_a_job_deletes_its_runs`.
  - `retention_keeps_ninety_days_and_never_fewer_than_twenty_per_job`.
  - `fold_moves_to_the_newest_occurrence_and_counts_missed`.
  - `three_failures_in_a_row_are_counted_and_reset_by_success`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** CI "Tests".
- [ ] **Step 4: Commit** "Engine: a store of jobs and runs with leases, holds and retention".

### Task 6: The planner and the engine loop core

**Files:**
- Create: `crates/shodh-rag/src/engine/plan.rs`, `runner.rs`, `testkit.rs`
- Test: `plan.rs` and `runner.rs` tests modules

**Interfaces:**
- `#[async_trait] pub trait JobHandler: Send + Sync`, with:
  - `fn kind(&self) -> JobKind`
  - `async fn run(&self, job: &Job, run: &Run) -> Handled`
  - `async fn folded(&self, job: &Job, run: &Run)` (a waiting decision moved to a newer occurrence: update the item, ring once)
  - `async fn held(&self, job: &Job, reason: HoldReason, last_error: &RunError)`
- `pub enum Handled`:
  - `Succeeded { summary }`
  - `Waiting { reason: WaitReason }` (here only `Decision`)
  - `Skipped { summary }`
  - `Transient { code, detail }`
  - `Permanent { code, detail }`
- `Engine::new(store, clock, token)`.
- `Engine::tick(&self, handlers) -> Tick`:
  - plans every enabled, unheld job;
  - starts due runs one at a time, ordered by `scheduled_for`;
  - resumes due `backoff` waits;
  - takes over stale leases.
- `Engine::event(job_id, Event) -> Result<Run>`:
  - when the active run is `waiting/decision`, it is handed back to the handler (trigger kept);
  - otherwise a new `event` run is inserted and run at once, with no one-minute coalescing (D7).
- `Engine::next_wake(now) -> Option<DateTime<Utc>>` and `plan::sleep_for(next, now)`, which is at most 60 s (`reminders::MAX_SLEEP`'s rule).

**Rules:**
- **Due occurrences** are `schedule.occurrences_in(Local, lower_bound, now, cap = 1000)`.
  - More than 5 min late counts as catch-up.
  - `catch_up = once` makes one run for the latest occurrence, with `missed` = the others.
  - `catch_up = skip` makes one `skipped` run, "Skipped: Shodh wasn't running at …".
- **Overlap**, when an occurrence passes while a run is active:
  - if the active run is `waiting/decision`, it is folded (D8): `scheduled_for` moves to the newest, `missed` grows, and `folded` is called after the write;
  - otherwise `skip` records a skipped run "The previous run was still going", and `queue_one` keeps at most one queued run.
- **Retries:** `Transient` waits `backoff` at `BACKOFF_LADDER_MINUTES[attempt-1]`, up to `max_attempts`. After that the run `failed`.
- **Hold:** at `record_failure` = 3 the job is held as `failing` and `held` is called once. `Succeeded` resets the streak.
- `next_due_at` is rewritten on every plan as a cache.

- [ ] **Step 1: Write the failing tests**, using `ManualClock` and the recording handler:
  - `a_three_day_sleep_makes_one_run_with_two_missed`;
  - `catch_up_skip_records_one_skipped_run`;
  - `an_occurrence_while_a_decision_waits_folds_into_it`: one active run, `missed` 2, `folded` twice, no new rows;
  - `skip_overlap_records_the_previous_run_was_still_going`;
  - `queue_one_keeps_at_most_one_behind`;
  - `transient_failures_climb_the_ladder_then_fail`;
  - `three_failed_runs_hold_the_job_and_tell_once`;
  - `a_run_abandoned_mid_way_is_resumed_once_after_the_lease`: the handler records its effect, then the run is left `running`; another token re-runs it after `LEASE_MINUTES`, not before, exactly once;
  - `an_event_answers_a_waiting_decision_instead_of_starting_a_run`;
  - `sleep_is_capped_at_a_minute`;
  - `due_runs_start_one_at_a_time_in_order`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** CI "Tests".
- [ ] **Step 4: Commit** "Engine: plan due runs with catch-up, overlap, retries and hold".

### Task 7: Repeating tasks in the app (handler, hook, launch reconcile, decision item and alert)

**Files:**
- Create: `app/src-tauri/src/task_repeat.rs`, `app/src-tauri/src/engine_commands.rs`
- Modify:
  - `app/src-tauri/src/calendar_store.rs`: `insert_task_if_absent(&mut self, id: &str, new: NewTask, subtasks: Vec<SubTask>) -> CalendarResult<(TodoItem, bool /* inserted */)>`, validating as `insert_task` does.
  - `app/src-tauri/src/agent_tools/tauri_host.rs:283`: `calendar_changed` → `crate::task_repeat::on_calendar_change(&self.app, &change)`.
  - `app/src-tauri/src/reminders.rs`: a launch barrier, so the closed-app summary is one notification for both.
  - `app/src-tauri/src/lib.rs`: manage `EngineState` after `InboxState`; spawn the loop; reconcile on launch.
- Test: `task_repeat.rs` tests module (temp `CalendarStore`, temp `shodh.db`, `ManualClock`, a recording `RepeatSink`), `calendar_store.rs` tests

**Behaviour:**
- **The snapshot** (`input_json`) is taken from the latest copy. It holds title, description, tags, priority, workspace, unchecked subtasks, `all_day`, the due time of day, the reminder offset (due − reminder, in minutes) and `origin_task_id`.
- **Edits.** `TaskSaved` of the subject refreshes the snapshot. That covers edits and `detach_workspace_tasks` (which emits `TaskSaved` per task). `TaskSaved` of an older copy does nothing.
- **Completion:**
  - `TaskSaved` of the subject with `status == completed` calls `Engine::event(TaskCompleted)`.
  - The handler is **state-based**: on any run it reads the calendar.
    - **Event trigger:**
      - a completed subject means make the copy;
      - an **open subject means do nothing**: no copy, no decision, and no run row (the run is cancelled and pruned, never shown in History). A UI and a phone completion arriving together therefore make one copy and never a bogus decision.
    - **Schedule, catch-up or reconcile trigger:**
      - a completed subject means make the copy;
      - an open subject means wait for a decision.
  - A missed event is therefore made good at the next occurrence or launch.
- **Making a copy:**
  1. Insert `rep-<job>-<local key>` if absent.
  2. Point `subject` at it.
  3. `calendar_changed(TaskSaved(copy))`, so it is indexed and views refresh.

  The order of these steps is what makes a crash between them harmless.
- **The target occurrence** of every copy is the next upcoming one: `next_after(max(now, planned_through))` (D9, ruled). A new copy is never overdue when it appears. The run that made it records the copy's occurrence as `scheduled_for`.
- **Deleting:** `TaskRemoved` of the subject deletes the job (stop the series) and resolves `repeat:<job>`. `TaskRemoved` of an older copy does nothing.
- **The decision.** The run waits as `decision`. The Inbox item is posted:
  - id `repeat:<job>`, kind `reminder`, status `needs_you`;
  - title "*Submit timesheet* was due again today, but last week's isn't done.";
  - `data`: `{taskId: <open latest copy>, jobId, choices: ['make','skip','mark_done'], occurrence, missed}`;
  - link: the task.

  The reminder alert then rings (`REMINDER_FIRED_EVENT` with `repeat: {jobId, occurrence}`), plus `notifications::notify(Reminders, …)`. On a fold, the item is updated ("due again twice since") and rings once.
- **Answers** (`task_repeat_decide`):
  - `make`: make the copy for the next upcoming occurrence (D9); run `succeeded` "You chose Make the next one (the one before wasn't done)".
  - `skip`: run `skipped` "Skipped by you"; the series waits for the next occurrence.
  - `mark_done`: complete the open copy through `update_task`'s path, so it is recorded like any completion; the event answers the run with "Made when last week's was done (…)".
  - **Every way the decision ends resolves `repeat:<job>`:** make, skip, completion, the series stopped or cleared, and a hold.
    - It cannot be left to `settled_reminders`. After Make or Skip the old copy is still open, so the item would otherwise stay as an answered question.
    - The item is resolved after the run's end is written, so a crash re-posts it at launch rather than losing it.
- **Locked or unreadable calendar:**
  - `CalendarError::Io`/`Unreadable` is `Transient("calendar_unreadable")`.
  - After 3 failed runs the series is held, with an Attention item `repeat-hold:<job>`, kind `automation`: "Couldn't make *Submit timesheet*'s next copy: the task list couldn't be read. [Resume]".
  - While waiting, the row reads "Couldn't make this week's copy: the task list couldn't be read. Trying again in N minutes."
- **Launch:**
  - Reconcile (see Decisions).
  - Re-post the item of every waiting decision (`put` is an upsert, so it repeats safely).
  - Occurrences that passed while closed fold into the one decision.
  - That decision's alert goes to `reminders`' launch summary through a **two-party barrier** in `ReminderState`: the reminders loop's first plan and the engine's first plan each hand over their missed list, and whichever arrives second sends the one "Missed while Shodh was closed" notification.
    - No timeout is involved.
    - The engine side reports on **every** exit path through a drop guard: success, error, panic, and `shodh.db` unavailable (empty). A failing engine therefore never swallows today's missed-reminder notice.

- [ ] **Step 1: Write the failing tests:**
  - `insert_if_absent_never_makes_two`;
  - `completing_the_latest_copy_makes_exactly_one_next_copy_at_the_following_occurrence`;
  - `completing_twice_ui_then_phone_change_makes_one_copy`: also asserts no decision item and exactly one new History row;
  - `an_occurrence_with_the_last_copy_open_makes_no_copy_and_one_decision`;
  - `make_the_next_one_creates_the_copy_for_the_next_upcoming_occurrence`;
  - `a_copy_completed_late_is_due_at_the_next_upcoming_occurrence`: never overdue when it appears;
  - `skip_this_one_records_skipped_and_waits_for_the_next`;
  - `mark_last_done_completes_it_and_makes_the_next_upcoming_copy`;
  - each of the three answer tests also asserts that `repeat:<job>` is gone from the Inbox;
  - `a_failing_engine_still_lets_the_missed_reminder_summary_through`;
  - `an_ignored_decision_across_two_more_occurrences_stays_one_item_and_one_possible_copy`;
  - `a_three_day_sleep_folds_into_the_same_decision`: `missed` = 2 and the alert rings once; with the app closed, the alert also reaches the launch summary;
  - `completing_from_the_phone_answers_the_decision` (drives `on_calendar_change` with the `TaskSaved` the phone's `update_task` produces; see D14);
  - `a_crash_mid_run_makes_no_duplicate_copy`: the copy is inserted, the run is left `running`, and the takeover re-runs it, so there is still one copy and `subject` points at it;
  - `a_locked_calendar_retries_then_holds_after_three_runs`;
  - `editing_the_latest_copy_refreshes_the_snapshot_and_older_edits_do_not`;
  - `deleting_the_latest_copy_stops_the_series_and_removes_its_decision`;
  - `deleting_an_older_copy_keeps_the_series`;
  - `detaching_a_workspace_clears_it_in_the_snapshot`;
  - `launch_reconcile_repoints_a_missing_subject_and_finishes_a_half_made_copy`;
  - `copy_reminders_keep_their_offset_from_due`;
  - `all_day_tasks_repeat_as_dates`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** CI "App tests and doc tests".
- [ ] **Step 4: Commit** "Tasks repeat: the next copy is made when the last one is done, or Shodh asks".

### Task 8: The five commands, ACL, coverage and the frontend API

**Files:**
- Modify: `app/src-tauri/src/task_repeat.rs` (commands), `app/src-tauri/src/lib.rs` (register), `app/src-tauri/permissions/main-window.json`, `app/src-tauri/agent-coverage.json`, `app/src/features/tasks/TasksStore.tsx` (wrappers with literal names), `app/src/features/tasks/types.ts`
- Test: the existing gates (`agent_coverage.rs`, `acl_manifest.rs`) and `task_repeat.rs` command tests

**Commands:**
- `task_repeat_get(task_id) -> Option<RepeatView>`, where `RepeatView` holds:
  - `jobId`, `schedule`, `label`, `nextDueAt`;
  - `state`: `active` | `waiting` | `held`;
  - `decision?`: `{occurrence, missed, openTaskId, previousLabel}`;
  - `hold?`: `{reason, detail}`;
  - `isLatest`.
- `task_repeat_set(task_id, schedule)`:
  - creates the series, or changes it from any copy;
  - re-saving clears a hold (D10);
  - refuses a task without a due date.
- `task_repeat_clear(task_id)`: stops the series and keeps every copy.
- `task_repeat_runs(task_id, limit) -> Vec<RunView>`.
- `task_repeat_decide(job_id, choice: make|skip|mark_done)`.

**Coverage:**

| Command | Maps to |
|---|---|
| `task_repeat_get` | `list_tasks` |
| `task_repeat_set` | `create_task`, `update_task` |
| `task_repeat_clear` | `update_task` |
| `task_repeat_runs` | `list_tasks` |
| `task_repeat_decide` | `update_task`, `complete_task` |

- [ ] **Step 1:** Write command tests:
  - `set_from_an_older_copy_changes_the_series`;
  - `set_refuses_a_task_without_a_due_date`;
  - `set_on_a_held_series_resumes_it`;
  - `clear_keeps_copies`.
- [ ] **Step 2:** Implement. Register in all four places in this commit.
- [ ] **Step 3: Run** CI "App tests and doc tests" (the coverage and ACL gates are there) and "Frontend Typecheck + Tests".
- [ ] **Step 4: Commit** "Tasks repeat: commands, with agent coverage and window grants".

### Task 9: Repeat and History in the task sheet

**Files:**
- Create: `app/src/features/tasks/repeat.ts`, `RepeatField.tsx`, `RepeatHistory.tsx`, `app/tests/taskRepeat.test.ts`
- Modify: `app/src/features/tasks/TaskDetailSheet.tsx` (Repeat under Due, History as the last section before the footer, so a new run never pushes another field), `TasksStore.tsx` (refresh the repeat view on `calendar-changed` and `inbox-changed`)
- Test: `app/tests/taskRepeat.test.ts`

- [ ] **Step 1: Write the failing tests** for `repeat.ts`:
  - `labelOf` for every offered schedule;
  - `optionsFromDue`: from Mon 24 Nov 17:00 it offers "Every day", "Every weekday", "Every week on Mon", "Every month on the 24th"; on the 29th–31st it also offers "Every month on the last day";
  - `seriesState`: no series, active, waiting, held, and no due date;
  - `historyRow`: one per run state (made, chosen, skipped, failed with reason, waiting);
  - `slotText`: always one line, including the empty state.
- [ ] **Step 2: Implement** to the UX section below.
- [ ] **Step 3: Run** CI "Frontend Typecheck + Tests".
- [ ] **Step 4: Commit** "Tasks repeat: Repeat and History in the task sheet".

### Task 10: The decision and the hold in the Inbox and the reminder toast; delete wording

**Files:**
- Modify:
  - `app/src/features/inbox/model.ts`: `ItemActions.choices?: Choice[]` for `reminder` items with `data.choices`, and `[Resume]` for `automation` holds; neither has Dismiss.
  - `app/src/features/inbox/InboxButton.tsx`.
  - `app/src/features/tasks/ReminderAlerts.tsx`: when `repeat` is present, the toast shows the three answers and no Snooze.
  - `app/src/features/tasks/TasksStore.tsx`: the undo toast for deleting the latest copy reads "Deleted *Submit timesheet*. It won't repeat. [Undo]" (D11).
- Test: `app/tests/inbox.test.ts`, `app/tests/taskRepeat.test.ts`

- [ ] **Step 1: Write the failing tests:**
  - `decision_items_offer_three_choices_and_no_dismiss`;
  - `hold_items_offer_resume`;
  - `plain_reminders_keep_dismiss`;
  - `the_delete_toast_says_it_stops_repeating_only_for_the_latest_copy`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** CI "Frontend Typecheck + Tests".
- [ ] **Step 4: Commit** "Tasks repeat: one-click answers in the Inbox and the reminder".

### Task 11: Agent tools

**Files:**
- Modify:
  - `app/src-tauri/src/agent_tools/mod.rs`: `HostEffects` gains `task_repeat(&self) -> Option<Arc<dyn RepeatAccess>>`, with no default. There are exactly two implementors: `TauriEffects` (`tauri_host.rs:75`) and the test `Recorder` (`agent_tools/mod.rs:379`), which returns a temp-store implementation. Both change in this task.
  - `app/src-tauri/src/agent_tools/calendar.rs`:
    - `create_task`/`update_task` take `repeat` (the `Schedule` JSON, or `"none"` on update); the preview reads "Repeats every Monday at 09:00".
    - `update_task` takes `repeat_next: make|skip`; the preview reads "Make the next *Submit timesheet* (the last one isn't done)".
    - `list_tasks` shows the repeat, the next due and a pending decision.
    - The `delete_task` preview adds "and stop repeating" when the task is the latest copy.
- Test: `calendar.rs` tests module (`expect` previews and texts)

- [ ] **Step 1: Write the failing tests:**
  - `create_task_with_repeat_previews_the_schedule`;
  - `update_task_repeat_none_stops_the_series`;
  - `repeat_next_make_and_skip_answer_the_decision_after_approval`;
  - `list_tasks_shows_repeat_next_and_decision`;
  - `delete_task_preview_says_and_stop_repeating_for_the_latest_copy_only`;
  - `repeat_on_a_task_without_due_is_refused_with_the_reason`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** CI "App tests and doc tests".
- [ ] **Step 4: Commit** "Tasks repeat: the assistant sets, reads and answers repeats with approval".

---

## UX: Repeat and History in the task sheet

**Layout.** Fixed slots, so nothing moves when the state changes.

```
 Due      Mon 24 Nov, 17:00
 Repeat   [Every week on Mon v]  (i)          <- the control never changes width (min-w, truncates)
          Next  Mon 1 Dec, 17:00             <- state line: always rendered, one line, truncates (full text in the tooltip)
          [                                ]  <- action row: always reserved at one button height, empty in states with no fix
 ...      Notes, Project, Subtasks, Workspace, Ask (unchanged)
 History                                      <- the last section before the footer, so new rows never push a field
   Mon 24 Nov  Made         when last week's was done (Fri 21 Nov)          [Open]
   Mon 17 Nov  Made         you chose "Make the next one"                   [Open]
   Mon 10 Nov  Skipped      by you
   Mon  3 Nov  Failed       the task list couldn't be read
                                                     Show all (12)
```

**The Repeat slot.**
- It has two fixed rows in every state, including "Doesn't repeat" and "Needs a due date":
  - a **state line** (a state word plus a short fact);
  - an **action row** at one button height.
- The action row holds at most three fixed-width buttons, which fit the sheet's narrowest width. When a state has no fix, the row is empty but keeps its height.
- History sits at the bottom of the sheet. Turning Repeat on adds it there, below every other field.

**States of the slot.** Each has a label, a state word and one fix. The explanation sits behind the `(i)` tooltip.

| State | State line (one line) | Action row | Tooltip |
|---|---|---|---|
| No due date | "Needs a due date" (muted); the control is disabled | none (Due is right above) | "Set a due date first; each copy is due on the next date." |
| Doesn't repeat | the control reads "Doesn't repeat"; the slot is blank but keeps its height | none | "Make a new copy of this task on a schedule." |
| Active | **Next** Mon 1 Dec, 17:00 | none | "The next one is made when this one is done." |
| Waiting for you | **Waiting** Due again Mon 1 Dec · last week's isn't done | [Make the next one] [Skip this one] [Mark done] | "Shodh makes one copy at a time. Pick what to do with this one." |
| Waiting, folded | **Waiting** Due again 3 times since Mon 17 Nov | same three | same, plus "Only one copy can come of it." |
| Paused (held) | **Paused** Couldn't read the task list 3 times | [Resume] | the last error in words, with its time |
| Retrying | **Retrying** in 5 min · couldn't read the task list | none (automatic) | "Shodh tries 3 times, then pauses the repeat." |
| Older copy | **Next** … plus "Repeat is set on the latest copy" (muted) | [Open latest] | "Changing Repeat here changes the whole series." |

**Rules:**
- **Fixed sizes:**
  - Answer buttons have fixed widths.
  - While an answer is saving, the button keeps its width and shows a spinner in place of its text.
  - A failed answer shows the error in the slot and keeps the buttons.
- **History:**
  - Rows are fixed height, each with a date, a state word (Made / Skipped / Failed / Waiting / Set) and a reason.
  - At most 5 rows show. "Show all (N)" expands only when clicked.
  - [Open] opens the copy the run made. It is absent when the copy was deleted, and the column keeps its width.
- **Every state is visible:**
  - a pending decision (slot, Inbox item, toast);
  - held (slot and Inbox item);
  - skipped and failed with reason (History).
  - Nothing is silent: the slot, the Inbox and History all read from `task_repeat_get` and `task_repeat_runs`, which are refreshed on `calendar-changed` and `inbox-changed`.
- **Inbox item copy:**
  - The decision reads "*Submit timesheet* was due again today, but last week's isn't done." with [Make the next one] [Skip this one] [Mark done].
  - The hold reads "*Submit timesheet* stopped repeating: the task list couldn't be read." with [Resume].
  - Neither has Dismiss.
- **Accessibility:**
  - The Repeat control is a labelled `<select>`-style menu.
  - The slot is an `aria-live="polite"` status.
  - The answer buttons are reachable in tab order.
  - History is a list with dates in `<time>`.

---

## Acceptance tests (§13, PR 1) → tasks

| Acceptance test | Delivered by |
|---|---|
| Table `jobs_tests`/`fill_tests` unchanged and green | Task 1 (byte-identical, by diff), Task 2 |
| Next-time tests across DST gaps and overlaps (London, New York, Kolkata), month ends, leap years | Task 3 |
| A simulated 3-day sleep makes one run with `missed = 2`; `skip` records `skipped` | Task 6 |
| A crash mid-run makes no duplicate copy | Task 6 (resume once after the lease), Task 7 (`a_crash_mid_run_makes_no_duplicate_copy`) |
| The index refuses a second active run | Task 4, Task 5 |
| A locked calendar retries, then holds after 3 runs | Task 6 (rule), Task 7 (`a_locked_calendar_retries_then_holds_after_three_runs`) |
| Completing the latest copy makes exactly one next copy, due at the following occurrence | Task 7 |
| An occurrence with the last copy open makes no copy and one decision | Task 7 |
| Each of the three answers does what it says | Task 7, Task 8 (command), Task 11 (agent) |
| An ignored decision across two more occurrences stays one item and one possible copy | Task 6 (fold), Task 7 |
| A 3-day sleep folds into the same decision and the missed-reminder summary | Task 7 (see D13) |
| Completing from the phone answers the decision | Task 7 (see D14) |
| Series lifecycle (edit latest, delete latest, delete older, workspace detach) | Task 7, Task 10 (delete wording), Task 11 (agent delete preview) |
| A `model` Inbox item is stored (bug regression) | Task 4 |
| Migration keeps every Inbox row | Task 4 |
| Coverage entries | Task 8 |

## Departures from the spec, for decision (not resolved here)

The plan follows the option marked **(plan)**. Each one needs the user's yes or a different choice.

- **D1. §5.1's move scope is smaller than written, and `run_source.rs` cannot move purely.**
  - `RunnerSource` returns `JobRunner`, and `RunEvents` takes `FillJob`/`RunReport`/`table_id`. Moving them would make `engine` depend on `research::tables`.
  - The same applies to the claim, heartbeat and panic SQL, `run_is_gone`/`hold_of`, and `next_rung`/`Streaks`.
  - Options:
    - **(plan)** they stay in `tables` until PR 4/5 needs a generic source;
    - or genericize them in a separate behaviour-preserving commit in this PR (`RunnerSource<R>`, `RunEvents<J, R>`). That touches `table_commands.rs` and is not a pure move.
- **D2. Stronger than the spec.** The spec allows "only import paths changed" in `jobs_tests`/`fill_tests`. The plan keeps them byte-identical through re-exports.
- **D3. `resolve_local` must move into the crate.** It lives in `app/src-tauri/src/reminders.rs`, and the engine (in `shodh-rag`) cannot call the app. §5.1 does not list it, so it gets its own pure-move commit (Task 2).
- **D4. No IANA zone is stored in PR 1. Schedules use the computer's zone.**
  - `TodoItem.due_date` and `reminder` are local wall-clock strings that `reminders.rs` reads in `chrono::Local`. A pinned zone would make a copy's decision time and its reminder disagree after the user changes zone.
  - `chrono-tz` becomes a dev-dependency of `shodh-rag` only, for the DST tests. `iana-time-zone` is not added.
  - **(plan)** the zone field arrives with PR 4, which needs it for automations. The alternative is to store `zone: Option<String>` now (None = the computer's) and add `chrono-tz` as a normal dependency of `shodh-rag`. That is a new dependency edge, with no new lockfile entry.
- **D5. Two schema changes:**
  - (a) `engine_job` has no column for §6's snapshot, so the plan adds `input_json`.
  - (b) §5.2's `ON DELETE CASCADE` would do nothing: `shodh.db` never enables `PRAGMA foreign_keys` (`open_connection`), and `research/tables/store.rs` already deletes children by hand. The plan drops the clause and deletes runs explicitly.
- **D6. `engine_step`, `engine_approval` and `engine_cache` in migration 19.** PR 1 writes none of them, and migrations are append-only, so a shape PR 4 needs to change would then cost a rebuild.
  - **Ruled (2026-11-23):** they are not created in migration 19. They belong to PR 4's migration (minimal first). The Inbox rebuild still adds all four kinds now.
- **D7. No one-minute coalescing for `task_completed`.** §5.3 says "events within one minute make one run", but §6 says "the next copy is made at once". The plan handles `task_completed` at once; coalescing stays for `files_added`/`paper_found` (PR 3).
- **D8. The `skip` overlap would write a row every occurrence while a decision waits.** §5.3's `skip` would add a "previous run was still going" row each time. §6 wants the same item updated. The plan adds the fold as an engine rule for `waiting/decision` runs: `scheduled_for` moves to the newest, `missed` grows, and no row is added. The overlap policy applies to `queued`/`running` only.
- **D9. Target occurrence when a copy is completed late.** §6 says "due at the next occurrence after the completed copy's due time". If that occurrence is already past (the app was closed, so no decision exists yet), the copy would be born overdue for an old date.
  - **Ruled (user, 2026-11-23):** the next copy is due at the next *upcoming* occurrence after the completion time, never a past one. A new copy is never overdue when it appears. The plan applies this to every copy the series makes (completion, Mark done and Make the next one): the target is `schedule.next_after(max(now, planned_through))`, where `planned_through` is the occurrence of the latest copy or the latest handled occurrence. The run that made the copy records the copy's occurrence as its `scheduled_for`, so the next decision comes only when the occurrence after the copy's passes.
- **D10. Hold has no one-click fix among the five commands.**
  - **(plan)** Resume is `task_repeat_set` re-saving the same schedule, which clears the hold and the streak.
  - Alternative: a sixth command `task_repeat_resume`, which also needs a coverage entry.
- **D11. Deleting a task has no confirm.** It is an undo toast (`TasksStore.tsx` `commitDelete`). The "and stop repeating" wording goes in that toast, and the series stops when the delete commits; Undo keeps it.
- **D12. "A phone delete does the same" has nothing to wire.** The phone link has no `delete_task` method today (only `delete_event`). Any future one goes through `calendar_commands::delete_task` and inherits the hook.
- **D13. The missed-reminder summary only exists for time the app was closed.** `reminders.rs` summarises reminders due before launch. During a sleep with the app running, it rings late reminders one by one; there is no sleep summary to join.
  - **(plan)** with the app closed, the decision joins the launch summary through a two-party barrier (one desktop notification). Asleep with the app running, the fold makes one decision and one alert.
  - Alternative: add sleep detection to `reminders.rs` (any loop gap of more than 5 min), so late reminders after a sleep are also summarised. That changes existing reminder behaviour and belongs to PR 5 ("One clock").
- **D14. The phone acceptance test drives the shared funnel, not the phone socket.**
  - `phone_link` dispatch is typed on the Wry `AppHandle`, which the mock runtime cannot build.
  - The phone's `update_task` arm calls `calendar_commands::update_task` → `TauriEffects::calendar_changed` → `task_repeat::on_calendar_change`. The test drives `on_calendar_change` with that change.
  - No end-to-end phone test is added.
- **D15. The spec's sheet mock shows "The next one is made when this one is done." inline.** Per this brief's UX rule (short copy, explanations behind a tooltip), it moves into the `(i)` tooltip, and the slot shows "Next Mon 1 Dec, 17:00".
- **D16. The existing reminder toast does not fit the decision.** It offers Snooze and Mark done on a `taskId`, and Snooze on the old copy would ring that copy's reminder again. The plan adds a decision variant with the three answers and no Snooze. The decision's Inbox item has no Dismiss, so the pending state cannot be hidden while the series silently waits.

## Self-review notes

- **Spec coverage:**
  - §5.1 is Tasks 1 and 2.
  - §5.0's typed schedules are Task 3.
  - §5.2 is Task 4.
  - §5.3 is Tasks 5 and 6: one loop, catch-up, no stampede (one at a time; the network and ledger waits are PR 4), overlap, retries, idempotent effects, leases, and the `task_completed` event.
  - §5.6 holds for PR 1: scheduled runs never navigate.
  - §6 is Tasks 7 to 11.
  - §12 item 1 is Task 8. No new view or target kind is added (items 2–4 do not apply), and item 5 is Task 11's "write tools highlight what they changed" via `reveal_task`.
- **Types shared across tasks** that must not drift:
  - `Schedule`, `MonthDay`, `JobKind::TaskRepeat`, `Handled`, `WaitReason::Decision`, `HoldReason::Failing`;
  - copy id `rep-<job>-<yyyymmddhhmm>`, Inbox ids `repeat:<job>` and `repeat-hold:<job>`;
  - `RepeatView`/`RunView` (camelCase on the wire);
  - `choices: 'make' | 'skip' | 'mark_done'`.
- **Not in PR 1:** the ledger and yielding, approvals, steps, playbooks, the Attention/Info tabs (PR 2), and the code-map system job (PR 5).
