/**
 * The memory lines of the chat: "Remembered · Undo" under a message that asked Shodh to
 * remember something, "Learned · Undo" under an answer memories were learned from, what
 * waits for review, and undone or forgotten memories shown without their words. Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/turnMemories.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseSuggestion } from '../src/features/memory/suggestions.ts';
import type { Suggestion } from '../src/features/memory/suggestions.ts';
import {
  MAX_TURNS,
  concernsTurns,
  groupByTurn,
  lineOf,
  lineText,
  linesOfRuns,
  runIdsOf,
  summaryText,
  turnIdsOf,
  turnsChanged,
  withStableTurns,
} from '../src/features/memory/turnMemories.ts';

function suggestion(overrides: Record<string, unknown> = {}, action: Record<string, unknown> = {}): Suggestion {
  const s = parseSuggestion({
    id: 'sug-1',
    kind: 'remember',
    origin: 'turn',
    status: 'accepted',
    action: {
      kind: 'remember',
      content: { kind: 'note', text: 'I use DiskANN' },
      text: 'I use DiskANN',
      decision: { kind: 'add' },
      decided_by: 'rule',
      target_text: null,
      evidence: 'Remember that I use DiskANN',
      model: '0.0.6',
      at: '2026-10-19T10:00:00Z',
      source: 'conversation://c1/turn/run-1',
      by_user: true,
      ...action,
    },
    confidence: 1,
    sensitive: [],
    conversationId: 'c1',
    turnId: 'run-1',
    scope: 'global',
    outcome: null,
    error: null,
    undoable: true,
    forgotten: false,
    createdAt: '2026-10-19T10:00:00Z',
    decidedAt: '2026-10-19T10:00:00Z',
    ...overrides,
  });
  assert.ok(s);
  return s;
}

test('a note the user asked for shows as remembered, with undo, under their message', () => {
  const line = lineOf(suggestion());
  assert.deepEqual(line, { id: 'sug-1', kind: 'remembered', asked: true, text: 'I use DiskANN', undoable: true });
  assert.deepEqual(lineText(line!), { lead: 'Remembered', quote: 'I use DiskANN', action: 'undo' });
});

test('what the learner saved shows as learned, under the answer', () => {
  const line = lineOf(suggestion({ status: 'learned' }, { by_user: false, text: 'Prefers dark roast coffee' }));
  assert.equal(line?.kind, 'learned');
  assert.equal(line?.asked, false);
  assert.deepEqual(lineText(line!), { lead: 'Learned', quote: 'Prefers dark roast coffee', action: 'undo' });
  // Suggestions of older builds had no `by_user`: the learner's.
  assert.equal(lineOf(suggestion({ status: 'learned' }, { by_user: undefined }))?.asked, false);
});

test('what waits for approval asks for review', () => {
  const line = lineOf(suggestion({ status: 'pending', undoable: false, sensitive: ['third_party_personal'] }));
  assert.equal(line?.kind, 'suggested');
  assert.deepEqual(lineText(line!), { lead: 'Shodh suggests remembering', quote: 'I use DiskANN', action: 'review' });
  assert.equal(lineOf(suggestion({ status: 'failed', undoable: false }))?.kind, 'suggested');
});

test('undone and forgotten memories show without their words', () => {
  const undone = lineOf(suggestion({ status: 'undone', undoable: false }));
  assert.deepEqual(undone, { id: 'sug-1', kind: 'undone', asked: true, text: '', undoable: false });
  assert.deepEqual(lineText(undone!), { lead: 'Remembered · Undone', quote: null, action: null });
  const learnedUndone = lineOf(suggestion({ status: 'undone', undoable: false }, { by_user: false }));
  assert.equal(lineText(learnedUndone!).lead, 'Learned · Undone');
  // Forgotten: the backend erased the words; the line never shows any.
  const forgotten = lineOf(suggestion({ forgotten: true, undoable: false }, { text: '' }));
  assert.deepEqual(forgotten, { id: 'sug-1', kind: 'forgotten', asked: true, text: '', undoable: false });
  assert.deepEqual(lineText(forgotten!), { lead: 'Remembered: a forgotten memory', quote: null, action: null });
});

test('nothing shows for what saved nothing, was refused, or is not a turn memory', () => {
  // A reinforcement, or words already remembered: nothing to undo.
  assert.equal(lineOf(suggestion({ undoable: false })), null);
  assert.equal(lineOf(suggestion({ status: 'rejected', undoable: false })), null);
  assert.equal(lineOf(suggestion({ status: 'stale', undoable: false })), null);
  // A waiting suggestion gone stale with a forgotten memory is never shown.
  assert.equal(lineOf(suggestion({ status: 'pending', forgotten: true, undoable: false })), null);
  assert.equal(lineOf(suggestion({ origin: 'consolidate' })), null);
});

test('lines are grouped by turn, the user’s requests apart from what was learned', () => {
  const turns = groupByTurn([
    suggestion({ id: 'a' }),
    suggestion({ id: 'b', status: 'learned' }, { by_user: false }),
    suggestion({ id: 'c', status: 'learned', turnId: 'run-2' }, { by_user: false }),
    suggestion({ id: 'd', status: 'rejected', undoable: false, turnId: 'run-2' }),
    suggestion({ id: 'e', turnId: null }),
  ]);
  assert.deepEqual([...turns.keys()], ['run-1', 'run-2']);
  assert.deepEqual(turns.get('run-1')!.asked.map(l => l.id), ['a']);
  assert.deepEqual(turns.get('run-1')!.learned.map(l => l.id), ['b']);
  assert.deepEqual(turns.get('run-2')!.asked, []);
  assert.deepEqual(turns.get('run-2')!.learned.map(l => l.id), ['c']);
});

test('unchanged turns keep their lines, so their answers are not re-rendered', () => {
  const first = groupByTurn([suggestion({ id: 'a' }), suggestion({ id: 'c', turnId: 'run-2' })]);
  const second = withStableTurns(
    first,
    groupByTurn([suggestion({ id: 'a' }), suggestion({ id: 'c', turnId: 'run-2', status: 'undone', undoable: false })]),
  );
  assert.equal(second.get('run-1'), first.get('run-1'));
  assert.notEqual(second.get('run-2'), first.get('run-2'));
  assert.equal(second.get('run-2')!.asked[0].kind, 'undone');
});

test('several memories are summed up by what is still saved', () => {
  const learned = (id: string, status = 'learned') =>
    lineOf(suggestion({ id, status, undoable: status === 'learned' }, { by_user: false }))!;
  const lines = [learned('a'), learned('b'), learned('c')];
  assert.equal(summaryText(lines), 'Learned 3 things');
  const pending = lineOf(suggestion({ id: 'd', status: 'pending', undoable: false }, { by_user: false }))!;
  assert.equal(summaryText([...lines.slice(0, 2), pending]), 'Learned 2 things, 1 to review');
  assert.equal(summaryText([pending, pending]), 'Shodh suggests remembering 2 things');
  // Undone and forgotten ones are not counted as learned.
  const undone = [learned('a'), learned('b', 'undone'), learned('c', 'undone')];
  assert.equal(summaryText(undone), 'Learned 1 thing, 2 undone or forgotten');
  assert.equal(summaryText([learned('b', 'undone'), learned('c', 'undone'), learned('e', 'undone')]), '3 things undone or forgotten');
  // What the user asked for is remembered, not learned.
  const asked = ['a', 'b', 'c'].map(id => lineOf(suggestion({ id }))!);
  assert.equal(summaryText(asked), 'Remembered 3 things');
});

test('a retried answer keeps the lines of the tries it replaced', () => {
  assert.deepEqual(runIdsOf({ runId: 'run-3', priorRunIds: ['run-1', 'run-2'] }), ['run-1', 'run-2', 'run-3']);
  assert.deepEqual(runIdsOf({ runId: 'run-1' }), ['run-1']);
  assert.deepEqual(runIdsOf(null), []);
  // The request was saved in the first try; the retry found it kept.
  const turns = groupByTurn([suggestion({ id: 'a', turnId: 'run-1' }), suggestion({ id: 'b', turnId: 'run-3', status: 'learned' }, { by_user: false })]);
  assert.deepEqual(linesOfRuns(turns, ['run-1', 'run-3'], 'asked')?.map(l => l.id), ['a']);
  assert.deepEqual(linesOfRuns(turns, ['run-1', 'run-3'], 'learned')?.map(l => l.id), ['b']);
  assert.equal(linesOfRuns(turns, ['run-9'], 'asked'), undefined);
});

test('the turns asked about are the answers’ run ids, newest last and bounded', () => {
  assert.deepEqual(turnIdsOf([undefined, 'run-1', null, 'run-2', 'run-1', '']), ['run-1', 'run-2']);
  const many = Array.from({ length: MAX_TURNS + 5 }, (_, i) => `run-${i}`);
  const ids = turnIdsOf(many);
  assert.equal(ids.length, MAX_TURNS);
  assert.equal(ids[ids.length - 1], `run-${MAX_TURNS + 4}`);
});

test('a change refreshes the chat when it is about its turns or names none', () => {
  assert.deepEqual(turnsChanged({ pending: 1, conversationId: 'c1', turnIds: ['run-1', 7], proposalIds: ['sug-1'] }), {
    conversationId: 'c1',
    turnIds: ['run-1'],
  });
  assert.equal(turnsChanged({ pending: 1 }), null);
  assert.equal(turnsChanged(null), null);
  const shown = ['run-1', 'run-2'];
  assert.equal(concernsTurns(null, 'c1', shown), true, 'an undo or a forget anywhere');
  assert.equal(concernsTurns({ conversationId: 'c1', turnIds: ['run-2'] }, 'c1', shown), true);
  assert.equal(concernsTurns({ conversationId: 'c1', turnIds: ['run-9'] }, 'c1', shown), false);
  assert.equal(concernsTurns({ conversationId: 'c2', turnIds: ['run-1'] }, 'c1', shown), false);
});
