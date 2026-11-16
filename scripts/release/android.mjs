#!/usr/bin/env node
// The Android app a release ships (Shodh_<version>_android.apk), checked
// after it is signed and before it is uploaded: what the phone installs and
// what Android decides from it.
//
//   version-code X.Y.Z  The versionCode Tauri derives from the version
//                       (major * 1000000 + minor * 1000 + patch), which must
//                       grow with every release: Android refuses to update an
//                       app to a lower one.
//   check               The signed APK, read with the SDK's own tools
//                       (apksigner, aapt2): one signer, APK Signature Scheme
//                       v2 and v3, the expected certificate; the release
//                       package (never the .dev one), versionName equal to the
//                       release's version, versionCode derived from it, not
//                       debuggable, native code for exactly the release ABIs,
//                       and no larger than the phone's one-tap update accepts.
//   debug-markers       The screenshot tour and the scripted computer of the
//                       screenshots (debug builds only) in a built frontend
//                       (--dist) or the app's native library (--lib): present
//                       in a debug build, absent from a release build.
//
//   node scripts/release/android.mjs version-code X.Y.Z
//   node scripts/release/android.mjs check --apk F --version X.Y.Z --build-tools DIR (--release-cert | --cert-sha256 HEX)
//   node scripts/release/android.mjs debug-markers --build debug|release [--dist DIR] [--lib FILE]

import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';

export class AndroidError extends Error {}

// Its own parse, not versions.mjs's: ci.yml's android job runs this script,
// and a change to the desktop's version script must not need that job.
function parseVersion(version) {
  const m = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec(version ?? '');
  if (!m) throw new AndroidError(`${JSON.stringify(version)} is not a version of the form X.Y.Z`);
  return m.slice(1).map(Number);
}

/** The real app's identifier (mobile/src-tauri/tauri.conf.json); test builds add ".dev". */
export const RELEASE_PACKAGE = 'com.shodhrag.mobile';

/** The ABIs a release APK carries: every Android phone Shodh supports is 64-bit ARM. */
export const RELEASE_ABIS = ['arm64-v8a'];

/**
 * The largest app file the phone's one-tap update downloads
 * (crates/shodh-link/src/update.rs MAX_APK_BYTES; a test keeps them equal).
 */
export const MAX_APK_BYTES = 300 * 1024 * 1024;

/**
 * The certificate of Shodh's Android release key (the `release`
 * environment's ANDROID_KEYSTORE_BASE64, alias shodh): Android updates an
 * installed Shodh only with an app signed by this same certificate.
 */
export const RELEASE_CERT_SHA256 = normalizeFingerprint(
  '6A:67:45:33:E6:C7:96:B4:C5:92:5E:E3:5C:F2:58:DF:07:E6:BD:A3:24:94:11:10:65:D0:0A:16:B6:06:17:79',
);

/**
 * What only a debug build's frontend has: the scripted computer of the
 * screenshots (src/lib/demo.ts) and the screenshot tour's listener
 * (src/lib/tour.ts), left out of a release build by vite.config.ts.
 */
export const FRONTEND_DEBUG_MARKERS = ['The sample computer has no', 'shodh://tour', 'shodh-tour-noblur'];

/**
 * What only a debug build's native library has: the tour's system properties
 * and event (src-tauri/src/tour.rs) and the first-screen property
 * (src-tauri/src/link.rs), compiled only with debug assertions.
 */
export const NATIVE_DEBUG_MARKERS = ['debug.shodh.tour', 'debug.shodh.noblur', 'debug.shodh.screen', 'shodh://tour'];

/**
 * A string every build of the native library has (shodh-link's release key
 * check, used by the one-tap update), so an absence check reads the right file.
 */
export const NATIVE_CONTROL = 'The release key is not readable.';

/** Tauri's versionCode for a version (tauri-utils AndroidConfig::version_code). */
export function versionCode(version) {
  const [major, minor, patch] = parseVersion(version);
  // Past 999 a minor or patch would collide with the next field: 0.1.0 and
  // 0.0.1000 would share a code, and the later release could not update.
  if (minor > 999 || patch > 999) throw new AndroidError(`${version}: a minor or patch version above 999 has no distinct versionCode`);
  const code = major * 1_000_000 + minor * 1_000 + patch;
  // Google Play's limit, which Tauri enforces too.
  if (code < 1 || code > 2_100_000_000) throw new AndroidError(`${version}: versionCode ${code} is outside 1..2100000000`);
  return code;
}

