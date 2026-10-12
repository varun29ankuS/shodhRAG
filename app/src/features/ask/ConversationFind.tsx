import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ChevronDown, ChevronUp, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import {
  findInMessages,
  occurrenceNumber,
  occurrenceOffsets,
  searchableText,
  startCursor,
  stepCursor,
  totalOccurrences,
} from './conversationWindow';
import type { FindCursor } from './conversationWindow';
import type { ChatMessage } from './types';

/** CSS custom highlight names (styled in index.css). */
const HIGHLIGHT_ALL = 'conversation-find';
const HIGHLIGHT_CURRENT = 'conversation-find-current';

/** Typing pauses this long before the conversation is searched again. */
const QUERY_SETTLE_MS = 120;

const FOCUS_RING = 'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring';

/** Searchable text per message object; streamed frames make new objects only for the live answer. */
const textCache = new WeakMap<ChatMessage, string>();
function textOf(message: ChatMessage): string {
  let text = textCache.get(message);
  if (text === undefined) {
    text = searchableText(message.content);
    textCache.set(message, text);
  }
  return text;
}

function highlightRegistry(): HighlightRegistry | null {
  return typeof CSS !== 'undefined' && 'highlights' in CSS && typeof Highlight !== 'undefined' ? CSS.highlights : null;
}

function clearHighlights() {
  const registry = highlightRegistry();
  registry?.delete(HIGHLIGHT_ALL);
  registry?.delete(HIGHLIGHT_CURRENT);
}

/** Ranges of each occurrence of `query` in the text shown inside `root` (matches may span elements). */
function rangesIn(root: HTMLElement, query: string): Range[] {
  const nodes: Text[] = [];
  const starts: number[] = [];
  let text = '';
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: node => (node.parentElement?.closest('script, style, [aria-hidden="true"]') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    nodes.push(node as Text);
    starts.push(text.length);
    text += node.nodeValue ?? '';
  }
  const locate = (offset: number, end: boolean): [Text, number] => {
    // The last node starting at or before the offset (for an end, strictly before).
    let lo = 0;
    let hi = nodes.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (end ? starts[mid] < offset : starts[mid] <= offset) lo = mid;
      else hi = mid - 1;
    }
    return [nodes[lo], offset - starts[lo]];
  };
  return occurrenceOffsets(text, query).map(([start, end]) => {
    const range = document.createRange();
    const [startNode, startOffset] = locate(start, false);
    const [endNode, endOffset] = locate(end, true);
    range.setStart(startNode, startOffset);
    range.setEnd(endNode, endOffset);
    return range;
  });
}

export interface ConversationFindProps {
  messages: readonly ChatMessage[];
  /** Index of the first message in view, where a new search starts. */
  firstVisibleIndex: () => number;
  /** Bring a message into view (mounting it) and return its element. */
  reveal: (index: number) => Promise<HTMLElement | null>;
  /** The message being shown as the current match (kept mounted), or null. */
  onTarget: (index: number | null) => void;
  onClose: () => void;
}

/**
 * Find in the conversation (Ctrl+F in Ask). Answers outside the view are not
 * mounted, so the browser's own find cannot see them: this searches every
 * message's text, brings the match into view and highlights it.
 */
