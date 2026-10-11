/**
 * Table model offer tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/tableModelOffer.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { claimTableModelOffer, PROMPT_SHOWN_KEY } from '../src/features/setup/tableModelOffer.ts';

function memoryStorage(): Pick<Storage, 'getItem' | 'setItem'> & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: key => data.get(key) ?? null,
    setItem: (key, value) => {
      data.set(key, value);
    },
  };
}

test('the offer is claimed once and remembered', () => {
  const storage = memoryStorage();
  assert.equal(claimTableModelOffer(storage), true);
  assert.equal(storage.data.get(PROMPT_SHOWN_KEY), '1');
  assert.equal(claimTableModelOffer(storage), false);
});

test('storage that fails never blocks the offer', () => {
  const broken = {
    getItem: () => {
      throw new Error('denied');
    },
    setItem: () => {
      throw new Error('denied');
    },
  };
  assert.equal(claimTableModelOffer(broken), true);
  assert.equal(claimTableModelOffer(null), true);
});
