# The proactive assistant on one scheduler engine (design)

Date: 2026-11-22. Status: draft, waiting for the user's review.
Tracks: GitHub issue #149 ("One engine (croner + jobs in shodh.db: catch-up after sleep, retries, overlap
policy, approval steps, run history) powering Automations, a What's new feed and Linear-like Projects + Tasks").

## 1. Intent

PM tools are passive: everything is typed in and nothing comes back to you. Shodh should notice what is due,
what is new and what is stuck, and propose the fix. The user then confirms it with one click, and every change
says where it came from.

All of this needs one thing underneath it: work that runs later, on a schedule or when something happens,
survives sleep and restarts, asks before it acts, and leaves a record the user can read. Today Shodh has three
separate timers:

- task reminders (`reminders.rs`);
- the evidence-table scheduler (`table_commands.rs::spawn_scheduler`, every minute);
- memory consolidation (`memory_learn.rs`, when idle).

A fourth and a fifth would make it worse. The decision of 2026-10-31 stands: **one engine** serves both the
work people do (Projects and Tasks) and the work Shodh does (Automations). The engine is never named in the UI.
Nothing on screen says cron, DAG, YAML, job or trigger.

## 2. Goals, non-goals, anti-goals

**Goals**
1. **The engine.** It handles:
   - schedules in local time;
   - catch-up after sleep without a stampede;
   - an overlap policy;
   - retries on a ladder;
   - approval steps that survive restarts;
   - run history in plain words.
   It is stored in `shodh.db` and reuses the evidence-table runner's tested parts (§5.1).
2. **Automations.** Playbooks are saved, reusable instructions for recurring jobs ("Weekly literature check",
   "Screen a new paper against my criteria"). An installed skill can be saved as a playbook (skills become
   saved recipes).
3. **What's new.** It shows new files in the user's folders and, only if the user opts in, new papers from
   arXiv, OpenAlex and Semantic Scholar.
4. **Projects and Tasks, Linear-like.**
   - Keyboard-first, with list and board views, plus My work and At risk.
   - Relationships between tasks: blocks / blocked by / relates to / duplicate of.
   - Links to people, documents, decisions and workspaces.
   - Provenance on every task and every change.
5. **The early assistant.**
   - Chat proposes tasks, decisions and facts, and the user saves each with one click, linked to its source.
   - A daily review suggests fixes and logs its reasoning.
   - Nudges stay inside the app.
6. **The Signal/Noise Inbox.** Every nudge, mention, assignment and agent reply is in one place, split into
   Signal (needs you) and Noise (FYI).

**Non-goals (decided by the user: later)**
- The agent following up with other people, nudging others, or reassigning work on its own.
- Phone notifications for any of this. The phone companion keeps its own approval and answer notices.
- Team sync, shared projects, iroh.
- "Show and explain" (whiteboard, callouts and walkthroughs).
- Agents co-editing documents in place.

**Anti-goals (a failure even if it works)**
- A schedule editor, cron strings or a step graph anywhere in the UI.
- Any model call the user did not see an estimate for, or that goes past a daily cap.
- Untrusted text (a paper, a web page, a Slack message, a file in a watched folder) causing a write, a command
  or an outward action without the user's approval.
- A background run moving the user's screen.
- A second job runner next to the evidence-table runner.
- A task change with no record of who or what made it, and why.

## 3. Constraints

- **Local-first.** Nothing leaves the machine unless the user turned that specific thing on. Paper alerts send
  only the alert's search text, and only to the three open APIs (`harness/web/papers.rs`). No paid services.
- **Free quota.**
  - Every model-spending run is estimated before it starts, counted before each call, cached and capped.
  - Caps already oversubscribe a 50-a-day free key: tables 30, Slack 20, Telegram 20, and chat is not counted
    at all. So the estimate always shows Shodh's total calls today, not one feature's (§5.4).
- **Rust only.** No new scripts. No new crate in the recommended design (§4).
- **Agent-first.**
  - Every UI command has an agent tool or an exclusion with its reason in `app/src-tauri/agent-coverage.json`,
    checked by `agent_coverage.rs`.
  - Every view and target kind the assistant can open has a tool, a parser and a handler.
- **Approvals belong to the user.**
  - The agent never approves its own steps, never confirms its own proposals, and never changes what runs
    without asking.
  - This follows the existing exclusions: `inbox_dismiss`, `memory_suggestions_list` and
    `set_default_agent_mode` are user-only.
- **Ideas only from AGPL projects** (macro-inc/macro: the Signal/Noise split and the source → task → change →
  result chain). No code.

## 4. What exists today

