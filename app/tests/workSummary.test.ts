/**
 * Folding a finished answer's working into one line.
 *   node --experimental-strip-types --test app/tests/workSummary.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { initialTranscript } from '../src/features/agent/reducer.ts';
import type { TranscriptState } from '../src/features/agent/reducer.ts';
import { workFold } from '../src/features/agent/workSummary.ts';
import { revisionText } from '../src/features/agent/revisionText.ts';

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

test('a working note after the last answer text never becomes the answer', () => {
  const fold = workFold(transcript({
    status: 'completed',
    durationMs: 3_000,
    blocks: [
      { kind: 'text', id: 't0', text: "I'll search the contracts." },
      { kind: 'step', stepId: 's1' },
      { kind: 'text', id: 't1', text: 'The notice period is 60 days [1].' },
      { kind: 'text', id: 't2', text: 'Let me also check the renewal clause.' },
      { kind: 'step', stepId: 's2' },
    ],
    interimIds: ['t0', 't2'],
  }));
  assert.equal(fold?.answerIndex, 2);
});

test('a finished run with a revision but no steps still folds its draft and revision row', () => {
  const blocks: TranscriptState['blocks'] = [
    { kind: 'text', id: 'draft', text: 'Draft answer [1][2].' },
    { kind: 'revision', id: 'revision-1', round: 1, reason: 'repair', flagged: 3, missingNeeds: [] },
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
  const fold = workFold(transcript({ status: 'completed', durationMs: 9_000, blocks, groundings }));
  assert.deepEqual(fold, { answerIndex: 2, label: 'Worked for 9s · re-checked 3 statements' });
  assert.ok(!fold?.label.includes('0 steps'));
  // Still running: the revision row stays open as live work.
  assert.equal(workFold(transcript({ status: 'running', blocks, groundings })), null);
});

test('a revision before the answer adds its re-checked count to a run with steps', () => {
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
  assert.equal(
    workFold(transcript({ status: 'completed', durationMs: 4_000, blocks, groundings }))?.label,
    'Worked for 4s · 1 step · re-checked 1 statement',
  );
});

test('a revision row reads as live work while running and as a record once finished', () => {
  assert.equal(revisionText('repair', 3, [], true), 'Re-checking 3 flagged statements against the sources');
  assert.equal(revisionText('repair', 3, [], false), 'Re-checked 3 flagged statements against the sources');
  assert.equal(revisionText('repair', 1, [], false), 'Re-checked 1 flagged statement against the sources');
  assert.equal(revisionText('coverage', 0, ['the renewal clause'], true), 'Searching for a part no passage covered: the renewal clause');
  assert.equal(revisionText('coverage', 0, ['a', 'b'], false), 'Searched for 2 parts no passage covered');
  assert.equal(
    revisionText('repair_and_coverage', 2, ['the renewal clause'], false),
    'Re-checked 2 flagged statements against the sources; searched for a part no passage covered: the renewal clause',
  );
  assert.equal(revisionText('retrieve', 3, [], true), 'Searching your documents for the question');
  assert.equal(revisionText('retrieve', 3, [], false), 'Searched your documents for the question');
});

test('the search the app made for an answer is a step, not re-checked statements', () => {
  const blocks: TranscriptState['blocks'] = [
    { kind: 'text', id: 'draft', text: 'FreshDiskANN keeps the graph on SSD [1][2].' },
    { kind: 'revision', id: 'revision-1', round: 1, reason: 'retrieve', flagged: 3, missingNeeds: [] },
    { kind: 'step', stepId: 'run-1-check-search-4' },
    { kind: 'text', id: 'final', text: 'FreshDiskANN keeps a compressed graph in memory [1].' },
  ];
  const groundings = [{
    round: 1,
    isFinal: true,
    method: 'entailment' as const,
    summary: { checked: 1, supported: 1, weak: 0, unsupported: 0, uncited: 0, invalid: 0, unchecked: 0, score: 1 },
    claims: [],
    needs: [],
    messageIds: ['final'],
    supersededMessageIds: ['draft'],
  }];
  const fold = workFold(transcript({ status: 'completed', durationMs: 6_000, blocks, groundings }));
  assert.deepEqual(fold, { answerIndex: 3, label: 'Worked for 6s · 1 step' });
});
