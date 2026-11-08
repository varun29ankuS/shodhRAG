import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Network, Pause, Play, RotateCcw, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { useSystemStatusContext } from '../status/SystemStatusContext';
import { codeMapApi, errorText } from './api';
import type { CodeMapView } from './api';
import { codeMapChipLabel, codeMapDetails, codeMapTone, readCodeMapView } from './codeMapFormat';
import type { CodeMapTone } from './codeMapFormat';

/** Emitted by `code_map_commands.rs` when a map's state or progress changes. */
const CODE_MAP_EVENT = 'code-map-status';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';
const SMALL_BUTTON = cn(
  'inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md border border-shodh-border bg-shodh-raised text-caption text-shodh-text hover:bg-shodh-raised-2 transition-colors duration-micro disabled:opacity-50 disabled:pointer-events-none',
  FOCUS_RING,
);

const TONE_DOT: Record<CodeMapTone, string> = {
  good: 'bg-shodh-success',
  busy: 'bg-shodh-accent',
  warn: 'bg-shodh-warning',
  bad: 'bg-shodh-error',
};

interface CodeMapChipProps {
  /** The Code conversation's workspace, whose folder is the project. */
  workspaceId: string;
}

/**
 * Code mode's status line: the project's code map ("Code map: up to date ·
 * 2,341 symbols", or its progress while it is built). Shown while the Code
 * conversation is, which is also what keeps the map watching the folder for
 * changes. Click for what is indexed and when, Rebuild, and Stop or Pause
 * while it builds. The agent's code tools read the same map.
 */
export function CodeMapChip({ workspaceId }: CodeMapChipProps) {
  const [view, setView] = useState<CodeMapView | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const popoverId = useId();
  const headingId = useId();
  const system = useSystemStatusContext();

  // Shown: build or refresh the map and watch the folder; hidden: stop watching.
  useEffect(() => {
    let live = true;
    // The lease this mount took, given back by its id (by the time it
    // unmounts, the workspace may be gone or point at another folder).
    let lease: number | null = null;
    setView(null);
    setOpen(false);
    codeMapApi
      .open(workspaceId)
      .then(next => {
        lease = next.lease ?? null;
        if (!live) {
          if (lease !== null) codeMapApi.close(lease).catch(() => undefined);
          return;
        }
        setView(next);
      })
      .catch(err => console.error('Code map unavailable:', errorText(err)));
    return () => {
      live = false;
      if (lease !== null) codeMapApi.close(lease).catch(() => undefined);
    };
  }, [workspaceId]);

  // Progress and results arrive as events for this project's map.
  const sourceId = view?.sourceId ?? null;
  useEffect(() => {
    if (!sourceId) return;
    const unlisten = listen(CODE_MAP_EVENT, event => {
      const next = readCodeMapView(event.payload);
      if (next && next.sourceId === sourceId) setView({ ...next, workspaceId });
    });
    return () => {
      unlisten.then(fn => fn()).catch(() => undefined);
    };
  }, [sourceId, workspaceId]);

  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) buttonRef.current?.focus();
  }, []);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        close(true);
      }
    };
    const onPointer = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!popoverRef.current?.contains(target) && !buttonRef.current?.contains(target)) close(false);
    };
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('pointerdown', onPointer);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('pointerdown', onPointer);
    };
  }, [open, close]);

  const act = (what: string, action: () => Promise<CodeMapView | void>) => {
    setBusy(true);
    action()
      .then(next => {
        if (next) setView(next);
      })
      .catch(err => notify.error(`Could not ${what}`, { description: errorText(err) }))
      .finally(() => setBusy(false));
  };

  if (!view) return null;
  const building = view.state === 'building' || view.state === 'queued' || view.state === 'waiting';
  const label = codeMapChipLabel(view);
  const details = codeMapDetails(view, new Date());

  return (
    <div className="relative shrink-0">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? popoverId : undefined}
        title={`${label}. The assistant uses it to find definitions and uses.`}
        className={cn(
          'inline-flex items-center gap-1.5 h-[26px] px-2 rounded-full text-caption font-medium border border-shodh-border',
          'text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised transition-colors duration-micro tabular-nums',
          FOCUS_RING,
        )}
      >
        <Network className="w-3 h-3 shrink-0" aria-hidden="true" />
        <span className="whitespace-nowrap">{label}</span>
        <span aria-hidden="true" className={cn('inline-block w-1.5 h-1.5 rounded-full shrink-0', TONE_DOT[codeMapTone(view)])} />
      </button>
      {open && (
        <div
          ref={popoverRef}
          id={popoverId}
          role="dialog"
          aria-labelledby={headingId}
          className="absolute bottom-full left-0 mb-2 z-40 w-[360px] max-w-[calc(100vw-32px)] rounded-[14px] border border-shodh-border bg-shodh-raised p-3.5 shadow-[0_12px_40px_rgba(0,0,0,0.35)] flex flex-col gap-3"
        >
          <h2 id={headingId} className="m-0 text-small font-semibold text-shodh-text tabular-nums">
            {label}
          </h2>
          <ul className="m-0 p-0 list-none flex flex-col gap-1">
            {details.map(line => (
              <li key={line} className="text-small text-shodh-text-secondary tabular-nums break-words">
                {line}
              </li>
            ))}
          </ul>
          <p className="m-0 text-small text-shodh-text-muted">
            Shodh keeps a map of {view.name}&apos;s Rust, TypeScript, JavaScript, Python and Go code on this computer, and
            updates it when files change and after the assistant edits or runs commands. The assistant uses it to find
            where things are defined and who uses them, and shows you each place. Ignored files, build folders, .env files
            and keys are never read.
          </p>
          <div className="flex flex-wrap gap-1.5">
            {building ? (
              <>
                <button
                  type="button"
                  className={SMALL_BUTTON}
                  disabled={busy || !system}
                  onClick={() => act(view.paused ? 'resume indexing' : 'pause indexing', async () => system?.setPaused(!view.paused))}
                >
                  {view.paused ? <Play className="w-3.5 h-3.5" aria-hidden="true" /> : <Pause className="w-3.5 h-3.5" aria-hidden="true" />}
                  {view.paused ? 'Resume indexing' : 'Pause indexing'}
                </button>
                <button type="button" className={SMALL_BUTTON} disabled={busy} onClick={() => act('stop the code map', () => codeMapApi.cancel(workspaceId))}>
                  <X className="w-3.5 h-3.5" aria-hidden="true" />
                  Stop
                </button>
              </>
            ) : (
              <button type="button" className={SMALL_BUTTON} disabled={busy} onClick={() => act('rebuild the code map', () => codeMapApi.rebuild(workspaceId))}>
                <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
                Rebuild
              </button>
            )}
          </div>
          {building && <p className="m-0 text-caption text-shodh-text-faint">Pause holds every indexing run; Stop keeps what was read.</p>}
        </div>
      )}
    </div>
  );
}

export default CodeMapChip;