| Area | Where | What it gives this design |
|---|---|---|
| Tasks and events | `app/src-tauri/src/calendar_store.rs` (`calendar_data.json`, one lock, atomic write), `calendar_commands.rs`, `app/src/features/tasks/*` (TasksStore, TaskBoard per workspace, TasksCalendar, ReminderAlerts, `dueDate.ts`, `board.ts`) | Task fields: `TodoItem`, with `workspace_id` as "its project", plus `source`/`source_ref` and `reminder`. `apply_task_patch` returns `FieldChange{field,before,after}`. Not relational and no change history. |
| Users of tasks | `reminders.rs`, `agent_tools/calendar.rs` (10 tools, approval previews from `apply_task_patch`), `phone_link.rs:1149` (phone calls `load_tasks`/`create_task`/…), `calendar_indexer::reconcile_task` (`calendar://task/{id}` in the index), `store_notices`, `detach_workspace_tasks` | Every one of these must keep working when storage moves (PR 6). |
| Reminders | `reminders.rs`: sleeps until the next reminder, at most `MAX_SLEEP` = 60 s, wakes on calendar change, marks fired before notifying, turns missed-while-closed into one summary | The wake-detection and DST pattern (`resolve_local`) the engine copies. |
| `shodh.db` | `crates/shodh-rag/src/audit/store.rs` `MIGRATIONS` (append-only, one sequence, applied under the write turn; 18 is the latest on master), `open_shared_connection` | Every new table here is one migration. SQLite can't change a CHECK in place, so CHECK changes rebuild the table (as migrations 16 and 18 did). |
| Evidence-table runner | `research/tables/jobs.rs` (lease with `run_token` and heartbeat, `LEASE_MINUTES` = 10, `BACKOFF_LADDER_MINUTES` [1,2,5,15,30,30,30], pause reasons, `MAX_PANICS`, failure streak), `schedule.rs` (`RunnerSource`/`RunEvents`, tick, `model_unavailable` pause), `calls.rs` (`CallCounter::try_record` reserves before the call, UTC day, `DailyCap`) | The parts §5.1 lifts out. |
| Other call counters | `connectors/slack_usage.rs` (JSON file), Telegram the same, `user_memory::learn::UsageToday` | They become one ledger (§5.4). |
| Inbox | `crates/shodh-rag/src/inbox.rs` (statuses working / needs_you / done / failed; `InboxStore::recover` drops approvals on launch), `inbox_commands.rs::post`, `features/inbox/InboxButton.tsx` and `model.ts` | Signal/Noise becomes a view over this store. **Bug found:** `InboxKind::Model` (`model_fallback.rs:586`) is not in migration 18's CHECK, so "model keeps failing" items are rejected and only logged. PR 2's rebuild fixes it. |
| Notifications | `notifications.rs` (categories; only while the window is unfocused; rate-paced; no content in text) | Automations post through `notify`. The daily review raises no desktop notification. |
| Memory proposals | `user_memory/learn/inbox.rs` (`ProposalStatus`, `ProposalView` with `conversation_id`/`turn_id`), `MemoryContent::Fact{class:"Decision"…}`, `Person` entities, `Actor` | Decisions and facts are memories. People are memory entities. One-click confirm is the proposal pattern. |
| Folder watch | `crates/shodh-rag/src/folder_sync.rs` (`SyncReport`: counts only), `file_watcher.rs` (`FOLDER_SYNC_EVENT`) | What's new needs the names of added files (§7). |
| Papers | `harness/web/papers.rs` (arXiv, OpenAlex, S2; merging and local ranking) | Paper alerts call this. No model calls. |
| Agent safety | `harness/modes.rs` (`decide(mode, ToolClass)`, plus a floor that always asks), `harness/tools/mod.rs` `UNTRUSTED_NOTICE` / `WEB_UNTRUSTED_NOTICE`, `RiskTier` Read/Write/Destructive | The rule for unattended runs (§5.5). |
| Headless agent | `agent_transcript.rs` (the backend folds `agent_event` exactly as the UI does, "with or without a window open") | Playbook runs save a real transcript. |
| Navigation | `harness/tools/navigate.rs` `VIEWS` = `lib/viewTabs.ts` `VIEW_TABS` (8 views, no Automations yet), `NavigationTarget::KINDS`, `agent_coverage.rs::kind_owner`, `features/agent/navigation.ts`, `ChatSessionContext.followNavigation` (the gate) | The checklist in §12. |
| Code map | `code_map_commands.rs::schedule`; the user asked for a nightly full reconcile "through the new scheduler" | A system job in PR 5. |

## 5. The engine

### 5.0 Approaches considered

| | A. Turn the table runner into the engine | **B. A shared engine core that both use (recommended)** | C. An in-memory cron crate (tokio-cron-scheduler) plus thin persistence |
|---|---|---|---|
| Idea | Make `table_fill_job` and `jobs.rs` generic and migrate table fills into engine jobs | Move the runner's generic parts into `shodh_rag::engine`. `research/tables` imports them unchanged. The engine adds schedules, triggers, runs, steps and approvals. Table fills stay `table_fill_job`s and are started by engine steps. | Schedule in memory and write run rows ourselves |
| Risk | Rewrites 120 KB of shipped, fix-rounded code and tests (`jobs.rs`, `jobs_tests.rs`). Table semantics (queued cells, per-paper calls) leak into every job. | Small: a pure move with the table tests as its proof. New code only where nothing exists. | No durable catch-up, overlap or approval state. We'd write the persistence anyway, plus another runtime. |
| One runner? | Yes, at high cost | Yes: one lease, one backoff ladder, one call ledger, one clock loop. Fill execution stays table-specific because it is a step, not a runner. | No |

**Recommendation: B.** It is the only option that reuses the runner rather than copying it, and it is the only
one whose first PR is small.

**Schedules: typed, not cron.** The UI offers only these schedules:
- once;
- every day;
- weekdays;
- weekly on chosen days;
- monthly on day N or on the last day;
- every N hours.

So the engine stores a typed `Schedule` enum as JSON and computes the next time with `chrono`, `chrono-tz` and
`iana-time-zone` (all already in `Cargo.lock`), plus `reminders::resolve_local` for daylight-saving gaps.

