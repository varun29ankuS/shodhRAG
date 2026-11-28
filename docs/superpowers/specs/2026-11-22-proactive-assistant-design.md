# The proactive assistant on one scheduler engine (design)

Date: 2026-11-22. Status: draft; the user's answers to its open questions are in §15.
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

A fourth and a fifth would make it worse. PR 5 folds the first two into the engine, with the code map's
nightly refresh. Consolidation stays separate, because it is triggered by idleness, not by time.

The decision of 2026-10-31 stands: **one engine** serves both the
work people do (Projects and Tasks) and the work Shodh does (Automations). The engine is never named in the UI.
Nothing on screen says cron, DAG, YAML, job or trigger.

## 2. Goals, non-goals, anti-goals

**Goals**
1. **The engine** (§5): local-time schedules, catch-up without a stampede, overlap policy, retries, durable
   approval steps, readable run history; in `shodh.db`, reusing the table runner's parts.
2. **Automations** (§8): playbooks, i.e. saved instructions for recurring jobs; skills become saved recipes.
3. **What's new** (§7): new files in the user's folders; new papers only if the user opts in.
4. **Projects and Tasks** (§9): projects of their own inside workspaces; Linear-like, keyboard-first,
   relationships, links, provenance on every change.
5. **The early assistant** (§10): one-click proposals from chat, a daily review with logged reasoning, nudges
   in the app only.
6. **The Attention/Info Inbox** (Signal/Noise, §11): one place for everything, in two tabs, **Attention** and **Info**.

**Non-goals (decided by the user: later)**
- The agent following up with other people, nudging others, or reassigning work on its own.
- Phone notifications for any of this. The phone companion keeps its own approval and answer notices.
- Team sync, shared projects, iroh.
- "Show and explain" (whiteboard, callouts and walkthroughs).
- Agents co-editing documents in place.

**Anti-goals (a failure even if it works)**
- A schedule editor, cron strings or a step graph anywhere in the UI.
- Any model call the user did not see an estimate for, or a background call that leaves the user's own chat
  without an answer.
- Untrusted text (a paper, a web page, a Slack message, a file in a watched folder) causing a write, a command
  or an outward action without the user's approval.
- A background run moving the user's screen.
- A second job runner next to the evidence-table runner.
- A task change with no record of who or what made it, and why.

## 3. Constraints

Beyond local-first, Rust only (no new crate here), agent coverage and ideas-only from AGPL projects
(macro-inc/macro: the Signal/Noise split, the source -> task -> change -> result chain):

