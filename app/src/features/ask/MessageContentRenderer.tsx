import React, { useCallback, useMemo, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import 'katex/dist/katex.min.css';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark, oneLight } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { Check, Copy, Globe } from 'lucide-react';
import { useTheme } from '../../contexts/ThemeContext';
import { cn } from '../../lib/utils';
import { stripChartContent } from '../../utils/artifactExtractor';
import { getArtifactKind } from '../../utils/artifactKind';
import { ChartArtifact } from '../../components/ChartArtifact';
import { TableArtifact } from '../../components/TableArtifact';
import { ArtifactPreviewCard } from '../../components/ArtifactPreviewCard';
import type { SearchHit } from './types';
import { sourceLabel, webHost } from './searchResults';
import { ChartBlock, MermaidBlock } from './visual/VisualBlocks';
import { SvgBlock } from './visual/SvgSketch';
import { PlotBlock } from './visual/PlotView';
import { SimulationBlock } from './visual/SimulationView';
import { FigureBlock } from './visual/FigureBlock';
import { DerivationBlock, SymbolsBlock } from './visual/MathBlocks';
import { SymbolLayer } from './visual/SymbolLayer';
import { AnswerBlocksContext } from './visual/answerContext';
import type { AnswerBlocks } from './visual/answerContext';
import rehypeSymbols from './visual/rehypeSymbols';
import { safeAnnotate } from './visual/symbolTex';
import { KATEX_SYMBOL_OPTIONS, symbolsInMessage } from './visual/symbols';
import { FocusFrame } from '../focus/FocusFrame';
import { tableRows } from '../focus/focusDom';
import rehypeFocusEquations, { FOCUS_EQUATION_TAG } from '../focus/rehypeFocusEquations';
import { chartTarget, equationTarget, imageTarget, tableTarget } from '../focus/targets';
import { escapeCurrency, isMermaidLanguage, mermaidSource, normalizeMathDelimiters, protectMath } from './visual/mathText';
import type { ClaimCheck } from '../agent/events';
import { FLAG_CLOSE, FLAG_OPEN, insertFlagMarkers, parseCitationMarkers } from '../agent/grounding';
import { ClaimFlag, InvalidCitation } from '../agent/GroundingFlags';

/** Citation placeholders: ASCII markers that survive markdown parsing. */
const CITE_OPEN = 'XCSHODH';
const CITE_CLOSE = 'XESHODH';
const CITE_PATTERN = new RegExp(`${CITE_OPEN}(\\d+)${CITE_CLOSE}`, 'g');
/** Citation and claim-flag placeholders, in one pass. */
const TOKEN_PATTERN = new RegExp(`${CITE_OPEN}(\\d+)${CITE_CLOSE}|${FLAG_OPEN}(\\d+)${FLAG_CLOSE}`, 'g');

/** Replace every citation marker of `text` with placeholders, one per number. */
function citationPlaceholders(text: string): string {
  const markers = parseCitationMarkers(text);
  let out = '';
  let last = 0;
  for (const m of markers) {
    out += text.slice(last, m.start) + m.numbers.map(n => `${CITE_OPEN}${n}${CITE_CLOSE}`).join('');
    last = m.end;
  }
  return out + text.slice(last);
}

/** A line made only of citation markers. */
function isCitationOnly(line: string): boolean {
  const markers = parseCitationMarkers(line);
  if (markers.length === 0) return false;
  let rest = line;
  for (const m of [...markers].reverse()) rest = rest.slice(0, m.start) + rest.slice(m.end);
  return rest.trim().length === 0;
}

/** Fenced languages drawn as visuals, which draw their own frame. */
const VISUAL_LANGUAGES = new Set(['chart', 'svg', 'plot', 'simulation', 'figure', 'derivation', 'symbols']);

/** Target of a rendered table (header row first). */
function tableFromElement(el: HTMLElement) {
  const table = el.querySelector('table');
  return table ? tableTarget(tableRows(table)) : null;
}

/** Target of a rendered image. */
function imageFromElement(el: HTMLElement) {
  const img = el.querySelector('img');
  return img ? imageTarget(img.getAttribute('src'), img.getAttribute('alt')) : null;
}

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

function CodeCopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch (error) {
      console.error('Failed to copy code:', error);
    }
  };
  return (
    <button
      type="button"
      onClick={handleCopy}
      aria-label={copied ? 'Code copied' : 'Copy code'}
      className={cn(
        'inline-flex items-center gap-1 h-6 px-1.5 rounded-md text-[11px] text-shodh-text-muted hover:bg-shodh-raised-2 hover:text-shodh-text transition-colors duration-micro',
        FOCUS_RING,
      )}
    >
      {copied ? <Check className="w-3 h-3 text-shodh-success" aria-hidden="true" /> : <Copy className="w-3 h-3" aria-hidden="true" />}
      <span>{copied ? 'Copied' : 'Copy'}</span>
    </button>
  );
}

export interface MessageContentRendererProps {
  content: string;
  /** Search hits for this answer; `[N]` maps to `hits` entry with number N. */
  hits: readonly SearchHit[];
  artifacts?: any[];
  /** Citation number whose source is currently open in the preview. */
  activeCitation?: number | null;
  onOpenCitation: (hit: SearchHit, trigger: HTMLElement) => void;
  onOpenArtifact?: (artifactId: string) => void;
  /** Smaller type for dense surfaces such as the conversation dock. */
  compact?: boolean;
  /**
   * Turn `[N]` into citation pills (answers). Off for text the reader
   * wrote, where `[1]` stays literal.
   */
  citations?: boolean;
  /**
   * Grounding verdicts on this text's claims; flagged ones get an inline
   * flag after the claim (found by its anchor text).
   */
  claims?: readonly ClaimCheck[];
  /**
   * Show a citation number with no matching source as flagged even when the
   * answer has no sources at all (agent answers). Otherwise such numbers are
   * dropped when there are no sources.
   */
  flagUnknownCitations?: boolean;
}

/**
 * Renders an assistant answer: markdown prose with inline citation pills,
 * followed by inline charts, tables and other artifact cards.
 *
 * Pipeline:
 * 1. Answers that arrived with backend artifacts: strip their fences (the
 *    artifacts render separately). Otherwise ```mermaid and ```chart fences
 *    render inline as diagrams and charts.
 * 2. Protect code blocks and math from citation rewriting.
 * 3. Collapse citation-only lines into the previous line.
 * 4. Replace `[N]` / `【N†…】` with placeholders that survive markdown parsing.
 * 5. Render markdown; text nodes swap placeholders for citation pills.
 */
