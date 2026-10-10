/**
 * The model picker's data, as the backend sends it
 * (`model_picker_commands.rs`, `harness/model_catalog.rs`,
 * `harness/model_choice.rs`, `harness/provider_error.rs`), and readers that
 * check it before use.
 */

export type ProviderId =
  | 'openrouter'
  | 'anthropic'
  | 'openai'
  | 'google'
  | 'grok'
  | 'ollama'
  | 'claude-sub'
  | 'chatgpt-sub'
  | 'copilot-sub'
  | 'gemini-sub'
  | 'lmstudio';

export const PROVIDERS: readonly ProviderId[] = [
  'openrouter',
  'anthropic',
  'openai',
  'google',
  'grok',
  'ollama',
  'claude-sub',
  'chatgpt-sub',
  'copilot-sub',
  'gemini-sub',
  'lmstudio',
];

export const PROVIDER_LABELS: Record<ProviderId, string> = {
  openrouter: 'OpenRouter',
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  google: 'Google',
  grok: 'xAI',
  ollama: 'Ollama',
  'claude-sub': 'Claude (Pro/Max)',
  'chatgpt-sub': 'ChatGPT (Plus/Pro)',
  'copilot-sub': 'GitHub Copilot',
  'gemini-sub': 'Google Gemini',
  lmstudio: 'LM Studio',
};

/** Providers connected with an API key. */
export const KEY_PROVIDERS: readonly ProviderId[] = ['openrouter', 'anthropic', 'openai', 'google', 'grok'];

export interface ModelRef {
  provider: ProviderId;
  model: string;
}

export type ToolSupport = 'yes' | 'no' | 'unknown';
export type ModelTier = 'free' | 'paid' | 'local' | 'subscription';
export type PrivacyNote = 'may_log_prompts' | 'provider_terms' | 'on_device';

export interface CatalogModel {
  provider: ProviderId;
  id: string;
  name: string;
  contextLength: number | null;
  /** USD per million input tokens; null when unknown. */
  promptPerMillion: number | null;
  /** USD per million output tokens; null when unknown. */
  completionPerMillion: number | null;
  tools: ToolSupport;
  tier: ModelTier;
  privacy: PrivacyNote;
  stealth: boolean;
}

export type ModelSource = 'session' | 'environment' | 'settings';

export interface ActiveModel {
  model: ModelRef;
  source: ModelSource;
}

export type PickId = 'best' | 'fast' | 'free' | 'private';

export const PICKS: readonly PickId[] = ['best', 'fast', 'free', 'private'];

export interface BaseUrl {
  provider: ProviderId;
  url: string;
}

export interface ModelPrefs {
  chosen: ModelRef | null;
  fallback: ModelRef | null;
  alwaysFallBack: boolean;
  stealthAccepted: string[];
  recent: ModelRef[];
  favourites: ModelRef[];
  /** The pick the chosen model came from; null for a model chosen by id. */
  pick: PickId | null;
  providerOrder: ProviderId[];
  baseUrls: BaseUrl[];
}

export const EMPTY_MODEL_PREFS: ModelPrefs = {
  chosen: null,
  fallback: null,
  alwaysFallBack: false,
  stealthAccepted: [],
  recent: [],
  favourites: [],
  pick: null,
  providerOrder: [],
  baseUrls: [],
};

/** One of the four picks, resolved from the connected providers. */
export interface PickOption {
  pick: PickId;
  label: string;
  model: ModelRef | null;
  name: string | null;
  fallbacks: ModelRef[];
}

export type SignInState = 'connected' | 'expired' | 'signed_out';

export interface SubscriptionStatus {
  provider: ProviderId;
  label: string;
  state: SignInState;
  accounts: string[];
  note: string | null;
}

export interface KeyStatus {
  provider: ProviderId;
  label: string;
  /** `vault` (OS credential store) or `environment` (cannot be removed here). */
  source: 'vault' | 'environment';
}

export interface LocalServer {
  running: boolean;
  models: string[];
}

export type CatalogStatus = 'fresh' | 'cached' | 'unavailable' | 'local_only';

export interface PickerView {
  active: ActiveModel | null;
  environment: ModelRef | null;
  models: CatalogModel[];
  catalogStatus: CatalogStatus;
  catalogFetchedAtMs: number | null;
  catalogError: string | null;
  ollamaRunning: boolean;
  llamaCppFile: string | null;
  keyed: ProviderId[];
  connected: ProviderId[];
  localOnly: boolean;
  envAllowsStealth: boolean;
  prefs: ModelPrefs;
  picks: PickOption[];
  subscriptions: SubscriptionStatus[];
  keys: KeyStatus[];
  lmstudio: LocalServer;
  ollama: LocalServer;
  ollamaNote: string;
  providerOrder: ProviderId[];
}

export type ProviderErrorKind = 'rate_limited' | 'quota_exhausted' | 'model_unavailable' | 'auth' | 'other';

