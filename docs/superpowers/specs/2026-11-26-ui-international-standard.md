# Desktop UI to an international standard (2026-11-26)

The goal: the desktop app behaves like a product sold in every market. "International standard" is
defined here as six measurable standards, each with a gate that fails CI, not as adjectives.

**How this audit was made.** By reading the code at `5a2efc9f` (232 `.tsx` files, `app/src/index.css`,
`tailwind.config.js`, the 172 Node tests in `app/tests`). Counts below are from `grep` over `app/src`. Nothing
here was measured in the running app; the geometry in the 2026-11-23 audit was, and is reused. Where a number
needs the running app (paint time, input latency) the spec says "baseline first" and sets no number yet.

## 1. The six standards, and where the app stands

| # | Standard | Today | Gap |
|---|---|---|---|
| 1 | **WCAG 2.2 AA**: text contrast 4.5:1, focus visible (2.4.7, 2.4.11), keyboard path to every action (2.1.1), targets 24×24 (2.5.8) | Contrast and the focus ring are tested per theme (`tests/contrast.test.ts`). 124 files style `focus-visible`. | 35 of 185 `outline-none` uses have no focus style on the same element. `h-5` (20 px) is used 36 times; where it is on a button, that is under the 24 px target. No test walks a keyboard path. Workspaces and Connectors title with `h2` (finding A). |
| 2 | **One type scale, one spacing scale** | Four named sizes (12, 13, 14, 15 px) in Tailwind; colours are tokens. | 31 off-scale sizes in use (16, 17, 18, 19, 20, 24, 28 px) plus Tailwind's `text-xs/sm/lg/xl/2xl` (81 uses) beside the named ones. 67 hex colours written in `.tsx`. No page frame: 5 title left edges, 2 title tops (finding A). |
| 3 | **Every state on every main view**: empty, loading, error, offline | Ask has all four. `ViewSkeleton` and an error boundary wrap lazy views. | No shared pattern: each view writes its own empty and error text. "Offline" (no model reachable, computer asleep for the phone) is shown only in Ask and the sidebar. |
| 4 | **i18n-ready**: strings external, layout survives +30% text, RTL-safe CSS, locale-aware dates and numbers | 30 date/number calls already pass `undefined` (the user's locale). `<html lang="en">`. | No message catalogue at all: every string is in JSX. 17 `toLocaleString('en-US')` and 2 `Intl.DateTimeFormat('en-CA')` pin a locale. 98 files use physical `left/right` utilities (`ml-`, `pl-`, `text-left`); 0 use logical ones (`ms-`, `ps-`). Buttons wrap at English length already (finding H). |
| 5 | **Performance budgets**: first paint, input latency, long lists virtualised | The chat log is virtualised (`@tanstack/react-virtual`). Views are lazy and preloaded. `app/perf` measures a long chat. | Only the chat is virtualised: Library files, Activity events, Tasks, Inbox and Artifacts render every row. No budget is a gate. |
| 6 | **Motion respects reduced-motion** | Duration tokens go to 0 ms under `prefers-reduced-motion`; loops are switched off (`index.css`); every `framer-motion` animation is under one `MotionConfig reducedMotion="user"` (`main.tsx`). | Not tested. 20 of 112 Tailwind loop animations (`animate-spin`, `animate-pulse`) had no `motion-reduce:animate-none`. |

## 2. What the benchmarks do that Shodh does not

Taken as patterns to copy, from using those products; not measured for this spec.

- **Linear**: one page frame on every view; every action reachable from the keyboard and listed in one
  command menu; lists never reflow while loading.
- **Notion**: a text column of about 700–900 px with wide blocks (tables, embeds) breaking out of it.
- **Raycast**: one obvious action per state, its shortcut shown beside it; errors carry their fix.
- **Antigravity**: an agent run shown as a plan, live progress against that plan, and a summary of what
  changed, in one place.

## 3. Unfinished from the 2026-11-23 audit

Checked against the code at `5a2efc9f`.

| Finding | Status | Evidence |
|---|---|---|
| A. No shared page frame | open | no `PageFrame`; Workspaces and Connectors still title with `h2` |
| B. Top bar moves between views | done | the top bar already ends with Sources then Inbox on every view, with Ask's own actions in a slot to their left (`App-SplitView.tsx`, "UI audit rule 2"); found while implementing, so no `TopBar` component is added |
| C. Ask message box jumps | open | the empty chat still centres its column (`AskView.tsx`, `min-h-full flex items-center justify-center`) |
| D. Floating pill covers controls | open | no reserved gutter in any view |
| E. Two sets of controls | partly | the floating chat has a model control, but it only opens Ask (see 4.1) |
| F. Model status shown twice, disagreeing | open | sidebar pill: green/amber/red/grey dot; composer chip: one accent dot |
| G. Known failure shown only after sending | open | verified: `sendBlock` knows nothing of the model's state, so a rejected key is said only after a send |
| H. Labels wrap in buttons | open | no shared no-wrap rule |
| I. Too much text | open | copy pass not done |
| J. Review panel covers the chat | fixed in 2a | The pane was already docked; its question box, model line and result states are fixed (section 8) |
| K. Ask says it made an artifact | open | no `make_artifact` tool anywhere in the code |

## 4. What gets built

### 4.1 Three composer gaps (found after #208)

1. **Change the model in the floating chat over the preview.** Today its model control says "change in
   Ask" and leaves the preview. It gets the same menu as Ask: main sends its model list in the dock
   snapshot, and the choice comes back as a dock action that main checks before applying.
2. **One model state, one colour.** The sidebar pill and the composer chip read one status and map it
   through one function: ready, degraded (fallback in use), failed, none. Same dot, same word. (Rule 6.)
3. **The HTML preview uses the wide width.** Today it is the chat column's width (700 px) and 360 px
   tall. It becomes a wide block (4.3): up to 1100 px, and 60% of the window's height (never under 360 px
   or over 720 px), still resizable. Its frame runs no scripts, so the page cannot report its own height.

