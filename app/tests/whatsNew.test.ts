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
  alertLine,
  blockedCopy,
  paperMeta,
  pdfOf,
  LOCAL_ONLY_COPY,
  localDayKey,
  renderedIds,
} from '../src/features/whatsNew/model.ts';
import { CONSENT_TEMPLATE, charCount, cleanWords, consentLine, toggleChip, wordsFromChips } from '../src/features/whatsNew/consent.ts';
import type { AlertView, FeedItem } from '../src/features/whatsNew/types.ts';

const read = (path: string) => readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');

/** Monday 23 Nov 2026, 15:00 local time. */
const NOW = new Date(2026, 10, 23, 15, 0);

function at(day: number, hour: number, minute = 0): string {
  return new Date(2026, 10, day, hour, minute).toISOString();
}

function file(id: string, foundAt: string, opts: { source?: string; folder?: string; seen?: boolean } = {}): FeedItem {
  const folder = opts.folder ?? 'Research Papers';
  const data: Record<string, unknown> = { sourceId: opts.source ?? 's1', path: `C:/library/${folder}/${id}`, folder };
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

test('the empty copy is the spec copy', () => {
  assert.equal(EMPTY_COPY, 'Nothing new yet. New files in your Library folders show up here. You can also turn on alerts for new papers on a topic.');
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
  const day = localDayKey(at(23, 9));
  const moreOf = (sourceId: string, d: string) => (sourceId === 's1' && d === day ? 36 : 0);
  const rows = rowsOf([file('a.pdf', at(23, 9)), file('b.pdf', at(23, 9))], NOW, moreOf)[0].rows;
  assert.equal(rows.length, 1);
  assert.equal(rows[0].title, '38 new files in Research Papers');
  assert.equal(rows[0].more, 36);
});

test('only rows on screen count as seen: a busy day once expanded', () => {
  const busy = Array.from({ length: GROUP_AFTER + 1 }, (_, i) => file(`d${i}.csv`, at(23, 9, 10 - i), { folder: 'Datasets', source: 's2' }));
  const items = [file('kv.pdf', at(23, 10)), ...busy, file('old.pdf', at(20, 9), { source: 's9', folder: 'Old' })];
  const buckets = rowsOf(items, NOW);
  const group = buckets[0].rows.find(r => r.kind === 'file_group');
  assert.ok(group);
  const shown = firstRows(buckets, 2);
  assert.deepEqual(renderedIds(shown, new Set()), ['kv.pdf'], 'a collapsed day and rows past the list are not seen');
  assert.deepEqual(renderedIds(shown, new Set([group.key])), ['kv.pdf', ...busy.map(b => b.id)]);
  assert.equal(localDayKey(new Date(2026, 0, 5, 9).toISOString()), '2026-01-05');
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

test('add to library waits while a save runs', () => {
  assert.match(section, /disabled=\{adding\}/);
  assert.match(section, /if \(adding\) return;/);
});

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

// Paper alerts (PR 3b).

function paper(id: string, foundAt: string, data: Record<string, unknown> = {}): FeedItem {
  return {
    id,
    kind: 'paper',
    sourceRef: `doi:10.1/${id}`,
    jobId: 'alert-1',
    title: `Paper ${id}`,
    detail: null,
    foundAt,
    postedRunId: null,
    seenAt: null,
    dismissedAt: null,
    link: null,
    data: { year: 2026, sources: ['arXiv', 'OpenAlex'], words: 'linear attention', ...data },
  };
}

function alert(state: AlertView['state'], extra: Partial<AlertView> = {}): AlertView {
  return {
    id: 'a1',
    words: 'linear attention',
    consentText: consentLine('linear attention'),
    state,
    lastCheckedAt: null,
    lastSummary: null,
    nextCheckAt: null,
    failedApis: [],
    retryAt: null,
    blockedReason: null,
    ...extra,
  };
}

test('the consent line names all three APIs and the exact words', () => {
  assert.equal(
    consentLine('  linear attention KV cache '),
    'Shodh sends these words: linear attention KV cache to arXiv, OpenAlex and Semantic Scholar to look for new papers. Your files, your library and who you are stay on this computer.',
  );
  assert.match(CONSENT_TEMPLATE, /\{words\}/);
  assert.equal(cleanWords('  a   b '), 'a b');
});

test('ticking chips fills the words and the consent line shows them', () => {
  const words = wordsFromChips(['linear attention', 'kv cache']);
  assert.equal(words, 'linear attention kv cache');
  assert.equal(consentLine(words), CONSENT_TEMPLATE.replace('{words}', 'linear attention kv cache'));
  assert.equal(wordsFromChips([]), '');
});

test('ticking a chip keeps what the user typed', () => {
  assert.equal(toggleChip('my own topic', 'linear attention', true), 'my own topic linear attention');
  assert.equal(toggleChip('linear attention', 'Linear Attention', true), 'linear attention', 'already there');
  assert.equal(toggleChip('my own topic linear attention', 'linear attention', false), 'my own topic');
  assert.equal(toggleChip('linear attentions', 'linear attention', false), 'linear attentions', 'whole phrases only');
  assert.equal(toggleChip('a (b)', '(b)', false), 'a', 'chip words are literal, not a pattern');
});

test('words are cleaned and counted as the backend does', () => {
  assert.equal(cleanWords(' a\u00A0b\u3000c\t'), 'a b c');
  assert.equal(cleanWords('a\uFEFFb'), 'a\uFEFFb', 'not whitespace to Rust either');
  assert.equal(charCount('\u{1F600}ab'), 3, 'code points, as chars().count()');
});

test('papers are rows of their own, never grouped', () => {
  const items = [paper('p1', at(23, 9)), paper('p2', at(23, 9)), paper('p3', at(23, 9)), paper('p4', at(23, 9)), file('a.pdf', at(23, 8))];
  const rows = rowsOf(items, NOW)[0].rows;
  assert.deepEqual(rows.map(r => r.kind), ['paper', 'paper', 'paper', 'paper', 'file']);
  assert.equal(rows[0].meta, '2026 · arXiv, OpenAlex · alert: linear attention');
  assert.equal(paperMeta(paper('p', at(23, 9), { year: null, sources: [] })), 'alert: linear attention');
});

test('only an https PDF the API reported can be added to the library', () => {
  assert.equal(pdfOf(paper('p', at(23, 9), { pdfUrl: 'https://arxiv.org/pdf/2312.06635' })), 'https://arxiv.org/pdf/2312.06635');
  assert.equal(pdfOf(paper('p', at(23, 9), { pdfUrl: 'http://example.com/x.pdf' })), null);
  assert.equal(pdfOf(paper('p', at(23, 9))), null);
});

test('alert lines for every state', () => {
  assert.deepEqual(alertLine(alert('checking')), { word: 'Checking', fact: 'Checking now…', fix: null });
  const retry = alertLine(alert('waiting_retry', { failedApis: ['Semantic Scholar'], retryAt: new Date(2026, 10, 23, 11, 40).toISOString() }));
  assert.deepEqual(retry, { word: 'Retrying', fact: "Semantic Scholar didn't answer; trying again at 11:40", fix: null });
  assert.equal(alertLine(alert('offline')).fact, "No internet connection; Shodh checks again when it's back");
  assert.equal(alertLine(alert('active', { nextCheckAt: new Date(2026, 10, 30, 9, 0).toISOString() })).fact, 'Next check Mon 30 Nov, 09:00');
  assert.deepEqual(alertLine(alert('off')), { word: 'Off', fact: 'Nothing is sent for it', fix: 'turn_on' });
  assert.deepEqual(alertLine(alert('held')), { word: 'Stopped', fact: 'Stopped after failing 3 weeks in a row', fix: 'resume' });
  assert.deepEqual(alertLine(alert('blocked', { blockedReason: 'Local-only mode is on, so nothing may leave this computer.' })), { word: 'Paused', fact: LOCAL_ONLY_COPY, fix: 'privacy' });
});

test('local-only and web access off read differently, and nothing blocked reads as nothing', () => {
  assert.equal(blockedCopy('Local-only mode is on, so nothing may leave this computer.'), LOCAL_ONLY_COPY);
  assert.equal(blockedCopy('Web access for the agent is turned off.'), 'Paper alerts are paused: web access is off, so nothing leaves this computer.');
  assert.equal(blockedCopy(null), null);
  assert.equal(LOCAL_ONLY_COPY, 'Paper alerts are paused: Local-only mode is on, so nothing leaves this computer.');
});

const alertForm = read('features/whatsNew/PaperAlertForm.tsx');

test('the form shows the consent line before Turn on, and only the section creates alerts', () => {
  assert.match(alertForm, /consentLine\(clean\)/);
  assert.doesNotMatch(alertForm, /createAlert\(/, 'the form only hands the words back');
  assert.match(section, /whatsNewApi\.createAlert\(words\)/);
  assert.match(alertForm, /Suggested from your library/);
  assert.match(alertForm, /suggestions && suggestions\.length > 0/, 'no chips without suggestions');
  assert.doesNotMatch(alertForm, /flex-wrap/);
  assert.doesNotMatch(alertForm, /dangerouslySetInnerHTML|ReactMarkdown/);
});

test('resuming a paper alert from the Inbox turns it on again', () => {
  const inbox = read('features/inbox/InboxButton.tsx');
  assert.match(inbox, /item\.data\.resume === 'paper_alert'/);
  assert.match(inbox, /whatsNewApi\.updateAlert\(jobId, \{ enabled: true \}\)/);
});

test('the alert commands are invoked by their literal names', () => {
  const api = read('features/whatsNew/api.ts');
  for (const command of ['paper_alert_create', 'paper_alert_update', 'paper_alert_delete', 'paper_alert_suggestions', 'whats_new_add_to_library']) {
    assert.match(api, new RegExp(`invoke<[^>]+>\\('${command}'`), command);
  }
});

test('the commands are invoked by their literal names', () => {
  const api = read('features/whatsNew/api.ts');
  for (const command of ['whats_new_list', 'whats_new_mark_seen', 'whats_new_dismiss', 'folder_watch_set']) {
    assert.match(api, new RegExp(`invoke<[^>]+>\\('${command}'`), command);
  }
});
