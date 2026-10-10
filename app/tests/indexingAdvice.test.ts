/**
 * Indexing failures: one line on what to do and the action that helps.
 *   node --experimental-strip-types --test app/tests/indexingAdvice.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { adviceForFailures, indexingAdvice } from '../src/features/library/indexingAdvice.ts';

test('reasons map to the action that helps', () => {
  assert.equal(indexingAdvice('Search models are not installed; search needs first-run setup').action, 'install_search_models');
  assert.equal(indexingAdvice('The process cannot access the file because it is being used by another process. (os error 32)').action, 'reindex');
  assert.equal(indexingAdvice('Access is denied. (os error 5)').action, 'show_in_folder');
  assert.equal(indexingAdvice('The system cannot find the path specified. No such file or directory').action, 'show_in_folder');
  assert.equal(indexingAdvice('PDF is encrypted with a password').action, 'show_in_folder');
  assert.equal(indexingAdvice('Unsupported file type: .xyz').action, 'none');
  assert.equal(indexingAdvice('Unsupported file type: .xyz').actionLabel, null);
  assert.equal(indexingAdvice('File too large (900 MB exceeds the limit)').action, 'none');
  assert.equal(indexingAdvice('There is not enough space on the disk').action, 'reindex');
});

test('unknown and empty reasons suggest indexing again', () => {
  for (const reason of ['', '   ', null, undefined, 'something odd happened']) {
    const advice = indexingAdvice(reason);
    assert.equal(advice.action, 'reindex');
    assert.equal(advice.actionLabel, 'Index again');
    assert.ok(advice.hint.length > 0);
  }
});

test('a folder gets the advice of its most common failure', () => {
  assert.equal(adviceForFailures([]), null);
  const advice = adviceForFailures(['Unsupported file type: .bin', 'Access is denied.', 'Unsupported file type: .dat']);
  assert.equal(advice?.action, 'none');
  assert.match(advice?.hint ?? '', /cannot read text/);
});
