/**
 * Command palette matching tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/paletteFilter.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { filterEntries, groupBySection, scoreEntry } from '../src/lib/paletteFilter.ts';

const ORDER = ['Actions', 'Go to', 'Chats', 'Library'];

const ENTRIES = [
  { id: 'new', label: 'New chat', keywords: 'conversation create', section: 'Actions' },
  { id: 'add', label: 'Add folder to Library', keywords: 'source index', section: 'Actions' },
  { id: 'ask', label: 'Ask', keywords: 'chat question', section: 'Go to' },
  { id: 'tasks', label: 'Tasks', keywords: 'todo calendar events', section: 'Go to' },
  { id: 'activity', label: 'Activity', keywords: 'audit usage log', section: 'Go to' },
  { id: 'c1', label: 'Quarterly revenue review', keywords: 'chat', section: 'Chats' },
  { id: 'c2', label: 'Résumé tips', keywords: 'chat', section: 'Chats' },
  { id: 'lib', label: 'Contracts', keywords: 'folder C:/work/contracts', section: 'Library' },
];

const ids = (query: string) => filterEntries(ENTRIES, query, ORDER).map(e => e.id);

test('an empty query keeps every entry in section order', () => {
  assert.deepEqual(ids(''), ENTRIES.map(e => e.id));
  assert.deepEqual(ids('   '), ENTRIES.map(e => e.id));
});

test('label prefix outranks word prefix, which outranks keyword matches', () => {
  // "Tasks" (label prefix) before "Activity" … and keyword-only "calendar" matches still appear.
  assert.equal(ids('ta')[0], 'tasks');
  assert.deepEqual(ids('calendar'), ['tasks']);
  // "chat": label word prefix ("New chat") beats keyword-only matches ("Ask", chats).
  assert.equal(ids('chat')[0], 'new');
});

test('every query word must match', () => {
  assert.deepEqual(ids('revenue quarterly'), ['c1']);
  assert.deepEqual(ids('revenue tasks'), []);
});

test('matching ignores case and accents', () => {
  assert.deepEqual(ids('RESUME'), ['c2']);
  assert.deepEqual(ids('résumé'), ['c2']);
});

test('keywords match paths and synonyms without showing them', () => {
  assert.deepEqual(ids('audit'), ['activity']);
  assert.deepEqual(ids('work contracts'), ['lib']);
});

test('ties keep section order, then input order', () => {
  const tied = [
    { id: 'b', label: 'Beta', section: 'Library' },
    { id: 'a', label: 'Beta', section: 'Actions' },
    { id: 'c', label: 'Beta', section: 'Actions' },
  ];
  assert.deepEqual(filterEntries(tied, 'beta', ORDER).map(e => e.id), ['a', 'c', 'b']);
});

test('scores are zero exactly when an entry is excluded', () => {
  assert.equal(scoreEntry(ENTRIES[0], 'zzz'), 0);
  assert.ok(scoreEntry(ENTRIES[0], 'new') > scoreEntry(ENTRIES[0], 'create'));
});

test('grouping keeps first-appearance order of sections', () => {
  const groups = groupBySection(filterEntries(ENTRIES, 'chat', ORDER));
  assert.equal(groups[0].section, 'Actions');
  assert.deepEqual(
    groups.map(g => g.section),
    [...new Set(filterEntries(ENTRIES, 'chat', ORDER).map(e => e.section))],
  );
  assert.equal(groups.reduce((n, g) => n + g.items.length, 0), filterEntries(ENTRIES, 'chat', ORDER).length);
});
