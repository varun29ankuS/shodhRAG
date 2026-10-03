import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { cn } from '../../lib/utils';
import { prettyJson } from '../agent/format';
import { formatTimestamp, summarise } from './summary';
import { eventLabel } from './types';
import type { AuditRow } from './types';

const ROW_HEIGHT = 34;
const OVERSCAN = 8;
/** Ask for the next page when this close to the end of the loaded rows. */
const LOAD_AHEAD = 30;

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-ground';

const GRID = 'grid grid-cols-[28px_132px_96px_minmax(0,1fr)_120px] items-center gap-2';

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
}

/**
 * Windowed event table. Rows have a fixed height; one row at a time can be
 * expanded, and its measured detail height shifts the rows below it.
 */
export function AuditTable({ rows, total, loading, onLoadMore, onSelectConversation }: AuditTableProps) {
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
    const observer = new ResizeObserver(() => setDetailHeight(el.offsetHeight));
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

  const visible: AuditRow[] = rows.slice(first, last + 1);

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
            'h-8 px-3 border-b border-shodh-border-subtle text-[11px] uppercase tracking-wide text-shodh-text-faint',
          )}
        >
          <span role="columnheader" className="sr-only">
            Details
          </span>
          <span role="columnheader">Time</span>
          <span role="columnheader">Type</span>
          <span role="columnheader">Event</span>
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
          <p className="px-4 py-10 text-center text-[13px] text-shodh-text-muted">
            {loading ? 'Loading events…' : 'No events match these filters.'}
          </p>
        ) : (
          <div style={{ height: totalHeight }} className="relative">
            {visible.map((row, i) => {
              const index = first + i;
              const expanded = row.id === expandedId;
              const detailId = `audit-detail-${row.id}`;
              return (
                <div
                  key={row.id}
                  role="row"
                  aria-rowindex={index + 2}
                  className="absolute left-0 right-0"
                  style={{ top: offsetOf(index) }}
                >
                  <div
                    className={cn(
                      GRID,
                      'px-3 text-[12.5px] border-b border-shodh-border-subtle',
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
                        onClick={() => setExpandedId(expanded ? null : row.id)}
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
                    <span role="cell" className="truncate text-shodh-text-muted">
                      {eventLabel(row.eventType)}
                    </span>
                    <span role="cell" className="truncate text-shodh-text" title={summarise(row)}>
                      {summarise(row)}
                    </span>
                    <span role="cell" className="truncate">
                      {row.conversationId ? (
                        <button
                          type="button"
                          onClick={() => row.conversationId && onSelectConversation(row.conversationId)}
                          title={`Show only conversation ${row.conversationId}`}
                          className={cn(
                            'font-mono text-[11.5px] text-shodh-text-faint hover:text-shodh-accent-text rounded',
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
                      className="px-3 py-2 border-b border-shodh-border-subtle bg-shodh-ground"
                    >
                      <dl className="m-0 mb-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-[11.5px]">
                        <dt className="text-shodh-text-faint">Event</dt>
                        <dd className="m-0 font-mono text-shodh-text-secondary">
                          #{row.id} · {row.eventType} · {row.principal}
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
                      <pre className="m-0 max-h-[320px] overflow-auto scrollbar-thin rounded-lg border border-shodh-border-subtle bg-shodh-surface px-3 py-2 font-mono text-[11.5px] leading-[1.55] text-shodh-text-secondary whitespace-pre-wrap break-words">
                        {prettyJson(row.payload, 20_000)}
                      </pre>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
      <div className="h-8 px-3 flex items-center border-t border-shodh-border-subtle text-[11.5px] text-shodh-text-faint tabular-nums">
        {loading && rows.length > 0
          ? `Loading… ${rows.length.toLocaleString()} of ${total.toLocaleString()}`
          : `${rows.length.toLocaleString()} of ${total.toLocaleString()} events`}
      </div>
    </div>
  );
}
