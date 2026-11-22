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

A fourth and a fifth would make it worse. PR 5 folds the first two into the engine. Consolidation stays
separate, because it is triggered by idleness, not by time. The decision of 2026-10-31 stands: **one engine** serves both the
work people do (Projects and Tasks) and the work Shodh does (Automations). The engine is never named in the UI.
Nothing on screen says cron, DAG, YAML, job or trigger.

## 2. Goals, non-goals, anti-goals

**Goals**
1. **The engine** (§5): local-time schedules, catch-up without a stampede, overlap policy, retries, durable
   approval steps, readable run history; in `shodh.db`, reusing the table runner's parts.
2. **Automations** (§8): playbooks, i.e. saved instructions for recurring jobs; skills become saved recipes.
3. **What's new** (§7): new files in the user's folders; new papers only if the user opts in.
4. **Projects and Tasks** (§9): Linear-like, keyboard-first, relationships, links, provenance on every change.
5. **The early assistant** (§10): one-click proposals from chat, a daily review with logged reasoning, nudges
   in the app only.
6. **The Signal/Noise Inbox** (§11): one place for everything, split into needs you and FYI.

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

Beyond local-first, Rust only (no new crate here), agent coverage and ideas-only from AGPL projects
(macro-inc/macro: the Signal/Noise split, the source -> task -> change -> result chain):

- **The free quota is already oversubscribed.** The default caps (tables 30, Slack 20, Telegram 20) add up to
  more than a 50-a-day free key, and chat is not counted at all. So every estimate shows Shodh's total calls
  today, not one feature's (§5.4).
- **Approvals belong to the user.** The agent never approves its own steps, never confirms its own proposals,
  and never changes what runs without asking. This follows the existing user-only exclusions
  (`inbox_dismiss`, `memory_suggestions_list`, `set_default_agent_mode`).

## 4. What exists today

| Area | Where | What it gives this design |
|---|---|---|
| Tasks and events | `app/src-tauri/src/calendar_store.rs` (`calendar_data.json`, one lock, atomic write), `calendar_commands.rs`, `app/src/features/tasks/*` (TasksStore, TaskBoard per workspace, TasksCalendar, ReminderAlerts, `dueDate.ts`, `board.ts`) | `TodoItem`, with `workspace_id` as "its project"; `source`/`source_ref`; `apply_task_patch` returns `FieldChange{field,before,after}`. Not relational, no history. |
| Users of tasks | `reminders.rs`, `agent_tools/calendar.rs` (10 tools), `phone_link.rs:1149` (the phone calls `load_tasks`, `create_task`, ...), `calendar_indexer::reconcile_task` (`calendar://task/{id}`), `store_notices`, `detach_workspace_tasks` | All must keep working when storage moves (PR 6). |
| Reminders | `reminders.rs`: `MAX_SLEEP` 60 s, wakes on change, marks fired before notifying, one summary for missed | The wake and DST pattern (`resolve_local`). |
| `shodh.db` | `crates/shodh-rag/src/audit/store.rs` `MIGRATIONS` (append-only; 18 is the latest on master), `open_shared_connection` | One migration per PR. A CHECK change rebuilds the table (as 16 and 18 did). |
| Table runner | `research/tables/jobs.rs` (lease, heartbeat, `LEASE_MINUTES` 10, `BACKOFF_LADDER_MINUTES`, pause reasons, `MAX_PANICS`), `schedule.rs` (`RunnerSource`/`RunEvents`), `calls.rs` (`CallCounter::try_record`, UTC day, `DailyCap`) | What §5.1 lifts out. |
| Other call counters | `connectors/slack_usage.rs` (JSON), Telegram (the same), `user_memory::learn::UsageToday` | One ledger (§5.4). |
| Inbox | `crates/shodh-rag/src/inbox.rs` (working / needs_you / done / failed; `recover` drops approvals on launch), `inbox_commands.rs::post`, `features/inbox/InboxButton.tsx` and `model.ts` | Signal/Noise is a view over it. **Bug found:** `InboxKind::Model` (`model_fallback.rs:586`) is missing from migration 18's CHECK, so "model keeps failing" items are rejected and only logged. |
| Notifications | `notifications.rs` (categories, unfocused window only, paced, no content) | Automations call `notify`; the review does not. |
| Memory proposals | `user_memory/learn/inbox.rs` (`ProposalView` with `conversation_id`/`turn_id`), `MemoryContent::Fact{class:"Decision"}`, `Person` entities, `Actor` | Decisions, facts, people, one-click confirm. |
| Folder watch | `folder_sync.rs` (`SyncReport`: counts only), `file_watcher.rs` | What's new needs added file names (§7). |
| Papers | `harness/web/papers.rs` (arXiv, OpenAlex, S2; merge; local ranking) | Paper alerts, no model calls. |
| Agent safety | `harness/modes.rs` (`decide(mode, ToolClass)`, an always-ask floor), `UNTRUSTED_NOTICE`, `RiskTier` | The unattended-run rule (§5.5). |
| Headless agent | `agent_transcript.rs` (folds `agent_event` as the UI does, with or without a window) | Playbook runs save a real transcript. |
| Navigation | `navigate.rs` `VIEWS` = `lib/viewTabs.ts` `VIEW_TABS` (8; no Automations), `NavigationTarget::KINDS`, `agent_coverage.rs::kind_owner`, `navigation.ts`, `ChatSessionContext.followNavigation` (the gate) | The checklist in §12. |
| Code map | `code_map_commands.rs::schedule`; the user asked for a nightly reconcile "through the new scheduler" | A system job (PR 5). |

