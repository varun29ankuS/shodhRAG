import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import {
  Archive,
  Bookmark,
  Check,
  ChevronRight,
  CircleDot,
  Copy,
  Cpu,
  Download,
  Eraser,
  FileText,
  FolderOpen,
  GitBranch,
  Globe,
  History,
  KeyRound,
  Layers,
  MessageCircle,
  MessageSquareText,
  Plug,
  Quote,
  Search,
  ShieldCheck,
  SlidersHorizontal,
  Wrench,
} from 'lucide-react';
import { cn } from '../../lib/utils';
import { prettyJson } from '../agent/format';
import { scrubSecrets } from './scrub';
import { formatTimestamp } from './summary';
import { describeEvent, sentenceText } from './sentence';
import type { EventIcon, EventSentence } from './sentence';
import { eventLabel } from './types';
import type { AuditRow } from './types';
import { countOf } from '../../lib/plural';
import { notify } from '../../lib/notify';
import { agentItemKey, revealAgentItem } from '../../lib/agentHighlight';
import { announce } from '../../lib/announcer';

const ROW_HEIGHT = 34;
const OVERSCAN = 8;
/** Ask for the next page when this close to the end of the loaded rows. */
const LOAD_AHEAD = 30;

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-ground';

const GRID = 'grid grid-cols-[28px_132px_minmax(0,1fr)_120px] items-center gap-2';

const ICONS: Record<EventIcon, typeof Archive> = {
  question: MessageCircle,
  answer: MessageSquareText,
  tool: Wrench,
  search: Search,
  document: FileText,
  web: Globe,
  approval: ShieldCheck,
  source: FolderOpen,
  settings: SlidersHorizontal,
  model: Cpu,
  key: KeyRound,
  runtime: Download,
  retention: Archive,
  memory: Bookmark,
  forget: Eraser,
  recall: History,
  code: GitBranch,
  citation: Quote,
  scope: Layers,
  connector: Plug,
  other: CircleDot,
};

/** An entry's sentence: its icon, title and details, the new value in bold. */
function Sentence({ sentence, wrap = false }: { sentence: EventSentence; wrap?: boolean }) {
  const Icon = ICONS[sentence.icon];
  return (
    <span className={cn('flex items-center gap-2 min-w-0', wrap && 'items-start')}>
      <Icon className={cn('w-3.5 h-3.5 shrink-0 text-shodh-text-muted', wrap && 'mt-0.5')} aria-hidden="true" />
      <span className={cn('min-w-0', wrap ? 'break-words' : 'truncate')}>
        <span className="font-medium text-shodh-text">{sentence.title}</span>
        {sentence.details.length > 0 && <span className="text-shodh-text-faint"> · </span>}
        {sentence.details.map((segment, i) => (
          <span key={i} className={segment.strong ? 'font-semibold text-shodh-text' : 'text-shodh-text-secondary'}>
            {segment.text}
          </span>
        ))}
      </span>
    </span>
  );
}

/** The raw entry behind a disclosure, with a Copy button. */
function RawEvent({ row }: { row: AuditRow }) {
  const [copied, setCopied] = useState(false);
  // A field named for a secret is never shown or copied (no event writes one today).
  const json = useMemo(() => prettyJson(scrubSecrets(row.payload), 20_000), [row.payload]);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(JSON.stringify(scrubSecrets(row), null, 2));
      setCopied(true);
    } catch (error) {
      notify.error('Could not copy the entry', { description: error instanceof Error ? error.message : String(error) });
    }
  };
  return (
    <details className="group/raw">
      <summary
        className={cn(
          'list-none [&::-webkit-details-marker]:hidden inline-flex items-center gap-1 h-7 px-1.5 -ml-1.5 rounded-md cursor-pointer text-caption text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised',
          FOCUS_RING,
        )}
      >
        <ChevronRight aria-hidden="true" className="w-3.5 h-3.5 transition-transform duration-micro group-open/raw:rotate-90" />
        Raw entry (JSON)
      </summary>
      <div className="mt-1 flex flex-col gap-1.5">
        <button
          type="button"
          onClick={() => void copy()}
          className={cn(
            'self-start h-7 px-2 inline-flex items-center gap-1.5 rounded-md border border-shodh-border text-caption text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text',
            FOCUS_RING,
          )}
        >
          {copied ? <Check className="w-3.5 h-3.5" aria-hidden="true" /> : <Copy className="w-3.5 h-3.5" aria-hidden="true" />}
          {copied ? 'Copied' : 'Copy'}
        </button>
        <pre className="m-0 max-h-[320px] overflow-auto scrollbar-thin rounded-lg border border-shodh-border-subtle bg-shodh-surface px-3 py-2 font-mono text-caption leading-[1.55] text-shodh-text-secondary whitespace-pre-wrap break-words">
          {json}
        </pre>
      </div>
    </details>
  );
}

