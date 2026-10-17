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

test('a labelled ```json block of questions is read as follow-ups', () => {
  const text = 'Answer.\n\nFollow-ups:\n```json\n["What is a posting list in SPANN?","How does SPANN balance clusters?"]\n```';
  assert.deepEqual(splitFollowups(text), {
    body: 'Answer.',
    followups: ['What is a posting list in SPANN?', 'How does SPANN balance clusters?'],
  });
  for (const label of ['**Follow-ups:**', 'follow-up questions:', '__Follow-up questions__:', 'Followups']) {
    assert.equal(stripFollowups(`Answer.\n\n${label}\n\`\`\`json\n["Show an example"]\n\`\`\`\n`), 'Answer.', label);
  }
});

test('an untagged final block of questions is read as follow-ups', () => {
  const { body, followups } = splitFollowups('Answer.\n\n```\n["Why C2?", "What about knots?"]\n```');
  assert.equal(body, 'Answer.');
  assert.deepEqual(followups, ['Why C2?', 'What about knots?']);
});

test('a label above a tagged block goes with it', () => {
  const { body, followups } = splitFollowups('Answer.\n\nFollow-ups:\n```followups\n["Why?"]\n```');
  assert.equal(body, 'Answer.');
  assert.deepEqual(followups, ['Why?']);
});

test('json code in an answer is never taken for follow-ups', () => {
  const middle = 'Use this config:\n```json\n["a?", "b?"]\n```\nThen restart.';
  assert.deepEqual(splitFollowups(middle), { body: middle, followups: [] });
  const notLast = 'Config:\n```json\n["a?"]\n```\n```python\nprint(1)\n```';
  assert.deepEqual(splitFollowups(notLast), { body: notLast, followups: [] });
  const objects = 'Follow-ups:\n```json\n[{"q": "Why?"}, {"q": "How?"}]\n```';
  assert.deepEqual(splitFollowups(`Answer.\n\n${objects}`), { body: `Answer.\n\n${objects}`, followups: [] });
  const tooMany = `Follow-ups:\n\`\`\`json\n${JSON.stringify(['a', 'b', 'c', 'd', 'e', 'f'])}\n\`\`\``;
  assert.equal(stripFollowups(`Answer.\n${tooMany}`), `Answer.\n${tooMany}`);
  const longItem = `Follow-ups:\n\`\`\`json\n${JSON.stringify(['x'.repeat(MAX_FOLLOWUP_CHARS + 1)])}\n\`\`\``;
  assert.equal(stripFollowups(`Answer.\n${longItem}`), `Answer.\n${longItem}`);
  // Without a follow-ups label, a final list of strings must all be questions.
  const fields = 'The keys are:\n```json\n["id", "name"]\n```';
  assert.deepEqual(splitFollowups(fields), { body: fields, followups: [] });
  const other = 'Example:\n```ts\n["Why?"]\n```';
  assert.deepEqual(splitFollowups(other), { body: other, followups: [] });
});

test('streaming: a labelled open ```json block is hidden, an unlabelled one stays', () => {
  assert.equal(stripFollowups('Answer.\n\nFollow-ups:\n```json\n["Wh'), 'Answer.');
  assert.equal(stripFollowups('Answer.\n\nFollow-ups:\n```js'), 'Answer.');
  assert.equal(stripFollowups('Answer.\n```json\n["Wh'), 'Answer.\n```json\n["Wh');
});
