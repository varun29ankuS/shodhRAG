/**
 * Side runs (a side question's answer, a summary, a visual's refinement, a diagram fix)
 * fall back to another model by themselves: nobody is asked, as for the app's
 * connectors and background work. They do so only when another model can help (the
 * model was unavailable, rate-limited, too slow or unreachable), only while none of the
 * answer reached the user, and at most MAX_SIDE_FALLBACKS times per run.
 *
 * Pure module, unit-tested with Node (`app/tests/modelFallback.test.ts`).
 */
import type { TranscriptState } from '../agent/reducer.ts';
import { answerText } from '../agent/reducer.ts';
import { MAX_SIDE_FALLBACKS } from '../modelPicker/fallbackChain.ts';
import type { ModelRef, ProviderErrorKind } from '../modelPicker/modelTypes.ts';
import { modelRefFromRun } from '../modelPicker/modelTypes.ts';
import { fallbackHelps } from '../modelPicker/providerErrors.ts';

/** What a failed side run falls back from. */
export interface SideFallback {
  /** The model that failed. */
  failed: ModelRef;
  kind: ProviderErrorKind;
  /** The provider's message. */
  detail: string | null;
}

/**
 * The failure a finished side run falls back from, or null when it keeps its result:
 * it did not fail at the provider, another model would not help, part of the answer was
 * already shown, its model is unknown, or it fell back `fallbacksSoFar` times already.
 */
export function sideFallback(transcript: TranscriptState, fallbacksSoFar: number): SideFallback | null {
  if (transcript.status !== 'error' || fallbacksSoFar >= MAX_SIDE_FALLBACKS) return null;
  const error = transcript.providerError;
  if (!error || !fallbackHelps(error)) return null;
  if (answerText(transcript).length > 0) return null;
  const failed = modelRefFromRun(transcript.model);
  if (!failed) return null;
  return { failed, kind: error.kind, detail: transcript.error ?? null };
}
