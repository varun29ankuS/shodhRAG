/**
 * The backup models: the chain the app falls back down (`model_fallback_chain`,
 * `harness/model_fallback`), each model's health and live uptime in plain words, and
 * the line under an answer that a backup model wrote.
 *
 * Pure module (no React, no Tauri), unit-tested with Node (`app/tests/modelFallback.test.ts`).
 */
import type { ModelRef, PrivacyNote, ProviderErrorKind } from './modelTypes.ts';
import { readModelRef } from './modelTypes.ts';

export type ChainOrigin = 'requested' | 'chosen_fallback' | 'pick_chain' | 'automatic';

export type ChainHealth =
  | { state: 'healthy' }
  | { state: 'cooling_down'; untilMs: number; failures: number; last: ProviderErrorKind }
  | { state: 'retrying'; failures: number; last: ProviderErrorKind }
  | { state: 'probing'; failures: number; last: ProviderErrorKind };

export interface ChainAvailability {
  endpoints: number;
  serving: number;
  /** Best uptime of the last 30 minutes, in percent. */
  uptime30m: number | null;
}

export interface ChainCandidate {
  model: ModelRef;
  name: string;
  origin: ChainOrigin;
  contextLength: number | null;
  privacy: PrivacyNote;
  free: boolean;
  /** Billed per token (an API key, a paid OpenRouter model). */
  paid: boolean;
  health: ChainHealth;
  availability: ChainAvailability | null;
  /** This kind of call can use it (a subscription answers only in Ask). */
  reachable: boolean;
}

export interface ChainPlan {
  requested: ChainCandidate;
  fallbacks: ChainCandidate[];
}

export interface FallbackChain {
  /** Ask's chain (through the agent). */
  plan: ChainPlan | null;
  /** The chain of connectors and background tasks (one call from the app). */
  direct: ChainPlan | null;
  alwaysFallBack: boolean;
  allowPaidBackups: boolean;
  localOnly: boolean;
  nowMs: number;
}

const ORIGINS: readonly ChainOrigin[] = ['requested', 'chosen_fallback', 'pick_chain', 'automatic'];
const PRIVACY: readonly PrivacyNote[] = ['may_log_prompts', 'provider_terms', 'on_device'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null);

function readHealth(value: unknown): ChainHealth {
  if (!isRecord(value)) return { state: 'healthy' };
  const failures = num(value.failures) ?? 1;
  const last = (typeof value.last === 'string' ? value.last : 'other') as ProviderErrorKind;
  switch (value.state) {
    case 'cooling_down':
      return { state: 'cooling_down', untilMs: num(value.untilMs) ?? 0, failures, last };
    case 'retrying':
      return { state: 'retrying', failures, last };
    case 'probing':
      return { state: 'probing', failures, last };
    default:
      return { state: 'healthy' };
  }
}

function readAvailability(value: unknown): ChainAvailability | null {
  if (!isRecord(value)) return null;
  const endpoints = num(value.endpoints);
  const serving = num(value.serving);
  if (endpoints === null || serving === null) return null;
  return { endpoints, serving, uptime30m: num(value.uptime30m) };
}

function readCandidate(value: unknown): ChainCandidate | null {
  if (!isRecord(value)) return null;
  const model = readModelRef(value.model);
  if (!model) return null;
  return {
    model,
    name: typeof value.name === 'string' && value.name.trim() ? value.name : model.model,
    origin: ORIGINS.includes(value.origin as ChainOrigin) ? (value.origin as ChainOrigin) : 'automatic',
    contextLength: num(value.contextLength),
    privacy: PRIVACY.includes(value.privacy as PrivacyNote) ? (value.privacy as PrivacyNote) : 'provider_terms',
    free: value.free === true,
    paid: value.paid === true,
    health: readHealth(value.health),
    availability: readAvailability(value.availability),
    reachable: value.reachable !== false,
  };
}

function readPlan(value: unknown): ChainPlan | null {
  if (!isRecord(value)) return null;
  const requested = readCandidate(value.requested);
  if (!requested) return null;
  const fallbacks = Array.isArray(value.fallbacks)
    ? value.fallbacks.map(readCandidate).filter((c): c is ChainCandidate => c !== null)
    : [];
  return { requested, fallbacks };
}

/** The backend's chain view, checked; null when it is not one. */
export function readFallbackChain(value: unknown): FallbackChain | null {
  if (!isRecord(value)) return null;
  return {
    plan: readPlan(value.plan),
    direct: readPlan(value.direct),
    alwaysFallBack: value.alwaysFallBack === true,
    allowPaidBackups: value.allowPaidBackups === true,
    localOnly: value.localOnly === true,
    nowMs: num(value.nowMs) ?? Date.now(),
  };
}

