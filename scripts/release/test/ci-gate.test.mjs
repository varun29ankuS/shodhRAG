import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, sep } from 'node:path';
import { test } from 'node:test';

import { GateError, NIGHTLY_CHECK, NOT_REQUIRED_CHECKS, REQUIRED_CHECKS, evaluateChecks, waitForCi } from '../ci-gate.mjs';
import { ROOT, WORKFLOWS, jobsOf } from './workflow-jobs.mjs';

const SHA = 'a'.repeat(40);
let nextId = 1;
const run = (name, status, conclusion = null, slug = 'github-actions') => ({ id: nextId++, name, status, conclusion, app: { slug } });
const allPassed = () => REQUIRED_CHECKS.map((n) => run(n, 'completed', 'success'));

const ciJobs = () => jobsOf('ci.yml');

test("the required checks are exactly ci.yml's jobs (a reusable workflow call's checks are its jobs')", () => {
  const names = ciJobs()
    .filter((j) => !j.calls)
    .map((j) => j.name);
  assert.ok(!names.includes(undefined), 'every ci.yml job has a name');
  // Every job is required but the named few, and none is both.
  assert.deepEqual([...names].sort(), [...REQUIRED_CHECKS, ...NOT_REQUIRED_CHECKS].sort());
  for (const name of NOT_REQUIRED_CHECKS) assert.ok(!REQUIRED_CHECKS.includes(name), `${name} is both required and not`);
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
      if (name) assert.ok(![...REQUIRED_CHECKS, ...NOT_REQUIRED_CHECKS].includes(name), `${file} has a job named ${name}`);
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

test("the Tests job's slow steps run in parallel jobs (#162)", () => {
  // Tests ran into its 60-minute limit with everything in one job: the app's
  // tests and doc tests run in App tests, and the ANN recall test in the
  // Library store job, each in parallel. Within a job every cargo test selects
  // the same packages, so Cargo resolves the same features and reuses the
  // job's build: run 43788930095 built shodh-rag again for a step selecting
  // other packages (21 minutes) and for the workspace's doc tests (19).
  const jobs = ciJobs();
  const block = (id) => {
    const job = jobs.find((j) => j.id === id);
    assert.ok(job, `ci.yml has a ${id} job`);
    return job.block;
  };
  const cargoTests = (text) => text.split('\n').filter((l) => /^\s*run: cargo test /.test(l));
  const tests = block('test');
  assert.match(tests, /cargo test --workspace --exclude shodh-rag-app --all-targets$/m);
  assert.match(tests, /cargo test --workspace --exclude shodh-rag-app --doc$/m);
  assert.match(tests, /harness::omp_live_tests::a_system_prompt_longer_than_a_command_line_reaches_omp_whole$/m);
  assert.doesNotMatch(tests, /ann_recall_tests/);
  for (const line of cargoTests(tests)) {
    assert.match(line, /cargo test --workspace --exclude shodh-rag-app /, line);
  }
  const app = block('test-app');
  assert.match(app, /cargo test --package shodh-rag-app --all-targets$/m);
  assert.match(app, /cargo test --package shodh-rag-app --doc$/m);
  for (const line of cargoTests(app)) {
    assert.match(line, /cargo test --package shodh-rag-app /, line);
  }
  assert.match(block('upgrade'), /storage::ann_recall_tests::ann_search_past_the_index_threshold_recalls_the_exact_neighbours$/m);
});

test('CI gate waits for every other job of ci.yml but the not-required ones', () => {
  const jobs = ciJobs();
  const aggregate = jobs.find((j) => j.id === 'ci-gate');
  assert.ok(aggregate, 'ci.yml has a ci-gate job');
  assert.equal(aggregate.name, 'CI gate');
  assert.match(aggregate.block, /^ {4}if: always\(\)$/m);
  const needs = aggregate.block
    .match(/^ {4}needs: \[(.+)\]$/m)[1]
    .split(',')
    .map((s) => s.trim());
  const notRequired = jobs.filter((j) => NOT_REQUIRED_CHECKS.includes(j.name)).map((j) => j.id);
  assert.equal(notRequired.length, NOT_REQUIRED_CHECKS.length, 'every not-required check is a ci.yml job');
  const others = jobs.map((j) => j.id).filter((id) => id !== 'ci-gate' && !notRequired.includes(id));
  assert.deepEqual([...needs].sort(), [...others].sort());
  // The gate neither waits for a not-required job nor reads its result:
  // its block never names one.
  for (const id of notRequired) {
    assert.ok(!new RegExp(`(^|[^A-Za-z0-9_-])${id}($|[^A-Za-z0-9_-])`).test(aggregate.block), `CI gate names ${id}`);
  }
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
  assert.deepEqual([...filtered].sort(), ['android', 'eval-recall', 'eval-retrieval', 'eval-retrieval-int8', 'footprint', 'ios', 'release-builds', 'upgrade']);
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
  for (const v of ['eval_skip', 'eval_keep', 'footprint_skip', 'release_skip', 'android_skip', 'android_keep', 'ios_skip', 'ios_keep']) assert.ok(vars[v], `ci.yml sets ${v}`);
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
  // Not vacuous: the app file shodh-rag's tests include, the recorded
  // transcript fixtures they compare with, and the eval corpus files its
  // tests read, are found.
  assert.ok(
    refs.some((r) => r.path === 'app/src/features/agent/events.ts'),
    'the events.ts include is found',
  );
  assert.ok(
    refs.some((r) => r.kind === 'manifest' && r.path === 'app/tests/fixtures/transcripts'),
    'the recorded transcript fixtures are found',
  );
  // A recorded fixture changed runs the evals; a hand-built one does not.
  assert.ok(!skippable('app/tests/fixtures/transcripts/recorded-01-repaired-answer.json', eval_skip, eval_keep));
  assert.ok(skippable('app/tests/fixtures/transcripts/01-cited-answer.json', eval_skip, eval_keep));
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
  const { eval_skip, eval_keep, footprint_skip, release_skip, android_skip, ios_skip } = skipSets();
  const documentation = [
    'README.md',
    'docs/adr/0001-agent-harness-omp.md',
    'docs/FEATURES.md',
    'marketing/x.png',
    // The website and the workflow that deploys it: no build reads them.
    'site/index.html',
    'site/assets/answer.png',
    '.github/workflows/pages.yml',
  ];
  for (const docs of documentation) {
    for (const skip of [eval_skip, footprint_skip, release_skip, android_skip, ios_skip]) assert.ok(skip.test(docs), docs);
  }
  // Only the top-level site/ folder is the website.
  for (const path of ['app/site/index.html', 'crates/shodh-rag/site/x.rs', '.github/workflows/pages-yml']) {
    assert.ok(!release_skip.test(path), `the release builds skip ${path}`);
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
  ];
  // The corpora hold Markdown: the evals never skip them.
  assert.ok(!skippable('crates/shodh-eval/data/papers/corpus/notes.md', eval_skip, eval_keep));
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

test('the android job runs for the phone app and what it reads; a phone-only change skips the desktop jobs', () => {
  const { eval_skip, eval_keep, footprint_skip, release_skip, android_skip, android_keep } = skipSets();
  const phone = [
    'mobile/src/App.tsx',
    'mobile/src/lib/invoke.ts',
    'mobile/tokens.ts',
    'mobile/package-lock.json',
    'mobile/src-tauri/src/link.rs',
    'mobile/src-tauri/Cargo.toml',
    'mobile/src-tauri/Cargo.lock',
    'mobile/src-tauri/tauri.conf.json',
    'mobile/src-tauri/icons/android/mipmap-hdpi/ic_launcher.png',
    'crates/shodh-link/src/client.rs',
    'crates/shodh-link/Cargo.toml',
    // The desktop files the phone app reads: its colour tokens and their parser,
    // and its sections' names and order (the phone's menu).
    'app/src/index.css',
    'app/src/lib/contrast.ts',
    'app/src/lib/viewTabs.ts',
    'app/src/shared/ChatMessage.tsx',
    'rust-toolchain.toml',
    '.github/workflows/ci.yml',
    '.github/workflows/nightly.yml',
    // The manifest checks it shares with the release APK, and the debug-only
    // code check it runs (the one release script it reads).
    '.github/actions/android-apk-manifest/action.yml',
    'scripts/release/android.mjs',
  ];
  for (const path of phone) assert.ok(!skippable(path, android_skip, android_keep), `the android job skips ${path}`);
  const desktopOnly = [
    'README.md',
    'app/src/App-SplitView.tsx',
    'app/src/lib/viewTabsExtra.ts',
    'app/package-lock.json',
    'app/src-tauri/src/phone_link.rs',
    'crates/shodh-rag/src/search/hybrid.rs',
    'crates/shodh-ontology/src/lib.rs',
    'Cargo.lock',
    'Cargo.toml',
    'scripts/release/versions.mjs',
    'scripts/release/verify.mjs',
    'scripts/release/test/android.test.mjs',
    '.github/workflows/release.yml',
  ];
  for (const path of desktopOnly) assert.ok(skippable(path, android_skip, android_keep), `the android job runs for ${path}`);
  // The evals and the footprint build never build the phone app.
  for (const path of ['mobile/src/App.tsx', 'mobile/src-tauri/Cargo.lock', 'mobile/package-lock.json']) {
    assert.ok(skippable(path, eval_skip, eval_keep), `the evals run for ${path}`);
    assert.ok(skippable(path, footprint_skip), `Footprint budget runs for ${path}`);
  }
  // The release builds do: release.yml's android job builds the release APK,
  // which the android job (a debug build) never does.
  for (const path of [
    'mobile/src/App.tsx',
    'mobile/src-tauri/Cargo.lock',
    'mobile/package-lock.json',
    'mobile/src-tauri/src/tour.rs',
    'mobile/src-tauri/tauri.conf.json',
    'mobile/plugins/shodh-android/android/src/main/AndroidManifest.xml',
    '.github/actions/android-apk-manifest/action.yml',
    'scripts/release/android.mjs',
  ]) {
    assert.ok(!skippable(path, release_skip), `the release builds skip ${path}`);
  }
  // The phone link is built into the desktop app too.
  assert.ok(!skippable('crates/shodh-link/src/client.rs', footprint_skip));
  assert.ok(!skippable('crates/shodh-link/src/client.rs', release_skip));
});

test('the ios job runs for the phone app and what it reads; each phone skips only the other phone’s own files', () => {
  const { eval_skip, eval_keep, footprint_skip, release_skip, android_skip, android_keep, ios_skip, ios_keep } = skipSets();
  // What both phones are built from.
  const shared = [
    'mobile/src/App.tsx',
    'mobile/src/lib/platform.ts',
    'mobile/tokens.ts',
    'mobile/package-lock.json',
    'mobile/src-tauri/src/link.rs',
    'mobile/src-tauri/src/vault_ios.rs',
    'mobile/src-tauri/src/vault_android.rs',
    'mobile/src-tauri/Cargo.toml',
    'mobile/src-tauri/Cargo.lock',
    'mobile/src-tauri/tauri.conf.json',
    'mobile/src-tauri/tauri.dev.conf.json',
    // tauri-build parses every capability file, whatever its platforms.
    'mobile/src-tauri/capabilities/default.json',
    'mobile/src-tauri/capabilities/android.json',
    'mobile/src-tauri/capabilities/ios.json',
    // Cargo resolves every path dependency, whatever the target.
    'mobile/plugins/shodh-ios/Cargo.toml',
    'mobile/plugins/shodh-android/Cargo.toml',
    'crates/shodh-link/src/client.rs',
    'app/src/index.css',
    'app/src/lib/contrast.ts',
    'app/src/lib/viewTabs.ts',
    'app/src/shared/ChatMessage.tsx',
    'rust-toolchain.toml',
    '.github/workflows/ci.yml',
    '.github/workflows/nightly.yml',
  ];
  for (const path of shared) {
    assert.ok(!skippable(path, ios_skip, ios_keep), `the ios job skips ${path}`);
    assert.ok(!skippable(path, android_skip, android_keep), `the android job skips ${path}`);
  }
  const iosOnly = [
    'mobile/plugins/shodh-ios/ios/Sources/ShodhPlugin.swift',
    'mobile/plugins/shodh-ios/ios/Package.swift',
    'mobile/plugins/shodh-ios/src/lib.rs',
    'mobile/src-tauri/icons/ios/AppIcon-512@2x.png',
    'mobile/src-tauri/Info.ios.plist',
  ];
  for (const path of iosOnly) {
    assert.ok(!skippable(path, ios_skip, ios_keep), `the ios job skips ${path}`);
    assert.ok(skippable(path, android_skip, android_keep), `the android job runs for ${path}`);
  }
  const androidOnly = [
    'mobile/plugins/shodh-android/android/src/main/java/ShodhPlugin.kt',
    'mobile/plugins/shodh-android/android/build.gradle.kts',
    'mobile/plugins/shodh-android/src/lib.rs',
    'mobile/src-tauri/icons/android/mipmap-hdpi/ic_launcher.png',
    '.github/actions/android-apk-manifest/action.yml',
    'scripts/release/android.mjs',
  ];
  for (const path of androidOnly) {
    assert.ok(!skippable(path, android_skip, android_keep), `the android job skips ${path}`);
    assert.ok(skippable(path, ios_skip, ios_keep), `the ios job runs for ${path}`);
  }
  // The desktop alone, and the TestFlight workflow (the signed build, not this job's).
  for (const path of ['README.md', 'app/src/App-SplitView.tsx', 'crates/shodh-rag/src/search/hybrid.rs', 'Cargo.lock', 'scripts/release/verify.mjs', '.github/workflows/release.yml', '.github/workflows/testflight.yml']) {
    assert.ok(skippable(path, ios_skip, ios_keep), `the ios job runs for ${path}`);
    assert.ok(skippable(path, android_skip, android_keep), `the android job runs for ${path}`);
  }
  // The desktop's jobs never build the iPhone app's own files; the release
  // builds read the TestFlight workflow release.yml calls.
  for (const path of iosOnly) {
    assert.ok(skippable(path, eval_skip, eval_keep), `the evals run for ${path}`);
    assert.ok(skippable(path, footprint_skip), `Footprint budget runs for ${path}`);
  }
  assert.ok(!skippable('.github/workflows/testflight.yml', release_skip));
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
