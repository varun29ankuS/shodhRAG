/**
 * Folding a finished answer's working into one line.
 *   node --experimental-strip-types --test app/tests/workSummary.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { initialTranscript } from '../src/features/agent/reducer.ts';
import type { TranscriptState } from '../src/features/agent/reducer.ts';
import { workFold } from '../src/features/agent/workSummary.ts';

function transcript(over: Partial<TranscriptState>): TranscriptState {
  return { ...initialTranscript('run-1', 0), ...over };
}

const steps: TranscriptState['blocks'] = [
  { kind: 'text', id: 't0', text: "I'll read the paper first." },
  { kind: 'step', stepId: 's1' },
  { kind: 'step', stepId: 's2' },
  { kind: 'text', id: 't1', text: 'Here is the explanation.' },
];

test('a completed answer folds everything before its final text', () => {
  const fold = workFold(transcript({
    status: 'completed',
    durationMs: 21_400,
    blocks: steps,
    passages: [
      { n: 1, file: 'a.pdf', path: 'c:/a.pdf', page: null, heading: null, score: 1, text: '' },
      { n: 2, file: 'a.pdf', path: 'c:/a.pdf', page: null, heading: null, score: 1, text: '' },
    ],
  }));
  assert.deepEqual(fold, { answerIndex: 3, label: 'Worked for 21s · 2 steps · 1 source' });
});

test('long runs show minutes', () => {
  const fold = workFold(transcript({ status: 'completed', durationMs: 125_000, blocks: steps }));
  assert.equal(fold?.label, 'Worked for 2m 05s · 2 steps');
});

test('nothing folds while running, on error, or without steps', () => {
  assert.equal(workFold(transcript({ status: 'running', blocks: steps })), null);
  assert.equal(workFold(transcript({ status: 'error', blocks: steps })), null);
  assert.equal(workFold(transcript({ status: 'completed', blocks: [{ kind: 'text', id: 'a', text: 'x' }] })), null);
  assert.equal(workFold(transcript({ status: 'completed', blocks: [{ kind: 'text', id: 'a', text: 'x' }, { kind: 'text', id: 'b', text: 'y' }] })), null);
});

test('a draft replaced by a revised answer folds with the working', () => {
  const blocks: TranscriptState['blocks'] = [
    { kind: 'step', stepId: 's1' },
    { kind: 'text', id: 'draft', text: 'Draft answer.' },
    { kind: 'revision', id: 'revision-1', round: 1, reason: 'repair', flagged: 1, missingNeeds: [] },
    { kind: 'text', id: 'final', text: 'Revised answer.' },
  ];
  const groundings = [{
    round: 1,
    isFinal: true,
    method: 'entailment' as const,
    summary: { checked: 0, supported: 0, weak: 0, unsupported: 0, uncited: 0, invalid: 0, unchecked: 0, score: null },
    claims: [],
    needs: [],
    messageIds: ['final'],
    supersededMessageIds: ['draft'],
  }];
  assert.equal(workFold(transcript({ status: 'completed', blocks, groundings }))?.answerIndex, 3);
  // A replaced last block never counts as the answer.
  const replacedLast = [...blocks.slice(0, 2)];
  const onlyDraft = [{ ...groundings[0], supersededMessageIds: ['draft'] }];
  assert.equal(workFold(transcript({ status: 'completed', blocks: replacedLast, groundings: onlyDraft })), null);
});
