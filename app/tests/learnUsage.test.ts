/**
 * Settings → Memory's reasons for invalid learning answers. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/learnUsage.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { invalidSummary, parseRejections } from '../src/features/memory/learnUsage.ts';

test('one reason reads as the count and the plain reason', () => {
  const rejections = [{ code: 'subject_class_mismatch', reason: 'subject did not fit the class', count: 1 }];
  assert.equal(invalidSummary(1, rejections), '1 invalid: subject did not fit the class');
});

test('several reasons list each with its count, without the refused ones', () => {
  const rejections = [
    { code: 'type_mismatch', reason: 'a value of the wrong type', count: 2 },
    { code: 'ungrounded', reason: 'it did not quote your words', count: 4 },
    { code: 'truncated', reason: 'the answer was cut off at the length limit', count: 1 },
  ];
  assert.equal(
    invalidSummary(3, rejections),
    '3 invalid: a value of the wrong type (2); the answer was cut off at the length limit (1)',
  );
});

test('nothing invalid says nothing, and a count without reasons stays a count', () => {
  assert.equal(invalidSummary(0, [{ code: 'malformed', reason: 'x', count: 1 }]), '');
  assert.equal(invalidSummary(2, []), '2 invalid dropped');
  assert.equal(invalidSummary(1, [{ code: 'ungrounded', reason: 'it did not quote your words', count: 1 }]), '1 invalid dropped');
});

test('the status list is parsed strictly and may be missing', () => {
  assert.deepEqual(parseRejections(undefined), []);
  assert.deepEqual(
    parseRejections([
      { code: 'malformed', reason: 'a memory was not in the expected shape', count: 2 },
      { code: 'x', reason: 'y', count: 0 },
      { code: 7, reason: 'y', count: 1 },
      'junk',
    ]),
    [{ code: 'malformed', reason: 'a memory was not in the expected shape', count: 2 }],
  );
});
