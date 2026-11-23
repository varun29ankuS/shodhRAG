/**
 * Library → What's new: the section's states, its rows and their words, and the
 * layout rules its component keeps. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/whatsNew.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  BUCKET_LABEL,
  EMPTY_COPY,
  GROUP_AFTER,
  VISIBLE_ROWS,
  bucketOf,
  digestTitle,
  firstRows,
  foundLabel,
  pathOf,
  rowCount,
  rowsOf,
  sectionState,
  unseenIds,
  watchLine,
} from '../src/features/whatsNew/model.ts';
import type { FeedItem } from '../src/features/whatsNew/types.ts';

const read = (path: string) => readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');

/** Monday 23 Nov 2026, 15:00 local time. */
const NOW = new Date(2026, 10, 23, 15, 0);

function at(day: number, hour: number, minute = 0): string {
  return new Date(2026, 10, day, hour, minute).toISOString();
}

function file(id: string, foundAt: string, opts: { source?: string; folder?: string; seen?: boolean; more?: number } = {}): FeedItem {
  const folder = opts.folder ?? 'Research Papers';
  const data: Record<string, unknown> = { sourceId: opts.source ?? 's1', path: `C:/library/${folder}/${id}`, folder };
  if (opts.more) data.more = opts.more;
  return {
    id,
    kind: 'file',
    sourceRef: `c:/library/${folder}/${id}`.toLowerCase(),
    jobId: `job-${opts.source ?? 's1'}`,
    title: id,
    detail: folder,
    foundAt,
    postedRunId: 'run-1',
    seenAt: opts.seen ? at(23, 14) : null,
    dismissedAt: null,
    link: null,
    data,
  };
}

test('loading is never shown as empty', () => {
  assert.equal(sectionState({ loaded: false, error: null, items: 0 }), 'loading');
  assert.equal(sectionState({ loaded: true, error: null, items: 0 }), 'empty');
  assert.equal(sectionState({ loaded: true, error: 'x', items: 0 }), 'error');
  assert.equal(sectionState({ loaded: false, error: 'x', items: 0 }), 'error');
  assert.equal(sectionState({ loaded: true, error: 'x', items: 2 }), 'list', 'the last list stays, with the error over it');
  assert.equal(sectionState({ loaded: true, error: null, items: 2 }), 'list');
});

test('the empty copy says where new files come from', () => {
  assert.equal(EMPTY_COPY, 'Nothing new yet. New files in your Library folders show up here.');
});

test('buckets follow local days and a Monday week', () => {
  assert.equal(bucketOf(at(23, 9), NOW), 'today');
  assert.equal(bucketOf(at(23, 0, 0), NOW), 'today');
  // Monday is the first day of the week: Sunday is earlier.
  assert.equal(bucketOf(at(22, 23, 59), NOW), 'earlier');
  const wednesday = new Date(2026, 10, 25, 10, 0);
  assert.equal(bucketOf(at(23, 9), wednesday), 'this_week');
  assert.equal(bucketOf(at(24, 9), wednesday), 'this_week');
  assert.equal(bucketOf(at(22, 9), wednesday), 'earlier');
  assert.equal(bucketOf('not a time', NOW), 'earlier');
  assert.deepEqual(BUCKET_LABEL, { today: 'Today', this_week: 'Earlier this week', earlier: 'Earlier' });
});

test('found times read as a clock today, a weekday this week, a date before', () => {
  const wednesday = new Date(2026, 10, 25, 10, 0);
  assert.equal(foundLabel(at(25, 9, 5), wednesday), '09:05');
  assert.equal(foundLabel(at(23, 10, 41), wednesday), 'Mon 10:41');
  assert.equal(foundLabel(at(12, 8), wednesday), '12 Nov');
});

test('digest titles match the backend', () => {
  assert.equal(digestTitle(1, 'Papers'), '1 new file in Papers');
  assert.equal(digestTitle(3, 'Papers'), '3 new files in Papers');
});

test('a few files of a folder stay one row each, newest first', () => {
  const items = [file('c.pdf', at(23, 11)), file('b.pdf', at(23, 10)), file('a.pdf', at(23, 9))];
  const buckets = rowsOf(items, NOW);
  assert.equal(buckets.length, 1);
  assert.equal(buckets[0].bucket, 'today');
  assert.deepEqual(buckets[0].rows.map(r => r.title), ['c.pdf', 'b.pdf', 'a.pdf']);
  assert.deepEqual(buckets[0].rows.map(r => r.kind), ['file', 'file', 'file']);
  assert.equal(buckets[0].rows[0].meta, 'Research Papers · 11:00');
});