## 5. The engine

### 5.0 Approaches considered

| | A. Make the table runner the engine | **B. A shared engine core (recommended)** | C. In-memory cron crate (tokio-cron-scheduler) + thin persistence |
|---|---|---|---|
| Idea | Make `table_fill_job`/`jobs.rs` generic; migrate fills into engine jobs | Move the runner's generic parts into `shodh_rag::engine`; tables import them. The engine adds schedules, triggers, runs, steps, approvals. Fills stay `table_fill_job`s, started by engine steps. | Schedule in memory, write run rows ourselves |
| Cost / risk | Rewrites ~120 KB of shipped, fix-rounded code and tests; table semantics (queued cells, per-paper calls) leak into every job | Small: a pure move proved by the unchanged table tests | No durable catch-up, overlap or approvals; we'd write persistence anyway, plus a second runtime |
| One runner? | Yes, expensively | Yes: one lease, one ladder, one ledger, one loop | No |

**Recommendation: B.** It reuses the runner instead of copying it, and its first PR is small.

**Schedules are typed, not cron.** The UI offers once, every day, weekdays, weekly on chosen days, monthly on
day N or the last day, and every N hours. The engine stores a typed `Schedule` enum (JSON, with an IANA zone)
and computes the next time with `chrono`, `chrono-tz` and `iana-time-zone` (all in `Cargo.lock`), using
`reminders::resolve_local` for DST gaps. Issue #149 names `croner` (MIT); it is not added, because the user never
sees an expression, the enum is compiler-checked and reads plainly in history, and the code is about 200 lines
with exhaustive tests. If schedules outgrow the enum, croner is the fallback, with a NOTICE entry.

### 5.1 What moves into `crates/shodh-rag/src/engine/` (PR 1, a pure move)

- `lease.rs`: the `run_token` claim, heartbeat, `LEASE_MINUTES`, the live-run slot and `PROCESS_TOKEN` (`jobs.rs`).
- `backoff.rs`: `BACKOFF_LADDER_MINUTES`, `STALE_RESET_WAIT_SECS`, failure-streak and panic rules (`jobs.rs`).
- `clock.rs`: `Clock`/`SystemClock`, `utc_day`, `next_utc_midnight`, `day_key` (`calls.rs`).
- `run_source.rs`: `RunnerSource`/`RunEvents` and `RunnerError::Blocked` (`schedule.rs`).

`research/tables/*` re-imports them; `jobs_tests.rs` and `fill_tests.rs` pass with only import paths changed.

### 5.2 Data model (next free migration: 19 on master today)

