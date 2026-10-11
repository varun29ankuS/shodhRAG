import React, { useCallback, useId, useMemo, useState } from 'react';
import { usePrintMode } from '../../print/printContext';
import { AlertTriangle, ChevronRight } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { FocusFrame } from '../../focus/FocusFrame';
import { derivationStepTarget } from '../../focus/targets';
import { citedNumbersIn } from '../../agent/grounding';
import { useAnswerBlocks } from './answerContext';
import { parseDerivationBlock, stepContext, unsupportedSteps } from './derivation';
import type { Derivation, DerivationStep } from './derivation';
import { parseSymbolsBlock, symbolDescription } from './symbols';
import { TexView } from './TexView';
import { BlockError } from './VisualBlocks';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

/**
 * A ```symbols block, shown as a compact glossary (the same meanings appear
 * when hovering or focusing the symbols in the answer's equations).
 */
export function SymbolsBlock({ source }: { source: string }) {
  const parsed = useMemo(() => parseSymbolsBlock(source), [source]);
  const headingId = useId();
  if ('error' in parsed) return <BlockError title="Symbols not shown" message={parsed.error} source={source} />;
  return (
    <aside aria-labelledby={headingId} className="my-3 rounded-xl border border-shodh-border-subtle bg-shodh-surface-2 px-3 py-2">
      <h4 id={headingId} className="m-0 mb-1 text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
        Symbols
      </h4>
      <dl className="m-0 grid grid-cols-[minmax(0,auto)_1fr] gap-x-3 gap-y-1 text-[13px] leading-snug">
        {parsed.symbols.map(s => (
          <React.Fragment key={s.symbol}>
            <dt className="text-shodh-text overflow-x-auto scrollbar-thin">
              <TexView tex={s.symbol} display={false} />
            </dt>
            <dd className="m-0 text-shodh-text-secondary">{symbolDescription(s)}</dd>
          </React.Fragment>
        ))}
      </dl>
    </aside>
  );
}

function stepText(step: DerivationStep): string {
  // Cites given as a list but not written in the text are appended as markers.
  const written = citedNumbersIn(step.justification);
  const missing = step.cites.filter(n => !written.has(n));
  return `${step.justification}${missing.length > 0 ? ` ${missing.map(n => `[${n}]`).join('')}` : ''}`.trim();
}

function Step({
  derivation,
  index,
  open,
  onToggle,
}: {
  derivation: Derivation;
  index: number;
  open: boolean;
  onToggle: () => void;
}) {
  const { symbols, renderInline } = useAnswerBlocks();
  const reasonId = useId();
  const step = derivation.steps[index];
  const getTarget = useCallback(() => {
    const around = stepContext(derivation, index);
    return derivationStepTarget({
      title: derivation.title,
      index,
      total: derivation.steps.length,
      latex: step.latex,
      justification: stepText(step),
      previous: around.previous,
      next: around.next,
      symbols,
    });
  }, [derivation, index, step, symbols]);
  const text = stepText(step);
  return (
    <li className="list-none">
      <FocusFrame noun={`step ${index + 1}`} getTarget={getTarget} doubleClick={false}>
        <div className="flex items-start gap-3 py-2 pr-28">
          <span
            className="shrink-0 mt-1 inline-flex items-center justify-center w-6 h-6 rounded-full bg-shodh-raised-2 text-[11.5px] font-semibold tabular-nums text-shodh-text-secondary"
            aria-hidden="true"
          >
            {index + 1}
          </span>
          <div className="flex-1 min-w-0">
            <div className="overflow-x-auto scrollbar-thin text-shodh-text">
              <TexView tex={step.latex} symbols={symbols} />
            </div>
            <button
              type="button"
              onClick={onToggle}
              aria-expanded={open}
              aria-controls={reasonId}
              className={cn(
                'mt-1 h-6 px-1.5 -ml-1.5 inline-flex items-center gap-1 rounded-md text-[12px] text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
                FOCUS_RING,
              )}
            >
              <ChevronRight className={cn('w-3.5 h-3.5 transition-transform duration-micro motion-reduce:transition-none', open && 'rotate-90')} aria-hidden="true" />
              {`Why step ${index + 1}`}
              {step.support === 'unsupported' && <AlertTriangle className="w-3 h-3 text-shodh-warning" aria-label="no source or algebra given" />}
            </button>
            <div id={reasonId} hidden={!open} className="mt-1 pl-2 border-l-2 border-shodh-border-subtle text-[13.5px] leading-relaxed text-shodh-text-secondary">
              {text ? renderInline(text) : <span className="italic text-shodh-text-muted">No justification was given.</span>}
              {step.support === 'unsupported' && (
                <span className="block mt-0.5 text-[12px] text-shodh-warning">This step cites no source and is not marked as algebra.</span>
              )}
            </div>
          </div>
        </div>
      </FocusFrame>
    </li>
  );
}

/** A ```derivation block: numbered steps, each with its reason and Expand & ask. */
export function DerivationBlock({ source }: { source: string }) {
  const parsed = useMemo(() => parseDerivationBlock(source), [source]);
  const headingId = useId();
  const printing = usePrintMode();
  const derivation = 'derivation' in parsed ? parsed.derivation : null;
  // Printed: every step's reason is shown (there is nothing to click on paper).
  const [open, setOpen] = useState<ReadonlySet<number>>(() =>
    printing && derivation ? new Set(derivation.steps.map((_, i) => i)) : new Set(),
  );
  const toggle = useCallback((i: number) => {
    setOpen(prev => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });
  }, []);
  if ('error' in parsed) return <BlockError title="Derivation not shown" message={parsed.error} source={source} />;
  if (!derivation) return null;
  const all = open.size === derivation.steps.length;
  const unsupported = unsupportedSteps(derivation).length;
  return (
    <section aria-labelledby={headingId} className="my-4 rounded-xl border border-shodh-border bg-shodh-surface">
      <header className="flex flex-wrap items-center gap-2 px-3 py-2 border-b border-shodh-border-subtle">
        <h4 id={headingId} className="m-0 flex-1 min-w-0 text-[14px] font-semibold text-shodh-text truncate">
          {derivation.title || 'Derivation'}
        </h4>
        <span className="text-[12px] text-shodh-text-muted">{`${derivation.steps.length} ${derivation.steps.length === 1 ? 'step' : 'steps'}`}</span>
        {unsupported > 0 && (
          <span className="inline-flex items-center gap-1 text-[12px] text-shodh-warning">
            <AlertTriangle className="w-3 h-3" aria-hidden="true" />
            {`${unsupported} without a source`}
          </span>
        )}
        <button
          type="button"
          onClick={() => setOpen(all ? new Set() : new Set(derivation.steps.map((_, i) => i)))}
          className={cn('h-7 px-2 rounded-lg text-[12px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro', FOCUS_RING)}
        >
          {all ? 'Hide reasons' : 'Show all reasons'}
        </button>
      </header>
      <ol className="m-0 px-3 py-1 divide-y divide-shodh-border-subtle">
        {derivation.steps.map((_, i) => (
          <Step key={i} derivation={derivation} index={i} open={open.has(i)} onToggle={() => toggle(i)} />
        ))}
      </ol>
    </section>
  );
}
