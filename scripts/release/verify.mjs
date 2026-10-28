#!/usr/bin/env node
// Prepares and checks what a release ships before it is published, and what
// users get after.
//
//   artifacts  The installers the build jobs uploaded (workflow artifacts):
//              every file present and non-empty, named for the version, and
//              each update bundle's signature valid for the key it was signed
//              with.
//   stage      artifacts, then the release's six files in one folder under
//              their release names, with the latest.json installed apps read
//              (the same shape tauri-action writes), and a local record of
//              them in the form of GitHub's release API (release.json).
//   release    A release's assets (downloaded to --dir) against its API record:
//              exactly the expected files, non-empty and complete; latest.json
//              parses, names this version, points each platform at this
//              release's update bundle, and carries that bundle's .sig; every
//              signature verifies against the updater public key. Writes the
//              SHA-256 table for the release notes.
//   is-newer   Prints true when --tag is newer than --than (or --than is
//              empty): only then may a release be marked the latest.
//   smoke      After publishing: the latest.json installed apps poll serves
//              this version (or, for a release that is not the latest, its own
//              latest.json), and every platform URL in it answers 200 with the
//              asset's size, fetched as the updater does (no credentials,
//              Accept: application/octet-stream).
//
//   node scripts/release/verify.mjs artifacts --dir D --version X.Y.Z [--pubkey-file F]
//   node scripts/release/verify.mjs stage --dir D --out O --tag vX.Y.Z --repo O/R [--pubkey-file F]
//   node scripts/release/verify.mjs release --release-json F --dir D --tag vX.Y.Z --repo O/R [--pubkey-file F] [--checksums-out F]
//   node scripts/release/verify.mjs is-newer --tag vX.Y.Z --than vA.B.C
//   node scripts/release/verify.mjs smoke --tag vX.Y.Z --repo O/R [--not-latest]
// Common: --config app/src-tauri/tauri.conf.json (product name, updater key);
// --pubkey-file replaces the key with one in Tauri's base64 form (pull
// requests sign with a throwaway key, never the release key).

import { createHash } from 'node:crypto';
import { copyFileSync, createReadStream, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';

import { SignatureError, verifyTauriSignature } from './minisign.mjs';
import { compareVersions } from './versions.mjs';

export class ReleaseError extends Error {}

// Updater platforms tauri-action writes for the two builds, and the asset
// each must point at.
export const PLATFORMS = {
  'windows-x86_64': 'windows-updater',
  'windows-x86_64-nsis': 'windows-updater',
  'darwin-aarch64': 'macos-updater',
  'darwin-aarch64-app': 'macos-updater',
};
export const REQUIRED_PLATFORMS = ['windows-x86_64', 'darwin-aarch64'];

/** The release assets for a version, by role. */
export function releaseAssetNames(product, version) {
  const windows = `${product}_${version}_x64-setup.exe`;
  const macos = `${product}_${version}_aarch64.app.tar.gz`;
  return {
    'windows-installer': windows,
    'windows-updater': windows,
    'windows-signature': `${windows}.sig`,
    'macos-dmg': `${product}_${version}_aarch64.dmg`,
    'macos-updater': macos,
    'macos-signature': `${macos}.sig`,
    'latest-json': 'latest.json',
  };
}

export function readConfig(path, pubkeyFile) {
  const config = JSON.parse(readFileSync(path, 'utf8'));
  const product = config.productName;
  let pubkey = config.plugins?.updater?.pubkey || null;
  if (pubkeyFile) pubkey = readFileSync(pubkeyFile, 'utf8').trim();
  if (!product) throw new ReleaseError(`${path} has no productName`);
  return { product, pubkey, version: config.version };
}

export async function sha256File(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}

function listFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...listFiles(path));
    else if (entry.isFile()) out.push(path);
  }
  return out;
}

async function checkSignature(file, signatureText, pubkey, label) {
  try {
    const comment = await verifyTauriSignature(file, signatureText, pubkey);
    console.log(`  signature ok: ${label} (${comment})`);
  } catch (e) {
    if (e instanceof SignatureError) throw new ReleaseError(`${label}: ${e.message}`);
    throw e;
  }
}

