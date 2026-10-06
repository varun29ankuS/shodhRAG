/**
 * The model picker's data, as the backend sends it
 * (`model_picker_commands.rs`, `harness/model_catalog.rs`,
 * `harness/model_choice.rs`, `harness/provider_error.rs`), and readers that
 * check it before use.
 */

export type ProviderId = 'openrouter' | 'anthropic' | 'openai' | 'google' | 'grok' | 'ollama';

export const PROVIDERS: readonly ProviderId[] = ['openrouter', 'anthropic', 'openai', 'google', 'grok', 'ollama'];

export const PROVIDER_LABELS: Record<ProviderId, string> = {
  openrouter: 'OpenRouter',
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  google: 'Google',
  grok: 'xAI',
  ollama: 'Ollama',
};

export interface ModelRef {
  provider: ProviderId;
  model: string;
}

export type ToolSupport = 'yes' | 'no' | 'unknown';
export type ModelTier = 'free' | 'paid' | 'local';
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

export interface ModelPrefs {
  chosen: ModelRef | null;
  fallback: ModelRef | null;
  alwaysFallBack: boolean;
  stealthAccepted: string[];
  recent: ModelRef[];
  favourites: ModelRef[];
}

export const EMPTY_MODEL_PREFS: ModelPrefs = {
  chosen: null,
  fallback: null,
  alwaysFallBack: false,
  stealthAccepted: [],
  recent: [],
  favourites: [],
};

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
  localOnly: boolean;
  envAllowsStealth: boolean;
  prefs: ModelPrefs;
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

export type PickerErrorCode = 'invalid' | 'local_only' | 'missing_key' | 'stealth_confirmation' | 'environment_set' | 'failed';

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
  };
}

const ERROR_KINDS: readonly ProviderErrorKind[] = ['rate_limited', 'quota_exhausted', 'model_unavailable', 'auth', 'other'];

/** A `providerError` from a `run_finished` event (or a stored transcript); null when absent or malformed. */
export function readProviderError(value: unknown): ProviderError | null {
  if (!isRecord(value) || !(ERROR_KINDS as readonly unknown[]).includes(value.kind)) return null;
  const whole = (x: unknown) => (typeof x === 'number' && Number.isInteger(x) && x >= 0 ? x : null);
  return { kind: value.kind as ProviderErrorKind, status: whole(value.status), retryAfterSecs: whole(value.retryAfterSecs) };
}

const PICKER_CODES: readonly PickerErrorCode[] = ['invalid', 'local_only', 'missing_key', 'stealth_confirmation', 'environment_set', 'failed'];

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
