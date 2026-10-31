import React, { useCallback, useEffect, useRef, useState } from 'react';
import { CheckCircle2, Download, Loader2, RotateCcw, TriangleAlert } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useRuntimeSetup } from '../setup/RuntimeSetupContext';
import { checksumRow, runtimeCheckPromise, runtimeVerifiedSentence, unverifiedSentence } from '../setup/verification';
import { TechnicalDetails } from '../setup/TechnicalDetails';
import { approxMegabytes } from '../setup/readiness';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

function megabytes(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

interface RuntimeCardProps {
  /** `invalid`: present but does not match the official copy. */
  reason: 'missing' | 'invalid';
  /** Detail from the failed start, if any. */
  message?: string | null;
  onInstalled: () => void;
  compact?: boolean;
}

/**
 * Shown instead of an answer when the answer engine (the pinned agent runtime)
 * is not installed or does not match the official copy: downloads it through the
 * shared setup (one install with first-run setup and the setup card) and says
 * what its verification means, with the checksum behind "Technical details".
 */
export function RuntimeCard({ reason, message, onInstalled, compact = false }: RuntimeCardProps) {
  const runtime = useRuntimeSetup();
  const [done, setDone] = useState(false);
  // Only an install that finished after this card appeared covers the failure it shows.
  const shownAt = useRef(Date.now());
  const source = runtime.status?.source ?? 'GitHub';
  const size = runtime.status?.downloadBytes ?? null;
  const pinned = runtime.status?.pinned ?? false;

  const install = useCallback(async () => {
    if (await runtime.install()) {
      setDone(true);
      onInstalled();
    }
  }, [runtime, onInstalled]);

  // Installed elsewhere (setup, another card) while this one was shown.
  useEffect(() => {
    if (!done && reason === 'missing' && runtime.justInstalled && (runtime.installedAt ?? 0) > shownAt.current) {
      setDone(true);
      onInstalled();
    }
  }, [done, reason, runtime.justInstalled, runtime.installedAt, onInstalled]);

  const installing = runtime.installing;
  const failedMessage = !installing && !done ? runtime.installError : null;
  const progress = installing ? runtime.progress : null;
  const total = progress?.total ?? size;
  const fraction = progress && total ? Math.min(1, progress.downloaded / total) : null;
  const result = done ? runtime.justInstalled : null;

  return (
    <section
      aria-label="Agent runtime"
      className={cn('ask-rise rounded-xl border border-shodh-border bg-shodh-surface flex flex-col gap-3', compact ? 'p-3' : 'p-4')}
    >
      <div className="flex items-start gap-2.5">
        {done ? (
          <CheckCircle2 className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-success" aria-hidden="true" />
        ) : (
          <TriangleAlert className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-warning" aria-hidden="true" />
        )}
        <div className="min-w-0 flex flex-col gap-1">
          <p className="text-body font-semibold text-shodh-text">
            {done
              ? 'Answer engine installed'
              : reason === 'invalid'
                ? "The answer engine couldn't be verified"
                : 'The answer engine is not installed yet'}
          </p>
          <p className="text-small leading-relaxed text-shodh-text-muted">
            {done
              ? `${runtimeVerifiedSentence(source, pinned)} Ask your question again.`
              : reason === 'invalid'
                ? `${unverifiedSentence()} ${runtimeCheckPromise(source, pinned)}`
                : `Answers are written by an answer engine that runs on this computer${size ? ` (${approxMegabytes(size)}, downloaded once)` : ''}. ${runtimeCheckPromise(source, pinned)}`}
          </p>

        </div>
      </div>

      {installing && (
        <div className="flex flex-col gap-1.5" aria-live="polite">
          <div
            role="progressbar"
            aria-label="Downloading the agent runtime"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={fraction === null ? undefined : Math.round(fraction * 100)}
            className="h-1.5 rounded-full bg-shodh-raised-2 overflow-hidden"
          >
            <div
              className={cn('h-full w-full origin-left bg-shodh-accent transition-transform duration-panel', fraction === null && 'ask-breathe')}
              style={{ transform: `scaleX(${fraction ?? 0.15})` }}
            />
          </div>
          <p className="text-caption text-shodh-text-muted tabular-nums">
            {progress
              ? total
                ? `${megabytes(progress.downloaded)} of ${megabytes(total)}`
                : megabytes(progress.downloaded)
              : 'Connecting…'}
          </p>
        </div>
      )}

      {failedMessage && (
        <p role="alert" className="text-small text-shodh-error break-words">{failedMessage}</p>
      )}

      {(result || (message && !installing && !done)) && (
        <TechnicalDetails
          rows={[
            ...(result ? [checksumRow(`Answer engine (omp ${result.version})`, result.sha256)] : []),
            ...(message && !done ? [{ label: 'What the check reported', value: message }] : []),
          ]}
        />
      )}

      {!done && (
        <div>
          <button
            type="button"
            onClick={() => void install()}
            disabled={installing}
            className={cn(
              'inline-flex items-center gap-2 h-8 px-3.5 rounded-lg bg-shodh-accent text-shodh-on-accent text-small font-semibold hover:bg-shodh-accent-hover disabled:opacity-60 disabled:cursor-not-allowed transition-colors duration-micro',
              FOCUS_RING,
            )}
          >
            {installing ? (
              <Loader2 className="w-3.5 h-3.5 agent-spin" aria-hidden="true" />
            ) : failedMessage ? (
              <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
            ) : (
              <Download className="w-3.5 h-3.5" aria-hidden="true" />
            )}
            {installing ? 'Installing…' : failedMessage ? 'Try again' : reason === 'invalid' ? 'Download it again' : 'Install the answer engine'}
          </button>
        </div>
      )}
    </section>
  );
}

export default RuntimeCard;
