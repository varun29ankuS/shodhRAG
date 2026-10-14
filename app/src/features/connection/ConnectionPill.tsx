import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { ChevronDown, KeyRound, Loader2, RefreshCw, SlidersHorizontal } from 'lucide-react';
import { cn } from '../../lib/utils';
import { MODEL_CHANGED_EVENT } from '../modelPicker/modelApi';
import { requestFixKey, requestModelPicker } from '../modelPicker/navigation';
import type { ProviderId } from '../modelPicker/modelTypes';
import type { ConnectionSnapshot, ProviderStatus } from './connectionStatus';
import {
  CONNECTION_STATUS_EVENT,
  DOT_CLASS,
  checkedText,
  fixLabel,
  healthDot,
  healthText,
  keyOriginNote,
  pillView,
  readSnapshot,
  sourceText,
} from './connectionStatus';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

const SMALL_BUTTON = cn(
  'inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md border border-shodh-border bg-shodh-raised text-[12px] text-shodh-text hover:bg-shodh-raised-2 transition-colors duration-micro disabled:opacity-50 disabled:pointer-events-none',
  FOCUS_RING,
);

/** How often relative times ("4 min ago") are refreshed while the panel is open. */
const CLOCK_TICK_MS = 30_000;

function Dot({ status, className }: { status: ProviderStatus | null; className?: string }) {
  const dot = status ? healthDot(status.health) : 'grey';
  return (
    <span
      aria-hidden="true"
      className={cn(
        'inline-block w-2 h-2 rounded-full shrink-0',
        DOT_CLASS[dot],
        status?.checking && 'animate-pulse motion-reduce:animate-none',
        className,
      )}
    />
  );
}

function ProviderRow({
  status,
  inUse,
  nowMs,
  onCheck,
}: {
  status: ProviderStatus;
  inUse: boolean;
  nowMs: number;
  onCheck: (provider: ProviderId) => void;
}) {
  const note = keyOriginNote(status);
  return (
    <li className="px-3.5 py-3 flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <Dot status={status} />
        <span className="text-[13px] font-semibold text-shodh-text">{status.label}</span>
        {inUse && (
          <span className="text-[10.5px] px-1.5 py-px rounded-full bg-shodh-accent-soft text-shodh-accent-text font-medium">
            In use
          </span>
        )}
        {status.keyHint && (
          <span className="ml-auto font-mono text-[11.5px] text-shodh-text-muted" title="The last characters of the key in use">
            {status.keyHint}
          </span>
        )}
      </div>
      <p className={cn('m-0 text-[12.5px] break-words', status.health === 'rejected' ? 'text-shodh-error' : 'text-shodh-text-secondary')}>
        {healthText(status)}
      </p>
      <p className="m-0 text-[12px] text-shodh-text-muted">
        {sourceText(status)} · {checkedText(status, nowMs)}
      </p>
      {note && <p className="m-0 text-[12px] text-shodh-text-muted">{note}</p>}
      <div className="flex flex-wrap gap-1.5 pt-0.5">
        <button type="button" className={SMALL_BUTTON} disabled={status.checking} onClick={() => onCheck(status.provider)}>
          {status.checking ? (
            <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          ) : (
            <RefreshCw className="w-3.5 h-3.5" aria-hidden="true" />
          )}
          Check now
        </button>
        <button type="button" className={SMALL_BUTTON} onClick={() => requestFixKey(status.provider)}>
          <KeyRound className="w-3.5 h-3.5" aria-hidden="true" />
          {fixLabel(status)}
        </button>
      </div>
    </li>
  );
}

/**
 * The top bar's connection status: the model that answers, its provider and
 * how it is connected, with a dot for whether it works (green), the key was
 * rejected (red), it is rate-limited or out of quota (amber), or it is not
 * checked or unreachable (grey). It opens a panel with every connected
 * provider: status, source, the key's last characters, when it was checked,
 * and Check now / Switch model / Fix key. Checks never spend a chat request.
 */
