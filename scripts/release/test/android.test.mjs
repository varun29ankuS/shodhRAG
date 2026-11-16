import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  AndroidError,
  FRONTEND_DEBUG_MARKERS,
  MAX_APK_BYTES,
  NATIVE_CONTROL,
  NATIVE_DEBUG_MARKERS,
  RELEASE_ABIS,
  RELEASE_CERT_SHA256,
  RELEASE_PACKAGE,
  debugMarkerProblems,
  normalizeFingerprint,
  parseBadging,
  parseSigners,
  releaseProblems,
  versionCode,
} from '../android.mjs';
import { LOCATIONS, compareVersions } from '../versions.mjs';
import { releaseAssetNames } from '../verify.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const read = (path) => readFileSync(join(ROOT, path), 'utf8');
const script = join(ROOT, 'scripts', 'release', 'android.mjs');

const CERT = '6a674533e6c796b4c5925ee35cf258df07e6bda32494111065d00a16b6061779';
const OTHER_CERT = 'ab'.repeat(32);

// What aapt2 (build-tools 35.0.0) prints for a release build, cut to the lines read.
const badging = ({ name = RELEASE_PACKAGE, code = '12', version = '0.0.12', debuggable = false, abis = ["'arm64-v8a'"] } = {}) =>
  [
    `package: name='${name}' versionCode='${code}' versionName='${version}' platformBuildVersionName='16' platformBuildVersionCode='36' compileSdkVersion='36' compileSdkVersionCodename='16'`,
    "minSdkVersion:'24'",
    "targetSdkVersion:'36'",
    "uses-permission: name='android.permission.INTERNET'",
    "application-label:'Shodh'",
    "application: label='Shodh' icon='res/mipmap-anydpi-v26/ic_launcher.xml'",
    ...(debuggable ? ['application-debuggable'] : []),
    "launchable-activity: name='com.shodhrag.mobile.MainActivity'  label='' icon=''",
    "supports-screens: 'small' 'normal' 'large' 'xlarge'",
    ...(abis.length ? [`native-code: ${abis.join(' ')}`] : []),
    '',
  ].join('\n');

// What apksigner verify --verbose --print-certs (build-tools 35.0.0) prints.
const signerText = ({ v2 = true, v3 = true, certs = [CERT], verifies = true } = {}) =>
  [
    ...(verifies ? ['Verifies'] : []),
    'Verified using v1 scheme (JAR signing): false',
    `Verified using v2 scheme (APK Signature Scheme v2): ${v2}`,
    `Verified using v3 scheme (APK Signature Scheme v3): ${v3}`,
    'Verified using v3.1 scheme (APK Signature Scheme v3.1): false',
    'Verified using v4 scheme (APK Signature Scheme v4): false',
    'Verified for SourceStamp: false',
    `Number of signers: ${certs.length}`,
    ...certs.flatMap((c, i) => [
      `Signer #${i + 1} certificate DN: CN=Shodh`,
      `Signer #${i + 1} certificate SHA-256 digest: ${c}`,
      `Signer #${i + 1} certificate SHA-1 digest: ${'cd'.repeat(20)}`,
      `Signer #${i + 1} key algorithm: RSA`,
      `Signer #${i + 1} key size (bits): 4096`,
      `Signer #${i + 1} public key SHA-256 digest: ${'ef'.repeat(32)}`,
    ]),
    '',
  ].join('\r\n');

const problems = (overrides = {}) =>
  releaseProblems({
    badging: parseBadging(badging(overrides.badging)),
    signers: parseSigners(signerText(overrides.signers)),
    version: '0.0.12',
    certSha256: CERT,
    size: 40 * 1024 * 1024,
    ...overrides.args,
  });

// --- versionCode -------------------------------------------------------------