### 4.2 Foundations

- **`PageFrame`** for every view but Ask: one left edge, one max width, `h1` at one top, a reserved
  primary-action slot at the title row's right, an optional toolbar row, a 56 px bottom gutter for the
  floating pill. (Rules 1, 4.)
- **The top bar** already has its fixed right group and reserved slot (finding B is done); nothing is built.
- **Type and spacing tokens.** The scale becomes 12, 13, 14, 15, 16, 17, 20, 28 px, each named. (16 px was
  added while implementing: it is the size answers are read in, and dropping it to 15 would have shrunk
  every answer.) Off-scale sizes and Tailwind's unnamed ones are replaced. Spacing is Tailwind's scale plus
  two named measures. Hex colours and Tailwind's palette classes leave the components; other products'
  logos keep their own colours.
- **`ViewState`**: one component for empty, loading, error and offline, with one line of text and one
  action each. Every main view uses it.
- **Buttons never wrap** (rule 7); minimum target 24×24, 28 px for icon buttons.
- **The Ask message box sits at the bottom from the first message** (rule 3).
- **A known model failure shows before sending** (rule 6): one line above the message box, with the fix
  and "Change model". Send stays enabled.

### 4.3 The chat layout

- **Text column 800 px** (from 700). **Wide blocks up to 1100 px**: tables, code, the HTML preview,
  charts and diagrams break out of the column, centred on it, and shrink with the window.
- **Question dots rail** at the conversation's right edge: one dot per question; hover shows the
  question's first line; click or Enter jumps to it; the dot of the question in view is filled. It is a
  `nav` with a label, reachable by Tab, and is hidden when there are fewer than three questions.
- **Agent run view**: a run shows **Plan** (the steps it intends), **Progress** (each step's state, live,
  against the plan) and **Summary** (what it changed, with links), in one panel that replaces the three
  separate pieces today (`PlanPanel`, the step lines, `workSummary`). It builds on #208's folded steps
  and changes no agent behaviour.

### 4.4 i18n readiness

- **A message catalogue** (`app/src/i18n/en.ts`, typed keys, a `t()` with plural and number/date helpers
  built on `Intl`). No new dependency.
