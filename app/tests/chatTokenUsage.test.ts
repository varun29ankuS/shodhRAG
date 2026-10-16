/**
 * Conversation token usage tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/chatTokenUsage.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chatTokenUsage, tokenUsageLine } from '../src/features/ask/tokenUsage.ts';

const usage = (inputTokens: number, outputTokens: number) => ({
  transcript: { usage: { inputTokens, outputTokens, cacheReadTokens: 0, costUsd: 0 } },
});

test('sums the counts every answer reported', () => {
  const result = chatTokenUsage([
    { role: 'user' },
    { role: 'assistant', ...usage(12_000, 800) },
    { role: 'user' },
    { role: 'assistant', ...usage(2_000, 450) },
  ]);
  assert.deepEqual(result, { input: 14_000, output: 1_250, counted: 2, answers: 2 });
  assert.equal(tokenUsageLine(result), 'Tokens used: 14k in · 1.3k out');
});

test('older answers report their counts in metadata', () => {
  const result = chatTokenUsage([{ role: 'assistant', metadata: { inputTokens: 900, outputTokens: 100 } }]);
  assert.deepEqual(result, { input: 900, output: 100, counted: 1, answers: 1 });
});

test('answers without counts are named in the line', () => {
  const result = chatTokenUsage([
    { role: 'assistant', ...usage(3_000, 200) },
    { role: 'assistant', transcript: { usage: null } },
    { role: 'assistant', metadata: { model: 'x', inputTokens: null } },
  ]);
  assert.deepEqual(result, { input: 3_000, output: 200, counted: 1, answers: 3 });
  assert.equal(tokenUsageLine(result), 'Tokens used: 3.0k in · 200 out (for 1 of 3 answers)');
});

test('no counts at all: no line', () => {
  assert.equal(chatTokenUsage([{ role: 'assistant', transcript: { usage: null } }]), null);
  assert.equal(chatTokenUsage([]), null);
  assert.equal(tokenUsageLine(null), null);
});

test('notices and user messages are not answers', () => {
  const result = chatTokenUsage([
    { role: 'user', metadata: { inputTokens: 50 } },
    // An OCR or upload notice: posted as the assistant, but no answer was run.
    { role: 'assistant' },
    { role: 'system' },
    { role: 'assistant', ...usage(10, 5) },
  ]);
  assert.deepEqual(result, { input: 10, output: 5, counted: 1, answers: 1 });
});

test('unusable counts are ignored', () => {
  const result = chatTokenUsage([{ role: 'assistant', metadata: { inputTokens: Number.NaN, outputTokens: -3 } }, { role: 'assistant', ...usage(7, 3) }]);
  assert.deepEqual(result, { input: 7, output: 3, counted: 1, answers: 2 });
});
