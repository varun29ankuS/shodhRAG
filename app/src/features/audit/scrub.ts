/**
 * An Activity entry as shown and copied in its "Raw entry (JSON)" view: the value
 * of every field whose name says it holds a secret (`api_key`, `password`,
 * `authToken`, `client_secret`) is replaced, at any depth. No event writes one
 * today; this keeps one that ever did from being shown or copied. The names
 * follow the redactor's strong names (crates/shodh-rag/src/recall/redact.rs,
 * `secrecy`).
 *
 * Pure module, unit-tested with Node (`app/tests/auditScrub.test.ts`).
 */

/** What a secret field's value is shown as. */
export const SCRUBBED = '[REDACTED]';

/** Words that make a name secret wherever they are in it. */
const ANYWHERE = new Set([
  'password', 'passwd', 'passphrase', 'pwd', 'secret', 'secrets', 'credential', 'credentials',
  'apikey', 'privatekey', 'authorization', 'dsn', 'cookie',
]);
/** Words that make a name secret when they end it. */
const LAST = new Set(['pass', 'pw', 'key', 'token', 'secret', 'auth', 'sig', 'signature', 'salt']);
/** Pairs of words that make a name secret together. */
const PAIRS = new Set([
  'api key', 'access key', 'private key', 'secret key', 'client secret', 'auth token',
  'access token', 'refresh token', 'session key', 'signing key', 'key data', 'webhook url',
]);

/** A field name's words, lower case: `authToken`, `AUTH_TOKEN` and `auth-token` are `auth token`. */
export function nameWords(name: string): string[] {
  return name
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .split(/[^A-Za-z0-9]+/)
    .filter(Boolean)
    .map(w => w.toLowerCase());
}

/** Whether a field called `name` holds a secret, whatever its value. */
export function isSecretName(name: string): boolean {
  const words = nameWords(name);
  if (words.length === 0) return false;
  if (words.some(w => ANYWHERE.has(w))) return true;
  if (LAST.has(words[words.length - 1])) return true;
  for (let i = 0; i + 1 < words.length; i += 1) {
    if (PAIRS.has(`${words[i]} ${words[i + 1]}`)) return true;
  }
  return false;
}

/** `value` with every secret field's value replaced by [`SCRUBBED`], at any depth. */
export function scrubSecrets(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(scrubSecrets);
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [key, field] of Object.entries(value as Record<string, unknown>)) {
      out[key] = isSecretName(key) && field !== null && field !== '' ? SCRUBBED : scrubSecrets(field);
    }
    return out;
  }
  return value;
}
