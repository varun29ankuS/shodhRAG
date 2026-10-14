/**
 * The reasoning text a provider streams alongside an answer is kept per
 * answer, in order with its tool steps, saved with the conversation, and
 * shown only on demand. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/reasoning.test.ts
 *
 * The panel itself (ReasoningPanel.tsx) renders exactly `reasoningSections`
 * behind a button whose open state is `reasoningOpen`; both are covered here.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import type { AgentEvent } from '../src/features/agent/events.ts';
import {
  MAX_REASONING_CHARS,
  answerText,
  fromPersisted,
  initialTranscript,
  reasoningSections,
  reduceAll,
  toPersisted,
} from '../src/features/agent/reducer.ts';
import { createReasoningOpenStore } from '../src/features/agent/reasoningOpen.ts';

const RUN = 'run-r';

function thinking(delta: string, messageId = 'm1'): AgentEvent {
  return { type: 'thinking', runId: RUN, messageId, delta };
}

function run(events: AgentEvent[]) {
  return reduceAll(initialTranscript(RUN, 1000), [
    { type: 'run_started', runId: RUN, sessionId: 's', model: 'm', atMs: 1000, warning: null },
    ...events,
  ]);
}

const searched: AgentEvent[] = [
  thinking('The user asks how HNSW keeps the graph connected. '),
  thinking('I should search the HNSW paper.'),
  {
    type: 'step_started', runId: RUN, stepId: 'st1', parentStepId: null, tool: 'search_documents',
    label: 'Searching “HNSW connectivity”', args: { query: 'HNSW connectivity' }, tier: 'read', atMs: 1010,
  },
  {
    type: 'step_finished', runId: RUN, stepId: 'st1', ok: true, summary: '1 passage', durationMs: 20,
    detail: { passages: [{ n: 1, file: 'hnsw.pdf', path: 'C:/papers/hnsw.pdf', page: '3', heading: null, score: 0.8, text: 'Fig. 2 caption' }] },
  },
  thinking('Passage [1] is the Fig. 2 caption; it says the heuristic keeps global connectivity.', 'm2'),
  { type: 'text_delta', runId: RUN, messageId: 'm2', delta: 'The heuristic keeps the graph globally connected [1].' },
  { type: 'run_finished', runId: RUN, status: 'completed', durationMs: 900, error: null } as AgentEvent,
];

test('reasoning is kept in order with the tool steps', () => {
  const state = run(searched);
  assert.deepEqual(reasoningSections(state), [
    { afterStep: null, text: 'The user asks how HNSW keeps the graph connected. I should search the HNSW paper.' },
    { afterStep: 'Searching “HNSW connectivity”', text: 'Passage [1] is the Fig. 2 caption; it says the heuristic keeps global connectivity.' },
  ]);
  assert.equal(state.reasoning?.truncated, false);
});

test('reasoning never feeds the answer text or its citations', () => {
  const state = run(searched);
  assert.equal(answerText(state), 'The heuristic keeps the graph globally connected [1].');
  assert.ok(state.blocks.every(b => b.kind !== 'text' || !b.text.includes('I should search')));
  assert.deepEqual(state.passages.map(p => p.n), [1]);
});

test('reasoning is capped at about 32 KB per answer and marked as cut', () => {
  const chunk = 'x'.repeat(10_000);
  const state = run([thinking(chunk), thinking(chunk), thinking(chunk), thinking(chunk), thinking('after the cap')]);
  const kept = (state.reasoning?.segments ?? []).reduce((n, s) => n + s.text.length, 0);
  assert.equal(kept, MAX_REASONING_CHARS);
  assert.equal(state.reasoning?.truncated, true);
  assert.ok(!reasoningSections(state).some(s => s.text.includes('after the cap')));
});

test('reasoning is saved with the conversation and restored', () => {
  const state = run(searched);
  const stored = JSON.parse(JSON.stringify(toPersisted(state)));
  assert.deepEqual(stored.reasoning, state.reasoning);
  const restored = fromPersisted(stored);
  assert.ok(restored);
  assert.deepEqual(reasoningSections(restored), reasoningSections(state));
  // The live status text is not saved.
  assert.equal(restored.thinking, '');
});

test('a stored reasoning log is validated and capped again', () => {
  const state = run(searched);
  const stored = JSON.parse(JSON.stringify(toPersisted(state)));
  stored.reasoning = {
    segments: [{ at: 0, text: 'y'.repeat(MAX_REASONING_CHARS + 5) }, { at: -1, text: 'bad' }, { at: 1, text: 7 }],
    truncated: false,
  };
  const restored = fromPersisted(stored);
  assert.equal(restored?.reasoning?.segments.length, 1);
  assert.equal(restored?.reasoning?.segments[0].text.length, MAX_REASONING_CHARS);
  assert.equal(restored?.reasoning?.truncated, true);
  stored.reasoning = 'not a log';
  assert.equal(fromPersisted(stored)?.reasoning, undefined);
});

test('without reasoning from the provider there is nothing to show', () => {
  const state = run(searched.filter(e => e.type !== 'thinking'));
  assert.equal(state.reasoning, undefined);
  assert.deepEqual(reasoningSections(state), []);
  // Transcripts saved before reasoning was kept restore without it.
  const old = JSON.parse(JSON.stringify(toPersisted(state)));
  delete old.reasoning;
  const restored = fromPersisted(old);
  assert.ok(restored);
  assert.deepEqual(reasoningSections(restored), []);
  // Whitespace only is not reasoning worth a button.
  assert.deepEqual(reasoningSections(run([thinking('  \n ')])), []);
});

test('the panel opens only when the reader opens it, and stays open across unmounts', () => {
  const store = createReasoningOpenStore();
  assert.equal(store.isOpen(RUN), false, 'never opens by itself');
  let notified = 0;
  const unsubscribe = store.subscribe(() => {
    notified += 1;
  });
  store.setOpen(RUN, true);
  assert.equal(notified, 1);
  // The answer scrolls out of the window (its component unmounts) and back:
  // the open state is read again from the store.
  unsubscribe();
  assert.equal(store.isOpen(RUN), true);
  assert.equal(store.isOpen('another-answer'), false);
  store.setOpen(RUN, true);
  store.setOpen(RUN, false);
  assert.equal(store.isOpen(RUN), false);
  assert.equal(notified, 1, 'an unsubscribed listener is not called');
});

/**
 * omp frames recorded from a Research session launched with `--thinking low`
 * ("Think before answering" on) against Nemotron on OpenRouter. The Rust normaliser
 * turns each `thinking_delta` into a `thinking` event with the frame's
 * `messageId` and `delta` (checked in omp.rs against the same file); this
 * replays that stream through the reducer the transcript uses.
 */
