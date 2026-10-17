/**
 * Library source persistence tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/librarySources.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  applyFolderSync,
  groupSourcesByKind,
  indexedFileCount,
  indexedNotice,
  parseStoredSources,
  progressPercent,
  readIndexingResult,
  scannedPagesStep,
  serializeSources,
  syncedFolders,
  withPathCase,
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
  const json = serializeSources([{
    ...parseStoredSources(JSON.stringify([stored()]))[0],
    progress: 10,
    currentFile: 'a',
    processedCount: 1,
    currentStep: 'Reading scanned pages: 3 of 9',
  }]);
  assert.ok(!json.includes('progress'));
  assert.ok(!json.includes('currentFile'));
  assert.ok(!json.includes('processedCount'));
  assert.ok(!json.includes('currentStep'));
  assert.ok(!json.includes('scanned'));
});

test('a scan being read says which page; other progress events say nothing', () => {
  assert.equal(scannedPagesStep({ scanned_page: 12, scanned_pages: 240 }), 'Reading scanned pages: 12 of 240');
  assert.equal(scannedPagesStep({ scanned_page: 1, scanned_pages: 1200 }), 'Reading scanned pages: 1 of 1,200');
  assert.equal(scannedPagesStep({}), undefined);
  assert.equal(scannedPagesStep({ scanned_page: null, scanned_pages: 4 }), undefined);
  assert.equal(scannedPagesStep({ scanned_page: 5, scanned_pages: 4 }), undefined);
  assert.equal(scannedPagesStep({ scanned_page: 0, scanned_pages: 4 }), undefined);
  assert.equal(scannedPagesStep({ scanned_page: 1.5, scanned_pages: 4 }), undefined);
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
    { filesProcessed: 41, filesUnchanged: 0, totalChunks: 900, failures: [{ file: 'C:\\a.pdf', reason: 'Encrypted' }] },
  );
});

test('files already indexed count toward the source and its notice', () => {
  const adopted = readIndexingResult({ files_processed: 0, files_unchanged: 12, total_chunks: 0, failed_files: [], failures: [], duration: 40 });
  assert.equal(adopted.filesUnchanged, 12);
  assert.equal(indexedFileCount(adopted), 12);
  assert.equal(indexedNotice(adopted), '12 files already indexed');
  const mixed = readIndexingResult({ files_processed: 1, files_unchanged: 11 });
  assert.equal(indexedFileCount(mixed), 12);
  assert.equal(indexedNotice(mixed), 'Indexed 1 file · 11 already indexed');
  assert.equal(indexedNotice(readIndexingResult({ files_processed: 3 })), 'Indexed 3 files');
  assert.equal(indexedNotice(readIndexingResult({ files_processed: 0 })), 'Indexed 0 files');
  assert.equal(indexedNotice(readIndexingResult({ files_unchanged: 1 })), '1 file already indexed');
});

test('falls back to failed_files paths and tolerates junk', () => {
  assert.deepEqual(readIndexingResult({ files_processed: 1, failed_files: ['x.pdf', 3, ''] }).failures, [{ file: 'x.pdf', reason: '' }]);
  assert.deepEqual(readIndexingResult(null), { filesProcessed: 0, filesUnchanged: 0, totalChunks: 0, failures: [] });
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

test('only finished sources are kept in sync', () => {
  const sources = parseStoredSources(JSON.stringify([
    stored(),
    stored({ id: 'folder-2', path: 'D:\\Docs', status: 'error' }),
    stored({ id: 'folder-3', path: 'E:\\Papers', status: 'indexing' }),
  ]));
  assert.deepEqual(syncedFolders(sources), [{ id: 'folder-1', path: 'C:\\Contracts' }]);
});

test('a sync updates the count, failures and time of its source only', () => {
  const [source] = parseStoredSources(JSON.stringify([stored({ failures: [{ file: 'C:\\Contracts\\old.pdf', reason: 'x' }] })]));
  const at = '2026-10-07T10:00:00.000Z';
  const synced = applyFolderSync(source, {
    sourceId: 'folder-1', at, indexed: 2, removed: 1, files: 14,
    failures: [{ file: 'C:\\Contracts\\scan.pdf', reason: 'No text' }],
  });
  assert.equal(synced.fileCount, 13);
  assert.equal(synced.indexedAt, at);
  assert.deepEqual(synced.failures, [{ file: 'C:\\Contracts\\scan.pdf', reason: 'No text' }]);
  // Nothing changed: the time stays; fixed files leave the failures.
  const quiet = applyFolderSync(source, { sourceId: 'folder-1', at, indexed: 0, removed: 0, files: 12, failures: [] });
  assert.equal(quiet.indexedAt, source.indexedAt);
  assert.equal(quiet.failures, undefined);
  assert.equal(quiet.fileCount, 12);
  // Another source's sync, or one during indexing, changes nothing.
  assert.equal(applyFolderSync(source, { sourceId: 'other', at, files: 1 }), source);
  const indexing = { ...source, status: 'indexing' as const };
  assert.equal(applyFolderSync(indexing, { sourceId: 'folder-1', at, files: 1 }), indexing);
});

test('a failed sync is shown and cleared by the next one, never stored', () => {
  const [source] = parseStoredSources(JSON.stringify([stored()]));
  const at = '2026-10-07T10:00:00.000Z';
  const failed = applyFolderSync(source, { sourceId: 'folder-1', at, error: 'D:\\Docs is not available' });
  assert.equal(failed.syncError, 'D:\\Docs is not available');
  assert.equal(failed.fileCount, source.fileCount);
  assert.ok(!serializeSources([failed]).includes('syncError'));
  const recovered = applyFolderSync(failed, { sourceId: 'folder-1', at, indexed: 0, removed: 0, files: 12, failures: [] });
  assert.equal(recovered.syncError, undefined);
});

test('a sync that could not read part of the folder counts what it found and says what it could not read', () => {
  const [source] = parseStoredSources(JSON.stringify([stored()]));
  const at = '2026-10-07T10:00:00.000Z';
  const partial = applyFolderSync(source, {
    sourceId: 'folder-1', at, indexed: 1, removed: 0, files: 13, failures: [],
    error: 'C:\\Contracts\\private (Access is denied.) could not be read; what the index holds of it was left as it was',
  });
  assert.equal(partial.fileCount, 13);
  assert.equal(partial.indexedAt, at);
  assert.match(partial.syncError ?? '', /private \(Access is denied\.\) could not be read/);
  const readable = applyFolderSync(partial, { sourceId: 'folder-1', at, indexed: 0, removed: 0, files: 13, failures: [], error: null });
  assert.equal(readable.syncError, undefined);
});

test('a path saved lowercased gets its case on disk back, keeping its identity', () => {
  const [saved] = parseStoredSources(JSON.stringify([stored({ name: 'papers', path: 'c:\\research\\papers' })]));
  const fixed = withPathCase(saved, 'C:\\Research\\papers');
  assert.equal(fixed.path, 'C:\\Research\\papers');
  assert.equal(fixed.name, 'papers', 'the name follows the folder');
  assert.equal(fixed.id, saved.id);

  const renamed = withPathCase({ ...saved, name: 'PAPERS' }, 'C:\\Research\\Papers');
  assert.equal(renamed.name, 'Papers', 'a name that is the folder in another case follows too');

  const custom = withPathCase({ ...saved, name: 'Reading list' }, 'C:\\Research\\papers');
  assert.equal(custom.name, 'Reading list', 'a name of its own is kept');

  // Separators and a trailing slash are spelling, not identity.
  assert.equal(withPathCase({ ...saved, path: 'c:/research/papers/' }, 'C:\\Research\\papers').path, 'C:\\Research\\papers');

  // Another folder (a short name expanded, a junction followed) is never applied.
  assert.equal(withPathCase(saved, 'C:\\Users\\Varun Sharma\\papers'), saved);
  assert.equal(withPathCase(saved, ''), saved);
  assert.equal(withPathCase(saved, saved.path), saved);
  // A source being indexed is left alone.
  const indexing = { ...saved, status: 'indexing' as const };
  assert.equal(withPathCase(indexing, 'C:\\Research\\papers'), indexing);
});