export interface ProviderError {
  kind: ProviderErrorKind;
  status: number | null;
  retryAfterSecs: number | null;
}

export interface FallbackOffer {
  model: ModelRef;
  name: string;
  /** "Always fall back" is on: retry without asking. */
  automatic: boolean;
}

/** One answer run with another model (sent with `agent_start`). */
export interface ModelOverride {
  model: ModelRef;
  automatic: boolean;
  failure: ProviderErrorKind | null;
}

export type PickerErrorCode =
  | 'invalid'
  | 'local_only'
  | 'missing_key'
  | 'not_connected'
  | 'stealth_confirmation'
  | 'environment_set'
  | 'failed';

export interface PickerError {
  code: PickerErrorCode;
  message: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function isProviderId(value: unknown): value is ProviderId {
  return typeof value === 'string' && (PROVIDERS as readonly string[]).includes(value);
}

export function readModelRef(value: unknown): ModelRef | null {
  if (!isRecord(value) || !isProviderId(value.provider) || typeof value.model !== 'string' || !value.model.trim()) return null;
  return { provider: value.provider, model: value.model };
}

function refs(value: unknown): ModelRef[] {
  return Array.isArray(value) ? value.map(readModelRef).filter((r): r is ModelRef => r !== null) : [];
}

export function isPickId(value: unknown): value is PickId {
  return typeof value === 'string' && (PICKS as readonly string[]).includes(value);
}

function baseUrls(value: unknown): BaseUrl[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter(isRecord)
    .filter(v => isProviderId(v.provider) && typeof v.url === 'string')
    .map(v => ({ provider: v.provider as ProviderId, url: v.url as string }));
}

/** The `models` section of the settings (from `app-settings-changed`); defaults for anything missing. */
export function readModelPrefs(value: unknown): ModelPrefs {
  if (!isRecord(value)) return EMPTY_MODEL_PREFS;
  return {
    chosen: readModelRef(value.chosen),
    fallback: readModelRef(value.fallback),
    alwaysFallBack: value.alwaysFallBack === true,
    stealthAccepted: Array.isArray(value.stealthAccepted) ? value.stealthAccepted.filter((s): s is string => typeof s === 'string') : [],
    recent: refs(value.recent),
    favourites: refs(value.favourites),
    pick: isPickId(value.pick) ? value.pick : null,
    providerOrder: Array.isArray(value.providerOrder) ? value.providerOrder.filter(isProviderId) : [],
    baseUrls: baseUrls(value.baseUrls),
  };
}

const ERROR_KINDS: readonly ProviderErrorKind[] = ['rate_limited', 'quota_exhausted', 'model_unavailable', 'auth', 'other'];

/** A `providerError` from a `run_finished` event (or a stored transcript); null when absent or malformed. */
export function readProviderError(value: unknown): ProviderError | null {
  if (!isRecord(value) || !(ERROR_KINDS as readonly unknown[]).includes(value.kind)) return null;
  const whole = (x: unknown) => (typeof x === 'number' && Number.isInteger(x) && x >= 0 ? x : null);
  return { kind: value.kind as ProviderErrorKind, status: whole(value.status), retryAfterSecs: whole(value.retryAfterSecs) };
}

const PICKER_CODES: readonly PickerErrorCode[] = [
  'invalid',
  'local_only',
  'missing_key',
  'not_connected',
  'stealth_confirmation',
  'environment_set',
  'failed',
];

/** Normalise anything a picker command rejected with. */
export function toPickerError(error: unknown): PickerError {
  if (isRecord(error) && typeof error.message === 'string') {
    const code = (PICKER_CODES as readonly unknown[]).includes(error.code) ? (error.code as PickerErrorCode) : 'failed';
    return { code, message: error.message };
  }
  if (error instanceof Error) return { code: 'failed', message: error.message };
  return { code: 'failed', message: typeof error === 'string' ? error : 'The model settings could not be changed.' };
}

/** The runtime's provider names in a run's model (`openrouter/vendor/model`). */
const RUNTIME_PROVIDERS: Record<string, ProviderId> = {
  openrouter: 'openrouter',
  anthropic: 'anthropic',
  openai: 'openai',
  google: 'google',
  xai: 'grok',
  ollama: 'ollama',
  'openai-codex': 'chatgpt-sub',
  'github-copilot': 'copilot-sub',
  'google-gemini-cli': 'gemini-sub',
  'lm-studio': 'lmstudio',
};

/** The model a run used, from its `run_started` model (`provider/model-id`). */
export function modelRefFromRun(model: string | null): ModelRef | null {
  if (!model) return null;
  const slash = model.indexOf('/');
  if (slash <= 0 || slash === model.length - 1) return null;
  const provider = RUNTIME_PROVIDERS[model.slice(0, slash).toLowerCase()];
  return provider ? { provider, model: model.slice(slash + 1) } : null;
}

export function sameModel(a: ModelRef | null | undefined, b: ModelRef | null | undefined): boolean {
  return !!a && !!b && a.provider === b.provider && a.model === b.model;
}
