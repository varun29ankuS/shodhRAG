import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Download, ShieldCheck } from 'lucide-react';
import { cn } from '../lib/utils';
import { isInstallProgress, isSearchModelsStatus } from '../features/setup/searchModels';
import type { InstallProgress } from '../features/setup/searchModels';

const TABLE_MODEL_PROGRESS_EVENT = 'table-model-progress';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

interface TableModelStatus {
  ready: boolean;
  installing: boolean;
  supported: boolean;
  totalBytes: number;
}

function megabytes(bytes: number): string {
  return `${Math.max(1, Math.round(bytes / 1_000_000))} MB`;
}

/**
 * Settings → Search → Table structure: the optional local table model. Papers are
 * indexed with the fast layout heuristics first; with the model installed, pages that
 * look like they hold tables are refined afterwards in the background.
 */
export default function TableModelSettings() {
  const [status, setStatus] = useState<TableModelStatus | null>(null);
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const value = await invoke<unknown>('table_model_status');
      if (isSearchModelsStatus(value)) {
        const v = value as unknown as TableModelStatus;
        setStatus({ ready: v.ready, installing: v.installing, supported: v.supported, totalBytes: v.totalBytes });
      }
    } catch (e) {
      setError(`The table model's status could not be read: ${String(e)}`);
    }
  }, []);

  useEffect(() => {
    let alive = true;
    void refresh();
    let unlisten: (() => void) | null = null;
    listen<unknown>(TABLE_MODEL_PROGRESS_EVENT, event => {
      if (isInstallProgress(event.payload)) setProgress(event.payload);
    })
      .then(fn => {
        if (alive) unlisten = fn;
        else fn();
      })
      .catch(e => console.error('Failed to listen for table model progress:', e));
    return () => {
      alive = false;
      if (unlisten) unlisten();
    };
  }, [refresh]);

  const install = useCallback(async () => {
    setError(null);
    setStatus(s => (s ? { ...s, installing: true } : s));
    try {
      await invoke<unknown>('install_table_model');
    } catch (e) {
      setError(`The table model could not be installed: ${String(e)}`);
    } finally {
      setProgress(null);
      await refresh();
    }
  }, [refresh]);

  const percent = progress && progress.overallTotal > 0
    ? Math.min(100, Math.round((progress.overallBytes / progress.overallTotal) * 100))
    : null;

  let description: string;
  if (status && !status.supported) {
    description = 'Not available on this system. Tables are read with the layout heuristics only, so table extraction quality is reduced.';
  } else if (status?.ready) {
    description = 'Installed. Pages that look like they hold tables are structured with it after indexing: merged headers, spanning cells and units are kept.';
  } else {
    description = `Without it, tables are read with the layout heuristics only and table extraction quality is reduced. A local layout and table-structure model (${megabytes(status?.totalBytes ?? 212_315_048)}, verified download).`;
  }

  return (
    <div className="mt-6 pt-6 border-t border-shodh-border-subtle flex items-start justify-between gap-6">
      <div className="min-w-0">
        <h3 className="m-0 text-[15px] font-semibold text-shodh-text">Table structure model</h3>
        <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">{description}</p>
        {status?.installing && (
          <p className="m-0 mt-1 text-[12px] text-shodh-text-secondary tabular-nums" role="status" aria-live="polite">
            {percent !== null ? `Downloading… ${percent}%` : 'Installing…'}
          </p>
        )}
        {error && (
          <p role="alert" className="m-0 mt-1 text-[12.5px] text-shodh-error break-words">
            {error}
          </p>
        )}
      </div>
      {status?.ready ? (
        <span className="shrink-0 inline-flex items-center gap-1.5 text-[12.5px] text-shodh-success">
          <ShieldCheck className="w-4 h-4" aria-hidden="true" />
          Ready
        </span>
      ) : (
        <button
          type="button"
          onClick={() => void install()}
          disabled={status === null || status.installing || !status.supported}
          className={cn(
            'shrink-0 h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text',
            'hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          <Download className="w-3.5 h-3.5" aria-hidden="true" />
          Install
        </button>
      )}
    </div>
  );
}
