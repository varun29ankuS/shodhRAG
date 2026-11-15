import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

import { NIGHTLY_CHECK } from '../ci-gate.mjs';
import { VerdictError, nightlyVerdict } from '../nightly-verdict.mjs';

const ROOT = fileURLToPath(new URL('../../../', import.meta.url));
const SCRIPT = join(ROOT, 'scripts', 'release', 'nightly-verdict.mjs');

let nextId = 1;
const job = (name, conclusion, status = 'completed') => ({ name, status, conclusion, databaseId: nextId++, url: `https://example.invalid/${name}` });

// A nightly run's jobs as `gh run view --json jobs` lists them (run
// 43661778614): ci.yml's jobs under the caller's job name "CI", the release
// builds one level deeper, and the report job itself, still running.
function run({ gate = 'success', tour } = {}) {
  const jobs = [
    job('CI / Changes', 'success'),
    job('CI / Workflow lint', 'success'),
    job('CI / Tests', 'success'),
    job('CI / Android build + launch', 'success'),
    job('CI / Release builds / Build (windows-x64)', 'success'),
    job('CI / Release builds / Create draft release', 'skipped'),
  ];
  if (tour) jobs.push(job('CI / Android screenshot tour (not required)', tour));
  if (gate) jobs.push(job(NIGHTLY_CHECK, gate));
  jobs.push(job('Open an issue when the nightly run failed', null, 'in_progress'));
  return jobs;
}

test("the gate it reads is the one a release reads of a nightly run", () => {
  assert.equal(NIGHTLY_CHECK, 'CI / CI gate');
});

test('a job CI gate does not wait for fails without failing the run', () => {
  // After the Android screenshot tour (not required) joined ci.yml: its
  // emulator crashes fail the call of ci.yml, never the gate.
  for (const tour of ['failure', 'cancelled', 'timed_out']) {
    const verdict = nightlyVerdict(run({ tour }));
    assert.equal(verdict.failed, false, tour);
    assert.equal(verdict.gate, 'success');
    assert.deepEqual(verdict.others, [`CI / Android screenshot tour (not required): ${tour}`]);
  }
  // Before it: a passing gate is a passing run.
  assert.deepEqual(nightlyVerdict(run()), { failed: false, gate: 'success', others: [] });
});

test('a gate that did not pass fails the run, whatever else passed', () => {
  for (const gate of ['failure', 'cancelled', 'timed_out', 'skipped', 'neutral', 'action_required', 'stale']) {
    const verdict = nightlyVerdict(run({ gate }));
    assert.equal(verdict.failed, true, gate);
    assert.equal(verdict.gate, gate);
  }
  // A required job and the tour failed together: the gate's failure counts.
  const both = nightlyVerdict([...run({ gate: 'failure', tour: 'failure' }), job('CI / Footprint budget', 'failure')]);
  assert.equal(both.failed, true);
  assert.ok(both.others.includes('CI / Footprint budget: failure'));
});

test('a run without a finished gate failed', () => {
  // The call of ci.yml never reached its gate (an invalid workflow, a
  // cancelled run): nothing says it passed.
  assert.deepEqual(nightlyVerdict(run({ gate: null })), { failed: true, gate: 'missing', others: [] });
  assert.equal(nightlyVerdict([]).failed, true);
  const unfinished = nightlyVerdict([...run({ gate: null }), job(NIGHTLY_CHECK, null, 'in_progress')]);
  assert.deepEqual([unfinished.failed, unfinished.gate], [true, 'in_progress']);
  // A gate of another workflow's name is not this one.
  assert.equal(nightlyVerdict([job('CI gate', 'success')]).failed, true);
});

test("a re-run's gate replaces the gate it repeats", () => {
  const jobs = [...run({ gate: 'failure' }), job(NIGHTLY_CHECK, 'success')];
  assert.equal(nightlyVerdict(jobs).failed, false);
  const worse = [...run(), job(NIGHTLY_CHECK, 'failure')];
  assert.equal(nightlyVerdict(worse).failed, true);
});

test('something that is not a list of jobs is refused', () => {
  for (const jobs of [undefined, null, {}, 'jobs']) {
    assert.throws(() => nightlyVerdict(jobs), VerdictError);
  }
});

test('the script writes the verdict for the workflow', () => {
  const dir = mkdtempSync(join(tmpdir(), 'nightly-verdict-'));
  const output = join(dir, 'output');
  writeFileSync(output, '');
  const check = (jobs, expected) => {
    const file = join(dir, 'jobs.json');
    writeFileSync(file, JSON.stringify({ jobs }));
    writeFileSync(output, '');
    execFileSync(process.execPath, [SCRIPT, '--jobs', file, '--github-output'], {
      env: { ...process.env, GITHUB_OUTPUT: output },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    assert.equal(readFileSync(output, 'utf8'), expected);
  };
  check(run({ tour: 'failure' }), 'failed=false\ngate=success\n');
  check(run({ gate: 'failure' }), 'failed=true\ngate=failure\n');
  check(run({ gate: null }), 'failed=true\ngate=missing\n');
  const bad = join(dir, 'bad.json');
  writeFileSync(bad, 'not json');
  assert.throws(() => execFileSync(process.execPath, [SCRIPT, '--jobs', bad], { stdio: 'pipe' }), /not the JSON/);
});

// nightly.yml's report job: its issue follows the verdict, never the call's
// result alone.
function reportJob() {
  const yml = readFileSync(join(ROOT, '.github', 'workflows', 'nightly.yml'), 'utf8');
  const start = yml.search(/^ {2}report:[ \t]*$/m);
  assert.ok(start >= 0, 'nightly.yml has a report job');
  const rest = yml.slice(start);
  const next = rest.slice(1).search(/^ {2}[A-Za-z_][A-Za-z0-9_-]*:[ \t]*$/m);
  return next < 0 ? rest : rest.slice(0, next + 1);
}

test('the nightly report opens its issue on the verdict of the CI gate', () => {
  const report = reportJob();
  const steps = report.split(/^ {6}- /m).slice(1);
  const verdict = steps.find((s) => /^ {8}id: verdict$/m.test(s));
  assert.ok(verdict, 'a step with id verdict');
  assert.match(verdict, /gh run view "\$GITHUB_RUN_ID" --repo "\$GITHUB_REPOSITORY" --json jobs > "\$RUNNER_TEMP\/jobs\.json"/);
  assert.match(verdict, /node scripts\/release\/nightly-verdict\.mjs --jobs "\$RUNNER_TEMP\/jobs\.json" --github-output/);
  const issue = steps.find((s) => /gh issue create/.test(s));
  assert.ok(issue, 'the step that opens the issue');
  // Opened unless the verdict says the gate passed: a verdict that could not
  // be reached opens it too.
  assert.match(issue, /^ {8}if: \$\{\{ !cancelled\(\) && steps\.verdict\.outputs\.failed != 'false' \}\}$/m);
  assert.doesNotMatch(issue, /needs\.ci\.result/);
  assert.ok(steps.indexOf(verdict) < steps.indexOf(issue), 'the verdict comes first');
  // The checkout the script needs, without credentials, and the permission
  // for it (job permissions replace the workflow's).
  const checkout = steps.find((s) => s.startsWith('uses: actions/checkout@'));
  assert.ok(checkout, 'the report job checks out the scripts');
  assert.match(checkout, /persist-credentials: false/);
  assert.match(report, /^ {6}contents: read\b/m);
});
