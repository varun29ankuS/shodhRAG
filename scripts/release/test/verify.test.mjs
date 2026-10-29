import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  ReleaseError,
  checksumsMarkdown,
  isNewer,
  latestJson,
  releaseAssetNames,
  smoke,
  stageRelease,
  verifyArtifacts,
  verifyLatestJsonShape,
  verifyRelease,
} from '../verify.mjs';
import { makeKey, signTauri } from './helpers.mjs';

const REPO = 'owner/app';
const TAG = 'v1.2.3';
const VERSION = '1.2.3';
const PRODUCT = 'Shodh';

/** A complete, valid release in a temp folder, and its API record. */
function makeRelease({ key = makeKey(), urlStyle = 'api' } = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'release-test-'));
  const names = releaseAssetNames(PRODUCT, VERSION);
  const contents = {
    [names['windows-installer']]: Buffer.from('windows installer'),
    [names['macos-dmg']]: Buffer.from('disk image'),
    [names['macos-updater']]: Buffer.from('app archive'),
  };
  contents[names['windows-signature']] = Buffer.from(signTauri(key, contents[names['windows-installer']]));
  contents[names['macos-signature']] = Buffer.from(signTauri(key, contents[names['macos-updater']]));
  let id = 100;
  const assets = Object.entries(contents).map(([name, data]) => ({ id: id++, name, size: data.length, state: 'uploaded' }));
  const url = (name) => {
    const asset = assets.find((a) => a.name === name);
    return urlStyle === 'api'
      ? `https://api.github.com/repos/${REPO}/releases/assets/${asset.id}`
      : `https://github.com/${REPO}/releases/download/${TAG}/${name}`;
  };
  const entry = (name) => ({ signature: contents[`${name}.sig`].toString(), url: url(name) });
  const latest = {
    version: VERSION,
    notes: '',
    pub_date: '2026-10-27T06:29:59.377Z',
    platforms: {
      'darwin-aarch64': entry(names['macos-updater']),
      'darwin-aarch64-app': entry(names['macos-updater']),
      'windows-x86_64': entry(names['windows-installer']),
      'windows-x86_64-nsis': entry(names['windows-installer']),
    },
  };
  contents['latest.json'] = Buffer.from(JSON.stringify(latest, null, 2));
  assets.push({ id: id++, name: 'latest.json', size: contents['latest.json'].length, state: 'uploaded' });
  for (const [name, data] of Object.entries(contents)) writeFileSync(join(dir, name), data);
  const release = { tag_name: TAG, draft: true, assets };
  return { dir, release, key, names, latest, contents };
}

function rewriteLatest(r, change) {
  change(r.latest);
  const data = Buffer.from(JSON.stringify(r.latest));
  writeFileSync(join(r.dir, 'latest.json'), data);
  r.release.assets.find((a) => a.name === 'latest.json').size = data.length;
}

const run = (r, overrides = {}) =>
  verifyRelease({ release: r.release, dir: r.dir, tag: TAG, repo: REPO, product: PRODUCT, pubkey: r.key.tauri, ...overrides });

