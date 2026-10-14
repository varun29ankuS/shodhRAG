/**
 * The top bar's connection status: reading the backend's snapshot, the dot
 * colour of each health, and the pill's and panel's text.
 *   node --experimental-strip-types --test app/tests/connectionStatus.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DOT_CLASS,
  activeStatus,
  checkedText,
  fixLabel,
  healthDot,
  healthText,
  keyOriginNote,
  pillView,
  readSnapshot,
  shortModel,
  sourceText,
} from '../src/features/connection/connectionStatus.ts';
import type { ConnectionSnapshot, ProviderStatus } from '../src/features/connection/connectionStatus.ts';

const NOW = 1_759_750_000_000;
const at = (ms: number) => `T+${Math.round((ms - NOW) / 60_000)}min`;

function status(extra: Partial<ProviderStatus> = {}): ProviderStatus {
  return {
    provider: 'openrouter',
    label: 'OpenRouter',
    source: 'saved',
    health: 'works',
    message: null,
    httpStatus: null,
    quota: false,
    resetAtMs: null,
    checkedAtMs: NOW,
    learnedFrom: 'check',
    keyHint: '…a3f9',
    envVar: null,
    checking: false,
    ...extra,
  };
}

function snapshot(providers: ProviderStatus[]): ConnectionSnapshot {
  return {
    active: { model: { provider: 'openrouter', model: 'nvidia/nemotron-3-super-120b-a12b:free' }, source: 'settings' },
    providers,
    localOnly: false,
  };
}

test('each health has its dot colour', () => {
  assert.equal(healthDot('works'), 'green');
  assert.equal(healthDot('rejected'), 'red');
  assert.equal(healthDot('limited'), 'amber');
  assert.equal(healthDot('unchecked'), 'grey');
  assert.equal(healthDot('unreachable'), 'grey');
  assert.equal(DOT_CLASS.green, 'bg-shodh-success');
  assert.equal(DOT_CLASS.red, 'bg-shodh-error');
  assert.equal(DOT_CLASS.amber, 'bg-shodh-warning');
  assert.equal(DOT_CLASS.grey, 'bg-shodh-text-faint');
});

test('the pill names the model, the provider and how it is connected', () => {
  const pill = pillView(snapshot([status()]), at);
  assert.equal(pill.model, 'nemotron-3-super-120b-a12b:free');
  assert.equal(pill.via, 'OpenRouter · saved key');
  assert.equal(pill.dot, 'green');
  assert.match(pill.description, /^nvidia\/nemotron-3-super-120b-a12b:free through OpenRouter \(Key saved in the app\)\. Working\.$/);

  const env = pillView(snapshot([status({ source: 'environment', envVar: 'OPENROUTER_API_KEY' })]), at);
  assert.equal(env.via, 'OpenRouter · env key');
  assert.match(env.description, /Key from the OPENROUTER_API_KEY environment variable/);

  const sub = pillView({ ...snapshot([status({ provider: 'claude-sub', label: 'Claude (Pro/Max)', source: 'subscription', keyHint: null })]), active: { model: { provider: 'claude-sub', model: 'claude-sonnet-4-5' }, source: 'settings' } }, at);
  assert.equal(sub.via, 'Claude (Pro/Max) · sign-in');
  const local = pillView({ ...snapshot([status({ provider: 'lmstudio', label: 'LM Studio', source: 'local', keyHint: null, health: 'unreachable', message: "LM Studio isn't running on this computer." })]), active: { model: { provider: 'lmstudio', model: 'qwen3-8b' }, source: 'settings' } }, at);
  assert.equal(local.via, 'LM Studio · local');
  assert.equal(local.dot, 'grey');
});

test('a rejected key is red with the provider message', () => {
  const rejected = status({ health: 'rejected', httpStatus: 401, message: 'User not found.', learnedFrom: 'answer' });
  const pill = pillView(snapshot([rejected]), at);
  assert.equal(pill.dot, 'red');
  assert.equal(healthText(rejected, at), 'Key rejected (401): User not found.');
  assert.match(pill.description, /Key rejected \(401\): User not found\.$/);
  assert.equal(checkedText(rejected, NOW + 30_000, at), 'Seen in a chat just now');
});

test('limits are amber with their reset time', () => {
  const daily = status({ health: 'limited', quota: true, resetAtMs: NOW + 120 * 60_000, message: 'Rate limit exceeded: free-models-per-day.' });
  assert.equal(pillView(snapshot([daily]), at).dot, 'amber');
  assert.equal(healthText(daily, at), 'Out of credit or daily quota, resets T+120min: Rate limit exceeded: free-models-per-day.');
  const minute = status({ health: 'limited', quota: false, resetAtMs: null, message: null });
  assert.equal(healthText(minute, at), 'Rate-limited');
});

test('grey covers not checked, unreachable, no model and loading', () => {
  assert.equal(healthText(status({ health: 'unchecked', checkedAtMs: null }), at), 'Not checked yet');
  assert.equal(healthText(status({ health: 'unreachable', message: 'connection refused' }), at), "Can't reach OpenRouter: connection refused");
  assert.equal(pillView(snapshot([status({ health: 'unchecked' })]), at).dot, 'grey');
  const none = pillView({ active: null, providers: [], localOnly: false }, at);
  assert.equal(none.model, 'No model chosen');
  assert.equal(none.dot, 'grey');
  assert.equal(pillView(null, at).dot, 'grey');
  // The active provider not connected at all.
  const missing = pillView(snapshot([]), at);
  assert.equal(missing.dot, 'grey');
  assert.equal(missing.via, 'OpenRouter · not connected');
});

test('the panel says which key is in use when an environment variable is set too', () => {
  assert.equal(
    keyOriginNote(status({ source: 'saved', envVar: 'OPENROUTER_API_KEY' })),
    'Using the key saved in the app. OPENROUTER_API_KEY is also set, but it is not used.',
  );
  assert.equal(
    keyOriginNote(status({ source: 'environment', envVar: 'OPENROUTER_API_KEY' })),
    'Using the key from OPENROUTER_API_KEY. No key is saved in the app.',
  );
  assert.equal(keyOriginNote(status({ source: 'saved', envVar: null })), null);
  assert.equal(sourceText(status({ source: 'saved' })), 'Key saved in the app');
  assert.equal(fixLabel(status()), 'Fix key');
  assert.equal(fixLabel(status({ source: 'subscription' })), 'Fix sign-in');
});

test('checked times read relative, then as a time', () => {
  assert.equal(checkedText(status(), NOW + 10_000, at), 'Checked just now');
  assert.equal(checkedText(status(), NOW + 4 * 60_000 + 5_000, at), 'Checked 4 min ago');
  assert.equal(checkedText(status(), NOW + 90 * 60_000, at), 'Checked at T+0min');
  assert.equal(checkedText(status({ checking: true }), NOW, at), 'Checking…');
  assert.equal(checkedText(status({ checkedAtMs: null }), NOW, at), 'Not checked yet');
});

test('the snapshot reader rejects malformed data and never shows more of a key than four characters', () => {
  assert.equal(readSnapshot(null), null);
  assert.equal(readSnapshot({ providers: 'x' }), null);
  const read = readSnapshot({
    active: { model: { provider: 'openrouter', model: 'openai/gpt-4o' }, source: 'settings' },
    providers: [
      { provider: 'openrouter', label: 'OpenRouter', source: 'saved', health: 'rejected', keyHint: '…0123456789a3f9', httpStatus: 401, checking: false },
      { provider: 'nope', health: 'works' },
      { provider: 'openai', health: 'mystery', keyHint: 'sk-proj-full-key' },
    ],
    localOnly: false,
  });
  assert.ok(read);
  assert.equal(read.providers.length, 2);
  assert.equal(read.providers[0].keyHint, '…a3f9');
  assert.equal(read.providers[1].health, 'unchecked');
  assert.equal(read.providers[1].keyHint, null, 'a hint must be masked');
  assert.equal(activeStatus(read)?.provider, 'openrouter');
  assert.equal(shortModel('openai/gpt-4o'), 'gpt-4o');
  assert.equal(shortModel('qwen3-8b'), 'qwen3-8b');
});
