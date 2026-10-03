import React, { useEffect, useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { cn } from '../../lib/utils';
import { formatCost, formatMs, formatTokens, shortModel } from './format';
import { currentStep, isLive, pendingApproval } from './reducer';
import type { TranscriptState } from './reducer';

/** Re-renders once a second while `active` (only this component ticks). */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

interface StatusLineProps {
  transcript: TranscriptState;
  /** Shown before the run reports its model. */
  fallbackModel: string | null;
  compact?: boolean;
}

/**
 * Sticky run status: model · tokens in/out · cost · elapsed, and the key
 * hint for whatever the run is waiting on.
 */
export function StatusLine({ transcript, fallbackModel, compact = false }: StatusLineProps) {
  const live = isLive(transcript);
  const now = useNow(live);
  const elapsed = live ? now - transcript.startedAtMs : transcript.durationMs ?? 0;
  const model = transcript.model ?? fallbackModel;
  const usage = transcript.usage;
  const cost = usage ? formatCost(usage.costUsd) : null;
  const approval = pendingApproval(transcript);
  const step = currentStep(transcript);

  let hint: string | null = null;
  if (approval) hint = approval.tier === 'write' ? 'Enter to approve · Esc to deny' : 'Esc to deny';
  else if (transcript.interrupting) hint = 'Interrupting…';
  else if (live) hint = 'Esc to interrupt';

  let state: string;
  switch (transcript.status) {
    case 'starting':
      state = 'Starting agent';
      break;
    case 'running':
      state = approval ? 'Waiting for you' : step ? step.label : 'Working';
      break;
    case 'completed':
      state = 'Done';
      break;
    case 'aborted':
      state = 'Interrupted';
      break;
    default:
      state = 'Failed';
  }

  const parts: string[] = [];
  if (model) parts.push(shortModel(model));
  if (usage) parts.push(`↑ ${formatTokens(usage.inputTokens)} ↓ ${formatTokens(usage.outputTokens)}`);
  if (cost) parts.push(cost);
  parts.push(formatMs(elapsed));

  return (
    <div className="flex flex-col gap-1">
      {transcript.warning && (
        <p role="note" className="flex items-center gap-1.5 text-[11.5px] text-shodh-warning">
          <AlertTriangle className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
          <span className="min-w-0 truncate" title={transcript.warning}>{transcript.warning}</span>
        </p>
      )}
      <div
        className={cn(
          'flex items-center gap-2 min-w-0 font-mono text-shodh-text-muted',
          compact ? 'text-[10.5px]' : 'text-[11.5px]',
        )}
      >
        <span
          className={cn(
            'w-[7px] h-[7px] rounded-full shrink-0',
            live ? 'bg-shodh-accent-text ask-breathe' : transcript.status === 'completed' ? 'bg-shodh-success' : transcript.status === 'aborted' ? 'bg-shodh-text-faint' : 'bg-shodh-error',
          )}
          aria-hidden="true"
        />
        <span className={cn('min-w-0 truncate', live ? 'text-shodh-text-secondary' : undefined)} aria-live="polite">
          {state}
        </span>
        <span className="shrink-0 text-shodh-text-faint" aria-hidden="true">·</span>
        <span className="min-w-0 truncate tabular-nums">{parts.join(' · ')}</span>
        {hint && <span className="ml-auto pl-2 shrink-0 text-shodh-text-faint">{hint}</span>}
      </div>
    </div>
  );
}

export default StatusLine;
