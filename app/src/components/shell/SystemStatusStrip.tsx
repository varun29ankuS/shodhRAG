/**
 * The app-wide status strip (sidebar footer): what runs search and how
 * fast, and the indexing in progress with Pause / Resume and Cancel. It
 * opens the System status panel: each part of the system with its live
 * state, details behind a disclosure, and the "Use the graphics card"
 * setting. Every state shown is the backend's (`indexing_control.rs`).
 */
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type React from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { Cpu, FileSearch, Gauge, ListOrdered, MessageSquare, Pause, Play, Plug, ScanText, Square, X } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { countOf } from '../../lib/plural';
import { pillView, readSnapshot } from '../../features/connection/connectionStatus';
import type { ConnectionSnapshot } from '../../features/connection/connectionStatus';
import { slackApi } from '../../features/connectors/api';
import { telegramApi } from '../../features/connectors/telegramApi';
import type { SlackState } from '../../features/connectors/slackModel';
import { useSystemStatusContext } from '../../features/status/SystemStatusContext';
import type { FolderProgress } from '../../features/status/useSystemStatus';
import {
  cpuNotice,
  etaText,
  indexingText,
  pagesPerMinuteText,
  runsOnText,
  searchSummary,
} from '../../features/status/systemStatus';
import type { SystemStatus, Tone } from '../../features/status/systemStatus';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-sidebar';
const PANEL_FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';
const SMALL_BUTTON = cn(
  'inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md border border-shodh-border bg-shodh-raised text-caption text-shodh-text hover:bg-shodh-raised-2 transition-colors duration-micro disabled:opacity-50 disabled:pointer-events-none',
  PANEL_FOCUS_RING,
);
const ICON_BUTTON = cn(
  'shrink-0 w-7 h-7 inline-flex items-center justify-center rounded-md text-shodh-text-muted hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro',
  FOCUS_RING,
);

const TONE_DOT: Record<Tone, string> = {
  good: 'bg-shodh-success',
  warn: 'bg-shodh-warning',
  bad: 'bg-shodh-error',
  idle: 'bg-shodh-text-faint',
};

const PANEL_GAP = 8;

/** A folder being indexed, as the sidebar knows it. */
export interface StripJob {
  id: string;
  name: string;
  progress?: number;
  fileCount?: number;
  processedCount?: number;
}

