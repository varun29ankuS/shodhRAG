import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  LOCATIONS,
  VersionError,
  agreedVersion,
  applyVersion,
  compareVersions,
  locateJsonString,
  locateLockVersion,
  locateManifestVersion,
  readVersions,
} from '../versions.mjs';

const repo = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const script = join(repo, 'scripts', 'release', 'versions.mjs');
const files = [...new Set(LOCATIONS.map((l) => l.file))];

function repoTexts() {
  return new Map(files.map((f) => [f, readFileSync(join(repo, f), 'utf8')]));
}

function bump(version) {
  const [x, y, z] = version.split('.').map(Number);
  return `${x}.${y}.${z + 1}`;
}

function copyRepoFiles() {
  const root = mkdtempSync(join(tmpdir(), 'versions-test-'));
  for (const f of files) {
    mkdirSync(dirname(join(root, f)), { recursive: true });
    writeFileSync(join(root, f), readFileSync(join(repo, f)));
  }
  return root;
}

test('every place in the repository has the same version', () => {
  const version = agreedVersion(readVersions(repoTexts()));
  assert.match(version, /^\d+\.\d+\.\d+$/);
});

test('set changes exactly the eight version strings of the real files', () => {
  const texts = repoTexts();
  const current = agreedVersion(readVersions(texts));
  const next = bump(current);
  const { previous, texts: updated } = applyVersion(texts, next);
  assert.equal(previous, current);
  assert.equal(agreedVersion(readVersions(updated)), next);
  let changedLines = 0;
  for (const f of files) {
    const [a, b] = [texts.get(f).split('\n'), updated.get(f).split('\n')];
    assert.equal(a.length, b.length, f);
    for (let k = 0; k < a.length; k++) {
      if (a[k] !== b[k]) {
        changedLines++;
        assert.equal(a[k].replace(current, next), b[k], `${f}:${k + 1}`);
      }
    }
  }
  assert.equal(changedLines, LOCATIONS.length);
});

test('the CLI sets and checks a copy of the repository files', () => {
  const root = copyRepoFiles();
  const current = agreedVersion(readVersions(repoTexts()));
  const next = bump(current);
  execFileSync(process.execPath, [script, 'set', next, '--root', root]);
  const out = execFileSync(process.execPath, [script, 'check', '--tag', `v${next}`, '--root', root], { encoding: 'utf8' });
  assert.match(out, new RegExp(`Version ${next.replace(/\./g, '\\.')}, matches v${next}`));
  assert.throws(() => execFileSync(process.execPath, [script, 'check', '--tag', `v${current}`, '--root', root], { stdio: 'pipe' }));
  assert.throws(() => execFileSync(process.execPath, [script, 'set', current, '--root', root], { stdio: 'pipe' }));
});

test('versions must be X.Y.Z and greater than the current one', () => {
  const texts = repoTexts();
  const current = agreedVersion(readVersions(texts));
  assert.throws(() => applyVersion(texts, current), /must be greater/);
  assert.throws(() => applyVersion(texts, `${current}-rc.1`), /X\.Y\.Z/);
  assert.throws(() => applyVersion(texts, `v${bump(current)}`), /X\.Y\.Z/);
  assert.throws(() => applyVersion(texts, '01.0.0'), /X\.Y\.Z/);
  assert.equal(compareVersions('0.0.10', '0.0.9'), 1);
  assert.equal(compareVersions('0.1.0', '0.0.99'), 1);
  assert.equal(compareVersions('1.2.3', '1.2.3'), 0);
});

test('differing versions are listed, not guessed', () => {
  const texts = repoTexts();
  const current = agreedVersion(readVersions(texts));
  const conf = 'app/src-tauri/tauri.conf.json';
  const span = locateJsonString(texts.get(conf), ['version']);
  texts.set(conf, texts.get(conf).slice(0, span.start) + '9.9.9' + texts.get(conf).slice(span.end));
  assert.throws(() => agreedVersion(readVersions(texts)), (e) => e instanceof VersionError && /9\.9\.9/.test(e.message) && e.message.includes(current));
  assert.throws(() => applyVersion(texts, '10.0.0'), /differs between files/);
});

test('JSON: only the value at the path, not a nested or similar key', () => {
  const text = '{\n  "name": "x",\n  "deps": { "a": { "version": "0.0.10" } },\n  "versions": ["0.0.10"],\n  "version": "0.0.10",\n  "packages": { "": { "version": "0.0.10" }, "node_modules/a": { "version": "0.0.10" } }\n}\n';
  const top = locateJsonString(text, ['version']);
  assert.equal(text.slice(top.start - 12, top.end + 1), '"version": "0.0.10"');
  assert.ok(text.slice(0, top.start).includes('"versions"'));
  const root = locateJsonString(text, ['packages', '', 'version']);
  assert.ok(text.slice(root.start - 30, root.start).includes('"": {'));
  assert.throws(() => locateJsonString('{"version": 1}', ['version']), /not a string/);
  assert.throws(() => locateJsonString('{"a": 1}', ['version']), /no string/);
  assert.throws(() => locateJsonString('{"version": "1", "version": "2"}', ['version']), /more than one/);
  assert.throws(() => locateJsonString('{"version": "1"', ['version']), VersionError);
  assert.equal(locateJsonString('{"a": [1, -2.5e3, true, null, {"b": "\\"q\\""}], "version": "1.2.3"}', ['version']).value, '1.2.3');
});

test('Cargo.toml: the [package] version only', () => {
  const text = [
    '[package]',
    'name = "shodh-rag"',
    'version = "0.0.10"  # the app version',
    'edition = "2021"',
    '',
    '[dependencies]',
    'version = "0.0.10"',
    '',
    "[target.'cfg(windows)'.dependencies]",
    'name = "decoy"',
    '',
  ].join('\n');
  const span = locateManifestVersion(text, 'shodh-rag');
  assert.equal(span.value, '0.0.10');
  assert.equal(text.slice(0, span.start).split('\n').length, 3);
  assert.throws(() => locateManifestVersion(text, 'shodh-rag-app'), /name is "shodh-rag"/);
  assert.throws(() => locateManifestVersion('[package]\nname = "x"\nversion.workspace = true\n', 'x'), /inherited/);
  assert.throws(() => locateManifestVersion('[dependencies]\nversion = "1"\n', 'x'), /one \[package\]/);
});

test('Cargo.lock: the workspace package, not a registry package of the same name', () => {
  const text = [
    'version = 4',
    '',
    '[[package]]',
    'name = "shodh-rag"',
    'version = "0.0.9"',
    'source = "registry+https://github.com/rust-lang/crates.io-index"',
    '',
    '[[package]]',
    'name = "shodh-rag"',
    'version = "0.0.10"',
    'dependencies = [',
    ' "ort",',
    ']',
    '',
    '[[package]]',
    'name = "shodh-rag-app"',
    'version = "0.0.10"',
    '',
  ].join('\n');
  assert.equal(text.slice(0, locateLockVersion(text, 'shodh-rag').start).split('\n').length, 10);
  assert.equal(locateLockVersion(text, 'shodh-rag-app').value, '0.0.10');
  assert.throws(() => locateLockVersion(text, 'missing'), /found 0/);
});

test('CRLF files keep their line endings', () => {
  const text = '[package]\r\nname = "x"\r\nversion = "1.0.0"\r\n';
  const span = locateManifestVersion(text, 'x');
  assert.equal(text.slice(span.start, span.end), '1.0.0');
});