- **The free quota is already oversubscribed.** The default caps (tables 30, Slack 20, Telegram 20) add up to
  more than a 50-a-day free key, and chat is not counted at all. Automations get no cap (the user's decision);
  instead every estimate shows Shodh's total calls today, and automations yield to chat (§5.4).
- **Approvals belong to the user.** The agent never approves its own steps, never confirms its own proposals,
  and never changes what runs without asking. This follows the existing user-only exclusions
  (`inbox_dismiss`, `memory_suggestions_list`, `set_default_agent_mode`).

## 4. What exists today

| Area | Where | What it gives this design |
|---|---|---|
| Tasks and events | `app/src-tauri/src/calendar_store.rs` (`calendar_data.json`, one lock, atomic write), `calendar_commands.rs`, `app/src/features/tasks/*` (TasksStore, TaskBoard per workspace, TasksCalendar, ReminderAlerts, `dueDate.ts`, `board.ts`) | `TodoItem`, with `workspace_id` (commented as "its project") and a free-text `project` string; `source`/`source_ref`; `apply_task_patch` returns `FieldChange{field,before,after}`. Not relational, no history. |
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
| Code map | `code_map_commands.rs::schedule`; the user asked for a nightly reconcile "through the new scheduler" | A system job (PR 5): 02:00 local, once on wake after a missed night. |

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
                hold_reason NULL CHECK IN ('user','failing','model_unavailable','offline'),
                failure_streak, created_by CHECK IN ('user','agent','system'), provenance_json,
                created_at, updated_at
engine_run      id PK, job_id FK ON DELETE CASCADE,
                trigger CHECK IN ('schedule','catch_up','manual','event','retry'),
                scheduled_for NULL, missed, state CHECK IN ('queued','running','waiting',
                'succeeded','failed','skipped','cancelled'), wait_reason NULL CHECK IN ('approval',
                'decision','yield','quota','table_job','model_unavailable','offline','backoff'), attempt,
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
  every plan, so zone and DST changes apply at once. Each kind ticks on a task of its own, one tick of a kind at a
  time, so an automation's agent run never holds a reminder up (PR 5).
- **System jobs** (PR 5) are the app's own work: never shown, never held (no one could resume them), kept 7 days.
  One without a schedule runs at a due time its owner sets, to the second: a fill's pause ending, the next
  reminder. Without `shodh.db` the loop still rings reminders from the calendar file.
- **Catch-up.** A run more than 5 minutes late means sleep or a closed app. With `once`, one run for the latest
  missed time, recording `missed` ("Ran when your computer woke; 2 earlier times were skipped"). With `skip`, a
  `skipped` run is recorded.
- **No stampede.** Due runs queue by `scheduled_for`, one at a time. Network or model runs also wait for the
  ledger (§5.4) and for 2 minutes after a wake. A failed connectivity check waits as `offline` on the ladder.
- **Overlap.** One active run per job (the unique index). At a due time with one active: `skip` (default) records
  "the previous run was still going"; `queue_one` keeps at most one queued behind it. Never two at once.
- **Retries.** Transient failures (network, 429 with no reset, `model_unavailable`) wait on the ladder up to
  `max_attempts` (3). A daily-quota error waits until the next UTC midnight. Permanent failures fail at once.
  Three failed runs in a row put the job on hold (`failing`) with an Attention item.
- **Idempotent effects.** A creating step derives its id from `(job_id, scheduled_for)` and inserts if absent, so
  a crash and resume never make two. Steps resume at their `idx`.
- **Leases** as in `jobs.rs`: a heartbeat before every step and model call; a stale lease is taken over after
  10 minutes.
- **Event triggers:** `files_added {source_id, pattern}`, `paper_found {alert_id}`, `task_unblocked {task_id}`,
  `task_completed {task_id}`.
  Events within one minute make one run, with their subjects as its input.

### 5.4 The call ledger and "chat first" (PR 4)

`model_calls(utc_day, spender CHECK IN ('chat','tables','automations','review','slack','telegram','learning'),
calls, PK (utc_day, spender))`. `CallCounter` takes a spender. The migration copies `table_calls` in and drops
it. Chat is now counted (never capped) so the total is honest. Slack, Telegram and learning move in when next
touched; until then the total reads their own stores. Tables, Slack and Telegram keep their existing caps.

**Automations have no daily cap** (the user's decision). Our suggestion, in its place, is a safeguard with no
number for the user to tune:

1. **An estimate before every run, and a ceiling per run.** `planned_calls` comes from the steps (an `agent`
   step's bound is the profile's step limit). A run never exceeds its estimate; at the ceiling it ends with what
   it has and says so.
2. **One shared counter, always visible:** in the estimate, on the Automations view and in Settings ->
   Model: "Shodh has made 23 calls today (chat 9, tables 8, automations 6). Your provider says 27 are left. Resets
   at 5:30 am." The provider figure appears only when the provider reports one (the rate-limit headers table
   fills already read, `fill.rs` `LimitWindow`).
3. **Automations yield to chat:**
   - An automation model call waits (`wait_reason = 'yield'`) while a chat answer is in flight, and before each
     call it re-checks.
   - After any rate-limit or quota error, from any spender, automations stop calling until the window the
     provider reports resets (or the next UTC midnight for a daily limit with no reset time). Chat keeps trying
     as it does today.
   - When the provider reports what is left today, automations stop once it falls to the **chat reserve**: the
     most chat calls the user made on any one day in the last 14 days (at least 1). It is measured from the
     ledger, so it needs no setting, and it keeps a normal day's chat answerable.
   - Paused runs resume by themselves when the window resets. One Info item says so and is updated, not
     repeated: "Automations paused so your chat keeps working. They go on after 5:30 am."
   - With a local model (LM Studio) only the first rule applies; there is no quota.

The review spends at most 1 call a day, only when turned on (§10).

### 5.5 Approvals and untrusted content

- An `approval` step writes `engine_approval` and posts an Attention item `engine:approval:<id>`. On launch,
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
  tags, priority, workspace, subtasks (unchecked), due time of day, the reminder as an offset from due. A copy
  for an occurrence has id `rep-<job>-<yyyymmddhhmm>`, created insert-if-absent through `CalendarStore::update`
  and `calendar_changed`. No calendar-file migration; `subject` is the latest copy's id.
- **The next copy waits until the last one is done** (the user's decision). Two things drive the series:
  - **The latest copy is completed** (a `task_completed` event, from the UI, the agent or the phone, detected
    in `calendar_changed`): the next copy is made at once, due at the next occurrence after the completed
    copy's due time.
  - **An occurrence's due time arrives while the latest copy is still open:** no copy is made. The run waits
    (`wait_reason = 'decision'`) and posts one Attention item, kind `reminder`, plus the usual reminder alert
    (the in-app reminder toast, and a desktop notification under the existing Reminders setting): "*Submit
    timesheet* was due again today, but last week's isn't done." Its three one-click answers:
    **Make the next one** (creates the copy for this occurrence), **Skip this one** (records `skipped`; the series
    waits for the next occurrence) and **Mark last week's done** (completes it, which then makes the copy for
    this occurrence).
- **No pile-up.** A series has at most one pending decision: it is the job's one active run (the unique index).
  If it is ignored and another occurrence passes, the same item is updated to the newest occurrence ("due again
  twice since"), its alert rings once for the new occurrence, and still only one copy can come of it. A pending
  decision never expires; the series simply waits. Completing the last copy answers it.
- **Sleep and catch-up.** On wake, occurrences that passed while the computer slept are folded into that same
  single decision, with `missed` counted. Their alert joins the existing "missed while closed" summary (one
  desktop notification for all of them). If the latest copy was already done, the completion already made the
  next copy, so the wake makes nothing.
- **Lifecycle.**
  - Every copy's sheet shows Repeat; changing it on any copy changes the series.
  - Editing the latest copy refreshes the snapshot; edits to older copies don't.
  - Completing a copy never stops the series; it makes the next copy.
  - Deleting the latest copy stops the series, from every path: the UI confirm says "and stop repeating", the
    agent's `delete_task` preview says the same, and a phone delete does the same. Deleting an older copy does
    not. Stopping the series removes its pending decision.
  - `detach_workspace_tasks` also clears the workspace (and, from PR 6, the project) in the snapshot.
- **Retries:** an unreadable or locked calendar file waits on the ladder, as `reminders.rs` handles
  `CalendarError`.
- **Commands:** `task_repeat_get`, `task_repeat_set`, `task_repeat_clear`, `task_repeat_runs`,
  `task_repeat_decide`. **Tools:** `create_task`/`update_task` take `repeat` (preview: "Repeats every Monday at
  09:00") and `update_task` takes `repeat_next: make|skip` (with approval, like any update); "Mark done" is
  `complete_task`; `list_tasks` shows the repeat, the next time and a pending decision. Coverage maps the five
  commands to these tools.
- **UX** (task detail sheet; no empty state, the field reads "Doesn't repeat"):

```
 Submit timesheet
 Due     Mon 24 Nov, 17:00
 Repeat  [Every week on Mon v]   The next one is made when this one is done.
         Next due: Mon 1 Dec, 17:00
 History
   Mon 24 Nov  Made when last week's was done (Fri 21 Nov)
   Mon 17 Nov  You chose "Make the next one" (the one before wasn't done)   [Open]
   Mon 10 Nov  Skipped by you

 Attention item:
   Submit timesheet was due again today, but last week's isn't done.
   [Make the next one] [Skip this one] [Mark last week's done]
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
- **Inbox:** one Info item per run ("4 new papers for 'linear attention'"), never one per paper.
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
| `automation_create`, `automation_update` | `create_automation`, `update_automation` | Write, `must_confirm` (future work, model calls); preview shows schedule and estimate |
| `automation_delete` | `delete_automation` | Destructive |
| `automation_pause`, `automation_resume` | `pause_automation`, `resume_automation` | Write, local edit |
| `automation_run_now` | `run_automation` | Write, `must_confirm`, with the estimate |
| `engine_approval_decide` | excluded | The assistant never approves its own steps |
| `model_usage_today` | `system_status` (its report gains the counter) | Read |

There is no automations cap and so no cap setting; the chat reserve is measured, not set (§5.4).

**UX.** A new **Automations** view between Tasks and Activity (the approved order). It ships with templates, so
it is never an empty placeholder.

```
 Automations                                                    [+ New automation]
 Weekly literature check   Mondays 09:00, next Mon 1 Dec      Last: found 4 papers   [Pause] [...]
 Screen new papers         When a file arrives in Research Papers   Last: filled 2 papers
 ! Monthly summary         On hold: failed 3 times in a row (model unavailable)  [Check model] [Resume]
 Today: Shodh has made 23 model calls (chat 9, tables 8, automations 6). Your provider says 27 are left.

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
"Set up". States in words: "Paused so your chat keeps working: your provider is close to today's limit. It goes
on after 5:30 am." / "Waiting while you chat." / "Stopped at its estimate of 4 calls; here is what it found." /
"Waiting for you: it wants to add 3 tasks. [Review]".

## 9. Projects and Tasks

### 9.1 Storage moves to `shodh.db`, with projects (PR 6)

**Projects are their own entity, several per workspace** (the user's decision). The project table comes in this
PR so the import converts the old free-text `project` once, and PR 7 only adds views and tools.

```
project      id PK, workspace_id NOT NULL, name, description, status CHECK IN ('active','paused',
             'done','archived'), target_date NULL, position, provenance_json, created_at, updated_at,
             UNIQUE (workspace_id, name)
task         id PK, title, description, status, priority, due, reminder, reminder_fired_at,
             snoozed_until, owner_kind CHECK IN ('me','person','shodh'), owner_ref NULL,
             workspace_id NULL, project_id NULL REFERENCES project(id), tags_json, subtasks_json,
             source, source_ref, provenance_json, created_at, updated_at, completed_at
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
  notice and retries next launch. The free-text `project`: each distinct (workspace, name) becomes a project
  (`created_by` import). A task with a project name but no workspace keeps the name as a tag, because a project
  must belong to a workspace; the import's store notice says how many.
- **Project rules**, enforced in `TaskStore` (one place for UI, agent and phone):
  - A task has at most one project, and that project must be in the task's workspace.
  - **Moving a task to another workspace** clears its project unless a project of the new workspace is given in
    the same change. The approval preview and the UI say "leaves project *Baselines*".
  - **Deleting a project** keeps its tasks: they stay in the workspace with no project, with one `task_change`
    each (reason "project deleted"). The confirm says "12 tasks stay in Thesis without a project". Archive is
    offered first, because it hides the project and keeps its tasks grouped.
  - **Deleting a workspace** (`detach_workspace_tasks`) deletes its projects and clears both `workspace_id` and
    `project_id` on their tasks, recording the changes. Repeat snapshots are cleared the same way.
- **Wire shapes kept** (`TodoItem` JSON, `load_tasks`, `create_task`, ...). `TodoItem` gains `projectId`.
  `project` stays, now the project's name. A write that gives `project` by name resolves it within the task's
  workspace, as the task tools resolve workspaces; an unknown name is an error listing that workspace's
  projects, never a silently created project. The phone, the agent tools (previews from the same
  `apply_task_patch` logic), `reminders.rs`, `calendar_indexer` and `detach_workspace_tasks` move to a
  `TaskStore` with the same functions.
- **History and provenance.** Every write stores its `FieldChange`s as `task_change` rows with actor and reason
  (the agent's stated reason, or the review rule). The audit log gets a counts-only `TaskChange` event, like
  `MemoryWrite`. The chain: source (`provenance_json`: conversation, turn, cited passages, Slack thread, feed
  item) -> task -> each change -> result (the completing change links the run, document or decision).
- **UX:** a History section in the detail sheet: "Shodh, in *Thesis plan* chat, 21 Nov: moved due to 28 Nov.
  Why: blocked by Data cleaning, due 27 Nov. [Open chat]".

### 9.2 Linear-like views (PR 7)

- **Where projects live.** In the Tasks view, a left list: workspaces, each with its projects (and "No
  project"). In a workspace's page, the existing task board groups by project. No new top-level view.
- **A project page:** name, status, target date, progress (done of total), at-risk count, and its tasks in List
  or Board. A project is at risk when it has a target date and an open task is due after it or is at risk.
- **Views:** List (by status), Board (`board.ts`, across projects), **My work** (mine, not done, by due),
  **At risk**, Calendar; each can be narrowed to a workspace or project.
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
 Tasks   [List] [Board] [Calendar]    My work | At risk (3) | All             / filter
 Thesis               | Baselines   active, target Fri 5 Dec   4 of 9 done   ! 2 at risk
   Baselines          |   o Data cleaning        Me      Due Thu 27   ! overdue, blocks 2 tasks
   Related work       |   o Run baselines        Me      Due Fri 28   waiting on: Data cleaning
   No project         |   o Draft related work   Shodh   Weekly literature check, last run Mon
 Reading group        |
   + New project      |
```

  Empty: "Nothing assigned to you. Press C to add a task, or ask Shodh to plan something." / "Nothing at risk.
  Shodh checks due dates and blockers here." / a workspace with no projects: "Group this workspace's tasks into
  projects. Press N to make one." Error: "A task can only be in a project of its own workspace. Move it to
  Thesis first, or pick one of Thesis's projects."
- **Keys** add `N` new project and `M` move to project.
- **Tools:** `list_projects` (Read), `create_project`, `update_project` (rename, status, target date, archive)
  (Write, local edit), `delete_project` (Destructive; the preview lists how many tasks lose their project);
  `create_task`/`update_task` take `project` (id or name within the workspace); `link_tasks`, `unlink_tasks`
  (Write, local edit); `list_tasks` gains `view: my_work|at_risk` and `project`; `show_tasks` (Read) opens a
  view, workspace or project and highlights a task; `task_history` (Read). Commands `project_list`,
  `project_create`, `project_update`, `project_delete`, `task_link_add`, `task_link_remove` and `task_history`
  map to them. The `tasks` target kind carries `{view, workspaceId, projectId, taskId}`.

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
  - Deterministic checks always run, with no calls: at-risk tasks and projects, a done blocker still linked, a
    due date before a blocker's, likely duplicates (title embedding similarity >= 0.9 within a project), a
    repeat decision left unanswered for 3 days or more.
  - Each finding is a suggestion with a structured fix (field -> value) and its reasoning in words.
  - **The model pass is off by default; the user can turn it on** in Settings -> General (an existing section): "Let the model help
    with the daily review (1 model call a day)". When on, it ranks and words the findings and may add
    suggestions; its reasoning is stored in the run, and its call is in the shared counter and yields to chat
    like any automation call. The setting is user-only (`AGENT_DENIED`, like notifications), because it spends
    model calls.
  - Apply writes a `task_change` (actor `automation`, run id, reason). Dismiss keys on (rule, task, value) and
    suppresses it for 14 days.
  - In-app only: one Attention item ("Today's review: 4 suggestions") and a Review strip atop Tasks; `notify` is
    never called for it.
  - Tools: `task_review` (Read, opens the strip); `review_apply` maps to `update_task`; `review_dismiss` is
    excluded (the user's judgement); `review_settings_set` is excluded (spending is the user's decision).

## 11. The Attention/Info Inbox (Signal/Noise, PR 2)

- **A view over the existing store.** Attention (Signal): every `needs_you` item (approvals, memory
  suggestions, rung reminders, repeat decisions, engine approvals, jobs on hold, review suggestions, assignments
  to me by a run or connector), plus `failed` items that stop something the user owns. Info (Noise): everything
  else (finished work, What's new digests, automation successes, "automations paused for chat", Slack replies
  already posted). The rules are fixed and shown in a "Why is this here?" popover. The badge counts Attention
  only.
- **UX:** two tabs in the Inbox panel, **Attention (3)** and **Info (12)**; Signal and Noise are design names
  only and appear nowhere in the UI. Keys and actions as today. Empty: "Nothing needs your attention. Shodh puts
  approvals, reminders and suggestions here." / "Nothing new. Finished work and updates show here."
- No new commands; `inbox_list` and `inbox_dismiss` keep their exclusions.

## 12. Agent coverage checklist (every PR with UI)

1. Every new command has an `agent-coverage.json` entry: tool(s), or `excluded` with a reason. The rule: **the
   agent may read, list and propose; it never approves its own steps, confirms its own proposals, or changes
   caps, spending settings or what runs without asking.**
2. A new view (Automations, PR 4) changes `VIEWS` (`navigate.rs`) and `VIEW_TABS` (`viewTabs.ts`) together,
   with labels, keywords and the `navigation.views` entry (`open_view`, `show_automation`).
3. A new target kind (`automation` PR 4, `tasks` PR 7) needs `NavigationTarget::KINDS` with a round-trip test, a
   `kind_owner` arm, `case '<kind>':` in `navigation.ts`, `useNavigationTarget('<kind>'` in its owner view, and
   an `expect_navigation` test per tool.
4. A new Library section (`whats_new`, PR 3) needs `LIBRARY_SECTIONS` on both sides and `target.section ===
   'whats_new'` in `LibraryView.tsx`.
5. Write tools highlight what they changed after saving; they never open a dialog.

## 13. Phasing into PRs

Each PR ships alone and is useful alone. The Attention/Info Inbox comes second because everything after it
posts there.

| PR | Scope | Acceptance tests |
|---|---|---|
| **1. Engine + repeating tasks** | `engine/` move; `Schedule`; engine tables and the inbox rebuild (+`model`); the loop; `task_completed` events; `task_repeat` with the wait-until-done decision; Repeat and History in the sheet | Table `jobs_tests`/`fill_tests` unchanged and green. Next-time tests across DST gaps and overlaps (Europe/London, America/New_York, Asia/Kolkata), month ends, leap years. A simulated 3-day sleep makes one run with `missed = 2`; `skip` records `skipped`. A crash mid-run makes no duplicate copy. The index refuses a second active run. A locked calendar retries, then holds after 3 runs. Repeats: completing the latest copy makes exactly one next copy, due at the following occurrence; an occurrence with the last copy open makes no copy and one decision; each of the three answers does what it says; an ignored decision across two more occurrences stays one item and one possible copy; a 3-day sleep folds into the same decision and the missed-reminder summary; completing from the phone answers the decision. Series lifecycle (edit latest, delete latest, delete older, workspace detach). A `model` Inbox item is stored (bug regression). Migration keeps every Inbox row. Coverage entries. |
| **2. Attention/Info Inbox** | two tabs, badge, popover | Classification test per kind and status; repeat decisions and automation yields land in the right tab; tab counts and empty states (frontend model tests). |
| **3. What's new** | `SyncReport.added`; `feed_item`; folder watch; paper alerts (arXiv, OpenAlex, Semantic Scholar); Library section; digests | First sync is a baseline. Added files show, changed don't. De-dup across APIs and against the library. The consent line names all three APIs. Local-only refuses alerts. Fixture API replies only. One Info item per run. `whats_new` navigation. |
| **4. Automations** | ledger (migrates `table_calls`; counts chat); playbooks; steps; `automation` profile; durable approvals; chat-first yielding; Automations view with templates | Table counts survive and the table cap holds. Chat calls are counted and never refused. A run never exceeds its estimate. An automation call waits while a chat answer runs. After a 429 from any spender, automations wait for the reported reset; chat does not. With a provider-reported remainder at the chat reserve, automations pause and resume after the reset, with one updated Info item. The reserve equals the busiest chat day of the last 14 (at least 1). An agent Write becomes an approval and never runs, in every `AgentMode`; execution tools absent. Approvals survive restart and are re-posted; expiry cancels. Cache hit = 0 calls. Estimate and counter text. Recorded model replies only. `automation` coverage and navigation. |
| **5. One clock** | one engine loop for every kind; the table tick, reminder planning and the nightly code-map refresh as `system` jobs; `spawn_scheduler`'s interval removed; Continue for an answer Shodh closed during | All `reminders.rs` and table scheduler tests pass; reminders ring once, those slept through are told in one notification; a table job due at T runs at T; no interval timer, one loop (a source test); the nightly refresh at 02:00, once after a missed night, never twice; code map shows "Refreshed nightly · last 02:00"; Continue carries the finished steps, passages and text (fixture), commands ask again; tour scene of the stopped answer. |
| **6. Tasks and projects in `shodh.db` + history** | `project`, `task`, `task_link`, `task_change`; import (free-text projects become projects); `TaskStore` with the project rules; `projectId` on the wire; History | Import round-trip of a fixture file (fields, ids, events kept, backup; projects made per workspace and name; workspace-less names kept as tags). Crash after commit: no duplicates. Failed import leaves the file. Phone wire golden JSON unchanged apart from the added `projectId`. Project rules: cross-workspace project refused; a workspace move clears the project; project delete keeps tasks with a change each; workspace delete removes its projects. Agent previews unchanged. Every write path records its actor. |
| **7. Linear-like Projects** | Tasks view's workspace/project list, project page, List and Board, My work, At risk (tasks and projects), links, owner Shodh, keys; project tools | Cycle refusal; at-risk rules table for tasks and projects; `task_unblocked` once; key map; project tools' previews (delete counts tasks); `tasks` navigation with workspace and project; coverage. |
| **8. Early assistant** | `propose_items` and cards; daily review; Review strip; the model-pass setting | Saved items carry turn and citations; proposals write nothing; review findings from fixtures; 14-day dismiss; no desktop notification; deterministic checks run with the model pass off; the pass is off by default, makes 1 call a day when on, counts in the ledger and yields to chat; the setting is refused to the agent. |

## 14. Risks

- **No cap on automations.** A playbook could spend much of a free day's quota. The per-run ceiling (its
  estimate), the shared counter and yielding to chat bound it, but a user who never chats can still let
  automations use the whole day. That is the user's choice, visible in the counter. The reserve also depends on
  the provider reporting what is left: without that report, only the in-flight and rate-limit rules apply.
- **Quota oversubscription elsewhere.** Tables, Slack and Telegram caps still add up to more than 50 a day;
  chat is now counted but not capped.
- **Repeat decisions nobody answers** stop the series quietly by design. The daily review flags one left 3 days
  or more.
- **Sleep inside a run.** The lease expires and the run resumes at its step; deterministic ids stop duplicates.
  An agent step restarts from its beginning, so its calls can be counted twice.
- **The storage move (PR 6)** now also turns free-text projects into entities, and it touches the phone, agent,
  reminders and index at once. Golden wire tests, kept ids, events left in the JSON and the backup file bound
  it. A phone build that writes `project` by name gets an error for an unknown name instead of a new project.
- **Prompt injection via playbooks** is bounded by the profile (no execution or outward tools, every agent write
  approved). What remains is a misleading summary; summaries cite their sources and are only text.
- **Approval fatigue:** one card per step with a checkbox per write, not one card per write.
- **Alert APIs changing or rate-limiting:** each fails alone and the item says which were checked.

## 15. Decisions (2026-11-22)

The user answered the open questions:

1. **Projects are their own entity, several per workspace** (§9.1, §9.2).
2. **No daily cap on automations' model calls.** In its place: a per-run estimate and ceiling, one shared
   counter, and yielding to chat with a measured reserve (§5.4).
3. **Inbox tabs are named "Attention" and "Info"** (§11).
4. **A repeating task waits until its last copy is done.** When the next copy falls due first, a single
   decision asks: make the next one, skip it, or mark the last one done (§6).
5. **The daily review's model pass is off by default and can be turned on in Settings** (1 call a day). The
   deterministic checks always run (§10).
6. **Paper alerts use arXiv, OpenAlex and Semantic Scholar**, all named in the consent line (§7).

Decided 2026-11-28 for PR 5:

7. **An answer Shodh closed during shows "Stopped when Shodh closed" with Continue.** Nothing runs by itself.
8. **Continue goes on from where the run stopped.** Finished steps, their results and the text written are
   kept; only the step that was running is run again. Commands, and anything leaving the computer, ask again.
9. **The code map refreshes nightly at 02:00 local time**, or once on wake when the computer slept through it.
10. **System-job history is kept 7 days.**
11. **Settings › Background says "Scheduled work runs while Shodh is open or in the tray."**
