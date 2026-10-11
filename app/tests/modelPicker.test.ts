/**
 * Model picker: prices and context as shown, grouping and search, reading
 * backend data, provider error copy, the retry cap and the fallback policy.
 *   node --experimental-strip-types --test app/tests/modelPicker.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  catalogNotice,
  chipEntries,
  pickUnavailableText,
  formatContext,
  formatPrice,
  formatUsd,
  matchesQuery,
  modelName,
  pickerSections,
  privacyText,
  sourceText,
} from '../src/features/modelPicker/modelFormat.ts';
import {
  EMPTY_MODEL_PREFS,
  modelRefFromRun,
  readModelPrefs,
  readProviderError,
  sameModel,
  toPickerError,
} from '../src/features/modelPicker/modelTypes.ts';
import type { CatalogModel, ModelPrefs, ProviderError } from '../src/features/modelPicker/modelTypes.ts';
import {
  AutoRetryLedger,
  DEFAULT_RETRY_SECS,
  MAX_AUTO_RETRY_SECS,
  autoRetrySeconds,
  canRetry,
  fallbackDecision,
  fallbackHelps,
  providerErrorCopy,
} from '../src/features/modelPicker/providerErrors.ts';

function model(partial: Partial<CatalogModel> & Pick<CatalogModel, 'id'>): CatalogModel {
  return {
    provider: 'openrouter',
    name: partial.id,
    contextLength: null,
    promptPerMillion: null,
    completionPerMillion: null,
    tools: 'yes',
    tier: 'paid',
    privacy: 'provider_terms',
    stealth: false,
    ...partial,
  };
}

const HAIKU = model({ id: 'anthropic/claude-haiku-4.5', name: 'Claude Haiku 4.5', promptPerMillion: 1, completionPerMillion: 5, contextLength: 200_000 });
const NEMOTRON = model({ id: 'nvidia/nemotron:free', name: 'Nemotron (free)', tier: 'free', privacy: 'may_log_prompts', promptPerMillion: 0, completionPerMillion: 0, contextLength: 262_144 });
const QWEN = model({ id: 'qwen3:4b', name: 'qwen3:4b', provider: 'ollama', tier: 'local', privacy: 'on_device', tools: 'unknown' });
const GPT = model({ id: 'gpt-5-mini', name: 'GPT-5 mini', provider: 'openai' });
const ALL = [HAIKU, NEMOTRON, QWEN, GPT];

test('prices: per million tokens, free, local and unknown', () => {
  assert.equal(formatUsd(3), '$3');
  assert.equal(formatUsd(0.18), '$0.18');
  assert.equal(formatUsd(0.076), '$0.08');
  assert.equal(formatUsd(0.000_5), '<$0.01');
  assert.equal(formatUsd(150.4), '$150');
  assert.equal(formatUsd(0), '$0');
  assert.equal(formatUsd(null), null);
  assert.equal(formatUsd(-1), null);
  assert.equal(formatUsd(Number.NaN), null);
  assert.equal(formatPrice(HAIKU), '$1 in · $5 out per 1M tokens');
  assert.equal(formatPrice(NEMOTRON), 'Free');
  assert.equal(formatPrice(QWEN), 'On this device');
  assert.equal(formatPrice(GPT), 'Price unknown', 'offline: no list to price from');
  assert.equal(formatPrice({ tier: 'paid', promptPerMillion: 1, completionPerMillion: null }), 'Price unknown');
});

test('context windows', () => {
  assert.equal(formatContext(200_000), '200K');
  assert.equal(formatContext(262_144), '262K');
  assert.equal(formatContext(1_000_000), '1M');
  assert.equal(formatContext(1_048_576), '1.0M');
  assert.equal(formatContext(8_192), '8,192');
  assert.equal(formatContext(null), null);
  assert.equal(formatContext(0), null);
});

test('privacy notes', () => {
  assert.match(privacyText('may_log_prompts', false), /may log prompts/);
  assert.match(privacyText('provider_terms', true), /Stealth model/);
  assert.equal(privacyText('on_device', false), 'Stays on this computer.');
});

test('sections: favourites, recent (without repeats), then Free / Paid / Local by name', () => {
  const prefs: ModelPrefs = {
    ...EMPTY_MODEL_PREFS,
    favourites: [{ provider: 'openai', model: 'gpt-5-mini' }],
    recent: [
      { provider: 'openai', model: 'gpt-5-mini' },
      { provider: 'openrouter', model: 'nvidia/nemotron:free' },
      { provider: 'openrouter', model: 'gone/model' },
    ],
  };
  const sections = pickerSections(ALL, prefs, '');
  assert.deepEqual(sections.map(s => s.id), ['favourites', 'recent', 'free', 'paid', 'local']);
  assert.deepEqual(sections[0].models.map(m => m.id), ['gpt-5-mini']);
  assert.deepEqual(sections[1].models.map(m => m.id), ['nvidia/nemotron:free'], 'a favourite is not repeated; unlisted models are skipped');
  assert.deepEqual(sections[3].models.map(m => m.name), ['Claude Haiku 4.5', 'GPT-5 mini']);
});

test('search matches every word across name, id and provider, and lists each match once', () => {
  const prefs: ModelPrefs = { ...EMPTY_MODEL_PREFS, favourites: [{ provider: 'openrouter', model: HAIKU.id }] };
  assert.ok(matchesQuery(HAIKU, 'claude haiku'));
  assert.ok(matchesQuery(HAIKU, 'OPENROUTER 4.5'));
  assert.ok(!matchesQuery(HAIKU, 'claude opus'));
  const found = pickerSections(ALL, prefs, 'haiku');
  assert.deepEqual(found.map(s => s.id), ['paid']);
  assert.equal(pickerSections(ALL, prefs, 'zzz').length, 0);
  assert.deepEqual(pickerSections(ALL, prefs, 'ollama').map(s => s.id), ['local']);
});

test('names and sources', () => {
  assert.equal(modelName(ALL, { provider: 'openrouter', model: HAIKU.id }), 'Claude Haiku 4.5');
  assert.equal(modelName(ALL, { provider: 'openrouter', model: 'x/unlisted' }), 'x/unlisted');
  assert.equal(modelName(ALL, null), 'No model');
  assert.equal(sourceText({ model: { provider: 'openai', model: 'gpt-5' }, source: 'environment' }), 'Set by environment');
  assert.equal(sourceText({ model: { provider: 'openai', model: 'gpt-5' }, source: 'session' }), 'This session only');
  assert.equal(sourceText({ model: { provider: 'openai', model: 'gpt-5' }, source: 'settings' }), null);
});

test('catalog notices: offline shows the age of the saved prices', () => {
  const now = 1_759_750_000_000;
  const base = { keyed: ['openrouter' as const], ollamaRunning: false };
  assert.equal(catalogNotice({ ...base, catalogStatus: 'fresh', catalogFetchedAtMs: now }, now), null);
  assert.equal(catalogNotice({ ...base, catalogStatus: 'cached', catalogFetchedAtMs: now - 3 * 3_600_000 }, now), 'Offline: prices from a list saved 3 hours ago.');
  assert.match(catalogNotice({ ...base, catalogStatus: 'unavailable', catalogFetchedAtMs: null }, now) ?? '', /Prices are unknown/);
  assert.equal(catalogNotice({ keyed: [], ollamaRunning: true, catalogStatus: 'unavailable', catalogFetchedAtMs: null }, now), null);
  assert.match(catalogNotice({ ...base, catalogStatus: 'local_only', catalogFetchedAtMs: null }, now) ?? '', /Local-only/);
});

test('reading backend data defensively', () => {
  assert.deepEqual(readProviderError({ kind: 'rate_limited', status: 429, retryAfterSecs: 12 }), { kind: 'rate_limited', status: 429, retryAfterSecs: 12 });
  assert.deepEqual(readProviderError({ kind: 'auth', status: null }), { kind: 'auth', status: null, retryAfterSecs: null });
  assert.equal(readProviderError({ kind: 'teapot' }), null);
  assert.equal(readProviderError(undefined), null);
  assert.equal(readProviderError({ kind: 'other', status: -3 })?.status, null);

  const prefs = readModelPrefs({
    chosen: { provider: 'openrouter', model: 'a/b' },
    fallback: { provider: 'acme', model: 'x' },
    alwaysFallBack: 'yes',
    recent: [{ provider: 'openai', model: 'gpt-5' }, null, { provider: 'openai', model: '' }],
    stealthAccepted: ['stealth/x', 3],
  });
  assert.deepEqual(prefs.chosen, { provider: 'openrouter', model: 'a/b' });
  assert.equal(prefs.fallback, null);
  assert.equal(prefs.alwaysFallBack, false);
  assert.equal(prefs.recent.length, 1);
  assert.deepEqual(prefs.stealthAccepted, ['stealth/x']);
  assert.deepEqual(readModelPrefs(undefined), EMPTY_MODEL_PREFS);

  assert.deepEqual(toPickerError({ code: 'stealth_confirmation', message: 'confirm' }), { code: 'stealth_confirmation', message: 'confirm' });
  assert.equal(toPickerError({ code: 'weird', message: 'm' }).code, 'failed');
  assert.equal(toPickerError('plain').message, 'plain');
});

test('the model a run used, from the runtime name', () => {
  assert.deepEqual(modelRefFromRun('openrouter/anthropic/claude-haiku-4.5'), { provider: 'openrouter', model: 'anthropic/claude-haiku-4.5' });
  assert.deepEqual(modelRefFromRun('xai/grok-4'), { provider: 'grok', model: 'grok-4' });
  assert.deepEqual(modelRefFromRun('ollama/qwen3:4b'), { provider: 'ollama', model: 'qwen3:4b' });
  assert.equal(modelRefFromRun('acme/x'), null);
  assert.equal(modelRefFromRun('openrouter/'), null);
  assert.equal(modelRefFromRun(null), null);
  assert.ok(sameModel({ provider: 'openai', model: 'a' }, { provider: 'openai', model: 'a' }));
  assert.ok(!sameModel({ provider: 'openai', model: 'a' }, null));
});

const RATE: ProviderError = { kind: 'rate_limited', status: 429, retryAfterSecs: 12 };

test('error copy says what happened and what to do', () => {
  const rate = providerErrorCopy(RATE, 'nemotron:free', 'openrouter');
  assert.equal(rate.title, 'nemotron:free is rate-limited');
  const auth = providerErrorCopy({ kind: 'auth', status: 401, retryAfterSecs: null }, 'gpt-5', 'openai');
  assert.equal(auth.title, 'Your OpenAI key was rejected');
  assert.match(auth.hint, /Settings → Model/);
  assert.match(providerErrorCopy({ kind: 'quota_exhausted', status: 402, retryAfterSecs: null }, 'm', null).title, /the provider credits/);
});

test('automatic retry: rate limits only, bounded wait, once per question', () => {
  assert.equal(autoRetrySeconds(RATE, 0), 12);
  assert.equal(autoRetrySeconds({ ...RATE, retryAfterSecs: null }, 0), DEFAULT_RETRY_SECS);
  assert.equal(autoRetrySeconds({ ...RATE, retryAfterSecs: MAX_AUTO_RETRY_SECS + 1 }, 0), null, 'long waits are a click');
  assert.equal(autoRetrySeconds({ ...RATE, retryAfterSecs: 0 }, 0), 1);
  assert.equal(autoRetrySeconds(RATE, 1), null, 'already retried automatically');
  assert.equal(autoRetrySeconds({ kind: 'model_unavailable', status: 503, retryAfterSecs: 5 }, 0), null);
  assert.equal(canRetry({ kind: 'auth', status: 401, retryAfterSecs: null }), false);
  assert.equal(canRetry({ kind: 'quota_exhausted', status: 402, retryAfterSecs: null }), false);
  assert.equal(canRetry(RATE), true);

  const ledger = new AutoRetryLedger(2);
  ledger.record('q1');
  assert.equal(ledger.count('q1'), 1);
  assert.equal(autoRetrySeconds(RATE, ledger.count('q1')), null);
  ledger.record('q2');
  ledger.record('q3');
  assert.equal(ledger.count('q1'), 0, 'the oldest questions are forgotten past the cap');
});

test('fallback: one click by default, automatic only when asked, never chained', () => {
  const offer = { model: { provider: 'anthropic' as const, model: 'claude-haiku-4-5' }, name: 'Claude Haiku 4.5', automatic: false };
  assert.equal(fallbackDecision(RATE, offer, false), 'offer');
  assert.equal(fallbackDecision(RATE, { ...offer, automatic: true }, false), 'automatic');
  assert.equal(fallbackDecision(RATE, { ...offer, automatic: true }, true), 'offer', 'a fallback answer does not fall back again by itself');
  assert.equal(fallbackDecision(RATE, null, false), 'none');
  assert.equal(fallbackDecision({ kind: 'auth', status: 401, retryAfterSecs: null }, offer, false), 'none', 'a bad key is fixed, not routed around');
  assert.equal(fallbackDecision({ kind: 'other', status: 400, retryAfterSecs: null }, offer, false), 'none');
  assert.ok(fallbackHelps({ kind: 'quota_exhausted', status: 402, retryAfterSecs: null }));
  assert.ok(fallbackHelps({ kind: 'model_unavailable', status: 503, retryAfterSecs: null }));
});

test('subscriptions and local servers: prices, sections and run names', () => {
  const plan = model({ id: 'claude-opus-5-5', name: 'Claude Opus 5.5', provider: 'claude-sub', tier: 'subscription' });
  assert.equal(formatPrice(plan), 'Included in your plan');
  const sections = pickerSections([plan, HAIKU], EMPTY_MODEL_PREFS, '');
  assert.deepEqual(sections.map(s => s.id), ['subscription', 'paid']);
  assert.equal(sections[0].title, 'Your plans');
  assert.deepEqual(modelRefFromRun('github-copilot/gpt-5.5'), { provider: 'copilot-sub', model: 'gpt-5.5' });
  assert.deepEqual(modelRefFromRun('lm-studio/qwen3-8b'), { provider: 'lmstudio', model: 'qwen3-8b' });
  assert.deepEqual(modelRefFromRun('google-gemini-cli/gemini-3.1-pro-preview'), { provider: 'gemini-sub', model: 'gemini-3.1-pro-preview' });
});

test('picks, fallback order and base URLs are read from settings', () => {
  const prefs = readModelPrefs({
    pick: 'fast',
    providerOrder: ['claude-sub', 'acme', 'openai'],
    baseUrls: [{ provider: 'openai', url: 'https://gw.example.com/v1' }, { provider: 'acme', url: 'x' }, { provider: 'openai' }],
  });
  assert.equal(prefs.pick, 'fast');
  assert.deepEqual(prefs.providerOrder, ['claude-sub', 'openai']);
  assert.deepEqual(prefs.baseUrls, [{ provider: 'openai', url: 'https://gw.example.com/v1' }]);
  assert.equal(readModelPrefs({ pick: 'cheapest' }).pick, null);
});

test('the composer menu lists only connected picks and recent models', () => {
  const best = { provider: 'claude-sub' as const, model: 'claude-opus-5-5' };
  const fast = { provider: 'openai' as const, model: 'gpt-5.4-mini' };
  const view = {
    picks: [
      { pick: 'best' as const, label: 'Best quality', model: best, name: 'Claude Opus 5.5', fallbacks: [] },
      { pick: 'fast' as const, label: 'Fast & cheap', model: fast, name: 'GPT-5.4 mini', fallbacks: [] },
      { pick: 'free' as const, label: 'Free', model: null, name: null, fallbacks: [] },
      { pick: 'private' as const, label: 'Private (local)', model: null, name: null, fallbacks: [] },
    ],
    prefs: {
      ...EMPTY_MODEL_PREFS,
      recent: [
        best,
        { provider: 'openai' as const, model: 'gpt-5' },
        { provider: 'google' as const, model: 'gemini-2.5-pro' },
        { provider: 'openai' as const, model: 'gpt-5.5' },
      ],
    },
    models: [GPT],
    connected: ['claude-sub' as const, 'openai' as const],
  };
  const entries = chipEntries(view);
  assert.deepEqual(entries.map(e => e.kind), ['pick', 'pick', 'recent', 'recent']);
  assert.deepEqual(entries.map(e => e.model.model), ['claude-opus-5-5', 'gpt-5.4-mini', 'gpt-5', 'gpt-5.5']);
  assert.ok(!entries.some(e => e.model.provider === 'google'), 'a provider that is no longer connected is left out');
  assert.equal(chipEntries(view, 1).length, 3);
  assert.equal(pickUnavailableText('free', { localOnly: false, lmstudio: { running: false, models: [] } }), 'Connect OpenRouter for free models.');
  assert.equal(pickUnavailableText('private', { localOnly: false, lmstudio: { running: true, models: [] } }), 'Load a model in LM Studio.');
});
