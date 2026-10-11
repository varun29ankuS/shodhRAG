import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, KeyRound, Repeat, RotateCcw, SlidersHorizontal } from 'lucide-react';
import { cn } from '../../lib/utils';
import { OPEN_MODEL_PICKER_EVENT, modelApi } from './modelApi';
import type { FallbackOffer, ModelOverride, ProviderError } from './modelTypes';
import { modelRefFromRun } from './modelTypes';
import {
  AutoRetryLedger,
  autoRetrySeconds,
  canRetry,
  fallbackDecision,
  fallbackHelps,
  providerErrorCopy,
} from './providerErrors';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

const BUTTON = cn(
  'inline-flex items-center gap-1.5 h-8 px-3 rounded-lg bg-shodh-raised-2 text-[12.5px] text-shodh-text hover:bg-shodh-pressed transition-colors duration-micro',
  FOCUS_RING,
);

/** Automatic retries per question, for this app session (bounds the auto-retry loop). */
const autoRetries = new AutoRetryLedger();

/** What the newest answer's card may do; absent on older answers, the dock and side answers. */
export interface ProviderErrorActions {
  /** Identifies the question across retries (bounds automatic retries). */
  questionKey: string;
  /** Retry the question; `override` runs it with a fallback model. */
  onRetry: (override: ModelOverride | null, from: string | null) => void;
}

interface ProviderErrorCardProps {
  error: ProviderError;
  /** The run's model (`provider/model`). */
  runModel: string | null;
  /** The raw message, kept behind "Details". */
  message: string | null;
  /** This answer was itself a fallback (fallbacks never chain automatically). */
  fellBack: boolean;
  actions: ProviderErrorActions | null;
  onOpenSettings: () => void;
  compact?: boolean;
}

/**
 * An answer the model provider refused: what happened in one line, what to
 * do, and the actions: Retry (automatic after the provider's wait for rate
 * limits, once per question), "Use <fallback> for this answer" (automatic
 * only with "always fall back" on, and announced), Change model, or update
 * a rejected key.
 */
export function ProviderErrorCard({ error, runModel, message, fellBack, actions, onOpenSettings, compact = false }: ProviderErrorCardProps) {
  const failed = useMemo(() => modelRefFromRun(runModel), [runModel]);
  const modelLabel = failed?.model ?? runModel ?? 'The model';
  const copy = providerErrorCopy(error, modelLabel, failed?.provider ?? null);
  const [offer, setOffer] = useState<FallbackOffer | null>(null);
  const [countdown, setCountdown] = useState<number | null>(null);
  const firedRef = useRef(false);

  // The fallback the app would use (only where the card can act on it).
  useEffect(() => {
    if (!actions || !failed || !fallbackHelps(error)) return;
    let alive = true;
    modelApi
      .fallbackOffer(failed)
      .then(next => {
        if (alive) setOffer(next);
      })
      .catch(e => console.error('Fallback model lookup failed:', e));
    return () => {
      alive = false;
    };
  }, [actions, failed, error]);

  const decision = fallbackDecision(error, offer, fellBack);
  const override = (automatic: boolean): ModelOverride | null =>
    offer ? { model: offer.model, automatic, failure: error.kind } : null;

  // "Always fall back": retry with the fallback at once (once; never chained).
  useEffect(() => {
    if (!actions || !offer || decision !== 'automatic' || firedRef.current) return;
    firedRef.current = true;
    setCountdown(null);
    actions.onRetry({ model: offer.model, automatic: true, failure: error.kind }, runModel);
  }, [actions, offer, decision, error.kind, runModel]);

  // Rate limits: count down the provider's wait, then retry the same model once.
  const autoSecs = actions && decision !== 'automatic' ? autoRetrySeconds(error, autoRetries.count(actions.questionKey)) : null;
  useEffect(() => {
    if (autoSecs === null) return;
    setCountdown(autoSecs);
  }, [autoSecs]);
  // Read at the moment the countdown ends, so a re-render never restarts the timer.
  const actionsRef = useRef(actions);
  actionsRef.current = actions;
  useEffect(() => {
    if (countdown === null) return;
    if (countdown <= 0) {
      const current = actionsRef.current;
      if (!current || firedRef.current) return;
      firedRef.current = true;
      autoRetries.record(current.questionKey);
      current.onRetry(null, null);
      return;
    }
    const timer = window.setTimeout(() => setCountdown(c => (c === null ? null : c - 1)), 1_000);
    return () => window.clearTimeout(timer);
  }, [countdown]);

  const retryNow = () => {
    if (!actions || firedRef.current) return;
    firedRef.current = true;
    setCountdown(null);
    actions.onRetry(null, null);
  };
  const useFallback = () => {
    const next = override(false);
    if (!actions || !next || firedRef.current) return;
    firedRef.current = true;
    setCountdown(null);
    actions.onRetry(next, runModel);
  };

  return (
    <div role="alert" className={cn('flex flex-col gap-2 rounded-xl border border-shodh-border bg-shodh-surface', compact ? 'p-3' : 'p-3.5')}>
      <p className="m-0 flex items-start gap-2 text-[13px] font-semibold text-shodh-text">
        <AlertTriangle className="w-4 h-4 mt-px shrink-0 text-shodh-warning" aria-hidden="true" />
        <span className="min-w-0 break-words">{copy.title}</span>
      </p>
      <p className="m-0 text-[12.5px] leading-relaxed text-shodh-text-secondary">
        {decision === 'automatic' && offer && actions
          ? `Always fall back is on: answering with ${offer.name} instead.`
          : copy.hint}
      </p>
      <div className="flex flex-wrap items-center gap-2">
        {error.kind === 'auth' ? (
          <button type="button" onClick={onOpenSettings} className={BUTTON}>
            <KeyRound className="w-3.5 h-3.5" aria-hidden="true" />
            Update the key
          </button>
        ) : (
          actions && (
            <>
              {canRetry(error) && (
                <button type="button" onClick={retryNow} className={BUTTON}>
                  <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
                  {countdown !== null && countdown > 0 ? `Retry (auto in ${countdown}s)` : 'Retry'}
                </button>
              )}
              {decision === 'offer' && offer && (
                <button type="button" onClick={useFallback} className={BUTTON}>
                  <Repeat className="w-3.5 h-3.5" aria-hidden="true" />
                  {`Use ${offer.name} for this answer`}
                </button>
              )}
              {countdown !== null && countdown > 0 && (
                <button
                  type="button"
                  onClick={() => setCountdown(null)}
                  className={cn('h-8 px-2 rounded-lg text-[12px] text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised', FOCUS_RING)}
                >
                  Don&apos;t retry automatically
                </button>
              )}
            </>
          )
        )}
        <button
          type="button"
          onClick={() => (actions ? window.dispatchEvent(new CustomEvent(OPEN_MODEL_PICKER_EVENT)) : onOpenSettings())}
          className={BUTTON}
        >
          <SlidersHorizontal className="w-3.5 h-3.5" aria-hidden="true" />
          Change model
        </button>
      </div>
      {message && (
        <details className="text-[12px] text-shodh-text-muted">
          <summary className={cn('cursor-pointer w-fit rounded-sm hover:text-shodh-text-secondary', FOCUS_RING)}>Details from the provider</summary>
          <p className="m-0 mt-1 break-words font-mono text-[11.5px]">{message}</p>
        </details>
      )}
    </div>
  );
}
