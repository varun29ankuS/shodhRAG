/**
 * One-time hints: dismissed once, remembered, shared by every listener.
 *   node --experimental-strip-types --test app/tests/firstUse.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { FIRST_USE_STORAGE_KEY, createFirstUseStore } from '../src/lib/firstUse.ts';

class MemoryStorage {
  values = new Map<string, string>();
  failWrites = false;
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    if (this.failWrites) throw new Error('QuotaExceededError');
    this.values.set(key, value);
  }
}

test('a dismissed hint stays dismissed across restarts', () => {
  const storage = new MemoryStorage();
  const store = createFirstUseStore(storage);
  assert.equal(store.seen('grounding-chip'), false);
  let heard = 0;
  const unsubscribe = store.subscribe(() => { heard += 1; });
  store.dismiss('grounding-chip');
  store.dismiss('grounding-chip');
  assert.equal(heard, 1, 'listeners hear the first dismissal only');
  unsubscribe();
  assert.equal(store.seen('grounding-chip'), true);
  assert.equal(store.seen('citation-flags'), false);
  const reopened = createFirstUseStore(storage);
  assert.equal(reopened.seen('grounding-chip'), true);
});

test('unreadable records and failing storage fall back to this session', () => {
  const storage = new MemoryStorage();
  storage.values.set(FIRST_USE_STORAGE_KEY, '{oops');
  const store = createFirstUseStore(storage);
  assert.equal(store.seen('citation-flags'), false);
  storage.failWrites = true;
  store.dismiss('citation-flags');
  assert.equal(store.seen('citation-flags'), true);
  storage.values.set(FIRST_USE_STORAGE_KEY, JSON.stringify(['citation-flags', 'unknown-hint', 7]));
  assert.equal(createFirstUseStore(storage).seen('citation-flags'), true);
  assert.equal(createFirstUseStore(null).seen('grounding-chip'), false);
});