test("versionCode is Tauri's major * 1000000 + minor * 1000 + patch", () => {
  assert.equal(versionCode('0.0.11'), 11);
  assert.equal(versionCode('0.0.12'), 12);
  assert.equal(versionCode('0.1.0'), 1000);
  assert.equal(versionCode('1.2.3'), 1_002_003);
  assert.equal(versionCode('2100.0.0'), 2_100_000_000);
  const out = execFileSync(process.execPath, [script, 'version-code', '0.2.7'], { encoding: 'utf8' });
  assert.equal(out.trim(), '2007');
});

test('versionCode grows with every release, so Android always takes the next one as an update', () => {
  const versions = ['0.0.9', '0.0.10', '0.0.11', '0.0.99', '0.0.999', '0.1.0', '0.1.1', '0.9.999', '0.10.0', '1.0.0', '1.0.1', '2.0.0', '10.5.3'];
  for (let i = 0; i < versions.length; i++) {
    for (let j = 0; j < versions.length; j++) {
      const [a, b] = [versions[i], versions[j]];
      assert.equal(Math.sign(versionCode(a) - versionCode(b)), compareVersions(a, b), `${a} vs ${b}`);
    }
  }
});

test('a version without a distinct versionCode is refused, not wrapped', () => {
  assert.throws(() => versionCode('0.0.1000'), AndroidError);
  assert.throws(() => versionCode('0.1000.0'), AndroidError);
  assert.throws(() => versionCode('2101.0.0'), /outside 1\.\.2100000000/);
  assert.throws(() => versionCode('0.0.0'), /outside/);
  assert.throws(() => versionCode('1.2'), /not a version/);
  assert.throws(() => versionCode('v1.2.3'), /not a version/);
});

test("the phone app's version is the one versions.mjs sets, and Tauri derives versionCode from it", () => {
  const conf = JSON.parse(read('mobile/src-tauri/tauri.conf.json'));
  // A path to mobile/package.json (resolved from src-tauri/): without a
  // version Tauri's Android build is version 1.0, whatever Cargo.toml says.
  assert.equal(conf.version, '../package.json');
  assert.ok(LOCATIONS.some((l) => l.file === 'mobile/package.json' && l.path?.join('.') === 'version'), 'versions.mjs sets mobile/package.json');
  // No override: versionCode is derived (Tauri) and checked (android.mjs check).
  assert.equal(conf.bundle?.android?.versionCode, undefined);
  assert.notEqual(conf.bundle?.android?.autoIncrementVersionCode, true);
  const version = JSON.parse(read('mobile/package.json')).version;
  assert.equal(versionCode(version), versionCode(JSON.parse(read('app/package.json')).version));
});

// --- Identity, name and size: the phone's contracts ---------------------------

test('the release package is the real app; test builds are .dev', () => {
  assert.equal(JSON.parse(read('mobile/src-tauri/tauri.conf.json')).identifier, RELEASE_PACKAGE);
  assert.equal(JSON.parse(read('mobile/src-tauri/tauri.dev.conf.json')).identifier, `${RELEASE_PACKAGE}.dev`);
});

test("the release's APK name and size limit are the phone's one-tap update's (shodh-link update.rs)", () => {
  const rust = read('crates/shodh-link/src/update.rs');
  const name = /pub fn apk_name\(version: Version\) -> String \{\s*format!\("([^"]+)"\)/.exec(rust);
  assert.ok(name, 'update.rs has apk_name');
  // verify.mjs names the release's files after the desktop's productName.
  const product = JSON.parse(read('app/src-tauri/tauri.conf.json')).productName;
  const names = releaseAssetNames(product, '0.0.12');
  assert.equal(name[1].replace('{version}', '0.0.12'), names['android-apk']);
  assert.match(rust, /let signature = format!\("\{name\}\.sig"\);/);
  assert.equal(names['android-signature'], `${name[1].replace('{version}', '0.0.12')}.sig`);
  const max = /pub const MAX_APK_BYTES: u64 = ([\d\s*]+);/.exec(rust);
  assert.ok(max, 'update.rs has MAX_APK_BYTES');
  assert.equal(max[1].split('*').reduce((p, n) => p * Number(n.trim()), 1), MAX_APK_BYTES);
});

