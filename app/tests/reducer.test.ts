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
import type { AgentEvent, GroundingReport } from '../src/features/agent/events.ts';
import {
  answerIncomplete,
  showsGroundingChip,
  answerText,
  awaitingRetry,
  currentStep,
  fromPersisted,
  initialTranscript,
  pendingApproval,
  planProgress,
  reduceAll,
  reduceTranscript,
  supersededBlocks,
  toCheckpoint,
  toPersisted,
} from '../src/features/agent/reducer.ts';
import { MAX_PERSISTED_COMMAND_OUTPUT_CHARS } from '../src/features/agent/commandRun.ts';

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

function groundingReport(partial: Partial<GroundingReport>): GroundingReport {
  return {
    round: 0,
    isFinal: false,
    method: 'entailment',
    summary: { checked: 1, supported: 0, weak: 0, unsupported: 1, uncited: 0, invalid: 0, unchecked: 0, score: 0 },
    claims: [],
    needs: [],
    messageIds: ['m1'],
    supersededMessageIds: [],
    ...partial,
  };
}

test('a revised answer replaces its draft in the answer text and keeps both in the transcript', () => {
  const events: AgentEvent[] = [
    ...spikeRun().slice(0, 3),
    { type: 'text_delta', runId: RUN, messageId: 'm1', delta: 'The notice period is 90 days [1].' },
    { type: 'grounding', runId: RUN, report: groundingReport({}) },
    { type: 'revision_started', runId: RUN, round: 1, reason: 'repair', flagged: 1, missingNeeds: [] },
    { type: 'text_delta', runId: RUN, messageId: 'm2', delta: 'The notice period is 60 days [1].' },
    {
      type: 'grounding',
      runId: RUN,
      report: groundingReport({ round: 1, isFinal: true, messageIds: ['m2'], supersededMessageIds: ['m1'] }),
    },
    { type: 'run_finished', runId: RUN, status: 'completed', durationMs: 5000, error: null },
  ];
  const state = reduceAll(initialTranscript(RUN, 1000), events);
  assert.equal(state.status, 'completed');
  assert.equal(answerText(state), 'The notice period is 60 days [1].');
  assert.deepEqual(state.blocks.map(b => b.kind), ['step', 'text', 'revision', 'text']);
  assert.equal(state.groundings.length, 2);
  assert.deepEqual([...supersededBlocks(state)], ['m1']);
  const restored = fromPersisted(JSON.parse(JSON.stringify(toPersisted(state))));
  assert.ok(restored);
  assert.equal(restored.groundings.length, 2, 'both checks are persisted');
  assert.equal(answerText(restored), 'The notice period is 60 days [1].');
});

test('transcripts stored before grounding restore with no checks', () => {
  const old = { ...toPersisted(reduceAll(initialTranscript(RUN, 1000), spikeRun())) } as Record<string, unknown>;
  delete old.groundings;
  const restored = fromPersisted(old);
  assert.ok(restored);
  assert.deepEqual(restored.groundings, []);
});

test('plan items from older builds get need defaults and coverage is kept', () => {
  const legacy = { type: 'plan_updated', runId: RUN, items: [{ id: '1', text: 'Search', status: 'done' }] } as unknown as AgentEvent;
  let state = reduceAll(initialTranscript(RUN, 0), [spikeRun()[0], legacy]);
  assert.deepEqual(state.plan, [{ id: '1', text: 'Search', status: 'done', need: false, coverage: null, evidence: [] }]);
  state = reduceTranscript(state, {
    type: 'plan_updated',
    runId: RUN,
    items: [{ id: '1', text: 'Notice period', status: 'done', need: true, coverage: 'covered', evidence: [3] }],
  });
  assert.equal(state.plan?.[0].coverage, 'covered');
  assert.deepEqual(state.plan?.[0].evidence, [3]);
});