/** The release tag's version, or a ReleaseError. */
export function tagVersion(tag) {
  if (!/^v\d+\.\d+\.\d+$/.test(tag ?? '')) throw new ReleaseError(`${tag} is not a release tag (vX.Y.Z)`);
  return tag.slice(1);
}

// --- Build artifacts -------------------------------------------------------------

/**
 * Finds and checks the installers and signed update bundles under dir (the
 * build jobs' artifacts, in any folder layout). Returns their paths.
 */
export async function verifyArtifacts({ dir, version, product, pubkey }) {
  const files = listFiles(dir);
  const errors = [];
  const one = (label, test) => {
    const matches = files.filter((f) => test(f.replace(/\\/g, '/')));
    if (matches.length !== 1) {
      errors.push(`${label}: expected one file, found ${matches.length}${matches.length ? ` (${matches.map((m) => relative(dir, m)).join(', ')})` : ''}`);
      return null;
    }
    return matches[0];
  };
  const names = releaseAssetNames(product, version);
  const windows = one(names['windows-installer'], (f) => f.endsWith(`/${names['windows-installer']}`));
  const dmg = one(names['macos-dmg'], (f) => f.endsWith(`/${names['macos-dmg']}`));
  // tauri bundles the update archive as <product>.app.tar.gz; the release
  // names it with the version and architecture (stage).
  const macos = one(`${product}.app.tar.gz`, (f) => f.endsWith(`/${product}.app.tar.gz`));
  for (const file of [windows, dmg, macos].filter(Boolean)) {
    const size = statSync(file).size;
    if (size === 0) errors.push(`${relative(dir, file)} is empty`);
    else console.log(`  ${String(size).padStart(12)}  ${relative(dir, file)}`);
  }
  if (errors.length) throw new ReleaseError(`The build artifacts are incomplete:\n  ${errors.join('\n  ')}`);

  if (!pubkey) throw new ReleaseError('No updater public key, so the update bundles cannot be checked');
  for (const file of [windows, macos]) {
    let signature;
    try {
      signature = readFileSync(`${file}.sig`, 'utf8');
    } catch {
      throw new ReleaseError(`${relative(dir, file)} has no .sig next to it: the update bundle was not signed`);
    }
    await checkSignature(file, signature, pubkey, relative(dir, file));
  }
  return { windows, dmg, macos };
}

/** latest.json as tauri-action writes it, with download URLs of this release. */
export function latestJson({ repo, tag, product, windowsSignature, macosSignature, pubDate }) {
  const version = tagVersion(tag);
  const names = releaseAssetNames(product, version);
  const url = (name) => `https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(name)}`;
  const windows = { signature: windowsSignature.trim(), url: url(names['windows-updater']) };
  const macos = { signature: macosSignature.trim(), url: url(names['macos-updater']) };
  return {
    version,
    notes: '',
    pub_date: pubDate,
    platforms: {
      'darwin-aarch64': macos,
      'darwin-aarch64-app': { ...macos },
      'windows-x86_64': windows,
      'windows-x86_64-nsis': { ...windows },
    },
  };
}

/**
 * Writes the six release files to `out` under their release names, plus
 * release.json (a local record of them shaped like GitHub's release API).
 */
export async function stageRelease({ dir, out, tag, repo, product, pubkey, pubDate = new Date().toISOString() }) {
  const version = tagVersion(tag);
  const found = await verifyArtifacts({ dir, version, product, pubkey });
  const names = releaseAssetNames(product, version);
  mkdirSync(out, { recursive: true });
  const copies = [
    [found.windows, names['windows-installer']],
    [`${found.windows}.sig`, names['windows-signature']],
    [found.dmg, names['macos-dmg']],
    [found.macos, names['macos-updater']],
    [`${found.macos}.sig`, names['macos-signature']],
  ];
  for (const [from, name] of copies) copyFileSync(from, join(out, name));
  const latest = latestJson({
    repo,
    tag,
    product,
    windowsSignature: readFileSync(`${found.windows}.sig`, 'utf8'),
    macosSignature: readFileSync(`${found.macos}.sig`, 'utf8'),
    pubDate,
  });
  writeFileSync(join(out, 'latest.json'), `${JSON.stringify(latest, null, 2)}\n`);
  const assets = [...copies.map(([, name]) => name), 'latest.json'].map((name, i) => ({
    id: i + 1,
    name,
    size: statSync(join(out, name)).size,
    state: 'uploaded',
  }));
  const record = { tag_name: tag, draft: true, assets };
  writeFileSync(join(out, 'release.json'), `${JSON.stringify(record, null, 2)}\n`);
  return { record, latest, files: assets.map((a) => a.name) };
}

