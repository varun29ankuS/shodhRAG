/**
 * What an answer that failed at the model provider offers: the message, a
 * retry (automatic after a wait for rate limits, at most once per question),
 * another model for this answer, or fixing the key. Nothing switches models
 * silently: a fallback is one click, or automatic only when the person turned
 * on "always fall back", and it is announced either way.
 *
 * Pure module (no React), unit-tested with Node (`app/tests/modelPicker.test.ts`).
 */
import type { FallbackOffer, ProviderError, ProviderId } from './modelTypes.ts';
import { PROVIDER_LABELS } from './modelTypes.ts';

/** Longest wait the card counts down before retrying by itself. */
export const MAX_AUTO_RETRY_SECS = 120;
/** Wait before an automatic retry when the provider gave no hint. */
export const DEFAULT_RETRY_SECS = 20;
/** Automatic retries per question (further retries are a click). */
export const MAX_AUTO_RETRIES = 1;

export interface ErrorCopy {
  title: string;
  /** One line on what to do. */
  hint: string;
}

/** The card's text. `model` is the model's display name. */
export function providerErrorCopy(error: ProviderError, model: string, provider: ProviderId | null): ErrorCopy {
  const who = provider ? PROVIDER_LABELS[provider] : 'the provider';
  switch (error.kind) {
    case 'rate_limited':
      return {
        title: `${model} is rate-limited`,
        hint: 'Too many requests for now. Retry in a moment, or answer this one with another model.',
      };
    case 'quota_exhausted':
      return {
        title: `${model}: the ${who} credits or daily limit are used up`,
        hint: 'Waiting will not help today. Use another model, or add credits with the provider.',
      };
    case 'model_unavailable':
      return {
        title: `${model} is unavailable right now`,
        hint: 'The provider is overloaded or no longer serves this model. Retry, or use another model.',
      };
    case 'auth':
      return {
        title: `Your ${who} key was rejected`,
        hint: 'Update the key in Settings → Model. It is kept in the system keychain.',
      };
    case 'other':
      return {
        title: `${model} could not answer`,
        hint: 'Retry, or change the model.',
      };
  }
}

/**
 * Seconds before the card retries by itself, or null for no automatic retry:
 * only rate limits wait and retry, only with a wait of at most
 * MAX_AUTO_RETRY_SECS, and only MAX_AUTO_RETRIES times per question.
 */
export function autoRetrySeconds(error: ProviderError, autoRetriesSoFar: number): number | null {
  if (error.kind !== 'rate_limited' || autoRetriesSoFar >= MAX_AUTO_RETRIES) return null;
  const wait = error.retryAfterSecs ?? DEFAULT_RETRY_SECS;
  return wait > MAX_AUTO_RETRY_SECS ? null : Math.max(1, wait);
}

/** Whether retrying the same model makes sense at all. */
export function canRetry(error: ProviderError): boolean {
  return error.kind !== 'auth' && error.kind !== 'quota_exhausted';
}

/** Whether another model could answer instead. */
export function fallbackHelps(error: ProviderError): boolean {
  return error.kind === 'rate_limited' || error.kind === 'quota_exhausted' || error.kind === 'model_unavailable';
}

export type FallbackDecision =
  /** Retry with the fallback now ("always fall back" is on); the answer says so. */
  | 'automatic'
  /** Show "Use <fallback> for this answer". */
  | 'offer'
  | 'none';

/**
 * What to do with a fallback offer. `fellBack`: this answer already came
 * from a fallback (never chain fallbacks automatically).
 */
export function fallbackDecision(error: ProviderError, offer: FallbackOffer | null, fellBack: boolean): FallbackDecision {
  if (!offer || !fallbackHelps(error)) return 'none';
  return offer.automatic && !fellBack ? 'automatic' : 'offer';
}

/** Counts automatic retries per question for this app session. */
export class AutoRetryLedger {
  private readonly counts = new Map<string, number>();
  private readonly limit: number;

  constructor(limit = 500) {
    this.limit = limit;
  }

  count(questionId: string): number {
    return this.counts.get(questionId) ?? 0;
  }

  /** Record one automatic retry of `questionId`. */
  record(questionId: string): void {
    this.counts.set(questionId, this.count(questionId) + 1);
    while (this.counts.size > this.limit) {
      const oldest = this.counts.keys().next().value;
      if (oldest === undefined) break;
      this.counts.delete(oldest);
    }
  }
}
