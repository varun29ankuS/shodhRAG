import React from 'react';
import { CheckCircle2, Download, Loader2, RotateCcw, TriangleAlert, WifiOff, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useSearchModels } from './SearchModelsContext';
import { useRuntimeSetup } from './RuntimeSetupContext';
import { useReadiness } from './useReadiness';
import { SEARCH_MODELS_SOURCE, setupDisclosure, setupHeading, waitingLine } from './readiness';
import type { PartView } from './readiness';
import { checksumRow, runtimeVerifiedSentence, verifiedSentence } from './verification';
import { TechnicalDetails } from './TechnicalDetails';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const PRIMARY_BUTTON =
  'inline-flex items-center gap-2 h-8 px-3.5 rounded-lg bg-shodh-accent text-shodh-on-accent text-small font-semibold hover:bg-shodh-accent-hover disabled:opacity-60 disabled:cursor-not-allowed transition-colors duration-micro';

const QUIET_BUTTON =
  'inline-flex items-center gap-2 h-8 px-3 rounded-lg text-small text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro';

interface SetupCardProps {
  compact?: boolean;
  className?: string;
  /** A question is waiting for setup: say so, and offer not to send it. */
  waiting?: boolean;
  onCancelWaiting?: () => void;
}

/**
 * "Getting Shodh ready": the one-time downloads a new install needs before it
 * answers, the search models and the answer engine, in one card with one
 * button and one progress view. Says what is downloaded, from where and how
 * much before it starts; what verification means, with the checksums only
 * behind "Technical details". Renders nothing once everything is ready, unless
 * a download just finished (then it confirms it until dismissed).
 */
