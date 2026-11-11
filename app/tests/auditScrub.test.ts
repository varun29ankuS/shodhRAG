/**
 * The Activity view's raw entry never shows or copies a secret field's value.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/auditScrub.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { SCRUBBED, isSecretName, scrubSecrets } from '../src/features/audit/scrub.ts';

test('fields named for secrets are scrubbed at any depth; the rest is kept', () => {
  const entry = {
    kind: 'api_key_set',
    payload: {
      provider: 'openrouter',
      apiKey: 'not-a-real-key',
      nested: { client_secret: 'shh', AUTH_TOKEN: 'tok', max_tokens: 512, token_count: 9 },
      headers: [{ Authorization: 'Bearer x' }, { accept: 'json' }],
      password: null,
    },
  };
  assert.deepEqual(scrubSecrets(entry), {
    kind: 'api_key_set',
    payload: {
      provider: 'openrouter',
      apiKey: SCRUBBED,
      nested: { client_secret: SCRUBBED, AUTH_TOKEN: SCRUBBED, max_tokens: 512, token_count: 9 },
      headers: [{ Authorization: SCRUBBED }, { accept: 'json' }],
      password: null,
    },
  });
});

test('names are read as the redactor reads them', () => {
  for (const name of ['api_key', 'apiKey', 'refresh-token', 'DB_PASS', 'sessionKey', 'cookie', 'tls.key']) {
    assert.ok(isSecretName(name), name);
  }
  for (const name of ['provider', 'keymap', 'tokens', 'max_tokens', 'keyboard', 'path', 'kind']) {
    assert.ok(!isSecretName(name), name);
  }
});

test('the raw entry is scrubbed where it is shown and where it is copied', () => {
  const source = readFileSync(new URL('../src/features/audit/AuditTable.tsx', import.meta.url), 'utf8');
  assert.match(source, /prettyJson\(scrubSecrets\(row\.payload\)/);
  assert.match(source, /JSON\.stringify\(scrubSecrets\(row\), null, 2\)/);
});