function shortId(id: string | null): string {
  if (!id) return '';
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

interface AuditTableProps {
  rows: AuditRow[];
  total: number;
  loading: boolean;
  onLoadMore: () => void;
  onSelectConversation: (conversationId: string) => void;
  /** An entry the assistant pointed at (show_audit): expanded, scrolled to and highlighted once loaded. */
  reveal?: { id: number; seq: number } | null;
  /** The reveal is done (found or known missing). */
  onRevealDone?: () => void;
}

/**
 * Windowed event table. Rows have a fixed height; one row at a time can be
 * expanded, and its measured detail height shifts the rows below it.
 */
export function AuditTable({ rows, total, loading, onLoadMore, onSelectConversation, reveal = null, onRevealDone }: AuditTableProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const detailRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(480);
  const [expandedId, setExpandedId] = useState<number | null>(null);
  const [detailHeight, setDetailHeight] = useState(0);

  const expandedIndex = useMemo(
    () => (expandedId === null ? -1 : rows.findIndex(r => r.id === expandedId)),
    [rows, expandedId],
  );
  const extra = expandedIndex >= 0 ? detailHeight : 0;

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    setViewport(el.clientHeight);
    const observer = new ResizeObserver(() => setViewport(el.clientHeight));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // Measure the expanded detail panel (it wraps, so its height varies).
  useLayoutEffect(() => {
    const el = detailRef.current;
    if (!el) {
      setDetailHeight(0);
      return;
    }
    setDetailHeight(el.offsetHeight);
    // A detached node reports 0; keep the last real height.
    const observer = new ResizeObserver(() => {
      if (el.offsetHeight > 0) setDetailHeight(el.offsetHeight);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [expandedId, expandedIndex]);

  const offsetOf = useCallback(
    (index: number) => index * ROW_HEIGHT + (expandedIndex >= 0 && index > expandedIndex ? extra : 0),
    [expandedIndex, extra],
  );

  const indexAt = useCallback(
    (y: number) => {
      if (expandedIndex >= 0 && y >= offsetOf(expandedIndex) + ROW_HEIGHT + extra) {
        return Math.floor((y - extra) / ROW_HEIGHT);
      }
      if (expandedIndex >= 0 && y >= offsetOf(expandedIndex)) return expandedIndex;
      return Math.floor(y / ROW_HEIGHT);
    },
    [expandedIndex, extra, offsetOf],
  );

  const first = Math.max(0, indexAt(scrollTop) - OVERSCAN);
  const last = Math.min(rows.length - 1, indexAt(scrollTop + viewport) + OVERSCAN);
  const totalHeight = rows.length * ROW_HEIGHT + extra;

  useEffect(() => {
    if (!loading && rows.length < total && last >= rows.length - LOAD_AHEAD) onLoadMore();
  }, [last, rows.length, total, loading, onLoadMore]);

  // The assistant pointed at an entry (rows loaded for the filters it set): load pages
  // until its row is there (rows are newest first, so a page whose oldest row is older
  // than the entry settles it), then expand it, bring it into the window and highlight
  // it (lib/agentHighlight). Not there: say so. Either way the request is then done, so
  // it never applies later under the user's hands.
  const revealedSeq = useRef(0);
  useEffect(() => {
    if (!reveal || reveal.seq === revealedSeq.current) return;
    const index = rows.findIndex(r => r.id === reveal.id);
    if (index >= 0) {
      revealedSeq.current = reveal.seq;
      setExpandedId(reveal.id);
      const el = scrollRef.current;
      if (el) el.scrollTop = Math.max(0, index * ROW_HEIGHT - el.clientHeight / 2);
      revealAgentItem(agentItemKey('audit', String(reveal.id)));
      onRevealDone?.();
      return;
    }
    if (loading) return;
    const oldest = rows.length > 0 ? rows[rows.length - 1].id : null;
    if (rows.length < total && (oldest === null || oldest > reveal.id)) {
      onLoadMore();
      return;
    }
    revealedSeq.current = reveal.seq;
    announce(`Shodh pointed at Activity entry ${reveal.id}, which is not in the log.`);
    onRevealDone?.();
  }, [reveal, rows, loading, total, onLoadMore, onRevealDone]);

  // The expanded row stays mounted even when scrolled out of the window, so
  // its measured detail height (which offsets the rows below) stays valid.
  const visibleIndexes: number[] = [];
  if (expandedIndex >= 0 && expandedIndex < first) visibleIndexes.push(expandedIndex);
  for (let i = first; i <= last; i++) visibleIndexes.push(i);
  if (expandedIndex > last) visibleIndexes.push(expandedIndex);

  return (
    <div
      role="table"
      aria-label="Audit events, newest first"
      aria-rowcount={total + 1}
      className="flex flex-col min-h-0 rounded-xl border border-shodh-border-subtle bg-shodh-surface overflow-hidden"
    >
      <div role="rowgroup">
        <div
          role="row"
          aria-rowindex={1}
          className={cn(
            GRID,
            'h-8 px-3 border-b border-shodh-border-subtle text-caption uppercase tracking-wide text-shodh-text-faint',
          )}
        >
          <span role="columnheader" className="sr-only">
            Details
          </span>
          <span role="columnheader">Time</span>
          <span role="columnheader">What happened</span>
          <span role="columnheader">Conversation</span>
        </div>
      </div>
      <div
        ref={scrollRef}
        role="rowgroup"
        onScroll={e => setScrollTop(e.currentTarget.scrollTop)}
        className="relative h-[480px] overflow-y-auto scrollbar-thin"
      >
        {rows.length === 0 ? (
          <p className="px-4 py-10 text-center text-small text-shodh-text-muted">
            {loading ? 'Loading events…' : 'No events match these filters.'}
          </p>
        ) : (
          <div style={{ height: totalHeight }} className="relative">
            {visibleIndexes.map(index => {
              const row = rows[index];
              const expanded = row.id === expandedId;
              const detailId = `audit-detail-${row.id}`;
              const sentence = describeEvent(row);
              const line = sentenceText(sentence);
              const toggle = () => setExpandedId(expanded ? null : row.id);
              return (
                <div
                  key={row.id}
                  data-agent-item={agentItemKey('audit', String(row.id))}
                  role="row"
                  aria-rowindex={index + 2}
                  className="absolute left-0 right-0"
                  style={{ top: offsetOf(index) }}
                >
                  {/* The whole row opens the entry; the chevron is its keyboard control. */}
                  <div
                    onClick={toggle}
                    className={cn(
                      GRID,
                      'px-3 text-small border-b border-shodh-border-subtle cursor-pointer',
                      expanded ? 'bg-shodh-raised' : 'hover:bg-shodh-raised',
                    )}
                    style={{ height: ROW_HEIGHT }}
                  >
                    <span role="cell">
                      <button
                        type="button"
                        aria-expanded={expanded}
                        aria-controls={expanded ? detailId : undefined}
                        aria-label={`${expanded ? 'Hide' : 'Show'} details of event ${row.id}`}
                        onClick={e => {
                          e.stopPropagation();
                          toggle();
                        }}
                        className={cn(
                          'w-6 h-6 grid place-items-center rounded-md text-shodh-text-faint hover:text-shodh-text hover:bg-shodh-raised-2',
                          FOCUS_RING,
                        )}
                      >
                        <ChevronRight
                          aria-hidden="true"
                          className={cn('w-3.5 h-3.5 transition-transform duration-micro', expanded && 'rotate-90')}
                        />
                      </button>
                    </span>
                    <span role="cell" className="tabular-nums text-shodh-text-secondary truncate">
                      <time dateTime={row.ts} title={row.ts}>
                        {formatTimestamp(row.ts)}
                      </time>
                    </span>
                    <span role="cell" className="min-w-0" title={line}>
                      <Sentence sentence={sentence} />
                    </span>
                    <span role="cell" className="truncate">
                      {row.conversationId ? (
                        <button
                          type="button"
                          onClick={e => {
                            e.stopPropagation();
                            if (row.conversationId) onSelectConversation(row.conversationId);
                          }}
                          title={`Show only conversation ${row.conversationId}`}
                          className={cn(
                            'font-mono text-caption text-shodh-text-faint hover:text-shodh-accent-text rounded',
                            FOCUS_RING,
                          )}
                        >
                          {shortId(row.conversationId)}
                        </button>
                      ) : null}
                    </span>
                  </div>
                  {expanded && (
                    <div
                      ref={detailRef}
                      id={detailId}
                      role="cell"
                      className="px-3 py-2 border-b border-shodh-border-subtle bg-shodh-ground flex flex-col gap-2"
                    >
                      <div className="text-small">
                        <Sentence sentence={sentence} wrap />
                      </div>
                      <dl className="m-0 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-caption">
                        <dt className="text-shodh-text-faint">Entry</dt>
                        <dd className="m-0 text-shodh-text-secondary">
                          <span className="font-mono">#{row.id}</span> · {eventLabel(row.eventType)} · {row.principal}
                        </dd>
                        {row.runId && (
                          <>
                            <dt className="text-shodh-text-faint">Run</dt>
                            <dd className="m-0 font-mono text-shodh-text-secondary truncate">{row.runId}</dd>
                          </>
                        )}
                        {row.profileId && (
                          <>
                            <dt className="text-shodh-text-faint">Profile</dt>
                            <dd className="m-0 font-mono text-shodh-text-secondary">{row.profileId}</dd>
                          </>
                        )}
                        <dt className="text-shodh-text-faint">Hash</dt>
                        <dd className="m-0 font-mono text-shodh-text-faint truncate" title={row.hash}>
                          {row.hash}
                        </dd>
                      </dl>
                      <RawEvent row={row} />
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
      <div className="h-8 px-3 flex items-center border-t border-shodh-border-subtle text-caption text-shodh-text-faint tabular-nums">
        {loading && rows.length > 0
          ? `Loading… ${rows.length.toLocaleString()} of ${total.toLocaleString()}`
          : `${rows.length.toLocaleString()} of ${countOf(total, 'event')}`}
      </div>
    </div>
  );
}
