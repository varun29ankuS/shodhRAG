/**
 * Side questions carry the passages the answers they continue cite, under the
 * same numbers. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/carriedPassages.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  carriedPassages,
  keptCarriedPassages,
  MAX_CARRIED_CHARS,
  MAX_CARRIED_PASSAGE_CHARS,
  MAX_CARRIED_PASSAGES,
  toWire,
} from '../src/features/focus/carriedPassages.ts';
import type { Passage } from '../src/features/agent/reducer.ts';
import { initialTranscript, reduceAll } from '../src/features/agent/reducer.ts';
import type { AgentEvent } from '../src/features/agent/events.ts';

function passage(n: number, file: string, text: string, extra: Partial<Passage> = {}): Passage {
  return { n, file, path: `C:/papers/${file}`, page: '3', heading: null, score: 0.5, text, ...extra };
}

// The answer of a recorded conversation: 32 passages, some cited.
const parentPassages = Array.from({ length: 32 }, (_, i) => passage(i + 1, i % 2 ? 'nsg.pdf' : 'hnsw.pdf', `passage ${i + 1}`));
const parentText =
  'NSG approximates the Monotonic Relative Neighborhood Graph [21]. HNSW keeps the global connectivity [28], ' +
  'and both prune neighbours [12][21].';

test('the passages an anchored answer cites are carried under its numbers', () => {
  const carried = carriedPassages([{ text: parentText, passages: parentPassages }]);
  assert.deepEqual(carried.map(p => p.n), [21, 28, 12]);
  assert.equal(carried[1].text, 'passage 28');
  // Passages the answer does not cite stay behind.
  assert.ok(!carried.some(p => p.n === 1));
});

test('passages cited in the text the question is about come first', () => {
  const carried = carriedPassages([{ text: parentText, passages: parentPassages }], 'connectivity [28] and [12]');
  assert.deepEqual(carried.map(p => p.n), [28, 12, 21]);
});

test("a thread's earlier answers add their own passages; the anchored answer wins a shared number", () => {
  const sideRun = [passage(33, 'diskann.pdf', 'Vamana uses alpha pruning'), passage(21, 'other.pdf', 'stale')];
  const carried = carriedPassages([
    { text: parentText, passages: parentPassages },
    { text: 'Vamana prunes with alpha [33]; see also [21].', passages: sideRun },
  ]);
  assert.deepEqual(carried.map(p => p.n), [21, 28, 12, 33]);
  assert.equal(carried[0].text, 'passage 21', 'the anchored answer keeps its number');
});

test('numbers without a passage, and empty passages, are not carried; every cited one is sent', () => {
  assert.deepEqual(carriedPassages([{ text: 'See [99] and [1].', passages: [passage(1, 'a.pdf', '  ')] }]), []);
  const many = Array.from({ length: 60 }, (_, i) => passage(i + 1, 'a.pdf', `t${i + 1}`));
  const text = many.map(p => `[${p.n}]`).join(' ');
  // Past the backend's count limit too: the numbers it leaves out stay taken in the side run.
  const carried = carriedPassages([{ text, passages: many }]);
  assert.equal(carried.length, 60);
  assert.equal(keptCarriedPassages(carried).length, MAX_CARRIED_PASSAGES);
});

test('the side transcript starts with the passages the backend keeps', () => {
  // 20 passages of 1,500 characters: the text limit keeps the first 16.
  const long = Array.from({ length: 20 }, (_, i) => passage(i + 1, 'a.pdf', `${'x'.repeat(1_499)}${i % 10}`));
  const kept = keptCarriedPassages(long);
  assert.equal(kept.length, Math.floor(MAX_CARRIED_CHARS / 1_500));
  assert.deepEqual(kept.map(p => p.n), long.slice(0, kept.length).map(p => p.n));
  assert.equal(kept[0], long[0], 'a passage within the limits is kept as it is');

  // Nothing after the first passage that does not fit, also a short one.
  const blocked = keptCarriedPassages([
    passage(1, 'a.pdf', 'y'.repeat(MAX_CARRIED_PASSAGE_CHARS)),
    passage(2, 'a.pdf', 'y'.repeat(MAX_CARRIED_PASSAGE_CHARS)),
    passage(3, 'a.pdf', 'y'.repeat(MAX_CARRIED_PASSAGE_CHARS)),
    passage(4, 'a.pdf', 'y'.repeat(MAX_CARRIED_PASSAGE_CHARS)),
    passage(5, 'a.pdf', 'y'.repeat(MAX_CARRIED_PASSAGE_CHARS)),
    passage(6, 'a.pdf', 'y'.repeat(MAX_CARRIED_PASSAGE_CHARS - 1)),
    passage(7, 'a.pdf', 'y'.repeat(2)),
    passage(8, 'a.pdf', 'short'),
  ]);
  assert.deepEqual(blocked.map(p => p.n), [1, 2, 3, 4, 5, 6]);

  // Trimmed, cut per passage as the backend cuts, blanks and repeated numbers skipped.
  const cut = keptCarriedPassages([
    passage(9, 'a.pdf', `  ${'z'.repeat(MAX_CARRIED_PASSAGE_CHARS + 10)}  `),
    passage(9, 'b.pdf', 'repeated'),
    passage(10, 'a.pdf', '   '),
    passage(11, 'a.pdf', ' kept '),
  ]);
  assert.deepEqual(cut.map(p => p.n), [9, 11]);
  assert.equal(Array.from(cut[0].text).length, MAX_CARRIED_PASSAGE_CHARS);
  assert.ok(cut[0].text.endsWith('\u2026'));
  assert.equal(cut[1].text, 'kept');
});

test('characters are counted as the backend counts them, by code point', () => {
  // 2,000 code points each, but 4,000 UTF-16 units for the emoji: counted in units, the
  // limit would keep 6 where the backend keeps 12.
  const emoji = Array.from({ length: 13 }, (_, i) => passage(i + 1, 'a.pdf', '\u{1F600}'.repeat(2_000)));
  assert.equal(keptCarriedPassages(emoji).length, 12);
  const cjk = Array.from({ length: 13 }, (_, i) => passage(i + 1, 'a.pdf', '東'.repeat(2_000)));
  assert.equal(keptCarriedPassages(cjk).length, 12);
  // Cut on a code point, never inside a surrogate pair.
  const [one] = keptCarriedPassages([passage(1, 'a.pdf', '\u{1F600}'.repeat(MAX_CARRIED_PASSAGE_CHARS + 1))]);
  assert.equal(Array.from(one.text).length, MAX_CARRIED_PASSAGE_CHARS);
  assert.ok(!/[\uD800-\uDBFF]\u2026$/.test(one.text));
});

test("the limits are the backend's", () => {
  const rust = readFileSync(new URL('../src-tauri/src/agent_session_commands.rs', import.meta.url), 'utf8');
  const constant = (name: string) => {
    const found = new RegExp(`const ${name}: usize = ([0-9_]+);`).exec(rust);
    assert.ok(found, `${name} is defined`);
    return Number(found[1].replace(/_/g, ''));
  };
  assert.equal(constant('MAX_CARRIED_PASSAGES'), MAX_CARRIED_PASSAGES);
  assert.equal(constant('MAX_CARRIED_CHARS'), MAX_CARRIED_CHARS);
  assert.equal(constant('MAX_CARRIED_PASSAGE_CHARS'), MAX_CARRIED_PASSAGE_CHARS);
});

test('the wire form is what agent_send takes', () => {
  const web = passage(5, 'Example', 'web text', { path: 'https://example.org', web: true, page: null });
  assert.deepEqual(toWire([passage(28, 'hnsw.pdf', 'caption'), web]), [
    { n: 28, file: 'hnsw.pdf', path: 'C:/papers/hnsw.pdf', page: '3', text: 'caption', web: false },
    { n: 5, file: 'Example', path: 'https://example.org', page: null, text: 'web text', web: true },
  ]);
});

test("a side run seeded with the carried passages resolves the side answer's citations", () => {
  const carried = carriedPassages([{ text: parentText, passages: parentPassages }]);
  const run = 'side-1';
  const events: AgentEvent[] = [
    { type: 'run_started', runId: run, sessionId: 's', model: 'm', atMs: 0, warning: null },
    {
      type: 'step_started', runId: run, stepId: 'st', parentStepId: null, tool: 'search_documents',
      label: 'Searching', args: {}, tier: 'read', atMs: 1,
    },
    // The backend numbers the side run's own search after the highest carried number.
    {
      type: 'step_finished', runId: run, stepId: 'st', ok: true, summary: '1 passage', durationMs: 5,
      detail: { passages: [{ n: 29, file: 'mrng.pdf', path: 'C:/papers/mrng.pdf', page: '2', heading: null, score: 0.7, text: 'MRNG edges' }] },
    },
    { type: 'text_delta', runId: run, messageId: 'm', delta: 'MRNG [21] keeps connectivity [28]; its edges [29].' },
  ];
  const state = reduceAll({ ...initialTranscript(run, 0), passages: [...carried] }, events);
  // Every number the side answer cites has its passage (what the citation badges open).
  const numbers = new Set(state.passages.map(p => p.n));
  for (const n of [21, 28, 12, 29]) assert.ok(numbers.has(n), `[${n}] has its passage`);
});

test('a sparse set of parent numbers reaches the backend unchanged', () => {
  // The reported answer cited passages 1, 2, 3, 7 and 8 of its run.
  const spann = [1, 2, 3, 7, 8].map(n => passage(n, 'spann.pdf', `SPANN passage ${n}`));
  const carried = carriedPassages([{ text: 'Closure [1][2]. Index [3]. Recall [7]. Conclusion [8].', passages: spann }]);
  assert.deepEqual(toWire(carried).map(p => p.n), [1, 2, 3, 7, 8]);
  assert.deepEqual(toWire(carried).map(p => p.text), spann.map(p => p.text), 'each number keeps its own passage');
});
