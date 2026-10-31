import React, { useCallback, useState } from 'react';
import { Download, ShieldCheck } from 'lucide-react';
import { cn } from '../lib/utils';
import { useSearchModels } from '../features/setup/SearchModelsContext';
import { scanReaderSummary } from '../features/setup/searchModels';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const ON_WINDOWS = typeof navigator !== 'undefined' && /Win/i.test(navigator.platform);

function megabytes(bytes: number): string {
  return `${Math.max(1, Math.round(bytes / 1_000_000))} MB`;
}

/**
 * Settings → Search → Scan reader: the OCR model that reads scanned PDFs and images.
 * It is installed with the search models; a library set up before it existed gets
 * it here (the Inbox offers it once). Installing it reads again the scans that were
 * read without it.
 */
export default function ScanReaderSettings() {
  const { status, installing, progress, installError, install, dismissInstalled } = useSearchModels();
  const [started, setStarted] = useState(false);
  const summary = status ? scanReaderSummary(status) : null;

  const installReader = useCallback(async () => {
    setStarted(true);
    try {
      await install();
    } finally {
      // Search was already set up: the "Search is set up" confirmation is not news.
      dismissInstalled();
      setStarted(false);
    }
  }, [install, dismissInstalled]);

  if (!status || !summary) return null;

  const size = megabytes(summary.totalBytes);
  let description: string;
  if (!status.ready) {
    description = `Installed with the search models when search is set up (${size}).`;
  } else if (summary.installed) {
    description = 'Installed. Scanned PDFs and images are read with it.';
  } else if (ON_WINDOWS) {
    description = `Without it, scanned PDFs and images are read with Windows OCR, which misses text in tables and noisy scans. Scans already read that way are read again once it is installed. A local text recognition model (${size}, verified download).`;
  } else {
    description = `Without it, scanned PDFs and images cannot be read; they are read automatically once it is installed. A local text recognition model (${size}, verified download).`;
  }

  const percent = installing && progress && progress.overallTotal > 0
    ? Math.min(100, Math.round((progress.overallBytes / progress.overallTotal) * 100))
    : null;

  return (
    <div className="mt-6 pt-6 border-t border-shodh-border-subtle flex items-start justify-between gap-6">
      <div className="min-w-0">
        <h3 className="m-0 text-body-lg font-semibold text-shodh-text">Scan reader</h3>
        <p className="m-0 mt-0.5 text-small text-shodh-text-muted">{description}</p>
        {installing && status.ready && !summary.installed && (
          <p className="m-0 mt-1 text-caption text-shodh-text-secondary tabular-nums" role="status" aria-live="polite">
            {percent !== null ? `Downloading… ${percent}%` : 'Installing…'}
          </p>
        )}
        {started === false && installError && status.ready && !summary.installed && (
          <p role="alert" className="m-0 mt-1 text-small text-shodh-error break-words">
            {`The scan reader could not be installed: ${installError}`}
          </p>
        )}
      </div>
      {summary.installed ? (
        <span className="shrink-0 inline-flex items-center gap-1.5 text-small text-shodh-success">
          <ShieldCheck className="w-4 h-4" aria-hidden="true" />
          Ready
        </span>
      ) : status.ready ? (
        <button
          type="button"
          onClick={() => void installReader()}
          disabled={installing}
          className={cn(
            'shrink-0 h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface text-small text-shodh-text',
            'hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          <Download className="w-3.5 h-3.5" aria-hidden="true" />
          {summary.resumable ? 'Resume install' : 'Install'}
        </button>
      ) : null}
    </div>
  );
}