test('a working note before tool calls stays visible but is not the answer', () => {
  const events: AgentEvent[] = [
    { type: 'run_started', runId: RUN, sessionId: 's1', model: 'openrouter/x', atMs: 1000, warning: null },
    { type: 'text_delta', runId: RUN, messageId: 'm1', delta: "I'll search for the monthly retainer." },
    { type: 'text_interim', runId: RUN, messageId: 'm1', interim: true },
    ...spikeRun().slice(1, 3),
    { type: 'text_delta', runId: RUN, messageId: 'm2', delta: 'The retainer is INR 2,75,000 [1].' },
    { type: 'run_finished', runId: RUN, status: 'completed', durationMs: 900, error: null, providerError: null },
  ];
  const state = reduceAll(initialTranscript(RUN, 1000), events);
  assert.deepEqual(state.blocks.map(b => b.kind), ['text', 'step', 'text'], 'the note is never removed');
  assert.equal(answerText(state), 'The retainer is INR 2,75,000 [1].');
  const restored = fromPersisted(JSON.parse(JSON.stringify(toPersisted(state))));
  assert.ok(restored);
  assert.equal(answerText(restored), 'The retainer is INR 2,75,000 [1].', 'notes are persisted');
});

test('a note taken back as the answer counts again', () => {
  const state = reduceAll(initialTranscript(RUN, 1000), [
    { type: 'text_delta', runId: RUN, messageId: 'm1', delta: 'Sixty days [1].' },
    { type: 'text_interim', runId: RUN, messageId: 'm1', interim: true },
    { type: 'text_interim', runId: RUN, messageId: 'm1', interim: false },
  ]);
  assert.equal(answerText(state), 'Sixty days [1].');
  const old = { ...toPersisted(state) } as Record<string, unknown>;
  delete old.interimIds;
  const restored = fromPersisted(old);
  assert.ok(restored);
  assert.deepEqual(restored.interimIds, [], 'transcripts saved before notes restore without any');
});
test('a call refused for its arguments shows as retried once the tool succeeds', () => {
  const started = (stepId: string, tool: string): AgentEvent => ({
    type: 'step_started', runId: RUN, stepId, parentStepId: null, tool, label: tool, args: {}, tier: 'read', atMs: 1010,
  });
  let state = reduceAll(initialTranscript(RUN, 1000), [
    { type: 'run_started', runId: RUN, sessionId: 's1', model: 'm', atMs: 1000, warning: null },
    started('bad', 'search_documents'),
    {
      type: 'step_finished', runId: RUN, stepId: 'bad', ok: false, summary: 'Invalid arguments',
      detail: { error: 'Validation failed for tool "search_documents"' }, durationMs: 9, invalidArguments: true,
    },
    started('other-fail', 'open_document'),
    { type: 'step_finished', runId: RUN, stepId: 'other-fail', ok: false, summary: 'Not found', detail: null, durationMs: 3 },
  ]);
  assert.equal(state.steps.bad.status, 'failed', 'failed until a later call of the tool succeeds');
  // Another tool succeeding does not retry it.
  state = reduceAll(state, [
    started('sources', 'list_sources'),
    { type: 'step_finished', runId: RUN, stepId: 'sources', ok: true, summary: '1 folder', detail: null, durationMs: 14 },
  ]);
  assert.equal(state.steps.bad.status, 'failed');
  state = reduceAll(state, [
    started('good', 'search_documents'),
    { type: 'step_finished', runId: RUN, stepId: 'good', ok: true, summary: '10 passages', detail: null, durationMs: 1700 },
  ]);
  assert.equal(state.steps.bad.status, 'retried');
  assert.equal(state.steps.good.status, 'done');
  // A failure for any other reason stays a failure.
  assert.equal(state.steps['other-fail'].status, 'failed');
  // A stored transcript keeps it.
  assert.equal(fromPersisted(JSON.parse(JSON.stringify(toPersisted(state))))?.steps.bad.status, 'retried');
});

