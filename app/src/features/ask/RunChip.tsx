import React, { useId, useState } from 'react';
import { ChevronDown, Lightbulb, Search, Wrench } from 'lucide-react';
import { cn } from '../../lib/utils';
import { formatDuration, summarizeRun } from './run';
import type { ResponseMetadata, RunRecord, RunStep } from './types';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const KIND_ICON: Record<RunStep['kind'], React.ElementType> = {
  search: Search,
  tool: Wrench,
  thinking: Lightbulb,
};

const KIND_BADGE: Record<RunStep['kind'], string> = {
  search: 'bg-shodh-raised-2 text-shodh-info',
  tool: 'bg-shodh-success-soft text-shodh-success',
  thinking: 'bg-shodh-raised-2 text-shodh-violet',
};

function stepDotClass(step: RunStep): string {
  if (step.status === 'running') return 'bg-shodh-accent-text ask-breathe';
  if (step.status === 'failed') return 'bg-shodh-error';
  if (step.status === 'stopped') return 'bg-shodh-text-faint';
  if (step.kind === 'search') return 'bg-shodh-info';
  if (step.kind === 'thinking') return 'bg-shodh-violet';
  return 'bg-shodh-success';
}

function stepMeta(step: RunStep): string | null {
  const parts: string[] = [];
  if (step.meta) parts.push(step.meta);
  if (typeof step.durationMs === 'number') parts.push(formatDuration(step.durationMs));
  if (step.status === 'failed') parts.push('failed');
  if (step.status === 'stopped') parts.push('stopped');
  return parts.length > 0 ? parts.join(' · ') : null;
}

interface RunChipProps {
  run: RunRecord;
  metadata?: ResponseMetadata;
  passageCount: number;
}

/**
 * Collapsed summary of how an answer was produced, expanding to a timeline.
 * Every value shown comes from streaming events or the final response; no
 * step is inferred or invented.
 */
export function RunChip({ run, metadata, passageCount }: RunChipProps) {
  const running = run.status === 'running';
  const [userToggled, setUserToggled] = useState<boolean | null>(null);
  const timelineId = useId();

  const hasSteps = run.steps.length > 0;
  const open = hasSteps && (userToggled ?? running);
  const summary = summarizeRun(run, metadata, passageCount);
  const kinds = (['search', 'tool', 'thinking'] as const).filter(kind => run.steps.some(s => s.kind === kind));

  const chipBody = (
    <>
      {running ? (
        <>
          <span className="ml-1 w-2 h-2 rounded-full bg-shodh-accent-text ask-breathe shrink-0" aria-hidden="true" />
          <span className="font-medium text-shodh-text ask-breathe-soft" aria-live="polite">
            {run.activity ?? 'Working'}…
          </span>
        </>
      ) : (
        <>
          {kinds.length > 0 && (
            <span className="inline-flex shrink-0" aria-hidden="true">
              {kinds.map((kind, i) => {
                const Icon = KIND_ICON[kind];
                return (
                  <span
                    key={kind}
                    className={cn(
                      'w-5 h-5 rounded-full inline-flex items-center justify-center border-2 border-shodh-surface',
                      KIND_BADGE[kind],
                      i > 0 && '-ml-1.5',
                    )}
                  >
                    <Icon className="w-2.5 h-2.5" strokeWidth={3} />
                  </span>
                );
              })}
            </span>
          )}
          <span className={cn(run.status === 'failed' && 'text-shodh-error')}>{summary.headline}</span>
          {summary.details.length > 0 && (
            <span className="text-shodh-text-faint">{`· ${summary.details.join(' · ')}`}</span>
          )}
        </>
      )}
    </>
  );

  const chipClass =
    'self-start inline-flex items-center gap-2.5 min-h-[32px] py-1.5 pl-2 pr-3 rounded-full border border-shodh-border bg-shodh-surface text-[13px] text-shodh-text-secondary max-w-full';

  return (
    <div className="flex flex-col gap-3">
      {hasSteps ? (
        <button
          type="button"
          onClick={() => setUserToggled(!open)}
          aria-expanded={open}
          aria-controls={timelineId}
          className={cn(chipClass, 'hover:bg-shodh-raised transition-colors duration-micro', FOCUS_RING)}
        >
          {chipBody}
          <ChevronDown
            className={cn('w-3 h-3 shrink-0 text-shodh-text-faint transition-transform duration-panel ease-standard', open && 'rotate-180')}
            strokeWidth={2.4}
            aria-hidden="true"
          />
        </button>
      ) : (
        <div className={chipClass}>{chipBody}</div>
      )}

      {open && (
        <ol
          id={timelineId}
          aria-label="Run steps"
          className="ask-rise list-none -mt-1 ml-0 py-1 pl-[18px] border-l border-shodh-border flex flex-col gap-3"
        >
          {run.steps.map(step => {
            const meta = stepMeta(step);
            return (
              <li key={step.id} className="ask-rise flex items-baseline gap-2.5 text-[13px] min-w-0">
                <span
                  className={cn('w-[7px] h-[7px] rounded-full shrink-0 -mr-[7px] -translate-x-[22px] -translate-y-px', stepDotClass(step))}
                  aria-hidden="true"
                />
                <span className="font-semibold text-shodh-text whitespace-nowrap">{step.title}</span>
                {step.detail && (
                  <span className="text-shodh-text-muted min-w-0 truncate" title={step.detail}>{step.detail}</span>
                )}
                {meta && (
                  <span className="ml-auto pl-2 font-mono text-[11px] text-shodh-text-faint whitespace-nowrap">{meta}</span>
                )}
                {step.status === 'running' && <span className="sr-only">(in progress)</span>}
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
}

export default RunChip;
