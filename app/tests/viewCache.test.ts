/**
 * Each view's last data, kept in memory for the session so a revisit renders
 * it at once while it refreshes. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/viewCache.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { VIEW_CACHE_KEYS, clearViewCache, dropViewCache, readViewCache, writeViewCache } from '../src/lib/viewCache.ts';

test('a view reads back the last value written for it, and nothing before the first load', () => {
  clearViewCache();
  assert.equal(readViewCache(VIEW_CACHE_KEYS.slackStatus), null);
  writeViewCache(VIEW_CACHE_KEYS.slackStatus, { state: 'connected' });
  writeViewCache(VIEW_CACHE_KEYS.slackStatus, { state: 'paused' });
  assert.deepEqual(readViewCache(VIEW_CACHE_KEYS.slackStatus), { state: 'paused' });
  assert.equal(readViewCache(VIEW_CACHE_KEYS.telegramStatus), null, 'keys are separate');
});

test('"not loaded" never replaces the last good data', () => {
  clearViewCache();
  writeViewCache(VIEW_CACHE_KEYS.auditStats, { total: 3 });
  writeViewCache(VIEW_CACHE_KEYS.auditStats, null);
  writeViewCache(VIEW_CACHE_KEYS.auditStats, undefined);
  assert.deepEqual(readViewCache(VIEW_CACHE_KEYS.auditStats), { total: 3 });
});

test('falsy loaded values are kept', () => {
  clearViewCache();
  writeViewCache(VIEW_CACHE_KEYS.modelPicker, 0);
  assert.equal(readViewCache(VIEW_CACHE_KEYS.modelPicker), 0);
});

test('dropping one view forgets only that view', () => {
  clearViewCache();
  writeViewCache(VIEW_CACHE_KEYS.tasksSnapshot, { tasks: [], events: [] });
  writeViewCache(VIEW_CACHE_KEYS.slackStatus, { state: 'connected' });
  dropViewCache(VIEW_CACHE_KEYS.tasksSnapshot);
  assert.equal(readViewCache(VIEW_CACHE_KEYS.tasksSnapshot), null);
  assert.deepEqual(readViewCache(VIEW_CACHE_KEYS.slackStatus), { state: 'connected' });
});

test('clearing stored data forgets every view', () => {
  writeViewCache(VIEW_CACHE_KEYS.tasksSnapshot, { tasks: [], events: [] });
  clearViewCache();
  for (const key of Object.values(VIEW_CACHE_KEYS)) assert.equal(readViewCache(key), null, key);
});

test('every cache key names a distinct view', () => {
  const keys = Object.values(VIEW_CACHE_KEYS);
  assert.equal(new Set(keys).size, keys.length);
});

function source(path: string): string {
  return readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');
}

test('views that load on mount start from their last data', () => {
  for (const [path, key] of [
    ['features/connectors/useSlackStatus.ts', 'slackStatus'],
    ['features/connectors/useTelegramStatus.ts', 'telegramStatus'],
    ['features/modelPicker/modelApi.ts', 'modelPicker'],
  ] as const) {
    assert.match(source(path), new RegExp(`useCachedState<\w+>\(VIEW_CACHE_KEYS\.${key}\)`), path);
  }
  // Statuses keep arriving while Connectors is closed; the cache follows them.
  assert.match(source('features/connectors/useSlackStatus.ts'), /statusFeed \?\?= onSlackStatus\(next => writeViewCache\(VIEW_CACHE_KEYS\.slackStatus, next\)\)/);
  assert.match(source('features/connectors/useTelegramStatus.ts'), /statusFeed \?\?= onTelegramStatus\(next => writeViewCache\(VIEW_CACHE_KEYS\.telegramStatus, next\)\)/);
  const tasks = source('features/tasks/TasksStore.tsx');
  // A write while Tasks is closed drops the snapshot, so a revisit never shows changed records.
  assert.match(tasks, /listen\('calendar-changed', \(\) => dropViewCache\(VIEW_CACHE_KEYS\.tasksSnapshot\)\)/);
  assert.match(tasks, /useReducer\(storeReducer, null, cachedStore\)/);
  assert.match(tasks, /writeViewCache<TasksSnapshot>\(VIEW_CACHE_KEYS\.tasksSnapshot, \{ tasks: t, events: e \}\)/);
  assert.match(tasks, /useState\(\(\) => readViewCache<TasksSnapshot>\(VIEW_CACHE_KEYS\.tasksSnapshot\) === null\)/);
  const audit = source('features/audit/AuditView.tsx');
  assert.match(audit, /peekTarget\('audit'\) \? null : readViewCache<FirstPage>\(VIEW_CACHE_KEYS\.auditFirstPage\)/);
  assert.match(audit, /if \(isUnfiltered\(filters\)\) writeViewCache<FirstPage>/);
});

test('the cache stays in memory: nothing in it is written to storage', () => {
  const cache = source('lib/viewCache.ts');
  assert.doesNotMatch(cache, /localStorage|sessionStorage|indexedDB|invoke\(/);
  const hook = source('hooks/useCachedState.ts');
  assert.doesNotMatch(hook, /localStorage|sessionStorage|indexedDB|invoke\(/);
  assert.match(source('App-SplitView.tsx'), /onSourcesCleared=\{\(\) => \{\s*clearViewCache\(\);/);
});