test('a call refused for its arguments is not shown as failed while the run can still retry it', () => {
  const live = reduceAll(initialTranscript(RUN, 1000), [
    { type: 'run_started', runId: RUN, sessionId: 's1', model: 'm', atMs: 1000, warning: null },
    {
      type: 'step_started', runId: RUN, stepId: 'bad', parentStepId: null, tool: 'search_documents',
      label: 'Searching', args: {}, tier: 'read', atMs: 1010,
    },
    {
      type: 'step_finished', runId: RUN, stepId: 'bad', ok: false, summary: 'Invalid arguments',
      detail: null, durationMs: 9, invalidArguments: true,
    },
    {
      type: 'step_started', runId: RUN, stepId: 'gone', parentStepId: null, tool: 'open_document',
      label: 'Opening', args: {}, tier: 'read', atMs: 1020,
    },
    { type: 'step_finished', runId: RUN, stepId: 'gone', ok: false, summary: 'Not found', detail: null, durationMs: 3 },
  ]);
  assert.equal(awaitingRetry(live.steps.bad, true), true);
  // Any other failure shows as one at once.
  assert.equal(awaitingRetry(live.steps.gone, true), false);
  // The run ended without a corrected call: it is a failure after all.
  assert.equal(awaitingRetry(live.steps.bad, false), false);
});

test('a run that fails mid-answer keeps its partial text, marked incomplete, not finished', () => {
  const field = '[ERROR: Agent failed (Function process_single_item_agent timed out after 90.0 seconds)]';
  const events: AgentEvent[] = [
    { type: 'run_started', runId: RUN, sessionId: 's1', model: 'openrouter/vendor/model', atMs: 1000, warning: null },
    { type: 'text_delta', runId: RUN, messageId: 'm1', delta: 'The notice period is ' },
    {
      type: 'run_finished', runId: RUN, status: 'error', durationMs: 900, error: field,
      providerError: { kind: 'timeout', status: null, retryAfterSecs: null, upstream: 'Nvidia' },
    },
  ];
  const state = reduceAll(initialTranscript(RUN, 1000), events);
  assert.equal(state.status, 'error', 'not saved as a finished answer');
  assert.equal(answerText(state), 'The notice period is', 'the partial text stays');
  assert.ok(answerIncomplete(state));
  assert.deepEqual(state.providerError, { kind: 'timeout', status: null, retryAfterSecs: null, upstream: 'Nvidia' });
  assert.equal(state.error, field, 'the raw provider text is kept for Details');
  const restored = fromPersisted(JSON.parse(JSON.stringify(toPersisted(state))));
  assert.ok(restored);
  assert.equal(restored.status, 'error');
  assert.ok(answerIncomplete(restored));

  const done = reduceAll(initialTranscript(RUN, 1000), spikeRun());
  assert.equal(answerIncomplete(done), false, 'a finished answer is complete');
  const failedEmpty = reduceAll(initialTranscript(RUN, 1000), [
    events[0],
    { type: 'run_finished', runId: RUN, status: 'error', durationMs: 5, error: 'x', providerError: null },
  ]);
  assert.equal(answerIncomplete(failedEmpty), false, 'no text, nothing to mark');
});

test('the grounding chip shows for a finished answer, never for a failed run', () => {
  const done = reduceAll(initialTranscript(RUN, 1000), spikeRun());
  assert.equal(showsGroundingChip(done), true);
  const live = reduceAll(initialTranscript(RUN, 1000), spikeRun().slice(0, -1));
  assert.equal(showsGroundingChip(live), false, 'not while the run is live');
  // Round 1 was answered (and checked); the follow-up round failed.
  const failed = reduceAll(initialTranscript(RUN, 1000), [
    ...spikeRun().slice(0, -1),
    { type: 'run_finished', runId: RUN, status: 'error', durationMs: 900, error: 'Provider returned error finish_reason', providerError: { kind: 'model_unavailable', status: null, retryAfterSecs: null } },
  ]);
  assert.equal(failed.status, 'error');
  assert.equal(showsGroundingChip(failed), false);
  const restored = fromPersisted(JSON.parse(JSON.stringify(toPersisted(failed))));
  assert.ok(restored);
  assert.equal(showsGroundingChip(restored), false, 'also once stored');
});

function commandStep(stepId = 'cmd-step'): AgentEvent {
  return {
    type: 'step_started', runId: RUN, stepId, parentStepId: null, tool: 'run_command',
    label: 'Running cargo test', args: { command: 'cargo test', timeout_secs: 120 }, tier: 'write', atMs: 1010,
  };
}

