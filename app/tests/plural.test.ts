/**
 * Counts read in the right number. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/plural.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { countOf, nounFor } from '../src/lib/plural.ts';

test('one source, never "1 sources"', () => {
  assert.equal(countOf(1, 'source'), '1 source');
  assert.equal(countOf(0, 'source'), '0 sources');
  assert.equal(countOf(2, 'source'), '2 sources');
  assert.equal(countOf(1204, 'file'), '1,204 files');
  assert.equal(countOf(1, 'match', 'matches'), '1 match');
  assert.equal(countOf(3, 'memory', 'memories'), '3 memories');
  assert.equal(nounFor(1, 'tool'), 'tool');
  assert.equal(nounFor(56, 'tool'), 'tools');
});
