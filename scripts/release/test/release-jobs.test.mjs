import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { ROOT, jobsOf } from './workflow-jobs.mjs';

// release.yml's Android jobs: the build never has a key, and the signing job
// runs no build code. A job's block runs up to the next job, so it also holds
// the next job's leading comments: the checks read the block without
// comments.
const jobs = () => jobsOf('release.yml');
const job = (id) => {
  const found = jobs().find((j) => j.id === id);
  assert.ok(found, `release.yml has a ${id} job`);
  return found;
};
const code = (block) =>
  block
    .split('\n')
    .filter((line) => !/^\s*#/.test(line))
    .map((line) => line.replace(/\s#.*$/, ''))
    .join('\n');
const needs = (block) => {
  const list = block.match(/^ {4}needs: \[(.+)\]$/m);
  assert.ok(list, 'the job lists its needs');
  return list[1].split(',').map((s) => s.trim());
};

test('the Android build job reads no secret and is not in the release environment', () => {
  const build = code(job('android').block);
  assert.doesNotMatch(build, /^ {4}environment:/m);
  assert.doesNotMatch(build, /secrets\./);
  for (const signing of [/apksigner/, /keytool/, /signer sign/, /pr-updater-key/, /KEYSTORE/, /TAURI_SIGNING/]) {
    assert.doesNotMatch(build, signing);
  }
  // It hands android-sign the unsigned APK and its digest.
  assert.match(build, /^ {6}unsigned-sha256: \$\{\{ steps\.unsigned\.outputs\.sha256 \}\}$/m);
  assert.match(build, /^ {10}name: android-arm64-unsigned$/m);
});

test('the Android signing job runs no build code', () => {
  const sign = job('android-sign');
  const body = code(sign.block);
  assert.ok(needs(sign.block).includes('android'));
  assert.match(body, /^ {4}environment: \$\{\{ needs\.plan\.outputs\.publish == 'true' && 'release' \|\| '' \}\}$/m);
  const forbidden = [
    /\bnpm\b/,
    /\bnpx\b/,
    /\bcargo (build|install|run|fetch)\b/,
    /gradle/i,
    /rust-cache/,
    /rust-toolchain/,
    /actions\/cache/,
    /setup-java/,
    /setup-android/,
    /working-directory/,
  ];
  for (const pattern of forbidden) assert.doesNotMatch(body, pattern);
  // A sparse checkout of the release scripts and the manifest checks only.
  const sparse = body.match(/^ {10}sparse-checkout: \|\n((?: {12}.+\n)+)/m);
  assert.ok(sparse, 'the checkout is sparse');
  assert.deepEqual(
    sparse[1].trim().split('\n').map((s) => s.trim()),
    ['scripts/release', '.github/actions/android-apk-manifest'],
  );
  // It signs only the download whose SHA-256 the build recorded.
  assert.match(body, /UNSIGNED_SHA256: \$\{\{ needs\.android\.outputs\.unsigned-sha256 \}\}/);
  assert.match(body, /sha256sum --check --strict/);
  // The keystore handling.
  assert.match(body, /umask 077/);
  assert.match(body, /mktemp -d "\$RUNNER_TEMP\/android-key\.XXXXXXXX"/);
  assert.match(body, /trap 'rm -rf "\$keys"' EXIT/);
  assert.match(body, /--ks-pass env:ANDROID_KEY_PASSWORD --key-pass env:ANDROID_KEY_PASSWORD/);
});

test('only the Android signing job reads the Android key', () => {
  const readers = jobs()
    .filter((j) => /secrets\.ANDROID_/.test(code(j.block)))
    .map((j) => j.id);
  assert.deepEqual(readers, ['android-sign']);
});

test('verify and publish take the APK android-sign uploaded', () => {
  for (const id of ['verify', 'publish']) {
    const list = needs(job(id).block);
    assert.ok(list.includes('android-sign'), `${id} needs android-sign`);
    assert.ok(!list.includes('android'), `${id} does not take the unsigned build's result for the signed APK's`);
  }
  assert.match(code(job('verify').block), /needs\.android-sign\.result == 'success'/);
  const uploaders = jobs()
    .filter((j) => /^ {10}name: shodh-android-arm64$/m.test(code(j.block)))
    .map((j) => j.id);
  assert.deepEqual(uploaders, ['android-sign']);
});

test("the signing job's Tauri CLI is the version the phone app's lockfile pins", () => {
  const body = code(job('android-sign').block);
  const pinned = body.match(/^ {6}TAURI_CLI_VERSION: (\S+)$/m)?.[1];
  assert.ok(pinned, 'android-sign pins TAURI_CLI_VERSION');
  assert.match(body, /^ {6}TAURI_CLI_SHA256: [0-9a-f]{64}$/m);
  const lock = JSON.parse(readFileSync(join(ROOT, 'mobile', 'package-lock.json'), 'utf8'));
  assert.equal(pinned, lock.packages['node_modules/@tauri-apps/cli'].version);
});

test("the Windows launch job's views tour is the probe's own list, in its order", () => {
  const probe = readFileSync(join(ROOT, 'app', 'src-tauri', 'src', 'launch_probe.rs'), 'utf8');
  const list = (name) => {
    const body = new RegExp(String.raw`pub const ${name}: &\[&str\] = &\[([^\]]*)\];`).exec(probe);
    assert.ok(body, `launch_probe.rs has ${name}`);
    return [...body[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  };
  const body = code(job('launch-windows').block);
  const quoted = (line) => [...line.matchAll(/'([^']+)'/g)].map((m) => m[1]);
  const views = body.match(/^ +\$tourViews = (.+)$/m);
  const themes = body.match(/^ +\$tourThemes = (.+)$/m);
  assert.ok(views && themes, 'the job lists the views and themes it photographs');
  assert.deepEqual(quoted(views[1]), list('TOUR_VIEWS'));
  assert.deepEqual(quoted(themes[1]), list('TOUR_THEMES'));
  // The job asks for the tour by its name, waits for each view by the probe's line, and
  // answers with the file the probe waits for.
  assert.match(body, /\$env:SHODH_LAUNCH_PROBE_TOUR = 'views'/);
  assert.match(probe, /pub const TOUR_VIEWS_NAME: &str = "views";/);
  assert.match(body, /\$shown = "Launch probe: view \$view in \$theme shown"/);
  assert.match(probe, /tracing::info!\("Launch probe: view \{view\} in \{theme\} shown"\);/);
  assert.match(body, /Join-Path \$data "launch-probe-captured-view-\$view-\$theme"/);
  assert.match(probe, /format!\("launch-probe-captured-view-\{view\}-\{theme\}"\)/);
  // Every picture is taken, or the job fails.
  assert.match(body, /if \(\$tourShots -ne \$tourExpected\) \{ throw /);
  // Every view's text is measured, and one cut-off text fails the job: by the probe's
  // own FAIL line, and by the count the tour ends with.
  assert.match(probe, /"Launch probe: view \{view\} in \{theme\} text page=\{\}x\{\} clipped=\{\}"/);
  assert.match(body, /if \(\$tourMeasured -ne \$tourExpected\) \{ throw /);
  assert.match(probe, /"Launch probe: views tour done views=\{\} themes=\{\} clipped=\{clipped_total\}"/);
  assert.match(body, /views tour done views=\$\(\$tourViews\.Count\) themes=\$\(\$tourThemes\.Count\) clipped=0\\s\*\$" \}\)\) \{ throw /);
  assert.match(probe, /if clipped_total > 0 \{\s+return Err\(/);
  // The macOS launch job does not ask for the tour, so the probe there ends as before.
  assert.doesNotMatch(code(job('launch-macos').block), /SHODH_LAUNCH_PROBE_TOUR/);
});
