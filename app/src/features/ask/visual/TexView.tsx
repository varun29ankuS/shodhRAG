import React, { Suspense, useMemo, useRef } from 'react';
import { cn } from '../../../lib/utils';
import { useKatex } from './katexRuntime';
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
export function TexView(props: TexViewProps) {
  const { tex, display = true, className } = props;
  const Tag = display ? 'div' : 'span';
  // The LaTeX source in place while KaTeX loads (first math of the session).
  const loading = <Tag className={cn('relative font-mono text-[0.9em] opacity-70', !display && 'inline-block', className)}>{tex}</Tag>;
  return (
    <Suspense fallback={loading}>
      <KatexTex {...props} />
    </Suspense>
  );
}

function KatexTex({ tex, display = true, symbols = [], ownLayer = false, className }: TexViewProps) {
  const ref = useRef<HTMLDivElement>(null);
  const katex = useKatex(true);
  // KaTeX output (trust limited to the symbol attribute; see symbols.ts).
  const html = useMemo(() => renderTex(tex, display, symbols), [tex, display, symbols, katex]);
  const Tag = display ? 'div' : 'span';
  return (
    <Tag ref={ref as React.Ref<HTMLDivElement & HTMLSpanElement>} className={cn('relative', !display && 'inline-block', className)}>
      <Tag dangerouslySetInnerHTML={{ __html: html }} />
      {ownLayer && <SymbolLayer containerRef={ref} symbols={symbols} watch={html} />}
    </Tag>
  );
}