/** What happens to prompts sent to a model, in a few words (shown on each chain row). */
export function privacyLabel(privacy: PrivacyNote): string {
  switch (privacy) {
    case 'on_device':
      return 'stays on this computer';
    case 'provider_terms':
      return "provider's API terms";
    case 'may_log_prompts':
      return 'may log prompts';
  }
}

/** A select option's value for `model`, and back (JSON: ids may hold any character). */
export function modelOptionValue(model: ModelRef): string {
  return JSON.stringify([model.provider, model.model]);
}

export function readModelOption(value: string): ModelRef | null {
  try {
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed) || parsed.length !== 2) return null;
    return readModelRef({ provider: parsed[0], model: parsed[1] });
  } catch {
    return null;
  }
}

/**
 * Keeps only the newest of overlapping requests: `begin` before a request, `isLatest`
 * when its answer arrives (an older answer arriving late is dropped).
 */
export class LatestOnly {
  private seq = 0;
  begin(): number {
    this.seq += 1;
    return this.seq;
  }
  isLatest(ticket: number): boolean {
    return ticket === this.seq;
  }
}

/** Where a model in the chain comes from, in words. */
export function originLabel(origin: ChainOrigin): string {
  switch (origin) {
    case 'requested':
      return 'Your model';
    case 'chosen_fallback':
      return 'Your backup';
    case 'pick_chain':
      return 'From your pick';
    case 'automatic':
      return 'Chosen by Shodh';
  }
}

export type HealthTone = 'ok' | 'warn' | 'down' | 'unknown';

export interface HealthWords {
  tone: HealthTone;
  text: string;
}

/** Plain words for why a model failed ("unavailable", "too slow to answer"). */
const FAILURE_WORDS: Partial<Record<ProviderErrorKind, string>> = {
  model_unavailable: 'unavailable',
  timeout: 'too slow to answer',
  network: 'unreachable',
};

function minutesLeft(untilMs: number, nowMs: number): number {
  return Math.max(1, Math.ceil((untilMs - nowMs) / 60_000));
}

/** A model's health and live uptime, as Settings shows it. */
export function healthWords(candidate: ChainCandidate, nowMs: number): HealthWords {
  const h = candidate.health;
  if (h.state === 'cooling_down') {
    const mins = minutesLeft(h.untilMs, nowMs);
    return { tone: 'down', text: `Was ${FAILURE_WORDS[h.last] ?? 'failing'} · skipped for ${mins} more min` };
  }
  if (h.state === 'retrying') return { tone: 'warn', text: `Was ${FAILURE_WORDS[h.last] ?? 'failing'} · the next answer tries it again` };
  if (h.state === 'probing') return { tone: 'warn', text: 'Being tried again now' };
  const a = candidate.availability;
  if (a && a.serving === 0) return { tone: 'down', text: 'No provider serves it right now' };
  if (a && a.uptime30m !== null) {
    const uptime = Math.round(a.uptime30m);
    return { tone: uptime >= 95 ? 'ok' : uptime >= 80 ? 'warn' : 'down', text: `${uptime}% uptime (last 30 min)` };
  }
  if (a) return { tone: 'ok', text: 'Served now' };
  return { tone: 'unknown', text: candidate.free ? 'Uptime not checked yet' : 'Working' };
}

/** "is unavailable", "is rate-limited": why the first model did not answer. */
const SWITCH_WORDS: Record<ProviderErrorKind, string> = {
  rate_limited: 'is rate-limited',
  quota_exhausted: 'has used up its quota',
  model_unavailable: 'is unavailable',
  auth: 'refused the key',
  timeout: 'did not answer in time',
  network: 'could not be reached',
  invalid_request: 'refused the request',
  context_too_long: 'cannot take a request this long',
  content_filtered: 'was blocked by a content filter',
  empty_response: 'answered with nothing',
  other: 'could not answer',
};

/** The model as runs name it (`provider/model`): its id after the provider. */
function shortName(run: string): string {
  const slash = run.indexOf('/');
  return slash > 0 ? run.slice(slash + 1) : run;
}

/**
 * The line under an answer a backup model wrote: "Answered by Gemma 4 31B — Nemotron
 * Super is unavailable". Names come from the fallback offer; a fallback recorded before
 * names were kept reads its model ids.
 */
export function answeredByLine(fallback: {
  from: string;
  kind: ProviderErrorKind;
  fromName?: string | null;
  toName?: string | null;
}, answeredWith: string | null): string {
  const to = fallback.toName?.trim() || (answeredWith ? shortName(answeredWith) : 'a backup model');
  const from = fallback.fromName?.trim() || shortName(fallback.from);
  return `Answered by ${to} — ${from} ${SWITCH_WORDS[fallback.kind]}`;
}

/** Side runs (summaries, refinements, diagram fixes) fall back by themselves at most this often. */
export const MAX_SIDE_FALLBACKS = 2;