test("the release certificate is the release key's, as a lowercase digest", () => {
  assert.equal(RELEASE_CERT_SHA256, CERT);
  assert.equal(normalizeFingerprint('6A:67:45:33:E6:C7:96:B4:C5:92:5E:E3:5C:F2:58:DF:07:E6:BD:A3:24:94:11:10:65:D0:0A:16:B6:06:17:79'), CERT);
  assert.equal(normalizeFingerprint(` ${CERT.toUpperCase()}\n`), CERT);
  assert.throws(() => normalizeFingerprint('6A:67'), AndroidError);
  assert.throws(() => normalizeFingerprint(''), AndroidError);
  assert.throws(() => normalizeFingerprint(`${CERT}00`), AndroidError);
});

// --- The tools' output ------------------------------------------------------------

test('aapt2 badging: package, version, debuggable and native code', () => {
  assert.deepEqual(parseBadging(badging()), {
    package: RELEASE_PACKAGE,
    versionCode: '12',
    versionName: '0.0.12',
    debuggable: false,
    nativeCode: ['arm64-v8a'],
  });
  const debug = parseBadging(badging({ name: `${RELEASE_PACKAGE}.dev`, debuggable: true, abis: ["'arm64-v8a'", "'x86_64'"] }));
  assert.equal(debug.package, 'com.shodhrag.mobile.dev');
  assert.equal(debug.debuggable, true);
  assert.deepEqual(debug.nativeCode, ['arm64-v8a', 'x86_64']);
  assert.throws(() => parseBadging('ERROR: dump failed'), /no package line/);
});

test('apksigner: the schemes that verified and every certificate', () => {
  assert.deepEqual(parseSigners(signerText()), { verifies: true, v1: false, v2: true, v3: true, signers: 1, certificates: [CERT] });
  // "v3.1" is not read as v3.
  const noV3 = parseSigners(signerText({ v3: false }).replace('Verified using v3.1 scheme (APK Signature Scheme v3.1): false', 'Verified using v3.1 scheme (APK Signature Scheme v3.1): true'));
  assert.equal(noV3.v3, false);
  // The public key's digest is not a certificate's.
  assert.equal(parseSigners(signerText({ certs: [OTHER_CERT] })).certificates.length, 1);
});

// --- A releasable APK ---------------------------------------------------------------

test('a release APK signed by the expected key, v2 and v3, for arm64, passes', () => {
  assert.deepEqual(problems(), []);
});

test('every reason not to ship it is reported', () => {
  const one = (overrides, pattern) => {
    const found = problems(overrides);
    assert.ok(found.some((p) => pattern.test(p)), `${pattern} in ${JSON.stringify(found)}`);
  };
  one({ signers: { certs: [OTHER_CERT] } }, /signed by certificate abab.*, not 6a67/);
  one({ args: { certSha256: OTHER_CERT } }, /not abab/);
  one({ signers: { certs: [CERT, OTHER_CERT] } }, /2 signers, not one/);
  one({ signers: { certs: [] } }, /no signer certificate/);
  one({ signers: { v2: false } }, /Scheme v2/);
  one({ signers: { v3: false } }, /Scheme v3/);
  one({ signers: { verifies: false } }, /does not verify/);
  one({ badging: { name: 'com.shodhrag.mobile.dev' } }, /package is com\.shodhrag\.mobile\.dev, not com\.shodhrag\.mobile/);
  one({ badging: { version: '1.0' } }, /versionName is 1\.0, not 0\.0\.12/);
  one({ badging: { code: '1' } }, /versionCode is 1, not 12/);
  one({ badging: { debuggable: true } }, /debuggable/);
  one({ badging: { abis: ["'arm64-v8a'", "'x86_64'"] } }, /native code is for "arm64-v8a x86_64"/);
  one({ badging: { abis: [] } }, /native code is for ""/);
  one({ args: { size: 0 } }, /empty/);
  one({ args: { size: MAX_APK_BYTES + 1 } }, /more than the 314572800/);
  assert.deepEqual(RELEASE_ABIS, ['arm64-v8a']);
});

