/**
 * Transcript reducer tests. Run with Node 22.6+ (no extra dependencies):
 *   node --experimental-strip-types --test app/tests/reducer.test.ts
 *
 * The event sequence mirrors what the Rust normaliser emits for the recorded
 * omp spike (crates/shodh-rag/src/harness/fixtures/omp-spike-events.jsonl):
 * run_started → step_started (intent label) → step_finished with passages →
 * text deltas → usage → run_finished.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { AgentEvent } from '../src/features/agent/events.ts';
import {
  answerText,
  currentStep,
  fromPersisted,
  initialTranscript,
  pendingApproval,
  planProgress,
  reduceAll,
  reduceTranscript,
  toPersisted,
} from '../src/features/agent/reducer.ts';

const RUN = 'req-1';

function spikeRun(): AgentEvent[] {
  return [
    { type: 'run_started', runId: RUN, sessionId: 's1', model: 'openrouter/x', atMs: 1000, warning: null },
    {
      type: 'step_started', runId: RUN, stepId: 'call-1', parentStepId: null, tool: 'search_documents',
      label: 'Finding Acme MSA notice period', args: { query: 'Acme MSA notice period' }, tier: 'read', atMs: 1010,
    },
    {
      type: 'step_finished', runId: RUN, stepId: 'call-1', ok: true, summary: '2 passages from 1 file',
      detail: {
        passages: [
          { n: 1, file: 'Acme_MSA.pdf', path: 'C:/docs/Acme_MSA.pdf', page: '4', heading: null, score: 0.9, text: 'Either party may terminate with 60 days notice.' },
          { n: 2, file: 'Acme_MSA.pdf', path: 'C:/docs/Acme_MSA.pdf', page: '5', heading: null, score: 0.5, text: 'Renewal terms.' },
        ],
      },
      durationMs: 1200,
    },
    { type: 'text_delta', runId: RUN, messageId: 'm1', delta: 'The notice period is ' },
    { type: 'text_delta', runId: RUN, messageId: 'm1', delta: '60 days [1].' },
    { type: 'usage', runId: RUN, inputTokens: 3700, outputTokens: 366, cacheReadTokens: 3240, costUsd: 0 },
    { type: 'run_finished', runId: RUN, status: 'completed', durationMs: 4200, error: null },
  ];
}

test('the spike run reduces to steps, passages, text, usage and completion', () => {
  const state = reduceAll(initialTranscript(RUN, 1000), spikeRun());
  assert.equal(state.status, 'completed');
  assert.equal(state.model, 'openrouter/x');
  assert.equal(state.blocks.length, 2);
  assert.deepEqual(state.blocks[0], { kind: 'step', stepId: 'call-1' });
  const step = state.steps['call-1'];
  assert.equal(step.label, 'Finding Acme MSA notice period');
  assert.equal(step.status, 'done');
  assert.equal(step.durationMs, 1200);
  assert.equal(answerText(state), 'The notice period is 60 days [1].');
  assert.ok(answerText(state).includes('60'));
  assert.deepEqual(state.passages.map(p => p.n), [1, 2]);
  assert.equal(state.passages[0].page, '4');
  assert.deepEqual(state.usage, { inputTokens: 3700, outputTokens: 366, cacheReadTokens: 3240, costUsd: 0 });
  assert.equal(state.durationMs, 4200);
  assert.equal(currentStep(state), null);
});

test('events of another run are ignored', () => {
  const state = reduceTranscript(initialTranscript(RUN, 0), {
    type: 'text_delta', runId: 'other', messageId: 'm', delta: 'x',
  });
  assert.equal(state.blocks.length, 0);
  assert.equal(state.status, 'starting');
});

test('passages from later searches keep their run-wide numbers', () => {
  const events: AgentEvent[] = [
    ...spikeRun().slice(0, 3),
    {
      type: 'step_started', runId: RUN, stepId: 'call-2', parentStepId: null, tool: 'search_documents',
      label: 'Searching renewal', args: {}, tier: 'read', atMs: 3000,
    },
    {
      type: 'step_finished', runId: RUN, stepId: 'call-2', ok: true, summary: '1 passage', durationMs: 5,
      detail: { passages: [{ n: 3, file: 'b.txt', path: 'C:/b.txt', page: null, score: 0.4, text: 'third' }] },
    },
  ];
  const state = reduceAll(initialTranscript(RUN, 0), events);
  assert.deepEqual(state.passages.map(p => p.n), [1, 2, 3]);
  assert.equal(state.passages[2].page, null);
});

test('sub-steps nest under their parent instead of the top level', () => {
  const state = reduceAll(initialTranscript(RUN, 0), [
    { type: 'step_started', runId: RUN, stepId: 'p', parentStepId: null, tool: 'delegate', label: 'Delegating', args: {}, tier: 'read', atMs: 1 },
    { type: 'step_started', runId: RUN, stepId: 'c', parentStepId: 'p', tool: 'search_documents', label: 'Searching', args: {}, tier: 'read', atMs: 2 },
  ]);
  assert.deepEqual(state.blocks, [{ kind: 'step', stepId: 'p' }]);
  assert.deepEqual(state.steps.p.children, ['c']);
  assert.equal(currentStep(state)?.id, 'c');
});

test('approvals: requested, answered locally, then finished', () => {
  let state = reduceAll(initialTranscript(RUN, 0), [
    { type: 'step_started', runId: RUN, stepId: 't', parentStepId: null, tool: 'create_task', label: 'Creating task', args: {}, tier: 'write', atMs: 1 },
    { type: 'approval_requested', runId: RUN, stepId: 't', tool: 'create_task', label: 'Create task “Pay invoice”', tier: 'write', preview: { title: 'Pay invoice' } },
  ]);
  assert.equal(state.steps.t.status, 'awaiting_approval');
  assert.equal(pendingApproval(state)?.id, 't');
  state = reduceTranscript(state, { type: 'local_approval', stepId: 't', approved: false });
  assert.equal(state.steps.t.approval?.decision, 'denied');
  assert.equal(pendingApproval(state), null);
  state = reduceTranscript(state, { type: 'step_finished', runId: RUN, stepId: 't', ok: false, summary: 'Declined', detail: null, durationMs: 9 });
  assert.equal(state.steps.t.status, 'failed');
  assert.equal(state.steps.t.summary, 'Declined');
});

test('a run that ends with open steps closes them, and a finished run is frozen', () => {
  let state = reduceAll(initialTranscript(RUN, 100), [
    { type: 'step_started', runId: RUN, stepId: 's', parentStepId: null, tool: 'search_documents', label: 'Searching', args: {}, tier: 'read', atMs: 110 },
    { type: 'run_finished', runId: RUN, status: 'aborted', durationMs: 50, error: null },
  ]);
  assert.equal(state.status, 'aborted');
  assert.equal(state.steps.s.status, 'failed');
  assert.equal(state.steps.s.summary, 'Interrupted');
  state = reduceTranscript(state, { type: 'text_delta', runId: RUN, messageId: 'm', delta: 'late' });
  assert.equal(state.blocks.length, 1);
});

test('local failure before the run starts records the code', () => {
  const state = reduceTranscript(initialTranscript(RUN, 0), {
    type: 'local_failed', error: 'not installed', code: 'runtime_missing', atMs: 30,
  });
  assert.equal(state.status, 'error');
  assert.equal(state.errorCode, 'runtime_missing');
  assert.equal(state.durationMs, 30);
});

test('steering appends a steer block between text', () => {
  const state = reduceAll(initialTranscript(RUN, 0), [
    { type: 'text_delta', runId: RUN, messageId: 'm1', delta: 'Looking…' },
    { type: 'local_steer', id: 'st1', text: 'only 2024 files' },
    { type: 'text_delta', runId: RUN, messageId: 'm2', delta: 'Done.' },
  ]);
  assert.deepEqual(state.blocks.map(b => b.kind), ['text', 'steer', 'text']);
  assert.equal(answerText(state), 'Looking…\n\nDone.');
});

test('plans replace each other and report progress', () => {
  const state = reduceAll(initialTranscript(RUN, 0), [
    { type: 'plan_updated', runId: RUN, items: [{ id: '1', text: 'a', status: 'done' }, { id: '2', text: 'b', status: 'in_progress' }] },
  ]);
  assert.deepEqual(planProgress(state.plan ?? []), { done: 1, total: 2 });
  const cleared = reduceTranscript(state, { type: 'plan_updated', runId: RUN, items: [] });
  assert.equal(cleared.plan, null);
});

test('a transcript persisted mid-run is restored as interrupted', () => {
  const live = reduceAll(initialTranscript(RUN, 0), spikeRun().slice(0, 2));
  const restored = fromPersisted(JSON.parse(JSON.stringify(toPersisted(live))));
  assert.ok(restored);
  assert.equal(restored.status, 'aborted');
  assert.equal(restored.steps['call-1'].status, 'failed');
  assert.equal(fromPersisted({ nope: true }), null);
  const done = reduceAll(initialTranscript(RUN, 1000), spikeRun());
  assert.deepEqual(fromPersisted(JSON.parse(JSON.stringify(toPersisted(done)))), done);
});
