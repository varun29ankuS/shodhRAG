import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, CornerDownRight, Download, FileDown, Loader2, MoreHorizontal, Pencil, Pin, PinOff, Search, Trash2 } from 'lucide-react';
import { useTheme } from '../../contexts/ThemeContext';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { relativeTime } from '../../utils/time';
import { useChatSession } from '../ask/ChatSessionContext';
import { useFocus } from '../focus/focusHost';
import { deleteWithUndo, goToMessage, runExport } from './actions';
import { onVisualsChanged, toVisualError, visualsApi } from './api';
import { EditVisualDialog } from './EditVisualDialog';
import { KIND_NOUN, VISUAL_KINDS } from './extract';
import { exportFormats, FORMAT_LABEL } from './exportVisual';
import type { ExportFormat } from './exportVisual';
import { applyChange, recordTarget, visualRef } from './model';
import type { VisualSummary } from './model';
import { entryCounts, filterEntries, mergeGallery } from '../research/galleryUnion';
import type { GalleryFilter } from '../research/galleryUnion';
import { SnippetCard } from '../research/SnippetCard';
import { useSnippetList } from '../research/useSnippets';
import { backfillOnce } from './recording';
import { VisualThumb } from './VisualThumb';
import { exportPdf } from '../print/exportPdf';
import { buildPrintDocument, visualPrintMarkdown } from '../print/printModel';
import { agentItemKey } from '../../lib/agentHighlight';
import { scrollParentOf, tabPastWindow, useWindowedList } from '../../hooks/useWindowedList';
import { gridColumns, gridRows, rowOfCard } from '../../lib/windowedRows';
import { useNavigationTarget } from '../agent/useNavigationTarget';
import type { VisualTarget } from '../agent/events';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';
const MENU_ITEM =
  'w-full h-8 px-2.5 rounded-lg inline-flex items-center gap-2 text-start text-small text-shodh-text hover:bg-shodh-raised focus:bg-shodh-raised focus:outline-none disabled:opacity-40';

/** Cards loaded at once (search narrows beyond that). */
const PAGE = 500;

/** The grid: a card's least width, the space between cards (`gap-3`), a row's height before it is measured. */
const CARD_MIN_PX = 220;
const CARD_GAP_PX = 12;
const CARD_ROW_ESTIMATE = 240;
const SEARCH_DELAY_MS = 200;

function dateLabel(iso: string): string {
  const time = Date.parse(iso);
  if (!Number.isFinite(time)) return '';
  return new Date(time).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
}

/** The card's actions menu, inside the card (it must not leave a modal it may sit in). */
function CardMenu({
  card,
  onClose,
  onPin,
  onEdit,
  onExport,
  onExportPdf,
  onGoTo,
  onDelete,
}: {
  card: VisualSummary;
  onClose: (refocus: boolean) => void;
  onPin: () => void;
  onEdit: () => void;
  onExport: (format: ExportFormat) => void;
  onExportPdf: () => void;
  onGoTo: (() => void) | null;
  onDelete: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const onPointer = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose(false);
    };
    window.addEventListener('pointerdown', onPointer, true);
    return () => window.removeEventListener('pointerdown', onPointer, true);
  }, [onClose]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([disabled])') ?? []);
    const index = items.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const step = e.key === 'ArrowDown' ? 1 : -1;
      items[(index + step + items.length) % items.length]?.focus();
    } else if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      items[e.key === 'Home' ? 0 : items.length - 1]?.focus();
    } else if (e.key === 'Escape') {
      // Closes the menu only, not a dialog around the gallery.
      e.preventDefault();
      e.stopPropagation();
      onClose(true);
    } else if (e.key === 'Tab') {
      onClose(false);
    }
  };

  const act = (run: () => void) => () => {
    onClose(true);
    run();
  };

  return (
    <div
      ref={ref}
      role="menu"
      aria-label={`Actions for ${card.title}`}
      // Esc closes this menu only (dialogs around the gallery skip it).
      data-esc-local=""
      onKeyDown={onKeyDown}
      className="absolute end-2 top-10 z-20 min-w-[200px] rounded-xl border border-shodh-border-strong bg-shodh-surface p-1 shadow-[0_12px_36px_rgba(0,0,0,0.32)] shell-pop"
    >
      <button type="button" role="menuitem" tabIndex={-1} className={MENU_ITEM} onClick={act(onPin)}>
        {card.pinned ? <PinOff className="w-3.5 h-3.5" aria-hidden="true" /> : <Pin className="w-3.5 h-3.5" aria-hidden="true" />}
        {card.pinned ? 'Unpin' : 'Pin to top'}
      </button>
      <button type="button" role="menuitem" tabIndex={-1} className={MENU_ITEM} onClick={act(onEdit)}>
        <Pencil className="w-3.5 h-3.5" aria-hidden="true" />
        Rename and note…
      </button>
      {exportFormats(card.kind).map(format => (
        <button key={format} type="button" role="menuitem" tabIndex={-1} className={MENU_ITEM} onClick={act(() => onExport(format))}>
          <Download className="w-3.5 h-3.5" aria-hidden="true" />
          {`Export ${FORMAT_LABEL[format]}…`}
        </button>
      ))}
      <button type="button" role="menuitem" tabIndex={-1} className={MENU_ITEM} onClick={act(onExportPdf)}>
        <FileDown className="w-3.5 h-3.5" aria-hidden="true" />
        Export PDF…
      </button>
      <button type="button" role="menuitem" tabIndex={-1} className={MENU_ITEM} disabled={!onGoTo} onClick={onGoTo ? act(onGoTo) : undefined}>
        <CornerDownRight className="w-3.5 h-3.5" aria-hidden="true" />
        Go to message
      </button>
      <div className="my-1 h-px bg-shodh-border-subtle" role="separator" />
      <button type="button" role="menuitem" tabIndex={-1} className={cn(MENU_ITEM, 'text-shodh-error')} onClick={act(onDelete)}>
        <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
        Delete…
      </button>
    </div>
  );
}

