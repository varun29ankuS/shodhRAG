/**
 * Settings → Memory logic: parsing, grouping, strength display, edits. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/memoryModel.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  contentFromEdit,
  editableFields,
  exportFileName,
  groupByClass,
  matchesFilter,
  parseMemories,
  parseMemory,
  propertyLabel,
  relativeTime,
  scopeLabel,
  strengthDisplay,
} from '../src/features/memory/model.ts';
import type { MemoryRecord } from '../src/features/memory/model.ts';
import { parseAppSettings } from '../src/lib/appSettings.ts';

function memory(overrides: Partial<MemoryRecord> = {}): MemoryRecord {
  return {
    id: 'mem-1',
    class: 'Note',
    classLabel: 'Note',
    text: 'Locker code is on the fridge',
    subject: null,
    properties: { noteText: 'Locker code is on the fridge' },
    values: { noteText: ['Locker code is on the fridge'] },
    scope: 'global',
    source: 'settings://memory',
    conversationId: null,
    extractor: 'user',
    confidence: 1,
    validFrom: '2026-09-01T10:00:00Z',
    validTo: null,
    supersededBy: null,
    expiresAt: null,
    createdAt: '2026-09-01T10:00:00Z',
    strength: 0.8,
    importance: 0.4,
    useCount: 0,
    lastUsedAt: null,
    pinned: false,
    current: true,
    expired: false,
    ...overrides,
  };
}

test('strength shows a level, a percentage and pins', () => {
  assert.deepEqual(strengthDisplay(0.834, false), { percent: 83, level: 'strong', label: 'Strong · 83%' });
  assert.deepEqual(strengthDisplay(0.6, false).level, 'strong');
  assert.deepEqual(strengthDisplay(0.59, false).level, 'fading');
  assert.deepEqual(strengthDisplay(0.25, false).level, 'fading');
  assert.deepEqual(strengthDisplay(0.249, false), { percent: 25, level: 'faint', label: 'Faint · 25%' });
  assert.deepEqual(strengthDisplay(0.1, true), { percent: 100, level: 'pinned', label: 'Pinned · does not fade' });
  // Out-of-range and non-finite values are clamped, never shown as > 100 % or NaN.
  assert.equal(strengthDisplay(1.7, false).percent, 100);
  assert.equal(strengthDisplay(-0.2, false).percent, 0);
  assert.equal(strengthDisplay(Number.NaN, false).percent, 0);
});

test('groups follow the class order; pinned, then strongest, then newest first', () => {
  const groups = groupByClass([
    memory({ id: 'n1', class: 'Note', classLabel: 'Note', strength: 0.9 }),
    memory({ id: 'p1', class: 'Preference', classLabel: 'Preference', strength: 0.3 }),
    memory({ id: 'p2', class: 'Preference', classLabel: 'Preference', strength: 0.7 }),
    memory({ id: 'p3', class: 'Preference', classLabel: 'Preference', strength: 0.1, pinned: true }),
    memory({ id: 'x1', class: 'Snippet', classLabel: 'Snippet' }),
    memory({ id: 'd1', class: 'Decision', classLabel: 'Decision' }),
    memory({ id: 'per', class: 'Person', classLabel: 'Person' }),
  ]);
  assert.deepEqual(groups.map(g => g.classId), ['Preference', 'Person', 'Decision', 'Note', 'Snippet']);
  assert.deepEqual(groups.map(g => g.heading), ['Preferences', 'People', 'Decisions', 'Notes', 'Snippet']);
  assert.deepEqual(groups[0].memories.map(m => m.id), ['p3', 'p2', 'p1']);
  const ties = groupByClass([
    memory({ id: 'old', validFrom: '2026-01-01T00:00:00Z' }),
    memory({ id: 'new', validFrom: '2026-06-01T00:00:00Z' }),
  ]);
  assert.deepEqual(ties[0].memories.map(m => m.id), ['new', 'old']);
  assert.deepEqual(groupByClass([]), []);
});

test('parsing accepts the backend shape and drops malformed entries', () => {
  const good = memory();
  assert.deepEqual(parseMemory(JSON.parse(JSON.stringify(good))), good);
  assert.equal(parseMemory({ ...good, strength: 'high' }), null);
  assert.equal(parseMemory({ ...good, pinned: 1 }), null);
  assert.equal(parseMemory({ ...good, validTo: 42 }), null);
  assert.equal(parseMemory({ ...good, values: { noteText: [1] } }), null);
  assert.equal(parseMemory(null), null);
  assert.deepEqual(parseMemories([good, { id: 'broken' }, 'x']).map(m => m.id), ['mem-1']);
  assert.deepEqual(parseMemories({ not: 'a list' }), []);
});

test('relative times', () => {
  const now = new Date(2026, 9, 3, 15, 0, 0);
  const at = (y: number, m: number, d: number) => new Date(y, m, d, 9, 0, 0).toISOString();
  assert.equal(relativeTime(null, now), 'never');
  assert.equal(relativeTime('nonsense', now), 'unknown');
  assert.equal(relativeTime(at(2026, 9, 3), now), 'today');
  assert.equal(relativeTime(at(2026, 9, 2), now), 'yesterday');
  assert.equal(relativeTime(at(2026, 8, 28), now), '5 days ago');
  assert.equal(relativeTime(at(2026, 8, 12), now), '3 weeks ago');
  assert.equal(relativeTime(at(2026, 4, 3), now), '5 months ago');
  assert.equal(relativeTime(at(2023, 9, 3), now), '3 years ago');
});

test('scopes and property labels in words', () => {
  const names = (id: string) => (id === 'src-1' ? 'Tax 2026' : undefined);
  assert.equal(scopeLabel('global', names), 'All conversations');
  assert.equal(scopeLabel('workspace:src-1', names), 'Conversations about Tax 2026');
  assert.equal(scopeLabel('workspace:gone', names), 'Conversations about a removed source');
  assert.equal(propertyLabel('preferenceTopic'), 'Preference topic');
  assert.equal(propertyLabel('name'), 'Name');
});

test('editing a note replaces its text; unchanged or emptied edits are refused', () => {
  const note = memory();
  assert.deepEqual(editableFields(note), [
    { name: 'noteText', label: 'Note', value: 'Locker code is on the fridge', multiline: true },
  ]);
  assert.equal(contentFromEdit(note, {}), null);
  assert.equal(contentFromEdit(note, { noteText: '  Locker code is on the fridge ' }), null);
  assert.equal(contentFromEdit(note, { noteText: '   ' }), null);
  assert.deepEqual(contentFromEdit(note, { noteText: 'Locker code moved to the drawer' }), {
    kind: 'note',
    text: 'Locker code moved to the drawer',
  });
});

test('editing a fact keeps the values it does not show', () => {
  const pref = memory({
    class: 'Preference',
    classLabel: 'Preference',
    text: 'Preference: preference holder the user; preference topic coffee; preference value black',
    properties: {
      preferenceHolder: { id: 'person:self', class: 'Person' },
      preferenceTopic: 'coffee',
      preferenceValue: 'black',
    },
  });
  assert.deepEqual(
    editableFields(pref).map(f => f.name),
    ['preferenceTopic', 'preferenceValue'],
  );
  assert.deepEqual(contentFromEdit(pref, { preferenceValue: 'with oat milk' }), {
    kind: 'fact',
    class: 'Preference',
    subject: null,
    properties: {
      preferenceHolder: { id: 'person:self', class: 'Person' },
      preferenceTopic: 'coffee',
      preferenceValue: 'with oat milk',
    },
  });
  const person = memory({ class: 'Person', subject: 'person:asha', properties: { name: 'Asha' } });
  const edit = contentFromEdit(person, { name: 'Asha Rao' });
  assert.ok(edit && edit.kind === 'fact');
  assert.deepEqual(edit.subject, { id: 'person:asha' });
});

test('search matches every word in text, class or values', () => {
  const pref = memory({ class: 'Preference', classLabel: 'Preference', text: 'coffee black', values: { preferenceTopic: ['coffee'] } });
  assert.ok(matchesFilter(pref, ''));
  assert.ok(matchesFilter(pref, 'COFFEE pref'));
  assert.ok(!matchesFilter(pref, 'coffee tea'));
});

test('export file names carry the local date', () => {
  assert.equal(exportFileName(new Date(2026, 9, 3)), 'shodh-memories-2026-10-03.json');
});

test('app settings carry the memory switch, defaulting to on for older files', () => {
  const base = { preferences: { theme: 'dark', searchMaxResults: 8 }, policy: { localOnly: false, webAccess: true } };
  assert.equal(parseAppSettings(base)?.memory.injectMemories, true);
  assert.equal(parseAppSettings({ ...base, memory: { injectMemories: false } })?.memory.injectMemories, false);
  assert.equal(parseAppSettings({ ...base, memory: { injectMemories: 'no' } }), null);
});