test('a complete release verifies and lists every asset with its SHA-256', async () => {
  const r = makeRelease();
  const rows = await run(r);
  assert.equal(rows.length, 6);
  const exe = rows.find((row) => row.name === r.names['windows-installer']);
  assert.equal(exe.sha256, createHash('sha256').update('windows installer').digest('hex'));
  const md = checksumsMarkdown(rows);
  assert.match(md, /^## SHA-256\n\n\| File \| SHA-256 \|\n\| --- \| --- \|\n/);
  assert.ok(md.includes(`| \`${r.names['windows-installer']}\` | \`${exe.sha256}\` |`));
});

test('download URLs (github.com/.../releases/download) are accepted too', async () => {
  await run(makeRelease({ urlStyle: 'download' }));
});

test('a missing, unexpected, empty or incomplete asset fails', async () => {
  let r = makeRelease();
  r.release.assets = r.release.assets.filter((a) => a.name !== r.names['macos-dmg']);
  await assert.rejects(run(r), /missing Shodh_1\.2\.3_aarch64\.dmg/);

  r = makeRelease();
  r.release.assets.push({ id: 999, name: 'Shodh_1.2.3_x64_en-US.msi', size: 5, state: 'uploaded' });
  await assert.rejects(run(r), /unexpected asset Shodh_1\.2\.3_x64_en-US\.msi/);

  r = makeRelease();
  r.release.assets.find((a) => a.name === r.names['macos-dmg']).size = 0;
  await assert.rejects(run(r), /is empty/);

  r = makeRelease();
  r.release.assets.find((a) => a.name === r.names['macos-dmg']).state = 'starter';
  await assert.rejects(run(r), /not uploaded/);

  r = makeRelease();
  writeFileSync(join(r.dir, r.names['macos-dmg']), 'disk');
  await assert.rejects(run(r), /downloaded 4 bytes/);

  r = makeRelease();
  rmSync(join(r.dir, r.names['macos-dmg']));
  await assert.rejects(run(r), /was not downloaded/);
});

test('the wrong tag, or a release without an updater key, fails', async () => {
  const r = makeRelease();
  await assert.rejects(run(r, { tag: 'v1.2.4' }), /for v1\.2\.3, not v1\.2\.4/);
  await assert.rejects(run(r, { tag: 'main' }), /not a release tag/);
  await assert.rejects(run(r, { pubkey: null }), /No updater public key/);
});

test('a bundle that does not match its signature fails', async () => {
  const r = makeRelease();
  writeFileSync(join(r.dir, r.names['macos-updater']), 'app archivE');
  await assert.rejects(run(r), /Shodh_1\.2\.3_aarch64\.app\.tar\.gz: The signature does not match the file/);
});

test('signatures by another key fail', async () => {
  const r = makeRelease();
  await assert.rejects(run(r, { pubkey: makeKey().tauri }), /Signed with key/);
});

test('latest.json must name this version and only known platforms', async () => {
  let r = makeRelease();
  rewriteLatest(r, (l) => (l.version = '1.2.2'));
  await assert.rejects(run(r), /version "1\.2\.2", not 1\.2\.3/);

  r = makeRelease();
  rewriteLatest(r, (l) => delete l.platforms['windows-x86_64']);
  await assert.rejects(run(r), /no windows-x86_64 entry/);

  r = makeRelease();
  rewriteLatest(r, (l) => (l.platforms['linux-x86_64'] = l.platforms['windows-x86_64']));
  await assert.rejects(run(r), /unexpected platform linux-x86_64/);

  r = makeRelease();
  writeFileSync(join(r.dir, 'latest.json'), '{"version":');
  r.release.assets.find((a) => a.name === 'latest.json').size = 11;
  await assert.rejects(run(r), /latest\.json does not parse/);
});

test('latest.json must point each platform at its own bundle in this release', async () => {
  let r = makeRelease();
  rewriteLatest(r, (l) => (l.platforms['windows-x86_64'].url = l.platforms['windows-x86_64'].url.replace(REPO, 'someone/else')));
  await assert.rejects(run(r), /windows-x86_64: .* is not an asset of this release/);

  r = makeRelease();
  rewriteLatest(r, (l) => (l.platforms['windows-x86_64'].url = 'https://api.github.com/repos/owner/app/releases/assets/424242'));
  await assert.rejects(run(r), /not an asset of this release/);

  r = makeRelease();
  const dmg = r.release.assets.find((a) => a.name === r.names['macos-dmg']);
  rewriteLatest(r, (l) => (l.platforms['darwin-aarch64'].url = `https://api.github.com/repos/${REPO}/releases/assets/${dmg.id}`));
  await assert.rejects(run(r), /points at Shodh_1\.2\.3_aarch64\.dmg, expected Shodh_1\.2\.3_aarch64\.app\.tar\.gz/);

  r = makeRelease({ urlStyle: 'download' });
  rewriteLatest(r, (l) => (l.platforms['darwin-aarch64'].url = l.platforms['darwin-aarch64'].url.replace(TAG, 'v1.2.2')));
  await assert.rejects(run(r), /not an asset of this release/);

  r = makeRelease();
  rewriteLatest(r, (l) => (l.platforms['windows-x86_64'].url += '?x=1'));
  await assert.rejects(run(r), /not an asset of this release/);
});

test("latest.json's signature must be the .sig asset", async () => {
  const r = makeRelease();
  const other = signTauri(r.key, r.contents[r.names['windows-installer']], { trustedComment: 'timestamp:2\tfile:x' });
  rewriteLatest(r, (l) => (l.platforms['windows-x86_64'].signature = other));
  await assert.rejects(run(r), /windows-x86_64: the signature differs from Shodh_1\.2\.3_x64-setup\.exe\.sig/);
});

test("v0.0.10's published latest.json has the shape and asset URLs a release must have", () => {
  const latest = JSON.parse(readFileSync(new URL('./fixtures/v0.0.10-latest.json', import.meta.url), 'utf8'));
  const release = JSON.parse(readFileSync(new URL('./fixtures/v0.0.10-release.json', import.meta.url), 'utf8'));
  verifyLatestJsonShape(latest, '0.0.10');
  const names = new Set(Object.values(releaseAssetNames('Shodh', '0.0.10')));
  assert.deepEqual(new Set(release.assets.map((a) => a.name)), names);
  for (const entry of Object.values(latest.platforms)) {
    const id = /\/releases\/assets\/(\d+)$/.exec(entry.url)[1];
    assert.ok(release.assets.some((a) => String(a.id) === id), entry.url);
  }
});

// --- Pull request artifacts ---

function makeArtifacts({ signed = true } = {}) {
  const key = makeKey();
  const dir = mkdtempSync(join(tmpdir(), 'artifacts-test-'));
  const win = join(dir, 'shodh-windows-x64');
  const mac = join(dir, 'shodh-macos-aarch64');
  mkdirSync(win, { recursive: true });
  mkdirSync(join(mac, 'dmg'), { recursive: true });
  mkdirSync(join(mac, 'macos'), { recursive: true });
  const exe = join(win, `${PRODUCT}_${VERSION}_x64-setup.exe`);
  const tgz = join(mac, 'macos', `${PRODUCT}.app.tar.gz`);
  writeFileSync(exe, 'installer');
  writeFileSync(join(mac, 'dmg', `${PRODUCT}_${VERSION}_aarch64.dmg`), 'dmg');
  writeFileSync(tgz, 'archive');
  if (signed) {
    writeFileSync(`${exe}.sig`, signTauri(key, Buffer.from('installer')));
    writeFileSync(`${tgz}.sig`, signTauri(key, Buffer.from('archive')));
  }
  return { dir, key, exe, tgz };
}

const artifacts = (a, overrides = {}) =>
  verifyArtifacts({ dir: a.dir, version: VERSION, product: PRODUCT, pubkey: a.key.tauri, ...overrides });

test('complete, signed pull request artifacts verify', async () => {
  await artifacts(makeArtifacts());
});

test('artifacts: a missing or empty installer fails', async () => {
  let a = makeArtifacts();
  rmSync(a.tgz);
  await assert.rejects(artifacts(a), /Shodh\.app\.tar\.gz: expected one file, found 0/);
  a = makeArtifacts();
  writeFileSync(a.exe, '');
  await assert.rejects(artifacts(a), /is empty/);
  a = makeArtifacts();
  await assert.rejects(artifacts(a, { version: '1.2.4' }), /Shodh_1\.2\.4_x64-setup\.exe: expected one file, found 0/);
});

test('artifacts: every update bundle must be signed by the given key', async () => {
  const a = makeArtifacts({ signed: false });
  await assert.rejects(artifacts(a), /has no \.sig next to it/);
  const b = makeArtifacts();
  writeFileSync(b.tgz, 'archivE');
  await assert.rejects(artifacts(b), /does not match the file/);
  await assert.rejects(artifacts(makeArtifacts(), { pubkey: makeKey().tauri }), /Signed with key/);
  await assert.rejects(artifacts(makeArtifacts(), { pubkey: null }), /No updater public key/);
});

test('stage: the six release files and a latest.json that verifies as a release', async () => {
  const a = makeArtifacts();
  const out = mkdtempSync(join(tmpdir(), 'stage-test-'));
  const { record, latest, files } = await stageRelease({
    dir: a.dir, out, tag: TAG, repo: REPO, product: PRODUCT, pubkey: a.key.tauri, pubDate: '2026-10-28T12:00:00.000Z',
  });
  const names = releaseAssetNames(PRODUCT, VERSION);
  assert.deepEqual(new Set(files), new Set(Object.values(names)));
  assert.equal(readFileSync(join(out, names['macos-updater']), 'utf8'), 'archive');
  assert.equal(latest.platforms['darwin-aarch64'].url, `https://github.com/${REPO}/releases/download/${TAG}/${names['macos-updater']}`);
  assert.equal(latest.platforms['windows-x86_64'].signature, readFileSync(`${a.exe}.sig`, 'utf8').trim());
  assert.deepEqual(JSON.parse(readFileSync(join(out, 'release.json'), 'utf8')), record);
  // What publishing checks, on the staged files.
  const rows = await verifyRelease({ release: record, dir: out, tag: TAG, repo: REPO, product: PRODUCT, pubkey: a.key.tauri });
  assert.equal(rows.length, 6);
});

test('stage refuses incomplete or unsigned artifacts', async () => {
  const a = makeArtifacts({ signed: false });
  const out = mkdtempSync(join(tmpdir(), 'stage-test-'));
  await assert.rejects(stageRelease({ dir: a.dir, out, tag: TAG, repo: REPO, product: PRODUCT, pubkey: a.key.tauri }), /has no \.sig/);
  await assert.rejects(stageRelease({ dir: makeArtifacts().dir, out, tag: 'latest', repo: REPO, product: PRODUCT, pubkey: a.key.tauri }), /not a release tag/);
});

test("latest.json has the shape of v0.0.10's (written by tauri-action)", () => {
  const published = JSON.parse(readFileSync(new URL('./fixtures/v0.0.10-latest.json', import.meta.url), 'utf8'));
  const ours = latestJson({ repo: REPO, tag: 'v0.0.10', product: 'Shodh', windowsSignature: 'w\n', macosSignature: 'm', pubDate: published.pub_date });
  assert.deepEqual(Object.keys(ours).sort(), Object.keys(published).sort());
  assert.deepEqual(Object.keys(ours.platforms).sort(), Object.keys(published.platforms).sort());
  for (const p of Object.keys(ours.platforms)) assert.deepEqual(Object.keys(ours.platforms[p]).sort(), ['signature', 'url']);
  assert.equal(ours.platforms['windows-x86_64'].signature, 'w');
  verifyLatestJsonShape(ours, '0.0.10');
});

test('only a newer release becomes the latest', () => {
  assert.equal(isNewer('v0.0.11', 'v0.0.10'), true);
  assert.equal(isNewer('v0.1.0', 'v0.0.99'), true);
  assert.equal(isNewer('v0.0.10', 'v0.0.11'), false);
  assert.equal(isNewer('v0.0.10', 'v0.0.10'), false);
  assert.equal(isNewer('v0.0.9', 'v0.0.10'), false);
  assert.equal(isNewer('v0.0.1', ''), true);
  assert.throws(() => isNewer('v0.0.1', 'nightly'), /not a release tag/);
});

// --- Smoke ---

function fakeGitHub({ staleReads = 0, headStatus = 200, headLength } = {}) {
  const r = makeRelease();
  const published = { ...r.release, draft: false };
  let reads = 0;
  const calls = [];
  const fetchImpl = async (url, init = {}) => {
    calls.push({ url, init });
    const json = (body, status = 200) => ({ ok: status < 400, status, json: async () => body, text: async () => JSON.stringify(body), headers: new Map() });
    if (url === `https://api.github.com/repos/${REPO}/releases/latest`) {
      return reads < staleReads ? json({ ...published, tag_name: 'v1.2.2' }) : json(published);
    }
    if (url === `https://github.com/${REPO}/releases/latest/download/latest.json`) {
      reads++;
      return reads <= staleReads ? json({ ...r.latest, version: '1.2.2' }) : json(r.latest);
    }
    const asset = /\/releases\/assets\/(\d+)$/.exec(url);
    if (asset && init.method === 'HEAD') {
      assert.equal(init.headers.Accept, 'application/octet-stream');
      assert.equal(init.headers.Authorization, undefined);
      const size = published.assets.find((a) => String(a.id) === asset[1]).size;
      return { ok: headStatus === 200, status: headStatus, headers: new Map([['content-length', String(headLength ?? size)]]) };
    }
    throw new Error(`unexpected request ${url}`);
  };
  return { fetchImpl, calls };
}

test('smoke: waits for GitHub to serve the new latest.json, then checks every bundle URL', async () => {
  const { fetchImpl, calls } = fakeGitHub({ staleReads: 2 });
  const sleeps = [];
  await smoke({ repo: REPO, tag: TAG, fetchImpl, sleep: async (ms) => sleeps.push(ms), delayMs: 5 });
  assert.equal(sleeps.length, 2);
  assert.equal(calls.filter((c) => c.init.method === 'HEAD').length, 4);
});

test('smoke: a latest.json that never updates fails', async () => {
  const { fetchImpl } = fakeGitHub({ staleReads: 100 });
  await assert.rejects(smoke({ repo: REPO, tag: TAG, fetchImpl, sleep: async () => {}, attempts: 3 }), (e) => e instanceof ReleaseError && /not 1\.2\.3/.test(e.message));
});

test('smoke: a bundle URL that does not download fails', async () => {
  const { fetchImpl } = fakeGitHub({ headStatus: 404 });
  await assert.rejects(smoke({ repo: REPO, tag: TAG, fetchImpl, sleep: async () => {} }), /answered 404/);
  const short = fakeGitHub({ headLength: 3 });
  await assert.rejects(smoke({ repo: REPO, tag: TAG, fetchImpl: short.fetchImpl, sleep: async () => {} }), /serves 3 bytes/);
});

test('smoke: a release that is not the latest checks its own latest.json, and that a newer one is latest', async () => {
  const r = makeRelease();
  const published = { ...r.release, draft: false };
  const newer = { ...published, tag_name: 'v1.3.0' };
  const make = (latestRelease) => async (url, init = {}) => {
    const json = (body) => ({ ok: true, status: 200, json: async () => body, text: async () => JSON.stringify(body), headers: new Map() });
    if (url === `https://api.github.com/repos/${REPO}/releases/latest`) return json(latestRelease);
    if (url === `https://api.github.com/repos/${REPO}/releases/tags/${TAG}`) return json(published);
    if (url === `https://github.com/${REPO}/releases/download/${TAG}/latest.json`) return json(r.latest);
    const asset = /\/releases\/assets\/(\d+)$/.exec(url);
    if (asset && init.method === 'HEAD') {
      const size = published.assets.find((a) => String(a.id) === asset[1]).size;
      return { ok: true, status: 200, headers: new Map([['content-length', String(size)]]) };
    }
    throw new Error(`unexpected request ${url}`);
  };
  await smoke({ repo: REPO, tag: TAG, expectLatest: false, fetchImpl: make(newer), sleep: async () => {} });
  // If this older release were the latest, the newer one lost "latest".
  await assert.rejects(
    smoke({ repo: REPO, tag: TAG, expectLatest: false, fetchImpl: make(published), sleep: async () => {}, attempts: 2 }),
    /which is not newer than v1\.2\.3/,
  );
});
