import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { findPassage, prepareHaystack, type TextRange } from './passageMatch';
import { readSourceText, scrollBehavior, type SourceText } from './sourceAccess';
import type { LocateResult } from './viewerTypes';
import { VIEWER_FOCUS_RING } from './viewerTypes';

interface Block {
  start: number;
  end: number;
}

/** Lines per block when rendering code (keeps whitespace exact). */
const CODE_BLOCK_LINES = 200;

/** Prose paragraphs: runs separated by blank lines, as original offsets. */
function proseBlocks(text: string): Block[] {
  const blocks: Block[] = [];
  const separator = /\n[ \t\r]*\n\s*/g;
  let start = 0;
  let match: RegExpExecArray | null;
  while ((match = separator.exec(text)) !== null) {
    if (match.index > start) blocks.push({ start, end: match.index });
    start = match.index + match[0].length;
  }
  if (start < text.length) blocks.push({ start, end: text.length });
  return blocks;
}

/** Code: fixed groups of whole lines, so no whitespace is lost. */
function codeBlocks(text: string): Block[] {
  const blocks: Block[] = [];
  let start = 0;
  let lines = 0;
  for (let i = 0; i < text.length; i += 1) {
    if (text.charCodeAt(i) === 10) {
      lines += 1;
      if (lines === CODE_BLOCK_LINES) {
        blocks.push({ start, end: i + 1 });
        start = i + 1;
        lines = 0;
      }
    }
  }
  if (start < text.length) blocks.push({ start, end: text.length });
  return blocks;
}

interface TextViewerProps {
  filePath: string;
  passage: string;
  /** Render as monospaced code. */
  code: boolean;
  onLocate: (result: LocateResult) => void;
  onError: (error: unknown) => void;
}

/**
 * Full extracted text of a document (same parser as the indexer) with the
 * cited passage highlighted and scrolled into view.
 */
export function TextViewer({ filePath, passage, code, onLocate, onError }: TextViewerProps) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const firstMarkRef = useRef<HTMLElement | null>(null);
  const [source, setSource] = useState<SourceText | null>(null);
  const [ranges, setRanges] = useState<TextRange[] | null>(null);
  const [scrollToken, setScrollToken] = useState(0);

  const onLocateRef = useRef(onLocate);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    onLocateRef.current = onLocate;
    onErrorRef.current = onError;
  }, [onLocate, onError]);

  useEffect(() => {
    let cancelled = false;
    setSource(null);
    setRanges(null);
    readSourceText(filePath)
      .then(result => {
        if (!cancelled) setSource(result);
      })
      .catch(error => {
        if (!cancelled) onErrorRef.current(error);
      });
    return () => {
      cancelled = true;
    };
  }, [filePath]);

  const text = source?.text ?? '';
  const haystack = useMemo(() => (source ? prepareHaystack(source.text) : null), [source]);
  const blocks = useMemo(() => (code ? codeBlocks(text) : proseBlocks(text)), [text, code]);

  useEffect(() => {
    if (!source || !haystack) return;
    const truncatedNote = source.truncated ? ' Only the beginning of this very large document is shown.' : '';
    if (!passage.trim()) {
      setRanges(null);
      onLocateRef.current({ status: 'notFound', message: `No passage text was stored for this source; showing the document from the start.${truncatedNote}` });
      return;
    }
    const match = findPassage(passage, haystack);
    setRanges(match ? match.ranges : null);
    setScrollToken(t => t + 1);
    if (!match) {
      scrollerRef.current?.scrollTo({ top: 0 });
      onLocateRef.current({
        status: 'notFound',
        message: `The cited passage could not be found in this document's text; showing it from the start.${truncatedNote}`,
      });
    } else if (match.partial || match.coverage < 0.6) {
      onLocateRef.current({ status: 'approximate', message: `Highlighted the closest match; the document has changed or differs from the indexed text.${truncatedNote}` });
    } else {
      onLocateRef.current({ status: 'found', message: `Cited passage highlighted.${truncatedNote}` });
    }
  }, [source, haystack, passage]);

  useEffect(() => {
    const scroller = scrollerRef.current;
    const mark = firstMarkRef.current;
    if (!scroller || !mark || !ranges) return;
    const offset = mark.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    scroller.scrollTo({ top: Math.max(0, scroller.scrollTop + offset - scroller.clientHeight / 3), behavior: scrollBehavior() });
  }, [scrollToken, ranges]);

  if (!source) {
    return (
      <div className="flex-1 flex items-center justify-center gap-2 text-[13px] text-shodh-text-muted">
        <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        Loading document…
      </div>
    );
  }

  firstMarkRef.current = null;
  let firstAssigned = false;
  const renderBlock = (block: Block): React.ReactNode[] => {
    const nodes: React.ReactNode[] = [];
    let cursor = block.start;
    if (ranges) {
      for (const r of ranges) {
        const a = Math.max(r.start, block.start);
        const b = Math.min(r.end, block.end);
        if (a >= b) continue;
        if (a > cursor) nodes.push(text.slice(cursor, a));
        const isFirst = !firstAssigned;
        firstAssigned = true;
        nodes.push(
          <mark
            key={a}
            className="source-mark"
            ref={isFirst ? (el: HTMLElement | null) => { if (el) firstMarkRef.current = el; } : undefined}
          >
            {text.slice(a, b)}
          </mark>,
        );
        cursor = b;
      }
    }
    if (cursor < block.end) nodes.push(text.slice(cursor, block.end));
    return nodes;
  };

  return (
    <div
      ref={scrollerRef}
      tabIndex={0}
      aria-label="Document text"
      className={cn('flex-1 min-h-0 overflow-y-auto scrollbar-thin px-6 py-5', VIEWER_FOCUS_RING)}
    >
      {text.trim().length === 0 ? (
        <p className="text-[13.5px] text-shodh-text-muted">No text could be extracted from this document.</p>
      ) : code ? (
        <div className="font-mono text-[12.5px] leading-[1.6] text-shodh-text-secondary">
          {blocks.map(block => (
            <pre key={block.start} className="m-0 whitespace-pre-wrap break-words [content-visibility:auto] [contain-intrinsic-size:auto_320px]">
              {renderBlock(block)}
            </pre>
          ))}
        </div>
      ) : (
        <article className="max-w-[72ch] mx-auto flex flex-col gap-3.5 text-[14px] leading-[1.7] text-shodh-text-secondary">
          {blocks.map(block => (
            <p key={block.start} className="m-0 whitespace-pre-wrap break-words [content-visibility:auto] [contain-intrinsic-size:auto_4em]">
              {renderBlock(block)}
            </p>
          ))}
        </article>
      )}
      {source.truncated && (
        <p className="mt-6 text-[12.5px] text-shodh-text-muted">
          This document has {source.totalChars.toLocaleString()} characters; only the beginning is shown here. Open it in its default app to read the rest.
        </p>
      )}
    </div>
  );
}