test('a run_command step shows its command starting, streaming and finishing', () => {
  let state = reduceAll(initialTranscript(RUN, 1000), [
    { type: 'run_started', runId: RUN, sessionId: 's1', model: 'm', atMs: 1000, warning: null },
    commandStep(),
    { type: 'command_started', runId: RUN, stepId: 'cmd-step', commandId: 'c1', command: 'cargo test', timeLimitSecs: 120, sandbox: 'Sandboxed: no network', atMs: 1020 },
    { type: 'command_output', runId: RUN, stepId: 'cmd-step', commandId: 'c1', text: 'Compiling x\n' },
    { type: 'command_output', runId: RUN, stepId: 'cmd-step', commandId: 'c1', text: 'error[E0308]: mismatched types\n' },
    // Output of another command is not this step's.
    { type: 'command_output', runId: RUN, stepId: 'cmd-step', commandId: 'other', text: 'stray' },
  ]);
  const running = state.steps['cmd-step'].command;
  assert.ok(running);
  assert.equal(running.state, 'running');
  assert.equal(running.commandId, 'c1');
  assert.equal(running.command, 'cargo test');
  assert.equal(running.startedAtMs, 1020);
  assert.equal(running.timeLimitSecs, 120);
  // The live panel says what sandbox the command runs in, as the backend said.
  assert.equal(running.sandbox, 'Sandboxed: no network');
  assert.equal(running.output, 'Compiling x\nerror[E0308]: mismatched types\n');
  state = reduceAll(state, [
    { type: 'command_finished', runId: RUN, stepId: 'cmd-step', commandId: 'c1', state: 'done', exitCode: 101, note: null, durationMs: 2300 },
    { type: 'step_finished', runId: RUN, stepId: 'cmd-step', ok: true, summary: 'exit 101', detail: null, durationMs: 2400 },
  ]);
  const done = state.steps['cmd-step'].command;
  assert.equal(done?.state, 'done');
  assert.equal(done?.exitCode, 101);
  assert.equal(done?.durationMs, 2300);
  assert.equal(done?.output, 'Compiling x\nerror[E0308]: mismatched types\n');
  // Events for steps that do not exist change nothing.
  const same = reduceTranscript(state, { type: 'command_output', runId: RUN, stepId: 'nope', commandId: 'c1', text: 'x' });
  assert.equal(same, state);
});

test('a refused command arrives without a start and takes its text from the call', () => {
  const state = reduceAll(initialTranscript(RUN, 1000), [
    commandStep(),
    {
      type: 'command_finished', runId: RUN, stepId: 'cmd-step', commandId: 'c9', state: 'refused', exitCode: null,
      note: 'Git for Windows is not installed', durationMs: 0,
    },
  ]);
  const run = state.steps['cmd-step'].command;
  assert.ok(run);
  assert.equal(run.state, 'refused');
  assert.equal(run.commandId, 'c9');
  assert.equal(run.command, 'cargo test');
  assert.equal(run.note, 'Git for Windows is not installed');
  assert.equal(run.startedAtMs, 1010);
  assert.equal(run.timeLimitSecs, null);
  assert.equal(run.output, '');
});

test('steps without a command carry none, and their stored form is unchanged', () => {
  const done = reduceAll(initialTranscript(RUN, 1000), spikeRun());
  assert.equal('command' in done.steps['call-1'], false);
  assert.equal(toPersisted(done).steps, done.steps);
});

