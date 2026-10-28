/**
 * Backup models: the chain Settings shows, the line under an answer a backup model
 * wrote, and when a side run falls back by itself. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/modelFallback.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { AgentEvent } from '../src/features/agent/events.ts';
import { initialTranscript, reduceAll } from '../src/features/agent/reducer.ts';
import {
  MAX_SIDE_FALLBACKS,
  answeredByLine,
  healthWords,
  originLabel,
  readFallbackChain,
} from '../src/features/modelPicker/fallbackChain.ts';
import type { ChainCandidate } from '../src/features/modelPicker/fallbackChain.ts';
import { sideFallback } from '../src/features/focus/sideFallback.ts';

const RUN = 'run-1';
const NEMOTRON = 'nvidia/nemotron-3-super-120b-a12b:free';
const NOW = 1_761_000_000_000;

function wire(id: string, name: string, extra: Record<string, unknown> = {}) {
  return {
    model: { provider: 'openrouter', model: id },
    name,
    origin: 'automatic',
    contextLength: 131072,
    privacy: 'may_log_prompts',
    free: true,
    health: { state: 'healthy' },
    availability: null,
    reachable: true,
    ...extra,
  };
}

test('the chain view is read as the backend sends it, and anything odd is dropped', () => {
  const chain = readFallbackChain({
    plan: {
      requested: wire(NEMOTRON, 'Nemotron Super', {
        origin: 'requested',
        health: { state: 'cooling_down', untilMs: NOW + 120_000, failures: 1, last: 'model_unavailable' },
        availability: { endpoints: 0, serving: 0, uptime30m: null },
      }),
      fallbacks: [
        wire('google/gemma-4-31b-it:free', 'Gemma 4 31B', { availability: { endpoints: 1, serving: 1, uptime30m: 99.6 } }),
        { model: { provider: 'nope', model: 'x' } },
        wire('nvidia/nemotron-3.5-lightning:free', 'Nemotron Lightning', { origin: 'chosen_fallback' }),
      ],
    },
    direct: null,
    alwaysFallBack: true,
    localOnly: false,
    nowMs: NOW,
  });
  assert.ok(chain?.plan);
  assert.equal(chain.plan.requested.health.state, 'cooling_down');
  assert.deepEqual(chain.plan.fallbacks.map(c => c.name), ['Gemma 4 31B', 'Nemotron Lightning'], 'an unknown provider is skipped');
  assert.equal(chain.plan.fallbacks[0].availability?.uptime30m, 99.6);
  assert.equal(chain.alwaysFallBack, true);
  assert.equal(chain.direct, null);
  assert.equal(readFallbackChain('nonsense'), null);
});

test('health and uptime read as plain words', () => {
  const base = readFallbackChain({ plan: { requested: wire(NEMOTRON, 'Nemotron Super'), fallbacks: [] } })!.plan!.requested;
  const at = (c: Partial<ChainCandidate>) => healthWords({ ...base, ...c }, NOW);
  assert.deepEqual(
    at({ health: { state: 'cooling_down', untilMs: NOW + 150_000, failures: 2, last: 'model_unavailable' } }),
    { tone: 'down', text: 'Was unavailable · skipped for 3 more min' },
  );
  assert.equal(at({ health: { state: 'retrying', failures: 1, last: 'timeout' } }).text, 'Was too slow to answer · the next answer tries it again');
  assert.equal(at({ availability: { endpoints: 0, serving: 0, uptime30m: null } }).text, 'No provider serves it right now');
  assert.deepEqual(at({ availability: { endpoints: 2, serving: 2, uptime30m: 99.4 } }), { tone: 'ok', text: '99% uptime (last 30 min)' });
  assert.equal(at({ availability: { endpoints: 1, serving: 1, uptime30m: 62 } }).tone, 'down');
  assert.equal(at({}).text, 'Uptime not checked yet', 'a free model before its uptime was fetched');
  assert.equal(at({ free: false }).text, 'Working');
  assert.equal(originLabel('chosen_fallback'), 'Your backup');
  assert.equal(originLabel('automatic'), 'Chosen by Shodh');
});

test('the line under a backup answer names both models', () => {
  assert.equal(
    answeredByLine({ from: `openrouter/${NEMOTRON}`, kind: 'model_unavailable', fromName: 'Nemotron Super', toName: 'Gemma 4 31B' }, 'openrouter/google/gemma-4-31b-it:free'),
    'Answered by Gemma 4 31B — Nemotron Super is unavailable',
  );
  // A fallback stored before names were kept reads the model ids.
  assert.equal(
    answeredByLine({ from: 'openrouter/qwen/qwen3-coder:free', kind: 'rate_limited' }, 'anthropic/claude-haiku-4-5'),
    'Answered by claude-haiku-4-5 — qwen/qwen3-coder:free is rate-limited',
  );
});

function failedRun(text: string | null, kind: 'model_unavailable' | 'auth' | 'rate_limited' = 'model_unavailable') {
  const events: AgentEvent[] = [
    { type: 'run_started', runId: RUN, sessionId: 's1', model: `openrouter/${NEMOTRON}`, atMs: 1000, warning: null },
    ...(text ? [{ type: 'text_delta', runId: RUN, messageId: 'm1', delta: text } as AgentEvent] : []),
    {
      type: 'run_finished', runId: RUN, status: 'error', durationMs: 900, error: 'No endpoints found',
      providerError: { kind, status: null, retryAfterSecs: null },
    },
  ];
  return reduceAll(initialTranscript(RUN, 1000), events);
}

test('a side run falls back by itself only when nothing was shown and another model can help', () => {
  const fallback = sideFallback(failedRun(null), 0);
  assert.deepEqual(fallback, {
    failed: { provider: 'openrouter', model: NEMOTRON },
    kind: 'model_unavailable',
    detail: 'No endpoints found',
  });
  assert.equal(sideFallback(failedRun('The notice period is '), 0), null, 'part of the answer was shown: no switch');
  assert.equal(sideFallback(failedRun(null, 'auth'), 0), null, 'a bad key is not routed around');
  assert.ok(sideFallback(failedRun(null, 'rate_limited'), 0));
  assert.equal(sideFallback(failedRun(null), MAX_SIDE_FALLBACKS), null, `at most ${MAX_SIDE_FALLBACKS} fallbacks`);
  const done = reduceAll(initialTranscript(RUN, 1000), [
    { type: 'run_started', runId: RUN, sessionId: 's1', model: `openrouter/${NEMOTRON}`, atMs: 1000, warning: null },
    { type: 'text_delta', runId: RUN, messageId: 'm1', delta: 'Done.' },
    { type: 'run_finished', runId: RUN, status: 'completed', durationMs: 900, error: null, providerError: null },
  ]);
  assert.equal(sideFallback(done, 0), null);
});
