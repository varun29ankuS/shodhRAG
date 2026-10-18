/**
 * Indexing progress moves only the card of the source it names, while that source
 * is indexing; a failed or finished source is never repainted by a late event.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/indexingProgress.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { endsRun, progressApplies, withIndexingProgress } from '../src/features/library/indexingProgress.ts';
import type { IndexingProgressPayload } from '../src/features/library/indexingProgress.ts';

const event = (space: string | undefined, percentage: number, file = 'paper.pdf'): IndexingProgressPayload => ({
  space_id: space,
  current_file: file,
  processed_files: 3,
  total_files: 10,
  percentage,
});

const source = (id: string, status: string) => ({ id, status, fileCount: 0 });

test('only the indexing source the event names moves', () => {
  const a = source('folder-a', 'indexing');
  const b = source('folder-b', 'indexing');
  assert.equal(progressApplies(a, event('folder-a', 30)), true);
  assert.equal(progressApplies(b, event('folder-a', 30)), false);
  const moved = withIndexingProgress(a, event('folder-a', 30.4), undefined);
  assert.deepEqual(moved, { ...a, status: 'indexing', progress: 30, currentFile: 'paper.pdf', currentStep: undefined, fileCount: 10, processedCount: 3 });
  assert.equal(withIndexingProgress(b, event('folder-a', 30), undefined), b);
});

test('a source that failed or finished is not repainted by a late event', () => {
  const failed = source('folder-a', 'error');
  const ready = source('folder-a', 'ready');
  assert.equal(withIndexingProgress(failed, event('folder-a', 100, 'Completed'), undefined), failed);
  assert.equal(withIndexingProgress(ready, event('folder-a', 40), undefined), ready);
});

test('an event without a source id still moves the indexing source', () => {
  const a = source('folder-a', 'indexing');
  assert.equal(withIndexingProgress(a, event(undefined, 100, 'Completed'), undefined).status, 'ready');
});

test('only the end of the run being indexed clears it', () => {
  assert.equal(endsRun(event('folder-a', 100, 'Completed'), 'folder-a'), true);
  assert.equal(endsRun(event('folder-b', 100, 'Completed'), 'folder-a'), false);
  assert.equal(endsRun(event('folder-a', 100, 'paper.pdf'), 'folder-a'), false);
  assert.equal(endsRun(event(undefined, 100, 'Completed'), null), true);
});

test('the listener is registered once and disposed safely', () => {
  const app = readFileSync(new URL('../src/App-SplitView.tsx', import.meta.url), 'utf8');
  const start = app.indexOf("listen<IndexingProgressPayload>('indexing-progress'");
  assert.ok(start > 0);
  const effect = app.slice(start, app.indexOf('}, []);', start) + 7);
  assert.match(effect, /if \(disposed\) fn\(\);/);
  assert.doesNotMatch(app, /\}, \[currentlyIndexing\]\);/);
});