function VisualCard({
  card,
  theme,
  conversationTitle,
  showConversation,
  onOpen,
  onChange,
  onOpenConversation,
}: {
  card: VisualSummary;
  theme: string;
  conversationTitle: string | null;
  showConversation: boolean;
  onOpen: (card: VisualSummary, trigger: HTMLElement) => void;
  onChange: (next: VisualSummary | null) => void;
  onOpenConversation: (card: VisualSummary) => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);
  const { switchConversation } = useChatSession();
  const titleId = useId();
  const noun = KIND_NOUN[card.kind];

  const closeMenu = useCallback((refocus: boolean) => {
    setMenuOpen(false);
    if (refocus) menuButtonRef.current?.focus();
  }, []);

  const pin = () => {
    const pinned = !card.pinned;
    onChange({ ...card, pinned, updatedAt: new Date().toISOString() });
    visualsApi.setPinned(card.id, pinned).catch(error => {
      onChange(card);
      notify.error(pinned ? 'The visual was not pinned' : 'The visual was not unpinned', { description: toVisualError(error).message });
    });
  };

  return (
    <li className="relative rounded-[14px]" data-agent-item={agentItemKey('visual', card.id)}>
      <article
        aria-labelledby={titleId}
        className="group relative flex flex-col rounded-[14px] border border-shodh-border bg-shodh-surface overflow-hidden hover:border-shodh-border-strong transition-colors duration-micro"
      >
        <button
          type="button"
          onClick={e => onOpen(card, e.currentTarget)}
          aria-label={`Open ${noun.toLowerCase()} ${card.title}`}
          className={cn('text-start flex flex-col', FOCUS_RING, 'focus-visible:ring-offset-0 rounded-[14px]')}
        >
          <div ref={thumbRef}>
            <VisualThumb record={card} theme={theme} className="border-b border-shodh-border-subtle" />
          </div>
          <div className="px-3 pt-2.5 pb-1 flex flex-col gap-0.5 min-w-0">
            <h3 id={titleId} className="text-small font-semibold text-shodh-text truncate" title={card.title}>
              {card.pinned && <Pin className="inline w-3 h-3 me-1 -mt-0.5 text-shodh-accent-text" aria-label="Pinned" />}
              {card.title}
            </h3>
            <p className="text-caption text-shodh-text-muted truncate">
              <span>{noun}</span>
              {card.versionCount > 1 && <span>{` · v${card.versionCount}`}</span>}
              <span>{' · '}</span>
              <time dateTime={card.firstCreatedAt} title={relativeTime(card.updatedAt)}>{dateLabel(card.firstCreatedAt)}</time>
            </p>
            {card.note && <p className="text-caption text-shodh-text-secondary line-clamp-2">{card.note}</p>}
          </div>
        </button>
        {showConversation && (
          <div className="px-3 pb-2.5 text-caption text-shodh-text-muted truncate">
            {'from '}
            <button
              type="button"
              onClick={() => onOpenConversation(card)}
              className={cn('underline decoration-dotted underline-offset-2 hover:text-shodh-text rounded', FOCUS_RING)}
              title={conversationTitle ?? 'A deleted conversation'}
              disabled={!conversationTitle}
            >
              {conversationTitle ?? 'a deleted conversation'}
            </button>
          </div>
        )}
        <button
          ref={menuButtonRef}
          type="button"
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          aria-label={`Actions for ${card.title}`}
          onClick={() => setMenuOpen(o => !o)}
          className={cn(
            'absolute end-2 top-2 w-7 h-7 inline-flex items-center justify-center rounded-lg bg-shodh-surface/90 border border-shodh-border text-shodh-text-muted hover:text-shodh-text',
            'opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 aria-expanded:opacity-100 transition-opacity duration-micro',
            FOCUS_RING,
          )}
        >
          <MoreHorizontal className="w-4 h-4" aria-hidden="true" />
        </button>
        {menuOpen && (
          <CardMenu
            card={card}
            onClose={closeMenu}
            onPin={pin}
            onEdit={() => setEditing(true)}
            onExport={format => void runExport(card, format, thumbRef.current, theme === 'dark')}
            onExportPdf={() => void exportPdf(buildPrintDocument({
              title: card.title || KIND_NOUN[card.kind],
              subtitle: `${KIND_NOUN[card.kind]} from Shodh`,
              createdAt: card.updatedAt,
              markdown: visualPrintMarkdown(card),
            }))}
            onGoTo={card.messageId ? () => goToMessage(card, switchConversation) : null}
            onDelete={() => {
              void deleteWithUndo(card).then(deleted => {
                if (deleted) onChange(null);
              });
            }}
          />
        )}
      </article>
      <EditVisualDialog
        record={card}
        open={editing}
        onOpenChange={setEditing}
        onSaved={detail => onChange({ ...card, title: detail.visual.title, note: detail.visual.note, updatedAt: detail.visual.updatedAt })}
      />
    </li>
  );
}