test('the CLI asks for exactly one expected certificate', () => {
  const run = (args) => {
    try {
      execFileSync(process.execPath, [script, 'check', '--apk', 'x.apk', '--version', '0.0.12', '--build-tools', '.', ...args], { encoding: 'utf8', stdio: 'pipe' });
      return '';
    } catch (e) {
      return e.stderr;
    }
  };
  assert.match(run([]), /exactly one of --release-cert and --cert-sha256/);
  assert.match(run(['--release-cert', '--cert-sha256', CERT]), /exactly one of --release-cert and --cert-sha256/);
});

// --- Debug-only code -----------------------------------------------------------------

function dist(markers) {
  const dir = mkdtempSync(join(tmpdir(), 'android-dist-'));
  mkdirSync(join(dir, 'assets'));
  writeFileSync(join(dir, 'index.html'), '<!doctype html><div id="root"></div>');
  writeFileSync(join(dir, 'assets', 'index-abc.js'), `console.log(1);${markers.map((m) => JSON.stringify(m)).join(';')}`);
  return dir;
}

const lib = (markers, { control = true } = {}) =>
  Buffer.concat([Buffer.from('\x7fELF'), ...markers.map((m) => Buffer.from(`${m}\0`)), Buffer.from(control ? `${NATIVE_CONTROL}\0` : '')]);

test('a release build has no debug-only code; a debug build has all of it', () => {
  assert.deepEqual(debugMarkerProblems({ build: 'release', dist: dist([]), lib: lib([]) }), []);
  assert.deepEqual(debugMarkerProblems({ build: 'debug', dist: dist(FRONTEND_DEBUG_MARKERS), lib: lib(NATIVE_DEBUG_MARKERS) }), []);
});

test('debug-only code in a release build, or missing from a debug build, is reported', () => {
  const release = debugMarkerProblems({ build: 'release', dist: dist(['shodh://tour']), lib: lib(['debug.shodh.tour']) });
  assert.deepEqual(release, ['the release frontend contains "shodh://tour"', 'the release native library contains "debug.shodh.tour"']);
  const debug = debugMarkerProblems({ build: 'debug', dist: dist(FRONTEND_DEBUG_MARKERS.slice(1)), lib: lib(NATIVE_DEBUG_MARKERS.slice(1)) });
  assert.deepEqual(debug, [`the debug frontend lacks "${FRONTEND_DEBUG_MARKERS[0]}"`, `the debug native library lacks "${NATIVE_DEBUG_MARKERS[0]}"`]);
});

test('the debug-only check refuses to read the wrong files', () => {
  // A library without Shodh's own string is not the app's library.
  assert.match(debugMarkerProblems({ build: 'release', lib: lib([], { control: false }) })[0], /it is not Shodh's/);
  const empty = mkdtempSync(join(tmpdir(), 'android-dist-'));
  assert.match(debugMarkerProblems({ build: 'release', dist: empty })[0], /no index\.html/);
  assert.throws(() => debugMarkerProblems({ build: 'profile', dist: empty }), /debug or release/);
  assert.throws(() => debugMarkerProblems({ build: 'release' }), /--dist, --lib or both/);
});

test("the markers are the ones in the phone app's debug-only code", () => {
  const tour = read('mobile/src-tauri/src/tour.rs');
  const link = read('mobile/src-tauri/src/link.rs');
  for (const m of ['debug.shodh.tour', 'debug.shodh.noblur', 'shodh://tour']) assert.ok(tour.includes(`"${m}"`), `tour.rs has ${m}`);
  assert.ok(link.includes('c"debug.shodh.screen"'), 'link.rs has debug.shodh.screen');
  assert.match(read('mobile/src-tauri/src/lib.rs'), /#\[cfg\(debug_assertions\)\]\s*tour::follow/);
  assert.ok(read('crates/shodh-link/src/update.rs').includes(`"${NATIVE_CONTROL}"`), 'the control string is shodh-link\'s');
});
