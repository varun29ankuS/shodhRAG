/**
 * A level opened from a "go deeper" mark asks its prepared question once
 * (src/features/focus/autoAsk.ts).
 *   node --experimental-strip-types --test app/tests/focusAutoAsk.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { autoAskQuestion } from '../src/features/focus/autoAsk.ts';

test('a fresh level with a prepared question asks it', () => {
  assert.equal(autoAskQuestion({ question: 'Why quadratic?', seq: 7 }, 0, new Set()), 'Why quadratic?');
});

test('a level without a prepared question asks nothing', () => {
  assert.equal(autoAskQuestion({ question: null, seq: 7 }, 0, new Set()), null);
  assert.equal(autoAskQuestion({ question: '   ', seq: 7 }, 0, new Set()), null);
});

test('a reopened thread that already has turns is not asked again', () => {
  assert.equal(autoAskQuestion({ question: 'Why quadratic?', seq: 7 }, 2, new Set()), null);
});

test('the same level asks only once', () => {
  assert.equal(autoAskQuestion({ question: 'Why quadratic?', seq: 7 }, 0, new Set([7])), null);
});