// --- A release's assets -------------------------------------------------------

function assetForUrl(url, release, repo, tag) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  const r = repo.toLowerCase();
  const api = /^\/repos\/([^/]+\/[^/]+)\/releases\/assets\/(\d+)$/.exec(parsed.pathname);
  if (parsed.protocol === 'https:' && parsed.host === 'api.github.com' && api && api[1].toLowerCase() === r && !parsed.search) {
    return release.assets.find((a) => String(a.id) === api[2]) ?? null;
  }
  const web = /^\/([^/]+\/[^/]+)\/releases\/download\/([^/]+)\/([^/]+)$/.exec(parsed.pathname);
  if (parsed.protocol === 'https:' && parsed.host === 'github.com' && web && web[1].toLowerCase() === r && !parsed.search) {
    if (decodeURIComponent(web[2]) !== tag) return null;
    return release.assets.find((a) => a.name === decodeURIComponent(web[3])) ?? null;
  }
  return null;
}

/**
 * Verifies a release (its API JSON and its assets downloaded into dir, under
 * their asset names). Returns [{ name, size, sha256 }] for the release notes.
 */
export async function verifyRelease({ release, dir, tag, repo, product, pubkey }) {
  const version = tagVersion(tag);
  if (release.tag_name !== tag) throw new ReleaseError(`The release is for ${release.tag_name}, not ${tag}`);
  if (!pubkey) throw new ReleaseError('No updater public key: installed apps could not verify this release');
  const names = releaseAssetNames(product, version);
  const expected = new Set(Object.values(names));
  const present = new Map(release.assets.map((a) => [a.name, a]));
  const errors = [];
  if (present.size !== release.assets.length) errors.push('two assets have the same name');
  for (const name of expected) if (!present.has(name)) errors.push(`missing ${name}`);
  for (const name of present.keys()) if (!expected.has(name)) errors.push(`unexpected asset ${name}`);
  for (const asset of release.assets) {
    if (asset.state !== undefined && asset.state !== 'uploaded') errors.push(`${asset.name} is ${asset.state}, not uploaded`);
    if (!(asset.size > 0)) errors.push(`${asset.name} is empty`);
  }
  if (errors.length) throw new ReleaseError(`The release assets of ${tag} are wrong:\n  ${errors.join('\n  ')}`);

  const rows = [];
  for (const asset of release.assets) {
    const path = join(dir, asset.name);
    let size;
    try {
      size = statSync(path).size;
    } catch {
      throw new ReleaseError(`${asset.name} was not downloaded to ${dir}`);
    }
    if (size !== asset.size) throw new ReleaseError(`${asset.name}: downloaded ${size} bytes, the release lists ${asset.size}`);
    rows.push({ name: asset.name, size, sha256: await sha256File(path) });
  }

  for (const role of ['windows', 'macos']) {
    const file = names[`${role}-updater`];
    await checkSignature(join(dir, file), readFileSync(join(dir, `${file}.sig`), 'utf8'), pubkey, file);
  }

  let latest;
  try {
    latest = JSON.parse(readFileSync(join(dir, 'latest.json'), 'utf8'));
  } catch (e) {
    throw new ReleaseError(`latest.json does not parse: ${e.message}`);
  }
  verifyLatestJsonShape(latest, version);
  for (const [platform, entry] of Object.entries(latest.platforms)) {
    const asset = assetForUrl(entry.url, release, repo, tag);
    const wanted = names[PLATFORMS[platform]];
    if (!asset) throw new ReleaseError(`latest.json ${platform}: ${entry.url} is not an asset of this release`);
    if (asset.name !== wanted) throw new ReleaseError(`latest.json ${platform}: points at ${asset.name}, expected ${wanted}`);
    const sig = readFileSync(join(dir, `${wanted}.sig`), 'utf8').trim();
    if (entry.signature.trim() !== sig) throw new ReleaseError(`latest.json ${platform}: the signature differs from ${wanted}.sig`);
    await checkSignature(join(dir, wanted), entry.signature, pubkey, `latest.json ${platform}`);
  }
  console.log(`${tag}: ${rows.length} assets, latest.json and every signature verified`);
  return rows;
}