/**
 * Generated visuals as a grid of thumbnails: kind filter, search (titles,
 * notes and sources), pinned first. A card opens the visual in the focus
 * pop-out. `conversationId` scopes it to one conversation; null shows all.
 */
export function VisualGallery({
  conversationId,
  conversationIds = null,
  autoFocusSearch = false,
  emptyText,
}: {
  conversationId: string | null;
  /** Only visuals of these conversations (a workspace's chats); null applies no limit. */
  conversationIds?: readonly string[] | null;
  autoFocusSearch?: boolean;
  emptyText?: string;
}) {
  const { theme } = useTheme();
  const focus = useFocus();
  const { conversations, switchConversation } = useChatSession();
  const searchId = useId();
  const [items, setItems] = useState<VisualSummary[]>([]);
  const [total, setTotal] = useState(0);
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading');
  const [error, setError] = useState<string | null>(null);
  const [kind, setKind] = useState<GalleryFilter>('all');
  const [query, setQuery] = useState('');
  const [debounced, setDebounced] = useState('');
  const [reload, setReload] = useState(0);
  const [backfilled, setBackfilled] = useState(false);
  // A stable key, so a new array with the same ids does not reload the gallery.
  const conversationIdsKey = conversationIds === null ? null : conversationIds.join(String.fromCharCode(31));
  const conversationIdList = useMemo(
    () => (conversationIdsKey === null ? null : conversationIdsKey === '' ? [] : conversationIdsKey.split(String.fromCharCode(31))),
    [conversationIdsKey],
  );

  // Conversations saved before the gallery existed are recorded once.
  useEffect(() => {
    let cancelled = false;
    backfillOnce()
      .catch(err => console.warn('Recording earlier visuals failed:', toVisualError(err).message))
      .finally(() => { if (!cancelled) setBackfilled(true); });
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(query.trim()), SEARCH_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [query]);

  useEffect(() => {
    let cancelled = false;
    setStatus(s => (s === 'ready' ? s : 'loading'));
    visualsApi.list({ conversationId, conversationIds: conversationIdList, text: debounced || null, limit: PAGE })
      .then(page => {
        if (cancelled) return;
        setItems(page.items);
        setTotal(page.total);
        setStatus('ready');
        setError(null);
      })
      .catch(err => {
        if (cancelled) return;
        setStatus('error');
        setError(toVisualError(err).message);
      });
    return () => { cancelled = true; };
  }, [conversationId, conversationIdsKey, debounced, reload, backfilled]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    onVisualsChanged(changed => {
      if (changed === null || conversationId === null || changed === conversationId) setReload(r => r + 1);
    }).then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [conversationId]);

  // Snippets join the gallery of every conversation (they belong to papers, not conversations).
  const snippetQuery = useMemo(() => ({ text: debounced || null, limit: PAGE }), [debounced]);
  const { state: snippetState, replace: replaceSnippet } = useSnippetList(snippetQuery, conversationId === null);
  const snippets = conversationId === null && snippetState.status === 'ready' ? snippetState.items : [];
  const entries = useMemo(() => mergeGallery(items, snippets), [items, snippets]);
  const shown = useMemo(() => filterEntries(entries, kind), [entries, kind]);
  const counts = useMemo(() => entryCounts(entries), [entries]);
  const titles = useMemo(() => new Map(conversations.map(c => [c.id, c.title])), [conversations]);

  // The grid draws only the rows of cards in view (useWindowedList), in whatever scrolls
  // around it (the Artifacts page, a workspace's page, the gallery dialog).
  const [gridNode, setGridNode] = useState<HTMLUListElement | null>(null);
  const [gridWidth, setGridWidth] = useState(0);
  const scroller = useMemo(() => (gridNode ? scrollParentOf(gridNode) : null), [gridNode]);
  useLayoutEffect(() => {
    if (!gridNode) return;
    const measure = () => setGridWidth(gridNode.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(gridNode);
    return () => observer.disconnect();
  }, [gridNode]);
  const columns = gridColumns(gridWidth, CARD_MIN_PX, CARD_GAP_PX);
  const cardRows = useMemo(() => gridRows(shown.length, columns), [shown.length, columns]);
  const windowed = useWindowedList({
    count: cardRows.length,
    scroller,
    list: gridNode,
    estimate: CARD_ROW_ESTIMATE,
    getKey: index => shown[cardRows[index].first].key,
    gap: CARD_GAP_PX,
  });
  const revealCardRow = windowed.reveal;
  // Tab past the last card drawn goes on to the next card, drawn first.
  const onGridKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLUListElement>) => tabPastWindow(e, shown.length, card => rowOfCard(card, columns), revealCardRow),
    [shown.length, columns, revealCardRow],
  );
  // A visual the assistant changed (its card is highlighted, VisualNavigator): its row is
  // drawn and scrolled to first, so the highlight finds it.
  const shownRef = useRef(shown);
  shownRef.current = shown;
  const columnsRef = useRef(columns);
  columnsRef.current = columns;
  const revealVisual = useCallback((target: VisualTarget) => {
    if (!target.highlightOnly) return;
    const index = shownRef.current.findIndex(entry => entry.kind !== 'snippet' && entry.visual.id === target.visualId);
    if (index >= 0) void revealCardRow(rowOfCard(index, columnsRef.current), 'center');
  }, [revealCardRow]);
  useNavigationTarget('visual', revealVisual, { consume: false });

  const open = useCallback((card: VisualSummary, trigger: HTMLElement) => {
    const target = recordTarget(card);
    if (!focus || !target) {
      notify.error('This visual cannot be drawn');
      return;
    }
    focus.openFocus({
      target,
      conversationId: card.conversationId,
      parentMessageId: card.messageId,
      trigger,
      visual: visualRef(card),
    });
  }, [focus]);

  const change = useCallback((card: VisualSummary) => (next: VisualSummary | null) => {
    setItems(list => (next
      ? applyChange(list, { type: 'replaced', card: next })
      : applyChange(list, { type: 'removed', rootId: card.rootId })));
    if (!next) setTotal(t => Math.max(0, t - 1));
  }, []);

  const openConversation = useCallback((card: VisualSummary) => {
    switchConversation(card.conversationId);
    window.dispatchEvent(new CustomEvent('switchTab', { detail: 'ask' }));
  }, [switchConversation]);

  const chip = (value: GalleryFilter, label: string, count: number) => (
    <button
      key={value}
      type="button"
      role="radio"
      aria-checked={kind === value}
      onClick={() => setKind(value)}
      className={cn(
        'h-7 px-2.5 rounded-full text-caption inline-flex items-center gap-1.5 border transition-colors duration-micro',
        kind === value
          ? 'bg-shodh-accent-soft border-shodh-accent text-shodh-accent-text'
          : 'border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text',
        FOCUS_RING,
      )}
    >
      {label}
      <span className="tabular-nums text-shodh-text-muted">{count}</span>
    </button>
  );

  return (
    <div className="flex flex-col gap-3 min-h-0">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative flex-1 min-w-[200px] max-w-[360px]">
          <Search className="absolute start-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-shodh-text-faint" aria-hidden="true" />
          <label htmlFor={searchId} className="sr-only">Search visuals</label>
          <input
            id={searchId}
            type="search"
            value={query}
            onChange={e => setQuery(e.target.value)}
            placeholder="Search titles, notes and content"
            autoFocus={autoFocusSearch}
            className="w-full h-8 ps-8 pe-2.5 rounded-lg border border-shodh-border bg-shodh-surface-2 text-small text-shodh-text placeholder:text-shodh-text-faint focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          />
        </div>
        <div role="radiogroup" aria-label="Kind" className="flex flex-wrap items-center gap-1.5">
          {chip('all', 'All', entries.length)}
          {VISUAL_KINDS.filter(k => (counts[k] ?? 0) > 0).map(k => chip(k, `${KIND_NOUN[k]}s`, counts[k] ?? 0))}
          {(counts.snippet ?? 0) > 0 && chip('snippet', 'Snippets', counts.snippet ?? 0)}
        </div>
      </div>

      <p className="sr-only" role="status" aria-live="polite">
        {status === 'ready' ? `${shown.length} ${shown.length === 1 ? 'item' : 'items'}` : ''}
      </p>

      {status === 'loading' && items.length === 0 ? (
        <div className="flex items-center gap-2 py-10 justify-center text-small text-shodh-text-muted">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          Loading visuals…
        </div>
      ) : status === 'error' ? (
        <div role="alert" className="flex items-start gap-2 rounded-xl border border-shodh-border bg-shodh-surface px-4 py-3 text-small text-shodh-text-secondary">
          <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
          <span className="flex-1">{`The gallery could not be loaded: ${error ?? ''}`}</span>
          <button type="button" onClick={() => setReload(r => r + 1)} className={cn('h-7 px-2.5 rounded-lg border border-shodh-border hover:bg-shodh-raised', FOCUS_RING)}>
            Try again
          </button>
        </div>
      ) : shown.length === 0 ? (
        <p className="py-8 text-center text-small text-shodh-text-muted">
          {debounced || kind !== 'all'
            ? 'No visual matches.'
            : emptyText ?? 'Nothing yet'}
        </p>
      ) : (
        <>
          <ul
            ref={setGridNode}
            className="relative m-0 p-0 list-none"
            style={{ height: windowed.total }}
            aria-label="Visuals"
            onKeyDown={onGridKeyDown}
          >
            {windowed.rows.map(row => {
              const { first, last } = cardRows[row.index];
              return (
            <div
              key={row.key}
              ref={windowed.virtualizer.measureElement}
              data-index={row.index}
              role="none"
              className="absolute start-0 end-0 grid gap-3"
              style={{ top: row.top, gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
            >
            {shown.slice(first, last + 1).map((entry, offset) => (
              <div key={entry.key} role="none" className="contents" data-card-index={first + offset}>
              {entry.kind === 'snippet' ? (
                <SnippetCard key={entry.key} snippet={entry.snippet} onChange={next => replaceSnippet(entry.snippet.id, next)} />
              ) : (
                <VisualCard
                  key={entry.key}
                  card={entry.visual}
                  theme={theme}
                  conversationTitle={titles.get(entry.visual.conversationId) ?? null}
                  showConversation={conversationId === null}
                  onOpen={open}
                  onChange={change(entry.visual)}
                  onOpenConversation={openConversation}
                />
              )}
              </div>
            ))}
            </div>
              );
            })}
          </ul>
          {total > items.length && (
            <p className="text-caption text-shodh-text-muted">{`Showing the first ${items.length} of ${total}. Search to find others.`}</p>
          )}
        </>
      )}
    </div>
  );
}
