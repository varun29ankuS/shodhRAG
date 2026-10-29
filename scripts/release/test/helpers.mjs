// Builds minisign keys and Tauri-style signatures from the format's
// definition, independently of minisign.mjs, so its tests check it against
// the specification rather than against itself.

import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';

export function makeKey(keyId = randomBytes(8)) {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const raw = Buffer.from(publicKey.export({ format: 'jwk' }).x, 'base64url');
  const idHex = Buffer.from(keyId).reverse().toString('hex').toUpperCase();
  const text = `untrusted comment: minisign public key: ${idHex}\n${Buffer.concat([Buffer.from('Ed'), keyId, raw]).toString('base64')}\n`;
  return { keyId, privateKey, idHex, text, tauri: Buffer.from(text).toString('base64') };
}

/** A minisign signature file (text) of `data` by `key`. */
export function signText(key, data, { trustedComment = 'timestamp:1793082594\tfile:test.bin', algorithm = 'ED' } = {}) {
  const message = algorithm === 'ED' ? createHash('blake2b512').update(data).digest() : data;
  const signature = sign(null, message, key.privateKey);
  const global = sign(null, Buffer.concat([signature, Buffer.from(trustedComment)]), key.privateKey);
  return [
    'untrusted comment: signature from tauri secret key',
    Buffer.concat([Buffer.from(algorithm), key.keyId, signature]).toString('base64'),
    `trusted comment: ${trustedComment}`,
    global.toString('base64'),
    '',
  ].join('\n');
}

/** What tauri writes to a .sig file and to latest.json: base64 of the signature file. */
export function signTauri(key, data, options) {
  return Buffer.from(signText(key, data, options)).toString('base64');
}
