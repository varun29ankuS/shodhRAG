/**
 * Each view's last data, kept in memory for the session so a revisit renders
 * it at once while it refreshes, and never shown as current when it is not.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/viewCache.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  VIEW_CACHE_KEYS,
  clearViewCache,
  dropViewCache,
  dropViewCacheOn,
  readViewCache,
  refreshViewCache,
  viewCacheTicket,
  writeViewCache,
  writeViewCacheIfCurrent,
} from '../src/lib/viewCache.ts';

const flush = () => new Promise<void>(resolve => setImmediate(resolve));

/** A promise the test settles. */
function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  let reject: (error: unknown) => void = () => {};
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

test('a view reads back the last value written for it, and nothing before the first load', () => {
  clearViewCache();
  assert.equal(readViewCache(VIEW_CACHE_KEYS.slackStatus), null);
  writeViewCache(VIEW_CACHE_KEYS.slackStatus, { state: 'connected' });
  writeViewCache(VIEW_CACHE_KEYS.slackStatus, { state: 'paused' });
  assert.deepEqual(readViewCache(VIEW_CACHE_KEYS.slackStatus), { state: 'paused' });
  assert.equal(readViewCache(VIEW_CACHE_KEYS.telegramStatus), null, 'keys are separate');
});

test('"not loaded" never replaces the last good data; falsy loaded values are kept', () => {
  clearViewCache();
  writeViewCache(VIEW_CACHE_KEYS.auditStats, { total: 3 });
  writeViewCache(VIEW_CACHE_KEYS.auditStats, null);
  writeViewCache(VIEW_CACHE_KEYS.auditStats, undefined);
  assert.deepEqual(readViewCache(VIEW_CACHE_KEYS.auditStats), { total: 3 });
  writeViewCache(VIEW_CACHE_KEYS.modelPicker, 0);
  assert.equal(readViewCache(VIEW_CACHE_KEYS.modelPicker), 0);
});

test('dropping one view forgets only that view; clearing forgets all', () => {
  clearViewCache();
  writeViewCache(VIEW_CACHE_KEYS.auditFirstPage, { rows: [], total: 0 });
  writeViewCache(VIEW_CACHE_KEYS.slackStatus, { state: 'connected' });
  dropViewCache(VIEW_CACHE_KEYS.auditFirstPage);
  assert.equal(readViewCache(VIEW_CACHE_KEYS.auditFirstPage), null);
  assert.deepEqual(readViewCache(VIEW_CACHE_KEYS.slackStatus), { state: 'connected' });
  clearViewCache();
  for (const key of Object.values(VIEW_CACHE_KEYS)) assert.equal(readViewCache(key), null, key);
});

test('a failed refresh drops the kept data, so it is not shown as current next time', async () => {
  clearViewCache();
  writeViewCache(VIEW_CACHE_KEYS.slackStatus, { state: 'connected' });
  const result = await refreshViewCache(VIEW_CACHE_KEYS.slackStatus, () => Promise.reject(new Error('backend down')));
  assert.equal(result.ok, false);
  assert.equal(readViewCache(VIEW_CACHE_KEYS.slackStatus), null);
  const ok = await refreshViewCache(VIEW_CACHE_KEYS.slackStatus, async () => ({ state: 'paused' }));
  assert.deepEqual(ok, { ok: true, value: { state: 'paused' }, error: null });
  assert.deepEqual(readViewCache(VIEW_CACHE_KEYS.slackStatus), { state: 'paused' });
});

test('a refresh that started before a drop does not write its older result back', async () => {
  clearViewCache();
  const read = deferred<{ version: number }>();
  const refreshing = refreshViewCache(VIEW_CACHE_KEYS.auditStats, () => read.promise);
  dropViewCache(VIEW_CACHE_KEYS.auditStats); // the data changed while the read was in flight
  read.resolve({ version: 1 });
  const result = await refreshing;
  assert.deepEqual(result, { ok: true, value: { version: 1 }, error: null }, 'the view still gets what it read');
  assert.equal(readViewCache(VIEW_CACHE_KEYS.auditStats), null, 'but it is not kept as the last data');

  const ticket = viewCacheTicket(VIEW_CACHE_KEYS.auditStats);
  assert.equal(writeViewCacheIfCurrent(ticket, { version: 2 }), true);
  clearViewCache();
  assert.equal(writeViewCacheIfCurrent(ticket, { version: 3 }), false, 'clearing stored data counts as a drop');
});

/** A change event the test fires, with a registration it settles. */
function eventSource() {
  const listeners: (() => void)[] = [];
  let registration = deferred<void>();
  let calls = 0;
  const subscribe = (onChange: () => void) => {
    calls += 1;
    listeners.push(onChange);
    registration = deferred<void>();
    return registration.promise;
  };
  return {
    subscribe,
    fire: () => listeners.forEach(listener => listener()),
    registered: () => registration.resolve(),
    failed: () => registration.reject(new Error('listen failed')),
    calls: () => calls,
  };
}

test('watched data is dropped when its source announces a change, and kept again after the next read', async () => {
  // A key of its own: watches last for the module's life.
  const key = VIEW_CACHE_KEYS.tasksSnapshot;
  const source = eventSource();
  dropViewCacheOn(key, source.subscribe);
  writeViewCache(key, { tasks: ['a'] });
  assert.equal(readViewCache(key), null, 'nothing is kept until the listener is registered');
  source.registered();
  await flush();
  writeViewCache(key, { tasks: ['a'] });
  assert.deepEqual(readViewCache(key), { tasks: ['a'] });

  // A refresh in flight when the change is announced keeps nothing.
  const read = deferred<{ tasks: string[] }>();
  const refreshing = refreshViewCache(key, () => read.promise);
  source.fire();
  assert.equal(readViewCache(key), null, 'the change drops the kept list');
  read.resolve({ tasks: ['a'] });
  await refreshing;
  assert.equal(readViewCache(key), null, 'the older read does not bring it back');

  dropViewCacheOn(key, source.subscribe);
  assert.equal(source.calls(), 1, 'registered once');
});

test('a failed registration keeps nothing and is tried again by the next caller', async () => {
  const key = VIEW_CACHE_KEYS.auditFirstPage;
  const source = eventSource();
  dropViewCacheOn(key, source.subscribe);
  source.failed();
  await flush();
  writeViewCache(key, { rows: [], total: 1 });
  assert.equal(readViewCache(key), null, 'not kept without its listener');
  dropViewCacheOn(key, source.subscribe);
  assert.equal(source.calls(), 2, 'tried again');
  source.registered();
  await flush();
  writeViewCache(key, { rows: [], total: 1 });
  assert.deepEqual(readViewCache(key), { rows: [], total: 1 });
});

test('the cache stays in memory: nothing in it is written to storage', () => {
  for (const path of ['lib/viewCache.ts', 'hooks/useCachedState.ts']) {
    const source = readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');
    assert.doesNotMatch(source, /localStorage|sessionStorage|indexedDB|invoke\(/, path);
  }
});