export function ConversationFind({ messages, firstVisibleIndex, reveal, onTarget, onClose }: ConversationFindProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [draft, setDraft] = useState('');
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState<FindCursor | null>(null);
  const [shownOccurrence, setShownOccurrence] = useState<number | null>(null);
  const showToken = useRef(0);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  useEffect(() => {
    if (draft === query) return;
    const timer = window.setTimeout(() => setQuery(draft), QUERY_SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [draft, query]);

  const texts = useMemo(() => messages.map(textOf), [messages]);
  const matches = useMemo(() => findInMessages(texts, query), [texts, query]);
  const total = totalOccurrences(matches);

  // A new query starts at the first match from the view onwards.
  const firstVisibleRef = useRef(firstVisibleIndex);
  firstVisibleRef.current = firstVisibleIndex;
  useEffect(() => {
    setCursor(matches.length > 0 ? startCursor(matches, firstVisibleRef.current()) : null);
    // Matches change with streamed text too; only a new query moves the cursor.
  }, [query]);

  const matchIndex = cursor && matches[cursor.match] ? matches[cursor.match].index : null;
  const occurrence = cursor?.occurrence ?? 0;

  // Show the current match: mount and scroll to its message, highlight there.
  useEffect(() => {
    const token = ++showToken.current;
    clearHighlights();
    setShownOccurrence(null);
    if (matchIndex === null || query.trim() === '') {
      onTarget(null);
      return;
    }
    onTarget(matchIndex);
    void reveal(matchIndex).then(element => {
      if (token !== showToken.current || !element) return;
      const ranges = rangesIn(element, query.trim());
      if (ranges.length === 0) {
        element.scrollIntoView({ block: 'center' });
        return;
      }
      const current = ranges[Math.min(occurrence, ranges.length - 1)];
      const registry = highlightRegistry();
      if (registry) {
        registry.set(HIGHLIGHT_ALL, new Highlight(...ranges));
        registry.set(HIGHLIGHT_CURRENT, new Highlight(current));
      }
      setShownOccurrence(Math.min(occurrence, ranges.length - 1));
      const anchor = current.startContainer.parentElement;
      anchor?.scrollIntoView({ block: 'center' });
    });
  }, [matchIndex, occurrence, query, reveal, onTarget]);

  useEffect(() => () => {
    showToken.current += 1;
    clearHighlights();
  }, []);

  const step = useCallback((direction: 1 | -1) => {
    if (query !== draft) {
      setQuery(draft);
      return;
    }
    setCursor(c => (c && matches.length > 0 ? stepCursor(matches, c, direction) : c));
  }, [matches, query, draft]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      step(e.shiftKey ? -1 : 1);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
  };

  const settled = query === draft;
  const status = draft.trim() === ''
    ? ''
    : !settled
      ? 'Searching…'
      : total === 0
        ? 'No matches'
        : cursor
          ? `${occurrenceNumber(matches, { match: cursor.match, occurrence: shownOccurrence ?? cursor.occurrence })} of ${total}`
          : `${total} matches`;

  return (
    <div
      role="search"
      aria-label="Find in this conversation"
      data-esc-local
      className="ask-fade-in flex items-center gap-1 h-10 pl-3 pr-1 rounded-xl border border-shodh-border-strong bg-shodh-surface shadow-[0_6px_20px_rgba(0,0,0,0.14)]"
    >
      <input
        ref={inputRef}
        type="text"
        value={draft}
        onChange={e => setDraft(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder="Find in conversation"
        aria-label="Find in conversation"
        spellCheck={false}
        className="w-48 min-w-0 bg-transparent text-[13px] text-shodh-text placeholder:text-shodh-text-faint outline-none"
      />
      <span className="min-w-[72px] text-right text-[12px] tabular-nums text-shodh-text-muted" role="status" aria-live="polite">
        {status}
      </span>
      <button
        type="button"
        onClick={() => step(-1)}
        disabled={total === 0}
        aria-label="Previous match"
        title="Previous match (Shift+Enter)"
        className={cn('w-7 h-7 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-40 transition-colors duration-micro', FOCUS_RING)}
      >
        <ChevronUp className="w-4 h-4" aria-hidden="true" />
      </button>
      <button
        type="button"
        onClick={() => step(1)}
        disabled={total === 0}
        aria-label="Next match"
        title="Next match (Enter)"
        className={cn('w-7 h-7 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-40 transition-colors duration-micro', FOCUS_RING)}
      >
        <ChevronDown className="w-4 h-4" aria-hidden="true" />
      </button>
      <button
        type="button"
        onClick={onClose}
        aria-label="Close find"
        title="Close (Esc)"
        className={cn('w-7 h-7 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro', FOCUS_RING)}
      >
        <X className="w-4 h-4" aria-hidden="true" />
      </button>
    </div>
  );
}
