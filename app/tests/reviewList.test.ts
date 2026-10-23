/**
 * The files a Code command prompt lists as changed without review, and the
 * token only that prompt sends back. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/reviewList.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  REVIEW_BLOCK_KEYS,
  REVIEW_TOKEN_KEY,
  UNREVIEWED_KEY,
  reviewTokenFor,
  unreviewedFiles,
} from '../src/features/agent/reviewList.ts';

test('every listed file is returned, however many there are', () => {
  const files = Array.from({ length: 13 }, (_, i) => `tests/test_${i}.py`);
  assert.deepEqual(unreviewedFiles({ [UNREVIEWED_KEY]: files, review_note: 'x' }), files);
  assert.deepEqual(unreviewedFiles({ [UNREVIEWED_KEY]: ['a.py', 7, null, 'b.py'] }), ['a.py', 'b.py']);
  assert.deepEqual(unreviewedFiles({ command: 'pytest' }), []);
  assert.deepEqual(unreviewedFiles(null), []);
  assert.deepEqual(unreviewedFiles({ [UNREVIEWED_KEY]: 'conftest.py' }), []);
});

test('the token goes back only from a prompt that showed the whole list', () => {
  const preview = { [UNREVIEWED_KEY]: ['conftest.py'], [REVIEW_TOKEN_KEY]: 'abc123' };
  assert.equal(reviewTokenFor(preview, true), 'abc123');
  assert.equal(reviewTokenFor(preview, false), undefined);
  assert.equal(reviewTokenFor({ [UNREVIEWED_KEY]: ['conftest.py'] }, true), undefined);
  assert.equal(reviewTokenFor({ [REVIEW_TOKEN_KEY]: '  ' }, true), undefined);
  assert.equal(reviewTokenFor('pytest', true), undefined);
});

test('the list and the token are not shown as plain rows', () => {
  assert.ok(REVIEW_BLOCK_KEYS.includes(UNREVIEWED_KEY));
  assert.ok(REVIEW_BLOCK_KEYS.includes(REVIEW_TOKEN_KEY));
});
