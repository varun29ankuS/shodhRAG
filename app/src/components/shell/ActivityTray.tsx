import React, { useEffect, useId, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Activity, Bot, Pause, Play, Square } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { formatMs } from '../../features/agent/format';
import { currentStep, pendingApproval } from '../../features/agent/reducer';
import type { TranscriptState } from '../../features/agent/reducer';
import { useChatSession } from '../../features/ask/ChatSessionContext';
import { useSessionCounts } from '../../features/agent/useAgentSession';
import { sessionCountsLabel } from '../../features/agent/sessionCounts';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-sidebar';

export interface IndexingJob {
  id: string;
  name: string;
  progress?: number;
  fileCount?: number;
  processedCount?: number;
}

function runActivity(transcript: TranscriptState): string {
  if (transcript.status === 'starting') return 'Starting agent…';
  if (transcript.interrupting) return 'Interrupting…';
  if (pendingApproval(transcript)) return 'Waiting for your approval';
  const step = currentStep(transcript);
  if (step) return step.label;
  return transcript.blocks.some(b => b.kind === 'text') ? 'Writing the answer' : 'Working…';
}

function useTick(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

interface ActivityTrayProps {
  jobs: readonly IndexingJob[];
  onOpenConversation: (id: string) => void;
}

/**
 * Everything running right now: the agent's answer and indexing jobs, from a
 * footer button with a running count, and how many assistant sessions (one
 * process each) are open.
 */
export function ActivityTray({ jobs, onOpenConversation }: ActivityTrayProps) {
  const { liveRun, conversations, cancel } = useChatSession();
  const sessions = useSessionCounts();
  const [open, setOpen] = useState(false);
  const [paused, setPaused] = useState(false);
  const panelId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const now = useTick(open && liveRun !== null);
  const count = (liveRun ? 1 : 0) + jobs.length;

  useEffect(() => {
    if (jobs.length === 0) setPaused(false);
  }, [jobs.length]);

  // Close on outside click and on Esc (focus returns to the button).
  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      e.preventDefault();
      setOpen(false);
      buttonRef.current?.focus();
    };
    window.addEventListener('pointerdown', onPointer);
    window.addEventListener('keydown', onKey, true);
    return () => {
      window.removeEventListener('pointerdown', onPointer);
      window.removeEventListener('keydown', onKey, true);
    };
  }, [open]);

  const togglePause = async () => {
    const command = paused ? 'resume_indexing' : 'pause_indexing';
    try {
      await invoke(command);
      setPaused(!paused);
    } catch (error) {
      notify.error(paused ? 'Could not resume indexing' : 'Could not pause indexing', {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const conversationTitle = liveRun
    ? conversations.find(c => c.id === liveRun.conversationId)?.title ?? 'Conversation'
    : '';
  const label = count === 0 ? 'Activity: nothing running' : `Activity: ${count} running`;

  return (
    <div ref={rootRef} className="relative">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-label={label}
        title={label}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        className={cn(
          'relative w-8 h-8 rounded-lg inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
          open && 'bg-shodh-raised text-shodh-text',
          FOCUS_RING,
        )}
      >
        <Activity className="w-4 h-4" aria-hidden="true" />
        {count > 0 && (
          <span
            className="absolute -top-0.5 -right-0.5 min-w-[15px] h-[15px] px-[3px] rounded-full bg-shodh-accent text-shodh-on-accent text-[9.5px] font-bold leading-[15px] text-center tabular-nums"
            aria-hidden="true"
          >
            {count}
          </span>
        )}
      </button>

      {open && (
        <div
          id={panelId}
          role="dialog"
          aria-label="Running now"
          className="ask-rise absolute z-50 bottom-full left-0 mb-2 w-[300px] rounded-xl border border-shodh-border-strong bg-shodh-surface shadow-[0_12px_36px_rgba(0,0,0,0.32)] p-2 flex flex-col gap-1"
        >
          <p className="px-2 pt-1 pb-0.5 text-[11px] font-semibold uppercase tracking-wide text-shodh-text-faint">Running now</p>
          {count === 0 && <p className="px-2 py-2 text-[12.5px] text-shodh-text-muted">Nothing is running.</p>}

          {liveRun && (
            <div className="flex items-start gap-2 rounded-lg px-2 py-2 hover:bg-shodh-raised">
              <span className="mt-[5px] w-2 h-2 rounded-full shrink-0 bg-shodh-accent-text ask-breathe" aria-hidden="true" />
              <button
                type="button"
                onClick={() => {
                  onOpenConversation(liveRun.conversationId);
                  setOpen(false);
                }}
                className={cn('min-w-0 flex-1 text-left rounded-sm', FOCUS_RING)}
              >
                <span className="block text-[12.5px] font-medium text-shodh-text truncate">{conversationTitle}</span>
                <span className="block text-[11.5px] text-shodh-text-muted truncate">{runActivity(liveRun.transcript)}</span>
                <span className="block text-[11px] font-mono text-shodh-text-faint tabular-nums">
                  {formatMs(Math.max(0, now - liveRun.transcript.startedAtMs))}
                </span>
              </button>
              <button
                type="button"
                onClick={cancel}
                aria-label={`Stop the answer in ${conversationTitle}`}
                title="Stop"
                className={cn(
                  'shrink-0 w-7 h-7 inline-flex items-center justify-center rounded-md text-shodh-text-muted hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro',
                  FOCUS_RING,
                )}
              >
                <Square className="w-3 h-3" fill="currentColor" aria-hidden="true" />
              </button>
            </div>
          )}

          {sessions && (
            <div className="flex items-start gap-2 px-2 py-1.5" role="status" aria-live="polite">
              <Bot className="w-3.5 h-3.5 mt-[2px] shrink-0 text-shodh-text-faint" aria-hidden="true" />
              <div className="min-w-0 flex flex-col">
                <span className="text-[12px] text-shodh-text-secondary tabular-nums">{sessionCountsLabel(sessions)}</span>
                {sessions.side > 0 && (
                  <span className="text-[11px] text-shodh-text-faint">Side discussions close after 5 minutes idle.</span>
                )}
              </div>
            </div>
          )}

          {jobs.map(job => {
            const percent = Math.max(0, Math.min(100, Math.round(job.progress ?? 0)));
            const total = job.fileCount ?? 0;
            const processed = job.processedCount ?? 0;
            return (
              <div key={job.id} className="flex items-start gap-2 rounded-lg px-2 py-2">
                <span className="mt-[5px] w-2 h-2 rounded-full shrink-0 bg-shodh-warning" aria-hidden="true" />
                <div className="min-w-0 flex-1 flex flex-col gap-1">
                  <span className="text-[12.5px] font-medium text-shodh-text truncate">{`Indexing ${job.name}`}</span>
                  <div
                    role="progressbar"
                    aria-label={`Indexing ${job.name}`}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={percent}
                    className="h-1 rounded-full bg-shodh-raised-2 overflow-hidden"
                  >
                    <div
                      className="h-full w-full origin-left bg-shodh-warning transition-transform duration-panel"
                      style={{ transform: `scaleX(${percent / 100})` }}
                    />
                  </div>
                  <span className="text-[11px] font-mono text-shodh-text-faint tabular-nums">
                    {paused ? 'Paused · ' : ''}
                    {total > 0 ? `${processed.toLocaleString()} of ${total.toLocaleString()} files · ${percent}%` : 'Preparing files'}
                  </span>
                </div>
                <button
                  type="button"
                  onClick={togglePause}
                  aria-label={paused ? `Resume indexing ${job.name}` : `Pause indexing ${job.name}`}
                  title={paused ? 'Resume' : 'Pause'}
                  className={cn(
                    'shrink-0 w-7 h-7 inline-flex items-center justify-center rounded-md text-shodh-text-muted hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro',
                    FOCUS_RING,
                  )}
                >
                  {paused ? <Play className="w-3 h-3" aria-hidden="true" /> : <Pause className="w-3 h-3" aria-hidden="true" />}
                </button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

export default ActivityTray;
