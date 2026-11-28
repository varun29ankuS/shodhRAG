# Proactive PR 5: one clock, and continuing a run after a restart (plan, 2026-11-28)

Spec: `docs/superpowers/specs/2026-11-22-proactive-assistant-design.md` §1, §5.3, §13 (row 5), §14.
Builds on PR 4's engine (`crates/shodh-rag/src/engine`, `app/src-tauri/src/engine_commands.rs`).

## What is there today

- **The engine** (`engine::runner::Engine`) already plans jobs from `next_due_at`, catches up missed times,
  holds one active run per job, retries on the ladder and resumes a run at its step after a crash (leases,
  deterministic ids). Two loops drive it in `engine_commands.rs`: one for `task_repeat`, What's new and paper
  alerts, one for playbooks (automations). `JobKind::System` exists and has no handler.
- **Three timers outside it**:
  - task reminders: `reminders.rs::spawn`, its own sleep loop;
  - the evidence-table runner: `table_commands.rs::spawn_scheduler`, a 1-minute `interval`;
  - the code map: `code_map_commands.rs::schedule`, a debounce after file changes. There is no nightly
    reconcile yet; the spec asks for one "through the new scheduler".
- **A chat answer interrupted by a restart** is marked `aborted` on load (`reducer.ts::fromPersisted`): "The app
  closed before this answer finished." Nothing offers to go on. An automation run already resumes at its step.

## What PR 5 builds

1. **One loop.** The two loops in `engine_commands.rs` become one, ticking every kind's handler, with one wake
   signal. It sleeps until the earliest due time, for at most 60 s, and wakes early on any job change (§5.3).
   Playbooks keep yielding to chat through the ledger (PR 4); one loop changes nothing there.
2. **System jobs**, one handler with a `system` input naming the work:
   - `table_tick`: the evidence-table runner's check, due when a table job is due (its own `next_due_at`),
     not every minute. `spawn_scheduler` and `SCHEDULER_TICK` are removed.
   - `reminder_plan`: reminders' next ring times planned by the engine; the ring itself stays in
     `reminders.rs` (notification, snooze). Its own loop is removed. Missed reminders after a sleep are
     summarised once (catch-up `once`), as the spec says.
   - `code_map_reconcile`: a nightly full reconcile of each open code map, at the time chosen below, catch-up
     once on wake. The debounced rebuild on file changes stays.
   System jobs never show in Automations; their runs are kept 7 days (a system job's history is for support,
   not for the user).
3. **Continue an interrupted answer** (decisions 1 and 2 below). On load, an answer the app closed during is
   still marked as stopped, and gains a **Continue** action. Continue starts a new turn in the same
   conversation that carries what the interrupted run already had: its question, the steps that finished
   with their results, its passages, and the text written so far. The model is told to go on from there,
   not to start again. A step that was running when the app closed is run again (its effect is idempotent
   where it writes, as engine steps are; a command or an outward action asks again).
4. **What the user sees**:
   - Code map: "Refreshed nightly · last 02:00" (or "not yet").
   - Settings › Background: one line, "Scheduled work runs while Shodh is open or in the tray."
   - An interrupted answer: "Stopped when Shodh closed" and **Continue**.

## Gates (tests)

- All `reminders.rs` and table-scheduler tests pass unchanged; a reminder rings once; reminders missed in a
  sleep are summarised in one notification (clock-driven tests with `testkit`'s clock).
- No `tokio::time::interval` and no second engine loop in `app/src-tauri/src` (a source test).
- A table job due at T runs at T, not at the next minute; an engine with no due job sleeps 60 s at most.
- Code map: a reconcile at the chosen time, one catch-up after a missed night, none twice.
- Continue: the continued turn's request carries the finished steps, passages and partial text of the
  interrupted transcript (replayed from a fixture in `tests/fixtures/transcripts`), and its answer replaces
  the stopped one's place in the conversation; the stopped answer stays readable above it.
- The launch tour adds a scene of an interrupted answer with Continue (both themes).

## Order of work

1. One loop (engine_commands) and the `system` handler, with the table tick moved first (smallest, best tested).
2. Reminder planning onto the engine.
3. Code map nightly reconcile and its line.
4. Continue: transcript restore keeps what the run had, the Continue action, the continuation turn in
   `agent_session_commands.rs`, the prompt note.
5. Tour scene, spec update.

## Decisions for the user

1. **An answer the app closed during: what happens on the next start?**
   - A. It shows "Stopped when Shodh closed" with a **Continue** button. Nothing runs until the user clicks.
     *(Recommended: a restart can be hours later, the question may no longer matter, and a run that calls a
     model or asks for approvals should not start by itself.)*
   - B. It continues by itself on the next start, and says so.
   - C. As today: it says it stopped, and the user asks again.
2. **What Continue does.**
   - A. Goes on from where it stopped: the finished steps, their results and the text written so far are
     kept; the step that was running is run again. *(Recommended: no repeated searches or reads, and the
     answer does not change course.)*
   - B. Asks the question again from the start, as a fresh answer.
3. **The code map's nightly refresh: when?**
   - A. At 02:00 your time; if the computer was asleep, once when it wakes. *(Recommended: the computer is
     usually idle then, and the map is fresh in the morning.)*
   - B. Whenever the computer has been idle for 10 minutes, at most once a day.
   - C. No nightly refresh: only after file changes, as today.