```
engine_job      id PK, kind CHECK IN ('task_repeat','folder_watch','paper_alert','playbook',
                'daily_review','system'), subject (task id, playbook id, source id, 'code_map:<p>'),
                title, schedule_json NULL, trigger_json NULL, enabled 0|1,
                overlap CHECK IN ('skip','queue_one'), catch_up CHECK IN ('once','skip'),
                max_attempts 1..5, next_due_at NULL (UTC; a cache, recomputed every plan),
                hold_reason NULL CHECK IN ('user','failing','daily_cap','model_unavailable','offline'),
                failure_streak, created_by CHECK IN ('user','agent','system'), provenance_json,
                created_at, updated_at
engine_run      id PK, job_id FK ON DELETE CASCADE,
                trigger CHECK IN ('schedule','catch_up','manual','event','retry'),
                scheduled_for NULL, missed, state CHECK IN ('queued','running','waiting',
                'succeeded','failed','skipped','cancelled'), wait_reason NULL CHECK IN ('approval',
                'daily_cap','quota','model_unavailable','offline','backoff'), attempt,
                next_attempt_at, run_token, heartbeat_at, planned_calls, used_calls,
                conversation_id NULL, summary, error_code, error_detail, started_at, ended_at, created_at
                UNIQUE INDEX engine_run_active ON engine_run(job_id)
                  WHERE state IN ('queued','running','waiting')
engine_step     run_id, idx, kind, state, input_json, output_json, started_at, ended_at, PK (run_id, idx)
engine_approval id PK, run_id, step_idx, title, preview_json, created_at, expires_at,
                decision NULL CHECK IN ('approved','denied','expired'), decided_at
engine_cache    job_id, input_key, output_json, created_at, PK (job_id, input_key)
```

The same migration rebuilds `inbox_items` with CHECK kinds plus `'model'` (fixing the bug in §4),
`'automation'`, `'whats_new'` and `'review'`. PR 1's hold item needs `'automation'`, and one rebuild serves every
later PR. Rust variants arrive with the PR that posts them.

Retention: all runs of the last 90 days, never fewer than the latest 20 per job. Deleting a job deletes its
runs, steps, approvals and cache.

### 5.3 Semantics

- **One loop.** It sleeps until the earliest `next_due_at`, for at most 60 s (the reminders rule: suspend and
  clock changes are seen within a minute), and wakes early on any job change. Local schedules become instants on
  every plan, so zone and DST changes apply at once.
- **Catch-up.** A run more than 5 minutes late means sleep or a closed app. With `once`, one run for the latest
  missed time, recording `missed` ("Ran when your computer woke; 2 earlier times were skipped"). With `skip`, a
  `skipped` run is recorded.
- **No stampede.** Due runs queue by `scheduled_for`, one at a time. Network or model runs also wait for the
  ledger (§5.4) and for 2 minutes after a wake. A failed connectivity check waits as `offline` on the ladder.
- **Overlap.** One active run per job (the unique index). At a due time with one active: `skip` (default) records
  "the previous run was still going"; `queue_one` keeps at most one queued behind it. Never two at once.
- **Retries.** Transient failures (network, 429 with no reset, `model_unavailable`) wait on the ladder up to
  `max_attempts` (3). A daily-quota error waits until the next UTC midnight. Permanent failures fail at once.
  Three failed runs in a row put the job on hold (`failing`) with a Signal item.
- **Idempotent effects.** A creating step derives its id from `(job_id, scheduled_for)` and inserts if absent, so
  a crash and resume never make two. Steps resume at their `idx`.
- **Leases** as in `jobs.rs`: a heartbeat before every step and model call; a stale lease is taken over after
  10 minutes.
- **Event triggers:** `files_added {source_id, pattern}`, `paper_found {alert_id}`, `task_unblocked {task_id}`.
  Events within one minute make one run, with their subjects as its input.

### 5.4 The call ledger (PR 4)

`model_calls(utc_day, spender CHECK IN ('tables','automations','review','slack','telegram','learning'), calls,
PK (utc_day, spender))`. `CallCounter` takes a spender. The migration copies `table_calls` in and drops it.
Slack, Telegram and learning move in when next touched; until then the total reads their own stores. Each
spender keeps its cap: automations default 10 a day (range 3-1,000); the review at most 1, only when turned on.
Every estimate says: "This run needs up to 4 model calls. Shodh has made 23 calls today (tables 12, Slack 6,
automations 5). Resets at 5:30 am."

### 5.5 Approvals and untrusted content

- An `approval` step writes `engine_approval` and posts a Signal item `engine:approval:<id>`. On launch,
  `InboxStore::recover` still drops session approvals; the engine then re-posts every open `engine_approval`. An
  approval expires after 7 days and cancels its run, saying so.
