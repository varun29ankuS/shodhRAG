import React, { useId, useMemo } from 'react';
import { AlertTriangle, ChevronRight, CircleCheck, CircleHelp, ShieldCheck } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { SearchHit } from '../ask/types';
import { sourceLabel } from '../ask/searchResults';
import type { ClaimCheck, GroundingReport } from './events';
import {
  flagDescription,
  flagLabel,
  isFlagged,
  methodLabel,
  missingNeeds,
  summaryDescription,
  summaryLabel,
  summaryTone,
} from './grounding';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

/** "4, contract.pdf p.3": the closest passage as words. */
function hitLabel(hit: SearchHit): string {
  return `${hit.number}, ${sourceLabel(hit)}`;
}

interface ClaimFlagProps {
  check: ClaimCheck;
  /** The passage that comes closest to supporting the claim, when there is one. */
  closest: SearchHit | null;
  onOpenCitation: (hit: SearchHit, trigger: HTMLElement) => void;
}

/**
 * Inline flag after a statement the grounding check could not ground: icon,
 * short text, a tooltip and a full description for screen readers. With a
 * closest passage it is a button that opens it.
 */
export function ClaimFlag({ check, closest, onOpenCitation }: ClaimFlagProps) {
  const label = flagLabel(check);
  const description = flagDescription(check, closest ? hitLabel(closest) : null);
  const className = cn(
    'inline-flex items-center gap-0.5 ml-1 px-1 h-[18px] rounded-[5px] align-[2px] text-[10.5px] font-medium leading-none border border-dashed',
    check.outcome === 'uncited_factual'
      ? 'border-shodh-warning/60 text-shodh-warning'
      : 'border-shodh-error/60 text-shodh-error',
  );
  const body = (
    <>
      <AlertTriangle className="w-2.5 h-2.5 shrink-0" aria-hidden="true" />
      <span aria-hidden="true">{label}</span>
      <span className="sr-only">{description}</span>
    </>
  );
  if (closest) {
    return (
      <button
        type="button"
        onClick={e => onOpenCitation(closest, e.currentTarget)}
        title={`${description} Click to open it.`}
        className={cn(className, 'hover:bg-shodh-raised-2 transition-colors duration-micro', FOCUS_RING)}
      >
        {body}
      </button>
    );
  }
  return (
    <span role="note" title={description} className={className}>
      {body}
    </span>
  );
}

/** A citation number that matches no source of the answer: never a working pill. */
export function InvalidCitation({ number }: { number: number }) {
  const description = `Citation ${number} does not match any source of this answer.`;
  return (
    <span
      role="note"
      title={description}
      className="inline-flex items-center justify-center min-w-[18px] h-[18px] px-1 ml-[3px] rounded-[5px] align-[3px] text-[10.5px] font-bold leading-none tabular-nums border border-dashed border-shodh-error/60 text-shodh-error line-through decoration-shodh-error/70"
    >
      <span aria-hidden="true">{number}</span>
      <span className="sr-only">{description}</span>
    </span>
  );
}

interface GroundingChipProps {
  report: GroundingReport;
  hits: readonly SearchHit[];
  onOpenCitation: (hit: SearchHit, trigger: HTMLElement) => void;
  compact?: boolean;
}

/**
 * "Grounded 14/16 ▸": how many checked statements their sources support.
 * Expands into the flagged statements, each with the passage that comes
 * closest, and the parts of the question no passage covered.
 */
