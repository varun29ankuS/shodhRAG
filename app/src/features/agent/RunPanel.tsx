import React, { useEffect, useId, useState } from 'react';
import { AlertCircle, ChevronRight, CircleCheck, SearchX } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { SearchHit } from '../ask/types';
import type { PlanItem, PlanStatus } from './events';
import { planSummary } from './reducer';
import type { RunSummary } from './runSummary';
import { Spinner } from './StepLine';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const GLYPH: Record<PlanStatus, string> = { pending: '☐', in_progress: '◐', done: '☑', blocked: '☒' };
const STATUS_TEXT: Record<PlanStatus, string> = { pending: 'to do', in_progress: 'in progress', done: 'done', blocked: 'blocked' };
const GLYPH_COLOR: Record<PlanStatus, string> = {
  pending: 'text-shodh-text-faint',
  in_progress: 'text-shodh-accent-text',
  done: 'text-shodh-success',
  blocked: 'text-shodh-warning',
};
const TEXT_STYLE: Record<PlanStatus, string> = {
  pending: 'text-shodh-text-secondary',
  in_progress: 'text-shodh-text font-medium',
  done: 'text-shodh-text-muted line-through decoration-shodh-text-faint',
  blocked: 'text-shodh-text',
};

/** The test id of the run view's header, which opens and closes it. */
export const RUN_PANEL_TOGGLE = 'run-panel-toggle';

export interface RunPanelProps {
  /** What the run is doing now, or what it did: one line (`workFold`'s), or null. */
  line: string | null;
  live: boolean;
  plan: readonly PlanItem[] | null;
  /** The run's steps, as rows (the Transcript draws them); null when there were none. */
  steps: React.ReactNode | null;
  /** What the finished run did; null while it runs. */
  summary: RunSummary | null;
  hits: readonly SearchHit[];
  onOpenCitation: (hit: SearchHit, trigger: HTMLElement) => void;
  compact: boolean;
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5" aria-label={title}>
      <h4 className="m-0 text-caption font-semibold uppercase tracking-[0.06em] text-shodh-text-faint">{title}</h4>
      {children}
    </section>
  );
}

/** A passage's number as the answer cites it: a pill that opens the passage. */
function CitationPill({ n, hits, onOpenCitation }: { n: number; hits: readonly SearchHit[]; onOpenCitation: RunPanelProps['onOpenCitation'] }) {
  const hit = hits.find(h => h.number === n) ?? null;
  const pill = 'inline-flex items-center justify-center min-w-6 h-[18px] px-1 rounded-[5px] bg-shodh-pressed text-caption font-bold leading-none tabular-nums text-shodh-text-secondary';
  if (!hit) return <span className={pill}>{n}</span>;
  return (
    <button
      type="button"
      onClick={e => onOpenCitation(hit, e.currentTarget)}
      aria-label={`Source ${n}, ${hit.fileName}`}
      title={hit.fileName}
      className={cn('target-24', pill, 'hover:bg-shodh-border-strong hover:text-shodh-text', FOCUS_RING)}
    >
      {n}
    </button>
  );
}

/** Coverage of an information need: found in which passages (each a pill), or not found. */
function NeedCoverage({ item, hits, onOpenCitation }: { item: PlanItem; hits: readonly SearchHit[]; onOpenCitation: RunPanelProps['onOpenCitation'] }) {
  if (!item.need || item.coverage === null) return null;
  if (item.coverage === 'covered') {
    return (
      <span className="flex flex-wrap items-center gap-1 text-caption text-shodh-success">
        <CircleCheck className="w-3 h-3 shrink-0" aria-hidden="true" />
        {item.evidence.length > 0 ? 'Found in' : 'Found in the sources'}
        {item.evidence.map(n => <CitationPill key={n} n={n} hits={hits} onOpenCitation={onOpenCitation} />)}
      </span>
    );
  }
  return (
    <span className="flex items-center gap-1 text-caption text-shodh-warning">
      <SearchX className="w-3 h-3 shrink-0" aria-hidden="true" />
      Not found in the sources
    </span>
  );
}

function PlanRows({ plan, hits, onOpenCitation }: { plan: readonly PlanItem[]; hits: readonly SearchHit[]; onOpenCitation: RunPanelProps['onOpenCitation'] }) {
  return (
    <ol className="list-none m-0 p-0 flex flex-col gap-1">
      {plan.map(item => (
        <li key={item.id} className="flex items-start gap-2 min-w-0">
          <span aria-hidden="true" className={cn('font-mono leading-[1.45] shrink-0', GLYPH_COLOR[item.status])}>
            {GLYPH[item.status]}
          </span>
          <span className="min-w-0 flex flex-col gap-0.5">
            {/* Only the item's own words are struck when it is done; what it found is not. */}
            <span className={cn('break-words leading-[1.45]', TEXT_STYLE[item.status])}>
              {item.text}
              <span className="sr-only">{` (${STATUS_TEXT[item.status]})`}</span>
            </span>
            <NeedCoverage item={item} hits={hits} onOpenCitation={onOpenCitation} />
            {item.status === 'blocked' && item.reason && (
              <span className="flex items-start gap-1 text-caption text-shodh-warning">
                <AlertCircle className="w-3 h-3 mt-0.5 shrink-0" aria-hidden="true" />
                <span className="min-w-0 break-words">{`Blocked: ${item.reason}`}</span>
              </span>
            )}
          </span>
        </li>
      ))}
    </ol>
  );
}