Issue #149 names `croner` (MIT). It is not added, for three reasons:
- the user never sees an expression;
- a typed enum is checked at compile time and reads plainly in run history;
- it is about 200 lines with exhaustive tests.

If schedules ever outgrow the enum (say, "the second Tuesday"), croner is the fallback, with a NOTICE entry.

### 5.1 What moves into `crates/shodh-rag/src/engine/` (PR 1, a pure move)

- `lease.rs`: the `run_token` claim, the heartbeat, `LEASE_MINUTES`, the in-process live-run slot and
  `PROCESS_TOKEN` (from `jobs.rs`).
- `backoff.rs`: `BACKOFF_LADDER_MINUTES`, `STALE_RESET_WAIT_SECS`, the failure-streak and panic-count rules.
- `clock.rs`: `Clock`/`SystemClock`, `utc_day`, `next_utc_midnight`, `day_key` (from `calls.rs`).
- `run_source.rs`: the `RunnerSource`/`RunEvents` split and `RunnerError::Blocked` (from `schedule.rs`).

`research/tables/*` re-imports these. The acceptance test for the move: `jobs_tests.rs` and `fill_tests.rs`
pass with no edits beyond import paths.

### 5.2 Data model (one migration per PR; the next free version, 19 on master today)

```
engine_job     id TEXT PK, kind TEXT CHECK (kind IN ('task_repeat','folder_watch','paper_alert',
                 'playbook','daily_review','system')),
               subject TEXT NOT NULL,          -- task id, playbook id, source id, 'code_map:<project>'
               title TEXT NOT NULL,
               schedule_json TEXT NULL,        -- typed Schedule + IANA zone, or NULL for event-only
               trigger_json TEXT NULL,         -- event triggers (§5.3)
               enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
               overlap TEXT NOT NULL CHECK (overlap IN ('skip','queue_one')),
               catch_up TEXT NOT NULL CHECK (catch_up IN ('once','skip')),
               max_attempts INTEGER NOT NULL CHECK (max_attempts BETWEEN 1 AND 5),
               next_due_at TEXT NULL,          -- UTC; a cache recomputed on every plan
               hold_reason TEXT NULL CHECK (hold_reason IN ('user','failing','daily_cap',
                 'model_unavailable','offline')),
               failure_streak INTEGER NOT NULL DEFAULT 0,
               created_by TEXT NOT NULL CHECK (created_by IN ('user','agent','system')),
               provenance_json TEXT NOT NULL,  -- conversation/turn/run that made it
               created_at TEXT NOT NULL, updated_at TEXT NOT NULL
engine_run     id TEXT PK, job_id TEXT NOT NULL REFERENCES engine_job(id) ON DELETE CASCADE,
               trigger TEXT NOT NULL CHECK (trigger IN ('schedule','catch_up','manual','event','retry')),
               scheduled_for TEXT NULL, missed INTEGER NOT NULL DEFAULT 0,
               state TEXT NOT NULL CHECK (state IN ('queued','running','waiting','succeeded',
                 'failed','skipped','cancelled')),
               wait_reason TEXT NULL CHECK (wait_reason IN ('approval','daily_cap','quota',
                 'model_unavailable','offline','backoff')),
               attempt INTEGER NOT NULL DEFAULT 1, next_attempt_at TEXT NULL,
               run_token TEXT NULL, heartbeat_at TEXT NULL,
               planned_calls INTEGER NOT NULL DEFAULT 0, used_calls INTEGER NOT NULL DEFAULT 0,
               conversation_id TEXT NULL,      -- the transcript of an agent step
               summary TEXT NULL, error_code TEXT NULL, error_detail TEXT NULL,
               started_at TEXT NULL, ended_at TEXT NULL, created_at TEXT NOT NULL
               UNIQUE INDEX engine_run_active ON engine_run(job_id)
                 WHERE state IN ('queued','running','waiting')
engine_step    run_id, idx, kind, state, input_json, output_json, started_at, ended_at,
               PRIMARY KEY (run_id, idx)
engine_approval id TEXT PK, run_id, step_idx, title, preview_json, created_at, expires_at,
               decision TEXT NULL CHECK (decision IN ('approved','denied','expired')),
               decided_at TEXT NULL
engine_cache   job_id, input_key, output_json, created_at, PRIMARY KEY (job_id, input_key)
```

Retention: a job keeps all of its runs from the last 90 days, and never fewer than its latest 20. Deleting a job
deletes its runs, steps, approvals and cache rows.

### 5.3 Semantics

- **One loop.**
  - It sleeps until the earliest `next_due_at`, for at most 60 s (the reminders rule: sleep runs on a monotonic
    clock, so suspend and clock changes are seen within a minute).
  - It wakes early on any job change.
  - Local-time schedules are turned into instants on every plan, never kept, so time-zone and DST changes apply
    at once.
