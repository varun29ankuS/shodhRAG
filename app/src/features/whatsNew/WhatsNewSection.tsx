/**
 * Library → What's new: files new in the Library's watched folders, newest first.
 *
 * - The list always reserves {@link VISIBLE_ROWS} rows of height, so new files never
 *   push the folder cards below it down; "Show all" expands in place, only when clicked.
 * - Every state is visible: loading, an error with Try again, nothing new, the rows.
 * - File names come from the user's folders and render as plain text.
 * - Each watched folder has a switch; one stopped after failing shows Resume.
 * - Papers that paper alerts found are rows too: the title opens the paper's page,
 *   Abstract shows the stored abstract (nothing is fetched), and Add to library saves
 *   the open-access PDF the API reported into a watched folder. Each alert shows its
 *   state in a word and a line, with its one fix.
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ChevronDown, ChevronRight, ExternalLink, Plus, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { agentItemKey } from '../../lib/agentHighlight';
import { openInDefaultApp } from '../library/fileActions';
import { whatsNewApi, WHATS_NEW_CHANGED_EVENT } from './api';
import type { AlertView, FeedItem, FolderWatchView, WhatsNewView } from './types';
import {
  BUCKET_LABEL,
  EMPTY_COPY,
  VISIBLE_ROWS,
  alertLine,
  blockedCopy,
  firstRows,
  landingOf,
  pathOf,
  pdfOf,
  rowCount,
  rowsOf,
  savedPathOf,
  sectionState,
  renderedIds,
  unseenIds,
  watchLine,
} from './model';
import { PaperAlertForm } from './PaperAlertForm';
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
  /** The alert form: a new alert, or rewording the alert with this id. */
  const [form, setForm] = useState<{ editing: AlertView | null } | null>(null);
  const [busyAlert, setBusyAlert] = useState<string | null>(null);
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
  const alerts = view?.alerts ?? [];
  const blocked = blockedCopy(view?.blockedReason ?? null);
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

  const saveAlert = useCallback(async (words: string, editing: AlertView | null) => {
    if (editing) await whatsNewApi.updateAlert(editing.id, { words });
    else await whatsNewApi.createAlert(words);
    setForm(null);
    await load();
  }, [load]);

  const changeAlert = useCallback(async (alert: AlertView, change: () => Promise<unknown>, failure: string) => {
    setBusyAlert(alert.id);
    try {
      await change();
      await load();
    } catch (e) {
      notify.error(failure, { description: describe(e) });
    } finally {
      setBusyAlert(null);
    }
  }, [load]);

  const addToLibrary = useCallback(async (item: FeedItem, folder: FolderWatchView) => {
    try {
      await whatsNewApi.addToLibrary(item.id, folder.sourceId);
      notify.success(`Saved to ${folder.folder}`, { description: item.title });
    } catch (e) {
      const message = describe(e);
      // The file may be saved even when its note in What's new failed: say which.
      if (message.startsWith('Saved to ')) notify.warning('Saved, but not noted in What’s new', { description: message });
      else notify.error('Could not add it to the library', { description: message });
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
        <button
          type="button"
          className={ROW_ACTION}
          onClick={() => setForm({ editing: null })}
          aria-expanded={form !== null && form.editing === null}
        >
          <Plus className="h-3.5 w-3.5" aria-hidden="true" />
          Paper alert
        </button>
      </div>

      {form && (
        <PaperAlertForm
          key={form.editing?.id ?? 'new'}
          initialWords={form.editing?.words ?? ''}
          submitLabel={form.editing ? 'Save' : 'Turn on'}
          blocked={blocked}
          onSubmit={words => saveAlert(words, form.editing)}
          onCancel={() => setForm(null)}
        />
      )}

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
                      folders={folders}
                      canSend={!blocked}
                      onAddToLibrary={addToLibrary}
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

      {alerts.length > 0 && (
        <ul className="flex flex-col gap-1" aria-label="Paper alerts">
          {blocked && (
            <li role="status" className="px-3 text-small text-shodh-text-secondary">
              {blocked} Change it in Settings → Privacy.
            </li>
          )}
          {alerts.map(alert => {
            const line = alertLine(alert);
            const busy = busyAlert === alert.id;
            return (
              <li
                key={alert.id}
                data-agent-item={agentItemKey('whats-new', `alert:${alert.id}`)}
                className="flex h-9 items-center justify-between gap-3 rounded-lg px-3 text-small"
              >
                <span className="min-w-0 truncate text-shodh-text" title={alert.words}>{alert.words}</span>
                <span className="flex min-w-0 shrink items-center gap-2 whitespace-nowrap">
                  <span className="font-medium text-shodh-text-secondary">{line.word}</span>
                  <span className="truncate text-shodh-text-muted" title={line.fact}>{line.fact}</span>
                  {line.fix === 'resume' && (
                    <button type="button" className={ROW_ACTION} disabled={busy} onClick={() => void changeAlert(alert, () => whatsNewApi.updateAlert(alert.id, { enabled: true }), 'The alert was not resumed')}>
                      Resume
                    </button>
                  )}
                  {line.fix === 'turn_on' && (
                    <button type="button" className={ROW_ACTION} disabled={busy} onClick={() => void changeAlert(alert, () => whatsNewApi.updateAlert(alert.id, { enabled: true }), 'The alert was not turned on')}>
                      Turn on
                    </button>
                  )}
                  {alert.state !== 'off' && alert.state !== 'held' && (
                    <button type="button" className={ROW_ACTION} disabled={busy} onClick={() => void changeAlert(alert, () => whatsNewApi.updateAlert(alert.id, { enabled: false }), 'The alert was not turned off')}>
                      Turn off
                    </button>
                  )}
                  <button type="button" className={ROW_ACTION} disabled={busy} onClick={() => setForm({ editing: alert })}>
                    Edit
                  </button>
                  <button
                    type="button"
                    className={ROW_ACTION}
                    disabled={busy}
                    aria-label={`Remove the alert for ${alert.words}`}
                    onClick={() => void changeAlert(alert, () => whatsNewApi.deleteAlert(alert.id), 'The alert was not removed')}
                  >
                    <X className="h-3.5 w-3.5" aria-hidden="true" />
                  </button>
                </span>
              </li>
            );
          })}
        </ul>
      )}

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
  folders,
  canSend,
  onAddToLibrary,
}: {
  row: Row;
  expanded: boolean;
  onToggle: () => void;
  onDismiss: () => void;
  folders: FolderWatchView[];
  canSend: boolean;
  onAddToLibrary: (item: FeedItem, folder: FolderWatchView) => Promise<void>;
}) {
  const first: FeedItem = row.items[0];
  if (row.kind === 'paper') {
    return (
      <PaperRow
        row={row}
        expanded={expanded}
        onToggle={onToggle}
        onDismiss={onDismiss}
        folders={folders.filter(f => f.on)}
        canSend={canSend}
        onAddToLibrary={onAddToLibrary}
      />
    );
  }
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

function PaperRow({
  row,
  expanded,
  onToggle,
  onDismiss,
  folders,
  canSend,
  onAddToLibrary,
}: {
  row: Row;
  expanded: boolean;
  onToggle: () => void;
  onDismiss: () => void;
  folders: FolderWatchView[];
  canSend: boolean;
  onAddToLibrary: (item: FeedItem, folder: FolderWatchView) => Promise<void>;
}) {
  const paper = row.items[0];
  const landing = landingOf(paper);
  const pdf = pdfOf(paper);
  const saved = savedPathOf(paper);
  const [choosing, setChoosing] = useState(false);
  /** A save is running: the button waits, so one paper is never saved twice. */
  const [adding, setAdding] = useState(false);
  const abstract = typeof paper.data.abstract === 'string' ? paper.data.abstract : null;
  const add = (folder: FolderWatchView) => {
    if (adding) return;
    setChoosing(false);
    setAdding(true);
    void onAddToLibrary(paper, folder).finally(() => setAdding(false));
  };
  return (
    <li className="flex flex-col" data-agent-item={agentItemKey('whats-new', paper.id)}>
      <div className="flex h-10 items-center gap-3 px-3">
        <span
          aria-label={row.isNew ? 'New' : undefined}
          className={cn('h-1.5 w-1.5 shrink-0 rounded-full', row.isNew ? 'bg-shodh-accent' : 'bg-transparent')}
        />
        <span className="min-w-0 flex-1 truncate text-small text-shodh-text" title={row.title}>{row.title}</span>
        <span className="hidden shrink-0 truncate text-caption text-shodh-text-muted sm:block max-w-[40%]" title={row.meta}>{row.meta}</span>
        <span className="flex shrink-0 flex-nowrap items-center gap-1">
          {landing && (
            <button type="button" className={ROW_ACTION} onClick={() => void openUrl(landing).catch(e => notify.error('The link could not be opened', { description: describe(e) }))}>
              <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
              Page
            </button>
          )}
          {abstract && (
            <button type="button" className={ROW_ACTION} onClick={onToggle} aria-expanded={expanded}>
              {expanded ? 'Hide' : 'Abstract'}
            </button>
          )}
          {saved ? (
            <button type="button" className={ROW_ACTION} onClick={() => void openInDefaultApp(saved)}>
              Open
            </button>
          ) : (
            pdf && canSend && folders.length > 0 && (
              <button
                type="button"
                className={ROW_ACTION}
                aria-expanded={choosing}
                aria-busy={adding}
                disabled={adding}
                onClick={() => (folders.length === 1 ? add(folders[0]) : setChoosing(v => !v))}
              >
                {adding ? 'Adding…' : 'Add to library'}
              </button>
            )
          )}
          <button type="button" className={ROW_ACTION} onClick={onDismiss} aria-label={`Remove ${row.title} from What's new`}>
            <X className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </span>
      </div>
      {choosing && (
        <ul className="flex flex-col pb-1 pl-8" aria-label="Save into">
          {folders.map(folder => (
            <li key={folder.sourceId} className="flex h-8 items-center">
              <button type="button" className={ROW_ACTION} disabled={adding} onClick={() => add(folder)}>
                Save into {folder.folder}
              </button>
            </li>
          ))}
        </ul>
      )}
      {expanded && abstract && (
        <p className="px-8 pb-2 text-small text-shodh-text-secondary whitespace-pre-line">{abstract}</p>
      )}
    </li>
  );
}
