/**
 * Document cache tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/lruCache.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { LruCache } from '../src/features/ask/viewer/lruCache.ts';

function makeCache(maxEntries: number, maxBytes: number) {
  const disposed: string[] = [];
  const cache = new LruCache<string>({ maxEntries, maxBytes, dispose: (value, key) => disposed.push(`${key}=${value}`) });
  return { cache, disposed };
}

test('evicts the least recently used entry when over the entry count', () => {
  const { cache, disposed } = makeCache(2, 1000);
  cache.set('a', 'A', 1);
  cache.set('b', 'B', 1);
  assert.equal(cache.get('a'), 'A'); // a is now most recent
  cache.set('c', 'C', 1);
  assert.deepEqual(cache.keys(), ['a', 'c']);
  assert.deepEqual(disposed, ['b=B']);
});

test('evicts by total bytes, oldest first, until under budget', () => {
  const { cache, disposed } = makeCache(10, 100);
  cache.set('a', 'A', 40);
  cache.set('b', 'B', 40);
  cache.set('c', 'C', 50);
  assert.deepEqual(cache.keys(), ['b', 'c']);
  assert.equal(cache.bytes, 90);
  cache.set('d', 'D', 95);
  assert.deepEqual(cache.keys(), ['d']);
  assert.deepEqual(disposed, ['a=A', 'b=B', 'c=C']);
});

test('peek does not change recency; get does', () => {
  const { cache } = makeCache(2, 100);
  cache.set('a', 'A', 1);
  cache.set('b', 'B', 1);
  assert.equal(cache.peek('a'), 'A');
  cache.set('c', 'C', 1);
  assert.equal(cache.has('a'), false);
});

test('a leased entry is skipped by eviction; the next unleased entry goes instead', () => {
  const { cache, disposed } = makeCache(1, 100);
  cache.set('a', 'A', 10);
  const lease = cache.acquire('a')!;
  cache.set('b', 'B', 10);
  assert.equal(cache.has('a'), true);
  assert.deepEqual(disposed, ['b=B']);
  lease.release();
  lease.release(); // idempotent
  assert.equal(cache.size, 1);
  assert.deepEqual(disposed, ['b=B']);
});

test('over budget while everything is leased, then shrinks on release', () => {
  const { cache, disposed } = makeCache(1, 100);
  const a = cache.setAndAcquire('a', 'A', 10);
  const b = cache.setAndAcquire('b', 'B', 10);
  assert.equal(cache.size, 2);
  assert.deepEqual(disposed, []);
  a.release();
  assert.deepEqual(cache.keys(), ['b']);
  assert.deepEqual(disposed, ['a=A']);
  b.release();
  assert.deepEqual(disposed, ['a=A']);
});

test('setAndAcquire hands over a value larger than the whole budget, disposed on release', () => {
  const { cache, disposed } = makeCache(4, 100);
  const lease = cache.setAndAcquire('big', 'BIG', 500);
  assert.equal(lease.value, 'BIG');
  assert.deepEqual(disposed, []);
  lease.release();
  assert.equal(cache.has('big'), false);
  assert.deepEqual(disposed, ['big=BIG']);
});

test('replacing a key disposes the old value (deferred while leased)', () => {
  const { cache, disposed } = makeCache(4, 100);
  cache.set('a', 'A1', 10);
  const lease = cache.acquire('a')!;
  cache.set('a', 'A2', 20);
  assert.equal(cache.peek('a'), 'A2');
  assert.equal(cache.bytes, 20);
  assert.deepEqual(disposed, []);
  lease.release();
  assert.deepEqual(disposed, ['a=A1']);
});

test('re-setting the same value only updates its size', () => {
  const { cache, disposed } = makeCache(4, 100);
  cache.set('a', 'A', 10);
  cache.set('a', 'A', 30);
  assert.equal(cache.bytes, 30);
  assert.deepEqual(disposed, []);
});

test('delete without dispose transfers ownership to the caller', () => {
  const { cache, disposed } = makeCache(4, 100);
  cache.set('a', 'A', 10);
  assert.equal(cache.delete('a', false), true);
  assert.equal(cache.bytes, 0);
  assert.deepEqual(disposed, []);
  assert.equal(cache.delete('a'), false);
});

test('clear disposes everything, leased entries on release', () => {
  const { cache, disposed } = makeCache(4, 100);
  cache.set('a', 'A', 10);
  cache.set('b', 'B', 10);
  const lease = cache.acquire('b')!;
  cache.clear();
  assert.equal(cache.size, 0);
  assert.equal(cache.bytes, 0);
  assert.deepEqual(disposed, ['a=A']);
  lease.release();
  assert.deepEqual(disposed, ['a=A', 'b=B']);
});

test('rejects nonsensical bounds', () => {
  assert.throws(() => new LruCache<string>({ maxEntries: 0, maxBytes: 1, dispose: () => {} }), RangeError);
  assert.throws(() => new LruCache<string>({ maxEntries: 1, maxBytes: 0, dispose: () => {} }), RangeError);
});
