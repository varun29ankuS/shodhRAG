/**
 * Suggested memories: parsing, decision descriptions, edits, batch accept, badge and the
 * learning settings. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/memorySuggestions.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  badgeText,
  batchAcceptable,
  contentFromSuggestionEdit,
  describeDecision,
  editableSuggestionFields,
  parseLearnStatus,
  parseSuggestion,
  parseSuggestions,
  sensitiveLabel,
  suggestionText,
} from '../src/features/memory/suggestions.ts';
import { DEFAULT_MEMORY_PREFS, parseAppSettings, parseMemoryPrefs } from '../src/lib/appSettings.ts';

function view(overrides: Record<string, unknown> = {}, action: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: 'sug-1',
    kind: 'remember',
    origin: 'turn',
    status: 'pending',
    action: {
      kind: 'remember',
      content: {
        kind: 'fact',
        class: 'Person',
        subject: { id: 'person:self', class: 'Person' },
        properties: { livesIn: { id: 'place:pune', class: 'Place' } },
        valid_from: '2026-10-01T10:00:00Z',
      },
      text: 'Person (the user): lives in @place:pune',
      decision: {
        kind: 'supersede',
        target: 'mem-1',
        changes: [{ property: 'livesIn', label: 'lives in', from: 'Delhi', to: 'Pune' }],
        explicit: false,
      },
      decided_by: 'rule',
      target_text: 'Person (the user): lives in @place:delhi',
      evidence: 'I moved to Pune',
      model: 'm',
      at: '2026-10-01T10:00:00Z',
      source: 'conversation://c1/turn/r1',
      ...action,
    },
    confidence: 0.9,
    sensitive: [],
    conversationId: 'c1',
    turnId: 'r1',
    outcome: null,
    error: null,
    undoable: false,
    createdAt: '2026-10-01T10:00:20Z',
    decidedAt: null,
    ...overrides,
  };
}

test('suggestions parse strictly', () => {
  const s = parseSuggestion(view());
  assert.ok(s);
  assert.equal(s.kind, 'remember');
  assert.equal(s.conversationId, 'c1');
  assert.equal(parseSuggestion(view({ status: 'maybe' })), null);
  assert.equal(parseSuggestion(view({ kind: 'link' })), null, 'kind must match the action');
  assert.equal(parseSuggestion(view({ sensitive: ['gossip'] })), null);
  assert.equal(parseSuggestion(view({}, { decided_by: 'magic' })), null);
  assert.equal(parseSuggestions([view(), 7, view({ id: 'sug-2' })]).length, 2);
});

test('decisions are described for the user', () => {
  const s = parseSuggestion(view())!;
  assert.equal(describeDecision(s), 'Supersedes: lives in Delhi → Pune (the old value is kept in history)');
  const noop = parseSuggestion(view({}, { decision: { kind: 'noop', existing: 'mem-1' } }))!;
  assert.match(describeDecision(noop), /^Already remembered/);
  const undecided = parseSuggestion(view({}, { decision: { kind: 'add' }, decided_by: 'undecided', target_text: null }))!;
  assert.match(describeDecision(undecided), /may overlap/);
  const resolve = parseSuggestion({
    ...view(),
    kind: 'resolve',
    origin: 'consolidate',
    action: {
      kind: 'resolve',
      keep: 'b',
      keep_text: 'name Varun S',
      retire: 'a',
      retire_text: 'name Varun',
      changes: [{ property: 'name', label: 'name', from: 'Varun', to: 'Varun S' }],
    },
  })!;
  assert.equal(suggestionText(resolve), 'name Varun S');
  assert.match(describeDecision(resolve), /Contradiction: name Varun → Varun S/);
});

test('edits keep the class and replace text values only', () => {
  const s = parseSuggestion(
    view({}, {
      content: { kind: 'fact', class: 'Preference', subject: null, properties: { preferenceTopic: 'coffee', preferenceValue: 'latte', preferenceStance: 'likes' } },
      decision: { kind: 'add' },
      target_text: null,
    }),
  )!;
  const fields = editableSuggestionFields(s).map(f => f.name);
  assert.deepEqual(fields, ['preferenceTopic', 'preferenceValue', 'preferenceStance']);
  assert.deepEqual(contentFromSuggestionEdit(s, { preferenceValue: ' flat white ' }), {
    kind: 'fact',
    class: 'Preference',
    subject: null,
    properties: { preferenceTopic: 'coffee', preferenceValue: 'flat white', preferenceStance: 'likes' },
  });
  assert.equal(contentFromSuggestionEdit(s, { preferenceValue: '  ' }), null, 'a field cannot be emptied');
  // Entity values are not text fields.
  assert.deepEqual(editableSuggestionFields(parseSuggestion(view())!), []);
});

test('batch accept leaves sensitive suggestions for one-by-one review', () => {
  const list = parseSuggestions([
    view({ id: 'a' }),
    view({ id: 'b', sensitive: ['health'] }),
    view({ id: 'c', status: 'learned' }),
  ]);
  assert.deepEqual(batchAcceptable(list).map(s => s.id), ['a']);
  assert.equal(sensitiveLabel(['health', 'third_party_personal']), 'Sensitive: health, about someone else');
  assert.equal(sensitiveLabel([]), '');
});

test('badge text', () => {
  assert.equal(badgeText(0), null);
  assert.equal(badgeText(1), '1 suggested memory');
  assert.equal(badgeText(3), '3 suggested memories');
  assert.equal(badgeText(250), '99+ suggested memories');
});

test('learning status parses', () => {
  const status = parseLearnStatus({
    mode: 'ask',
    killSwitch: false,
    available: false,
    unavailableReason: 'Local-only mode is on',
    model: null,
    pending: 2,
    usage: { day: '2026-10-04', llmCalls: 1, inputChars: 10, outputChars: 5, proposals: 2, invalid: 0, refused: 1 },
    caps: { maxCallsPerDay: 60, maxInputCharsPerDay: 400000, maxProposalsPerDay: 40 },
    lastConsolidation: null,
  });
  assert.ok(status);
  assert.equal(status.unavailableReason, 'Local-only mode is on');
  assert.equal(parseLearnStatus({ mode: 'sometimes' }), null);
});

test('memory settings keep every field and default the learning ones', () => {
  assert.deepEqual(parseMemoryPrefs(undefined), DEFAULT_MEMORY_PREFS);
  const old = parseMemoryPrefs({ injectMemories: false });
  assert.equal(old?.injectMemories, false);
  assert.equal(old?.learnMode, 'ask');
  assert.equal(parseMemoryPrefs({ learnMode: 'always' }), null);
  assert.equal(parseMemoryPrefs({ autoMinConfidence: 'high' }), null);
  const full = parseMemoryPrefs({
    injectMemories: true,
    learnMode: 'auto',
    learnModel: 'claude-haiku-4-5',
    autoMinConfidence: 0.9,
    learnCaps: { maxCallsPerDay: 10, maxInputCharsPerDay: 5000, maxProposalsPerDay: 5 },
  });
  assert.equal(full?.learnMode, 'auto');
  assert.equal(full?.learnCaps.maxCallsPerDay, 10);
  const settings = parseAppSettings({
    preferences: { theme: 'dark', searchMaxResults: 8 },
    policy: { localOnly: false, webAccess: true },
    memory: { injectMemories: true, learnMode: 'off' },
  });
  assert.equal(settings?.memory.learnMode, 'off');
});
