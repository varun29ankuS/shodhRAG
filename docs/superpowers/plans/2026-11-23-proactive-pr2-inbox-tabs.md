# Proactive assistant, PR 2 (Attention/Info Inbox) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Inbox panel splits into two tabs, **Attention** and **Info**. The bell's badge counts Attention only. A "Why is this here?" pop-up states the fixed rules, and the bell stays at one x on every view.

**Architecture:**
- This is a view over the existing store (spec §11). There are no new commands, no migration and no Rust change.
- **Classification is one pure function, `tabOf(item)`, in `app/src/features/inbox/model.ts`.**
  - It keys an exhaustive `Record<InboxKind, …>`, so tsc refuses PR 3's `whats_new` and PR 8's `review` until each one has a tab.
- `InboxButton.tsx` renders the tabs, counts, badge, empty, loading and error states from pure helpers in the same model file. `node --experimental-strip-types` tests them directly.
- `App-SplitView.tsx` reorders the top bar's right group, so the Inbox is the last control.

**Tech stack:**
- React and TypeScript.
- Node strip-types tests (`app/tests/inbox.test.ts`), run by CI "Frontend Typecheck + Tests".

**Spec:** `docs/superpowers/specs/2026-11-22-proactive-assistant-design.md`. The parts that apply:
- §11 (the Inbox);
- the §13 row "2. Attention/Info Inbox";
- §15 decision 3 (the tabs are named "Attention" and "Info");
- §5.4 (the automation yield item that PR 4 will post).

