# Visual explanations, part 1: visual-first answers and "go deeper" marks — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:**
- Answers lead with the visual that fits the question.
- The model can mark up to 4 parts of an answer with a prepared question.
- One click on a mark opens a nested window that asks that question. Nothing is sent before
  the click.

**Architecture:**
- **Marks are data in what the model already writes:**
  - a `deeper` string on diagram nodes, chart series, plot and simulation parameters,
    derivation steps and symbol entries;
  - `[[shown text|question]]` in prose.
- **Shared pure module.** One module (`marks.ts`) reads marks and applies the 4-per-answer
  budget in document order.
- **Rendering.**
  - Diagrams draw a glow on marked nodes and add a "Go deeper" button to the node panel.
  - Other visuals get a row of "Go deeper" chips under them.
  - Prose marks become inline buttons.
- **Opening.** Every mark opens the existing focus pop-out (`useOpenFocus`) with a new
  optional `question`. `SideThread` sends it once when the level opens with no turns yet.
- **Backend.** The grounding splitter learns not to split inside a prose mark and to read it
  as its shown text. The research prompt gains a visual-first section.

**Tech Stack:**
- React 18 and TypeScript.
- Node's built-in test runner (`node --experimental-strip-types --test`).
- Rust (`shodh-rag` crate) for the prompt and the grounding splitter.

**Spec:** `docs/superpowers/specs/2026-10-16-visual-explanations-design.md`. This plan
implements Section 3 (A) and the spec's work item 1 only. Sections 4 (source labels) and 5
(interactive pages) get their own plans after this one is merged.

## Global Constraints

- **Production code only.** No TODOs, placeholders, mocks or stubs (CLAUDE.md).
- **No local builds.** No `cargo build` or `cargo test` on this machine; Rust is verified by
  CI. Node unit tests (`npm test` in `app/`) run locally; they need no `node_modules` for pure
  modules.
- **PR workflow.**
  - Branch `feat/visual-marks` from `origin/master`; commit, push, PR, CI green, merge.
  - Commits carry no `Co-Authored-By` or "Generated with" lines.
- **Limits (spec 3.2).**
  - At most 4 marks per answer, counted in document order; extra marks are dropped.
  - A prepared question is non-empty and at most 160 characters; longer ones are dropped,
    never truncated.
  - Prose mark syntax is `[[shown text|prepared question]]`; shown text is 1–80 characters.
- **No call before the click (spec 1).** A mark sends nothing until clicked. A click while
  another answer runs puts the question into the window's composer instead of sending it.
- **Depth.** Limited by the existing `MAX_DEPTH`. At the limit a mark shows the existing depth
  notice (`notifyDepthLimit`) and opens nothing.
- **Prompt rule.** The research prompt may contain no snake_case identifier that is not a
  tool name, because `instructions_name_only_allowed_tools` enforces it. The field is
  `deeper`, never `go_deeper`.
- **Visible text.** Mark syntax never appears in what the user sees, copies or prints.

## Spec amendments (made in Task 1)

v1 places marks on the elements that can already be selected or listed today:

- diagram nodes;
- chart series;
- plot and simulation parameters;
- derivation steps;
- symbol entries;
- prose.

Diagram edges, single chart points and plot curves have no selectable element today. They
are left for a later version, and the spec says so. Diagrams draw the glow on the node. The
other visuals list their marks as a chip row under the visual, rather than drawing a glow
inside renderers that are lazily loaded.

## Review Focus

1. **The answer is still streaming.** The model writes `[[shown|ques` and the closing `]]` has
   not arrived. Expected: the partial text shows as plain text, never as broken markup. No
   button appears until the mark is complete. Test in Task 2.
2. **Marks inside code.** Inline code `` `[[a|b]]` `` and fenced code that contain mark syntax
   must stay literal code. They must not consume the budget either. Test in Task 2.
3. **A question with a "?" in a sentence.** The grounding splitter must not end a sentence
   inside `[[…|…?]]`. The claim text must read the shown text, not the question. Test in
   Task 6.
4. **Reopening a mark that was already asked.** The existing thread is reopened
   (`findChildThread`). The question must not be sent a second time. Test in Task 3.
5. **Copying an answer.** "Copy answer" must give the shown text, not `[[shown|question]]`.
   Test in Task 2 (`stripTextMarks`); wiring in Task 4.

---

## File structure

| File | Responsibility |
|---|---|
| Create `app/src/features/ask/visual/marks.ts` | Pure: read `deeper`, find prose marks, strip them, list a block's marks, apply the answer budget. |
| Create `app/tests/visualMarks.test.ts` | Unit tests for `marks.ts` and the diagram `deeper` field. |
| Create `app/src/features/focus/autoAsk.ts` | Pure: whether a level should send its prepared question now. |
| Create `app/tests/focusAutoAsk.test.ts` | Unit tests for `autoAsk.ts`. |
| Create `app/src/features/ask/visual/DeeperMarks.tsx` | `MarkText` (inline prose button) and `DeeperChips` (row under a visual), both opening a window with the prepared question. |
| Modify `app/src/features/ask/visual/diagramSpec.ts` | `deeper` on `DiagramNode`. |
| Modify `app/src/features/ask/visual/diagramSvg.ts` | Glow class and accessible name for marked nodes. |
| Modify `app/src/features/ask/visual/DiagramBlock.tsx` | "Go deeper" button in the node panel, within the budget. |
| Modify `app/src/features/ask/visual/answerContext.ts` | `markAllowance(source)` in `AnswerBlocks`. |
| Modify `app/src/features/ask/MessageContentRenderer.tsx` | Prose mark placeholders, chips under visuals, budget, allowance in context. |
| Modify `app/src/features/ask/AskView.tsx` | Copy answer without mark syntax. |
| Modify `app/src/features/focus/FocusContext.tsx` | `question` on `FocusRequest`, `DrillRequest`, `OpenFocus`. |
| Modify `app/src/features/focus/FocusFrame.tsx` | `useOpenFocus` takes `{ question }`. |
| Modify `app/src/features/focus/SideThread.tsx` | Send the prepared question once. |
| Modify `crates/shodh-rag/src/harness/grounding/claims.rs` | Prose marks: protected from sentence splits; claim text reads the shown text. |
| Modify `crates/shodh-rag/src/harness/profile.rs` | Visual-first section and how to write marks; tests. |
| Modify `docs/superpowers/specs/2026-10-16-visual-explanations-design.md` | The v1 element list (amendments above). |

---

### Task 1: Branch and spec amendment

**Files:**
- Modify: `docs/superpowers/specs/2026-10-16-visual-explanations-design.md` (Section 3.2,
  "Where marks go" and "Rendering")

- [ ] **Step 1: Create the branch in a fresh worktree**

```bash
cd "C:/Users/Varun Sharma/OneDrive/Documents/Roshera/Vector-DB/vectora/kalki-v2/shodh"
git fetch origin
git worktree add -b feat/visual-marks C:/wt/marks origin/master
git -C C:/wt/marks merge --no-edit origin/docs/visual-explanations
```

If `docs/visual-explanations` is not on the remote yet, push it first:
`git -C C:/wt/visx push -u origin docs/visual-explanations`.

- [ ] **Step 2: Amend the spec's mark list and rendering**

