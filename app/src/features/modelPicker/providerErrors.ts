/**
 * What an answer that failed at the model provider offers: the message, a
 * retry (automatic after a wait for rate limits, at most once per question),
 * another model for this answer, or fixing the key. Nothing switches models
 * silently: a fallback is one click, or automatic only when the person turned
 * on "always fall back", and it is announced either way.
 *
 * Pure module (no React), unit-tested with Node (`app/tests/modelPicker.test.ts`).
 */
import type { FallbackOffer, ProviderError, ProviderErrorKind, ProviderId } from './modelTypes.ts';
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

/** Where to change the model, as the hints say it. */
const CHOOSE_ANOTHER = 'choose another model in Settings → Model';

/** OpenRouter's privacy settings leave no provider for the model (common for :free models). */
const OPENROUTER_PRIVACY = /no endpoints available matching your guardrail restrictions and data policy/i;

/** OpenRouter's daily limit of free-model requests (per account, across all free models). */
const OPENROUTER_FREE_DAILY = /free-models-per-day/i;

/** The reset time as the clock on this computer shows it (24-hour). */
export function resetClock(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** Shown as text; never opened without an explicit click. */
export const OPENROUTER_PRIVACY_SETTINGS = 'openrouter.ai/settings/privacy';

/**
 * The card's text. `model` is the model's display name; `message` is the
 * provider's raw text, when known (some failures need their own hint).
 */
export function providerErrorCopy(error: ProviderError, model: string, provider: ProviderId | null, message: string | null = null): ErrorCopy {
  const who = provider ? PROVIDER_LABELS[provider] : 'the provider';
  // "OpenRouter via Nvidia · model": who failed, in the titles below.
  const where = `${provider ? PROVIDER_LABELS[provider] : 'provider'}${error.upstream ? ` via ${error.upstream}` : ''} · ${model}`;
  switch (error.kind) {
    case 'rate_limited':
      return {
        title: `${model} is rate-limited`,
        hint: 'Too many requests for now. Retry in a moment, or answer this one with another model.',
      };
    case 'quota_exhausted':
      if (message && OPENROUTER_FREE_DAILY.test(message)) {
        return {
          title: error.resetsAtMs
            ? `Today's free OpenRouter quota is used up — it resets at ${resetClock(error.resetsAtMs)}`
            : `Today's free OpenRouter quota is used up`,
          hint: 'Another free model shares the same quota. Use a model from another provider for this answer, or connect one in Settings → Model.',
        };
      }
      return {
        title: `${model}: the ${who} credits or daily limit are used up`,
        hint: 'Waiting will not help today. Use another model, or add credits with the provider.',
      };
    case 'model_unavailable':
      if (message && OPENROUTER_PRIVACY.test(message)) {
        return {
          title: `${model} is unavailable with your OpenRouter privacy settings`,
          hint: `OpenRouter's privacy settings leave no provider for this model. Change them at ${OPENROUTER_PRIVACY_SETTINGS}, or choose another model in Settings → Model.`,
        };
      }
      return {
        title: `${model} is unavailable right now`,
        hint: 'The provider is overloaded or no longer serves this model. Retry, or use another model.',
      };
    case 'auth':
      return {
        title: `Your ${who} key was rejected`,
        hint: 'Use Fix key to paste a new one in Settings → Model. Keys are kept in the system keychain.',
      };
    case 'timeout':
      return {
        title: `The model's provider did not answer in time (${where})`,
        hint: `Try again, or ${CHOOSE_ANOTHER}.`,
      };
    case 'network':
      return {
        title: `The connection to the model's provider broke (${where})`,
        hint: `Try again, or ${CHOOSE_ANOTHER}.`,
      };
    case 'empty_response':
      return {
        title: `The model sent back an empty answer (${where})`,
        hint: `Try again, or ${CHOOSE_ANOTHER}.`,
      };
    case 'invalid_request':
      return {
        title: `The model's provider refused the request (${where})`,
        hint: `Trying again will not help; ${CHOOSE_ANOTHER}.`,
      };
    case 'context_too_long':
      return {
        title: `The question and its sources are too long for the model (${where})`,
        hint: `Ask about fewer documents, or ${CHOOSE_ANOTHER} with a longer context.`,
      };
    case 'content_filtered':
      return {
        title: `The provider's content filter stopped this answer (${where})`,
        hint: `Rephrase the question, or ${CHOOSE_ANOTHER}.`,
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

/** Kinds where the same request to the same model fails the same way again. */
const NO_RETRY: readonly ProviderErrorKind[] = ['auth', 'quota_exhausted', 'invalid_request', 'context_too_long', 'content_filtered'];

/** Whether retrying the same model makes sense at all. */
export function canRetry(error: ProviderError): boolean {
  return !NO_RETRY.includes(error.kind);
}

/** Whether another model could answer instead. */
export function fallbackHelps(error: ProviderError): boolean {
  // Timeouts and broken connections were "unavailable" (504, connection reset) before they had
  // their own kinds; they keep the fallback offer.
  return (
    error.kind === 'rate_limited' ||
    error.kind === 'quota_exhausted' ||
    error.kind === 'model_unavailable' ||
    error.kind === 'timeout' ||
    error.kind === 'network'
  );
}

export type FallbackDecision =
  /** Retry with the fallback now ("always fall back" is on); the answer says so. */
  | 'automatic'
  /** Show "Use <fallback> for this answer". */
  | 'offer'
  | 'none';

/**
 * What to do with a fallback offer. `fellBack`: this answer already came
 * from a fallback (never chain fallbacks automatically). `streamed`: part of
 * the answer had already reached the user, so nothing switches by itself;
 * the other model is offered as a click.
 */
export function fallbackDecision(error: ProviderError, offer: FallbackOffer | null, fellBack: boolean, streamed = false): FallbackDecision {
  if (!offer || !fallbackHelps(error)) return 'none';
  return offer.automatic && !fellBack && !streamed ? 'automatic' : 'offer';
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