export function ConnectionPill() {
  const [snapshot, setSnapshot] = useState<ConnectionSnapshot | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  const headingId = useId();

  const load = useCallback(async () => {
    try {
      const next = readSnapshot(await invoke<unknown>('connection_status'));
      if (next) setSnapshot(next);
      setLoadError(null);
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : String(error));
    }
  }, []);

  // The backend sends every change; a model change elsewhere re-reads it.
  useEffect(() => {
    void load();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<unknown>(CONNECTION_STATUS_EVENT, event => {
      const next = readSnapshot(event.payload);
      if (next) setSnapshot(next);
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(error => console.error('Connection status listener failed:', error));
    const onModelChanged = () => void load();
    window.addEventListener(MODEL_CHANGED_EVENT, onModelChanged);
    return () => {
      disposed = true;
      if (unlisten) unlisten();
      window.removeEventListener(MODEL_CHANGED_EVENT, onModelChanged);
    };
  }, [load]);

  // Relative times stay current while the panel is open.
  useEffect(() => {
    if (!open) return;
    setNowMs(Date.now());
    const timer = window.setInterval(() => setNowMs(Date.now()), CLOCK_TICK_MS);
    return () => window.clearInterval(timer);
  }, [open, snapshot]);

  // Close on a click outside.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  const check = useCallback(async (provider: ProviderId | null) => {
    try {
      const next = readSnapshot(await invoke<unknown>('connection_check', { provider }));
      if (next) setSnapshot(next);
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : String(error));
    }
  }, []);

  const close = (refocus: boolean) => {
    setOpen(false);
    if (refocus) buttonRef.current?.focus();
  };

  const pill = pillView(snapshot);
  const activeProvider = snapshot?.active?.model.provider ?? null;
  const anyChecking = snapshot?.providers.some(p => p.checking) ?? false;

  return (
    <div className="relative" ref={rootRef}>
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        aria-label={`Connection: ${pill.description}`}
        title={pill.description}
        className={cn(
          'h-6 max-w-[320px] pl-2 pr-1.5 rounded-full inline-flex items-center gap-1.5 bg-shodh-raised text-[11px] text-shodh-text-secondary hover:text-shodh-text hover:bg-shodh-raised-2 transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <span
          aria-hidden="true"
          className={cn('inline-block w-2 h-2 rounded-full shrink-0', DOT_CLASS[pill.dot], anyChecking && 'animate-pulse motion-reduce:animate-none')}
        />
        <span className="truncate font-medium text-shodh-text">{pill.model}</span>
        {pill.via && <span className="truncate text-shodh-text-muted">{pill.via}</span>}
        <ChevronDown className="w-3 h-3 shrink-0 text-shodh-text-muted" aria-hidden="true" />
      </button>

      {open && (
        <section
          id={panelId}
          aria-labelledby={headingId}
          onKeyDown={e => {
            if (e.key === 'Escape') {
              e.preventDefault();
              close(true);
            }
          }}
          className="absolute left-0 top-8 z-50 w-[360px] max-w-[calc(100vw-24px)] rounded-xl border border-shodh-border bg-shodh-surface shadow-lg overflow-hidden"
        >
          <header className="flex items-center justify-between gap-2 px-3.5 py-2.5 border-b border-shodh-border-subtle">
            <h2 id={headingId} className="m-0 text-[13px] font-semibold text-shodh-text">Connections</h2>
            <div className="flex gap-1.5">
              <button
                type="button"
                className={SMALL_BUTTON}
                onClick={() => {
                  close(false);
                  requestModelPicker();
                }}
              >
                <SlidersHorizontal className="w-3.5 h-3.5" aria-hidden="true" />
                Switch model
              </button>
            </div>
          </header>
          {snapshot?.active && (
            <p className="m-0 px-3.5 pt-2.5 text-[12px] text-shodh-text-muted break-words">
              Answers use <span className="font-mono text-shodh-text-secondary">{snapshot.active.model.model}</span>
            </p>
          )}
          {loadError && (
            <p role="alert" className="m-0 px-3.5 pt-2.5 text-[12.5px] text-shodh-error break-words">
              {loadError}
            </p>
          )}
          {snapshot && snapshot.providers.length > 0 ? (
            <ul className="m-0 p-0 list-none flex flex-col divide-y divide-shodh-border-subtle max-h-[60vh] overflow-y-auto scrollbar-thin" aria-live="polite">
              {snapshot.providers.map(status => (
                <ProviderRow
                  key={status.provider}
                  status={status}
                  inUse={status.provider === activeProvider}
                  nowMs={nowMs}
                  onCheck={provider => void check(provider)}
                />
              ))}
            </ul>
          ) : (
            <p className="m-0 px-3.5 py-5 text-[12.5px] text-shodh-text-muted">
              {snapshot ? 'No provider is connected. Connect one in Settings → Model.' : 'Loading the connection status…'}
            </p>
          )}
          {snapshot?.localOnly && (
            <p className="m-0 px-3.5 py-2 border-t border-shodh-border-subtle text-[12px] text-shodh-text-muted">
              Local-only mode is on: cloud providers are not used or checked.
            </p>
          )}
          <p className="m-0 px-3.5 py-2 border-t border-shodh-border-subtle text-[11.5px] text-shodh-text-faint">
            Checks never send a chat request.
          </p>
        </section>
      )}
    </div>
  );
}

export default ConnectionPill;