- **Locale-pinned calls removed**: dates, times and numbers go through the helpers. `en-CA` used as an
  ISO-date trick is replaced by an explicit ISO formatter.
- **Logical CSS**: `ml/pl/left/text-left` become `ms/ps/start/text-start` across `app/src`.
- **Strings move to the catalogue for the shell, Ask and Settings in this work.** The other views follow
  view by view. No second language ships; the gate (below) is what makes the app ready for one.

## 5. Gates (each fails CI)

All run in the existing Frontend job (`node --test`), which has no DOM, so they read sources and pure
functions, as `contrast.test.ts` and `mobile/tests/rules.test.ts` do.

- **Scale**: no font size, spacing or colour outside the tokens in `app/src`.
- **Focus**: no `outline-none` without a focus style on the same element.
- **Targets**: no interactive element under 24 px.
- **Logical CSS**: no physical `left/right` utility in `app/src`.
- **Locale**: no `toLocale*String` or `Intl.*` call with a literal locale outside the helpers.
- **Strings**: in the migrated folders, no JSX text or `aria-label`/`title` literal outside the catalogue.
- **+30% text**: the catalogue is run through a pseudo-locale (every string 30% longer) and each button
  and chip label is checked against its container's width class. A test, not a shipped language.
- **Frame geometry**: every view renders inside `PageFrame`; the title's left and top are constants.
- **Screenshots**: the Windows launch test already drives the real app and uploads one screenshot. It is
  extended to open every view and upload one screenshot of each, in dark and light (18 pictures). They are
  for people to review; no check reads them. That the pill covers no control is held by the frame's
  gutter, which the frame test ties to the pill's own height and position.
- **Performance**: a baseline is recorded first with `app/perf`. Budgets (first paint, keystroke-to-paint in
  the composer, scroll of a 5,000-row Library) are set from it in a later PR. No number is invented here.

## 6. How it is split

Each PR stands alone and leaves the app working.

| PR | Contents | Why separate |
|---|---|---|
| **2a** | The three composer gaps (4.1); `PageFrame`, `ViewState`; the Ask box at the bottom; the model notice before a send; buttons never wrap; the scale, spacing, colour, focus, target, motion and frame gates; per-view screenshots in CI | The user-visible fixes first, with the gates that keep them |
| **2b** | Chat column 800 / wide blocks 1100; the question dots rail; virtualised Library, Activity, Tasks, Inbox and Artifacts lists | One layout change, reviewed on screenshots |
| **2c** | The agent run view: plan, progress, summary | Touches the transcript only |
| **2d** | i18n: catalogue, helpers, logical CSS, locale gate, strings for the shell, Ask and Settings, the +30% test | Mechanical and wide; kept out of the layout diffs |

## 7. Decisions for the user to overrule

1. **No second language ships.** The work makes the app ready for one and proves it with a pseudo-locale.
2. **Strings are externalised for the shell, Ask and Settings only** in 2d. Doing all 232 files in one PR
   would be unreviewable; the gate grows folder by folder.
3. **Finding K (`make_artifact` in every mode) is not in PR 2.** It is a new agent tool with prompt and
   backend work, not a layout change. It should be its own PR.
4. **Finding I (the copy pass) is done only where a view moves into `PageFrame`**: each section's opening
   paragraph becomes one line. A full copy pass is separate.
5. **Performance budgets get no numbers yet.** They are set from a recorded baseline, so they are real.
6. **The text column goes to 800 px for every conversation**, not as a setting.
7. **The dots rail hides below three questions** and has no setting.
8. **RTL is made safe (logical CSS) but not tested in a right-to-left locale.**

## 8. What 2a leaves, said plainly

Closed in 2a, after first being left out:

- **Activity's exports are the page's action**, in the title row's slot. `AuditView` hands its exports to
  whoever frames it (`frame(action, body)`), so the log's filter state stays where it was.
