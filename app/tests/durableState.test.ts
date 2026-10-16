/**
 * The Library's sources and the setup state survive a force-quit: the
 * backend's copy on disk wins over localStorage, which the WebView may not
 * have written. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/durableState.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { reconcileFirstRun, reconcileSources } from '../src/lib/durableState.ts';
import { parseStoredSources, serializeSources } from '../src/features/library/sources.ts';
import type { LibrarySource } from '../src/features/library/sources.ts';
import { hasSavedFirstRun, readFirstRun, FIRST_RUN_KEYS } from '../src/features/setup/firstRun.ts';

function source(id: string, status: LibrarySource['status'] = 'ready'): LibrarySource {
  return {
    id,
    name: id,
    path: `C:/papers/${id}`,
    type: 'documents',
    fileCount: 12,
    indexedAt: '2026-10-12T20:00:00Z',
    status,
    selected: true,
  };
}

test('after a force-quit lost localStorage, the sources come back from disk', () => {
  const onDisk = JSON.parse(serializeSources([source('papers')]));
  const { sources, save } = reconcileSources({ kind: 'value', value: onDisk }, [], false);
  assert.deepEqual(sources.map(s => s.id), ['papers']);
  assert.equal(save, false);
});

test('a source being indexed when the app was killed comes back interrupted', () => {
  const onDisk = JSON.parse(serializeSources([source('papers', 'indexing')]));
  const { sources } = reconcileSources({ kind: 'value', value: onDisk }, [], false);
  assert.equal(sources[0].status, 'interrupted');
});

test('sources only an older build saved are moved to disk once', () => {
  const local = parseStoredSources(serializeSources([source('papers')]));
  assert.deepEqual(reconcileSources({ kind: 'none' }, local, false), { sources: local, save: true });
  // Nothing anywhere: nothing to save.
  assert.deepEqual(reconcileSources({ kind: 'none' }, [], false), { sources: [], save: false });
});

test('a source added while the copy was read is kept and saved', () => {
  const onDisk = JSON.parse(serializeSources([source('old')]));
  const local = [source('old'), source('new')];
  assert.deepEqual(reconcileSources({ kind: 'value', value: onDisk }, local, true), { sources: local, save: true });
});

test('an unreadable copy on disk is replaced by the local list', () => {
  const local = [source('papers')];
  assert.deepEqual(reconcileSources({ kind: 'failed' }, local, false), { sources: local, save: true });
});

function memoryStore(entries: Record<string, string> = {}) {
  const map = new Map(Object.entries(entries));
  return { getItem: (k: string) => map.get(k) ?? null, setItem: (k: string, v: string) => void map.set(k, v) };
}

test('a finished setup saved on disk is not started again when localStorage was lost', () => {
  const lost = memoryStore();
  assert.equal(hasSavedFirstRun(lost), false);
  const local = readFirstRun(lost);
  assert.equal(local.status, 'pending', 'what a fresh install would show');
  const { state, save, saved } = reconcileFirstRun({ kind: 'value', value: { status: 'completed', step: 'done' } }, local, false);
  assert.deepEqual(state, { status: 'completed', step: 'done' });
  assert.equal(save, false);
  assert.equal(saved, true, 'the disk copy is a saved state');
});

test('setup progress only localStorage holds is moved to disk; a fresh install saves nothing yet', () => {
  const store = memoryStore({ [FIRST_RUN_KEYS.status]: 'pending', [FIRST_RUN_KEYS.step]: 'model' });
  assert.equal(hasSavedFirstRun(store), true);
  const local = readFirstRun(store);
  assert.deepEqual(reconcileFirstRun({ kind: 'none' }, local, true), { state: { status: 'pending', step: 'model' }, save: true, saved: true });
  const fresh = readFirstRun(memoryStore());
  // Nothing saved anywhere: whether setup opens waits for the index (see firstRunStart).
  assert.deepEqual(reconcileFirstRun({ kind: 'none' }, fresh, false), { state: fresh, save: false, saved: false });
  assert.equal(reconcileFirstRun({ kind: 'failed' }, fresh, false).saved, false);
  // A malformed copy on disk is ignored.
  assert.deepEqual(reconcileFirstRun({ kind: 'value', value: { status: 'bogus' } }, local, true).state, local);
  // The previous onboarding's key counts as saved.
  assert.equal(hasSavedFirstRun(memoryStore({ [FIRST_RUN_KEYS.legacyCompleted]: 'true' })), true);
});