/** A certificate digest as lowercase hex without separators. */
export function normalizeFingerprint(text) {
  const hex = String(text ?? '').trim().replace(/[:\s]/g, '').toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(hex)) throw new AndroidError(`${JSON.stringify(text)} is not a SHA-256 digest`);
  return hex;
}

/** `aapt2 dump badging`: the package, its version, debuggability and native code. */
export function parseBadging(text) {
  const lines = text.replace(/\r/g, '').split('\n');
  const pkg = lines.find((l) => l.startsWith('package: '));
  if (!pkg) throw new AndroidError('aapt2 dump badging printed no package line');
  const attr = (name) => new RegExp(`\\b${name}='([^']*)'`).exec(pkg)?.[1] ?? null;
  const native = lines.find((l) => l.startsWith('native-code: '));
  return {
    package: attr('name'),
    versionCode: attr('versionCode'),
    versionName: attr('versionName'),
    debuggable: lines.some((l) => l.trim() === 'application-debuggable'),
    nativeCode: native ? [...native.matchAll(/'([^']+)'/g)].map((m) => m[1]) : [],
  };
}

/** `apksigner verify --verbose --print-certs`: the schemes that verified and every signer certificate. */
export function parseSigners(text) {
  const lines = text.replace(/\r/g, '').split('\n');
  const scheme = (n) => {
    const line = lines.find((l) => l.startsWith(`Verified using ${n} scheme `));
    if (!line) return null;
    return /: true$/.test(line);
  };
  const count = /^Number of signers: (\d+)$/m.exec(lines.join('\n'));
  return {
    verifies: lines.some((l) => l.trim() === 'Verifies'),
    v1: scheme('v1'),
    v2: scheme('v2'),
    v3: scheme('v3'),
    signers: count ? Number(count[1]) : null,
    certificates: lines
      .map((l) => /certificate SHA-256 digest: ([0-9a-fA-F:]+)\s*$/.exec(l)?.[1])
      .filter(Boolean)
      .map(normalizeFingerprint),
  };
}

/** Every problem with a signed release APK; empty when it may ship. */
export function releaseProblems({ badging, signers, version, certSha256, size, maxBytes = MAX_APK_BYTES }) {
  const problems = [];
  const expectedCert = normalizeFingerprint(certSha256);
  if (!signers.verifies) problems.push('apksigner does not verify it');
  if (signers.v2 !== true) problems.push('it is not signed with APK Signature Scheme v2');
  if (signers.v3 !== true) problems.push('it is not signed with APK Signature Scheme v3');
  if (signers.signers !== 1) problems.push(`it has ${signers.signers ?? 'no'} signers, not one`);
  if (!signers.certificates.length) problems.push('apksigner printed no signer certificate');
  for (const cert of new Set(signers.certificates)) {
    if (cert !== expectedCert) problems.push(`it is signed by certificate ${cert}, not ${expectedCert}`);
  }
  if (badging.package !== RELEASE_PACKAGE) problems.push(`its package is ${badging.package}, not ${RELEASE_PACKAGE}`);
  if (badging.versionName !== version) problems.push(`its versionName is ${badging.versionName}, not ${version}`);
  const code = String(versionCode(version));
  if (badging.versionCode !== code) problems.push(`its versionCode is ${badging.versionCode}, not ${code} (from ${version})`);
  if (badging.debuggable) problems.push('it is debuggable');
  const abis = [...badging.nativeCode].sort().join(' ');
  if (abis !== [...RELEASE_ABIS].sort().join(' ')) problems.push(`its native code is for "${abis}", not "${RELEASE_ABIS.join(' ')}"`);
  if (!(size > 0)) problems.push('it is empty');
  else if (size > maxBytes) problems.push(`it is ${size} bytes, more than the ${maxBytes} the phone's one-tap update downloads`);
  return problems;
}

function textFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...textFiles(path));
    else if (entry.isFile()) out.push(path);
  }
  return out;
}

/** The markers found in `data` (a Buffer), in the order given. */
export function markersIn(data, markers) {
  return markers.filter((m) => data.includes(Buffer.from(m)));
}

/**
 * Problems with the debug-only code of a build: a debug build must have every
 * marker (so the release check is not vacuous), a release build none.
 * `dist` is a built frontend folder, `lib` the native library's bytes.
 */