export function SetupCard({ compact = false, className, waiting = false, onCancelWaiting }: SetupCardProps) {
  const state = useReadiness();
  const search = useSearchModels();
  const runtime = useRuntimeSetup();

  const shell = cn(
    'ask-rise rounded-xl border border-shodh-border bg-shodh-surface flex flex-col gap-3',
    compact ? 'p-3' : 'p-4',
    className,
  );

  // Neither status could be read: say so and offer a retry rather than nothing.
  if (state.phase === 'checking' && !search.status && search.statusError && !runtime.status && runtime.statusError) {
    return (
      <section aria-label="Shodh setup" className={shell}>
        <div className="flex items-start gap-2.5">
          <TriangleAlert className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-warning" aria-hidden="true" />
          <div className="min-w-0 flex flex-col gap-1">
            <p className="text-body font-semibold text-shodh-text">Could not check whether Shodh is set up</p>
            <p role="alert" className="text-small text-shodh-error break-words">{search.statusError}</p>
          </div>
        </div>
        <div>
          <button type="button" onClick={state.refresh} className={cn(PRIMARY_BUTTON, FOCUS_RING)}>
            <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
            Check again
          </button>
        </div>
      </section>
    );
  }

  if (state.phase === 'ready' || state.phase === 'checking') {
    const searchDone = search.installed && !search.installing ? search.installed : null;
    const runtimeDone = runtime.justInstalled;
    if (state.phase !== 'ready' || (!searchDone && !runtimeDone)) return null;
    const rows = [
      ...(searchDone ? searchDone.artifacts.map(a => checksumRow(a.name, a.sha256)) : []),
      ...(runtimeDone ? [checksumRow(`Answer engine (omp ${runtimeDone.version})`, runtimeDone.sha256)] : []),
    ];
    // Each part says what its own check proved (the answer engine's differs off Windows).
    const checks = [
      searchDone ? verifiedSentence(SEARCH_MODELS_SOURCE, true) : null,
      runtimeDone ? runtimeVerifiedSentence(state.runtimeSource, runtime.status?.pinned ?? false) : null,
    ].filter(Boolean).join(' ');
    return (
      <section aria-label="Shodh setup" className={shell}>
        <div className="flex items-start gap-2.5">
          <CheckCircle2 className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-success" aria-hidden="true" />
          <div className="min-w-0 flex-1 flex flex-col gap-1">
            <p className="text-body font-semibold text-shodh-text">Shodh is ready</p>
            <p className="text-small leading-relaxed text-shodh-text-muted">
              {`${searchDone && runtimeDone ? 'Search models and answer engine downloaded.' : searchDone ? 'Search models downloaded.' : 'Answer engine downloaded.'} ${checks}`}
            </p>
          </div>
          <button
            type="button"
            onClick={() => {
              search.dismissInstalled();
              runtime.dismissInstalled();
            }}
            aria-label="Dismiss"
            className={cn('shrink-0 p-1 rounded-md text-shodh-text-muted hover:text-shodh-text', FOCUS_RING)}
          >
            <X className="w-3.5 h-3.5" aria-hidden="true" />
          </button>
        </div>
        <TechnicalDetails rows={rows} />
      </section>
    );
  }

  const installing = state.phase === 'installing';
  const failed = state.phase === 'failed';
  const errors = state.parts.filter(p => p.error).map(p => p.error as string);

  return (
    <section aria-label="Shodh setup" className={shell}>
      <div className="flex items-start gap-2.5">
        {failed ? (
          state.offline
            ? <WifiOff className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-warning" aria-hidden="true" />
            : <TriangleAlert className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-warning" aria-hidden="true" />
        ) : (
          <Download className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-accent" aria-hidden="true" />
        )}
        <div className="min-w-0 flex flex-col gap-1">
          <p className="text-body font-semibold text-shodh-text">{setupHeading(state)}</p>
          {state.phase === 'needed' && (
            <p className="text-small leading-relaxed text-shodh-text-muted">{setupDisclosure(state, state.runtimeSource)}</p>
          )}
          {waiting && (
            <p className="text-small leading-relaxed text-shodh-text" aria-live="polite">{waitingLine(state)}</p>
          )}
        </div>
      </div>

      {(installing || failed) && (
        <ul className="flex flex-col gap-2" aria-label="Setup progress">
          {state.parts.map(part => <PartRow key={part.id} part={part} />)}
        </ul>
      )}

      {failed && errors.length > 0 && (
        <div role="alert" className="flex flex-col gap-1">
          {errors.map(error => (
            <p key={error} className="text-small text-shodh-error break-words">{error}</p>
          ))}
        </div>
      )}

      <div className="flex items-center gap-2 flex-wrap">
        {installing ? (
          <button type="button" disabled className={cn(PRIMARY_BUTTON, FOCUS_RING)}>
            <Loader2 className="w-3.5 h-3.5 agent-spin" aria-hidden="true" />
            Setting up…
          </button>
        ) : (
          <button type="button" onClick={state.startSetup} className={cn(PRIMARY_BUTTON, FOCUS_RING)}>
            {failed
              ? <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
              : <Download className="w-3.5 h-3.5" aria-hidden="true" />}
            {failed ? 'Try again' : 'Set up Shodh'}
          </button>
        )}
        {waiting && onCancelWaiting && (
          <button type="button" onClick={onCancelWaiting} className={cn(QUIET_BUTTON, FOCUS_RING)}>
            Don&apos;t send it
          </button>
        )}
        {installing && !compact && (
          <span className="text-caption text-shodh-text-muted">The download continues if you move on.</span>
        )}
      </div>
    </section>
  );
}

function PartRow({ part }: { part: PartView }) {
  const icon = part.state === 'ready'
    ? <CheckCircle2 className="w-3.5 h-3.5 shrink-0 text-shodh-success" aria-hidden="true" />
    : part.state === 'installing'
      ? <Loader2 className="w-3.5 h-3.5 shrink-0 agent-spin text-shodh-accent" aria-hidden="true" />
      : part.state === 'failed'
        ? <TriangleAlert className="w-3.5 h-3.5 shrink-0 text-shodh-warning" aria-hidden="true" />
        : null;
  return (
    <li className="flex flex-col gap-1">
      <div className="flex items-center justify-between gap-3 text-small">
        <span className="inline-flex items-center gap-1.5 font-medium text-shodh-text">
          {icon}
          {part.label}
        </span>
        <span className="text-caption text-shodh-text-muted tabular-nums text-right">{part.detail}</span>
      </div>
      {part.state === 'installing' && (
        <div
          role="progressbar"
          aria-label={`Downloading the ${part.label.toLowerCase()}`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={part.fraction === null ? undefined : Math.round(part.fraction * 100)}
          className="h-1.5 rounded-full bg-shodh-raised-2 overflow-hidden"
        >
          <div
            className={cn(
              'h-full w-full origin-left bg-shodh-accent transition-transform duration-panel',
              part.fraction === null && 'ask-breathe',
            )}
            style={{ transform: `scaleX(${part.fraction ?? 0.15})` }}
          />
        </div>
      )}
    </li>
  );
}

export default SetupCard;