test('a command still running when the run ends is shown as stopped', () => {
  const live = reduceAll(initialTranscript(RUN, 1000), [
    commandStep(),
    { type: 'command_started', runId: RUN, stepId: 'cmd-step', commandId: 'c1', command: 'cargo test', timeLimitSecs: 600, atMs: 1020 },
  ]);
  const finished = reduceTranscript(live, { type: 'run_finished', runId: RUN, status: 'completed', durationMs: 5020, error: null, providerError: null });
  const stopped = finished.steps['cmd-step'].command;
  assert.equal(stopped?.state, 'stopped');
  assert.equal(stopped?.note, 'Ended with the answer');
  assert.equal(stopped?.durationMs, 5000);
  const interrupted = reduceTranscript(live, { type: 'local_interrupted', atMs: 3020 });
  assert.equal(interrupted.steps['cmd-step'].command?.state, 'stopped');
  assert.equal(interrupted.steps['cmd-step'].command?.durationMs, 2000);
  const failed = reduceTranscript(live, { type: 'local_failed', error: 'x', code: 'runtime_error', atMs: 2020 });
  assert.equal(failed.steps['cmd-step'].command?.state, 'stopped');
  // A command that ended keeps its own end.
  const ended = reduceAll(live, [
    { type: 'command_finished', runId: RUN, stepId: 'cmd-step', commandId: 'c1', state: 'timed_out', exitCode: null, note: null, durationMs: 600000 },
    { type: 'run_finished', runId: RUN, status: 'completed', durationMs: 700000, error: null, providerError: null },
  ]);
  assert.equal(ended.steps['cmd-step'].command?.state, 'timed_out');
  assert.equal(ended.steps['cmd-step'].command?.note, null);
});

test('a command stored while running is restored as stopped by the app closing', () => {
  const live = reduceAll(initialTranscript(RUN, 1000), [
    { type: 'run_started', runId: RUN, sessionId: 's1', model: 'm', atMs: 1000, warning: null },
    commandStep(),
    { type: 'command_started', runId: RUN, stepId: 'cmd-step', commandId: 'c1', command: 'cargo test', timeLimitSecs: 600, atMs: 1020 },
    { type: 'command_output', runId: RUN, stepId: 'cmd-step', commandId: 'c1', text: 'running 3 tests\n' },
  ]);
  for (const stored of [toPersisted(live), toCheckpoint(live)]) {
    const restored = fromPersisted(JSON.parse(JSON.stringify(stored)));
    assert.ok(restored);
    const run = restored.steps['cmd-step'].command;
    assert.equal(run?.state, 'stopped');
    assert.equal(run?.note, 'The app closed before it finished');
    assert.equal(run?.durationMs, null);
    assert.equal(run?.output, 'running 3 tests\n');
  }
  // A finished command round-trips unchanged; a malformed one is dropped.
  const done = reduceAll(live, [
    { type: 'command_finished', runId: RUN, stepId: 'cmd-step', commandId: 'c1', state: 'done', exitCode: 0, note: null, durationMs: 900 },
    { type: 'step_finished', runId: RUN, stepId: 'cmd-step', ok: true, summary: 'exit 0', detail: null, durationMs: 950 },
    { type: 'run_finished', runId: RUN, status: 'completed', durationMs: 2000, error: null, providerError: null },
  ]);
  assert.deepEqual(fromPersisted(JSON.parse(JSON.stringify(toPersisted(done)))), done);
  const broken = JSON.parse(JSON.stringify(toPersisted(done)));
  broken.steps['cmd-step'].command = { commandId: 'c1' };
  const restored = fromPersisted(broken);
  assert.equal(restored?.steps['cmd-step'].command, undefined);
  assert.equal(restored?.steps['cmd-step'].status, 'done');
});

test('stored command output is cut to its tail and marked', () => {
  const big = `${'x'.repeat(MAX_PERSISTED_COMMAND_OUTPUT_CHARS * 2)}LAST LINE\n`;
  const live = reduceAll(initialTranscript(RUN, 1000), [
    commandStep(),
    { type: 'command_started', runId: RUN, stepId: 'cmd-step', commandId: 'c1', command: 'cargo build', timeLimitSecs: 600, atMs: 1020 },
    { type: 'command_output', runId: RUN, stepId: 'cmd-step', commandId: 'c1', text: big },
  ]);
  assert.equal(live.steps['cmd-step'].command?.output, big, 'the live view keeps more than is stored');
  assert.equal(live.steps['cmd-step'].command?.outputCut, false);
  for (const stored of [toPersisted(live), toCheckpoint(live)]) {
    const run = stored.steps['cmd-step'].command;
    assert.equal(run?.output.length, MAX_PERSISTED_COMMAND_OUTPUT_CHARS);
    assert.ok(run?.output.endsWith('LAST LINE\n'));
    assert.equal(run?.outputCut, true);
  }
  // Storing never changes the live transcript.
  assert.equal(live.steps['cmd-step'].command?.output, big);
});
