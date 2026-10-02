import React, { useCallback, useMemo, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark, oneLight } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { Check, Copy } from 'lucide-react';
import { useTheme } from '../../contexts/ThemeContext';
import { cn } from '../../lib/utils';
import { stripChartContent } from '../../utils/artifactExtractor';
import { getArtifactKind } from '../../utils/artifactKind';
import { ChartArtifact } from '../../components/ChartArtifact';
import { TableArtifact } from '../../components/TableArtifact';
import { ArtifactPreviewCard } from '../../components/ArtifactPreviewCard';
import type { SearchHit } from './types';

/** Citation placeholders: ASCII markers that survive markdown parsing. */
const CITE_OPEN = 'XCSHODH';
const CITE_CLOSE = 'XESHODH';
const CITE_PATTERN = new RegExp(`${CITE_OPEN}(\\d+)${CITE_CLOSE}`, 'g');

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
}

/**
 * Renders an assistant answer: markdown prose with inline citation pills,
 * followed by inline charts, tables and other artifact cards.
 *
 * Pipeline (unchanged from the original chat renderer):
 * 1. Strip artifact fences (rendered separately) and inline chart JSON.
 * 2. Protect code blocks from citation rewriting.
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
}: MessageContentRendererProps) {
  const { theme } = useTheme();
  const isDark = theme === 'dark';

  const hitsByNumber = useMemo(() => {
    const map = new Map<number, SearchHit>();
    for (const hit of hits) map.set(hit.number, hit);
    return map;
  }, [hits]);

  const preprocessed = useMemo(() => {
    let text = content;

    text = text.replace(/```(?:chart|table|mermaid|flowchart|sequence|classDiagram|erDiagram|stateDiagram|gantt|gitGraph|journey|form|action)\s*\n[\s\S]*?```/g, '');
    text = stripChartContent(text);

    const codeBlocks: string[] = [];
    text = text.replace(/```[\s\S]*?```/g, match => {
      codeBlocks.push(match);
      return `\x01CODE${codeBlocks.length - 1}\x01`;
    });

    const lines = text.split('\n');
    const merged: string[] = [];
    for (const line of lines) {
      const trimmed = line.trim();
      if (/^(\[(?:Document\s+)?\d+(?:\s*,\s*(?:Document\s+)?\d+)*\]\s*)+$/.test(trimmed) && merged.length > 0) {
        merged[merged.length - 1] = `${merged[merged.length - 1].trimEnd()} ${trimmed}`;
      } else {
        merged.push(line);
      }
    }
    text = merged.join('\n');

    text = text.replace(/【(\d+)†[^】]*】/g, `${CITE_OPEN}$1${CITE_CLOSE}`);
    text = text.replace(/\[(?:Document\s+)?(\d+(?:\s*,\s*(?:Document\s+)?\d+)*)\]/gi, (_, nums: string) =>
      nums
        .split(',')
        .map(n => `${CITE_OPEN}${n.replace(/Document\s+/gi, '').trim()}${CITE_CLOSE}`)
        .join(''),
    );

    text = text.replace(/\x01CODE(\d+)\x01/g, (_, idx: string) => codeBlocks[Number(idx)] ?? '');
    return text.replace(/\n{3,}/g, '\n\n');
  }, [content]);

  const renderWithCitations = useCallback((text: string): React.ReactNode => {
    if (hitsByNumber.size === 0) {
      return text.replace(CITE_PATTERN, '');
    }
    const parts: React.ReactNode[] = [];
    let lastIndex = 0;
    for (const match of text.matchAll(CITE_PATTERN)) {
      const index = match.index ?? 0;
      if (index > lastIndex) parts.push(text.slice(lastIndex, index));
      const number = Number(match[1]);
      const hit = hitsByNumber.get(number);
      if (hit) {
        const isActive = activeCitation === number;
        parts.push(
          <button
            key={`cite-${index}-${number}`}
            type="button"
            onClick={e => onOpenCitation(hit, e.currentTarget)}
            aria-label={`Source ${number}, ${hit.fileName}`}
            aria-pressed={isActive}
            title={hit.fileName}
            className={cn(
              'inline-flex items-center justify-center min-w-[18px] h-[18px] px-1 ml-[3px] rounded-[5px] align-[3px] text-[10.5px] font-bold leading-none tabular-nums transition-colors duration-micro',
              isActive
                ? 'bg-shodh-accent text-shodh-on-accent'
                : 'bg-shodh-pressed text-shodh-text-secondary hover:bg-shodh-border-strong hover:text-shodh-text',
              FOCUS_RING,
            )}
          >
            {number}
          </button>,
        );
      } else {
        parts.push(`[${number}]`);
      }
      lastIndex = index + match[0].length;
    }
    if (lastIndex < text.length) parts.push(text.slice(lastIndex));
    return parts.length > 0 ? <>{parts}</> : text;
  }, [hitsByNumber, activeCitation, onOpenCitation]);

  const processChildren = useCallback((children: React.ReactNode): React.ReactNode =>
    React.Children.map(children, child => {
      if (typeof child === 'string') return renderWithCitations(child);
      if (React.isValidElement<{ children?: React.ReactNode }>(child) && child.props.children) {
        return React.cloneElement(child, undefined, processChildren(child.props.children));
      }
      return child;
    }), [renderWithCitations]);

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
    pre: ({ children }) => (
      <div className="my-4 rounded-xl overflow-hidden border border-shodh-border bg-shodh-surface">{children}</div>
    ),
    code: ({ children, className }) => {
      const match = /language-(\w+)/.exec(className || '');
      if (match) {
        const codeString = String(children).replace(/\n$/, '');
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
    blockquote: ({ children }) => (
      <blockquote className="my-4 pl-4 border-l-2 border-shodh-border-strong text-shodh-text-tertiary">{children}</blockquote>
    ),
    table: ({ children }) => (
      <div className="my-4 overflow-x-auto rounded-xl border border-shodh-border">
        <table className="w-full text-[14px] border-collapse">{children}</table>
      </div>
    ),
    thead: ({ children }) => <thead className="bg-shodh-raised">{children}</thead>,
    th: ({ children }) => (
      <th className="px-3 py-2 text-left font-semibold text-shodh-text border-b border-shodh-border">{processChildren(children)}</th>
    ),
    td: ({ children }) => (
      <td className="px-3 py-2 align-top text-shodh-text-secondary border-b border-shodh-border-subtle">{processChildren(children)}</td>
    ),
    tr: ({ children }) => <tr>{children}</tr>,
    hr: () => <hr className="my-6 border-shodh-border" />,
  }), [isDark, processChildren]);

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
        <div className="text-[16px] leading-[1.75] text-shodh-text-secondary break-words">
          <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>
            {preprocessed}
          </ReactMarkdown>
        </div>
      )}

      {charts.map(artifact => (
        <div key={artifact.id} className="rounded-2xl overflow-hidden border border-shodh-border">
          <ChartArtifact artifact={artifact} theme={theme} />
        </div>
      ))}

      {tables.map(artifact => (
        <div key={artifact.id} className="rounded-2xl overflow-hidden border border-shodh-border">
          <TableArtifact artifact={artifact} theme={theme} />
        </div>
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
