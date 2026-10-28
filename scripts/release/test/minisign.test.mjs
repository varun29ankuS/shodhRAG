import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { SignatureError, parsePublicKey, parseSignature, unwrapTauri, verifyTauriSignature } from '../minisign.mjs';
import { makeKey, signTauri, signText } from './helpers.mjs';

const dir = mkdtempSync(join(tmpdir(), 'minisign-test-'));
const file = join(dir, 'bundle.bin');
const data = Buffer.from('an update bundle '.repeat(5000));
writeFileSync(file, data);

const conf = JSON.parse(readFileSync(new URL('../../../app/src-tauri/tauri.conf.json', import.meta.url), 'utf8'));

test('a Tauri signature of the file verifies and returns the trusted comment', async () => {
  const key = makeKey();
  const comment = await verifyTauriSignature(file, signTauri(key, data), key.tauri);
  assert.equal(comment, 'timestamp:1793082594\tfile:test.bin');
});

test('a legacy (not prehashed) signature verifies', async () => {
  const key = makeKey();
  await verifyTauriSignature(file, signTauri(key, data, { algorithm: 'Ed' }), key.tauri);
});

test('a changed file fails', async () => {
  const key = makeKey();
  const other = join(dir, 'other.bin');
  writeFileSync(other, Buffer.concat([data, Buffer.from('x')]));
  await assert.rejects(verifyTauriSignature(other, signTauri(key, data), key.tauri), /does not match the file/);
});

test('a signature by another key fails on the key id', async () => {
  await assert.rejects(verifyTauriSignature(file, signTauri(makeKey(), data), makeKey().tauri), /Signed with key/);
});

test('a signature by another key with the same key id fails on the signature', async () => {
  const id = Buffer.from('0123456789abcdef', 'hex');
  await assert.rejects(verifyTauriSignature(file, signTauri(makeKey(id), data), makeKey(id).tauri), /does not match the file/);
});

test('a changed trusted comment fails', async () => {
  const key = makeKey();
  const text = signText(key, data).replace('file:test.bin', 'file:other.bin');
  await assert.rejects(
    verifyTauriSignature(file, Buffer.from(text).toString('base64'), key.tauri),
    /trusted comment signature/,
  );
});

test('malformed input is refused', () => {
  assert.throws(() => unwrapTauri('not base64!', 'x'), SignatureError);
  assert.throws(() => parsePublicKey('just one line'), SignatureError);
  assert.throws(() => parseSignature('untrusted comment: a\nAAAA\n'), SignatureError);
  const key = makeKey();
  const text = signText(key, data);
  assert.throws(() => parseSignature(text.replace(/^untrusted comment: [^\n]*\n[^\n]*/, (m) => m.slice(0, -4))), SignatureError);
});

test("the app's updater public key is a minisign key with the id in its comment", () => {
  const key = parsePublicKey(unwrapTauri(conf.plugins.updater.pubkey, 'pubkey'));
  const comment = unwrapTauri(conf.plugins.updater.pubkey, 'pubkey').split('\n')[0];
  assert.ok(comment.endsWith(key.keyIdHex), `${comment} names ${key.keyIdHex}`);
});

test('a real signature (v0.0.10 Windows installer) parses with the release key id', () => {
  const v0010 =
    'dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUTkRraHg5L2JDdm9keGdsS3RUeUYrOWRyRDl6aG5vMjVTeUZraHRRSnNTWVRpQytXWTVDVWVsTFRYa1hLNk1RQzhTbGt6U0NhS2E4bDJ6bjNOTnJtb2VxZmFvK1R2NmdjPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzMDgyNTk0CWZpbGU6U2hvZGhfMC4wLjEwX3g2NC1zZXR1cC5leGUKb3NHNTVQbWREaTh4a01va2VwTkI5NElaUlFXRGx0SXJhRjRhN040ZjBqdk53RVVMd0tBNkloMTVGcEVicFVTbFlGTk9HcG1NQU42bThTeG1BTDRIQlE9PQo=';
  const signature = parseSignature(unwrapTauri(v0010, 'signature'));
  assert.equal(signature.algorithm, 'ED');
  assert.equal(signature.keyIdHex, 'BEC2F6F771480ECD');
  assert.equal(signature.trustedComment, 'timestamp:1793082594\tfile:Shodh_0.0.10_x64-setup.exe');
});
