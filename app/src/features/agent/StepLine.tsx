import React, { useId, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { cn } from '../../lib/utils';
import { ApprovalPrompt } from './ApprovalPrompt';
import { formatMs, prettyJson } from './format';
import { passagesFromDetail, webSourcesFromDetail } from './reducer';
import type { TranscriptStep } from './reducer';
import { AttributionFrame, WebSources } from './WebSources';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-ground';

/** Rotating ring while a step runs (static under reduced motion). */
export function Spinner({ className }: { className?: string }) {
  return (
    <span
      className={cn(
        'inline-block w-[9px] h-[9px] rounded-full border-[1.5px] border-shodh-accent-text border-t-transparent agent-spin',
        className,
      )}
      aria-hidden="true"
    />
  );
}

function hasContent(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (typeof value === 'object' && !Array.isArray(value)) return Object.keys(value as object).length > 0;
  if (Array.isArray(value)) return value.length > 0;
  return true;
}

function detailString(detail: unknown, key: string): string | null {
  if (typeof detail !== 'object' || detail === null) return null;
  const value = (detail as Record<string, unknown>)[key];
  return typeof value === 'string' && value.length > 0 ? value : null;
}

function DetailView({ step }: { step: TranscriptStep }) {
  const webSources = webSourcesFromDetail(step.detail);
  const passages = passagesFromDetail(step.detail).filter(p => !p.web);
  return (
    <div className="mt-1 ml-[18px] rounded-lg border border-shodh-border-subtle bg-shodh-surface px-3 py-2 font-mono text-[11.5px] leading-[1.55] text-shodh-text-secondary overflow-x-auto scrollbar-thin">
      {hasContent(step.args) && (
        <>
          <p className="text-shodh-text-faint">{step.tool}</p>
          <pre className="m-0 whitespace-pre-wrap break-words">{prettyJson(step.args)}</pre>
        </>
      )}
      {webSources.length > 0 ? (
        <WebSources sources={webSources} provider={detailString(step.detail, 'provider')} />
      ) : passages.length > 0 ? (
        <ol className="mt-1.5 list-none p-0 flex flex-col gap-0.5" aria-label="Passages found">
          {passages.map(p => (
            <li key={p.n} className="whitespace-nowrap">
              <span className="text-shodh-text">{`[${p.n}]`}</span>
              {` ${p.file}`}
              {p.page ? ` · p.${p.page}` : ''}
              <span className="text-shodh-text-faint">{` · ${p.score.toFixed(3)}`}</span>
            </li>
          ))}
        </ol>
      ) : (
        hasContent(step.detail) && (
          <>
            <p className="mt-1.5 text-shodh-text-faint">result</p>
            <pre className="m-0 whitespace-pre-wrap break-words">{prettyJson(step.detail)}</pre>
          </>
        )
      )}
    </div>
  );
}

interface StepLineProps {
  step: TranscriptStep;
  steps: Record<string, TranscriptStep>;
  onDecide?: (stepId: string, approved: boolean) => void;
  compact?: boolean;
  depth?: number;
}

/**
 * One tool step, Claude Code style:
 *
 *   ● Searching “notice period”
 *     ⎿  8 passages from 3 files · 1.2s
 *
 * The label row expands to the monospace arguments and result. Sub-steps
 * are indented under their parent.
 */
export function StepLine({ step, steps, onDecide, compact = false, depth = 0 }: StepLineProps) {
  const [open, setOpen] = useState(false);
  const detailId = useId();
  const running = step.status === 'running';
  const waiting = step.status === 'awaiting_approval';
  const failed = step.status === 'failed';
  const expandable = hasContent(step.args) || hasContent(step.detail);
  const attribution = step.status === 'done' ? detailString(step.detail, 'attributionHtml') : null;

  const marker = running ? (
    <Spinner className="mt-[5px]" />
  ) : (
    <span
      className={cn(
        'mt-[5px] w-[9px] h-[9px] rounded-full shrink-0',
        failed ? 'bg-shodh-error' : waiting ? 'bg-shodh-warning ask-breathe' : step.tier === 'read' ? 'bg-shodh-success' : 'bg-shodh-info',
      )}
      aria-hidden="true"
    />
  );

  const resultParts: string[] = [];
  if (step.summary) resultParts.push(step.summary);
  if (step.durationMs !== null && !running && !waiting) resultParts.push(formatMs(step.durationMs));
  const result = running ? step.progress : resultParts.join(' · ');
  const stateText = running ? 'running' : waiting ? 'waiting for your approval' : failed ? 'failed' : 'done';

  const labelBody = (
    <>
      <span className={cn('font-medium text-shodh-text break-words', running && 'ask-breathe-soft')}>{step.label}</span>
      <span className="sr-only">{` (${stateText})`}</span>
      {expandable && (
        <ChevronRight
          className={cn('w-3 h-3 mt-[3px] shrink-0 text-shodh-text-faint transition-transform duration-micro', open && 'rotate-90')}
          aria-hidden="true"
        />
      )}
    </>
  );

  return (
    <div className={cn('ask-rise flex flex-col min-w-0', depth > 0 && 'ml-[18px] pl-2.5 border-l border-shodh-border-subtle')}>
      <div className={cn('flex items-start gap-2 min-w-0', compact ? 'text-[12.5px]' : 'text-[13.5px]')}>
        {marker}
        {expandable ? (
          <button
            type="button"
            onClick={() => setOpen(o => !o)}
            aria-expanded={open}
            aria-controls={detailId}
            className={cn('flex items-start gap-1.5 min-w-0 text-left rounded-sm hover:text-shodh-text', FOCUS_RING)}
          >
            {labelBody}
          </button>
        ) : (
          <span className="flex items-start gap-1.5 min-w-0">{labelBody}</span>
        )}
      </div>
      {result && (
        <p
          className={cn(
            'ml-[3px] flex gap-1.5 min-w-0 font-mono',
            compact ? 'text-[11px]' : 'text-[12px]',
            failed ? 'text-shodh-error' : 'text-shodh-text-muted',
          )}
        >
          <span aria-hidden="true" className="text-shodh-text-faint">⎿</span>
          <span className={cn('min-w-0', running ? 'truncate' : 'break-words')}>{result}</span>
        </p>
      )}
      {open && <div id={detailId}><DetailView step={step} /></div>}
      {attribution && <AttributionFrame html={attribution} />}
      {step.approval && onDecide && (
        <div className="mt-2 ml-[17px]">
          <ApprovalPrompt approval={step.approval} onDecide={approved => onDecide(step.id, approved)} compact={compact} />
        </div>
      )}
      {step.children.length > 0 && (
        <div className="mt-1.5 flex flex-col gap-1.5">
          {step.children.map(childId => {
            const child = steps[childId];
            return child ? (
              <StepLine key={childId} step={child} steps={steps} onDecide={onDecide} compact={compact} depth={depth + 1} />
            ) : null;
          })}
        </div>
      )}
    </div>
  );
}

export default StepLine;
