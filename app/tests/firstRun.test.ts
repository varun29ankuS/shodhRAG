/**
 * First-run setup state tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/firstRun.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  FIRST_RUN_KEYS,
  FIRST_RUN_STEPS,
  nextStep,
  previousStep,
  readFirstRun,
  resumeStep,
  writeFirstRun,
} from '../src/features/setup/firstRun.ts';

function memoryStore(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    data,
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => { data.set(k, v); },
  };
}

test('a new viewer starts setup at the welcome step', () => {
  assert.deepEqual(readFirstRun(memoryStore()), { status: 'pending', step: 'welcome' });
});

test('people who finished or skipped the old onboarding are not onboarded again', () => {
  assert.equal(readFirstRun(memoryStore({ [FIRST_RUN_KEYS.legacyCompleted]: 'true' })).status, 'completed');
  assert.equal(readFirstRun(memoryStore({ [FIRST_RUN_KEYS.legacySkipped]: 'true' })).status, 'skipped');
});

test('saved state round-trips and wins over legacy keys', () => {
  const store = memoryStore({ [FIRST_RUN_KEYS.legacyCompleted]: 'true' });
  writeFirstRun(store, { status: 'pending', step: 'model' });
  assert.deepEqual(readFirstRun(store), { status: 'pending', step: 'model' });
});

test('unknown saved steps fall back to welcome', () => {
  const store = memoryStore({ [FIRST_RUN_KEYS.status]: 'pending', [FIRST_RUN_KEYS.step]: 'nonsense' });
  assert.deepEqual(readFirstRun(store), { status: 'pending', step: 'welcome' });
});

test('missing or failing storage never traps anyone in setup', () => {
  assert.equal(readFirstRun(null).status, 'completed');
  const throwing = { getItem: () => { throw new Error('denied'); }, setItem: () => { throw new Error('denied'); } };
  assert.equal(readFirstRun(throwing).status, 'completed');
  assert.doesNotThrow(() => writeFirstRun(throwing, { status: 'skipped', step: 'search' }));
});

test('steps move forward and back within bounds', () => {
  assert.deepEqual([...FIRST_RUN_STEPS], ['welcome', 'search', 'model', 'folder', 'done']);
  assert.equal(nextStep('welcome'), 'search');
  assert.equal(nextStep('done'), 'done');
  assert.equal(previousStep('search'), 'welcome');
  assert.equal(previousStep('welcome'), 'welcome');
});

test('resuming returns to the saved step, but not to the final summary', () => {
  assert.equal(resumeStep({ status: 'skipped', step: 'folder' }), 'folder');
  assert.equal(resumeStep({ status: 'completed', step: 'done' }), 'welcome');
});