export function GroundingChip({ report, hits, onOpenCitation, compact = false }: GroundingChipProps) {
  const listId = useId();
  const byNumber = useMemo(() => new Map(hits.map(h => [h.number, h])), [hits]);
  const { summary } = report;
  const checkable = summary.checked - summary.unchecked;
  const missing = missingNeeds(report);
  if (checkable <= 0 && missing.length === 0) return null;
  const tone = summaryTone(summary);
  const flagged = report.claims.filter(c => isFlagged(c.outcome));
  const weak = report.claims.filter(c => c.outcome === 'weak');
  const Icon = tone === 'ok' ? ShieldCheck : tone === 'partial' ? CircleHelp : AlertTriangle;

  return (
    <details className="group/grounding">
      <summary
        aria-controls={listId}
        className={cn(
          'list-none [&::-webkit-details-marker]:hidden w-fit inline-flex items-center gap-1.5 h-7 px-2.5 rounded-full border cursor-pointer select-none tabular-nums transition-colors duration-micro',
          compact ? 'text-[11.5px]' : 'text-[12px]',
          tone === 'ok' && 'border-shodh-success/40 bg-shodh-success-soft text-shodh-success',
          tone === 'partial' && 'border-shodh-border bg-shodh-raised text-shodh-text-secondary',
          tone === 'flagged' && 'border-shodh-warning/50 bg-shodh-warning-soft text-shodh-warning',
          FOCUS_RING,
        )}
      >
        <Icon className="w-3.5 h-3.5" aria-hidden="true" />
        <span aria-hidden="true">{checkable > 0 ? summaryLabel(summary) : 'Grounding'}</span>
        <span className="sr-only">{`Grounding: ${summaryDescription(summary)}. Show details.`}</span>
        <ChevronRight
          className="w-3.5 h-3.5 transition-transform duration-micro group-open/grounding:rotate-90 motion-reduce:transition-none"
          aria-hidden="true"
        />
      </summary>
      <div id={listId} className={cn('mt-2 flex flex-col gap-2 rounded-xl border border-shodh-border bg-shodh-surface p-3', compact ? 'text-[12px]' : 'text-[12.5px]')}>
        <p className="text-shodh-text-muted">{`${summaryDescription(summary)}; ${methodLabel(report.method)}.`}</p>
        {flagged.length === 0 && weak.length === 0 && missing.length === 0 && (
          <p className="flex items-center gap-1.5 text-shodh-success">
            <CircleCheck className="w-3.5 h-3.5" aria-hidden="true" />
            Every checked statement is supported by the passage it cites.
          </p>
        )}
        {flagged.length > 0 && (
          <ClaimList title="Flagged" checks={flagged} byNumber={byNumber} onOpenCitation={onOpenCitation} />
        )}
        {weak.length > 0 && (
          <ClaimList title="Partly supported" checks={weak} byNumber={byNumber} onOpenCitation={onOpenCitation} />
        )}
        {missing.length > 0 && (
          <div className="flex flex-col gap-1">
            <p className="font-semibold text-shodh-text">Not found in the sources</p>
            <ul className="list-none m-0 p-0 flex flex-col gap-1">
              {missing.map(need => (
                <li key={need.id} className="text-shodh-text-secondary break-words">{need.text}</li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </details>
  );
}

function ClaimList({
  title,
  checks,
  byNumber,
  onOpenCitation,
}: {
  title: string;
  checks: readonly ClaimCheck[];
  byNumber: Map<number, SearchHit>;
  onOpenCitation: (hit: SearchHit, trigger: HTMLElement) => void;
}) {
  return (
    <div className="flex flex-col gap-1">
      <p className="font-semibold text-shodh-text">{title}</p>
      <ul className="list-none m-0 p-0 flex flex-col gap-1.5">
        {checks.map((check, i) => {
          const closest = check.closest !== null ? byNumber.get(check.closest) ?? null : null;
          return (
            <li key={`${check.messageId}-${i}`} className="flex flex-col gap-0.5 min-w-0">
              <span className="text-shodh-text-secondary break-words">{check.text}</span>
              <span className="flex flex-wrap items-center gap-x-2 text-shodh-text-muted">
                <span>{flagLabel(check)}</span>
                {closest && (
                  <button
                    type="button"
                    onClick={e => onOpenCitation(closest, e.currentTarget)}
                    className={cn('underline underline-offset-2 decoration-shodh-text-faint hover:text-shodh-text rounded-sm', FOCUS_RING)}
                  >
                    {`closest passage: ${hitLabel(closest)}`}
                  </button>
                )}
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
