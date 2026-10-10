import React, { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Download, Table2 } from 'lucide-react';
import { toast } from 'sonner';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { claimTableModelOffer, TABLE_MODEL_SUGGESTED_EVENT } from './tableModelOffer';

const TOAST_ID = 'table-model-suggested';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';
const ACTION = cn(
  'h-7 px-2.5 inline-flex items-center gap-1 rounded-md text-[12px] font-medium transition-colors duration-micro',
  FOCUS_RING,
);

function browserStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function install() {
  toast.dismiss(TOAST_ID);
  notify.info('Installing the table model', { description: 'Tables are re-read in the background when it is ready.' });
  invoke('install_table_model')
    .then(() => notify.success('Table model installed', { description: 'Indexed PDFs with tables are being refined.' }))
    .catch(err => notify.error('The table model could not be installed', { description: errorText(err) }));
}

function SuggestionToast() {
  return (
    <div
      role="status"
      aria-labelledby="table-model-suggested-title"
      aria-describedby="table-model-suggested-text"
      className="w-[356px] max-w-full flex flex-col gap-2 p-3 rounded-[10px] border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[0_8px_24px_rgba(0,0,0,0.3)]"
    >
      <div className="flex items-start gap-2">
        <Table2 className="w-4 h-4 mt-0.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
        <div className="min-w-0 flex flex-col">
          <span id="table-model-suggested-title" className="text-[13px] font-medium">
            Install the table model for better tables (212 MB)
          </span>
          <span id="table-model-suggested-text" className="text-[11.5px] text-shodh-text-muted">
            Some of your PDFs have tables. Without the model they are read with layout rules only, and rows or cells
            can be missed. You can also install it later in Settings → Search.
          </span>
        </div>
      </div>
      <div className="flex items-center gap-1.5 justify-end">
        <button
          type="button"
          className={cn(ACTION, 'text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text')}
          onClick={() => toast.dismiss(TOAST_ID)}
        >
          Not now
        </button>
        <button
          type="button"
          className={cn(ACTION, 'bg-shodh-accent text-shodh-on-accent hover:opacity-90')}
          onClick={install}
        >
          <Download className="w-3.5 h-3.5" aria-hidden="true" />
          Install
        </button>
      </div>
    </div>
  );
}

/**
 * Offers the table model once: the backend reports the first PDF with
 * table-candidate pages indexed while the model is missing, and the offer is a
 * dismissible toast (indexing never waits on it). Shown at most once across runs.
 */
export function TableModelPrompt() {
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listen<unknown>(TABLE_MODEL_SUGGESTED_EVENT, () => {
      if (!claimTableModelOffer(browserStorage())) return;
      toast.custom(() => <SuggestionToast />, { id: TOAST_ID, duration: Infinity });
    })
      .then(fn => {
        if (active) unlisten = fn;
        else fn();
      })
      .catch(err => console.error('Failed to listen for the table model suggestion:', err));
    return () => {
      active = false;
      if (unlisten) unlisten();
    };
  }, []);
  return null;
}