In Section 3.2, replace the "Where marks go" bullets with:

```markdown
- **Where marks go (v1):**
  - diagram nodes;
  - chart series;
  - plot and simulation parameters;
  - derivation steps;
  - symbol entries (equation terms).

  Diagram edges, single chart points and plot curves have no selectable element yet and
  come later.
```

Replace the "Rendering" bullet with:

```markdown
- **Rendering.**
  - A marked diagram node gets a small glow, and its node panel offers "Go deeper".
  - Other visuals list their marks as a row of "Go deeper" chips under the visual.
  - Marked text gets a dotted underline.
  - Hovering shows the prepared question.
  - Marks are keyboard reachable: Tab moves through them, Enter opens.
```

- [ ] **Step 3: Commit**

```bash
cd C:/wt/marks
git add docs/superpowers/specs/2026-10-16-visual-explanations-design.md
git commit -m "Spec: v1 mark elements and chip rendering"
```

---

### Task 2: The marks module

**Files:**
- Create: `app/src/features/ask/visual/marks.ts`
- Test: `app/tests/visualMarks.test.ts`

**Interfaces:**
- Produces:
  - `MAX_MARKS = 4`, `MAX_QUESTION_CHARS = 160`, `MAX_SHOWN_CHARS = 80`.
  - `readDeeper(value: unknown): string | null`.
  - `interface Mark { label: string; question: string }`.
  - `interface TextMark { start: number; end: number; shown: string; question: string | null }`.
  - `findTextMarks(text: string): TextMark[]`.
  - `stripTextMarks(text: string): string`.
  - `placeTextMarks(text: string, allowed: number, placeholder: (index: number) => string): { text: string; marks: { shown: string; question: string }[] }`.
  - `MARK_LANGUAGES: ReadonlySet<string>`.
  - `blockMarks(lang: string, source: string): Mark[]`.
  - `interface MarkBudget { text: number; blocks: Map<string, number> }`.
  - `markBudget(content: string): MarkBudget`.

`text` in `MarkBudget` is how many prose marks, counted in order over the prose with fenced
and inline code removed, may render as buttons. `blocks` maps a fence body (trimmed) to how
many of its marks may render.

- [ ] **Step 1: Write the failing tests**

```ts
/**
 * "Go deeper" marks: reading prepared questions, prose marks and the
 * per-answer budget (src/features/ask/visual/marks.ts).
 *   node --experimental-strip-types --test app/tests/visualMarks.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_MARKS,
  blockMarks,
  findTextMarks,
  markBudget,
  placeTextMarks,
  readDeeper,
  stripTextMarks,
} from '../src/features/ask/visual/marks.ts';

const fence = (lang: string, body: unknown) => `\`\`\`${lang}\n${JSON.stringify(body)}\n\`\`\``;

test('a prepared question is flattened, and empty or overlong ones are dropped', () => {
  assert.equal(readDeeper('  Why does   attention scale?  '), 'Why does attention scale?');
  assert.equal(readDeeper(''), null);
  assert.equal(readDeeper('   '), null);
  assert.equal(readDeeper(42), null);
  assert.equal(readDeeper('x'.repeat(160)), 'x'.repeat(160));
  assert.equal(readDeeper('x'.repeat(161)), null);
});

test('prose marks are found with their shown text and question', () => {
  const text = 'It uses [[self-attention|Why does self-attention cost grow with length?]] over tokens.';
  const marks = findTextMarks(text);
  assert.equal(marks.length, 1);
  assert.equal(marks[0].shown, 'self-attention');
  assert.equal(marks[0].question, 'Why does self-attention cost grow with length?');
  assert.equal(text.slice(marks[0].start, marks[0].end), '[[self-attention|Why does self-attention cost grow with length?]]');
});

test('an unfinished mark (still streaming) is not a mark', () => {
  assert.deepEqual(findTextMarks('It uses [[self-attention|Why does'), []);
  assert.equal(stripTextMarks('It uses [[self-attention|Why does'), 'It uses [[self-attention|Why does');
});

test('a mark whose question is too long keeps its shown text but has no question', () => {
  const marks = findTextMarks(`[[term|${'q'.repeat(161)}]]`);
  assert.equal(marks.length, 1, 'still a mark, so its syntax never shows');
  assert.equal(marks[0].question, null);
  assert.equal(stripTextMarks(`A [[term|${'q'.repeat(161)}]].`), 'A term.');
  const ok = findTextMarks(`[[term|${'q'.repeat(160)}]]`);
  assert.equal(ok[0].question, 'q'.repeat(160));
});

test('placing marks skips inline code and keeps only the allowed number', () => {
  const text = 'Use `[[a|qa?]]`, then [[b|qb?]], [[c|qc?]] and [[d|]].';
  const placed = placeTextMarks(text, 1, i => `<${i}>`);
  assert.equal(placed.text, 'Use `[[a|qa?]]`, then <0>, c and d.');
  assert.deepEqual(placed.marks, [{ shown: 'b', question: 'qb?' }]);
});

test('stripping marks leaves the shown text', () => {
  assert.equal(stripTextMarks('Uses [[RoPE|What is rotary position embedding?]] and [[ALiBi|How does ALiBi bias attention?]].'), 'Uses RoPE and ALiBi.');
});

test('marks inside code do not count and are not stripped', () => {
  const content = 'See `[[a|b?]]` here.\n\n```text\n[[c|d?]]\n```\n\nThen [[e|f?]].';
  const budget = markBudget(content);
  assert.equal(budget.text, 1);
  assert.equal(stripTextMarks('See `[[a|b?]]` here.'), 'See `[[a|b?]]` here.');
});

test('block marks are read from each visual kind', () => {
  assert.deepEqual(
    blockMarks('diagram', JSON.stringify({ layout: 'flow', nodes: [{ id: 'a', label: 'Attention', deeper: 'Why quadratic?' }, { id: 'b', label: 'FFN' }] })),
    [{ label: 'Attention', question: 'Why quadratic?' }],
  );
  assert.deepEqual(
    blockMarks('chart', JSON.stringify({ type: 'line', xKey: 'year', series: [{ key: 's', label: 'Score', deeper: 'What drove the 2024 jump?' }], data: [{ year: 2023, s: 1 }] })),
    [{ label: 'Score', question: 'What drove the 2024 jump?' }],
  );
  assert.deepEqual(
    blockMarks('chart', JSON.stringify({ type: 'bar', data: { labels: ['a'], datasets: [{ label: 'Cost', data: [1], deeper: 'Why so high?' }] } })),
    [{ label: 'Cost', question: 'Why so high?' }],
  );
  assert.deepEqual(
    blockMarks('plot', JSON.stringify({ params: [{ name: 'v0', label: 'v0 (m/s)', deeper: 'What sets the launch speed?' }, { name: 'g' }] })),
    [{ label: 'v0 (m/s)', question: 'What sets the launch speed?' }],
  );
  assert.deepEqual(
    blockMarks('simulation', JSON.stringify({ params: [{ name: 'k', deeper: 'What is a spring constant?' }] })),
    [{ label: 'k', question: 'What is a spring constant?' }],
  );
  assert.deepEqual(
    blockMarks('derivation', JSON.stringify({ steps: [{ latex: 'a=b' }, { latex: 'b=c', deeper: 'Why can we substitute here?' }] })),
    [{ label: 'Step 2', question: 'Why can we substitute here?' }],
  );
  assert.deepEqual(
    blockMarks('symbols', JSON.stringify([{ symbol: '\\Phi_q', meaning: 'feature map', deeper: 'What is a feature map?' }])),
    [{ label: '\\Phi_q', question: 'What is a feature map?' }],
  );
  assert.deepEqual(blockMarks('diagram', '{not json'), []);
  assert.deepEqual(blockMarks('python', '{"deeper": "x"}'), []);
});