- **`ViewState` draws loading, empty and failed in Library (and a folder's files), Tasks (list, calendar and a workspace's board) and Activity** as well as
  Artifacts, Workspaces and Automations, with one "Try again" button drawn by the state itself. The stale
  notice (`components/ui/StaleNotice`) stays a separate, shared component: it sits above data that is still
  shown, where `ViewState` stands in place of data.
- **Finding J was re-checked and its open parts fixed.** The pane was already docked (the shell gives it its
  width, so it covers nothing). Still open, now fixed: the question box showed only for "Your question"; a
  subscription model, which cannot take a review, was named as where the code would go and the review then
  failed with a line of text; and the result area was blank until a review came back. Now the app names the
  model a review would really reach (`code_review_target` reads the call's plan), "Review this" is off with
  "No model can review code" and "Switch model" when there is none, and the result area says no model,
  working, failed or none yet.
- **A keyboard-path test** (`app/tests/keyboardPath.test.ts`): the order Tab takes through the shell, Ask,
  the message box, the model menu and Settings, focus shown at every stop on it, nothing clickable by mouse
  only, and the menus' arrow keys walked through one shared function. Writing it found the focus gate
  accepting `focus:outline-none` as a focus style; the gate is now strict, and the nine stops it had let
  through (tab panels, two boxless task fields, an editor) draw a ring or a line.

Still true:

- **The keyboard path is read from the sources, not walked in a rendered page.** The app's tests run in Node
  without a DOM. The test holds the order elements are written in and the styles they carry; it cannot see a
  focus ring that is drawn but clipped or covered.
- **The first look at the 18 CI pictures found text cut off that every gate had passed**: the Automations
  title and footer, the Tasks add row, the sidebar's search chip and a chat list squeezed to one row. The
  gates read class names; none measured a rendered page. Now one does. In the Windows launch job's views
  tour the window measures every text it draws, in each view and theme (`features/launch/textFit.ts`): text
  wider than its box, text taller than a box that hides the rest, and a field's placeholder or value wider
  than the field. One hit fails the job, with every hit in the log. Nothing is passed over by name.
  What it covers is what the tour shows: nine views with the launch profile's data, at the probe's window
  size. A long name a user types can still be shortened with an ellipsis where a row gives it a title.
- **The fixes it forced are in the layout, not in widths**: the page title never shortens and the action slot
  wraps under it; Automations' model choice is in the toolbar row and its side pane opens only when it holds
  something; the Tasks add row wraps and the page has one scroller; the sidebar's views and chats share a
  scroller when the window is short, so the chats keep room for a few rows.

## 9. What 2b leaves, said plainly

Done in 2b:

- **The 800 px column and 1100 px wide blocks** in Ask and a connector's conversation. An answer's code, tables,
  charts and diagrams are wide; math, derivations and symbol tables read with the text. Questions, notices and
  side threads keep their blocks in their own box (`MessageContentRenderer`'s `wide`, set only on answers).
- **The question dots rail**, in the 28 px gutter wide blocks leave, below the find box and above the message
  box. One Tab stop, the menus' arrows. Hidden below three questions.
- **The docked plan only where it covers nothing**, the widest block included (`planDocks`). Below that it stays
  in its answer. 2c replaces it with the run view.
- **A short window (800 px tall or less) tightens the sidebar**: 28 px view rows, a chat row's time hidden (it
  stays in the row's label). The launch tour checks that every view shows at least three chats whole.
- **Windowed lists**: Library's files (the 500-row cap is gone), Tasks, the Inbox, the Artifacts creations and
  the visuals gallery (up to 500 drawn thumbnails before). Activity's event table was already windowed, so the
  audit's claim about it was wrong.

Still true:

- **The runner's screen is 1024×768.** The `conversation` scene folds the sidebar, so the column is 800 px and
  its wide blocks are about 880 px. No CI picture shows a full 1100 px block.
- **Library's folder cards and the Workspaces list are not windowed.** Neither was named in the audit; each
  holds what a user adds by hand (folders, workspaces), tens of rows.
- **The windowed lists are checked by reading their sources and by the launch tour's pictures**; the launch
  profile has few rows, so no CI run scrolls a list of thousands.
