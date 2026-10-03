import React, { useEffect, useId, useMemo, useRef, useState } from 'react';
import { ChevronRight, CornerDownRight } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { FocusThread } from './focusTypes';
import { buildThreadTree, flattenTree, treeKey } from './threadTree';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset';

export interface ExplorationMapProps {
  /** Every side thread of this answer (or of this device's object). */
  threads: readonly FocusThread[];
  /** Thread shown in the pop-out now. */
  currentThreadId: string;
  onOpen: (threadId: string) => void;
  onClose: () => void;
}

/**
 * The exploration map: every side discussion of this answer as a tree
 * (nested discussions under the answer they were opened from), the one on
 * screen highlighted. A WAI-ARIA tree: ↑/↓ move, →/← expand, collapse or
 * move to the parent, Home/End, Enter opens.
 */
export function ExplorationMap({ threads, currentThreadId, onOpen, onClose }: ExplorationMapProps) {
  const headingId = useId();
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const roots = useMemo(() => buildThreadTree(threads), [threads]);
  const rows = useMemo(() => flattenTree(roots, collapsed), [roots, collapsed]);
  const [active, setActive] = useState<string | null>(() => (rows.some(r => r.id === currentThreadId) ? currentThreadId : rows[0]?.id ?? null));
  const itemRefs = useRef(new Map<string, HTMLLIElement>());

  // Move focus into the tree when it opens (later moves focus the row they move to).
  const initial = useRef(active);
  useEffect(() => {
    if (initial.current) itemRefs.current.get(initial.current)?.focus();
  }, []);

  const move = (id: string) => {
    setActive(id);
    itemRefs.current.get(id)?.focus();
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLUListElement>) => {
    if (!active) return;
    const result = treeKey(rows, active, e.key);
    if (!result) return;
    e.preventDefault();
    e.stopPropagation();
    switch (result.type) {
      case 'focus':
        move(result.id);
        break;
      case 'expand':
        setCollapsed(prev => {
          const next = new Set(prev);
          next.delete(result.id);
          return next;
        });
        break;
      case 'collapse':
        setCollapsed(prev => new Set(prev).add(result.id));
        break;
      case 'activate':
        onOpen(result.id);
        break;
    }
  };

  const toggle = (id: string) => {
    setCollapsed(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  return (
    <div
      role="region"
      aria-labelledby={headingId}
      className="ask-fade-in absolute right-3 top-full mt-1.5 z-30 w-[min(380px,calc(100%-24px))] max-h-[min(420px,60vh)] flex flex-col rounded-xl border border-shodh-border-strong bg-shodh-surface shadow-[0_12px_40px_rgba(0,0,0,0.35)]"
    >
      <div className="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-shodh-border-subtle">
        <h3 id={headingId} className="text-[12.5px] font-semibold text-shodh-text mr-auto">Exploration map</h3>
        <button
          type="button"
          onClick={onClose}
          className={cn('h-6 px-2 rounded-md text-[11.5px] text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
        >
          Close
        </button>
      </div>
      {rows.length === 0 ? (
        <p className="px-3 py-3 text-[12.5px] leading-relaxed text-shodh-text-muted">
          Side discussions appear here once you ask about something.
        </p>
      ) : (
        <ul role="tree" aria-labelledby={headingId} onKeyDown={onKeyDown} className="min-h-0 overflow-y-auto scrollbar-thin py-1">
          {rows.map(row => {
            const current = row.id === currentThreadId;
            return (
              <li
                key={row.id}
                ref={el => {
                  if (el) itemRefs.current.set(row.id, el);
                  else itemRefs.current.delete(row.id);
                }}
                role="treeitem"
                aria-level={row.level}
                aria-posinset={row.posInSet}
                aria-setsize={row.setSize}
                aria-expanded={row.hasChildren ? row.expanded : undefined}
                aria-current={current ? 'true' : undefined}
                aria-selected={row.id === active}
                tabIndex={row.id === active ? 0 : -1}
                onFocus={() => setActive(row.id)}
                onClick={() => onOpen(row.id)}
                className={cn(
                  'flex items-center gap-1 min-w-0 h-8 pr-2 cursor-pointer text-[12.5px] transition-colors duration-micro',
                  current ? 'bg-shodh-accent-soft text-shodh-text font-medium' : 'text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text',
                  FOCUS_RING,
                )}
                style={{ paddingLeft: 8 + (row.level - 1) * 16 }}
              >
                {row.hasChildren ? (
                  <span
                    aria-hidden="true"
                    onClick={e => {
                      e.stopPropagation();
                      toggle(row.id);
                    }}
                    className="w-5 h-5 shrink-0 inline-flex items-center justify-center rounded hover:bg-shodh-raised-2"
                  >
                    <ChevronRight className={cn('w-3.5 h-3.5 transition-transform duration-micro', row.expanded && 'rotate-90')} />
                  </span>
                ) : (
                  <span aria-hidden="true" className="w-5 h-5 shrink-0 inline-flex items-center justify-center text-shodh-text-faint">
                    {row.level > 1 && <CornerDownRight className="w-3 h-3" />}
                  </span>
                )}
                <span className="truncate" title={row.label}>{row.label}</span>
                {current && <span className="sr-only">(shown now)</span>}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