test('the budget allows four marks per answer in document order', () => {
  const diagram = { layout: 'flow', nodes: [
    { id: 'a', label: 'A', deeper: 'qa?' }, { id: 'b', label: 'B', deeper: 'qb?' }, { id: 'c', label: 'C', deeper: 'qc?' },
  ] };
  const content = `First [[x|qx?]] then [[y|qy?]].\n\n${fence('diagram', diagram)}\n\nLast [[z|qz?]].`;
  const budget = markBudget(content);
  assert.equal(MAX_MARKS, 4);
  assert.equal(budget.text, 2, 'the two prose marks before the diagram; the last one is over budget');
  assert.equal(budget.blocks.get(JSON.stringify(diagram)), 2, 'the diagram gets the remaining two');
});

test('an answer without marks has an empty budget', () => {
  const budget = markBudget('Plain text.\n\n```chart\n{"type":"bar"}\n```');
  assert.equal(budget.text, 0);
  assert.equal(budget.blocks.get('{"type":"bar"}') ?? 0, 0);
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd C:/wt/marks/app && node --experimental-strip-types --test tests/visualMarks.test.ts`

Expected: FAIL with `Cannot find module '…/src/features/ask/visual/marks.ts'`.

- [ ] **Step 3: Write `marks.ts`**

```ts
/**
 * "Go deeper" marks: questions the model prepares on parts of its answer. A
 * mark is a `deeper` string on a visual's element (a diagram node, a chart
 * series, a plot or simulation parameter, a derivation step, a symbol), or
 * `[[shown text|question]]` in prose. Clicking one opens a nested window that
 * asks the question; nothing is sent before the click.
 *
 * An answer shows at most MAX_MARKS marks, the first ones in document order
 * (prose and visuals together); the rest render as plain text or no button.
 * Marks in inline or fenced code are code, not marks.
 *
 * Pure module, unit-tested with Node (`app/tests/visualMarks.test.ts`).
 */

export const MAX_MARKS = 4;
export const MAX_QUESTION_CHARS = 160;
export const MAX_SHOWN_CHARS = 80;
const MAX_LABEL_CHARS = 60;

/** Fenced languages whose elements may carry `deeper`. */
export const MARK_LANGUAGES: ReadonlySet<string> = new Set(['diagram', 'chart', 'plot', 'simulation', 'derivation', 'symbols']);

export interface Mark {
  /** What the mark is on (a node's label, a series, "Step 2"). */
  label: string;
  question: string;
}

export interface TextMark {
  start: number;
  end: number;
  shown: string;
  /** Null when the question is unusable; the shown text is then plain. */
  question: string | null;
}

/**
 * `[[shown|question]]`: shown 1–80 characters, no brackets or line breaks. The
 * question part matches up to 400 characters so an overlong one is still a
 * mark (shown as plain text, never as syntax); readDeeper drops it past 160.
 */
const TEXT_MARK = /\[\[([^[\]|\n]{1,80})\|([^[\]\n]{0,400})\]\]/g;
const FENCE = /^```([\w-]*)[^\n]*\n([\s\S]*?)^```[ \t]*$/gm;
const INLINE_CODE = /`[^`\n]*`/g;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function flat(value: unknown, max: number): string | null {
  if (typeof value !== 'string') return null;
  const text = value.replace(/\s+/g, ' ').trim();
  if (!text) return null;
  const chars = Array.from(text);
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : text;
}

/** A prepared question: flattened, non-empty, at most MAX_QUESTION_CHARS (longer ones are dropped). */
export function readDeeper(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const text = value.replace(/\s+/g, ' ').trim();
  if (!text || Array.from(text).length > MAX_QUESTION_CHARS) return null;
  return text;
}

/** Prose marks of `text`, in order. Code is not excluded here: callers pass prose. */
export function findTextMarks(text: string): TextMark[] {
  const out: TextMark[] = [];
  for (const m of text.matchAll(TEXT_MARK)) {
    const start = m.index ?? 0;
    const shown = m[1].trim();
    if (!shown) continue;
    out.push({ start, end: start + m[0].length, shown, question: readDeeper(m[2]) });
  }
  return out;
}

/** Ranges of inline code spans in `text`. */
function codeRanges(text: string): [number, number][] {
  return Array.from(text.matchAll(INLINE_CODE), m => [m.index ?? 0, (m.index ?? 0) + m[0].length] as [number, number]);
}

/** `text` with every complete prose mark (outside inline code) replaced by its shown text. */
export function stripTextMarks(text: string): string {
  const code = codeRanges(text);
  let out = '';
  let last = 0;
  for (const mark of findTextMarks(text)) {
    if (code.some(([s, e]) => mark.start >= s && mark.start < e)) continue;
    out += text.slice(last, mark.start) + mark.shown;
    last = mark.end;
  }
  return out + text.slice(last);
}

/**
 * `text` with its first `allowed` usable prose marks (outside inline code)
 * replaced by `placeholder(i)` and the other marks by their shown text;
 * marks inside inline code stay as written.
 */
export function placeTextMarks(
  text: string,
  allowed: number,
  placeholder: (index: number) => string,
): { text: string; marks: { shown: string; question: string }[] } {
  const code = codeRanges(text);
  const marks: { shown: string; question: string }[] = [];
  let out = '';
  let last = 0;
  for (const mark of findTextMarks(text)) {
    if (code.some(([s, e]) => mark.start >= s && mark.start < e)) continue;
    out += text.slice(last, mark.start);
    if (mark.question !== null && marks.length < allowed) {
      out += placeholder(marks.length);
      marks.push({ shown: mark.shown, question: mark.question });
    } else {
      out += mark.shown;
    }
    last = mark.end;
  }
  return { text: out + text.slice(last), marks };
}

function list(value: unknown, key: string): unknown[] {
  if (Array.isArray(value)) return value;
  if (isRecord(value) && Array.isArray(value[key])) return value[key] as unknown[];
  return [];
}

/** The marks of one fenced block, in element order. Unknown languages and invalid JSON have none. */
export function blockMarks(lang: string, source: string): Mark[] {
  if (!MARK_LANGUAGES.has(lang)) return [];
  let raw: unknown;
  try {
    raw = JSON.parse(source);
  } catch {
    return [];
  }
  const entries: { label: string | null; deeper: unknown }[] = [];
  if (lang === 'diagram') {
    for (const n of list(raw, 'nodes')) if (isRecord(n)) entries.push({ label: flat(n.label, MAX_LABEL_CHARS) ?? flat(n.id, MAX_LABEL_CHARS), deeper: n.deeper });
  } else if (lang === 'chart') {
    const series = isRecord(raw) && Array.isArray(raw.series)
      ? raw.series
      : isRecord(raw) && isRecord(raw.data) && Array.isArray(raw.data.datasets) ? raw.data.datasets : [];
    for (const s of series) if (isRecord(s)) entries.push({ label: flat(s.label, MAX_LABEL_CHARS) ?? flat(s.key, MAX_LABEL_CHARS), deeper: s.deeper });
  } else if (lang === 'plot' || lang === 'simulation') {
    for (const p of isRecord(raw) && Array.isArray(raw.params) ? raw.params : []) {
      if (isRecord(p)) entries.push({ label: flat(p.label, MAX_LABEL_CHARS) ?? flat(p.name, MAX_LABEL_CHARS), deeper: p.deeper });
    }
  } else if (lang === 'derivation') {
    list(raw, 'steps').forEach((s, i) => {
      if (isRecord(s)) entries.push({ label: `Step ${i + 1}`, deeper: s.deeper });
    });
  } else if (lang === 'symbols') {
    for (const s of list(raw, 'symbols')) if (isRecord(s)) entries.push({ label: flat(s.symbol ?? s.latex, MAX_LABEL_CHARS), deeper: s.deeper });
  }
  const out: Mark[] = [];
  for (const e of entries) {
    const question = readDeeper(e.deeper);
    if (question && e.label) out.push({ label: e.label, question });
  }
  return out;
}

export interface MarkBudget {
  /** How many prose marks (in order, outside code) render as buttons. */
  text: number;
  /** Fence body (trimmed) to how many of its marks render. */
  blocks: Map<string, number>;
}

/** Which marks of an answer render: the first MAX_MARKS in document order. */
export function markBudget(content: string): MarkBudget {
  const blocks = new Map<string, number>();
  let left = MAX_MARKS;
  let text = 0;
  const prose = (part: string) => {
    const withoutCode = part.replace(INLINE_CODE, m => ' '.repeat(m.length));
    const usable = findTextMarks(withoutCode).filter(m => m.question !== null).length;
    const allowed = Math.min(usable, left);
    text += allowed;
    left -= allowed;
  };
  let last = 0;
  for (const m of content.matchAll(FENCE)) {
    const start = m.index ?? 0;
    prose(content.slice(last, start));
    last = start + m[0].length;
    const body = m[2].replace(/\n$/, '').trim();
    const count = blockMarks(m[1].toLowerCase(), body).length;
    if (count > 0 && !blocks.has(body)) {
      const allowed = Math.min(count, left);
      blocks.set(body, allowed);
      left -= allowed;
    }
  }
  prose(content.slice(last));
  return { text, blocks };
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd C:/wt/marks/app && node --experimental-strip-types --test tests/visualMarks.test.ts`

Expected: PASS, 10 tests.

- [ ] **Step 5: Commit**

```bash
cd C:/wt/marks
git add app/src/features/ask/visual/marks.ts app/tests/visualMarks.test.ts
git commit -m "Marks: read prepared questions, prose marks and the per-answer budget"
```

---

### Task 3: Open a window with a prepared question

**Files:**
- Create: `app/src/features/focus/autoAsk.ts`
- Test: `app/tests/focusAutoAsk.test.ts`
- Modify:
  - `app/src/features/focus/FocusContext.tsx`: interfaces at lines 65–106, `openFocus` at
    line 895, `drillDown` at line 924.
  - `app/src/features/focus/FocusFrame.tsx:29-45`: `useOpenFocus`.
  - `app/src/features/focus/SideThread.tsx`, after `askSuggested` (around line 282).

**Interfaces:**
- Produces:
  - `autoAskQuestion(level: { question: string | null; seq: number }, turnCount: number, asked: ReadonlySet<number>): string | null`.
  - `FocusRequest.question?: string | null`.
  - `DrillRequest.question?: string | null`.
  - `OpenFocus.question: string | null`.
  - `useOpenFocus(): ((target: FocusTarget, trigger: HTMLElement | null, options?: { question?: string | null }) => void) | null`.

- [ ] **Step 1: Write the failing test**

```ts
/**
 * A level opened from a "go deeper" mark asks its prepared question once
 * (src/features/focus/autoAsk.ts).
 *   node --experimental-strip-types --test app/tests/focusAutoAsk.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { autoAskQuestion } from '../src/features/focus/autoAsk.ts';

test('a fresh level with a prepared question asks it', () => {
  assert.equal(autoAskQuestion({ question: 'Why quadratic?', seq: 7 }, 0, new Set()), 'Why quadratic?');
});

test('a level without a prepared question asks nothing', () => {
  assert.equal(autoAskQuestion({ question: null, seq: 7 }, 0, new Set()), null);
  assert.equal(autoAskQuestion({ question: '   ', seq: 7 }, 0, new Set()), null);
});

test('a reopened thread that already has turns is not asked again', () => {
  assert.equal(autoAskQuestion({ question: 'Why quadratic?', seq: 7 }, 2, new Set()), null);
});

test('the same level asks only once', () => {
  assert.equal(autoAskQuestion({ question: 'Why quadratic?', seq: 7 }, 0, new Set([7])), null);
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd C:/wt/marks/app && node --experimental-strip-types --test tests/focusAutoAsk.test.ts`

Expected: FAIL with `Cannot find module`.

- [ ] **Step 3: Write `autoAsk.ts`**

```ts
/**
 * Whether a focus level opened from a "go deeper" mark sends its prepared
 * question now: only on a fresh thread (a reopened one already has the
 * answer), and only once per level (`seq` is unique per opened level).
 *
 * Pure module, unit-tested with Node (`app/tests/focusAutoAsk.test.ts`).
 */
export function autoAskQuestion(
  level: { question: string | null; seq: number },
  turnCount: number,
  asked: ReadonlySet<number>,
): string | null {
  const question = level.question?.trim() ?? '';
  if (!question || turnCount > 0 || asked.has(level.seq)) return null;
  return question;
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cd C:/wt/marks/app && node --experimental-strip-types --test tests/focusAutoAsk.test.ts`

Expected: PASS, 4 tests.

- [ ] **Step 5: Carry `question` through the focus context**

In `FocusContext.tsx`:

1. Add to `FocusRequest`, after `trigger`:

```ts
  /** A prepared question the new level sends once (a "go deeper" mark). */
  question?: string | null;
```

2. Add the same two lines to `DrillRequest`, after `parentTurnId`.

3. Add to `OpenFocus`, after `visual`:

```ts
  /** A prepared question this level sends once when its thread has no turns yet. */
  question: string | null;
```

4. In `openFocus`, inside the `initialStack({...})` object, after `visual: request.visual ?? null,`, add:

```ts
        question: request.question ?? null,
```

5. In `drillDown`, in `const level: OpenFocus = {...}`, after `visual: null,`, add:

```ts
      question: request.question ?? null,
```

6. Three other places build an `OpenFocus` and must set `question: null`. Add `question: null,`
   after the `visual:` line of each:
   - `visual: visualRef(record),` (about line 754, the level of `showVisualVersion`);
   - `visual: null,` in the diagram-correction level (about line 818, after
     `target: { kind: 'mermaid', label: 'Diagram correction', … }`);
   - `visual: null,` in `levelsFor`'s `path.map(t => ({ … }))` (about line 891).

   The CI typecheck catches any literal that still lacks the field.

- [ ] **Step 6: Let `useOpenFocus` pass the question**

Replace the body of `useOpenFocus` in `FocusFrame.tsx` (lines 29–45) with:

```ts
export function useOpenFocus(): ((target: FocusTarget, trigger: HTMLElement | null, options?: { question?: string | null }) => void) | null {
  const focus = useFocus();
  const drill = useFocusDrill();
  const answerAnchor = useFocusAnchor();
  const anchor = drill ? null : answerAnchor;
  const open = useCallback((target: FocusTarget, trigger: HTMLElement | null, options?: { question?: string | null }) => {
    if (!focus) return;
    const question = options?.question ?? null;
    if (drill) {
      const result = focus.drillDown({ target, parentThreadId: drill.parentThreadId, parentTurnId: drill.parentTurnId, question });
      if (result === 'depth') notifyDepthLimit();
      return;
    }
    if (!anchor) return;
    focus.openFocus({ target, conversationId: anchor.conversationId, parentMessageId: anchor.messageId, trigger, question });
  }, [focus, anchor, drill]);
  return focus && (anchor || drill) ? open : null;
}
```

- [ ] **Step 7: Send the prepared question once in `SideThread`**

In `SideThread.tsx`, add the import:

```ts
import { autoAskQuestion } from './autoAsk';
```

After the `askSuggested` callback, add:

```ts
  // A level opened from a "go deeper" mark asks its prepared question once.
  // While another answer runs, askSuggested puts it in the composer instead.
  const askedLevels = useRef<Set<number>>(new Set());
  useEffect(() => {
    const question = autoAskQuestion(open, turns.length, askedLevels.current);
    if (question === null) return;
    askedLevels.current.add(open.seq);
    void askSuggested(question);
  }, [open, turns.length, askSuggested]);
```

- [ ] **Step 8: Run the whole frontend unit suite**

Run: `cd C:/wt/marks/app && npm test`

Expected: PASS. Every existing test passes, as do the 4 new ones.

- [ ] **Step 9: Commit**

```bash
cd C:/wt/marks
git add app/src/features/focus/autoAsk.ts app/tests/focusAutoAsk.test.ts app/src/features/focus/FocusContext.tsx app/src/features/focus/FocusFrame.tsx app/src/features/focus/SideThread.tsx
git commit -m "Focus: a level can open with a prepared question, asked once"
```

---

### Task 4: Prose marks and chips under visuals

**Files:**
- Create: `app/src/features/ask/visual/DeeperMarks.tsx`
- Modify:
  - `app/src/features/ask/visual/answerContext.ts`;
  - `app/src/features/ask/MessageContentRenderer.tsx`: the preprocessing memo (the
    `useMemo` that ends `return text.replace(/\n{3,}/g, '\n\n');`), `TOKEN_PATTERN` at line
    42, `renderWithCitations` at line 258, the `code` component at line 349 and
    `answerBlocks` at line 460;
  - `app/src/features/ask/AskView.tsx:476`.

**Interfaces:**
- Consumes:
  - `markBudget`, `findTextMarks`, `stripTextMarks`, `blockMarks`, `Mark` (Task 2);
  - `useOpenFocus(...)(target, trigger, { question })` (Task 3);
  - `selectionTarget` from `../../focus/targets`.
- Produces:
  - `AnswerBlocks.markAllowance: (source: string) => number`;
  - `<MarkText shown question />`;
  - `<DeeperChips lang source />`.

- [ ] **Step 1: Add `markAllowance` to the answer context**

In `answerContext.ts`, add to `AnswerBlocks` after `openCitation`:

```ts
  /**
   * How many "go deeper" marks of the fenced block with this body may
   * render (the answer's budget, in document order); 0 outside model answers.
   */
  markAllowance: (source: string) => number;
```

Change `NONE` to:

```ts
const NONE: AnswerBlocks = { symbols: [], renderInline: text => text, modelAnswer: false, citations: null, openCitation: null, markAllowance: () => 0 };
```

- [ ] **Step 2: Write `DeeperMarks.tsx`**

```tsx
import { Sparkles } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { useOpenFocus } from '../../focus/FocusFrame';
import { selectionTarget } from '../../focus/targets';
import { useAnswerBlocks } from './answerContext';
import { blockMarks } from './marks';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

/**
 * A "go deeper" mark in prose: the shown text, dotted underline, opening a
 * nested window that asks the prepared question. Plain text where nothing
 * can be opened (no answer around, a printout).
 */
export function MarkText({ shown, question, context }: { shown: string; question: string; context: string }) {
  const openFocus = useOpenFocus();
  if (!openFocus) return <>{shown}</>;
  return (
    <button
      type="button"
      title={question}
      aria-label={`${shown}. Go deeper: ${question}`}
      className={cn(
        'inline p-0 m-0 bg-transparent border-0 font-[inherit] text-[inherit] text-left cursor-pointer',
        'underline decoration-dotted decoration-shodh-accent underline-offset-[3px] hover:decoration-solid',
        FOCUS_RING,
      )}
      onClick={e => {
        const target = selectionTarget({ text: shown, context, origin: 'answer' });
        if (target) openFocus(target, e.currentTarget, { question });
      }}
    >
      {shown}
    </button>
  );
}

/**
 * The "go deeper" marks of a chart, plot, simulation, derivation or symbol
 * list, as chips under it (diagrams draw theirs on the node). Only as many
 * as the answer's budget gives this block.
 */
export function DeeperChips({ lang, source }: { lang: string; source: string }) {
  const { markAllowance } = useAnswerBlocks();
  const openFocus = useOpenFocus();
  const allowed = markAllowance(source);
  if (!openFocus || allowed === 0) return null;
  const marks = blockMarks(lang, source.trim()).slice(0, allowed);
  if (marks.length === 0) return null;
  return (
    <div className="-mt-2 mb-4 flex flex-wrap items-center gap-1.5" role="group" aria-label="Go deeper">
      <span className="text-[11.5px] text-shodh-text-muted">Go deeper:</span>
      {marks.map(mark => (
        <button
          key={`${mark.label}\n${mark.question}`}
          type="button"
          title={mark.question}
          className={cn(
            'inline-flex items-center gap-1 h-6 px-2 rounded-full border border-shodh-border bg-shodh-surface text-[11.5px] text-shodh-text-secondary',
            'hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
            FOCUS_RING,
          )}
          onClick={e => {
            const target = selectionTarget({ text: mark.label, context: mark.question, origin: 'answer' });
            if (target) openFocus(target, e.currentTarget, { question: mark.question });
          }}
        >
          <Sparkles className="w-3 h-3 text-shodh-accent" aria-hidden="true" />
          <span className="font-medium">{mark.label}</span>
          <span className="text-shodh-text-muted truncate max-w-[260px]">{mark.question}</span>
        </button>
      ))}
    </div>
  );
}
```

- [ ] **Step 3: Turn prose marks into placeholders in the preprocessing memo**

In `MessageContentRenderer.tsx`:

1. Add the imports:

```ts
import { markBudget, placeTextMarks } from './visual/marks';
import { DeeperChips, MarkText } from './visual/DeeperMarks';
```

2. Under the citation placeholder constants (after line 42), add:

```ts
/** "Go deeper" prose marks: placeholders that survive markdown parsing. */
const MARK_OPEN = 'XMSHODH';
const MARK_CLOSE = 'XNSHODH';
const MARK_TEST = new RegExp(`${MARK_OPEN}\\d+${MARK_CLOSE}`);
```

3. Change `TOKEN_PATTERN` to:

```ts
const TOKEN_PATTERN = new RegExp(`${CITE_OPEN}(\\d+)${CITE_CLOSE}|${FLAG_OPEN}(\\d+)${FLAG_CLOSE}|${MARK_OPEN}(\\d+)${MARK_CLOSE}`, 'g');
```

4. Before the preprocessing memo, add:

```ts
  // Which "go deeper" marks render: the first four of the answer, in order.
  const budget = useMemo(() => markBudget(content), [content]);
```

5. Change the memo's declaration from `const preprocessed = useMemo(() => {` to
   `const prepared = useMemo(() => {`. Inside it, directly after the line
   `text = math.text;`, add:

```ts
    // Prose marks within the budget become placeholders; the rest (and every
    // mark in text the reader wrote or in a printout) become their shown text.
    // Fenced code was protected above; placeTextMarks skips inline code.
    const placed = placeTextMarks(text, citations && !print ? budget.text : 0, i => `${MARK_OPEN}${i}${MARK_CLOSE}`);
    text = placed.text;
    const textMarks = placed.marks;
```

6. Change the memo's final `return text.replace(/\n{3,}/g, '\n\n');` to:

```ts
    return { text: text.replace(/\n{3,}/g, '\n\n'), textMarks };
```

7. Add `budget` and `print` to that memo's dependency list. Directly after the memo, add:

```ts
  const preprocessed = prepared.text;
  const textMarks = prepared.textMarks;
```

- [ ] **Step 4: Render the placeholders**

In `renderWithCitations`:

1. Change the early return condition to:

```ts
    if (hitsByNumber.size === 0 && !flagUnknownCitations && !(claims && claims.length > 0) && !MARK_TEST.test(text)) {
```

2. Inside the `for (const match of text.matchAll(TOKEN_PATTERN))` loop, right after
   `lastIndex = index + match[0].length;`, add:

```ts
      if (match[3] !== undefined) {
        const mark = textMarks[Number(match[3])];
        if (mark) parts.push(<MarkText key={`mark-${index}`} shown={mark.shown} question={mark.question} context={stripForContext(text)} />);
        continue;
      }
```

3. Add `textMarks` to the `useCallback` dependency list of `renderWithCitations`.

4. Above the component, add the helper the context uses:

```ts
/** A text node's words for a mark's context: placeholders removed. */
function stripForContext(text: string): string {
  return text.replace(TOKEN_PATTERN, '').replace(/\s+/g, ' ').trim();
}
```

- [ ] **Step 5: Chips under visuals, and the allowance in context**

In the `code` component, replace the five lines that return `ChartBlock`, `PlotBlock`,
`SimulationBlock`, `DerivationBlock` and `SymbolsBlock` with:

```tsx
        if (match[1] === 'chart') return <><ChartBlock source={codeString} theme={theme} /><DeeperChips lang="chart" source={codeString} /></>;
        if (match[1] === 'plot') return <><PlotBlock source={codeString} /><DeeperChips lang="plot" source={codeString} /></>;
        if (match[1] === 'simulation') return <><SimulationBlock source={codeString} /><DeeperChips lang="simulation" source={codeString} /></>;
        if (match[1] === 'derivation') return <><DerivationBlock source={codeString} /><DeeperChips lang="derivation" source={codeString} /></>;
        if (match[1] === 'symbols') return <><SymbolsBlock source={codeString} /><DeeperChips lang="symbols" source={codeString} /></>;
```

In the `answerBlocks` memo, add after `openCitation: …,`:

```ts
      markAllowance: citations && !print ? (source: string) => budget.blocks.get(source.trim()) ?? 0 : () => 0,
```

Then add `budget` to that memo's dependency list.

- [ ] **Step 6: Copy an answer without mark syntax**

In `AskView.tsx`, add the import:

```ts
import { stripTextMarks } from './visual/marks';
```

Change line 476 to:

```tsx
          {message.content.length > 0 && <CopyAnswerButton text={stripTextMarks(message.content)} />}
```

- [ ] **Step 7: Run the unit suite**

Run: `cd C:/wt/marks/app && npm test`

Expected: PASS. The `.tsx` changes are typechecked in CI (Frontend Typecheck + Tests), since
this machine has no `node_modules`.

- [ ] **Step 8: Commit**

```bash
cd C:/wt/marks
git add app/src/features/ask/visual/DeeperMarks.tsx app/src/features/ask/visual/answerContext.ts app/src/features/ask/MessageContentRenderer.tsx app/src/features/ask/AskView.tsx
git commit -m "Answers: go-deeper marks in prose and as chips under visuals"
```

---

### Task 5: Marked diagram nodes

**Files:**
- Modify:
  - `app/src/features/ask/visual/diagramSpec.ts`: `DiagramNode` at line 45, the
    `nodes.push({...})` at line 224, `describeNode` at line 403;
  - `app/src/features/ask/visual/diagramSvg.ts`: node loop at lines 123–137;
  - `app/src/features/ask/visual/DiagramBlock.tsx`: `DiagramBlock` and `DiagramView`.
- Test: `app/tests/visualMarks.test.ts` (append).

**Interfaces:**
- Consumes:
  - `readDeeper` (Task 2);
  - `useAnswerBlocks().markAllowance` (Task 4);
  - `useOpenFocus` with `{ question }` (Task 3).
- Produces:
  - `DiagramNode.deeper?: string | null`;
  - `diagramSvg` option `marked?: ReadonlySet<number>` (node indices drawn with the glow).

- [ ] **Step 1: Write the failing tests (append to `visualMarks.test.ts`)**

```ts
import { parseDiagram } from '../src/features/ask/visual/diagramSpec.ts';
import { layoutDiagram } from '../src/features/ask/visual/diagramLayout.ts';
import { diagramSvg } from '../src/features/ask/visual/diagramSvg.ts';

test('diagram nodes keep a usable prepared question and drop an unusable one', () => {
  const r = parseDiagram(JSON.stringify({ layout: 'flow', nodes: [
    { id: 'a', label: 'Attention', deeper: ' Why is it quadratic? ' },
    { id: 'b', label: 'FFN', deeper: 'x'.repeat(161) },
    { id: 'c', label: 'Norm' },
  ] }), { citations: null, codeMode: false });
  assert.ok(r.ok);
  if (!r.ok) return;
  assert.equal(r.spec.nodes[0].deeper, 'Why is it quadratic?');
  assert.equal(r.spec.nodes[1].deeper, null);
  assert.equal(r.spec.nodes[2].deeper, null);
});

test('marked nodes are drawn with the glow and say so to screen readers', () => {
  const r = parseDiagram(JSON.stringify({ layout: 'flow', nodes: [
    { id: 'a', label: 'Attention', deeper: 'Why quadratic?' }, { id: 'b', label: 'FFN' },
  ], edges: [{ from: 'a', to: 'b' }] }), { citations: null, codeMode: false });
  assert.ok(r.ok);
  if (!r.ok) return;
  const svg = diagramSvg(r.spec, layoutDiagram(r.spec), { interactive: true, idPrefix: 't', marked: new Set([0]) });
  assert.match(svg, /data-node="0"[^>]*data-deeper="true"/);
  assert.match(svg, /aria-label="Attention, go deeper available"/);
  assert.doesNotMatch(svg, /data-node="1"[^>]*data-deeper/);
  const plain = diagramSvg(r.spec, layoutDiagram(r.spec), { interactive: true, idPrefix: 't' });
  assert.doesNotMatch(plain, /data-deeper/);
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd C:/wt/marks/app && node --experimental-strip-types --test tests/visualMarks.test.ts`

Expected: FAIL. `nodes[0].deeper` is `undefined`, and the SVG has no `data-deeper`.

- [ ] **Step 3: Parse `deeper` on nodes**

In `diagramSpec.ts`:

1. Add the import at the top, after the doc comment:

```ts
import { readDeeper } from './marks.ts';
```

2. In `DiagramNode`, after `at: string | null;`, add:

```ts
  /** A prepared question that explains this node on its own ("go deeper"). */
  deeper?: string | null;
```

3. In `nodes.push({...})`, after `at: kind === 'timeline' ? at : null,`, add:

```ts
      deeper: readDeeper(item.deeper),
```

4. In `describeNode`, after `if (node.cite !== null) lines.push(...)`, add:

```ts
  if (node.deeper) lines.push(`Prepared question: ${node.deeper}`);
```

The `.ts` extension in the import matches the other visual modules (`diagramSvg.ts` imports
`./diagramSpec.ts`), which lets the Node tests load them directly.

- [ ] **Step 4: Draw the glow**

In `diagramSvg.ts`:

1. Add to `SvgOptions`, after `interactive: boolean;`:

```ts
  /** Indices of nodes with a "go deeper" mark the answer's budget allows (glow and name). */
  marked?: ReadonlySet<number>;
```

2. In the node loop, replace the `const name = …` line and the `const group = …` statement
   with:

```ts
    const deeper = o.interactive && o.marked?.has(b.node) === true;
    const name = [node.label, node.kind, node.change, node.path, node.cite !== null ? `source ${node.cite}` : null, deeper ? 'go deeper available' : null]
      .filter(Boolean)
      .join(', ');
    const group = o.interactive
      ? `<g data-node="${b.node}"${deeper ? ' data-deeper="true"' : ''} role="button" tabindex="0" aria-label="${escapeXml(name)}" class="${NODE_CLASS}${deeper ? ` ${DEEPER_CLASS}` : ''}">`
      : '<g>';
```

3. Under the `NODE_CLASS` constant, add:

```ts
/** A soft accent halo on a node with a "go deeper" mark (`--c-accent` is the theme's accent, index.css). */
const DEEPER_CLASS = '[&>:is(rect,circle):first-of-type]:[filter:drop-shadow(0_0_4px_var(--c-accent))]';
```

- [ ] **Step 5: Use the budget and offer "Go deeper" in the node panel**

In `DiagramBlock.tsx`:

1. In `DiagramBlock`, read the allowance:

```ts
  const { citations, openCitation, markAllowance } = useAnswerBlocks();
```

2. Pass it to the view, adding one prop:

```tsx
    <DiagramView
      spec={spec}
      files={files.status === 'ready' ? files.found : null}
      openCitation={openCitation}
      allowance={markAllowance(source)}
    />
```

3. Add to `DiagramViewProps`:

```ts
  /** How many of its marked nodes the answer's budget allows. */
  allowance: number;
```

4. In `DiagramView`, take `allowance` from the props. Compute the marked set before
   `markup`, and pass it to `diagramSvg`:

```ts
  const marked = useMemo(() => {
    const out = new Set<number>();
    spec.nodes.forEach((n, i) => {
      if (n.deeper && out.size < allowance) out.add(i);
    });
    return out;
  }, [spec, allowance]);
  const markup = useMemo(() => diagramSvg(spec, drawing, { interactive: true, idPrefix: prefix, marked }), [spec, drawing, prefix, marked]);
```

5. Add the click handler next to `askAboutNode`:

```ts
  const goDeeper = (trigger: HTMLElement) => {
    if (!openFocus || !node || !node.deeper) return;
    const target = selectionTarget({ text: describeNode(spec, node.id), context: description, origin: 'answer' });
    if (target) openFocus(target, trigger, { question: node.deeper });
  };
```

6. In the node panel's button row, before the "Ask about this" button, add:

```tsx
                {openFocus && selected !== null && marked.has(selected) && node.deeper && (
                  <button type="button" className={cn(ACTION, FOCUS_RING, 'border-shodh-accent text-shodh-text')} title={node.deeper} onClick={e => goDeeper(e.currentTarget)}>
                    <Sparkles className="w-3.5 h-3.5 text-shodh-accent" aria-hidden="true" />
                    Go deeper
                  </button>
                )}
```

7. Add `Sparkles` to the `lucide-react` import.

8. In the hint line at the bottom, change `'Select a part to open its file or source, or to ask about it.'` to:

```ts
                marked.size > 0
                  ? 'Glowing parts have more to explain: select one and choose Go deeper.'
                  : 'Select a part to open its file or source, or to ask about it.',
```

Also make the hint show when only marks exist. Change its condition to
`!node && (hasDelta || actionable || drawing.loops.length > 0 || marked.size > 0)`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd C:/wt/marks/app && npm test`

Expected: PASS, including the two new diagram tests and the existing `visualDiagram.test.ts`.

- [ ] **Step 7: Commit**

```bash
cd C:/wt/marks
git add app/src/features/ask/visual/diagramSpec.ts app/src/features/ask/visual/diagramSvg.ts app/src/features/ask/visual/DiagramBlock.tsx app/tests/visualMarks.test.ts
git commit -m "Diagrams: marked nodes glow and offer Go deeper"
```

---

### Task 6: Grounding reads prose marks as their shown text

**Files:**
- Modify: `crates/shodh-rag/src/harness/grounding/claims.rs`
  - `protected_spans` at line 306;
  - `plain` at line 188;
  - tests module at line 810.

**Interfaces:**
- Produces: private `fn mark_spans(text: &str) -> Vec<(usize, usize)>` and
  `fn reduce_marks(text: &str) -> String`.

- [ ] **Step 1: Write the failing tests (append inside `mod tests` in `claims.rs`)**

```rust
    #[test]
    fn a_go_deeper_mark_does_not_split_a_sentence() {
        let answer = "The model uses [[self-attention|Why does its cost grow with the square of the length?]] over all tokens [2].";
        let claims = split_claims(answer);
        assert_eq!(claims.len(), 1, "{claims:?}");
        assert_eq!(claims[0].text, "The model uses self-attention over all tokens.");
        assert_eq!(claims[0].citations, vec![2]);
    }

    #[test]
    fn an_unfinished_mark_is_left_as_text() {
        let answer = "The model uses [[self-attention|Why does";
        let claims = split_claims(answer);
        assert_eq!(claims.len(), 1, "{claims:?}");
        assert!(claims[0].text.contains("[[self-attention|Why does"));
    }
```

- [ ] **Step 2: Commit the failing tests on the branch and let CI show the failure**

Rust is not built on this machine (Global Constraints). Push the branch after Task 6 Step 4
and confirm in CI that these two tests pass. They fail before Step 3 because `?` inside the
mark ends the sentence, and `plain` keeps the mark syntax.

- [ ] **Step 3: Protect marks and reduce them to their shown text**

In `claims.rs`:

1. Add a cached regex next to the other statics:

```rust
static TEXT_MARK: OnceLock<Option<Regex>> = OnceLock::new();

/// `[[shown|question]]`: a "go deeper" mark the app renders as `shown`
/// (app/src/features/ask/visual/marks.ts uses the same pattern).
fn text_mark() -> Option<&'static Regex> {
    cached(&TEXT_MARK, r"\[\[([^\[\]|\n]{1,80})\|([^\[\]\n]{0,400})\]\]")
}