function ToneDot({ tone, className }: { tone: Tone; className?: string }) {
  return <span aria-hidden="true" className={cn('inline-block w-2 h-2 rounded-full shrink-0', TONE_DOT[tone], className)} />;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** "12 of 40 files · about 3 min left". */
function jobDetail(job: StripJob, progress: FolderProgress | undefined): string {
  const total = progress?.total || job.fileCount || 0;
  const processed = progress?.processed ?? job.processedCount ?? 0;
  if (total === 0) return 'Preparing files';
  const parts = [`${processed.toLocaleString()} of ${countOf(total, 'file')}`];
  const left = etaText(progress?.etaSeconds);
  if (left) parts.push(left);
  return parts.join(' · ');
}

function connectorText(state: SlackState | null): string {
  switch (state) {
    case null:
      return 'Checking';
    case 'not_set_up':
      return 'Not set up';
    case 'off':
      return 'Off';
    case 'connecting':
      return 'Connecting';
    case 'connected':
      return 'Connected';
    case 'reconnecting':
      return 'Reconnecting';
    case 'error':
      return 'Not working';
    case 'local_only':
      return 'Off (local-only mode)';
  }
}

function connectorTone(state: SlackState | null): Tone {
  if (state === 'connected') return 'good';
  if (state === 'error') return 'bad';
  if (state === 'connecting' || state === 'reconnecting') return 'warn';
  return 'idle';
}

/** One part of the system in the panel: icon, name, state, details. */
function PartRow({
  icon: Icon,
  name,
  tone,
  state,
  children,
}: {
  icon: LucideIcon;
  name: string;
  tone: Tone;
  state: string;
  children?: React.ReactNode;
}) {
  return (
    <li className="px-3.5 py-3 flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <Icon className="w-4 h-4 shrink-0 text-shodh-text-muted" aria-hidden="true" />
        <span className="text-small font-semibold text-shodh-text">{name}</span>
        <ToneDot tone={tone} className="ml-auto" />
      </div>
      <p className="m-0 text-small text-shodh-text-secondary break-words">{state}</p>
      {children}
    </li>
  );
}

function SearchDetails({ status }: { status: SystemStatus }) {
  const report = status.search;
  if (!report) return null;
  const measured = report.calibratedAt ? new Date(report.calibratedAt) : null;
  return (
    <details className="text-caption text-shodh-text-muted">
      <summary className={cn('cursor-pointer select-none w-fit rounded-sm', PANEL_FOCUS_RING)}>Details</summary>
      <dl className="mt-1.5 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
        <dt>Runs on</dt>
        <dd className="m-0 text-shodh-text-secondary break-words">{runsOnText(report)}</dd>
        {report.threads !== null && (
          <>
            <dt>Processor threads</dt>
            <dd className="m-0 text-shodh-text-secondary">{report.threads}</dd>
          </>
        )}
        <dt>Measured speed</dt>
        <dd className="m-0 text-shodh-text-secondary">{pagesPerMinuteText(report.pagesPerMinute)}</dd>
        {report.recentPagesPerMinute !== null && (
          <>
            <dt>Recent speed</dt>
            <dd className="m-0 text-shodh-text-secondary">{pagesPerMinuteText(report.recentPagesPerMinute)}</dd>
          </>
        )}
        {measured && !Number.isNaN(measured.getTime()) && (
          <>
            <dt>Measured on</dt>
            <dd className="m-0 text-shodh-text-secondary">{measured.toLocaleString()}</dd>
          </>
        )}
        <dt>In memory</dt>
        <dd className="m-0 text-shodh-text-secondary">{report.loaded ? 'Yes' : 'No (loads when search runs)'}</dd>
      </dl>
    </details>
  );
}

function SystemStatusPanel({
  status,
  jobs,
  progress,
  onPause,
  onCancel,
  onUseGpu,
  onOpenLibrary,
}: {
  status: SystemStatus | null;
  jobs: readonly StripJob[];
  progress: Readonly<Record<string, FolderProgress>>;
  onPause: (paused: boolean) => void;
  onCancel: (sourceId?: string) => void;
  onUseGpu: (useGpu: boolean) => void;
  onOpenLibrary: () => void;
}) {
  const [connection, setConnection] = useState<ConnectionSnapshot | null>(null);
  const [slack, setSlack] = useState<SlackState | null>(null);
  const [telegram, setTelegram] = useState<SlackState | null>(null);

  useEffect(() => {
    let live = true;
    invoke<unknown>('connection_status')
      .then(value => {
        if (live) setConnection(readSnapshot(value));
      })
      .catch(err => console.error('Failed to read the model status:', err));
    slackApi
      .status()
      .then(s => {
        if (live) setSlack(s.state);
      })
      .catch(err => console.error('Failed to read the Slack status:', err));
    telegramApi
      .status()
      .then(s => {
        if (live) setTelegram(s.state);
      })
      .catch(err => console.error('Failed to read the Telegram status:', err));
    return () => {
      live = false;
    };
  }, []);

  const search = searchSummary(status);
  const notice = cpuNotice(status);
  const pill = pillView(connection);
  const paused = status?.indexingPaused ?? false;
  const toggleId = useId();

  return (
    <>
      {notice && (
        <div
          role="status"
          className={cn(
            'mx-3.5 mt-3 rounded-lg border px-3 py-2.5 flex flex-col gap-1',
            notice.tone === 'bad' ? 'border-shodh-error/40 bg-shodh-error/10' : 'border-shodh-warning/40 bg-shodh-warning-soft',
          )}
        >
          <p className="m-0 text-small font-semibold text-shodh-text">{notice.title}</p>
          <p className="m-0 text-caption text-shodh-text-secondary">{notice.reason}</p>
          <p className="m-0 text-caption text-shodh-text-secondary">{notice.advice}</p>
        </div>
      )}
      <ul className="m-0 p-0 list-none flex flex-col divide-y divide-shodh-border-subtle max-h-[60vh] overflow-y-auto scrollbar-thin" aria-live="polite">
        <PartRow icon={Gauge} name="Search" tone={search.tone} state={search.sentence}>
          {status && <SearchDetails status={status} />}
          {status?.searchInstalled && (
            <label htmlFor={toggleId} className="flex items-center gap-2 text-caption text-shodh-text-secondary cursor-pointer w-fit">
              <input
                id={toggleId}
                type="checkbox"
                checked={status.useGpu}
                onChange={e => onUseGpu(e.target.checked)}
                className={cn('w-3.5 h-3.5 accent-shodh-accent', PANEL_FOCUS_RING)}
              />
              Use the graphics card for search
            </label>
          )}
        </PartRow>
        <PartRow
          icon={FileSearch}
          name="Reranker"
          tone={status?.rerankerInstalled ? 'good' : 'idle'}
          state={status?.rerankerInstalled ? 'Ready. Runs on the processor (CPU).' : 'Not installed. Results are ranked without it.'}
        />
        <PartRow
          icon={ScanText}
          name="Scan reader"
          tone={status?.scanReaderInstalled ? 'good' : 'idle'}
          state={
            status?.scanReaderInstalled
              ? 'Ready. Reads scanned pages on the processor (CPU).'
              : 'Not installed. Scanned pages are read by the system’s text recognition.'
          }
        />
        <PartRow
          icon={MessageSquare}
          name="Answer model"
          tone={pill.dot === 'green' ? 'good' : pill.dot === 'red' ? 'bad' : pill.dot === 'amber' ? 'warn' : 'idle'}
          state={pill.description}
        />
        <PartRow
          icon={Plug}
          name="Connectors"
          tone={connectorTone(slack) === 'bad' || connectorTone(telegram) === 'bad' ? 'bad' : connectorTone(slack) === 'good' || connectorTone(telegram) === 'good' ? 'good' : 'idle'}
          state={`Slack: ${connectorText(slack)}. Telegram: ${connectorText(telegram)}.`}
        />
        <PartRow
          icon={ListOrdered}
          name="Indexing"
          tone={paused ? 'warn' : jobs.length > 0 ? 'good' : 'idle'}
          state={indexingText(status, jobs.length) || 'Checking'}
        >
          {jobs.map(job => (
            <div key={job.id} className="flex items-center gap-2">
              <div className="min-w-0 flex-1 flex flex-col">
                <span className="text-caption font-medium text-shodh-text truncate">{job.name}</span>
                <span className="text-caption text-shodh-text-muted tabular-nums">
                  {jobDetail(job, progress[job.id])}
                  {progress[job.id]?.speed ? ` · ${countOf(Math.max(1, Math.round(progress[job.id].speed * 60)), 'file')} a minute` : ''}
                </span>
              </div>
              <button type="button" className={SMALL_BUTTON} onClick={() => onCancel(job.id)} aria-label={`Cancel indexing ${job.name}`}>
                <X className="w-3.5 h-3.5" aria-hidden="true" />
                Cancel
              </button>
            </div>
          ))}
          <div className="flex flex-wrap gap-1.5 pt-0.5">
            <button type="button" className={SMALL_BUTTON} onClick={() => onPause(!paused)}>
              {paused ? <Play className="w-3.5 h-3.5" aria-hidden="true" /> : <Pause className="w-3.5 h-3.5" aria-hidden="true" />}
              {paused ? 'Resume indexing' : 'Pause indexing'}
            </button>
            <button type="button" className={SMALL_BUTTON} onClick={onOpenLibrary}>
              <ListOrdered className="w-3.5 h-3.5" aria-hidden="true" />
              Open Library
            </button>
          </div>
          {paused && (
            <p className="m-0 text-caption text-shodh-text-muted">Paused indexing stays paused after Shodh restarts.</p>
          )}
        </PartRow>
      </ul>
    </>
  );
}

function panelPosition(button: HTMLElement, collapsed: boolean): React.CSSProperties {
  const rect = button.getBoundingClientRect();
  return collapsed
    ? { left: rect.right + PANEL_GAP, bottom: Math.max(PANEL_GAP, window.innerHeight - rect.bottom) }
    : { left: rect.left, bottom: window.innerHeight - rect.top + PANEL_GAP };
}

export function SystemStatusStrip({
  jobs,
  collapsed,
  onOpenLibrary,
}: {
  jobs: readonly StripJob[];
  collapsed: boolean;
  onOpenLibrary: () => void;
}) {
  const system = useSystemStatusContext();
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<React.CSSProperties | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLElement>(null);
  const panelId = useId();
  const headingId = useId();

  const status = system?.status ?? null;
  const refresh = system?.refresh;
  const progress = system?.progress ?? {};
  const search = searchSummary(status);
  const paused = status?.indexingPaused ?? false;

  // Read again when opened; closed by a click outside.
  useEffect(() => {
    if (!open) return;
    refresh?.();
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      if (rootRef.current?.contains(target) || panelRef.current?.contains(target)) return;
      setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open, refresh]);

  useEffect(() => {
    if (!open) {
      setPosition(null);
      return;
    }
    const place = () => {
      if (buttonRef.current) setPosition(panelPosition(buttonRef.current, collapsed));
    };
    place();
    window.addEventListener('resize', place);
    return () => window.removeEventListener('resize', place);
  }, [open, collapsed]);

  const shown = open && position !== null;
  useEffect(() => {
    if (shown) panelRef.current?.querySelector<HTMLElement>('button, summary, input')?.focus();
  }, [shown]);

  const close = (refocus: boolean) => {
    setOpen(false);
    if (refocus) buttonRef.current?.focus();
  };

  const run = useCallback(async (what: string, action: () => Promise<unknown>) => {
    try {
      await action();
    } catch (error) {
      notify.error(`Could not ${what}`, { description: errorText(error) });
    }
  }, []);

  const onPause = (next: boolean) =>
    void run(next ? 'pause indexing' : 'resume indexing', () => system?.setPaused(next) ?? Promise.resolve());
  const onCancel = (sourceId?: string) =>
    void run('cancel indexing', () => system?.cancel(sourceId) ?? Promise.resolve());
  const onUseGpu = (useGpu: boolean) =>
    void run(useGpu ? 'use the graphics card' : 'switch search to the processor', () => system?.setUseGpu(useGpu) ?? Promise.resolve());

  const job = jobs[0] ?? null;
  const jobProgress = job ? progress[job.id] : undefined;
  const percent = job ? Math.max(0, Math.min(100, Math.round(job.progress ?? 0))) : 0;
  const stripLabel = `System status. Search: ${search.sentence}${job ? ` ${indexingText(status, jobs.length)}.` : ''}`;
  const SearchIcon = status?.search?.accelerator === 'cpu' ? Cpu : Gauge;

  return (
    <div ref={rootRef} className={cn('flex flex-col gap-1', !collapsed && 'w-full min-w-0')}>
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        aria-label={stripLabel}
        title={search.sentence}
        className={cn(
          collapsed
            ? 'relative w-10 h-10 rounded-[10px] inline-flex items-center justify-center hover:bg-shodh-raised transition-colors duration-micro'
            : 'flex items-center gap-2 h-8 w-full px-3 rounded-[10px] border border-shodh-border text-left hover:bg-shodh-raised transition-colors duration-micro min-w-0',
          FOCUS_RING,
        )}
      >
        <SearchIcon className="w-4 h-4 shrink-0 text-shodh-text-muted" aria-hidden="true" />
        {!collapsed && (
          <span className="text-caption font-medium text-shodh-text-secondary truncate flex-1">Search · {search.short}</span>
        )}
        <ToneDot tone={search.tone} className={collapsed ? 'absolute top-1.5 right-1.5' : undefined} />
      </button>

      {job && !collapsed && (
        <div className="flex flex-col gap-1 px-1">
          <div className="flex items-center gap-1">
            <button
              type="button"
              onClick={onOpenLibrary}
              aria-label={`Indexing ${job.name}: ${jobDetail(job, jobProgress)}. Open Library`}
              className={cn('min-w-0 flex-1 flex flex-col text-left rounded-sm', FOCUS_RING)}
            >
              <span className="text-caption font-semibold text-shodh-text truncate">
                {paused ? 'Paused: ' : 'Indexing '}
                {job.name}
                {jobs.length > 1 ? ` and ${countOf(jobs.length - 1, 'more folder')}` : ''}
              </span>
              <span className="text-caption text-shodh-text-faint tabular-nums truncate">{jobDetail(job, jobProgress)}</span>
            </button>
            <button
              type="button"
              className={ICON_BUTTON}
              onClick={() => onPause(!paused)}
              aria-label={paused ? 'Resume indexing' : 'Pause indexing'}
              title={paused ? 'Resume indexing' : 'Pause indexing'}
            >
              {paused ? <Play className="w-3.5 h-3.5" aria-hidden="true" /> : <Pause className="w-3.5 h-3.5" aria-hidden="true" />}
            </button>
            <button
              type="button"
              className={ICON_BUTTON}
              onClick={() => onCancel(job.id)}
              aria-label={`Cancel indexing ${job.name}`}
              title="Cancel indexing"
            >
              <Square className="w-3 h-3" fill="currentColor" aria-hidden="true" />
            </button>
          </div>
          <div
            role="progressbar"
            aria-label={`Indexing ${job.name}`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent}
            className="h-[3px] rounded-[3px] bg-shodh-border overflow-hidden"
          >
            <div
              className={cn('h-full origin-left transition-transform duration-panel ease-standard', paused ? 'bg-shodh-text-faint' : 'bg-shodh-warning')}
              style={{ transform: `scaleX(${percent / 100})` }}
            />
          </div>
        </div>
      )}

      {open && position && createPortal(
        <section
          ref={panelRef}
          id={panelId}
          aria-labelledby={headingId}
          onKeyDown={e => {
            if (e.key === 'Escape') {
              e.preventDefault();
              close(true);
            }
          }}
          style={position}
          className="fixed z-50 w-[380px] max-w-[calc(100vw-24px)] rounded-xl border border-shodh-border bg-shodh-surface shadow-lg overflow-hidden"
        >
          <header className="flex items-center justify-between gap-2 px-3.5 py-2.5 border-b border-shodh-border-subtle">
            <h2 id={headingId} className="m-0 text-small font-semibold text-shodh-text">System status</h2>
            <button type="button" className={SMALL_BUTTON} onClick={() => close(true)}>
              <X className="w-3.5 h-3.5" aria-hidden="true" />
              Close
            </button>
          </header>
          <SystemStatusPanel
            status={status}
            jobs={jobs}
            progress={progress}
            onPause={onPause}
            onCancel={onCancel}
            onUseGpu={onUseGpu}
            onOpenLibrary={() => {
              close(false);
              onOpenLibrary();
            }}
          />
        </section>,
        document.body,
      )}
    </div>
  );
}

export default SystemStatusStrip;