export function debugMarkerProblems({ build, dist, lib }) {
  if (build !== 'debug' && build !== 'release') throw new AndroidError(`--build is debug or release, not ${build}`);
  if (!dist && !lib) throw new AndroidError('Give --dist, --lib or both');
  const problems = [];
  const judge = (what, found, markers) => {
    if (build === 'debug') {
      for (const m of markers) if (!found.includes(m)) problems.push(`the debug ${what} lacks "${m}"`);
    } else {
      for (const m of found) problems.push(`the release ${what} contains "${m}"`);
    }
  };
  if (dist) {
    const files = textFiles(dist);
    if (!files.some((f) => f.endsWith('index.html'))) problems.push(`${dist} has no index.html: it is not a built frontend`);
    const found = new Set();
    for (const f of files) for (const m of markersIn(readFileSync(f), FRONTEND_DEBUG_MARKERS)) found.add(m);
    judge('frontend', [...found], FRONTEND_DEBUG_MARKERS);
  }
  if (lib) {
    const data = Buffer.isBuffer(lib) ? lib : readFileSync(lib);
    if (!markersIn(data, [NATIVE_CONTROL]).length) problems.push(`the native library lacks "${NATIVE_CONTROL}": it is not Shodh's`);
    judge('native library', markersIn(data, NATIVE_DEBUG_MARKERS), NATIVE_DEBUG_MARKERS);
  }
  return problems;
}

// --- CLI ------------------------------------------------------------------------

function tool(buildTools, name, args) {
  try {
    return execFileSync(join(buildTools, name), args, { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });
  } catch (e) {
    throw new AndroidError(`${name} ${args.join(' ')} failed (${e.status ?? e.code}): ${String(e.stderr ?? '').trim() || e.message}`);
  }
}

function main(argv) {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: {
      apk: { type: 'string' },
      version: { type: 'string' },
      'build-tools': { type: 'string' },
      'cert-sha256': { type: 'string' },
      'release-cert': { type: 'boolean', default: false },
      build: { type: 'string' },
      dist: { type: 'string' },
      lib: { type: 'string' },
    },
  });
  const need = (name) => {
    if (!values[name]) throw new AndroidError(`--${name} is required`);
    return values[name];
  };
  const [command, arg] = positionals;
  if (command === 'version-code') {
    if (!arg) throw new AndroidError('Usage: android.mjs version-code X.Y.Z');
    console.log(versionCode(arg));
    return;
  }
  if (command === 'check') {
    if (values['release-cert'] === Boolean(values['cert-sha256'])) throw new AndroidError('Give exactly one of --release-cert and --cert-sha256');
    const certSha256 = values['release-cert'] ? RELEASE_CERT_SHA256 : normalizeFingerprint(values['cert-sha256']);
    const apk = need('apk');
    const version = need('version');
    const buildTools = need('build-tools');
    const badgingText = tool(buildTools, 'aapt2', ['dump', 'badging', apk]);
    const signerText = tool(buildTools, 'apksigner', ['verify', '--verbose', '--print-certs', apk]);
    const badging = parseBadging(badgingText);
    const signers = parseSigners(signerText);
    const size = statSync(apk).size;
    console.log(`package ${badging.package}, versionName ${badging.versionName}, versionCode ${badging.versionCode}, debuggable ${badging.debuggable}, native code ${badging.nativeCode.join(' ') || 'none'}`);
    console.log(`signature: verifies ${signers.verifies}, v2 ${signers.v2}, v3 ${signers.v3}, ${signers.signers} signer(s), certificate ${signers.certificates.join(', ') || 'none'}`);
    console.log(`size ${size} bytes`);
    const problems = releaseProblems({ badging, signers, version, certSha256, size });
    if (problems.length) throw new AndroidError(`${apk} is not a releasable Shodh for Android:\n  ${problems.join('\n  ')}`);
    console.log(`${apk}: ${RELEASE_PACKAGE} ${version}, signed by ${certSha256}`);
    return;
  }
  if (command === 'debug-markers') {
    const problems = debugMarkerProblems({ build: need('build'), dist: values.dist, lib: values.lib });
    if (problems.length) throw new AndroidError(problems.join('\n'));
    console.log(`The ${values.build} build's debug-only code is ${values.build === 'debug' ? 'all present' : 'absent'} (${[values.dist, values.lib].filter(Boolean).join(', ')})`);
    return;
  }
  throw new AndroidError('Usage: android.mjs version-code|check|debug-markers ...');
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    if (!(e instanceof AndroidError)) throw e;
    console.error(`::error::${e.message.replace(/\n/g, '%0A')}`);
    process.exit(1);
  }
}
