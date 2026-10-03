import React from 'react';
import { CheckCircle2, Download, Loader2, RotateCcw, ShieldCheck, TriangleAlert, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useSearchModels } from './SearchModelsContext';
import { describeProgress, formatMegabytes, installFraction, summarizeSetup } from './searchModels';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const PRIMARY_BUTTON =
  'inline-flex items-center gap-2 h-8 px-3.5 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12.5px] font-semibold hover:bg-shodh-accent-hover disabled:opacity-60 disabled:cursor-not-allowed transition-colors duration-micro';

interface SearchSetupCardProps {
  compact?: boolean;
  className?: string;
}

/**
 * First-run setup for search: downloads the pinned E5 + reranker models
 * (≈600 MB, once), shows progress and the verified checksums, and offers a
 * retry on failure. Renders nothing once search is ready, unless an install
 * just finished (then it shows the verified result until dismissed).
 */
export function SearchSetupCard({ compact = false, className }: SearchSetupCardProps) {
  const { status, statusError, installing, progress, installError, installed, install, refresh, dismissInstalled } =
    useSearchModels();

  const shell = cn(
    'ask-rise rounded-xl border border-shodh-border bg-shodh-surface flex flex-col gap-3',
    compact ? 'p-3' : 'p-4',
    className,
  );

  // The status check failed: say so and offer a retry rather than nothing.
  if (!status && statusError) {
    return (
      <section aria-label="Search setup" className={shell}>
        <div className="flex items-start gap-2.5">
          <TriangleAlert className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-warning" aria-hidden="true" />
          <div className="min-w-0 flex flex-col gap-1">
            <p className="text-[13.5px] font-semibold text-shodh-text">Could not check whether search is set up</p>
            <p role="alert" className="text-[12.5px] text-shodh-error break-words">{statusError}</p>
          </div>
        </div>
        <div>
          <button type="button" onClick={() => void refresh()} className={cn(PRIMARY_BUTTON, FOCUS_RING)}>
            <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
            Check again
          </button>
        </div>
      </section>
    );
  }

  if (installed && !installing) {
    return (
      <section aria-label="Search setup" className={shell}>
        <div className="flex items-start gap-2.5">
          <CheckCircle2 className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-success" aria-hidden="true" />
          <div className="min-w-0 flex-1 flex flex-col gap-1">
            <p className="text-[13.5px] font-semibold text-shodh-text">Search is set up</p>
            <p className="text-[12.5px] leading-relaxed text-shodh-text-muted">
              Every model file matched its pinned SHA-256 checksum.
            </p>
          </div>
          <button
            type="button"
            onClick={dismissInstalled}
            aria-label="Dismiss"
            className={cn('shrink-0 p-1 rounded-md text-shodh-text-muted hover:text-shodh-text', FOCUS_RING)}
          >
            <X className="w-3.5 h-3.5" aria-hidden="true" />
          </button>
        </div>
        <ul className="flex flex-col gap-1" aria-label="Verified files">
          {installed.artifacts.map(a => (
            <li key={a.relativePath} className="flex items-center gap-1.5 text-[11.5px] font-mono text-shodh-text-muted break-all">
              <ShieldCheck className="w-3.5 h-3.5 shrink-0 text-shodh-success" aria-hidden="true" />
              {`${a.name} · sha256 ${a.sha256.slice(0, 16)}…`}
            </li>
          ))}
        </ul>
      </section>
    );
  }

  if (!status || (status.ready && !installing)) return null;

  const summary = summarizeSetup(status);
  const fraction = installFraction(progress);
  const sizeLabel = `≈${Math.round(summary.totalBytes / 1_000_000 / 100) * 100} MB`;

  return (
    <section aria-label="Search setup" className={shell}>
      <div className="flex items-start gap-2.5">
        <Download className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-accent" aria-hidden="true" />
        <div className="min-w-0 flex flex-col gap-1">
          <p className="text-[13.5px] font-semibold text-shodh-text">{`Set up search (${sizeLabel}, one time)`}</p>
          <p className="text-[12.5px] leading-relaxed text-shodh-text-muted">
            Searching and indexing your files needs two models that run on this computer: an embedding model and a
            reranker. Shodh downloads pinned versions once from Hugging Face and checks each file&apos;s SHA-256 before
            using it.
          </p>
          {!installing && summary.resumable && (
            <p className="text-[12px] text-shodh-text-faint">
              {`A previous download was interrupted; ${formatMegabytes(summary.remainingBytes)} left. It resumes where it stopped.`}
            </p>
          )}
          {!installing && summary.hasCorrupt && (
            <p className="text-[12px] text-shodh-text-faint">
              A model file on disk failed verification and will be downloaded again.
            </p>
          )}
        </div>
      </div>

      {installing && (
        <div className="flex flex-col gap-1.5">
          <div
            role="progressbar"
            aria-label="Downloading the search models"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={fraction === null ? undefined : Math.round(fraction * 100)}
            className="h-1.5 rounded-full bg-shodh-raised-2 overflow-hidden"
          >
            <div
              className={cn(
                'h-full w-full origin-left bg-shodh-accent transition-transform duration-panel',
                fraction === null && 'ask-breathe',
              )}
              style={{ transform: `scaleX(${fraction ?? 0.15})` }}
            />
          </div>
          <p className="text-[11.5px] font-mono text-shodh-text-muted" aria-live="polite">
            {describeProgress(progress)}
          </p>
        </div>
      )}

      {installError && !installing && (
        <p role="alert" className="text-[12.5px] text-shodh-error break-words">{installError}</p>
      )}

      {!installing && (
        <div>
          <button type="button" onClick={() => void install()} className={cn(PRIMARY_BUTTON, FOCUS_RING)}>
            {installError ? (
              <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
            ) : (
              <Download className="w-3.5 h-3.5" aria-hidden="true" />
            )}
            {installError ? 'Try again' : summary.resumable ? 'Resume setup' : 'Set up search'}
          </button>
        </div>
      )}
      {installing && (
        <div>
          <button type="button" disabled className={cn(PRIMARY_BUTTON, FOCUS_RING)}>
            <Loader2 className="w-3.5 h-3.5 agent-spin" aria-hidden="true" />
            Setting up…
          </button>
        </div>
      )}
    </section>
  );
}

export default SearchSetupCard;
