# Proactive PR 7: Linear-like project and issue pages (plan, 2026-11-28)

Spec: `docs/superpowers/specs/2026-11-22-proactive-assistant-design.md` §9.2 and §13 row 7.
Builds on PR 6 (`docs/superpowers/plans/2026-11-28-proactive-pr6-tasks-database.md`), which stores what this PR
shows: issue keys, the five statuses, Linear's priorities, owners, shared labels, sub-issues (`parent_id`),
links (`task_link`), projects (`task_project`: Linear's six statuses, lead, target date, archive) and every
change with its actor (`task_change`). This PR adds views, keys, commands and the assistant's tools. It adds
no table.

The bar is Linear: one fast list, one board, one issue page, everything editable in place and by key.

## What PR 7 builds

### 1. Where it lives

The Tasks view becomes the home of issues and projects. It keeps its place in the nav, as the approved
navigation says (Ask, Workspaces, Library, Tasks, Automations, Activity, Settings); its label is decision 1.
Inside it:

- **The left rail:**
  - My issues, Inbox (assigned to Shodh, waiting on you), All issues, Projects;
  - then each workspace, with its projects under it.
- **Calendar** stays as a tab of the view, beside List and Board.
- **A workspace's page** keeps its tasks section, which opens the same list filtered to that workspace.

### 2. Projects

- **The Projects list** is a table:
  - name, status (Linear's six), lead, target date;
  - progress (done of total, a bar), and at-risk count;
  - grouped by workspace and sorted by target date.
  - `N` makes a project.
- **The project page shows:**
  - a header with name, status, lead (me, Shodh, a person), target date and description, all editable in place;
  - progress, counted from its issues: done and canceled out of all, never stored;
  - its issues in List or Board, with the same grouping and filters as the issue views;
  - its History: changes of the project and of its issues, newest first.
- **At risk** is deterministic (spec §9.2). A project is at risk when it has a target date and an open issue is due after it, or is itself at risk. The reason is shown.
- **Archive and delete.**
  - Archive hides a project and keeps its issues grouped.
  - Delete keeps the issues, which move to "No project". The confirmation says how many.

### 3. Issue list and board

- **List:** rows of key, title, status, priority, assignee, labels, project and due date. Grouping is by status (the default), priority, assignee, project or label, with collapsible group headers and counts. The list is virtualised, so a workspace of thousands scrolls at 60 fps.
- **Board:** columns by status (the default), or by priority, assignee or project. Cards are dragged between columns; `Alt+Arrow` moves a card by keyboard (as today).
- **Filters** are a filter bar of chips: status, priority, assignee, label, project, due (overdue / this week / none), "has sub-issues" and "blocked". A filter shows as a chip, and `/` opens the bar.
- **Saved views** are decision 4.
- **Display options:** grouping, ordering (priority, due, updated, created, manual) and "show sub-issues". They are kept per view and per workspace on this computer.
- **Empty and error states** are as in spec §9.2, in short words.

### 4. The issue page

Opened by `Enter` or a click. It is a full page beside the list, not a sheet: decision 3.

- **Title and description.** The description is Markdown, edited in place, with a preview.
- **The properties column:**
  - status, priority, assignee, labels (with colours), project, due date and reminder;
  - repeat, workspace and the issue key (read-only, copied by a click);
  - each set by a popover with type-to-filter, the same popover the key opens.
- **Sub-issues:** a list with progress, made inline (`Enter` adds the next). Each opens its own page; a parent's done count shows on the row.
- **Relations:**
  - blocks / blocked by, relates to, duplicate of, each picked by typing a key or a title;
  - a cycle is refused with its chain (PR 6's rule);
  - "blocked" shows on rows and on the board.
- **Checklist** stays (PR 6 decision 5).
- **History and provenance:**
  - every change, with who made it: you, Shodh (its conversation opens), an automation, a repeat, or the move;
  - the old and new value, and the reason when one was given;
  - where the issue came from: a chat with its passages, a Slack thread, a document.
- **Ask about it** keeps the side discussion it has today.

### 5. Keyboard first

| Key | What it does |
|---|---|
| `C` | New issue, in the current workspace and project |
| `J` / `K` | Move down / up |
| `Enter` | Open |
| `Esc` | Back to the list |
| `X` | Select (and `Shift+J/K` to extend) |
| `E` | Edit the title in place |
| `S` | Status |
| `P` | Priority |
| `A` | Assignee |
| `L` | Labels |
| `M` | Move to project |
| `D` | Due date |
| `N` | New project |
| `/` | Filter |
| `G` then `I` / `P` / `B` | Go to My issues / Projects / Board |
| `Ctrl+K` | Command palette |

- Every key acts on the selection when there is one.
- Each key has its menu item, and every action is a command palette entry. Examples: "New issue", "Set status…", "Assign to Shodh", "Go to project…", and "Open issue THE-12" (typing a key opens it).
- Keys are shown in tooltips and in the palette, from `lib/shortcuts.ts`.
- The list and the board are ARIA grids, like the evidence table.
- No key conflicts with the composer's: keys work only when focus is in the Tasks view and not in a field.

### 6. The issue key field

A workspace's settings get "Issue key" (`THE`). It validates as `check_key`, shows the next id ("Next: THE-14"), and says that ids already given stay. It uses PR 6's `task_issue_key_get` / `task_issue_key_set`.

### 7. The assistant's tools (spec §9.2)

- **Reading:** `list_projects`; `list_tasks` gains `view: my_work | at_risk`, `project`, `label` and `key`.
- **Writing projects:** `create_project`, `update_project` (Write, local edit) and `delete_project` (Destructive). The preview of a delete counts the issues that lose their project.
- **Writing issues:**
  - `create_task` and `update_task` take `project`, `parent` (a key or an id), `owner`, `labels`, and the new statuses and priorities;
  - `link_tasks` / `unlink_tasks` (Write, local edit);
  - `task_history` (Read);
  - `show_tasks` (Read) opens a view, workspace, project or issue, and highlights it.
- Each tool's previews say what changes ("leaves project *Baselines*", "makes THE-12 a sub-issue of THE-9"). Each change is recorded with the run, as PR 6 does.
- Agent coverage: every new command maps to a tool, or is excluded with its reason.

### 8. The phone

Decision 5 sets how much of this the phone shows. In every option:

- the phone's Tasks screen shows the key, the five statuses and the priorities;
- it reads `/tasks` as today (the wire is PR 6's, compatible);
- an old phone build keeps working.

### 9. Tour shots (both themes, no clipped text at 1000x700)

- Projects list;
- project page (List);
- project page (Board, grouped by status);
- issue list grouped by priority, with a filter chip;
- issue page with sub-issues, relations and History;
- the command palette with issue commands;
- the status popover opened by `S`;
- the workspace's issue key field;
- empty states: "My issues", "At risk";
- the phone's Tasks screen with keys (in the phone app's own screenshot set).

## Gates (tests)

- **Views:** grouping, ordering and filters are pure and tested, including every grouping with sub-issues hidden and shown.
- **Progress and at risk:** tested against a table of cases.
- **Keys:** a table test of every key against its action, focus rules (no key acts while typing), and palette entries mapped to the same actions.
- **Inline edit:** each property's popover writes one patch and records one change. An undo toast reverts it.
- **Performance:** the list of 5,000 issues renders its first screen in under 100 ms in a timed component test (no network), and scrolling stays virtualised.
- **Tools:** each tool's preview text, refusals (cycle, cross-workspace project, unknown key), and coverage.
- **Phone:** golden screens for keys and statuses, and an old-shape payload still renders.
- **Tour:** the shots above, in both themes, with no clipped text and three whole chats.

## Order of work (one PR, small commits)

1. Pure models: grouping, filters, ordering, progress, at risk, keys, and the palette's action table, with tests.
2. Issue list and board on the models (virtualised), with display options.
3. Issue page with inline properties, sub-issues, relations and History.
4. Projects list and project page; the workspace's issue key field.
5. The keyboard map and palette entries.
6. The assistant's tools and coverage.
7. The phone's Tasks screen.
8. Tour shots; spec update.

## Decisions for the user

1. **What is the view called in the nav?**
   - A. **"Tasks"**, as today, with issues and projects inside. *(Recommended: the approved nav says Tasks, and people say "my tasks". Ids like THE-12 work under either name.)*
   - B. **"Issues"**, as Linear says.
   - C. **"Projects"**, opening on the projects list.
2. **What opens first?**
   - A. **My issues:** assigned to you, not done, by due date. *(Recommended: what you open Tasks for most days; Linear opens here too.)*
   - B. **All issues of the current workspace**, grouped by status.
   - C. **The projects list.**
3. **Where does an issue open?**
   - A. **A full page beside the list** (list on the left, issue on the right; `Esc` goes back). *(Recommended: room for sub-issues, relations and History; `J`/`K` still move through the list.)*
   - B. **A side sheet over the list**, as tasks open today.
4. **Saved views (a named set of filters and grouping)?**
   - A. **Not now:** the filters and grouping are remembered per view on this computer. *(Recommended: less to build and to learn; saved views can come with team workspaces.)*
   - B. **Yes, saved views**, listed in the left rail.
5. **How much does the phone show?**
   - A. **Read and quick changes:** the list with keys, statuses and priorities, and an issue's page where status, priority, due date and the checklist can be changed. Projects and relations are read-only. *(Recommended: what you do away from the desk. Planning stays on the computer.)*
   - B. **Read only:** keys, statuses and priorities shown, nothing new to change.
   - C. **The same pages as the desktop**, projects and relations editable.
6. **Who can be an assignee?**
   - A. **You or Shodh**, and a person's name as a label (no one is told). *(Recommended: as PR 6 stores it; real people come with team workspaces.)*
   - B. **You or Shodh only**, with no names until teams exist.

## Decided (2026-11-28)

All A:

1. **Nav:** the item stays "Tasks", with issues and projects inside.
2. **First screen:** My issues opens first.
3. **Issue page:** an issue opens as a full page beside the list.
4. **Saved views:** none for now. Filters and grouping are remembered per view.
5. **Phone:** the list with ids, plus quick edits of status, priority, due date and checklist. Projects and relations are read-only.
6. **Assignee:** you, Shodh, or a name kept as a plain label that notifies no one.

Coding starts once PR 6 (#221) is green; this PR builds on it.
