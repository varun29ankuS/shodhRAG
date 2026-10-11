/**
 * Focus pop-out: suggested next questions of side answers.
 *   node --experimental-strip-types --test app/tests/focusFollowups.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  FOLLOWUPS_INSTRUCTION,
  MAX_FOLLOWUP_BLOCK_CHARS,
  MAX_FOLLOWUP_CHARS,
  MAX_FOLLOWUPS,
  parseFollowupPayload,
  readFollowups,
  splitFollowups,
  stripFollowups,
} from '../src/features/focus/followups.ts';

test('valid block: removed from the body, questions returned', () => {
  const text = 'The spline is $C^2$.\n\n```followups\n["Why C2?", "What about knots?", "Show an example"]\n```';
  const { body, followups } = splitFollowups(text);
  assert.equal(body, 'The spline is $C^2$.');
  assert.deepEqual(followups, ['Why C2?', 'What about knots?', 'Show an example']);
});

test('count and length caps; duplicates and non-strings dropped', () => {
  const items = ['One?', 'one?', 2, null, 'Two?', '   ', 'x'.repeat(MAX_FOLLOWUP_CHARS + 1), 'Three?', 'Four?'];
  assert.deepEqual(parseFollowupPayload(JSON.stringify(items)), ['One?', 'Two?', 'Three?']);
  assert.equal(parseFollowupPayload(JSON.stringify(items)).length, MAX_FOLLOWUPS);
  assert.deepEqual(parseFollowupPayload(JSON.stringify(['  spaced \n out  '])), ['spaced out']);
});

test('malformed block: removed, no chips', () => {
  for (const payload of ['not json', '{"q": "a"}', '"just a string"', '[1, 2]', '']) {
    const { body, followups } = splitFollowups(`Answer.\n\`\`\`followups\n${payload}\n\`\`\``);
    assert.equal(body, 'Answer.', payload);
    assert.deepEqual(followups, [], payload);
  }
});

test('oversized block is not parsed', () => {
  const big = JSON.stringify(Array.from({ length: 2000 }, () => 'q?'));
  assert.ok(big.length > MAX_FOLLOWUP_BLOCK_CHARS);
  const { body, followups } = splitFollowups(`A.\n\`\`\`followups\n${big}\n\`\`\``);
  assert.equal(body, 'A.');
  assert.deepEqual(followups, []);
});

test('streaming: an open block and a half-typed opening line are hidden', () => {
  assert.equal(stripFollowups('Answer.\n```followups\n["Wh'), 'Answer.');
  assert.equal(stripFollowups('Answer.\n```fol'), 'Answer.');
  assert.equal(stripFollowups('Answer.\n```py'), 'Answer.\n```py', 'other code fences stay');
  assert.equal(stripFollowups('Answer.\n```'), 'Answer.\n```', 'a bare fence may be any block');
});

test('other fences and text are untouched; last block wins', () => {
  const text = '```python\nprint(1)\n```\nMore text.\n```followups\n["a"]\n```\n```followups\n["b"]\n```';
  const { body, followups } = splitFollowups(text);
  assert.equal(body, '```python\nprint(1)\n```\nMore text.');
  assert.deepEqual(followups, ['b']);
  assert.deepEqual(splitFollowups('No block here.'), { body: 'No block here.', followups: [] });
});

test('stored followups are read defensively', () => {
  assert.equal(readFollowups('x'), undefined);
  assert.equal(readFollowups([]), undefined);
  assert.deepEqual(readFollowups(['a', 1, 'b', 'c', 'd']), ['a', 'b', 'c']);
});

test('the instruction shows the exact block shape', () => {
  assert.match(FOLLOWUPS_INSTRUCTION, /```followups\n\[.*\]\n```/);
  // The instruction itself, echoed back, parses as a valid block.
  assert.deepEqual(splitFollowups(`x\n${FOLLOWUPS_INSTRUCTION.split('\n').slice(1).join('\n')}`).followups, ['…']);
});
