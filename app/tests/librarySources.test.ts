/**
 * Library source persistence tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/librarySources.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  groupSourcesByKind,
  parseStoredSources,
  progressPercent,
  readIndexingResult,
  serializeSources,
} from '../src/features/library/sources.ts';

const stored = (over: Record<string, unknown> = {}) => ({
  id: 'folder-1',
  name: 'Contracts',
  path: 'C:\\Contracts',
  type: 'documents',
  fileCount: 12,
  indexedAt: '2026-10-01T10:00:00.000Z',
  status: 'ready',
  selected: true,
  ...over,
});

test('unreadable storage yields no sources', () => {
  assert.deepEqual(parseStoredSources(null), []);
  assert.deepEqual(parseStoredSources(''), []);
  assert.deepEqual(parseStoredSources('{not json'), []);
  assert.deepEqual(parseStoredSources('{"id":"x"}'), []);
});

test('malformed and duplicate entries are dropped', () => {
  const raw = JSON.stringify([stored(), stored(), { id: 'no-path' }, null, 'x', stored({ id: 'folder-2', path: 'D:\\Docs', name: '' })]);
  const sources = parseStoredSources(raw);
  assert.deepEqual(sources.map(s => s.id), ['folder-1', 'folder-2']);
  assert.equal(sources[1].name, 'D:\\Docs');
});

test('a source saved mid-index is marked interrupted', () => {
  const [source] = parseStoredSources(JSON.stringify([stored({ status: 'indexing', progress: 40, currentFile: 'a.pdf' })]));
  assert.equal(source.status, 'interrupted');
  assert.equal(source.progress, undefined);
  assert.equal(source.currentFile, undefined);
});

test('unknown statuses become ready; failures survive a round trip', () => {
  const failures = [{ file: 'C:\\Contracts\\scan.pdf', reason: 'No text layer' }, { file: '', reason: 'x' }];
  const [source] = parseStoredSources(JSON.stringify([stored({ status: 'weird', failures })]));
  assert.equal(source.status, 'ready');
  assert.deepEqual(source.failures, [failures[0]]);
  const again = parseStoredSources(serializeSources([{ ...source, progress: 50, currentFile: 'x', processedCount: 3 }]));
  assert.deepEqual(again, [source]);
});

test('serialisation leaves out live progress', () => {
  const json = serializeSources([{ ...parseStoredSources(JSON.stringify([stored()]))[0], progress: 10, currentFile: 'a', processedCount: 1 }]);
  assert.ok(!json.includes('progress'));
  assert.ok(!json.includes('currentFile'));
  assert.ok(!json.includes('processedCount'));
});

test('reads snake_case indexing results with failures', () => {
  assert.deepEqual(
    readIndexingResult({
      files_processed: 41,
      total_chunks: 900,
      failed_files: ['C:\\a.pdf'],
      failures: [{ file: 'C:\\a.pdf', reason: 'Encrypted' }],
      duration: 12,
    }),
    { filesProcessed: 41, totalChunks: 900, failures: [{ file: 'C:\\a.pdf', reason: 'Encrypted' }] },
  );
});

test('falls back to failed_files paths and tolerates junk', () => {
  assert.deepEqual(readIndexingResult({ files_processed: 1, failed_files: ['x.pdf', 3, ''] }).failures, [{ file: 'x.pdf', reason: '' }]);
  assert.deepEqual(readIndexingResult(null), { filesProcessed: 0, totalChunks: 0, failures: [] });
  assert.equal(readIndexingResult({ filesProcessed: 7 }).filesProcessed, 7);
});

test('progress is clamped and rounded', () => {
  assert.equal(progressPercent({ progress: 49.6 }), 50);
  assert.equal(progressPercent({ progress: 140 }), 100);
  assert.equal(progressPercent({ progress: -3 }), 0);
  assert.equal(progressPercent({}), 0);
});

test('source kinds: only kinds with sources are shown', () => {
  assert.deepEqual(groupSourcesByKind([]), []);
  const sources = parseStoredSources(JSON.stringify([stored(), stored({ id: 'folder-2' })]));
  const groups = groupSourcesByKind(sources);
  assert.deepEqual(groups.map(g => [g.id, g.label, g.sources.length]), [['folders', 'Folders', 2]]);
});
