/**
 * "Used N memories" under an answer: the chip's count, the ids kept with the answer (and
 * only the ids), memories read live with a forgotten one shown without its words, and the
 * "Not relevant" state. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/usedMemories.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { AgentEvent } from '../src/features/agent/events.ts';
import {
  MAX_MEMORY_IDS,
  fromPersisted,
  initialTranscript,
  memoryIdsOf,
  reduceTranscript,
  toPersisted,
} from '../src/features/agent/reducer.ts';
import {
  FORGOTTEN_MEMORY,
  chipText,
  coInjected,
  markedNotRelevant,
  parseUsedMemories,
  rowOf,
  shortScope,
} from '../src/features/memory/usedMemories.ts';

function record(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: 'mem-1',
    class: 'Note',
    classLabel: 'Note',
    text: 'Invoices go to Priya',
    subject: null,
    properties: {},
    values: {},
    scope: 'global',
    source: 'settings://memory',
    conversationId: null,
    extractor: 'user',
    confidence: 1,
    validFrom: '2026-03-03T10:00:00Z',
    validTo: null,
    supersededBy: null,
    expiresAt: null,
    createdAt: '2026-03-03T10:00:00Z',
    strength: 1,
    importance: 0.3,
    useCount: 2,
    lastUsedAt: null,
    pinned: false,
    current: true,
    expired: false,
    ...overrides,
  };
}

const memoriesUsed = (ids: unknown): AgentEvent => ({ type: 'memories_used', runId: 'run-1', ids } as AgentEvent);

const noWorkspace = () => undefined;

test('the chip counts the memories and is hidden when there are none', () => {
  assert.equal(chipText(0), null);
  assert.equal(chipText(1), 'Used 1 memory');
  assert.equal(chipText(2), 'Used 2 memories');
  assert.equal(chipText(-1), null);
  assert.equal(chipText(1.5), null);
});

test('the run keeps the ids of the memories it was given, once each', () => {
  const start = initialTranscript('run-1', 1);
  const next = reduceTranscript(start, memoriesUsed(['mem-1', 'mem-2', 'mem-1', '', 7]));
  assert.deepEqual(next.memoryIds, ['mem-1', 'mem-2']);
  // An event of another run, or with no ids, changes nothing.
  assert.equal(reduceTranscript(start, { type: 'memories_used', runId: 'run-2', ids: ['mem-1'] }), start);
  assert.equal(reduceTranscript(start, memoriesUsed([])), start);
  assert.equal(memoryIdsOf(Array.from({ length: 50 }, (_, i) => `m${i}`)).length, MAX_MEMORY_IDS);
  assert.deepEqual(memoryIdsOf('mem-1'), []);
});

test('only the ids are stored with the answer, and they survive a reload', () => {
  let state = initialTranscript('run-1', 1);
  state = reduceTranscript(state, memoriesUsed(['mem-1', 'mem-2']));
  state = reduceTranscript(state, { type: 'text_delta', runId: 'run-1', messageId: 'm1', delta: 'Send them to Priya.' });
  state = reduceTranscript(state, {
    type: 'run_finished',
    runId: 'run-1',
    status: 'completed',
    durationMs: 5,
    error: null,
    providerError: null,
  });
  const stored = JSON.parse(JSON.stringify(toPersisted(state)));
  assert.deepEqual(stored.memoryIds, ['mem-1', 'mem-2']);
  // Nothing but ids: no memory text, class or scope is kept with the message.
  const text = JSON.stringify(stored);
  assert.ok(!text.includes('Invoices'), text);
  const restored = fromPersisted(stored);
  assert.deepEqual(restored?.memoryIds, ['mem-1', 'mem-2']);
  // Malformed ids are dropped on reload; a transcript without them has none.
  assert.deepEqual(fromPersisted({ ...stored, memoryIds: ['mem-1', 3, null] })?.memoryIds, ['mem-1']);
  assert.equal(fromPersisted({ ...stored, memoryIds: 'mem-1' })?.memoryIds, undefined);
  const { memoryIds: _ids, ...without } = stored;
  assert.equal('memoryIds' in (fromPersisted(without) ?? {}), false);
});

test('memories are read live: a forgotten one has no words', () => {
  const used = parseUsedMemories(
    [
      { id: 'mem-1', memory: record(), notRelevant: false },
      { id: 'mem-2', memory: null, notRelevant: false },
    ],
    ['mem-1', 'mem-2', 'mem-3'],
  );
  assert.equal(used.length, 3);
  const [kept, forgotten, missing] = used.map(u => rowOf(u, noWorkspace));
  assert.equal(kept.text, 'Invoices go to Priya');
  assert.equal(kept.classLabel, 'Note');
  assert.ok(kept.since.startsWith('since '), kept.since);
  assert.equal(kept.scope, 'Global');
  assert.equal(kept.canEdit, true);
  for (const gone of [forgotten, missing]) {
    assert.equal(gone.text, null);
    assert.equal(gone.canEdit, false);
    assert.equal(gone.since, '');
    assert.equal(gone.scope, '');
  }
  assert.equal(FORGOTTEN_MEMORY, 'a forgotten memory');
  // A malformed record is treated as forgotten, never shown half-read.
  const [broken] = parseUsedMemories([{ id: 'mem-1', memory: { id: 'mem-1' } }], ['mem-1']);
  assert.equal(broken.memory, null);
  assert.deepEqual(parseUsedMemories('nonsense', ['mem-1']), [{ id: 'mem-1', memory: null, notRelevant: false }]);
});

test('an edited memory shows its old version as replaced, without an editor', () => {
  const [old] = parseUsedMemories(
    [{ id: 'mem-1', memory: record({ current: false, validTo: '2026-04-01T00:00:00Z', supersededBy: 'mem-9' }), notRelevant: false }],
    ['mem-1'],
  );
  const row = rowOf(old, noWorkspace);
  assert.equal(row.replaced, true);
  assert.equal(row.canEdit, false);
  assert.equal(row.text, 'Invoices go to Priya');
});

test('a workspace memory names its workspace', () => {
  const names = (id: string) => (id === 'ws-1' ? 'Thesis' : undefined);
  assert.equal(shortScope('global', names), 'Global');
  assert.equal(shortScope('workspace:ws-1', names), 'Thesis');
  assert.equal(shortScope('workspace:ws-gone', names), 'A deleted workspace');
});

test('Not relevant is shown once recorded and weakens links to the other memories only', () => {
  const used = parseUsedMemories(
    [
      { id: 'mem-1', memory: record(), notRelevant: false },
      { id: 'mem-2', memory: record({ id: 'mem-2', text: 'Gym on Tuesdays' }), notRelevant: true },
    ],
    ['mem-1', 'mem-2'],
  );
  assert.equal(rowOf(used[0], noWorkspace).notRelevant, false);
  assert.equal(rowOf(used[1], noWorkspace).notRelevant, true);
  const marked = markedNotRelevant(used, 'mem-1');
  assert.equal(marked[0].notRelevant, true);
  assert.equal(marked[1].notRelevant, true);
  assert.equal(used[0].notRelevant, false, 'not changed in place');
  assert.deepEqual(coInjected(['mem-1', 'mem-2', 'mem-3'], 'mem-2'), ['mem-1', 'mem-3']);
});