- **Due and catch-up.**
  - A run is due when `next_due_at <= now`. If now is more than 5 minutes past the scheduled time, the computer
    was asleep or Shodh was closed.
  - `catch_up = once`: one run for the latest missed time, with `missed = n-1` recorded ("Ran once when your
    computer woke; 2 earlier times were skipped").
  - `catch_up = skip`: a `skipped` run is recorded and the next time is planned.
- **No stampede.**
  - Due runs queue in order of `scheduled_for`. Local runs go one at a time.
  - Runs that use the network or a model go one at a time, only while the ledger allows (§5.4), and not in the
    first 2 minutes after a wake, so the network has time to come back.
  - A failed connectivity check waits as `offline` and is retried on the ladder.
- **Overlap.** The unique index allows one active run per job. At a due time with a run still active:
  - `skip` (the default) records a `skipped` run, "the previous run was still going";
  - `queue_one` leaves at most one queued run behind it.
  - Two runs of one job never run at the same time.
- **Retries.**
  - Transient failures (network, a 429 without a reset time, `model_unavailable`) wait on the ladder up to
    `max_attempts` (default 3).
  - A daily-quota error waits until the provider's next UTC midnight.
  - Permanent failures (a bad playbook, refused auth) fail at once.
  - Three failed runs in a row put the job on hold (`hold_reason = failing`) with a Signal item.
- **Idempotent effects.**
  - A step that creates something derives its id from `(job_id, scheduled_for)` and inserts only if it is
    missing, so a crash and a resume never make two.
  - Steps are resumable at their `idx`.
- **Leases.** As in `jobs.rs`: the heartbeat before every step and every model call. A stale lease can be taken
  over after 10 minutes.
- **Event triggers** (`trigger_json`): `files_added {source_id, pattern}`, `paper_found {alert_id}`,
  `task_unblocked {task_id}`. An event makes a run with `trigger = 'event'`. Several events within a minute make
  one run, with the subjects collected in its input.

### 5.4 The call ledger (PR 4)

Migration: `model_calls(utc_day TEXT, spender TEXT CHECK (spender IN ('tables','automations','review','slack',
'telegram','learning')), calls INTEGER CHECK (calls >= 0), PRIMARY KEY (utc_day, spender))`.

- `CallCounter` takes a spender.
- The existing `table_calls` rows are copied in, and the table is dropped in the same migration.
- Slack, Telegram and learning move their counters in as each is next touched. Until then, their counts are read
  from their current stores for the total.
- Each spender keeps its own cap.
  - The automations cap defaults to 10 calls a day, with Settings range 3–1,000.
  - The review spends at most 1 call a day, and only when the user turns it on.
- Every estimate dialog shows: "This run needs up to 4 model calls. Shodh has made 23 calls today (tables 12,
  Slack 6, automations 5). Resets at 5:30 am."

### 5.5 Approval steps and untrusted content

- An `approval` step writes an `engine_approval` row and posts a Signal item (`id = engine:approval:<id>`).
- On launch, `InboxStore::recover` still drops session approvals. The engine then re-posts an item for every open
  `engine_approval`, because these belong to a durable run, not a live session.
- An approval expires after 7 days. The run is then cancelled, and the item says so.
- **The unattended-run rule** is an engine rule, not prompt text. A playbook's agent step runs in a dedicated
  profile, `automation`.
  - Reads run.
  - Execution and outward tools are hidden: `run_command`, Code mode, GitHub writes, connector posts, downloads,
    and MCP tools not set to read.
  - Every Write or Destructive call ends the step and becomes an approval step. Its preview is the tool's own
    `ApprovalPreview`, for exactly what would be saved.
  - This holds whatever the user's default mode is.
  - Because the profile reads untrusted content (`UNTRUSTED_NOTICE` sources) by design, no write in a run is
    ever pre-approved.
  - Approving one step runs that one call, then continues the run in a new session. Its context is the
    transcript and the approved result.
- The agent cannot decide approvals: `engine_approval_decide` is excluded in coverage.

### 5.6 Visibility of the agent

- **Runs the user starts.** "Run now" opens the run's conversation in Ask, and its navigation goes through
  `followNavigation`'s gate like any answer.
- **Scheduled runs never move the screen.** Their visibility is:
  - the run page (steps in words, calls used, the transcript);
  - the Inbox;
  - **Show me what it did**, which replays the run's navigation targets one at a time through the same gate.

## 6. PR 1's visible use: repeating tasks

This was chosen against five criteria: it needs no model calls, it adds no empty tab, it exercises catch-up,
overlap, retries and history, and it uses an existing surface.

The other candidate, the nightly code-map reconcile, only reaches Code-mode users, and missing a night costs
nothing. It becomes a system job in PR 5.

- **Data.**
  - A `task_repeat` job, with the task's id as its `subject`.
  - Its input is a snapshot of the task: title, description, tags, priority, workspace, subtasks (unchecked),
    due time of day, and the reminder as an offset from the due time.
  - Each run creates the next copy, id `rep-<job>-<yyyymmddhhmm>`, through `CalendarStore::update` and
    `calendar_changed`. The calendar file is unchanged and needs no migration.
- **"If the last one isn't done":** *make the next one anyway* (the default) or *wait until it's done*. The
  second is a precondition step that records a `skipped` run.
- **Retries.** An unreadable or locked calendar file waits on the ladder (reuse `reminders::retry_delay`'s
  handling of `CalendarError`).
- **Commands:** `task_repeat_get`, `task_repeat_set`, `task_repeat_clear`, `task_repeat_runs`.
- **Agent tools.** `create_task` and `update_task` take `repeat`, with the same approval preview: "Repeats every
  Monday at 09:00". `list_tasks` shows the repeat and its next time. Coverage maps all four commands to these
  three tools.
- **UX**, in the task detail sheet:

