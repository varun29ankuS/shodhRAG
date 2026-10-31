import React, { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { ChevronRight, Square } from 'lucide-react';
import { cn } from '../../lib/utils';
import {
  commandElapsedMs,
  commandFailed,
  commandStateLabel,
  commandSummary,
  formatElapsed,
  formatTimeLimit,
} from './commandRun';
import type { CommandRun } from './commandRun';
import { agentApi, toAgentError } from './useAgentSession';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-ground';

/** Distance from the bottom (px) within which the output keeps following new text. */
const FOLLOW_SLACK_PX = 24;

/**
 * A command's output: scrolls inside a fixed height and keeps itself at the
 * bottom as text arrives, unless the user scrolled up to read.
 */
function CommandOutput({ run, id }: { run: CommandRun; id?: string }) {
  const ref = useRef<HTMLPreElement>(null);
  const follow = useRef(true);

  useLayoutEffect(() => {
    const el = ref.current;
    if (el && follow.current) el.scrollTop = el.scrollHeight;
  }, [run.output]);

  return (
    <div id={id} className="flex flex-col gap-1">
      {run.outputCut && <p className="m-0 text-caption text-shodh-text-faint">Earlier output not shown</p>}
      <pre
        ref={ref}
        tabIndex={0}
        aria-label="Command output"
        onScroll={e => {
          const el = e.currentTarget;
          follow.current = el.scrollHeight - el.scrollTop - el.clientHeight <= FOLLOW_SLACK_PX;
        }}
        className={cn(
          'm-0 max-h-64 overflow-auto scrollbar-thin rounded-md bg-shodh-raised px-2.5 py-2 font-mono text-caption leading-[1.5] text-shodh-text-secondary whitespace-pre-wrap break-words',
          FOCUS_RING,
        )}
      >
        {run.output}
      </pre>
    </div>
  );
}

/**
 * The live status of a Code-mode command (`run_command`), under its step:
 * while it runs, the command, its running time, its output as it arrives and
 * a Stop button; once it ends, one line saying how, with the output behind a
 * toggle.
 */
export function CommandPanel({ run, compact = false }: { run: CommandRun; compact?: boolean }) {
  const running = run.state === 'running';
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [stopping, setStopping] = useState(false);
  const [stopError, setStopError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const outputId = useId();
  const mounted = useRef(true);
  // Whether Stop had focus: when the command ends the button goes, and focus
  // moves to the line that says how it ended instead of to the page.
  const stopFocused = useRef(false);
  const summaryRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  // The running time ticks once a second, only while the command runs.
  useEffect(() => {
    if (!running) return undefined;
    setNowMs(Date.now());
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [running]);

  // A new state (or another command) ends any stop in flight.
  useEffect(() => {
    setStopping(false);
    setStopError(null);
  }, [run.state, run.commandId]);

  useLayoutEffect(() => {
    if (running || !stopFocused.current) return;
    stopFocused.current = false;
    summaryRef.current?.focus({ preventScroll: true });
  }, [running]);

  const stop = useCallback(() => {
    setStopping(true);
    setStopError(null);
    agentApi
      .stopCommand(run.commandId)
      .then(found => {
        if (!mounted.current || found) return;
        setStopping(false);
        setStopError('The command had already ended.');
      })
      .catch(err => {
        if (!mounted.current) return;
        setStopping(false);
        setStopError(`It could not be stopped: ${toAgentError(err).message}`);
      });
  }, [run.commandId]);

  const textSize = compact ? 'text-caption' : 'text-small';
  const stateLabel = running && stopping ? 'Stopping' : commandStateLabel(run);

  return (
    <div className="min-w-0">
      {/* One live region for the whole life of the command, so its end is announced. */}
      <span className="sr-only" aria-live="polite">{`Command ${stateLabel}`}</span>
      {running ? (
        <RunningCommand
          run={run}
          nowMs={nowMs}
          stateLabel={stateLabel}
          stopping={stopping}
          stopError={stopError}
          onStop={stop}
          onStopFocus={focused => { stopFocused.current = focused; }}
          textSize={textSize}
        />
      ) : (
        <div ref={summaryRef} tabIndex={-1} className="rounded-sm focus-visible:outline-none">
          <FinishedCommand run={run} open={open} onToggle={() => setOpen(o => !o)} outputId={outputId} textSize={textSize} />
        </div>
      )}
    </div>
  );
}

interface RunningCommandProps {
  run: CommandRun;
  nowMs: number;
  stateLabel: string;
  stopping: boolean;
  stopError: string | null;
  onStop: () => void;
  onStopFocus: (focused: boolean) => void;
  textSize: string;
}

/** Share of the time limit used, 0..1 (0 when the limit is unknown). */
function limitUsed(run: CommandRun, nowMs: number): number {
  if (run.timeLimitSecs === null || run.timeLimitSecs <= 0) return 0;
  return Math.min(1, commandElapsedMs(run, nowMs) / (run.timeLimitSecs * 1000));
}

function RunningCommand({ run, nowMs, stateLabel, stopping, stopError, onStop, onStopFocus, textSize }: RunningCommandProps) {
  const used = limitUsed(run, nowMs);
  // The last fifth of the time limit shows as a warning.
  const nearLimit = used >= 0.8;
  return (
    <div className="mt-1 ml-[18px] rounded-lg border border-shodh-border-subtle bg-shodh-surface px-3 py-2 flex flex-col gap-1.5 min-w-0">
      <div className="flex items-start gap-2 min-w-0">
        <code className={cn('flex-1 min-w-0 font-mono text-shodh-text break-all line-clamp-3', textSize)} title={run.command}>
          {run.command}
        </code>
        <button
          type="button"
          onClick={() => { if (!stopping) onStop(); }}
          onFocus={() => onStopFocus(true)}
          onBlur={() => onStopFocus(false)}
          aria-disabled={stopping}
          className={cn(
            'shrink-0 inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md text-caption font-medium text-shodh-text bg-shodh-raised-2 hover:bg-shodh-pressed aria-disabled:opacity-60 aria-disabled:cursor-not-allowed transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          <Square className="w-3 h-3 fill-current" aria-hidden="true" />
          {stopping ? 'Stopping…' : 'Stop'}
        </button>
      </div>
      <p className={cn('m-0 flex flex-wrap items-baseline gap-x-1.5 font-mono text-shodh-text-muted', textSize)}>
        <span className="inline-flex items-center gap-1.5 text-shodh-accent-text">
          <span
            aria-hidden="true"
            className={cn('w-1.5 h-1.5 rounded-full bg-shodh-accent', !stopping && 'animate-pulse motion-reduce:animate-none')}
          />
          {stateLabel}
        </span>
        <span aria-hidden="true">·</span>
        <span>
          <span className="sr-only">for </span>
          {formatElapsed(commandElapsedMs(run, nowMs))}
        </span>
        {run.timeLimitSecs !== null && (
          <span className="text-shodh-text-faint">{`(limit ${formatTimeLimit(run.timeLimitSecs)})`}</span>
        )}
      </p>
      {run.timeLimitSecs !== null && (
        // How much of the time limit is used; the text above says it in words.
        <div aria-hidden="true" className="h-0.5 w-full rounded-full bg-shodh-raised-2 overflow-hidden">
          <div
            className={cn(
              'h-full rounded-full transition-[width] duration-1000 ease-linear motion-reduce:transition-none',
              nearLimit ? 'bg-shodh-warning' : 'bg-shodh-accent',
            )}
            style={{ width: `${(used * 100).toFixed(2)}%` }}
          />
        </div>
      )}
      {run.sandbox && <p className="m-0 text-caption text-shodh-text-muted">{run.sandbox}</p>}
      {stopError && <p role="alert" className="m-0 text-caption text-shodh-error">{stopError}</p>}
      {run.output.length > 0 || run.outputCut ? (
        <CommandOutput run={run} />
      ) : (
        <p className="m-0 text-caption text-shodh-text-faint">No output yet</p>
      )}
    </div>
  );
}

interface FinishedCommandProps {
  run: CommandRun;
  open: boolean;
  onToggle: () => void;
  outputId: string;
  textSize: string;
}

function FinishedCommand({ run, open, onToggle, outputId, textSize }: FinishedCommandProps) {
  const failed = commandFailed(run);
  const hasOutput = run.output.length > 0 || run.outputCut;
  // A killed or refused command names its limit or reason in its label already.
  const note = run.note && run.state !== 'killed' && run.state !== 'refused' ? run.note : null;
  const summary = (
    <span className={cn('min-w-0 break-words', failed ? 'text-shodh-error' : 'text-shodh-text-muted')}>
      {commandSummary(run)}
    </span>
  );

  return (
    <div className={cn('ml-[3px] flex flex-col gap-1 min-w-0 font-mono', textSize)}>
      <div className="flex items-start gap-1.5 min-w-0">
        <span aria-hidden="true" className="text-shodh-text-faint">⎿</span>
        {hasOutput ? (
          <button
            type="button"
            onClick={onToggle}
            aria-expanded={open}
            aria-controls={outputId}
            className={cn('flex items-start gap-1.5 min-w-0 text-left rounded-sm hover:text-shodh-text', FOCUS_RING)}
          >
            {summary}
            <span className="sr-only">{open ? ', hide output' : ', show output'}</span>
            <ChevronRight
              className={cn('w-3 h-3 mt-[3px] shrink-0 text-shodh-text-faint transition-transform duration-micro', open && 'rotate-90')}
              aria-hidden="true"
            />
          </button>
        ) : (
          summary
        )}
      </div>
      {note && <p className="m-0 ml-[18px] text-shodh-text-muted break-words">{note}</p>}
      {open && hasOutput && (
        <div className="ml-[15px] rounded-lg border border-shodh-border-subtle bg-shodh-surface px-3 py-2 flex flex-col gap-1.5 min-w-0">
          <code className="font-mono text-shodh-text break-all">{run.command}</code>
          <CommandOutput run={run} id={outputId} />
        </div>
      )}
    </div>
  );
}

export default CommandPanel;
