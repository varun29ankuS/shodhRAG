/**
 * Library → What's new: files new in the Library's watched folders, newest first.
 *
 * - The list always reserves {@link VISIBLE_ROWS} rows of height, so new files never
 *   push the folder cards below it down; "Show all" expands in place, only when clicked.
 * - Every state is visible: loading, an error with Try again, nothing new, the rows.
 * - File names come from the user's folders and render as plain text.
 * - Each watched folder has a switch; one stopped after failing shows Resume.
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { ChevronDown, ChevronRight, ExternalLink, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { agentItemKey } from '../../lib/agentHighlight';
import { openInDefaultApp } from '../library/fileActions';
import { whatsNewApi, WHATS_NEW_CHANGED_EVENT } from './api';
import type { FeedItem, FolderWatchView, WhatsNewView } from './types';
import {
  BUCKET_LABEL,
  EMPTY_COPY,
  VISIBLE_ROWS,
  firstRows,
  pathOf,
  rowCount,
  rowsOf,
  sectionState,
  renderedIds,
  unseenIds,
  watchLine,
} from './model';
import type { Row } from './model';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const ROW_ACTION = cn(
  'h-7 px-2 inline-flex items-center gap-1 rounded-md text-small text-shodh-text-secondary whitespace-nowrap shrink-0 hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
  FOCUS_RING,
);

/** Height of one row (h-10), so the list reserves VISIBLE_ROWS of them. */
const ROW_HEIGHT_PX = 40;

/** How long the section stays on screen before its rows count as seen. */
const SEEN_AFTER_MS = 1000;

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === 'string' ? error : 'Unknown error';
}

