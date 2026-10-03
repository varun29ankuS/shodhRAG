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
