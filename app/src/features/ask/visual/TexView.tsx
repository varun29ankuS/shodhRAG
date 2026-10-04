import React, { useMemo, useRef } from 'react';
import { cn } from '../../../lib/utils';
import { renderTex } from './symbolTex';
import { SymbolLayer } from './SymbolLayer';
import type { SymbolNote } from './symbols';

export interface TexViewProps {
  tex: string;
  display?: boolean;
  symbols?: readonly SymbolNote[];
  /**
   * Draw its own symbol explanations. Off inside an answer, whose message
   * already has one layer over all of its math.
   */
  ownLayer?: boolean;
  className?: string;
}

/** LaTeX rendered by KaTeX, symbols annotated when meanings are given. */
export function TexView({ tex, display = true, symbols = [], ownLayer = false, className }: TexViewProps) {
  const ref = useRef<HTMLDivElement>(null);
  // KaTeX output (trust limited to the symbol attribute; see symbols.ts).
  const html = useMemo(() => renderTex(tex, display, symbols), [tex, display, symbols]);
  const Tag = display ? 'div' : 'span';
  return (
    <Tag ref={ref as React.Ref<HTMLDivElement & HTMLSpanElement>} className={cn('relative', !display && 'inline-block', className)}>
      <Tag dangerouslySetInnerHTML={{ __html: html }} />
      {ownLayer && <SymbolLayer containerRef={ref} symbols={symbols} watch={html} />}
    </Tag>
  );
}