export function WhatsNewSection() {
  const [view, setView] = useState<WhatsNewView | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [busyFolder, setBusyFolder] = useState<string | null>(null);
  const sectionRef = useRef<HTMLElement | null>(null);

  const load = useCallback(async () => {
    try {
      setView(await whatsNewApi.list());
      setError(null);
    } catch (e) {
      setError(describe(e));
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void load();
    const unlisten = [
      listen(WHATS_NEW_CHANGED_EVENT, () => void load()),
      listen('folder-sync', () => void load()),
    ];
    return () => {
      for (const off of unlisten) void off.then(fn => fn());
    };
  }, [load]);

  const items = view?.items ?? [];
  const folders = view?.folders ?? [];
  const buckets = useMemo(() => {
    const more = new Map(folders.map(f => [f.sourceId, f.moreByDay]));
    return rowsOf(items, new Date(), (sourceId, day) => more.get(sourceId)?.[day] ?? 0);
  }, [items, folders]);
  const total = rowCount(buckets);
  const shown = showAll ? buckets : firstRows(buckets, VISIBLE_ROWS);
  // Only what is on screen counts as seen: the rows shown, and a busy day once expanded.
  const unseen = useMemo(() => {
    const onScreen = new Set(renderedIds(shown, expanded));
    return unseenIds(items).filter(id => onScreen.has(id));
  }, [items, shown, expanded]);

  // Rows count as seen once the section has been on screen for a moment.
  useEffect(() => {
    const element = sectionRef.current;
    if (!element || unseen.length === 0 || typeof IntersectionObserver === 'undefined') return;
    let timer: number | undefined;
    const observer = new IntersectionObserver(entries => {
      const visible = entries.some(e => e.isIntersecting);
      if (visible && timer === undefined) {
        timer = window.setTimeout(() => {
          void whatsNewApi.markSeen(unseen).catch(() => {});
        }, SEEN_AFTER_MS);
      } else if (!visible && timer !== undefined) {
        window.clearTimeout(timer);
        timer = undefined;
      }
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [unseen]);

  const dismiss = useCallback(async (row: Row) => {
    try {
      for (const item of row.items) await whatsNewApi.dismiss(item.id, false);
      notify.success(row.kind === 'file' ? `Removed ${row.title} from What's new` : `Removed ${row.title}`, {
        action: {
          label: 'Undo',
          onClick: () => {
            void (async () => {
              try {
                for (const item of row.items) await whatsNewApi.dismiss(item.id, true);
              } catch (e) {
                notify.error('Could not bring it back', { description: describe(e) });
              }
            })();
          },
        },
      });
    } catch (e) {
      notify.error('Could not remove it', { description: describe(e) });
    }
  }, []);

  const setWatch = useCallback(async (folder: FolderWatchView, on: boolean) => {
    setBusyFolder(folder.sourceId);
    try {
      await whatsNewApi.setFolderWatch(folder.sourceId, on);
    } catch (e) {
      notify.error(`Could not change ${folder.folder}`, { description: describe(e) });
    } finally {
      setBusyFolder(null);
    }
  }, []);

  const toggleGroup = (key: string) =>
    setExpanded(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const state = sectionState({ loaded, error, items: items.length });

  return (
    <section
      ref={sectionRef}
      aria-labelledby="library-whats-new"
      className="flex flex-col gap-3"
      data-agent-item={agentItemKey('library-section', 'whats_new')}
    >
      <div className="flex items-end justify-between gap-3">
        <h2 id="library-whats-new" className="text-caption font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
          What's new
        </h2>
      </div>

      {error && state === 'list' && (
        <div role="alert" className="flex items-center justify-between gap-3 rounded-lg border border-shodh-border px-3 py-2 text-small text-shodh-text-secondary">
          <span className="truncate">Couldn't refresh What's new: {error}</span>
          <button type="button" className={ROW_ACTION} onClick={() => void load()}>Try again</button>
        </div>
      )}

      <div
        className="flex flex-col rounded-xl border border-shodh-border bg-shodh-surface"
        style={{ minHeight: VISIBLE_ROWS * ROW_HEIGHT_PX + 16 }}
      >
        {state === 'loading' && (
          <p aria-busy="true" className="m-auto text-small text-shodh-text-muted">Loading what's new…</p>
        )}
        {state === 'error' && (
          <div role="alert" className="m-auto flex flex-col items-center gap-2 px-4 text-center">
            <p className="text-small text-shodh-text-secondary">Couldn't load What's new: {error}</p>
            <button type="button" className={ROW_ACTION} onClick={() => void load()}>Try again</button>
          </div>
        )}
        {state === 'empty' && (
          <p className="m-auto max-w-md px-4 text-center text-small text-shodh-text-muted">{EMPTY_COPY}</p>
        )}
        {state === 'list' && (
          <ul className="flex flex-col py-2" aria-label="New files">
            {shown.map(bucket => (
              <li key={bucket.bucket} className="flex flex-col">
                <p className="px-3 pt-1.5 pb-1 text-caption font-medium text-shodh-text-faint">{BUCKET_LABEL[bucket.bucket]}</p>
                <ul className="flex flex-col">
                  {bucket.rows.map(row => (
                    <FeedRow
                      key={row.key}
                      row={row}
                      expanded={expanded.has(row.key)}
                      onToggle={() => toggleGroup(row.key)}
                      onDismiss={() => void dismiss(row)}
                    />
                  ))}
                </ul>
              </li>
            ))}
          </ul>
        )}
        {state === 'list' && total > VISIBLE_ROWS && (
          <div className="mt-auto flex justify-end px-3 pb-2">
            <button type="button" className={ROW_ACTION} onClick={() => setShowAll(v => !v)} aria-expanded={showAll}>
              {showAll ? 'Show fewer' : `Show all (${total})`}
            </button>
          </div>
        )}
      </div>

      {folders.length > 0 && (
        <ul className="flex flex-col gap-1" aria-label="Folders that show new files">
          {folders.map(folder => (
            <li
              key={folder.sourceId}
              data-agent-item={agentItemKey('whats-new', `folder:${folder.sourceId}`)}
              className="flex h-9 items-center justify-between gap-3 rounded-lg px-3 text-small"
            >
              <span className="min-w-0 truncate text-shodh-text" title={folder.folder}>{folder.folder}</span>
              <span className="flex shrink-0 items-center gap-2 whitespace-nowrap">
                <span className="text-shodh-text-muted">{watchLine(folder)}</span>
                {folder.held ? (
                  <button
                    type="button"
                    className={ROW_ACTION}
                    disabled={busyFolder === folder.sourceId}
                    onClick={() => void setWatch(folder, true)}
                  >
                    Resume
                  </button>
                ) : (
                  <button
                    type="button"
                    role="switch"
                    aria-checked={folder.on}
                    aria-label={`Show new files in ${folder.folder} in What's new`}
                    className={ROW_ACTION}
                    disabled={busyFolder === folder.sourceId}
                    onClick={() => void setWatch(folder, !folder.on)}
                  >
                    {folder.on ? 'Turn off' : 'Turn on'}
                  </button>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function FeedRow({
  row,
  expanded,
  onToggle,
  onDismiss,
}: {
  row: Row;
  expanded: boolean;
  onToggle: () => void;
  onDismiss: () => void;
}) {
  const first: FeedItem = row.items[0];
  const path = pathOf(first);
  return (
    <li className="flex flex-col" data-agent-item={agentItemKey('whats-new', first.id)}>
      <div className="flex h-10 items-center gap-3 px-3">
        <span
          aria-label={row.isNew ? 'New' : undefined}
          className={cn('h-1.5 w-1.5 shrink-0 rounded-full', row.isNew ? 'bg-shodh-accent' : 'bg-transparent')}
        />
        <span className="min-w-0 flex-1 truncate text-small text-shodh-text" title={row.title}>{row.title}</span>
        <span className="hidden shrink-0 truncate text-caption text-shodh-text-muted sm:block max-w-[40%]" title={row.meta}>{row.meta}</span>
        <span className="flex shrink-0 flex-nowrap items-center gap-1">
          {row.kind === 'file' ? (
            <button
              type="button"
              className={ROW_ACTION}
              disabled={!path}
              onClick={() => path && void openInDefaultApp(path)}
            >
              <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
              Open
            </button>
          ) : (
            <button type="button" className={ROW_ACTION} onClick={onToggle} aria-expanded={expanded}>
              {expanded ? <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" /> : <ChevronRight className="h-3.5 w-3.5" aria-hidden="true" />}
              {expanded ? 'Hide' : 'Show'}
            </button>
          )}
          <button type="button" className={ROW_ACTION} onClick={onDismiss} aria-label={`Remove ${row.title} from What's new`}>
            <X className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </span>
      </div>
      {row.kind === 'file_group' && expanded && (
        <ul className="flex flex-col pb-1 pl-8">
          {row.items.map(item => {
            const itemPath = pathOf(item);
            return (
              <li key={item.id} data-agent-item={agentItemKey('whats-new', item.id)} className="flex h-8 items-center gap-3 pr-3">
                <span className="min-w-0 flex-1 truncate text-small text-shodh-text-secondary" title={item.title}>{item.title}</span>
                <button
                  type="button"
                  className={ROW_ACTION}
                  disabled={!itemPath}
                  onClick={() => itemPath && void openInDefaultApp(itemPath)}
                >
                  Open
                </button>
              </li>
            );
          })}
          {row.more > 0 && (
            <li className="flex h-8 items-center text-caption text-shodh-text-muted">
              and {row.more} more, not listed one by one
            </li>
          )}
        </ul>
      )}
    </li>
  );
}