export function verifyLatestJsonShape(latest, version) {
  if (typeof latest !== 'object' || latest === null) throw new ReleaseError('latest.json is not an object');
  if (latest.version !== version) throw new ReleaseError(`latest.json is for version ${JSON.stringify(latest.version)}, not ${version}`);
  if (latest.pub_date !== undefined && Number.isNaN(Date.parse(latest.pub_date))) {
    throw new ReleaseError(`latest.json pub_date ${JSON.stringify(latest.pub_date)} is not a date`);
  }
  const platforms = latest.platforms;
  if (typeof platforms !== 'object' || platforms === null) throw new ReleaseError('latest.json has no platforms');
  for (const p of REQUIRED_PLATFORMS) if (!(p in platforms)) throw new ReleaseError(`latest.json has no ${p} entry`);
  for (const [platform, entry] of Object.entries(platforms)) {
    if (!(platform in PLATFORMS)) throw new ReleaseError(`latest.json has an unexpected platform ${platform}`);
    if (typeof entry?.url !== 'string' || typeof entry?.signature !== 'string' || !entry.signature.trim()) {
      throw new ReleaseError(`latest.json ${platform} needs a url and a signature`);
    }
  }
}

export function checksumsMarkdown(rows) {
  const lines = ['## SHA-256', '', '| File | SHA-256 |', '| --- | --- |'];
  for (const { name, sha256 } of [...rows].sort((a, b) => a.name.localeCompare(b.name))) {
    lines.push(`| \`${name}\` | \`${sha256}\` |`);
  }
  return `${lines.join('\n')}\n`;
}

/**
 * Whether a release of `tag` may become the latest: only when it is newer
 * than the current latest release (`latest`, empty when there is none). An
 * older release finishing last must not take "latest" from a newer one: the
 * updater would offer the older version.
 */
export function isNewer(tag, latest) {
  const version = tagVersion(tag);
  if (!latest) return true;
  return compareVersions(version, tagVersion(latest)) > 0;
}

// --- After publishing -----------------------------------------------------------

/**
 * Fetches what installed apps fetch. `fetchImpl` and `sleep` are injectable
 * for tests. GitHub serves the new latest.json a few seconds after the
 * release is published, so a stale answer is re-read for up to
 * `attempts` x `delayMs`; a wrong answer after that fails.
 * With `expectLatest` false (a release published without becoming the
 * latest, see isNewer), the release's own latest.json is checked instead and
 * the latest release must be another, newer one.
 */
