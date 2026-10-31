import React from 'react';
import { RotateCcw } from 'lucide-react';
import { cn } from '../../lib/utils';
import { STALE_DETAIL, STALE_TITLE } from '../../lib/freshness';

/**
 * Over data a view could not refresh (lib/freshness "stale"): says the data
 * may be out of date, why, and offers the refresh again. Polite, never
 * focused: it reports a background read, not something the user did.
 */
export function StaleNotice({ error, onRetry, className }: { error: string; onRetry: () => void; className?: string }) {
  return (
    <div
      role="status"
      className={cn(
        'flex items-start gap-2.5 rounded-lg border border-shodh-warning/40 bg-shodh-warning-soft px-3.5 py-2.5 text-small text-shodh-text-secondary',
        className,
      )}
    >
      <span aria-hidden="true" className="mt-1.5 w-1.5 h-1.5 rounded-full shrink-0 bg-shodh-warning" />
      <div className="min-w-0 flex-1">
        <p className="m-0">
          <span className="font-semibold text-shodh-text">{STALE_TITLE}.</span> {STALE_DETAIL}
        </p>
        <p className="m-0 mt-0.5 text-caption text-shodh-text-faint break-words">{error}</p>
      </div>
      <button
        type="button"
        onClick={onRetry}
        className="shrink-0 h-7 px-2.5 inline-flex items-center gap-1.5 rounded-md border border-shodh-border bg-shodh-surface text-caption font-medium text-shodh-text-secondary hover:text-shodh-text hover:bg-shodh-raised transition-colors duration-micro focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
        Try again
      </button>
    </div>
  );
}