function SummaryRows({ summary, hits, onOpenCitation }: { summary: RunSummary; hits: readonly SearchHit[]; onOpenCitation: RunPanelProps['onOpenCitation'] }) {
  const hitOf = (n: number) => hits.find(hit => hit.number === n) ?? null;
  return (
    <div className="flex flex-col gap-2">
      {summary.sources.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="text-shodh-text-muted">Read</span>
          {summary.sources.map(source => {
            const hit = hitOf(source.n);
            return hit ? (
              <button
                key={source.n}
                type="button"
                onClick={e => onOpenCitation(hit, e.currentTarget)}
                className={cn('min-h-6 px-2 py-0.5 inline-flex items-center rounded-md bg-shodh-raised-2 text-caption text-start text-shodh-text-secondary hover:text-shodh-text break-all', FOCUS_RING)}
              >
                {source.label}
              </button>
            ) : (
              <span key={source.n} className="min-h-6 px-2 py-0.5 inline-flex items-center rounded-md bg-shodh-raised-2 text-caption text-shodh-text-secondary break-all">
                {source.label}
              </span>
            );
          })}
        </div>
      )}
      {summary.changed.length > 0 && <Lines label="Changed" lines={summary.changed} />}
      {summary.commands.length > 0 && <Lines label="Ran" lines={summary.commands} />}
      {summary.open.length > 0 && <Lines label="Open" lines={summary.open} tone="warn" />}
    </div>
  );
}

function Lines({ label, lines, tone }: { label: string; lines: readonly string[]; tone?: 'warn' }) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-shodh-text-muted">{label}</span>
      <ul className="list-none m-0 p-0 flex flex-col gap-0.5">
        {lines.map(line => (
          <li key={line} className={cn('break-words', tone === 'warn' ? 'text-shodh-warning' : 'text-shodh-text-secondary')}>
            {line}
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * An agent run in one place: its Plan (the steps it intends), its Steps (each one's state,
 * live, against the plan) and, once it has finished, its Summary (what it read, changed
 * and ran, and what is left open, with the sources one click away). One line while
 * closed: what is happening now, or what was done. Open while the run goes on; closed
 * once it finishes, so the answer starts at the top.
 */
export function RunPanel({ line, live, plan, steps, summary, hits, onOpenCitation, compact }: RunPanelProps) {
  const [open, setOpen] = useState(live);
  const bodyId = useId();
  useEffect(() => {
    setOpen(live);
  }, [live]);
  const planLine = plan && plan.length > 0 ? `Plan: ${planSummary(plan)}` : null;
  const heading = [line, planLine].filter(Boolean).join(' · ') || 'Plan';

  return (
    <div className={cn('flex flex-col min-w-0', compact ? 'text-caption' : 'text-small')} data-testid="answer-steps" data-live={live || undefined}>
      <button
        type="button"
        data-testid={RUN_PANEL_TOGGLE}
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setOpen(o => !o)}
        className={cn(
          'max-w-full w-fit inline-flex items-start gap-1.5 rounded-md text-start text-shodh-text-muted hover:text-shodh-text transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <ChevronRight
          className={cn('w-3.5 h-3.5 mt-0.5 shrink-0 transition-transform duration-micro motion-reduce:transition-none', open && 'rotate-90')}
          aria-hidden="true"
        />
        {live && <Spinner className="mt-1 shrink-0" />}
        <span className={cn('min-w-0 break-words', live && 'text-shodh-text-secondary')} aria-live={live ? 'polite' : undefined}>
          {heading}
        </span>
      </button>
      {open && (
        <div id={bodyId} className={cn('mt-2 ps-3 border-s border-shodh-border-subtle flex flex-col', compact ? 'gap-2.5' : 'gap-3.5')}>
          {summary && (
            <Section title="Summary">
              <SummaryRows summary={summary} hits={hits} onOpenCitation={onOpenCitation} />
            </Section>
          )}
          {plan && plan.length > 0 && (
            <Section title="Plan">
              <PlanRows plan={plan} hits={hits} onOpenCitation={onOpenCitation} />
            </Section>
          )}
          {steps && (
            <Section title="Steps">
              <div className={cn('flex flex-col', compact ? 'gap-2' : 'gap-3')}>{steps}</div>
            </Section>
          )}
        </div>
      )}
    </div>
  );
}

export default RunPanel;