test('more than three files of one folder in a day are one group row', () => {
  const busy = Array.from({ length: GROUP_AFTER + 1 }, (_, i) => file(`d${i}.csv`, at(23, 9, 10 - i), { folder: 'Datasets', source: 's2' }));
  const items = [file('kv-cache-survey.pdf', at(23, 10)), ...busy];
  const rows = rowsOf(items, NOW)[0].rows;
  assert.deepEqual(rows.map(r => r.kind), ['file', 'file_group']);
  assert.equal(rows[1].title, '4 new files in Datasets');
  assert.equal(rows[1].items.length, 4);
  // Another day of the same folder is its own row.
  const yesterday = rowsOf([...busy, file('old.csv', at(22, 9), { folder: 'Datasets', source: 's2' })], NOW);
  assert.deepEqual(yesterday.map(b => b.bucket), ['today', 'earlier']);
});

test('files beyond the sync cap are counted in their group', () => {
  const rows = rowsOf([file('a.pdf', at(23, 9), { more: 36 }), file('b.pdf', at(23, 9))], NOW)[0].rows;
  assert.equal(rows.length, 1);
  assert.equal(rows[0].title, '38 new files in Research Papers');
  assert.equal(rows[0].more, 36);
});

test('a row is new until it is seen', () => {
  const items = [file('a.pdf', at(23, 9)), file('b.pdf', at(23, 8), { seen: true })];
  const rows = rowsOf(items, NOW)[0].rows;
  assert.deepEqual(rows.map(r => r.isNew), [true, false]);
  assert.deepEqual(unseenIds(items), ['a.pdf']);
});

test('the first rows keep their buckets, and the count covers all', () => {
  const items = [
    file('a.pdf', at(23, 12)),
    file('b.pdf', at(23, 11)),
    file('c.pdf', at(23, 10)),
    file('d.pdf', at(22, 9), { source: 's3', folder: 'Notes' }),
    file('e.pdf', at(22, 8), { source: 's4', folder: 'Drafts' }),
    file('f.pdf', at(21, 8), { source: 's5', folder: 'Old' }),
  ];
  const buckets = rowsOf(items, NOW);
  assert.equal(rowCount(buckets), 6);
  const first = firstRows(buckets, VISIBLE_ROWS);
  assert.equal(rowCount(first), 5);
  assert.deepEqual(first.map(b => b.bucket), ['today', 'earlier']);
  assert.equal(VISIBLE_ROWS, 5);
});

test('a row opens the file as spelled on disk', () => {
  assert.equal(pathOf(file('a.pdf', at(23, 9))), 'C:/library/Research Papers/a.pdf');
  assert.equal(pathOf({ ...file('a.pdf', at(23, 9)), data: {} }), null);
});

test('folder switches say their state', () => {
  assert.equal(watchLine({ on: true, held: false }), 'Shows new files');
  assert.equal(watchLine({ on: false, held: false }), 'Off');
  assert.equal(watchLine({ on: true, held: true }), 'Stopped after failing 3 times');
});

// The component keeps the layout rules (audit rules 1 and 7, "things stay where they were").

const section = read('features/whatsNew/WhatsNewSection.tsx');

test('the section reserves its height and expands only on click', () => {
  assert.match(section, /minHeight: VISIBLE_ROWS \* ROW_HEIGHT_PX/);
  assert.match(section, /onClick=\{\(\) => setShowAll\(v => !v\)\}/);
  assert.match(section, /Show all \(\$\{total\}\)/);
});

test('no button in the section wraps', () => {
  assert.doesNotMatch(section, /flex-wrap/);
  assert.match(section, /const ROW_ACTION = cn\(\s*'[^']*whitespace-nowrap/);
  const buttons = section.match(/<button[\s\S]*?>/g) ?? [];
  assert.ok(buttons.length > 0);
  for (const button of buttons) assert.match(button, /className=\{ROW_ACTION\}/, button);
});

test('file names are plain text', () => {
  assert.doesNotMatch(section, /ReactMarkdown|dangerouslySetInnerHTML/);
});

test('the section carries the keys the navigation reveals', () => {
  assert.match(section, /agentItemKey\('library-section', 'whats_new'\)/);
  assert.match(section, /agentItemKey\('whats-new', first\.id\)/);
  assert.match(section, /agentItemKey\('whats-new', `folder:\$\{folder\.sourceId\}`\)/);
  const library = read('features/library/LibraryView.tsx');
  assert.match(library, /target\.section === 'whats_new'/);
  assert.match(library, /agentItemKey\('whats-new', target\.item\)/);
});

test('resuming a folder from the Inbox turns its watch on again', () => {
  const inbox = read('features/inbox/InboxButton.tsx');
  assert.match(inbox, /item\.data\.resume === 'folder_watch'/);
  assert.match(inbox, /whatsNewApi\.setFolderWatch\(sourceId, true\)/);
});

test('the commands are invoked by their literal names', () => {
  const api = read('features/whatsNew/api.ts');
  for (const command of ['whats_new_list', 'whats_new_mark_seen', 'whats_new_dismiss', 'folder_watch_set']) {
    assert.match(api, new RegExp(`invoke<[^>]+>\\('${command}'`), command);
  }
});
