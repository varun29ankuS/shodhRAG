# Proactive PR 6: tasks and projects in `shodh.db`, built for Linear-like PR 7 (plan, 2026-11-28)

Spec: `docs/superpowers/specs/2026-11-22-proactive-assistant-design.md` §9.1 (this PR), §9.2 (PR 7), §13 row 6.
Builds on PR 1 (`engine_job.subject` is a task id) and PR 5 (reminders are the `reminder_plan` system job).

The aim of this PR is the data PR 7 needs, so PR 7 is views, keys and tools only: no second migration of tasks.
What PR 7 shows (the bar is Linear) decides what is stored here:

| PR 7 shows | Stored by this PR |
|---|---|
| Issue ids like `SHO-12` | `workspace.issue_key` + `task.number` (one counter per key, never reused) |
| A status workflow | `task.status` from a fixed set of states, each in a category (decision 2) |
| Priority | `task.priority` 0 to 4 (decision 3) |
| Assignee: the user or Shodh | `task.owner_kind` `me` / `shodh` / `person` + `owner_ref` (spec §9.1) |
| Labels | `label` + `task_label` (decision 4) |
| Due date | `task.due` (kept) |
| Sub-issues | `task.parent_id` (decision 5) |
| Relations | `task_link` `blocks` / `relates` / `duplicate_of` (spec §9.1) |
| Projects: status, lead, target date, progress | `project` with `status`, `lead_kind`/`lead_ref`, `target_date`; progress is counted, never stored |
| What the assistant changed, per issue | `task_change` with actor `agent`, the conversation and turn, and the reason |
| Fast inline edit | one `TaskStore::patch` that validates and records a change per field, in one transaction |

## What is there today

- **Where tasks live.** Tasks and events are in `calendar_data.json`, handled by `calendar_store.rs` (`CalendarData`, `TodoItem`, `TaskPatch`, durable file, backup).
- **Who reads and writes them:**
  - the Tasks UI, through `calendar_commands.rs`;
  - the agent tools (`agent_tools/calendar.rs`);
  - the phone (`shodh-link`; `mobile/src/lib/agenda.tsx` and `outbox.ts`);
  - reminders;
  - repeating tasks (`task_repeat.rs`);
  - the calendar indexer (`calendar://task/{id}`);
  - workspace delete (`detach_workspace_tasks`).
- **Fields today:**
  - priority `low`/`medium`/`high`;
  - status `pending`/`in_progress`/`completed`;
  - free-text `project`;
  - free-text `tags`;
  - `subtasks` as a checklist;
  - `workspace_id`, optional.
- The next free migration of `shodh.db` is 24 at the time of writing.

## What PR 6 builds

1. **Migration 24.** Every table is in `shodh.db`, encrypted with it, with foreign keys deleted by hand as the engine does.
   - **`project`**: `id`, `workspace_id NOT NULL`, `name`, `description`, `status`, `lead_kind`, `lead_ref`, `target_date`, `position`, `provenance_json`, `created_at`, `updated_at`, `archived_at`. `UNIQUE (workspace_id, name)`.
   - **`task`**: the spec's columns, plus:
     - `number`, unique per issue key;
     - `issue_key`, copied at creation so an id never changes;
     - `parent_id`;
     - `position`, for manual order on the board;
     - `priority` as an integer.
   - **`task_link`, `task_change`**: as in the spec.
   - **`label`** (`id`, `name`, `color`, `UNIQUE (name)`) and **`task_label`** (`task_id`, `label_id`).
   - **`issue_counter`** (`issue_key` PK, `next`).
   - **`workspace.issue_key`**: a new column on the workspaces table.