/// Byte spans of complete marks.
fn mark_spans(text: &str) -> Vec<(usize, usize)> {
    text_mark()
        .map(|re| re.find_iter(text).map(|m| (m.start(), m.end())).collect())
        .unwrap_or_default()
}

/// Each complete mark replaced by its shown text.
fn reduce_marks(text: &str) -> String {
    match text_mark() {
        Some(re) => re.replace_all(text, |c: &regex::Captures| c[1].trim().to_string()).into_owned(),
        None => text.to_string(),
    }
}
```

2. In `protected_spans`, after `spans.extend(markers.iter().map(|m| (m.start, m.end)));`, add:

```rust
    spans.extend(mark_spans(text));
```

Also update the doc comment above `protected_spans` to read "inline code, inline math,
citation markers and go-deeper marks."

3. In `plain`, change the first line to:

```rust
    let stripped = strip_markers(&reduce_marks(text));
```

- [ ] **Step 4: Commit**

```bash
cd C:/wt/marks
git add crates/shodh-rag/src/harness/grounding/claims.rs
git commit -m "Grounding: go-deeper marks never split a sentence and read as their shown text"
```

---

### Task 7: The visual-first prompt

**Files:**
- Modify: `crates/shodh-rag/src/harness/profile.rs`
  - the `diagram_guide!` macro at lines 154–169;
  - `ASSISTANT_INSTRUCTIONS` from line 178, specifically the "How answers are displayed"
    list and the closing paragraph at lines 252–258;
  - tests at line 338.

- [ ] **Step 1: Write the failing test (append inside `mod tests` in `profile.rs`)**

```rust
    #[test]
    fn answers_are_asked_to_lead_with_a_visual_and_mark_what_goes_deeper() {
        let p = AgentProfile::assistant();
        let text = &p.instructions;
        assert!(text.contains("Lead with the visual that fits the question"), "visual-first section");
        assert!(text.contains("\"deeper\""), "the deeper field is documented");
        assert!(text.contains("[[shown text|question]]"), "prose marks are documented");
        assert!(text.contains("at most four"), "the mark budget is stated");
        assert!(DIAGRAM_GUIDE.contains("\"deeper\""), "diagram nodes may carry deeper");
    }