```
 Submit timesheet                                   ⋯
 Due   Mon 24 Nov, 17:00
 Repeat  [ Every week on Mon ▾ ]   If the last one isn't done: [ Make the next one anyway ▾ ]
         Next: Mon 1 Dec, 17:00
 History
   Mon 24 Nov  Made this one (your computer was off at 17:00; made at 08:12 Tue)
   Mon 17 Nov  Made                          [Open]
```

- **Plain-word errors:** "Couldn't make this week's copy: the task list couldn't be read. Trying again in
  5 minutes." There is no empty state; the field reads "Doesn't repeat".

## 7. What's new (PR 3)

- **New files.**
  - `SyncReport` gains `added: Vec<String>`: files indexed for the first time in this sync, not changed or
    re-chunked. It is capped at 200, with `added_more`.
  - A folder's first sync (no manifest before it) sets a baseline and reports nothing.
  - Each Library folder has "Show new files in What's new", on by default. It is a `folder_watch` job with an
    event trigger, not a schedule.
- **Paper alerts (opt-in).**
  - A saved search, weekly by default.
  - One `papers.rs` request per API per run, de-duplicated by DOI, arXiv id or title against the library and
    against earlier items.
  - Ranked by the local cross-encoder. No model calls.
  - Creating one shows: "Once a week, Shodh will send the words *linear attention KV cache* to arXiv, OpenAlex
    and Semantic Scholar. Nothing else leaves your computer." [Turn on] [Cancel]. Local-only mode refuses it.
- **Data:** `feed_item(id, kind CHECK (kind IN ('file','paper')), source_ref, title, detail, found_at, seen_at,
  dismissed_at, link_json, data_json)`. The unique key is `(kind, source_ref)`. Items are kept 90 days.
