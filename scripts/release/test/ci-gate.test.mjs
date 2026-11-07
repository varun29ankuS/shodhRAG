import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

import { GateError, NIGHTLY_CHECK, REQUIRED_CHECKS, evaluateChecks, waitForCi } from '../ci-gate.mjs';

const SHA = 'a'.repeat(40);
let nextId = 1;
const run = (name, status, conclusion = null, slug = 'github-actions') => ({ id: nextId++, name, status, conclusion, app: { slug } });
const allPassed = () => REQUIRED_CHECKS.map((n) => run(n, 'completed', 'success'));

const ROOT = fileURLToPath(new URL('../../../', import.meta.url));
const WORKFLOWS = join(ROOT, '.github', 'workflows');
const workflow = (file) => readFileSync(join(WORKFLOWS, file), 'utf8');

// A workflow's jobs: each id, its block (up to the next job) and its name
// (quotes and a trailing comment removed). Fails when there is no jobs: key,
// so a test over a workflow can never pass on nothing.
function jobsOf(file) {
  const yml = workflow(file);
  const start = yml.search(/^jobs:[ \t]*(#.*)?$/m);
  assert.ok(start >= 0, `${file} has a top-level jobs: key`);
  const section = yml.slice(start);
  const starts = [...section.matchAll(/^ {2}([A-Za-z_][A-Za-z0-9_-]*):[ \t]*(#.*)?$/gm)];
  assert.ok(starts.length > 0, `${file} has jobs`);
  return starts.map((m, i) => {
    const block = section.slice(m.index, starts[i + 1]?.index ?? section.length);
    const raw = block.match(/^ {4}name:[ \t]*(.+)$/m)?.[1];
    const name = raw
      ?.replace(/[ \t]+#.*$/, '')
      .trim()
      .replace(/^(['"])(.*)\1$/, '$2');
    return { id: m[1], block, name, calls: /^ {4}uses:[ \t]/m.test(block) };
  });
}
const ciJobs = () => jobsOf('ci.yml');

test("the required checks are exactly ci.yml's jobs (a reusable workflow call's checks are its jobs')", () => {
  const names = ciJobs()
    .filter((j) => !j.calls)
    .map((j) => j.name);
  assert.ok(!names.includes(undefined), 'every ci.yml job has a name');
  assert.deepEqual([...names].sort(), [...REQUIRED_CHECKS].sort());
  // The one call: the release builds, counted through CI gate.
  assert.deepEqual(
    ciJobs()
      .filter((j) => j.calls)
      .map((j) => j.id),
    ['release-builds'],
  );
});

test("no other workflow has a job named like one of ci.yml's checks", () => {
  // The gate and branch protection find checks by name on the commit, and
  // every workflow's check runs come from the same app: a job of the same
  // name elsewhere (tag-release.yml's wait was once named CI passed) would be
  // read in its place.
  const files = readdirSync(WORKFLOWS).filter((f) => /\.ya?ml$/.test(f) && f !== 'ci.yml');
  assert.ok(files.length >= 4, 'the other workflows are found');
  for (const file of files) {
    for (const { name } of jobsOf(file)) {
      if (name) assert.ok(!REQUIRED_CHECKS.includes(name), `${file} has a job named ${name}`);
    }
  }
  // Nor is the aggregate the old name, which older check runs still carry.
  assert.ok(!REQUIRED_CHECKS.includes('CI passed'));
});

test("the nightly check the gate reads is nightly.yml's call of ci.yml", () => {
  const ci = jobsOf('nightly.yml').find((j) => j.id === 'ci');
  assert.ok(ci, 'nightly.yml has a ci job');
  assert.match(ci.block, /^ {4}uses: \.\/\.github\/workflows\/ci\.yml$/m);
  assert.equal(NIGHTLY_CHECK, `${ci.name} / CI gate`);
});

test('CI gate waits for every other job of ci.yml', () => {
  const jobs = ciJobs();
  const aggregate = jobs.find((j) => j.id === 'ci-gate');
  assert.ok(aggregate, 'ci.yml has a ci-gate job');
  assert.equal(aggregate.name, 'CI gate');
  assert.match(aggregate.block, /^ {4}if: always\(\)$/m);
  const needs = aggregate.block
    .match(/^ {4}needs: \[(.+)\]$/m)[1]
    .split(',')
    .map((s) => s.trim());
  const others = jobs.map((j) => j.id).filter((id) => id !== 'ci-gate');
  assert.deepEqual([...needs].sort(), [...others].sort());
});

test('a job skipped by the Changes job is decided by its own output', () => {
  // CI gate accepts a skipped job only when the Changes output named after
  // that job is 'false', so each filtered job must read its own output, and
  // every output must belong to a filtered job.
  const jobs = ciJobs();
  const changes = jobs.find((j) => j.id === 'changes');
  const outputs = [...changes.block.matchAll(/^ {6}([a-z][a-z0-9-]*): \$\{\{ steps\.filter\.outputs\.\1 \}\}$/gm)].map((m) => m[1]);
  const filtered = [];
  for (const job of jobs) {
    const condition = job.block.match(/^ {4}if: (.+)$/m)?.[1];
    if (!condition?.includes('needs.changes.outputs.')) continue;
    assert.equal(condition, `needs.changes.outputs.${job.id} == 'true'`, job.id);
    assert.match(job.block, /^ {4}needs: changes$/m, job.id);
    filtered.push(job.id);
  }
  assert.deepEqual([...outputs].sort(), [...filtered].sort());
  assert.deepEqual([...filtered].sort(), ['eval-retrieval', 'eval-retrieval-int8', 'footprint', 'release-builds']);
  // The script's own list of the jobs it decides is the same set.
  const heavy = changes.block.match(/^ +heavy=\((.+)\)$/m)[1].split(/\s+/);
  assert.deepEqual([...heavy].sort(), [...filtered].sort());
});

// The Changes job's skip sets, read from ci.yml: each name='...' or
// name="..." assignment, with $other expanded. The sets are bash extended
// regular expressions written in the subset JavaScript reads the same way.
function skipSets() {
  const block = ciJobs().find((j) => j.id === 'changes').block;
  const vars = {};
  for (const m of block.matchAll(/^ +([a-z_]+)=(?:'([^']*)'|"([^"]*)")$/gm)) {
    vars[m[1]] =
      m[2] ??
      m[3].replace(/\$([a-z_]+)/g, (_, v) => {
        assert.ok(v in vars, `${m[1]} uses ${v} before it is set`);
        return vars[v];
      });
  }
  for (const v of ['eval_skip', 'eval_keep', 'footprint_skip', 'release_skip']) assert.ok(vars[v], `ci.yml sets ${v}`);
  return Object.fromEntries(Object.entries(vars).map(([k, v]) => [k, new RegExp(v)]));
}

function rustFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    if (['target', 'gen', 'node_modules'].includes(entry)) continue;
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) out.push(...rustFiles(path));
    else if (entry.endsWith('.rs')) out.push(path);
  }
  return out;
}

const repoPath = (abs) => relative(ROOT, abs).split(sep).join('/');
const crateRoot = (file) => {
  let dir = dirname(file);
  while (!existsSync(join(dir, 'Cargo.toml'))) dir = dirname(dir);
  return dir;
};

// Every file a crate reads by path: include_str!/include_bytes!/include!
// (relative to the source file, built into the binary) and string literals
// joined to CARGO_MANIFEST_DIR in the same statement (relative to the crate,
// read by its tests).
function pathsReadBy(crateDirs) {
  const refs = [];
  for (const crate of crateDirs) {
    for (const file of rustFiles(join(ROOT, crate))) {
      const text = readFileSync(file, 'utf8');
      for (const m of text.matchAll(/include(?:_str|_bytes)?!\(\s*"([^"]+)"/g)) {
        refs.push({ kind: 'include', from: repoPath(file), path: repoPath(join(dirname(file), m[1])) });
      }
      for (const m of text.matchAll(/env!\(\s*"CARGO_MANIFEST_DIR"\s*\)/g)) {
        const rest = text.slice(m.index + m[0].length, m.index + m[0].length + 400);
        const end = rest.indexOf(';');
        const statement = end < 0 ? rest : rest.slice(0, end);
        for (const lit of statement.matchAll(/"([^"\\]+)"/g)) {
          if (!lit[1].includes('/')) continue;
          refs.push({ kind: 'manifest', from: repoPath(file), path: repoPath(join(crateRoot(file), lit[1].replace(/^\//, ''))) });
        }
      }
    }
  }
  for (const r of refs) assert.ok(!r.path.startsWith('..'), `${r.from} reads ${r.path}, outside the repository`);
  return refs;
}

const skippable = (path, skip, keep) => (skip.test(path) || skip.test(`${path}/x`)) && !(keep && keep.test(path));

test('no file the evals build or read is in their skip set', () => {
  const { eval_skip, eval_keep } = skipSets();
  const refs = pathsReadBy(['crates/shodh-rag', 'crates/shodh-eval', 'crates/shodh-ontology']);
  // Not vacuous: the one app file shodh-rag's tests include, and the eval
  // corpus files its tests read, are found.
  assert.ok(
    refs.some((r) => r.path === 'app/src/features/agent/events.ts'),
    'the events.ts include is found',
  );
  assert.ok(
    refs.some((r) => r.kind === 'manifest' && r.path.startsWith('crates/shodh-eval/data/')),
    'the corpus reads are found',
  );
  for (const r of refs) assert.ok(!skippable(r.path, eval_skip, eval_keep), `${r.from} reads ${r.path}, which the evals may skip`);
});

test('no file built into the app is in the footprint or release-build skip sets', () => {
  // The app's binary holds what its crates include; files tests read through
  // CARGO_MANIFEST_DIR are not part of it (shodh-rag's tests read the eval
  // corpora under crates/shodh-eval, which the app never builds).
  const { footprint_skip, release_skip } = skipSets();
  const refs = pathsReadBy(['app/src-tauri', 'crates/shodh-rag', 'crates/shodh-ontology']).filter((r) => r.kind === 'include');
  assert.ok(
    refs.some((r) => r.path === 'app/src-tauri/tauri.conf.json'),
    'the tauri.conf.json include is found',
  );
  for (const r of refs) {
    assert.ok(!skippable(r.path, footprint_skip), `${r.from} includes ${r.path}, which Footprint budget may skip`);
    assert.ok(!skippable(r.path, release_skip), `${r.from} includes ${r.path}, which the release builds may skip`);
  }
});

test('the skip sets skip documentation and keep everything a build reads', () => {
  const { eval_skip, eval_keep, footprint_skip, release_skip } = skipSets();
  for (const docs of ['README.md', 'docs/adr/0001-agent-harness-omp.md', 'marketing/x.png']) {
    for (const skip of [eval_skip, footprint_skip, release_skip]) assert.ok(skip.test(docs), docs);
  }
  const everyBuild = [
    'crates/shodh-rag/src/search/hybrid.rs',
    'crates/shodh-ontology/src/lib.rs',
    'Cargo.lock',
    'Cargo.toml',
    'rust-toolchain.toml',
    '.gitattributes',
    '.github/workflows/ci.yml',
    '.github/actions/onnxruntime/action.yml',
    'scripts/ci/ort_archive.py',
    'crates/shodh-eval/data/papers/corpus/notes.md',
  ];
  for (const path of everyBuild) {
    assert.ok(!skippable(path, eval_skip, eval_keep), `the evals skip ${path}`);
    assert.ok(!skippable(path, footprint_skip), `Footprint budget skips ${path}`);
    assert.ok(!skippable(path, release_skip), `the release builds skip ${path}`);
  }
  const installers = [
    'app/src/App.tsx',
    'app/index.html',
    'app/vite.config.ts',
    'app/public/logo.png',
    'app/package-lock.json',
    'app/src-tauri/src/main.rs',
    'app/src-tauri/build.rs',
    'app/src-tauri/tauri.conf.json',
    'app/src-tauri/tauri.windows.conf.json',
    'app/src-tauri/tauri.macos.conf.json',
    'app/src-tauri/tauri.conf.json5',
    'app/src-tauri/Tauri.toml',
    'app/src-tauri/capabilities/main.json',
    'app/src-tauri/permissions/main-window.json',
    'app/src-tauri/icons/icon.ico',
    'crates/shodh-rag/build.rs',
    '.github/workflows/release.yml',
    '.github/workflows/tag-release.yml',
    'scripts/release/verify.mjs',
  ];
  for (const path of installers) assert.ok(!skippable(path, release_skip), `the release builds skip ${path}`);
});

test('a red or running nightly run of the commit holds the release', () => {
  assert.equal(evaluateChecks([...allPassed(), run(NIGHTLY_CHECK, 'completed', 'success')]).state, 'success');
  assert.equal(evaluateChecks([...allPassed(), run(NIGHTLY_CHECK, 'in_progress')]).state, 'pending');
  for (const conclusion of ['failure', 'cancelled', 'timed_out', 'skipped']) {
    assert.equal(evaluateChecks([...allPassed(), run(NIGHTLY_CHECK, 'completed', conclusion)]).state, 'failure', conclusion);
  }
  // A re-run that passed replaces the failure.
  const rerun = [...allPassed(), run(NIGHTLY_CHECK, 'completed', 'failure'), run(NIGHTLY_CHECK, 'completed', 'success')];
  assert.equal(evaluateChecks(rerun).state, 'success');
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