export async function smoke({
  repo,
  tag,
  expectLatest = true,
  fetchImpl = fetch,
  sleep = (ms) => new Promise((r) => setTimeout(r, ms)),
  attempts = 12,
  delayMs = 10_000,
  token,
}) {
  const version = tagVersion(tag);
  const api = (path) =>
    fetchImpl(`https://api.github.com/repos/${repo}${path}`, {
      headers: { Accept: 'application/vnd.github+json', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    });
  const endpoint = expectLatest
    ? `https://github.com/${repo}/releases/latest/download/latest.json`
    : `https://github.com/${repo}/releases/download/${tag}/latest.json`;
  let latest;
  let release;
  for (let attempt = 1; ; attempt++) {
    const problems = [];
    const latestResponse = await api('/releases/latest');
    const latestRelease = latestResponse.ok ? await latestResponse.json() : null;
    if (expectLatest) {
      release = latestRelease;
      if (!release) problems.push(`releases/latest answered ${latestResponse.status}`);
      else if (release.tag_name !== tag) problems.push(`the latest release is ${release.tag_name}, not ${tag}`);
    } else {
      const own = await api(`/releases/tags/${tag}`);
      release = own.ok ? await own.json() : null;
      if (!release) problems.push(`releases/tags/${tag} answered ${own.status}`);
      if (latestRelease && !isNewer(latestRelease.tag_name, tag)) {
        problems.push(`the latest release is ${latestRelease.tag_name}, which is not newer than ${tag}`);
      }
    }
    const response = await fetchImpl(endpoint, { redirect: 'follow' });
    if (!response.ok) problems.push(`${endpoint} answered ${response.status}`);
    else {
      const text = await response.text();
      try {
        latest = JSON.parse(text);
        if (latest.version !== version) problems.push(`${endpoint} is version ${JSON.stringify(latest.version)}, not ${version}`);
      } catch {
        problems.push(`${endpoint} is not JSON`);
      }
    }
    if (!problems.length) break;
    if (attempt >= attempts) throw new ReleaseError(`After publishing ${tag}: ${problems.join('; ')}`);
    console.log(`  not yet (${problems.join('; ')}); reading again in ${delayMs / 1000} s`);
    await sleep(delayMs);
  }
  verifyLatestJsonShape(latest, version);
  for (const [platform, entry] of Object.entries(latest.platforms)) {
    const asset = assetForUrl(entry.url, release, repo, tag);
    if (!asset) throw new ReleaseError(`${platform}: ${entry.url} is not an asset of ${tag}`);
    // As tauri-plugin-updater downloads it: no credentials, octet-stream.
    const response = await fetchImpl(entry.url, {
      method: 'HEAD',
      redirect: 'follow',
      headers: { Accept: 'application/octet-stream' },
    });
    const length = Number(response.headers.get('content-length'));
    if (response.status !== 200) throw new ReleaseError(`${platform}: HEAD ${entry.url} answered ${response.status}`);
    if (length !== asset.size) throw new ReleaseError(`${platform}: ${entry.url} serves ${length} bytes, ${asset.name} has ${asset.size}`);
    console.log(`  ${platform}: ${asset.name}, 200, ${length} bytes`);
  }
  console.log(`${endpoint} serves ${tag} and every update bundle in it downloads`);
}

// --- CLI ------------------------------------------------------------------------

async function main(argv) {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: {
      config: { type: 'string', default: 'app/src-tauri/tauri.conf.json' },
      'pubkey-file': { type: 'string' },
      dir: { type: 'string' },
      out: { type: 'string' },
      version: { type: 'string' },
      tag: { type: 'string' },
      than: { type: 'string', default: '' },
      repo: { type: 'string' },
      'release-json': { type: 'string' },
      'checksums-out': { type: 'string' },
      'not-latest': { type: 'boolean', default: false },
    },
  });
  const need = (name) => {
    if (!values[name]) throw new ReleaseError(`--${name} is required`);
    return values[name];
  };
  const [command] = positionals;
  if (command === 'is-newer') {
    console.log(isNewer(need('tag'), values.than) ? 'true' : 'false');
    return;
  }
  if (command === 'smoke') {
    await smoke({
      repo: need('repo'),
      tag: need('tag'),
      expectLatest: !values['not-latest'],
      token: process.env.GH_TOKEN || process.env.GITHUB_TOKEN,
    });
    return;
  }
  const { product, pubkey } = readConfig(values.config, values['pubkey-file']);
  if (command === 'artifacts') {
    await verifyArtifacts({ dir: need('dir'), version: need('version'), product, pubkey });
    console.log('The build artifacts are complete and signed');
    return;
  }
  if (command === 'stage') {
    const { files } = await stageRelease({ dir: need('dir'), out: need('out'), tag: need('tag'), repo: need('repo'), product, pubkey });
    console.log(`Staged ${files.join(', ')} in ${values.out}`);
    return;
  }
  if (command === 'release') {
    const release = JSON.parse(readFileSync(need('release-json'), 'utf8'));
    const rows = await verifyRelease({ release, dir: need('dir'), tag: need('tag'), repo: need('repo'), product, pubkey });
    const markdown = checksumsMarkdown(rows);
    console.log(markdown);
    if (values['checksums-out']) writeFileSync(values['checksums-out'], markdown);
    return;
  }
  throw new ReleaseError('Usage: verify.mjs artifacts|stage|release|is-newer|smoke ...');
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  main(process.argv.slice(2)).catch((e) => {
    if (!(e instanceof ReleaseError)) throw e;
    console.error(`::error::${e.message.replace(/\n/g, '%0A')}`);
    process.exit(1);
  });
}
