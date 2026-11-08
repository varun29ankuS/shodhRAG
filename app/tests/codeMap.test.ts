/**
 * Code mode's code map words (src/features/codeIntel/codeMapFormat.ts).
 *   node --experimental-strip-types --test app/tests/codeMap.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  codeMapChipLabel,
  codeMapDetails,
  codeMapTone,
  readCodeMapView,
  sinceText,
  symbolRow,
} from '../src/features/codeIntel/codeMapFormat.ts';
import type { CodeMapView } from '../src/features/codeIntel/api.ts';

const NOW = new Date('2026-11-08T12:00:00Z');

function view(extra: Partial<CodeMapView> = {}): CodeMapView {
  return {
    workspaceId: 'ws',
    sourceId: 'code-map:ab',
    name: 'shodhRAG',
    state: 'idle',
    paused: false,
    watching: true,
    map: {
      files: 1079,
      symbols: 2341,
      references: 90210,
      languages: [['typescript', 300], ['rust', 700], ['tsx', 79]],
      completeAt: '2026-11-08T11:57:00Z',
      updatedAt: '2026-11-08T11:57:00Z',
      partial: false,
    },
    lastRun: { listed: 1100, read: 3, unchanged: 1097, removed: 1, skipped: 0, millis: 840, stopped: false, partial: false },
    ...extra,
  };
}

test('the chip says whether the map is up to date and how much it holds', () => {
  assert.equal(codeMapChipLabel(view()), 'Code map: up to date · 2,341 symbols');
  assert.equal(codeMapTone(view()), 'good');
  assert.equal(codeMapChipLabel(view({ state: 'building', done: 120, total: 900 })), 'Code map: 120 of 900 files');
  assert.equal(codeMapTone(view({ state: 'building', done: 1, total: 2 })), 'busy');
  assert.equal(codeMapChipLabel(view({ state: 'building', done: 0, total: 0 })), 'Code map: listing files');
  assert.equal(codeMapChipLabel(view({ state: 'building', paused: true, done: 3, total: 9 })), 'Code map: paused');
  assert.equal(codeMapTone(view({ state: 'building', paused: true })), 'warn');
  assert.equal(codeMapChipLabel(view({ state: 'queued' })), 'Code map: updating · 2,341 symbols');
  assert.equal(codeMapChipLabel(view({ state: 'queued', map: null })), 'Code map: starting');
  assert.equal(codeMapChipLabel(view({ state: 'idle', map: null })), 'Code map: not built');
  assert.equal(codeMapChipLabel(view({ state: 'failed', error: 'disk full' })), 'Code map: not working');
  assert.equal(codeMapTone(view({ state: 'failed' })), 'bad');
});

test('the details say what is indexed, when, and what the last check did', () => {
  const lines = codeMapDetails(view(), NOW);
  assert.equal(lines[0], 'Indexed 1,079 files (Rust 700, TypeScript 300, TSX 79): 2,341 symbols, 90,210 uses.');
  assert.equal(lines[1], 'Last change 3 min ago.');
  assert.equal(lines[2], 'The last check read 3 changed files and dropped 1 deleted file (840 ms).');
  const quiet = codeMapDetails(view({ lastRun: { listed: 9, read: 0, unchanged: 9, removed: 0, skipped: 0, millis: 12, stopped: false, partial: false } }), NOW);
  assert.ok(quiet.includes('The last check found no changes (12 ms).'));
  assert.deepEqual(codeMapDetails(view({ map: null, lastRun: null }), NOW), ['Nothing indexed yet.']);
  const failed = codeMapDetails(view({ state: 'failed', error: 'disk full' }), NOW);
  assert.equal(failed[failed.length - 1], 'Error: disk full');
});

test('times read in plain words', () => {
  assert.equal(sinceText('2026-11-08T11:59:50Z', NOW), 'just now');
  assert.equal(sinceText('2026-11-08T10:00:00Z', NOW), '2 hours ago');
  assert.equal(sinceText('2026-11-04T10:00:00Z', NOW), 'on 4 Nov');
  assert.equal(sinceText('nonsense', NOW), null);
  assert.equal(sinceText(null, NOW), null);
});

test('only a code map view is read as one, and symbol rows name kind and place', () => {
  assert.ok(readCodeMapView(view()));
  assert.equal(readCodeMapView({ sourceId: 'x', name: 'y', state: 'gone' }), null);
  assert.equal(readCodeMapView('x'), null);
  assert.deepEqual(
    symbolRow({ name: 'open', kind: 'method', path: 'src/store.rs', line: 12, endLine: 20, container: 'Store', module: 'app::store', signature: 'pub fn open()', exported: true, language: 'rust' }),
    { title: 'method Store::open', where: 'src/store.rs:12' },
  );
});

test('a map that waits says why', () => {
  assert.equal(
    codeMapChipLabel(view({ state: 'waiting', reason: 'otherIndexing', done: 12, total: 900 })),
    'Code map: waiting for other indexing (12 of 900 files)',
  );
  assert.equal(codeMapChipLabel(view({ state: 'waiting', reason: 'paused', paused: true })), 'Code map: paused');
  assert.equal(codeMapTone(view({ state: 'waiting', reason: 'paused' })), 'warn');
  assert.ok(readCodeMapView({ sourceId: 'x', name: 'y', state: 'waiting' }));
});