```

- [ ] **Step 2: Document `deeper` on diagram nodes**

In the `diagram_guide!` macro text, after the sentence that begins
`Every edge joins declared ids; \"cite\" only a source [n] of this answer.`, insert:

```text
 A node may carry \"deeper\": one short question (under 160 characters) that would explain \
that part on its own; the reader can open it in a window of its own.
```

Mind the macro's string continuation: end the previous line with `\` and keep a leading
space, as the surrounding lines do.

- [ ] **Step 3: Add the visual-first section and the other `deeper` fields**

In `ASSISTANT_INSTRUCTIONS`:

1. In item 4 (charts), after `\"series\": [{\"key\": \"score\", \"label\": \"Score\"}]`,
   change the series example to
   `\"series\": [{\"key\": \"score\", \"label\": \"Score\", \"deeper\": \"What drove the jump in 2024?\"}]`.

2. After item 10 (derivations), add item 11:

```text
11. \"Go deeper\" marks: a \"deeper\" question may also go on a chart series, a plot or simulation \
parameter, a derivation step or a symbol entry, and in text as [[shown text|question]] (for example \
\"It uses [[self-attention|Why does self-attention cost grow with the square of the length?]]\"). \
Mark only what a newcomer would not know and the answer does not already explain, at most four \
marks in an answer; each question must make sense on its own.
```