- **The unattended-run rule** is enforced by the engine, not the prompt. A playbook's `agent` step runs in a
  dedicated `automation` profile: reads run; execution and outward tools are not given (`run_command`, Code mode,
  GitHub writes, connector posts, downloads, MCP tools not set to read); every Write or Destructive call ends the
  step as an approval step whose preview is the tool's own `ApprovalPreview`. This holds in every `AgentMode`;
  since these runs read untrusted content (`UNTRUSTED_NOTICE` sources) by design, no agent write is
  pre-approved. Approving runs that one call, then the run continues in a new session with the transcript and
  the result as context. Several writes in one step make one card with a checkbox per write.
- **Deterministic steps are different.** Steps the user configured (`table_add_rows_and_fill`, `create_task`
  from a template, `find_papers`) are approved when the user saves the playbook, with its estimate. Their
  inputs (a new file, a found paper) are data; the content never chooses an action. Only writes the agent
  decides become approval steps.
- `engine_approval_decide` is excluded from agent coverage.

### 5.6 Visibility

"Run now" opens the run's conversation in Ask, and its navigation goes through `followNavigation`'s gate like
any answer. Scheduled runs never move the screen: they show in the run page (steps in words, calls, transcript)
and the Inbox, and **Show me what it did** replays their navigation targets one by one through the same gate.

## 6. PR 1's visible use: repeating tasks

Picked because it makes no model calls, needs no empty tab, exercises catch-up, overlap, retries and history,
and has an existing surface. The nightly code-map reconcile reaches only Code-mode users and a missed night
costs nothing; it becomes a system job in PR 5.

- **The series** is a `task_repeat` job. Its input is a snapshot of the **latest copy**: title, description,
  tags, priority, workspace, subtasks (unchecked), due time of day, the reminder as an offset from due. Each run
  creates the next copy, id `rep-<job>-<yyyymmddhhmm>`, insert-if-absent through `CalendarStore::update` and
  `calendar_changed`. No calendar-file migration; `subject` is the latest copy's id.
- **Lifecycle.**
  - Every copy's sheet shows Repeat; changing it on any copy changes the series.
  - Editing the latest copy refreshes the snapshot; edits to older copies don't.
  - Completing a copy never stops the series.
  - Deleting the latest copy stops the series, from every path: the UI confirm says "and stop repeating", the
    agent's `delete_task` preview says the same, and a phone delete does the same. Deleting an older copy does
    not.
  - `detach_workspace_tasks` also clears the workspace in the snapshot.
- **"If the last one isn't done":** *make the next one anyway* (default) or *wait until it's done*, a
  precondition step that records `skipped`.
- **Retries:** an unreadable or locked calendar file waits on the ladder, as `reminders.rs` handles
  `CalendarError`.
- **Commands:** `task_repeat_get`, `task_repeat_set`, `task_repeat_clear`, `task_repeat_runs`. **Tools:**
  `create_task`/`update_task` take `repeat` (preview: "Repeats every Monday at 09:00"); `list_tasks` shows the
  repeat and next time. Coverage maps the four commands to these tools.
- **UX** (task detail sheet; no empty state, the field reads "Doesn't repeat"):

```
 Submit timesheet
 Due     Mon 24 Nov, 17:00
 Repeat  [Every week on Mon v]   If the last one isn't done: [Make the next one anyway v]
         Next: Mon 1 Dec, 17:00
 History
   Mon 24 Nov  Made this one (your computer was off at 17:00; made Tue 08:12)
   Mon 17 Nov  Made                                   [Open]
```

  Error: "Couldn't make this week's copy: the task list couldn't be read. Trying again in 5 minutes."

## 7. What's new (PR 3)

- **New files.** `SyncReport` gains `added: Vec<String>` (first indexed in this sync, not changed or re-chunked;
  at most 200, plus `added_more`). A folder's first sync is a baseline and reports nothing. Each Library folder
  has "Show new files in What's new", on by default: a `folder_watch` job with an event trigger.
- **Paper alerts (opt-in).** A saved search, weekly: one `papers.rs` request per API per run, de-duplicated by
  DOI, arXiv id or title against the library and earlier items, ranked by the local cross-encoder, no model
  calls. Turning one on shows: "Once a week, Shodh will send the words *linear attention KV cache* to arXiv,
  OpenAlex and Semantic Scholar. Nothing else leaves your computer. [Turn on] [Cancel]". Local-only mode refuses.
