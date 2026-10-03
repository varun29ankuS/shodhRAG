/**
 * Focus pop-out: side-thread store (validation, list operations, metadata
 * merge, local storage, replay history).
 *   node --experimental-strip-types --test app/tests/focusThreads.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  LOCAL_KEY_PREFIX,
  MAX_LOCAL_THREADS,
  METADATA_KEY,
  appendTurn,
  createLocalThreadStore,
  metadataWithThreads,
  metadataWithoutThreads,
  readThreads,
  removeThread,
  repliesLabel,
  sideSessionKey,
  threadHistory,
  threadsForMessage,
  threadsFromMetadata,
  upsertThread,
} from '../src/features/focus/threadStore.ts';
import type { KeyValueStorage } from '../src/features/focus/threadStore.ts';
import type { FocusThread } from '../src/features/focus/focusTypes.ts';

function thread(id: string, parent: string | null = 'm1', conversationId = 'c1'): FocusThread {
  return {
    id,
    anchor: { conversationId, parentMessageId: parent, target: { kind: 'mermaid', label: 'Flow', source: 'graph TD; A-->B' } },
    turns: [],
    createdAt: '2026-10-03T10:00:00.000Z',
    updatedAt: '2026-10-03T10:00:00.000Z',
  };
}

class MemoryStorage implements KeyValueStorage {
  map = new Map<string, string>();
  getItem(key: string) { return this.map.has(key) ? this.map.get(key)! : null; }
  setItem(key: string, value: string) { this.map.set(key, value); }
  removeItem(key: string) { this.map.delete(key); }
}

test('add, append and list threads', () => {
  let list: FocusThread[] = [];
  list = upsertThread(list, thread('a'));
  list = upsertThread(list, thread('b', 'm2'));
  const a = appendTurn(list[0], { id: 'u1', role: 'user', content: 'why?', timestamp: '2026-10-03T10:01:00.000Z' });
  list = upsertThread(list, appendTurn(a, { id: 'r1', role: 'assistant', content: 'because', timestamp: '2026-10-03T10:02:00.000Z' }));
  assert.deepEqual(list.map(t => t.id), ['b', 'a']);
  assert.equal(threadsForMessage(list, 'm1').length, 1);
  assert.equal(threadsForMessage(list, 'm1')[0].turns.length, 2);
  assert.equal(threadsForMessage(list, 'm1')[0].updatedAt, '2026-10-03T10:02:00.000Z');
  assert.equal(repliesLabel(threadsForMessage(list, 'm1')[0]), '1 reply about Flow');
  assert.deepEqual(removeThread(list, 'a').map(t => t.id), ['b']);
});

test('appending a turn with an existing id replaces it', () => {
  const t = appendTurn(thread('a'), { id: 'r1', role: 'assistant', content: 'draft', timestamp: 't1' });
  const u = appendTurn(t, { id: 'r1', role: 'assistant', content: 'final', timestamp: 't2' });
  assert.equal(u.turns.length, 1);
  assert.equal(u.turns[0].content, 'final');
});

test('validation: malformed threads and turns are dropped, never thrown', () => {
  const good = { ...thread('ok'), turns: [{ id: 'u', role: 'user', content: 'q', timestamp: 't' }, { id: 'bad', role: 'robot', content: 'x', timestamp: 't' }] };
  const parsed = readThreads([
    good,
    null,
    42,
    { id: 'no-anchor' },
    { ...thread('bad-target'), anchor: { conversationId: 'c1', parentMessageId: 'm1', target: { kind: 'video', label: 'v' } } },
    { ...thread('no-label'), anchor: { conversationId: 'c1', parentMessageId: 'm1', target: { kind: 'equation', label: '', tex: 'x' } } },
    good,
  ]);
  assert.deepEqual(parsed.map(t => t.id), ['ok']);
  assert.equal(parsed[0].turns.length, 1);
  assert.deepEqual(readThreads('not a list'), []);
  assert.deepEqual(readThreads(undefined), []);
});

test('metadata: threads merge in without touching other keys and come back out', () => {
  const metadata = { model: 'm', searchQueriesUsed: ['q'] };
  const merged = metadataWithThreads(metadata, [thread('a')]);
  assert.equal(merged?.model, 'm');
  assert.ok(Array.isArray(merged?.[METADATA_KEY]));
  assert.deepEqual(threadsFromMetadata(merged).map(t => t.id), ['a']);
  assert.deepEqual(metadataWithoutThreads(merged), metadata);
  assert.equal(metadataWithThreads(undefined, []), undefined);
  assert.equal(metadataWithoutThreads({ [METADATA_KEY]: [] }), undefined);
  assert.deepEqual(threadsFromMetadata({ [METADATA_KEY]: 'garbage' }), []);
});

test('local store: persists per conversation and round-trips', () => {
  const storage = new MemoryStorage();
  const store = createLocalThreadStore(storage);
  assert.equal(store.save('c1', [thread('a', null), thread('x', null, 'c2')]), true);
  assert.deepEqual(store.list('c1').map(t => t.id), ['a']);
  assert.deepEqual(store.list('c2'), []);
  assert.ok(storage.map.has(`${LOCAL_KEY_PREFIX}c1`));
  store.save('c1', []);
  assert.equal(storage.map.has(`${LOCAL_KEY_PREFIX}c1`), false);
});

test('local store: keeps only the newest threads', () => {
  const store = createLocalThreadStore(new MemoryStorage());
  const many = Array.from({ length: MAX_LOCAL_THREADS + 5 }, (_, i) => thread(`t${i}`, null));
  store.save('c1', many);
  const kept = store.list('c1');
  assert.equal(kept.length, MAX_LOCAL_THREADS);
  assert.equal(kept[0].id, 't5');
});

test('local store: corrupt or unavailable storage reads empty and reports failed writes', () => {
  const storage = new MemoryStorage();
  storage.setItem(`${LOCAL_KEY_PREFIX}c1`, '{not json');
  assert.deepEqual(createLocalThreadStore(storage).list('c1'), []);
  storage.setItem(`${LOCAL_KEY_PREFIX}c1`, JSON.stringify({ not: 'a list' }));
  assert.deepEqual(createLocalThreadStore(storage).list('c1'), []);

  const throwing: KeyValueStorage = {
    getItem() { throw new Error('denied'); },
    setItem() { throw new Error('QuotaExceededError'); },
    removeItem() { throw new Error('denied'); },
  };
  const store = createLocalThreadStore(throwing);
  assert.deepEqual(store.list('c1'), []);
  assert.equal(store.save('c1', [thread('a', null)]), false);
  assert.doesNotThrow(() => store.clear('c1'));

  const none = createLocalThreadStore(null);
  assert.deepEqual(none.list('c1'), []);
  assert.equal(none.save('c1', []), false);
});

test('replay history: thread turns win, main turns fill the rest', () => {
  const main = Array.from({ length: 8 }, (_, i) => ({ role: (i % 2 ? 'assistant' : 'user') as 'user' | 'assistant', content: `m${i}` }));
  const own = Array.from({ length: 4 }, (_, i) => ({ role: (i % 2 ? 'assistant' : 'user') as 'user' | 'assistant', content: `t${i}` }));
  const h = threadHistory(main, own, 10);
  assert.equal(h.length, 10);
  assert.deepEqual(h.slice(-4).map(t => t.content), ['t0', 't1', 't2', 't3']);
  assert.equal(h[0].content, 'm2');
  assert.deepEqual(threadHistory(main, Array.from({ length: 12 }, () => ({ role: 'user' as const, content: 'x' })), 10).length, 10);
  assert.deepEqual(threadHistory([{ role: 'user', content: '  ' }], [], 10), []);
});

test('side session key: only safe characters and within the id limit', () => {
  const key = sideSessionKey('conv-1:2/3', 'thread#9');
  assert.match(key, /^[A-Za-z0-9_-]+$/);
  assert.ok(sideSessionKey('c'.repeat(300), 't'.repeat(300)).length <= 200);
});