3. Before the final paragraph that begins `Use them when they make an explanation clearer`,
   insert:

```text
Lead with the visual that fits the question, and let the text caption and connect it: how \
something works or flows as a ```diagram; how quantities relate as a ```plot or ```chart; change \
over time as a ```simulation or a timeline diagram; a formula as a ```derivation with \
```symbols; a comparison as a table or a quadrant diagram. One main visual per answer, two at \
most. Facts, definitions and writing stay plain text.
```

Every string in this step contains no snake_case word, so
`instructions_name_only_allowed_tools` still passes.

- [ ] **Step 4: Commit**

```bash
cd C:/wt/marks
git add crates/shodh-rag/src/harness/profile.rs
git commit -m "Prompt: lead with the visual that fits; document go-deeper marks"
```

---

### Task 8: Verify, push, PR

- [ ] **Step 1: Run the frontend unit suite once more**

Run: `cd C:/wt/marks/app && npm test`

Expected: PASS (all tests).

- [ ] **Step 2: Push and open the PR**

```bash
cd C:/wt/marks
git push -u origin feat/visual-marks
gh pr create -R varun29ankuS/shodhRAG --base master --head feat/visual-marks \
  --title "Visual explanations part 1: visual-first answers and go-deeper marks" \
  --body-file C:/wt/marks/.pr-body.md
```