- **Inbox:** one Noise item per run ("4 new papers for 'linear attention'"), never one per paper.
- **Commands and tools:**
  - `whats_new_list` → `whats_new` (Read; opens Library → What's new and highlights an item).
  - `paper_alert_create`/`update` → `create_paper_alert`/`update_paper_alert` (Write, `must_confirm`: they send
    text off the computer).
  - `paper_alert_delete` → `delete_paper_alert`.
  - `folder_watch_set` → `set_folder_watch`.
  - `whats_new_dismiss` and `whats_new_mark_seen` are excluded: what the user has seen is theirs, as with
    `inbox_dismiss`.
- **UX:** Library → **What's new**.

```
 What's new                                   [ + Paper alert ]
 Today
   📄 kv-cache-survey.pdf   Research Papers folder · 10:41      [Open] [Add to table ▾] [×]
   📑 "Gated Linear Attention…" (2026) arXiv · alert: linear attention   [Open abstract] [Add to library]
 Earlier this week …
```

  - **Empty state:** "Nothing new yet. New files in your Library folders show up here. You can also turn on
    alerts for new papers on a topic."
  - **Error:** "arXiv didn't answer this week's check. Shodh will try again in an hour; OpenAlex and Semantic
    Scholar were checked."
  - "Add to library" uses the existing download path (`download_file`), only for open-access PDFs the API
    reported.

## 8. Automations and playbooks (PR 4)

- **A playbook** has:
  - a name;
  - an instruction (text);
  - a scope (a workspace, or the whole library);
  - when it runs: a typed schedule, an event ("when a new file arrives in…", "when a paper alert finds
    something"), or only by hand;
  - a step list.

  Built-in templates set the steps. A user-written playbook has one `agent` step.
- **Step kinds** (v1): `find_papers`, `table_add_rows_and_fill`, `create_task`, `agent`, `approval`, `notify`.
  - `table_add_rows_and_fill` adds rows and starts the table's existing `table_fill_job` (the table runner does
    the work and keeps its own cap).
  - The step waits on that job's end through `RunEvents`.
- **Templates.**
  - *Weekly literature check*: `find_papers` (the alert's search), then one `agent` call that summarises what is
    new, citing each paper. Estimate: 1 call a week.
  - *Screen a new paper against my criteria*: on `files_added`, add the paper as a row to a chosen evidence table
    and fill it. Its yes/no columns are the criteria, and the estimate is 1–2 calls per paper. Include/exclude
    decisions arrive with evidence-table v2 screening; until then the row's cells are the result.
- **Skills as recipes.** "Save as playbook" on an installed skill copies its instructions into a new playbook's
  `agent` step. Installing skills stays user-only.
- **Cache.** `engine_cache` keys on (playbook revision, input hash), so a run over unchanged inputs makes no call.
- **Data:** `playbook(id, name, instruction, scope_json, steps_json, revision, created_by, provenance_json,
  created_at, updated_at)` plus its `engine_job`.
- **Commands and tools.**

| Command | Agent tool | Tier / why |
|---|---|---|
| `automations_list`, `automation_get`, `automation_runs`, `automation_run_get` | `list_automations`, `show_automation` | Read; opens Automations and highlights |
| `automation_create`, `automation_update` | `create_automation`, `update_automation` | Write, `must_confirm` (schedules future work and spends quota); the preview shows the schedule and the estimate |
| `automation_delete` | `delete_automation` | Destructive |
| `automation_pause`, `automation_resume` | `pause_automation`, `resume_automation` | Write (local edit) |
| `automation_run_now` | `run_automation` | Write, `must_confirm`, with the estimate |
| `engine_approval_decide` | excluded | "The assistant never approves its own steps." |
| `automations_settings_set` (cap) | excluded | "The daily call cap is the user's spending decision", as `tables_settings_set` is |

- **UX.**
  - A new view, **Automations**, between Tasks and Activity (the order the user approved). It ships in this PR,
    with templates, so it is never empty.

```
 Automations                                              [ + New automation ]
 Weekly literature check      Mondays 09:00 · next Mon 1 Dec   Last: found 4 papers ✓   ⏸ ⋯
 Screen new papers            When a file arrives in Research Papers   Last: 2 papers filled ✓
 ! Monthly progress summary   On hold: failed 3 times in a row (model unavailable)  [Check model] [Resume]

 New automation
   What should Shodh do?   [ Every Monday, look for new papers on my topics and summarise them … ]
   When?                   ( ) Every [week ▾] on [Mon] at [09:00]   ( ) When a new file arrives in [folder ▾]
                           ( ) Only when I start it
   Where?                  [ Workspace: Thesis ▾ ]
   Each run: up to 4 model calls (about 16 a month). Anything that changes your data waits for your OK.
                                                                     [Cancel] [Save]
```

  - **Run page:** time, trigger in words ("Ran when your computer woke; Monday's run was missed"), steps with
    their outcome, calls used out of estimated, [Open conversation], [Show me what it did].
  - **Empty state:** the three templates as cards, each with "Set up". The cards are content, not a placeholder.
  - **Errors in words:** "Paused: today's 10 automation calls are used. It goes on after 5:30 am." / "Waiting for
    you: it wants to add 3 tasks. [Review]".

## 9. Projects and Tasks (PR 6 storage and history, PR 7 the Linear-like views)

### 9.1 Storage moves to `shodh.db` (PR 6)

```
task         id PK, title, description, status, priority, due, reminder, reminder_fired_at,
             snoozed_until, owner_kind CHECK (owner_kind IN ('me','person','shodh')), owner_ref NULL,
             workspace_id NULL, tags_json, subtasks_json, source, source_ref,
             provenance_json NOT NULL, created_at, updated_at, completed_at
task_link    task_id, kind CHECK (kind IN ('blocks','relates','duplicate_of','person','document',
             'decision','workspace','conversation')), target TEXT, created_at, created_by,
             PRIMARY KEY (task_id, kind, target)
task_change  id PK, task_id, at, actor CHECK (actor IN ('user','agent','phone','automation','import')),
             actor_ref NULL,   -- conversation+turn, run id, phone device name
             field, before_json, after_json, reason NULL, source_json NULL
```

- **The import.**
  - On first open after the update, every task in `calendar_data.json` is imported in one transaction, with one
    `task_change` of actor `import` each.
  - The file keeps its events. The tasks are removed from it only after the transaction commits.
  - The original is kept as `calendar_data.pre-tasks.json`.
  - An import that fails leaves the file untouched, shows a store notice, and is retried on the next launch.
- **Wire shapes stay the same:** `TodoItem` JSON, `load_tasks`/`create_task`/…. The phone, the agent tools
  (previews from the same `apply_task_patch` logic), `reminders.rs`, `calendar_indexer` and
  `detach_workspace_tasks` switch to a `TaskStore` with the same functions.
- **History.**
  - Every write records its `FieldChange`s as `task_change` rows with the actor and the reason. The reason is
    the agent's stated reason, or the review rule.
  - The audit log gets a `TaskChange` event with counts only, as `MemoryWrite` does.
- **The provenance chain** is source → task → change → result:
  - the source: `provenance_json` = {conversation, turn, cited passages, a Slack thread, a feed item};
  - each change;
  - the result: the completing change links the run, document or decision it produced.
- **UX:** a **History** section in the task detail sheet: "Shodh, in *Thesis plan* chat, 21 Nov: moved due to
  28 Nov. Why: 'blocked by Data cleaning, due 27 Nov'. [Open chat]".

### 9.2 Linear-like views (PR 7)

- **Projects are workspaces** (see Q1). A workspace already owns its tasks (`workspace_id`).
- **Views:** List (grouped by status), Board (`board.ts` across projects), **My work** (owner = me, not done,
  sorted by due), **At risk**, Calendar (kept).
- **At risk is deterministic.** A task is at risk if any of these holds:
  - it is overdue;
  - it is due within 2 days and still To do;
  - a blocker is not done and is due after this task;
  - a blocker is overdue.

  The reason is shown per task.
- **Relationships.**
  - Blocks / blocked by form a DAG, and a link that would close a cycle is refused: "That would make *A* wait
    for itself (A → B → A)".
  - `task_unblocked` fires when the last blocker is done.
  - People are memory `Person` entities, decisions are `Fact{class:"Decision"}` memories, and documents are
    indexed sources.
- **Owner.**
  - Me, a person (a label only: no one is notified, see Non-goals), or **Shodh**.
  - Giving a task to Shodh asks "What should Shodh do?" and links or creates a playbook. The task completes when
    that run succeeds.
- **Keys:**
  - `C` new; `J`/`K` move; `X` select; `S` status; `P` priority; `D` due; `A` owner; `L` link; `Enter` open;
    `G then M` My work; `G then R` At risk; `/` filter; `Ctrl+K` palette.
  - Every key has a visible menu equivalent, and the list is an ARIA grid like the evidence table.

```
 Tasks   [List] [Board] [Calendar]   My work · At risk (3) · All      / filter
 Thesis ─────────────────────────────────────────────────────────────
  ● Data cleaning        Me     Due Thu 27   ⚠ overdue blocker of 2 tasks
  ○ Run baselines        Me     Due Fri 28   ⛓ blocked by Data cleaning
  ○ Draft related work   Shodh  Weekly lit check → last run Mon ✓
```

- **Empty states:**
  - My work: "Nothing assigned to you. Press C to add a task, or ask Shodh to plan something."
  - At risk: "Nothing at risk. Shodh checks due dates and blockers here."
- **Tools.**
  - `link_tasks` and `unlink_tasks` (Write, local edit).
  - `list_tasks` gains `view: my_work|at_risk` and `project`.
  - `show_tasks` (Read) opens a view or project and highlights a task.
  - The commands `task_link_add`, `task_link_remove`, `task_history` map to `link_tasks`, `unlink_tasks` and
    `task_history` (Read).

## 10. The early assistant (PR 8)

- **Proposals in chat.** A Read-tier tool `propose_items` takes `[{kind: task|decision|fact, text, fields,
  source: {conversation_id, turn_id, citations}}]`. It writes nothing; the answer shows a card per item:

```
 Shodh suggests
  ☐ Task  "Re-run baselines with the fixed split"  Thesis · due Fri   from [3]   [Save] [Edit] [×]
  ☐ Decision  "Use SIFT1M as the main benchmark"   from your message          [Save] [×]
```

  - **Save** creates the task (provenance = the turn and the cited passages) or the memory (`Fact`, class
    `Decision`, or the fact's class, `Actor` = the user, with the source turn).
  - Unsaved cards stay with the message.
  - Coverage: `proposal_save` is excluded, because it is the user's confirmation. The agent can still create
    directly with `create_task`, under approval.
  - This costs no extra calls, since it is part of the answer.
- **The daily review** is a `daily_review` job, every day at 09:00, `catch_up = once`.
  - **Deterministic checks first** (no calls): at-risk tasks, a blocker that is done but still linked, a due
    date before a blocker's, likely duplicates (local embedding similarity of titles ≥ 0.9 within a project),
    repeat copies piling up undone.
  - Each finding is a suggestion with a structured fix (field → new value) and its reasoning in words.
  - **An optional model pass** (off by default, 1 call a day, shown in the estimate) ranks and words the
    findings and may add suggestions. Its reasoning is stored in the run.
  - **Apply** writes the fix as a `task_change` with actor `automation`, the run id and the reason.
  - **Dismiss** keys on (rule, task, value), so the same suggestion isn't repeated for 14 days.
  - **Nudges in the app only.** One Signal item ("Today's review: 4 suggestions") and a Review strip at the top of
    Tasks. No desktop notification: `notify` is not called for `review`.
  - **Tools.** `task_review` (Read, opens the strip). `review_apply` maps to `update_task`.
    `review_dismiss` is excluded (the user's judgement).

## 11. The Signal/Noise Inbox (PR 2)

- **A view over the existing store, not a new one.**
  - **Signal:** every `needs_you` item (approvals, memory suggestions, rung reminders, engine approvals, jobs on
    hold, review suggestions, an assignment to me made by a run or a connector), plus `failed` items that stop
    something the user owns.
  - **Noise:** everything else (finished work, What's new digests, automation successes, Slack replies Shodh
    already posted).
  - These rules are fixed and listed in a "Why is this here?" popover. The badge counts Signal only.
- **The migration** rebuilds `inbox_items` with CHECK kinds plus `'model'` (fixing the bug in §4), `'automation'`,
  `'whats_new'` and `'review'`, so later PRs need no rebuild. Rust variants are added in the PRs that post them.
- **UX:** the Inbox panel gets two tabs, **Needs you (3)** and **FYI (12)**, with "Signal" and "Noise" as the
  section headings' subtitle. Keys and actions stay as today.
  - **Empty Signal:** "Nothing needs you. Shodh puts approvals, reminders and suggestions here."
  - **Empty Noise:** "Nothing new. Finished work and updates show here."
- **Commands:** none new. `inbox_list` and `inbox_dismiss` keep their exclusions.

## 12. Agent coverage checklist (every PR that adds UI)

1. Every new command has an `agent-coverage.json` entry: its tool(s), or `excluded` with its reason.
   - The rule for this design: **the agent may read, list and propose. It never approves its own steps, never
     confirms its own proposals, and never changes caps or what runs without asking.**
2. A new view (Automations, PR 4) updates `VIEWS` (`navigate.rs`) and `VIEW_TABS` (`viewTabs.ts`) together, its
   labels and keywords, and the coverage `navigation.views` entry (`open_view`, `show_automation`).
3. A new target kind (`automation` in PR 4, `tasks` in PR 7) needs all of these:
   - `NavigationTarget::KINDS` with a round-trip test;
   - a `kind_owner` arm (`features/automations/AutomationsView.tsx`, `features/tasks/TasksView.tsx`);
   - `case '<kind>':` in `navigation.ts`;
   - `useNavigationTarget('<kind>'` in the owner;
   - a test per tool with `testing::expect_navigation`.
4. A new Library section (`whats_new`, PR 3) needs `LIBRARY_SECTIONS` on both sides and `target.section ===
   'whats_new'` in `LibraryView.tsx`.
5. Write tools highlight what they changed after it is saved. They never open a dialog.

## 13. Phasing into PRs

Each PR ships alone, passes CI and is useful by itself. Signal/Noise comes second, because What's new,
Automations and the review all post into it.

| PR | Scope | Acceptance tests |
|---|---|---|
| **1. Engine + repeating tasks** | `engine/` move (§5.1); `Schedule`; jobs, runs and steps migration; the loop; `task_repeat`; detail-sheet Repeat and History | Table `jobs_tests`/`fill_tests` unchanged and green. Schedule next-time tests across DST gap and overlap (Europe/London, America/New_York, Asia/Kolkata), last-day-of-month and leap year. A simulated 3-day sleep makes one run with `missed = 2`; `skip` records `skipped`. A crash between create and run end makes no duplicate (deterministic id). The overlap index refuses a second active run. A locked calendar retries on the ladder, then holds after 3 runs. Agent `update_task {repeat}` preview text. Coverage entries. |
| **2. Signal/Noise Inbox** | inbox CHECK rebuild (+`model`); split view; badge | Migration keeps every row. A `model` item is now stored (regression test for the bug). Classification table test per kind and status. Frontend model tests for tab counts and empty states. |
| **3. What's new** | `SyncReport.added`; `feed_item`; folder watch and paper alerts; Library section; Noise digests | The first sync is a baseline (0 items). Added files show and changed files don't. De-dup across APIs and against the library. Local-only refuses an alert. Fixture API replies only, no live calls in CI. One Inbox item per run. Navigation test for `whats_new`. |
| **4. Automations** | `model_calls` ledger (migrates `table_calls`); playbooks; step kinds; the `automation` profile rule; durable approvals; the Automations view with templates | Ledger migration keeps the table counts, and the table cap still holds. A refused reservation waits as `daily_cap`. An agent step's Write call becomes an approval and is never run, in every `AgentMode`. Execution tools are absent from the profile. Approvals survive a restart and are re-posted. Expiry cancels. A cache hit makes 0 calls. Estimate text. Recorded-model fixtures only. Coverage and navigation for `automation`. |
| **5. One clock** | the table tick, the reminder planning and the code-map nightly reconcile run as `system` jobs in the engine loop; `spawn_scheduler`'s interval removed | Every existing `reminders.rs` and tables scheduler test passes. Reminders still ring once, and missed ones are summarised once. Code map: "refreshed nightly · last 02:00". |
| **6. Tasks in `shodh.db` + history** | `task`, `task_link`, `task_change`; import; `TaskStore`; History section | Import round-trip of a fixture `calendar_data.json` (all fields, events kept, backup written). A failed import leaves the file untouched. Phone wire shapes unchanged (golden JSON). Agent previews unchanged. Every write path records a change with its actor. |
| **7. Linear-like Projects** | List and Board across projects, My work, At risk, links and DAG, owner Shodh, keys | Cycle refusal. At-risk rules table test. `task_unblocked` fires once. Key map tests. Navigation for `tasks`. Coverage. |
| **8. Early assistant** | `propose_items` + cards; daily review job; Review strip | Saved items carry turn and citations. Proposals write nothing. The review's deterministic findings come from fixtures. Dismiss suppresses for 14 days. No desktop notification from the review. The model pass is off by default and counted when on. |

## 14. Risks

- **Quota oversubscription.** The default caps already add up to more than 50 a day, and chat is uncounted. The
  ledger makes the total visible, but chat still isn't capped. Mitigation: the total is shown in every estimate,
  and the automations default is a low 10.
- **A Windows sleep inside a run.** The lease expires and the run resumes at its step. The deterministic ids
  prevent duplicate effects. Agent steps are re-run from their start, so their calls are counted twice at worst.
- **The task storage move (PR 6)** touches the phone, the agent, reminders and the index at once. The golden wire
  tests and keeping events in the JSON file bound the blast radius. The backup file is the way back.
- **Prompt injection through playbooks.** It is bounded by the profile: no execution or outward tools, every
  write as an approval. The remaining risk is a misleading summary. Summaries cite their sources, and the user
  sees them only as text.
- **Approval fatigue.** A playbook that proposes many writes gets one approval card listing all of them, with
  per-item checkboxes, not one card per write.
- **Alert APIs changing or rate-limiting.** Each API fails alone, and the item says which one was checked.

## 15. Open questions for the user

1. **Are projects your workspaces, or a new thing inside them?**
   - (a) Workspace = project. Its sources, chats and tasks are already together.
   - (b) A separate Project, with several per workspace.
   - **Recommendation: (a).** It is one concept, no migration of meaning, and Linear-like views work across
     workspaces.
2. **The default daily cap for automations.**
   - (a) 10 calls.
   - (b) 5 calls.
   - (c) None by default, with each playbook asking.
   - **Recommendation: (a).** Tables 30 + Slack 20 + Telegram 20 already exceed the free 50.
3. **The Inbox tab names.**
   - (a) "Needs you" / "FYI", with Signal / Noise as subtitles.
   - (b) "Signal" / "Noise".
   - **Recommendation: (a).** Plain words, which the app's text rules prefer.
4. **A repeating task when the last copy isn't done.** Should the default be "make the next one anyway" or "wait
   until it's done"? **Recommendation: make it anyway.** It is Linear's behaviour, and catch-up never makes more
   than one.
5. **The daily review's model pass.**
   - (a) Off by default, deterministic checks only.
   - (b) On, at 1 call a day.
   - **Recommendation: (a).** The checks find the real problems for free.
6. **Paper alerts once turned on: all three APIs or arXiv only?** **Recommendation: all three.** It is one request
   each per weekly run, with the consent line naming each.
