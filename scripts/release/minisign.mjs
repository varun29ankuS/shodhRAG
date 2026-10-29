// Minisign signature verification, as Tauri's updater does it
// (tauri-plugin-updater -> minisign-verify), with Node's built-in Ed25519 and
// BLAKE2b-512.
//
// Tauri stores both halves base64-encoded: tauri.conf.json's updater pubkey
// is base64 of a minisign public key file, and a .sig file (and the
// `signature` field of latest.json) is base64 of a minisign signature file.
//
// Public key file:  untrusted comment line, then base64("Ed" | key id (8) | Ed25519 key (32)).
// Signature file:   untrusted comment line,
//                   base64(algorithm (2) | key id (8) | Ed25519 signature (64)),
//                   "trusted comment: <text>",
//                   base64(Ed25519 signature of (signature | text)).
// Algorithm "ED" signs BLAKE2b-512(file) (prehashed, what Tauri produces);
// "Ed" (legacy) signs the file itself.

import { createHash, createPublicKey, verify as ed25519Verify } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { readFile } from 'node:fs/promises';

const TRUSTED_PREFIX = 'trusted comment: ';
const UNTRUSTED_PREFIX = 'untrusted comment: ';

export class SignatureError extends Error {}

function strictBase64(text, what) {
  const trimmed = text.trim();
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(trimmed) || trimmed.length % 4 !== 0) {
    throw new SignatureError(`${what} is not base64`);
  }
  return Buffer.from(trimmed, 'base64');
}

function lines(text) {
  return text.replace(/\r\n/g, '\n').split('\n');
}

/** Decodes Tauri's base64 wrapping of a minisign file into its text. */
export function unwrapTauri(base64Text, what) {
  return strictBase64(base64Text, what).toString('utf8');
}

/** Parses a minisign public key file (text). */
export function parsePublicKey(text) {
  const body = lines(text).filter((l) => l.trim() !== '');
  if (body.length !== 2 || !body[0].startsWith(UNTRUSTED_PREFIX)) {
    throw new SignatureError('The public key is not a minisign public key (comment line, key line)');
  }
  const raw = strictBase64(body[1], 'The public key line');
  if (raw.length !== 42 || raw.subarray(0, 2).toString('latin1') !== 'Ed') {
    throw new SignatureError(`The public key line has ${raw.length} bytes or an unknown algorithm; expected "Ed" and 42 bytes`);
  }
  const keyId = raw.subarray(2, 10);
  const publicKey = raw.subarray(10, 42);
  const key = createPublicKey({
    key: { kty: 'OKP', crv: 'Ed25519', x: publicKey.toString('base64url') },
    format: 'jwk',
  });
  return { keyId, keyIdHex: keyIdHex(keyId), key };
}

/** Parses a minisign signature file (text). */
export function parseSignature(text) {
  const body = lines(text);
  while (body.length && body[body.length - 1].trim() === '') body.pop();
  if (body.length !== 4 || !body[0].startsWith(UNTRUSTED_PREFIX) || !body[2].startsWith(TRUSTED_PREFIX)) {
    throw new SignatureError('The signature is not a minisign signature (4 lines: comment, signature, trusted comment, global signature)');
  }
  const raw = strictBase64(body[1], 'The signature line');
  if (raw.length !== 74) throw new SignatureError(`The signature line has ${raw.length} bytes, expected 74`);
  const algorithm = raw.subarray(0, 2).toString('latin1');
  if (algorithm !== 'ED' && algorithm !== 'Ed') {
    throw new SignatureError(`Unknown signature algorithm ${JSON.stringify(algorithm)}`);
  }
  const globalSignature = strictBase64(body[3], 'The global signature line');
  if (globalSignature.length !== 64) {
    throw new SignatureError(`The global signature has ${globalSignature.length} bytes, expected 64`);
  }
  const keyId = raw.subarray(2, 10);
  return {
    algorithm,
    keyId,
    keyIdHex: keyIdHex(keyId),
    signature: raw.subarray(10, 74),
    trustedComment: body[2].slice(TRUSTED_PREFIX.length),
    globalSignature,
  };
}

// minisign prints key ids as the little-endian 64-bit number in hex.
function keyIdHex(keyId) {
  return Buffer.from(keyId).reverse().toString('hex').toUpperCase();
}

async function blake2b512File(path) {
  const hash = createHash('blake2b512');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest();
}

/**
 * Verifies `filePath` against a parsed signature and public key. Throws a
 * SignatureError naming what failed; returns the trusted comment.
 */
export async function verifyFile(filePath, signature, publicKey) {
  if (!signature.keyId.equals(publicKey.keyId)) {
    throw new SignatureError(`Signed with key ${signature.keyIdHex}, but the public key is ${publicKey.keyIdHex}`);
  }
  const message = signature.algorithm === 'ED' ? await blake2b512File(filePath) : await readFile(filePath);
  if (!ed25519Verify(null, message, publicKey.key, signature.signature)) {
    throw new SignatureError('The signature does not match the file');
  }
  const global = Buffer.concat([signature.signature, Buffer.from(signature.trustedComment, 'utf8')]);
  if (!ed25519Verify(null, global, publicKey.key, signature.globalSignature)) {
    throw new SignatureError('The trusted comment signature does not verify');
  }
  return signature.trustedComment;
}

/** Tauri's form: base64 pubkey (tauri.conf.json) and base64 signature (.sig / latest.json). */
export async function verifyTauriSignature(filePath, signatureBase64, pubkeyBase64) {
  const publicKey = parsePublicKey(unwrapTauri(pubkeyBase64, 'The updater public key'));
  const signature = parseSignature(unwrapTauri(signatureBase64, 'The signature'));
  return verifyFile(filePath, signature, publicKey);
}