Write `.pr-body.md` first. It covers what changed, the spec link, the Review Focus items and
how each is tested, and the CDP check below. End it with the line
`🤖 Generated with [Claude Code](https://claude.com/claude-code)`. Do not commit the file.

- [ ] **Step 3: CI**

Wait for every check. In particular:

- **Frontend Typecheck + Tests:** covers the `.tsx` changes.
- **Rust tests:** covers Task 6 and Task 7.

Any failure is fixed at its cause. Never weaken an assertion.

- [ ] **Step 4: Check in the real app (CDP, demo profile)**

This runs after CI builds an installer or exe for the branch. Use the demo profile only,
never the user's data.

1. Start the app with `SHODH_DATA_DIR` set to the demo profile and
   `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333`.
2. Ask: "Explain how a transformer layer processes a sentence."
3. Over CDP, check:
   - the answer has a `diagram` with at least one `[data-deeper="true"]` node, or a
     "Go deeper" chip or mark;
   - no side thread exists yet: no `[role="dialog"]` focus pop-out, and the conversation's
     thread list is empty;
   - clicking a glowing node and then "Go deeper" opens the pop-out. Its first user turn is
     exactly the node's prepared question, and an answer starts streaming;
   - closing and clicking the same mark again reopens the same thread with one user turn,
     not two (Review Focus 4).
4. This uses about 2 LLM requests from the free quota.

Record the results in the PR description.

- [ ] **Step 5: Merge when every check is green**

The user authorised merging when CI is green ("merge when green"). Then delete the worktree:
`git worktree remove C:/wt/marks`.
