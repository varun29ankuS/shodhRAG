import React, { useCallback, useEffect, useId, useLayoutEffect, useState } from 'react';
import { cn } from '../../../lib/utils';
import { SYMBOL_ATTR, symbolDescription } from './symbols';
import type { SymbolNote } from './symbols';

interface Mark {
  index: number;
  left: number;
  top: number;
  width: number;
  height: number;
}

function sameMarks(a: readonly Mark[], b: readonly Mark[]): boolean {
  return (
    a.length === b.length &&
    a.every((m, i) => {
      const n = b[i];
      return m.index === n.index && Math.abs(m.left - n.left) < 0.5 && Math.abs(m.top - n.top) < 0.5 && Math.abs(m.width - n.width) < 0.5 && Math.abs(m.height - n.height) < 0.5;
    })
  );
}

export interface SymbolLayerProps {
  /** The positioned element (`position: relative`) holding the rendered math. */
  containerRef: React.RefObject<HTMLElement | null>;
  symbols: readonly SymbolNote[];
  /** Changes whenever the rendered math may have changed (the source text). */
  watch: unknown;
}

/**
 * Hover and keyboard explanations for the symbols KaTeX marked with
 * `data-shodh-sym`. KaTeX's visual layer is hidden from assistive
 * technology (its MathML is read instead), so the focusable controls are
 * transparent buttons laid over the marked symbols rather than the symbols
 * themselves; each is described by the symbol's meaning.
 */
export function SymbolLayer({ containerRef, symbols, watch }: SymbolLayerProps) {
  const id = useId().replace(/[^a-zA-Z0-9_-]/g, '');
  const [marks, setMarks] = useState<Mark[]>([]);
  const [active, setActive] = useState<number | null>(null);

  const measure = useCallback(() => {
    const root = containerRef.current;
    if (!root || symbols.length === 0) {
      setMarks(prev => (prev.length === 0 ? prev : []));
      return;
    }
    const base = root.getBoundingClientRect();
    // The container may be scaled (the pop-out's zoom): convert screen pixels to its own.
    const sx = root.offsetWidth > 0 ? base.width / root.offsetWidth : 1;
    const sy = root.offsetHeight > 0 ? base.height / root.offsetHeight : 1;
    const next: Mark[] = [];
    root.querySelectorAll<HTMLElement>(`[data-${SYMBOL_ATTR}]`).forEach(el => {
      const index = Number(el.getAttribute(`data-${SYMBOL_ATTR}`));
      if (!Number.isInteger(index) || !symbols[index]) return;
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) return;
      next.push({
        index,
        left: (r.left - base.left) / (sx || 1),
        top: (r.top - base.top) / (sy || 1),
        width: r.width / (sx || 1),
        height: r.height / (sy || 1),
      });
    });
    setMarks(prev => (sameMarks(prev, next) ? prev : next));
  }, [containerRef, symbols]);

  useLayoutEffect(() => {
    measure();
  }, [measure, watch]);

  useEffect(() => {
    const root = containerRef.current;
    if (!root) return;
    const observer = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(() => measure()) : null;
    observer?.observe(root);
    // Wide equations scroll sideways inside the container.
    const onScroll = () => measure();
    root.addEventListener('scroll', onScroll, true);
    let cancelled = false;
    document.fonts?.ready.then(() => {
      if (!cancelled) measure();
    }).catch(() => undefined);
    return () => {
      cancelled = true;
      observer?.disconnect();
      root.removeEventListener('scroll', onScroll, true);
    };
  }, [containerRef, measure]);

  if (symbols.length === 0 || marks.length === 0) return null;
  const shown = active !== null ? marks[active] : null;
  const shownNote = shown ? symbols[shown.index] : null;
  return (
    <>
      {symbols.map((s, i) => (
        <span key={`d-${i}`} id={`${id}-sym-${i}`} className="sr-only">
          {symbolDescription(s)}
        </span>
      ))}
      {marks.map((m, k) => (
        <button
          key={`m-${k}`}
          type="button"
          aria-label={`Symbol ${symbols[m.index].symbol}`}
          aria-describedby={`${id}-sym-${m.index}`}
          onMouseEnter={() => setActive(k)}
          onMouseLeave={() => setActive(a => (a === k ? null : a))}
          onFocus={() => setActive(k)}
          onBlur={() => setActive(a => (a === k ? null : a))}
          onKeyDown={e => {
            if (e.key === 'Escape' && active === k) {
              e.stopPropagation();
              setActive(null);
            }
          }}
          style={{ left: m.left, top: m.top, width: m.width, height: m.height }}
          className={cn(
            'absolute z-[5] p-0 m-0 rounded-[3px] bg-transparent cursor-help border-b border-dotted border-shodh-accent-text/70',
            'hover:bg-shodh-accent-soft/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
          )}
        />
      ))}
      {shown && shownNote && (
        <div
          role="tooltip"
          className="absolute z-20 max-w-[320px] rounded-lg border border-shodh-border-strong bg-shodh-surface px-2.5 py-1.5 text-[12.5px] leading-snug text-shodh-text shadow-md pointer-events-none"
          style={{ left: Math.max(0, shown.left), top: shown.top + shown.height + 6 }}
        >
          <span className="font-mono text-[11.5px] text-shodh-text-muted">{shownNote.symbol}</span>
          <span className="block">{symbolDescription(shownNote)}</span>
        </div>
      )}
    </>
  );
}
