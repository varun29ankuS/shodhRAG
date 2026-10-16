/**
 * The answer status line's model name and text (src/features/agent/format.ts).
 *   node --experimental-strip-types --test app/tests/agentFormat.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { friendlyModel, statusSummary } from '../src/features/agent/format.ts';

test('a model reads without its provider prefix or a :free suffix', () => {
  assert.equal(friendlyModel('meta-llama/llama-3.3-70b-instruct:free'), 'llama-3.3-70b-instruct');
  assert.equal(friendlyModel('openrouter/anthropic/claude-x'), 'claude-x');
  assert.equal(friendlyModel('gpt-4o-mini'), 'gpt-4o-mini');
  assert.equal(friendlyModel('qwen2.5:7b'), 'qwen2.5:7b');
  assert.equal(friendlyModel('  '), '');
  assert.equal(friendlyModel('vendor/'), 'vendor/');
});

test('the line shows the model, a cost and the time; tokens go to the tooltip', () => {
  const summary = statusSummary({
    model: 'meta-llama/llama-3.3-70b-instruct:free',
    usage: { inputTokens: 14_200, outputTokens: 820, costUsd: 0 },
    elapsedMs: 12_400,
  });
  assert.equal(summary.line, 'llama-3.3-70b-instruct · 12s');
  assert.equal(summary.title, 'meta-llama/llama-3.3-70b-instruct:free · ↑ 14k in · ↓ 820 out');

  const paid = statusSummary({ model: 'anthropic/claude-x', usage: { inputTokens: 900, outputTokens: 40, costUsd: 0.0042 }, elapsedMs: 3_000 });
  assert.equal(paid.line, 'claude-x · $0.0042 · 3.0s');
});

test('without a model or usage the line is the time alone', () => {
  const summary = statusSummary({ model: null, usage: null, elapsedMs: 800 });
  assert.equal(summary.line, '800ms');
  assert.equal(summary.title, undefined);
});