- **Data:** `feed_item(id, kind CHECK IN ('file','paper'), source_ref, title, detail, found_at, seen_at,
  dismissed_at, link_json, data_json)`, unique `(kind, source_ref)`, kept 90 days.
- **Inbox:** one Noise item per run ("4 new papers for 'linear attention'"), never one per paper.
- **Commands -> tools:** `whats_new_list` -> `whats_new` (Read; opens Library -> What's new, highlights);
  `paper_alert_create`/`_update`/`_delete` -> `create_paper_alert`/`update_paper_alert`/`delete_paper_alert`
  (create and update `must_confirm`: they send text off the computer); `folder_watch_set` -> `set_folder_watch`.
  `whats_new_dismiss` and `whats_new_mark_seen` are excluded: what the user has seen is theirs, as with
  `inbox_dismiss`.
- **UX:** Library -> **What's new**.

```
 What's new                                              [+ Paper alert]
 Today
   kv-cache-survey.pdf       Research Papers folder, 10:41     [Open] [Add to table v] [x]
   "Gated Linear Attention..." (2026)  arXiv, alert: linear attention  [Abstract] [Add to library]
 Earlier this week
   ...
```

  Empty: "Nothing new yet. New files in your Library folders show up here. You can also turn on alerts for new
  papers on a topic." Error: "arXiv didn't answer this week's check. Shodh will try again in an hour; OpenAlex
  and Semantic Scholar were checked." "Add to library" uses `download_file`, only for open-access PDFs the API
  reported.

## 8. Automations and playbooks (PR 4)

- **A playbook:** name, instruction, scope (a workspace or the library), when (a typed schedule, an event such
  as "a new file arrives in..." or "a paper alert finds something", or by hand only) and steps. Templates set
  the steps; a user-written playbook has one `agent` step. Stored as `playbook(id, name, instruction,
  scope_json, steps_json, revision, created_by, provenance_json, created_at, updated_at)` plus its `engine_job`.
- **Step kinds (v1):** `find_papers`, `table_add_rows_and_fill` (adds rows, starts the table's own
  `table_fill_job`, which keeps its own cap, and waits for its end through `RunEvents`), `create_task`, `agent`,
  `approval`, `notify`.
- **Templates.** *Weekly literature check*: `find_papers`, then one `agent` call summarising what is new, citing
  each paper (1 call a week). *Screen a new paper against my criteria*: on `files_added`, add the paper to a
  chosen evidence table and fill it; its yes/no columns are the criteria (1-2 calls per paper). Include/exclude
  decisions come with evidence-table v2 screening; until then the row's cells are the result.
- **Skills as recipes:** "Save as playbook" on an installed skill copies its instructions into an `agent` step.
  Installing skills stays user-only.
- **Cache:** `engine_cache` keyed by (playbook revision, input hash); unchanged inputs make no call.

| Command | Agent tool | Tier / reason |
|---|---|---|
| `automations_list`, `automation_get`, `automation_runs`, `automation_run_get` | `list_automations`, `show_automation` | Read; opens Automations and highlights |
| `automation_create`, `automation_update` | `create_automation`, `update_automation` | Write, `must_confirm` (future work, quota); preview shows schedule and estimate |
| `automation_delete` | `delete_automation` | Destructive |
| `automation_pause`, `automation_resume` | `pause_automation`, `resume_automation` | Write, local edit |
| `automation_run_now` | `run_automation` | Write, `must_confirm`, with the estimate |
| `engine_approval_decide` | excluded | The assistant never approves its own steps |
| `automations_settings_set` | excluded | The daily cap is the user's spending decision, as `tables_settings_set` is |

**UX.** A new **Automations** view between Tasks and Activity (the approved order). It ships with templates, so
it is never an empty placeholder.

```
 Automations                                                    [+ New automation]
 Weekly literature check   Mondays 09:00, next Mon 1 Dec      Last: found 4 papers   [Pause] [...]
 Screen new papers         When a file arrives in Research Papers   Last: filled 2 papers
 ! Monthly summary         On hold: failed 3 times in a row (model unavailable)  [Check model] [Resume]

 New automation
   What should Shodh do?  [Every Monday, look for new papers on my topics and summarise them...]
   When?    (o) Every [week v] on [Mon] at [09:00]   ( ) When a new file arrives in [folder v]
            ( ) Only when I start it
   Where?   [Workspace: Thesis v]
   Each run: up to 4 model calls (about 16 a month). Anything that changes your data waits for your OK.
                                                                    [Cancel] [Save]
```

Run page: when and why it ran ("Ran when your computer woke; Monday's run was missed"), each step's outcome,
calls used of estimated, [Open conversation], [Show me what it did]. Empty: the three templates as cards with
"Set up". Errors: "Paused: today's 10 automation calls are used. It goes on after 5:30 am." / "Waiting for you:
it wants to add 3 tasks. [Review]".

## 9. Projects and Tasks

### 9.1 Storage moves to `shodh.db` (PR 6)

```
task         id PK, title, description, status, priority, due, reminder, reminder_fired_at,
             snoozed_until, owner_kind CHECK IN ('me','person','shodh'), owner_ref NULL,
             workspace_id NULL, tags_json, subtasks_json, source, source_ref, provenance_json,
             created_at, updated_at, completed_at
task_link    task_id, kind CHECK IN ('blocks','relates','duplicate_of','person','document',
             'decision','workspace','conversation'), target, created_at, created_by,
             PK (task_id, kind, target)
task_change  id PK, task_id, at, actor CHECK IN ('user','agent','phone','automation','import'),
             actor_ref NULL (conversation+turn, run id, device), field, before_json, after_json,
             reason NULL, source_json NULL
```

- **Import.** On first launch after the update, tasks from `calendar_data.json` are inserted in one transaction,
  **keeping their ids** (so `engine_job.subject` and `calendar://task/{id}` still match), insert-if-absent, one
  `import` change each. The original is kept as `calendar_data.pre-tasks.json`; tasks leave the JSON only after
  the commit, and events stay there. A crash between the commit and the JSON rewrite is safe: the next launch
  skips the existing ids and finishes the rewrite. A failed import leaves the file untouched, shows a store
  notice and retries next launch.
- **Wire shapes unchanged** (`TodoItem` JSON, `load_tasks`, `create_task`, ...). The phone, the agent tools
  (previews from the same `apply_task_patch` logic), `reminders.rs`, `calendar_indexer` and
  `detach_workspace_tasks` move to a `TaskStore` with the same functions.
- **History and provenance.** Every write stores its `FieldChange`s as `task_change` rows with actor and reason
  (the agent's stated reason, or the review rule). The audit log gets a counts-only `TaskChange` event, like
  `MemoryWrite`. The chain: source (`provenance_json`: conversation, turn, cited passages, Slack thread, feed
  item) -> task -> each change -> result (the completing change links the run, document or decision).
- **UX:** a History section in the detail sheet: "Shodh, in *Thesis plan* chat, 21 Nov: moved due to 28 Nov.
  Why: blocked by Data cleaning, due 27 Nov. [Open chat]".

### 9.2 Linear-like views (PR 7)

- **Projects are workspaces** (Q1); a workspace already owns its tasks.
- **Views:** List (by status), Board (`board.ts`, across projects), **My work** (mine, not done, by due),
  **At risk**, Calendar.
- **At risk is deterministic:** overdue; due within 2 days and still To do; a blocker not done and due after
  this task; or a blocker overdue. The reason shows on the row.
- **Relationships.** Blocks/blocked-by must stay acyclic: "That would make *A* wait for itself (A -> B -> A)".
  `task_unblocked` fires when the last blocker is done. People are memory `Person` entities, decisions are
  `Fact{class:"Decision"}` memories, documents are indexed sources.
- **Owner:** me, a person (a label; no one is notified), or Shodh. Giving a task to Shodh asks "What should
  Shodh do?" and links or creates a playbook; the task completes when that run succeeds.
- **Keys:** `C` new, `J`/`K` move, `X` select, `S` status, `P` priority, `D` due, `A` owner, `L` link, `Enter`
  open, `G M` My work, `G R` At risk, `/` filter, `Ctrl+K` palette. Each has a menu equivalent; the list is an
  ARIA grid like the evidence table.

```
 Tasks   [List] [Board] [Calendar]    My work | At risk (3) | All        / filter
 Thesis
   o Data cleaning        Me      Due Thu 27   ! overdue, blocks 2 tasks
   o Run baselines        Me      Due Fri 28   waiting on: Data cleaning
   o Draft related work   Shodh   Weekly literature check, last run Mon
```

  Empty: "Nothing assigned to you. Press C to add a task, or ask Shodh to plan something." / "Nothing at risk.
  Shodh checks due dates and blockers here."
- **Tools:** `link_tasks`, `unlink_tasks` (Write, local edit); `list_tasks` gains `view: my_work|at_risk` and
  `project`; `show_tasks` (Read) opens a view or project and highlights a task; `task_history` (Read).
  Commands `task_link_add`, `task_link_remove`, `task_history` map to them.

## 10. The early assistant (PR 8)

- **Proposals in chat.** A Read-tier tool `propose_items([{kind: task|decision|fact, text, fields, source:
  {conversation_id, turn_id, citations}}])` writes nothing; the answer shows a card per item:

```
 Shodh suggests
   [ ] Task      "Re-run baselines with the fixed split"  Thesis, due Fri, from [3]  [Save] [Edit] [x]
   [ ] Decision  "Use SIFT1M as the main benchmark"       from your message          [Save] [x]
```

  **Save** creates the task (provenance: the turn and cited passages) or the memory (`Fact`, class `Decision`
  or the fact's class, `Actor` the user, with the source turn). Unsaved cards stay with the message. No extra
  calls: it is part of the answer. `proposal_save` is excluded (the user's confirmation); the agent can still
  `create_task` under approval.
- **Daily review:** a `daily_review` job at 09:00, `catch_up = once`.
  - Deterministic checks, no calls: at-risk tasks, a done blocker still linked, a due date before a blocker's,
    likely duplicates (title embedding similarity >= 0.9 within a project), repeat copies piling up.
  - Each finding is a suggestion with a structured fix (field -> value) and its reasoning in words.
  - An optional model pass (off by default, 1 call a day, in the estimate) ranks and words them and may add
    suggestions; its reasoning is stored in the run.
  - Apply writes a `task_change` (actor `automation`, run id, reason). Dismiss keys on (rule, task, value) and
    suppresses it for 14 days.
  - In-app only: one Signal item ("Today's review: 4 suggestions") and a Review strip atop Tasks; `notify` is
    never called for it.
  - Tools: `task_review` (Read, opens the strip); `review_apply` maps to `update_task`; `review_dismiss` is
    excluded (the user's judgement).

## 11. The Signal/Noise Inbox (PR 2)

- **A view over the existing store.** Signal: every `needs_you` item (approvals, memory suggestions, rung
  reminders, engine approvals, jobs on hold, review suggestions, assignments to me by a run or connector), plus
  `failed` items that stop something the user owns. Noise: everything else (finished work, What's new digests,
  automation successes, Slack replies already posted). The rules are fixed and shown in a "Why is this here?"
  popover. The badge counts Signal only.
- **UX:** two tabs in the Inbox panel, **Needs you (3)** and **FYI (12)**, subtitled Signal and Noise; keys and
  actions as today. Empty: "Nothing needs you. Shodh puts approvals, reminders and suggestions here." / "Nothing
  new. Finished work and updates show here."
- No new commands; `inbox_list` and `inbox_dismiss` keep their exclusions.

## 12. Agent coverage checklist (every PR with UI)

1. Every new command has an `agent-coverage.json` entry: tool(s), or `excluded` with a reason. The rule: **the
   agent may read, list and propose; it never approves its own steps, confirms its own proposals, or changes
   caps or what runs without asking.**
2. A new view (Automations, PR 4) changes `VIEWS` (`navigate.rs`) and `VIEW_TABS` (`viewTabs.ts`) together,
   with labels, keywords and the `navigation.views` entry (`open_view`, `show_automation`).
3. A new target kind (`automation` PR 4, `tasks` PR 7) needs `NavigationTarget::KINDS` with a round-trip test, a
   `kind_owner` arm, `case '<kind>':` in `navigation.ts`, `useNavigationTarget('<kind>'` in its owner view, and
   an `expect_navigation` test per tool.
4. A new Library section (`whats_new`, PR 3) needs `LIBRARY_SECTIONS` on both sides and `target.section ===
   'whats_new'` in `LibraryView.tsx`.
5. Write tools highlight what they changed after saving; they never open a dialog.

## 13. Phasing into PRs

Each PR ships alone and is useful alone. Signal/Noise comes second because everything after it posts there.

| PR | Scope | Acceptance tests |
|---|---|---|
| **1. Engine + repeating tasks** | `engine/` move; `Schedule`; engine tables and the inbox rebuild (+`model`); the loop; `task_repeat`; Repeat and History in the sheet | Table `jobs_tests`/`fill_tests` unchanged and green. Next-time tests across DST gaps and overlaps (Europe/London, America/New_York, Asia/Kolkata), month ends, leap years. A simulated 3-day sleep makes one run with `missed = 2`; `skip` records `skipped`. A crash mid-run makes no duplicate copy. The index refuses a second active run. A locked calendar retries, then holds after 3 runs. Series lifecycle (edit latest, delete latest, delete older, workspace detach). A `model` Inbox item is stored (bug regression). Migration keeps every Inbox row. Coverage entries. |
| **2. Signal/Noise Inbox** | split view, badge, popover | Classification test per kind and status; tab counts and empty states (frontend model tests). |
| **3. What's new** | `SyncReport.added`; `feed_item`; folder watch; paper alerts; Library section; digests | First sync is a baseline. Added files show, changed don't. De-dup across APIs and against the library. Local-only refuses alerts. Fixture API replies only. One Inbox item per run. `whats_new` navigation. |
| **4. Automations** | ledger (migrates `table_calls`); playbooks; steps; `automation` profile; durable approvals; Automations view with templates | Table counts survive and the table cap holds. A refused reservation waits as `daily_cap`. An agent Write becomes an approval and never runs, in every `AgentMode`; execution tools absent. Approvals survive restart and are re-posted; expiry cancels. Cache hit = 0 calls. Estimate text. Recorded model replies only. `automation` coverage and navigation. |
| **5. One clock** | the table tick, reminder planning and the nightly code-map reconcile as `system` jobs; `spawn_scheduler`'s interval removed | All `reminders.rs` and table scheduler tests pass; reminders ring once, missed ones are summarised once; code map shows "refreshed nightly, last 02:00". |
| **6. Tasks in `shodh.db` + history** | `task`, `task_link`, `task_change`; import; `TaskStore`; History | Import round-trip of a fixture file (fields, ids, events kept, backup). Crash after commit: no duplicates. Failed import leaves the file. Phone wire golden JSON unchanged. Agent previews unchanged. Every write path records its actor. |
| **7. Linear-like Projects** | List and Board across projects, My work, At risk, links, owner Shodh, keys | Cycle refusal; at-risk rules table; `task_unblocked` once; key map; `tasks` navigation; coverage. |
| **8. Early assistant** | `propose_items` and cards; daily review; Review strip | Saved items carry turn and citations; proposals write nothing; review findings from fixtures; 14-day dismiss; no desktop notification; model pass off by default and counted when on. |

## 14. Risks

- **Quota oversubscription.** Default caps exceed 50 a day and chat is uncounted. The ledger shows the total in
  every estimate, and automations default to a low 10; chat stays uncapped.
- **Sleep inside a run.** The lease expires and the run resumes at its step; deterministic ids stop duplicates.
  An agent step restarts from its beginning, so its calls can be counted twice.
- **The storage move (PR 6)** touches the phone, agent, reminders and index at once. Golden wire tests, kept ids,
  events left in the JSON and the backup file bound it.
- **Prompt injection via playbooks** is bounded by the profile (no execution or outward tools, every agent write
  approved). What remains is a misleading summary; summaries cite their sources and are only text.
- **Approval fatigue:** one card per step with a checkbox per write, not one card per write.
- **Alert APIs changing or rate-limiting:** each fails alone and the item says which were checked.

## 15. Open questions for the user

1. Projects: (a) workspace = project, or (b) a separate Project, several per workspace? Recommend (a): one concept, already holding sources, chats and tasks.
2. Automations' daily call cap: (a) 10, (b) 5, or (c) none, each playbook asks? Recommend (a): tables 30 + Slack 20 + Telegram 20 already exceed the free 50.
3. Inbox tab names: (a) "Needs you" / "FYI" with Signal / Noise as subtitles, or (b) "Signal" / "Noise"? Recommend (a): plain words.
4. A repeating task whose last copy isn't done: (a) make the next anyway, or (b) wait until it's done? Recommend (a): Linear's behaviour, and catch-up makes at most one.
5. Daily review's model pass: (a) off by default, or (b) on at 1 call a day? Recommend (a): the deterministic checks find the real problems for free.
6. Paper alerts once turned on: (a) arXiv, OpenAlex and Semantic Scholar, or (b) arXiv only? Recommend (a): one request each per weekly run, all named in the consent line.