The UI rules come from `docs/superpowers/specs/2026-11-23-ui-structure-audit.md` (on master, #200):
- rule 2: one top bar, with Sources then Inbox at a fixed x;
- rule 7: buttons never wrap;
- the memory rule "UI: sturdy, unambiguous".

This branch, `feat/proactive-inbox-tabs`, is cut from `origin/feat/proactive-engine` (PR #199, PR 1). PR 1 already gives the Inbox:
- the `automation` kind (Rust and TS);
- `actionsFor` with `choices`;
- the repeat-decision item (`repeat:<job>`, kind `reminder`, `needs_you`, `data.choices`);
- the hold item (`repeat-hold:<job>`, kind `automation`, `needs_you`, `data.fix = 'resume'`).

## Global Constraints

- **No local builds.** Don't run `npm install`, `npm test`, cargo or the app. CI "Frontend Typecheck + Tests" is the verifier for every task.
- **Code quality:**
  - Production code only: no TODO, stub, mock or placeholder.
  - Never weaken a test. Existing assertions in `inbox.test.ts` keep their expected values. A renamed function changes only the call (see Task 1).
- **Commits:**
  - Plain prose subjects, like the repo history.
  - **No footers:** no Co-Authored-By, no "Generated with".
- **No new commands.** `inbox_list` and `inbox_dismiss` keep their `agent-coverage.json` exclusions unchanged.
- **Copy:**
  - "Signal" and "Noise" appear nowhere in UI text, `aria-label`s or titles. They may appear in code comments only as a pointer to the spec.
  - The tab labels are exactly `Attention` and `Info`.
  - The two empty-state lines are taken verbatim from §11:
    - "Nothing needs your attention. Shodh puts approvals, reminders and suggestions here."
    - "Nothing new. Finished work and updates show here."
- **Keys stay as today** (§11): J/K or Up/Down move, Enter opens, A, D and U act.
  - The tab list adds only the standard ARIA tablist keys (Left and Right while a tab has focus). It adds no panel-wide key.
- **Buttons never wrap:**
  - every action button carries `whitespace-nowrap`;
  - the action row is `flex-nowrap`.

## Review Focus

1. **An item stopped at close must never add to Attention.** On every launch, `InboxStore::recover` turns `working` into `failed`. Any rule that makes a recovered `indexing`, `tables`, `citation_graph`, `install` or `export` item Attention would raise the badge on every restart.
   - Test `an_item_stopped_at_close_never_adds_to_attention` (Task 1).
2. **A folder with one unreadable file must not live in Attention forever.** A watcher sync posts `failed` whenever any file can't be read.
   - Only a sync that failed outright (`data.failure === 'sync'`) is Attention.
   - Test `a_folder_that_could_not_sync_is_attention_and_a_partial_sync_is_info` (Task 1).
3. **A click or key inside the "Why is this here?" pop-up must neither close the Inbox nor act on an item.**
   - `InfoPopover` portals to `document.body`. The Inbox's outside-mousedown closes on any target outside `rootRef`.
   - React bubbles the pop-up's keydown into the panel's `onKeyDown`, so pressing A inside it would approve the selected item.
   - Both handlers must return early when `insideOtherDialog(target, rootRef.current)`.
   - Test `the_why_popover_neither_closes_nor_drives_the_inbox` (Task 2).
4. **A slow first load must not claim the Inbox is empty.** Today the panel shows "Nothing is waiting for you." before `inbox_list` returns.
   - `panelState` returns `loading` until the first answer arrives.
   - Test `loading_is_never_shown_as_empty` (Task 1).
5. **The bell must not move between views.** Today the right group is Inbox, More, Instructions, Sources, so the bell sits 126 px further left on Ask.
   - The test `the_inbox_is_the_last_control_of_the_top_bar` (Task 3) is a source-order check.
   - Real geometry is checked by screenshot on the user's build (see open question Q6). CI has no desktop geometry probe.

---

## Classification (the rules, fixed)

| Status | Tab | Why |
|---|---|---|
| `needs_you`, any kind | **Attention** | It waits for the user's answer: approvals, memory suggestions, rung reminders, repeat decisions, holds, install offers, a connector that wants a decision (a Telegram group, a GitHub sign-in). |
| `failed`, kind `model` | **Attention** | "*Model* has been failing for over an hour": chat keeps failing until the user changes the model. |
| `failed`, kind `connector` | **Attention** | Slack or Telegram lost, stopped, short of permissions or at its cap; a GitHub push or limit. Something the user set up has stopped (see D3 and Q1). |
| `failed`, kind `indexing`, with `data.failure === 'sync'` | **Attention** | The folder could not be synced at all, so the Library stays out of date. |
| `failed`, any other case | **Info** | A one-off that ended: a partial sync, a first index that failed, export, tables, citation graph, install, a single failed automation run (three in a row become a `needs_you` hold), and anything `recover` stopped at close. |
| `working`, `done` | **Info** | Progress and finished work. This includes the automation yield (D4). |

## File Structure

| File | Responsibility |
|---|---|
| `app/src/features/inbox/model.ts` (modify, Task 1) | `InboxTab`, `tabOf`, `tabItems`, `tabCounts`, `attentionCount` (replaces `waitingCount`), `countLabel`, `panelState`, `EMPTY_COPY`, `TAB_RULES`, `AUTOMATION_YIELD_ID`. |
| `app/tests/inbox.test.ts` (modify, Tasks 1–3) | Classification table, counts, states, source checks. |
| `app/src/features/inbox/InboxButton.tsx` (modify, Task 2) | Tabs, badge, the "Why is this here?" pop-up, loading, error and empty states, the non-wrapping action row, guards for the inner dialog. |
| `app/src/App-SplitView.tsx` (modify, Task 3, around lines 1613–1833) | The top bar's right group: Ask-only actions, then Sources, then Inbox. |

---

### Task 1: Classification, counts and panel states (pure model)

**Files:**
- Modify: `app/src/features/inbox/model.ts`
- Test: `app/tests/inbox.test.ts`

**Interfaces (produces):**
```ts
export type InboxTab = 'attention' | 'info';
export const INBOX_TABS: readonly InboxTab[];   // ['attention', 'info'], the display order
export const TAB_LABEL: Record<InboxTab, string>; // { attention: 'Attention', info: 'Info' }
export function tabOf(item: Pick<InboxItem, 'kind' | 'status' | 'data'>): InboxTab;
export function tabItems(items: readonly InboxItem[], tab: InboxTab): InboxItem[]; // keeps the input order
export function tabCounts(items: readonly InboxItem[]): Record<InboxTab, number>;
export function attentionCount(items: readonly InboxItem[]): number; // the bell; replaces waitingCount
export function countLabel(n: number, max: number): string;          // '0'..String(max), else `${max}+`
export type PanelState = 'loading' | 'error' | 'empty' | 'list';
export function panelState(input: { loaded: boolean; error: string | null; count: number }): PanelState;
export interface EmptyCopy { title: string; detail: string }
export const EMPTY_COPY: Record<InboxTab, EmptyCopy>;
export const TAB_RULES: Record<InboxTab, string>;  // the text of "Why is this here?"
export const AUTOMATION_YIELD_ID = 'automation:yield'; // D4: the item PR 4 posts and updates
```

- `tabOf` is implemented as:
  - `status === 'needs_you'` gives `attention`;
  - `status === 'failed'` gives `FAILED_STOPS[item.kind](item.data)`, where `FAILED_STOPS: Record<InboxKind, (data: Record<string, unknown>) => boolean>` is true for `model` and `connector`, true for `indexing` only when `data.failure === 'sync'`, and false for every other kind;
  - anything else gives `info`.
- `panelState` precedence, in order:
  1. not loaded and no error gives `loading`;
  2. an error with `count === 0` gives `error`;
  3. `count === 0` gives `empty`;
  4. otherwise `list`.

  With an error and items, the panel shows `list` plus the error line (Task 2).
- `EMPTY_COPY`:
  - `attention: { title: 'Nothing needs your attention.', detail: 'Shodh puts approvals, reminders and suggestions here.' }`
  - `info: { title: 'Nothing new.', detail: 'Finished work and updates show here.' }`
- `TAB_RULES`:
  - `attention: 'Attention has what waits for you (approvals, suggestions, reminders, a repeating task’s question, something on hold) and what stays stopped until you fix it: a model that keeps failing, a connector that stopped, a folder that could not be synced. The bell counts these.'`
  - `info: 'Info has everything else: work in progress, finished work, updates, and one-off failures that need nothing more from you.'`

- [ ] **Step 1: Write the failing tests** in `app/tests/inbox.test.ts`. Import the new names. Add the tests below and change the existing tests only where noted.
  - `every kind and status lands in its tab`.
    - An explicit table, written out by hand for all 11 kinds × 4 statuses (44 rows), with `data: {}`.
    - Every `needs_you` row is `'attention'`. `failed` is `'attention'` for `model` and `connector` only. Every `working` and `done` row is `'info'`.
    - Assert `tabOf(item(...)) === expected` for each row. The expectation must be written as literals, never derived from `FAILED_STOPS`.
  - `a folder that could not sync is attention and a partial sync is info`:
    - `{kind:'indexing', status:'failed', data:{failure:'sync'}}` gives `'attention'`;
    - the same with `data:{quiet:false}` gives `'info'`.
  - `a repeating task question and a hold land in attention`. Use the exact shapes from `task_repeat.rs`:
    - `{id:'repeat:j1', kind:'reminder', status:'needs_you', data:{taskId:'t1', jobId:'j1', choices:['make','skip','mark_done'], occurrence:'2026-11-24T17:00:00Z', missed:0}}`;
    - `{id:'repeat-hold:j1', kind:'automation', status:'needs_you', data:{taskId:'t1', jobId:'j1', fix:'resume'}}`;
    - both give `'attention'`.
  - `an automation that yields to chat lands in info`. `{id: AUTOMATION_YIELD_ID, kind:'automation', status:'working', data:{paused:'chat', resumesAt:'2026-11-24T00:00:00Z'}}` gives `'info'`. After `recover` (status `'failed'`, same data) it is still `'info'`.
  - `an item stopped at close never adds to attention`: for `indexing`, `tables`, `citation_graph`, `install`, `export` and `automation` with `status:'failed'` and the data their working item had (`{}`, or `{quiet:true}`), `attentionCount` is 0.
  - `tab counts and the bell agree`:
    - `[approval needs_you, memory needs_you, model failed, export done, tables failed, indexing working]` gives `tabCounts` `{attention:3, info:3}`;
    - `attentionCount` = 3;
    - `tabItems(list,'info')` keeps the input order.
  - **Existing test `the bell counts only what waits on the user`:** change only the call, `waitingCount(…)` to `attentionCount(…)`. Its list and its expected value `2` stay byte-identical: the failed `tables` item is still not counted.
  - `count labels cap without changing width`:
    - `countLabel(0, 99) === '0'`;
    - `countLabel(99, 99) === '99'`;
    - `countLabel(100, 99) === '99+'`;
    - `countLabel(10, 9) === '9+'` (the bell).
  - `loading is never shown as empty`. `panelState` gives:
    - `{loaded:false, error:null, count:0}`: `'loading'`;
    - `{loaded:true, error:null, count:0}`: `'empty'`;
    - `{loaded:true, error:'x', count:0}`: `'error'`;
    - `{loaded:true, error:'x', count:2}`: `'list'`;
    - `{loaded:false, error:'x', count:0}`: `'error'`.
  - `empty states use the spec's words`: `EMPTY_COPY.attention.title + ' ' + EMPTY_COPY.attention.detail` equals the §11 sentence verbatim, and the same for `info`.
  - `no UI text says Signal or Noise`: `TAB_LABEL`, `EMPTY_COPY` and `TAB_RULES` values don't match `/signal|noise/i`.
- [ ] **Step 2: Expected failure** in CI "Frontend Typecheck + Tests": the imports don't exist.
- [ ] **Step 3: Implement** the Interfaces block in `model.ts`.
  - Remove `waitingCount`. Its only other caller is `InboxButton.tsx:106`, which Task 2 changes. In this commit, switch that line to `attentionCount` so tsc stays green.
  - Document `AUTOMATION_YIELD_ID` and the yield's pinned shape (D4) in a doc comment.
- [ ] **Step 4: Run** CI "Frontend Typecheck + Tests". Expected: `inbox.test.ts` passes, tsc is clean.
- [ ] **Step 5: Commit** "Inbox: every item has a tab, Attention or Info, and the bell counts Attention".

### Task 2: The panel. Tabs, badge, "Why is this here?", states, a row that never wraps

**Files:**
- Modify: `app/src/features/inbox/InboxButton.tsx`
- Test: `app/tests/inbox.test.ts` (source checks with the existing `read()` helper)

**Interfaces:**
- Consumes everything Task 1 produces.
- Also consumes `InfoPopover` (`components/ui/InfoPopover.tsx`) and `insideOtherDialog` (`lib/escapeScope.ts`).

**Behaviour:**
- **State:**
  - `tab: InboxTab`, set to `'attention'` every time the panel opens (from the bell or `OPEN_INBOX_EVENT`);
  - `selected: Record<InboxTab, number>`, so each tab keeps its own selection through `keepSelection` on `tabItems(visible, tab)`;
  - `loaded: boolean`, true after the first `inbox_list` answer, success or error.
- **Counts** come from `visible`, after hidden and rejecting items are removed, so a dismissal waiting on Undo leaves both the tab count and the badge at once.
- **Badge:**
  - `attentionCount(visible)`, shown when > 0, with `countLabel(n, 9)`;
  - `aria-label` reads "Inbox, N need your attention" (singular "Inbox, 1 needs your attention").
- **Tabs:**
  - `role="tablist"` with two `role="tab"` buttons in a `grid grid-cols-2`, so each half keeps its width whatever the counts;
  - each label reads `Attention` or `Info`, with a `tabular-nums` count pill (`countLabel(n, 99)`) that shows `0` too;
  - `aria-selected`; Left and Right move between the tabs while a tab has focus;
  - the list is `role="tabpanel"`, labelled by its tab.
- **Focus:** a row takes focus when the panel opens and on J/K moves. Switching tabs leaves focus on the tab.
- **"Why is this here?":**
  - `<InfoPopover label="Why is this here?" title="Why is this here?">` at the right of the title row;
  - its body is the two `TAB_RULES` paragraphs, each under its tab's label.
- **Inner dialog guards:**
  - the outside-mousedown handler returns when `insideOtherDialog(target, rootRef.current)`;
  - the panel's `onKeyDown` returns at once (before Escape handling) when `insideOtherDialog(e.target, rootRef.current)`.
- **States**, per `panelState({loaded, error: loadError, count: tabItems(visible, tab).length})`:
  - `loading`: one muted line, "Loading the Inbox…", with `aria-busy`;
  - `error`: the error text and a **Try again** button that calls `load()`;
  - `list` with an error: a one-line `role="alert"` strip under the tabs ("Couldn't refresh the Inbox: …" + **Try again**), with the last list below it;
  - `empty`: `EMPTY_COPY[tab]`. On Attention, when the Info count is > 0, it adds one button, **Show Info (N)**, which switches the tab.
- **Action row:**
  - `flex flex-nowrap`, and every button carries `whitespace-nowrap`;
  - an item with `choices` shows no Open button, because its row already opens on click or Enter (the three answers need the width);
  - panel width `w-[400px]` (was 380).
- **Footer:** the key hint moves from the header to a footer line, `J/K move · A approve · D deny or dismiss · U undo`, so the title row holds only the title and the pop-up.

- [ ] **Step 1: Write the failing source checks** in `inbox.test.ts`, reading `features/inbox/InboxButton.tsx`:
  - `the panel has two tabs and opens on attention`: matches `role="tablist"`, `role="tab"` and `role="tabpanel"`. Matches `setTab('attention')` in both the bell's open path and the `OPEN_INBOX_EVENT` handler (two occurrences).
  - `the badge counts attention`: matches `attentionCount(` and not `waitingCount`; the aria-label contains `need your attention`.
  - `the inbox action row never wraps`: no `flex-wrap` in the file; every `className={cn(ACTION` button's `ACTION` constant contains `whitespace-nowrap`.
  - `the why popover neither closes nor drives the inbox`: `insideOtherDialog` appears at least twice; `label="Why is this here?"` is present.
  - `the empty state is never shown before the first load`: matches `panelState(` and `'loading'`; the old line "Nothing is waiting for you." is gone.
- [ ] **Step 2: Expected failure** in CI "Frontend Typecheck + Tests".
- [ ] **Step 3: Implement** the behaviour above in `InboxButton.tsx`. Reuse the existing `FOCUS_RING`, `ACTION` and `STATUS_TONE`. No new component file is needed.
- [ ] **Step 4: Run** CI "Frontend Typecheck + Tests". Expected: pass.
- [ ] **Step 5: Commit** "Inbox: Attention and Info tabs, a badge for Attention, and why each item is where it is".

### Task 3: The bell stays at one x

**Files:**
- Modify: `app/src/App-SplitView.tsx` (the right group, `<div className="flex items-center gap-2">` at about line 1612)
- Test: `app/tests/inbox.test.ts`

The new order, left to right:
1. `ChatActionsMenu` (Ask only);
2. Instructions (Ask only);
3. the Sources `Badge` (`librarySourcesChip`);
4. `InboxButton`, last.

The Inbox panel and the Instructions pop-up both anchor `right-0` to their own trigger, so neither changes.

- [ ] **Step 1: Write the failing test** `the inbox is the last control of the top bar`.
  - Read `App-SplitView.tsx`.
  - Assert `src.indexOf('<InboxButton') > src.indexOf('librarySourcesChip(searched)')`.
  - Assert `src.indexOf('librarySourcesChip(searched)') > src.indexOf('<ChatActionsMenu')`.
  - Between `<InboxButton` and the closing `</div>` of that group, no other element tag appears: the slice up to the first `</div>` after the closing `/>` contains no `<` other than that `</div>`.
- [ ] **Step 2: Expected failure** in CI "Frontend Typecheck + Tests".
- [ ] **Step 3: Move** the `InboxButton` element to the end of the group. Change nothing else.
- [ ] **Step 4: Run** CI "Frontend Typecheck + Tests". Expected: pass.
- [ ] **Step 5: Commit** "Top bar: the Inbox is always the last control, so it stays put between views".

---

## UX

**Where the badge sits.**
- The bell is the right-most control of the top bar on every view, 20 px (`px-5`) from the window's right edge.
- Sources sits just left of it, and Ask's More and Instructions sit left of Sources. Nothing to the bell's right can change width, so its x is fixed.
- The badge is the existing amber pill on the bell's top-right corner (`-top-1.5 -right-1.5`, 18 px):
  - it counts **Attention** only, `1`–`9`, then `9+`;
  - it is hidden at 0.
- The panel drops from the bell, right-aligned, 400 px wide.

**Empty (nothing at all):**
```
 Inbox                                          (i)
 [ Attention  0 ][ Info  0 ]
        Nothing needs your attention.
   Shodh puts approvals, reminders and suggestions here.
 J/K move · A approve · D deny or dismiss · U undo
```

**Attention only:**
```
 Inbox                                          (i)
 [ Attention  2 ][ Info  0 ]
 • Submit timesheet                                      Needs you · ... · 2h
     [Make the next one] [Skip this one] [Mark done]
 • Write notes.md                                        Needs you · ... · 5m
     [✓ Approve] [✕ Deny] [↗ Open]
 J/K move · A approve · D deny or dismiss · U undo
```
- Info shows "Nothing new. Finished work and updates show here."

**Both, Attention empty:** the panel still opens on Attention, with one action.
```
 [ Attention  0 ][ Info  12 ]
        Nothing needs your attention.
   Shodh puts approvals, reminders and suggestions here.
              [Show Info (12)]
```

**Both, with items:** Attention opens. Rows keep the store's order: needs-you first, then failures that stop something, newest first.

**Many items:**
- The list scrolls inside `max-h-[420px]`. The title row, the tabs and the footer stay fixed.
- Counts above 99 read `99+` in a fixed-width pill, so the tab halves never move.
- J/K stay within the open tab, and each tab remembers its selection.

**Loading** (first open before `inbox_list` answers): "Loading the Inbox…", never the empty line.

**Error:**
- Before anything loaded: the error and **[Try again]**.
- After a list loaded: a one-line alert strip under the tabs, "Couldn't refresh the Inbox: … [Try again]", with the last list below it. This holds in both tabs.

**"Why is this here?" (i):**
```
 Why is this here?                                 ✕
 Attention
   Attention has what waits for you (...) The bell counts these.
 Info
   Info has everything else: ...
```

---

## Departures from the spec

- **D1. One "Why is this here?" pop-up per panel, not per row.**
  - The rules are fixed and global (§11: "The rules are fixed and shown in a 'Why is this here?' popover").
  - A per-row pop-up would add a control to every row and push the action row toward wrapping (audit rule 7).
  - The pop-up sits in the title row, where it never moves.
- **D2. "Failed items that stop something the user owns" is defined by kind, plus one data key.** The rule:
  - `model` and `connector`: always;
  - `indexing`: only with `data.failure === 'sync'`;
  - nothing else.

  The spec doesn't enumerate the kinds. This rule keeps a restart (`recover`) and a folder with one unreadable file out of Attention (Review Focus 1–2). A first index that failed outright ("X was not indexed") carries no `failure` key, so it is Info (Q2).
- **D3. Every failed `connector` item is Attention**, including three one-offs: "A Slack answer failed", a rejected GitHub push and a GitHub rate limit.
  - Telling them apart from the client would mean matching id prefixes.
  - Doing it properly needs a producer-side flag in `slack_notices.rs`/`github_notices.rs`. That is a Rust change this view-only PR avoids (Q1).
- **D4. The automation yield's shape is pinned here, ahead of PR 4:**
  - id `automation:yield`, kind `automation`, status `working`;
  - `data: {paused: 'chat', resumesAt}`;
  - updated in place, never repeated.

  It lands in Info. `recover` turns it into `failed` "Stopped when Shodh closed…", which is still Info. PR 4 must re-post or resolve it on launch so the label is right. `done` was rejected because it would render a green "Done" for something paused.
- **D5. The top bar's right group is reordered** (Task 3). §11 doesn't ask for this. Audit rule 2 does, and the brief requires the bell at a fixed x. Only the order changes; the full top-bar and page-frame work of the audit stays its own PR.
- **D6. The Open button is dropped on items with one-click answers**, and the panel widens from 380 to 400 px. Three answers plus Open don't fit on one line (audit rule 7). The row itself still opens on click or Enter.
- **D7. A loading state and an error strip are added** in place of today's false empty state and the error that replaced the list ("every state visible").
- **D8. The key hint moves from the header to a footer**, so the title row holds the title and the pop-up only. The keys don't change.
- **D9. The panel always opens on Attention**, even when Attention is empty and Info is not. The badge counts Attention, so a click on the badge shows what it counted. Info is one obvious click away (**Show Info (N)**).

## Open questions for the user

- **Q1.** Should the three one-off connector failures move to Info? These are "A Slack answer failed", a rejected GitHub push and a GitHub rate limit. Doing it right means a small Rust change: a `data.stops: true` flag on the notices that do stop something.
- **Q2.** Should "*Folder* was not indexed" (a first index that failed outright) be Attention? It would need `enhanced_rag_commands.rs::finished_item` to add `failure: "index"`, a one-line Rust change.
- **Q3.** The Info count is the total kept (up to 30 days of finished work), so it can read "Info (47)" for weeks.
  - Is that acceptable for now?
  - The alternative is an unseen count, which needs a `seen_at` column (a migration) and a mark-seen command. The spec rules out new commands for this PR.
- **Q4.** Memory suggestions are capped at 50 in the Inbox (`MAX_MEMORY_ITEMS`). Past that, the Attention count understates. Should it show "50+" when the cap is hit (backend change), or stay as is?
- **Q5.** Should there be a "Clear finished" action on the Info tab? It would loop `inbox_dismiss`, so no new command. It is not in the spec, and it is left out under minimal-first.
- **Q6.** CI has no desktop geometry probe. Can you check the bell's x on Ask versus Library, and the three-answer row on one line, on your build? If not, should a launch-test hit-test probe be added in a later PR?
