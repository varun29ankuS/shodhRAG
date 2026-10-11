/**
 * Source labelling tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/searchResults.test.ts
 *
 * Calendar items are indexed as `calendar://task/<uuid>`; chips and badges
 * must show the item's title, never the id.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { appRecordKind, recordLabel, sourceLabel, toSearchHits } from '../src/features/ask/searchResults.ts';

const TASK = 'calendar://task/ae887fee-eaa5-4c05-b53d-9b3e9905edef';

test('recognises in-app record sources', () => {
  assert.equal(appRecordKind(TASK), 'task');
  assert.equal(appRecordKind('calendar://event/42'), 'event');
  assert.equal(appRecordKind('calendar://other/1'), 'calendar');
  assert.equal(appRecordKind('note://n1'), 'note');
  assert.equal(appRecordKind('c:/docs/calendar.pdf'), null);
  assert.equal(appRecordKind('https://example.com/calendar://task/1'), null);
});

test('record labels use the title, or say untitled', () => {
  assert.equal(recordLabel('task', '  File GST return '), 'Task: File GST return');
  assert.equal(recordLabel('event', ''), 'Event (untitled)');
  assert.equal(recordLabel('note', null), 'Note (untitled)');
});

test('chip label is the file stem for files and the record label for records', () => {
  assert.equal(sourceLabel({ sourceFile: 'c:/docs/acme_msa.pdf', fileName: 'acme_msa.pdf' }), 'acme_msa');
  assert.equal(sourceLabel({ sourceFile: TASK, fileName: 'Task: File GST return' }), 'Task: File GST return');
  // Answers saved before records carried titles hold the bare id.
  assert.equal(sourceLabel({ sourceFile: TASK, fileName: 'ae887fee-eaa5-4c05-b53d-9b3e9905edef' }), 'Task (untitled)');
});

test('search results label calendar hits by their citation title', () => {
  const [hit] = toSearchHits([{ sourceFile: TASK, text: 'Task: "File GST return".', citation: { title: 'File GST return' } }]);
  assert.equal(hit.fileName, 'Task: File GST return');
  assert.equal(sourceLabel(hit), 'Task: File GST return');
});

test('web sources are labelled by title, falling back to the site', async () => {
  const { sourceLabel, webHost } = await import('../src/features/ask/searchResults.ts');
  assert.equal(webHost('https://www.arxiv.org/abs/2404.19756'), 'arxiv.org');
  assert.equal(webHost('not a url'), 'not a url');
  assert.equal(sourceLabel({ sourceFile: 'https://doi.org/10.1/x', fileName: 'Diffusing Blame' }), 'Diffusing Blame');
  assert.equal(sourceLabel({ sourceFile: 'https://www.example.com/a?b=1', fileName: '' }), 'example.com');
});