export function MessageContentRenderer({
  content,
  hits,
  artifacts,
  activeCitation = null,
  onOpenCitation,
  onOpenArtifact,
  compact = false,
  citations = true,
  claims,
  flagUnknownCitations = false,
}: MessageContentRendererProps) {
  const { theme } = useTheme();
  const isDark = theme === 'dark';

  const hasArtifacts = (artifacts?.length ?? 0) > 0;
  const proseRef = useRef<HTMLDivElement>(null);

  // Symbol meanings of the whole answer (from its ```symbols blocks), shown on its math.
  const symbols = useMemo(() => symbolsInMessage(content), [content]);
  const symbolsRef = useRef(symbols);
  symbolsRef.current = symbols;
  const symbolsKey = symbols.map(s => `${s.symbol}\u0001${s.meaning}`).join('\u0002');
  const rehypePlugins = useMemo(
    () => [
      [rehypeSymbols, { annotate: (tex: string, display: boolean) => safeAnnotate(tex, symbols, display) }],
      [rehypeKatex, KATEX_SYMBOL_OPTIONS],
      rehypeFocusEquations,
    ],
    // symbolsKey stands for the symbols' content.
    [symbolsKey],
  );

  const hitsByNumber = useMemo(() => {
    const map = new Map<number, SearchHit>();
    for (const hit of hits) map.set(hit.number, hit);
    return map;
  }, [hits]);

  const preprocessed = useMemo(() => {
    // Flags first: their anchors are exact text of the message as written.
    let text = citations && claims && claims.length > 0 ? insertFlagMarkers(content, claims).text : content;

    if (hasArtifacts) {
      text = text.replace(/```(?:chart|table|mermaid|flowchart|sequence|classDiagram|erDiagram|stateDiagram|gantt|gitGraph|journey|form|action)\s*\n[\s\S]*?```/g, '');
      text = stripChartContent(text);
    }

    const codeBlocks: string[] = [];
    text = text.replace(/```[\s\S]*?```/g, match => {
      codeBlocks.push(match);
      return `\x01CODE${codeBlocks.length - 1}\x01`;
    });

    text = escapeCurrency(normalizeMathDelimiters(text));
    const math = protectMath(text);
    text = math.text;

    if (citations) {
      const lines = text.split('\n');
      const merged: string[] = [];
      for (const line of lines) {
        const trimmed = line.trim();
        if (isCitationOnly(trimmed) && merged.length > 0) {
          merged[merged.length - 1] = `${merged[merged.length - 1].trimEnd()} ${trimmed}`;
        } else {
          merged.push(line);
        }
      }
      text = citationPlaceholders(merged.join('\n'));
    }

    text = math.restore(text);
    text = text.replace(/\x01CODE(\d+)\x01/g, (_, idx: string) => codeBlocks[Number(idx)] ?? '');
    return text.replace(/\n{3,}/g, '\n\n');
  }, [content, hasArtifacts, citations, claims]);

  const renderWithCitations = useCallback((text: string): React.ReactNode => {
    if (hitsByNumber.size === 0 && !flagUnknownCitations && !(claims && claims.length > 0)) {
      return text.replace(CITE_PATTERN, '');
    }
    const parts: React.ReactNode[] = [];
    let lastIndex = 0;
    for (const match of text.matchAll(TOKEN_PATTERN)) {
      const index = match.index ?? 0;
      if (index > lastIndex) parts.push(text.slice(lastIndex, index));
      lastIndex = index + match[0].length;
      if (match[2] !== undefined) {
        const check = claims?.[Number(match[2])];
        if (check) {
          const closest = check.closest !== null ? hitsByNumber.get(check.closest) ?? null : null;
          parts.push(<ClaimFlag key={`flag-${index}`} check={check} closest={closest} onOpenCitation={onOpenCitation} />);
        }
        continue;
      }
      const number = Number(match[1]);
      const hit = hitsByNumber.get(number);
      if (hit) {
        const isActive = activeCitation === number;
        // Web sources are untrusted and outside the user's documents: they
        // get an outlined badge and say so to screen readers.
        const isWeb = hit.url !== null;
        parts.push(
          <button
            key={`cite-${index}-${number}`}
            type="button"
            onClick={e => onOpenCitation(hit, e.currentTarget)}
            aria-label={`${isWeb ? 'Web source' : 'Source'} ${number}, ${sourceLabel(hit)}`}
            aria-pressed={isActive}
            title={isWeb && hit.url ? `${sourceLabel(hit)} — ${webHost(hit.url)} (opens in your browser)` : sourceLabel(hit)}
            className={cn(
              'inline-flex items-center justify-center min-w-[18px] h-[18px] px-1 ml-[3px] rounded-[5px] align-[3px] text-[10.5px] font-bold leading-none tabular-nums transition-colors duration-micro',
              isActive
                ? 'bg-shodh-accent text-shodh-on-accent'
                : isWeb
                  ? 'border border-dashed border-shodh-info text-shodh-info hover:bg-shodh-raised-2'
                  : 'bg-shodh-pressed text-shodh-text-secondary hover:bg-shodh-border-strong hover:text-shodh-text',
              FOCUS_RING,
            )}
          >
            {isWeb && <Globe className="w-2.5 h-2.5 mr-0.5" aria-hidden="true" />}
            {number}
          </button>,
        );
      } else if (hitsByNumber.size > 0 || flagUnknownCitations) {
        // Never a working pill: the number matches no source of this answer.
        parts.push(<InvalidCitation key={`invalid-${index}-${number}`} number={number} />);
      }
    }
    if (lastIndex < text.length) parts.push(text.slice(lastIndex));
    return parts.length > 0 ? <>{parts}</> : text;
  }, [hitsByNumber, activeCitation, onOpenCitation, claims, flagUnknownCitations]);

  const processChildren = useCallback((children: React.ReactNode): React.ReactNode =>
    React.Children.map(children, child => {
      if (typeof child === 'string') return renderWithCitations(child);
      if (React.isValidElement<{ children?: React.ReactNode }>(child) && child.props.children) {
        return React.cloneElement(child, undefined, processChildren(child.props.children));
      }
      return child;
    }), [renderWithCitations]);

  // Fenced blocks get their own memo keyed on the theme only: a new `code`
  // component would remount every visual below it (plot sliders, running
  // simulations) whenever citations or hits change.
  const codeComponents = useMemo<Record<string, React.FC<any>>>(() => ({
    pre: ({ children }) => {
      // Diagrams and charts draw their own frame.
      const child = React.Children.toArray(children)[0];
      const lang = React.isValidElement<{ className?: string }>(child)
        ? /language-([\w-]+)/.exec(child.props.className || '')?.[1] ?? ''
        : '';
      if (VISUAL_LANGUAGES.has(lang) || isMermaidLanguage(lang)) return <>{children}</>;
      return <div className="my-4 rounded-xl overflow-hidden border border-shodh-border bg-shodh-surface">{children}</div>;
    },
    code: ({ children, className }) => {
      const match = /language-([\w-]+)/.exec(className || '');
      if (match) {
        const codeString = String(children).replace(/\n$/, '');
        if (isMermaidLanguage(match[1])) return <MermaidBlock source={mermaidSource(match[1], codeString)} dark={isDark} />;
        if (match[1] === 'chart') return <ChartBlock source={codeString} theme={theme} />;
        if (match[1] === 'svg') return <SvgBlock source={codeString} />;
        if (match[1] === 'plot') return <PlotBlock source={codeString} />;
        if (match[1] === 'simulation') return <SimulationBlock source={codeString} />;
        if (match[1] === 'figure') return <FigureBlock source={codeString} />;
        if (match[1] === 'derivation') return <DerivationBlock source={codeString} />;
        if (match[1] === 'symbols') return <SymbolsBlock source={codeString} />;
        return (
          <div>
            <div className="flex items-center justify-between pl-3 pr-1.5 h-8 border-b border-shodh-border-subtle bg-shodh-raised">
              <span className="font-mono text-[11px] uppercase tracking-wider text-shodh-text-muted">{match[1]}</span>
              <CodeCopyButton text={codeString} />
            </div>
            <SyntaxHighlighter
              style={isDark ? oneDark : oneLight}
              language={match[1]}
              PreTag="div"
              customStyle={{ margin: 0, padding: '14px 16px', fontSize: '13px', lineHeight: 1.6, borderRadius: 0, background: 'transparent' }}
            >
              {codeString}
            </SyntaxHighlighter>
          </div>
        );
      }
      return (
        <code className="px-1.5 py-0.5 rounded-md bg-shodh-raised-2 font-mono text-[0.875em] text-shodh-text">{children}</code>
      );
    },
  }), [isDark, theme]);

  const markdownComponents = useMemo<Record<string, React.FC<any>>>(() => ({
    h1: ({ children }) => <h2 className="text-[20px] font-semibold leading-snug text-shodh-text mt-6 mb-2 first:mt-0">{processChildren(children)}</h2>,
    h2: ({ children }) => <h3 className="text-[17px] font-semibold leading-snug text-shodh-text mt-5 mb-1.5 first:mt-0">{processChildren(children)}</h3>,
    h3: ({ children }) => <h4 className="text-[16px] font-semibold text-shodh-text mt-4 mb-1 first:mt-0">{processChildren(children)}</h4>,
    h4: ({ children }) => <h5 className="text-[15px] font-semibold text-shodh-text-secondary mt-3 mb-1 first:mt-0">{processChildren(children)}</h5>,
    p: ({ children }) => <p className="my-3 first:mt-0 last:mb-0">{processChildren(children)}</p>,
    ul: ({ children }) => <ul className="my-3 pl-6 list-disc space-y-1 marker:text-shodh-text-faint">{children}</ul>,
    ol: ({ children }) => <ol className="my-3 pl-6 list-decimal space-y-1 marker:text-shodh-text-faint">{children}</ol>,
    li: ({ children }) => <li className="pl-1">{processChildren(children)}</li>,
    strong: ({ children }) => <strong className="font-semibold text-shodh-text">{processChildren(children)}</strong>,
    em: ({ children }) => <em className="italic">{processChildren(children)}</em>,
    a: ({ href, children }) => (
      <a
        href={href}
        target="_blank"
        rel="noopener noreferrer"
        className={cn('text-shodh-accent-text underline underline-offset-2 decoration-shodh-accent-text/40 hover:decoration-shodh-accent-text rounded-sm', FOCUS_RING)}
      >
        {children}
      </a>
    ),
    ...codeComponents,
    blockquote: ({ children }) => (
      <blockquote className="my-4 pl-4 border-l-2 border-shodh-border-strong text-shodh-text-tertiary">{children}</blockquote>
    ),
    table: ({ children }) => (
      <FocusFrame noun="table" getTarget={tableFromElement} doubleClick={false} className="my-4">
        <div className="overflow-x-auto rounded-xl border border-shodh-border">
          <table className="w-full text-[14px] border-collapse">{children}</table>
        </div>
      </FocusFrame>
    ),
    img: ({ src, alt }) => (
      <FocusFrame noun="image" getTarget={imageFromElement} inline className="my-1 max-w-full align-top">
        <img src={src} alt={alt ?? ''} loading="lazy" className="block max-w-full h-auto rounded-lg border border-shodh-border" />
      </FocusFrame>
    ),
    [FOCUS_EQUATION_TAG]: ({ node, children }) => {
      const tex = typeof node?.properties?.dataTex === 'string' ? node.properties.dataTex : '';
      if (!tex) return <>{children}</>;
      return (
        <FocusFrame noun="equation" getTarget={() => equationTarget(tex, symbolsRef.current)}>
          {children}
        </FocusFrame>
      );
    },
    thead: ({ children }) => <thead className="bg-shodh-raised">{children}</thead>,
    th: ({ children }) => (
      <th className="px-3 py-2 text-left font-semibold text-shodh-text border-b border-shodh-border">{processChildren(children)}</th>
    ),
    td: ({ children }) => (
      <td className="px-3 py-2 align-top text-shodh-text-secondary border-b border-shodh-border-subtle">{processChildren(children)}</td>
    ),
    tr: ({ children }) => <tr>{children}</tr>,
    hr: () => <hr className="my-6 border-shodh-border" />,
  }), [codeComponents, processChildren]);

  const answerBlocks = useMemo<AnswerBlocks>(
    () => ({ symbols, renderInline: text => renderWithCitations(citations ? citationPlaceholders(text) : text) }),
    [symbols, renderWithCitations, citations],
  );

  const { charts, tables, others } = useMemo(() => {
    const list = artifacts ?? [];
    return {
      charts: list.filter(a => getArtifactKind(a.artifact_type) === 'chart'),
      tables: list.filter(a => getArtifactKind(a.artifact_type) === 'table'),
      others: list.filter(a => {
        const kind = getArtifactKind(a.artifact_type);
        return kind !== 'chart' && kind !== 'table';
      }),
    };
  }, [artifacts]);

  return (
    <div className="flex flex-col gap-4">
      {preprocessed.trim().length > 0 && (
        <div ref={proseRef} className={cn('relative text-shodh-text-secondary break-words', compact ? 'text-[13.5px] leading-[1.6]' : 'text-[16px] leading-[1.75]')}>
          <AnswerBlocksContext.Provider value={answerBlocks}>
            <ReactMarkdown remarkPlugins={[remarkGfm, remarkMath]} rehypePlugins={rehypePlugins as never} components={markdownComponents}>
              {preprocessed}
            </ReactMarkdown>
          </AnswerBlocksContext.Provider>
          <SymbolLayer containerRef={proseRef} symbols={symbols} watch={preprocessed} />
        </div>
      )}

      {charts.map(artifact => (
        <FocusFrame key={artifact.id} noun="chart" getTarget={() => chartTarget(String(artifact.content ?? ''), artifact.title)}>
          <div className="rounded-2xl overflow-hidden border border-shodh-border">
            <ChartArtifact artifact={artifact} theme={theme} />
          </div>
        </FocusFrame>
      ))}

      {tables.map(artifact => (
        <FocusFrame key={artifact.id} noun="table" getTarget={tableFromElement} doubleClick={false}>
          <div className="rounded-2xl overflow-hidden border border-shodh-border">
            <TableArtifact artifact={artifact} theme={theme} />
          </div>
        </FocusFrame>
      ))}

      {others.length > 0 && onOpenArtifact && (
        <div className="flex flex-col gap-2">
          {others.map(artifact => (
            <ArtifactPreviewCard key={artifact.id} artifact={artifact} onClick={() => onOpenArtifact(artifact.id)} />
          ))}
        </div>
      )}
    </div>
  );
}

export default MessageContentRenderer;