2. **`TaskStore`** (`crates/shodh-rag/src/tasks/`). It is the one place for the rules, used by the UI, the agent, the phone, reminders and repeats.
   - **Reads:** `list` (filters: workspace, project, status category, owner, label, parent, due range), `get`, `get_by_key("SHO-12")`, `history`.
   - **Writes:** `create`, `patch`, `delete`, `link` / `unlink`, and projects' `create` / `patch` / `archive` / `delete`.
   - **One transaction per write.** Each write checks the rules, then records one `task_change` per field with the actor, the reason and the source. Writes return the task as changed.
   - **Rules** (spec §9.1, plus PR 7's):
     - a project must be in the task's workspace;
     - a workspace move clears the project;
     - deleting a project keeps its tasks;
     - a parent is in the same workspace;
     - no cycles of parents;
     - no cycles of blocks.
3. **The import.** Tasks move from `calendar_data.json` as the spec says:
   - one transaction, keeping ids, insert-if-absent;
   - a backup is kept in `calendar_data.pre-tasks.json`;
   - events stay in the file;
   - a crash after the commit is finished by the next launch;
   - a failed import leaves the file as it was and shows a store notice.

   How the old fields map:
   - Each (workspace, project name) pair becomes a project.
   - A project name on a task with no workspace stays as a label.
   - Tags become labels.
   - Statuses and priorities map as decisions 2 and 3 say.
   - Each task gets a number in creation order.
4. **Wire shapes kept.**
   - `TodoItem` JSON, `load_tasks`, `create_task` and `update_task` take and give what they do today. `TodoItem` gains `projectId`, `key` (`SHO-12`), `parentId`, `labels` and `owner`, all optional on input.
   - Old status and priority words are still accepted on input, and given back next to the new fields, so an older phone build keeps working.
   - Golden JSON tests pin the phone's shape.
5. **The movers.** These move from `CalendarStore` to `TaskStore`, keeping their behaviour and their tests:
   - `calendar_commands.rs`, `agent_tools/calendar.rs` (previews from the same `apply_task_patch` logic);
   - `reminders.rs` (the `reminder_plan` system job reads due reminders from `TaskStore`, marking a reminder fired in the same transaction);
   - `task_repeat.rs`, the calendar indexer, `detach_workspace_tasks`, and the phone's handlers.
6. **History in the task sheet.** The spec's minimal History section, so the change log is visible from day one: "Shodh, in *Thesis plan* chat, 21 Nov: moved due to 28 Nov. Why: ...". PR 7 makes it the full activity feed.
7. **No new view.** The Tasks view shows the same things, read from the new store. The key (`SHO-12`) shows on rows and in the sheet, so ids are seen and reported from now on. Tour shots: the task sheet with its key and History, in both themes.

## Gates (tests)

- **Import round trip.**
  - A fixture `calendar_data.json` with every field gives the same `TodoItem`s, ids included.
  - Projects are made per workspace and name; workspace-less names become labels; tags become labels; numbers follow creation order.
  - The backup exists, and events stay in the file.
- **Crash safety.**
  - A crash after the commit, before the file rewrite, gives no duplicates and finishes next launch.
  - A failed import leaves the file byte-identical and shows a notice.
- **Rules.** Each rule has a test with its refusal text: a cross-workspace project, a parent cycle, a blocks cycle ("That would make *A* wait for itself (A -> B -> A)"), and the counts of a project delete.
- **Keys.**
  - Numbers are unique per key under concurrent creates, and never reused after a delete.
  - A renamed workspace key keeps old ids readable (`get_by_key` resolves both).
- **History.** Every write path records its actor: user, agent with conversation and turn, phone with device, automation with run, import.
- **Wire.**
  - The phone's golden JSON is unchanged apart from the new optional fields.
  - An old-style patch (`status: "completed"`, `priority: "high"`) still works.
- **Unchanged behaviour.** Every existing test of reminders, repeats, the agent's task tools, the phone and the indexer passes unchanged, apart from imports.
- **Tour.** The task sheet with its key and History, in both themes, with no clipped text at 1000x700.

## Order of work (one PR, small commits)

1. Migration 24 and the store's types and rules, with their tests (crate only).
2. The import, with its fixture and crash tests.
3. `TaskStore` behind the existing commands; wire golden tests.
4. Reminders, repeats, the indexer and workspace delete move to it.
5. The agent tools and the phone move to it.
6. The key and History in the sheet; tour shots; spec update.

## Decisions for the user

1. **Issue ids: what goes before the number?**
   - A. **A short key per workspace**, made from its name and editable in the workspace's settings (*Thesis* -> `THE-12`). Tasks with no workspace use `TASK-12`. *(Recommended: as in Linear, the key says where an issue belongs, and the ids stay short.)*
   - B. **One key for everything**, `SHO-12`, numbered across all workspaces.
2. **The status workflow.**
   - A. **A fixed set: Backlog, Todo, In progress, Done, Canceled.** Today's statuses map as: pending to Todo, in progress to In progress, completed to Done. *(Recommended: Linear's own default; one obvious set, nothing to set up. Custom states can come later without a migration, because each state stores its category.)*
   - B. **The same set, plus "In review".**
   - C. **States each workspace can rename and add to**, now.
3. **Priority.**
   - A. **Linear's five: No priority, Urgent, High, Medium, Low.** Today's low/medium/high keep their names. New tasks start at No priority. *(Recommended: Urgent is what people sort by; "no priority" tells unsorted tasks from medium ones.)*
   - B. **Keep three (Low, Medium, High)**, defaulting to Medium as now.
4. **Labels: shared or per workspace?**
   - A. **Shared across workspaces**, each with a colour. Today's tags become labels. *(Recommended: tags are global today, and one "bug" label works everywhere.)*
   - B. **Per workspace**, so each project area has its own list.
5. **Sub-issues and today's checklists.**
   - A. **Keep the checklist inside a task, and add sub-issues as real tasks under a parent**, each with its own id, status and assignee. *(Recommended: nothing the phone shows changes, and a checklist stays light for small steps.)*
   - B. **Turn every checklist item into a sub-issue at the import**, with one way to break work down, as in Linear.
6. **Project statuses.**
   - A. **Linear's: Backlog, Planned, In progress, Paused, Completed, Canceled**, with archive kept apart. *(Recommended: PR 7's project page shows them as Linear does.)*
   - B. **The spec's: Active, Paused, Done, Archived.**

## Decided (2026-11-28)

All A: 1A a key per workspace from its name, editable (`THE-12`), `TASK-12` without a workspace; 2A Backlog,
Todo, In progress, Done, Canceled; 3A Linear's five priorities, new tasks at No priority; 4A shared coloured
labels, today's tags become labels; 5A checklists kept, real sub-issues added; 6A Linear's six project
statuses, archive apart.

## As built

- **One model, a new home.** `CalendarData` keeps its rules (every mutation, preview and test of today), and
  `CalendarStore` stores its tasks in `shodh.db` (`task_db.rs`) when the app gives it the database; events stay
  in `calendar_data.json`. A save diffs the tasks before and after the change and writes, in one transaction,
  the rows that changed and one `task_change` per field with its actor. Without the database (it cannot be
  opened) tasks stay in the file as today, and the next launch with it imports them.
- **Wire words kept.** Todo is stored and sent as `pending` and Done as `completed`, as old clients read them;
  Backlog and Canceled are the new `backlog` and `canceled`. Priorities add `none` and `urgent` to
  `low`/`medium`/`high`. `TodoItem` gains `key`, `projectId`, `parentId` and `owner`, absent when unset.
- **A project name without a workspace** is kept as the task's project name (`project_name`), not turned into
  a label: a project entity needs a workspace, and the name keeps its meaning until the task has one.
- **Issue keys** live in `task_issue_key` (workspace -> key); a task keeps the key it was numbered under, so
  its id never changes when the key is edited.