const RECORDED = readFileSync(
  new URL('../../crates/shodh-rag/src/harness/fixtures/omp-thinking-low-events.jsonl', import.meta.url),
  'utf8',
)
  .split('\n')
  .filter(line => line.trim() !== '')
  .map(line => JSON.parse(line) as Record<string, any>);

function recordedEvents(): AgentEvent[] {
  const events: AgentEvent[] = [];
  for (const frame of RECORDED) {
    const update = frame.assistantMessageEvent;
    if (frame.type === 'message_update' && update?.type === 'thinking_delta' && update.delta) {
      events.push(thinking(update.delta, frame.messageId));
    } else if (frame.type === 'message_update' && update?.type === 'text_delta' && update.delta) {
      events.push({ type: 'text_delta', runId: RUN, messageId: frame.messageId, delta: update.delta });
    } else if (frame.type === 'host_tool_call') {
      events.push({
        type: 'step_started', runId: RUN, stepId: frame.toolCallId, parentStepId: null, tool: frame.toolName,
        label: 'Searching documents', args: frame.arguments, tier: 'read', atMs: 1010,
      });
      events.push({ type: 'step_finished', runId: RUN, stepId: frame.toolCallId, ok: true, summary: '3 passages', durationMs: 5, detail: null });
    }
  }
  events.push({ type: 'run_finished', runId: RUN, status: 'completed', durationMs: 900, error: null } as AgentEvent);
  return events;
}

test('reasoning recorded at the launch level is stored and gets the Reasoning panel', () => {
  const state = getStateFrame();
  assert.equal(state.data.thinkingLevel, 'low', 'recorded at the level the setting launches with');
  const streamed = RECORDED.filter(f => f.assistantMessageEvent?.type === 'thinking_delta')
    .map(f => f.assistantMessageEvent.delta as string)
    .join('');
  assert.ok(streamed.length > 100, 'the provider streamed reasoning');

  const transcript = run(recordedEvents());
  const sections = reasoningSections(transcript);
  // ReasoningPanel renders its button exactly when there are sections.
  assert.equal(sections.length, 2, 'reasoning before the search and after it');
  assert.equal(sections[0].afterStep, null);
  assert.equal(sections[1].afterStep, 'Searching documents');
  assert.equal(sections.map(s => s.text).join(''), streamed, 'every streamed character is kept');
  assert.equal(transcript.reasoning?.truncated, false);
  assert.ok(answerText(transcript).includes('sixty (60) days'));
  assert.ok(!answerText(transcript).includes(streamed.slice(0, 40)), 'reasoning stays out of the answer');
  // Saved with the conversation, so the panel is there after a restart too.
  const restored = fromPersisted(JSON.parse(JSON.stringify(toPersisted(transcript))));
  assert.deepEqual(restored && reasoningSections(restored), sections);
});

function getStateFrame(): Record<string, any> {
  const frame = RECORDED.find(f => f.type === 'response' && f.command === 'get_state');
  assert.ok(frame, 'the recording holds the session state');
  return frame;
}
