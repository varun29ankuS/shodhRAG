import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { GateError, REQUIRED_CHECKS, evaluateChecks, waitForCi } from '../ci-gate.mjs';

const SHA = 'a'.repeat(40);
let nextId = 1;
const run = (name, status, conclusion = null, slug = 'github-actions') => ({ id: nextId++, name, status, conclusion, app: { slug } });
const allPassed = () => REQUIRED_CHECKS.map((n) => run(n, 'completed', 'success'));

test("the required checks are exactly ci.yml's jobs", () => {
  const yml = readFileSync(new URL('../../../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const jobs = yml.slice(yml.indexOf('\njobs:\n'));
  const names = [...jobs.matchAll(/^ {2}[a-z][a-z0-9-]*:\n(?: {4}#.*\n)*? {4}name: (.+)$/gm)].map((m) => m[1].trim());
  assert.deepEqual([...names].sort(), [...REQUIRED_CHECKS].sort());
});

test('all required checks passed', () => {
  assert.equal(evaluateChecks(allPassed()).state, 'success');
});

test('a missing or running check is pending', () => {
  const runs = allPassed().filter((r) => r.name !== 'Tests');
  assert.equal(evaluateChecks(runs).state, 'pending');
  assert.equal(evaluateChecks([...runs, run('Tests', 'in_progress')]).state, 'pending');
});

test('any other conclusion fails, and a failure wins over pending', () => {
  for (const conclusion of ['failure', 'cancelled', 'timed_out', 'skipped', 'neutral', 'action_required', 'stale']) {
    const runs = allPassed().filter((r) => r.name !== 'Footprint budget');
    runs.push(run('Footprint budget', 'completed', conclusion));
    assert.equal(evaluateChecks(runs).state, 'failure', conclusion);
  }
  const runs = allPassed().filter((r) => r.name !== 'Tests' && r.name !== 'Format');
  runs.push(run('Tests', 'queued'), run('Format', 'completed', 'failure'));
  assert.equal(evaluateChecks(runs).state, 'failure');
});

test('the newest run of a check counts (a re-run replaces a failure)', () => {
  const runs = allPassed().filter((r) => r.name !== 'Tests');
  runs.push(run('Tests', 'completed', 'failure'), run('Tests', 'completed', 'success'));
  assert.equal(evaluateChecks(runs).state, 'success');
  runs.push(run('Tests', 'completed', 'failure'));
  assert.equal(evaluateChecks(runs).state, 'failure');
});

test('a check of the same name from another app does not count', () => {
  const runs = allPassed().filter((r) => r.name !== 'Tests');
  runs.push(run('Tests', 'completed', 'success', 'someone-elses-app'));
  assert.equal(evaluateChecks(runs).state, 'pending');
});

function fakeApi(sequence) {
  let i = 0;
  return async (url) => {
    assert.match(url, new RegExp(`/commits/${SHA}/check-runs`));
    const runs = sequence[Math.min(i++, sequence.length - 1)];
    return { ok: true, status: 200, json: async () => ({ total_count: runs.length, check_runs: runs }) };
  };
}

test('waits while CI runs, then passes', async () => {
  const running = allPassed().map((r) => (r.name === 'Tests' ? { ...r, status: 'in_progress', conclusion: null } : r));
  let slept = 0;
  await waitForCi({ repo: 'o/r', sha: SHA, token: 't', fetchImpl: fakeApi([running, running, allPassed()]), sleep: async () => slept++ });
  assert.equal(slept, 2);
});

test('fails as soon as a check fails', async () => {
  const failed = allPassed().map((r) => (r.name === 'Tests' ? { ...r, conclusion: 'failure' } : r));
  await assert.rejects(
    waitForCi({ repo: 'o/r', sha: SHA, token: 't', fetchImpl: fakeApi([failed]), sleep: async () => assert.fail('no wait') }),
    (e) => e instanceof GateError && /did not pass/.test(e.message) && /Tests: failure/.test(e.message),
  );
});

test('fails when CI does not finish in time', async () => {
  let t = 0;
  const running = [run('Tests', 'in_progress')];
  await assert.rejects(
    waitForCi({
      repo: 'o/r', sha: SHA, token: 't', timeoutMinutes: 5, intervalSeconds: 60,
      fetchImpl: fakeApi([running]), sleep: async (ms) => (t += ms), now: () => t,
    }),
    /had not finished after 5 minutes/,
  );
  assert.ok(t <= 5 * 60_000);
});

test('refuses something that is not a commit SHA', async () => {
  await assert.rejects(waitForCi({ repo: 'o/r', sha: 'master', token: 't', fetchImpl: fakeApi([[]]) }), /not a commit SHA/);
});
