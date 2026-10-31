/**
 * Plain-words verification wording. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/verification.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  checksumRow,
  runtimeCheckPromise,
  runtimeVerifiedSentence,
  unverifiedSentence,
  verificationPromise,
  verifiedSentence,
} from '../src/features/setup/verification.ts';

const HASH = /sha-?256|checksum|digest|\b[0-9a-f]{16,}\b/i;

test('verification sentences say what the check means, never show a hash', () => {
  assert.equal(
    verifiedSentence('GitHub'),
    "Verified: this file matches the official copy from GitHub, so it hasn't been changed or tampered with.",
  );
  assert.match(verifiedSentence('Hugging Face', true), /^Verified: these files match the official copies from Hugging Face/);
  assert.match(unverifiedSentence(), /Shodh won't use it/);
  assert.match(unverifiedSentence(true), /Shodh won't use them/);
  assert.match(verificationPromise('GitHub'), /^Shodh downloads it from GitHub and checks/);
  for (const sentence of [verifiedSentence('GitHub'), verifiedSentence('x', true), unverifiedSentence(), unverifiedSentence(true), verificationPromise('GitHub')]) {
    assert.doesNotMatch(sentence, HASH, sentence);
  }
});

test('the technical details row keeps the whole checksum for comparing', () => {
  const hash = 'ABCDEF0123456789'.repeat(4);
  assert.deepEqual(checksumRow('Answer engine', hash), { label: 'Answer engine', value: `SHA-256 ${hash.toLowerCase()}` });
});

test('setup cards show checksums only inside Technical details', () => {
  const root = join(dirname(fileURLToPath(import.meta.url)), '../src/features');
  for (const file of ['setup/SetupCard.tsx', 'agent/RuntimeCard.tsx', 'setup/FirstRunFlow.tsx']) {
    const source = readFileSync(join(root, file), 'utf8');
    // No visible SHA wording in the markup; hashes reach the page only through checksumRow.
    assert.doesNotMatch(source, /sha256 \$\{|SHA-256 checksum|checks its SHA-256|\.sha256\.slice/, file);
  }
});

test('off Windows the answer engine is checked against the release checksum, and says only that', () => {
  assert.equal(runtimeVerifiedSentence('GitHub', true), verifiedSentence('GitHub'));
  const unpinned = runtimeVerifiedSentence('GitHub', false);
  assert.match(unpinned, /checksum GitHub published with the release/);
  assert.doesNotMatch(unpinned, /official copy|tampered/);
  assert.match(runtimeCheckPromise('GitHub', false), /checksum published with the release/);
  assert.doesNotMatch(runtimeCheckPromise('GitHub', false), HASH);
});

test('a status that only saw the file never says verified', () => {
  const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '../src/features/setup/readiness.ts'), 'utf8');
  assert.doesNotMatch(source, /Installed and verified/);
  assert.match(source, /Installed \(checked before each launch\)/);
});
