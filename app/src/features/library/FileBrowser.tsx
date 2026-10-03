import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  AlertTriangle,
  ArrowDownAZ,
  ArrowUpAZ,
  ChevronRight,
  Copy,
  ExternalLink,
  FileText,
  Folder,
  FolderSearch,
  Maximize2,
  MessageCircle,
  Minimize2,
  MoreHorizontal,
  RotateCcw,
  Search,
  X,
} from 'lucide-react';
import { cn } from '../../lib/utils';
import { prefetchPdfBytes } from '../ask/viewer/pdfDocCache';
import { VIEWER_FIND_EVENT } from '../ask/viewer/PdfViewer';
import { getSourceFileInfo } from '../ask/viewer/sourceAccess';
import { viewerCommand } from '../ask/viewer/viewerKeys';
import { browserStorage } from '../ask/viewer/viewerStores';
import { readNumberPreference, writeNumberPreference } from '../ask/viewer/viewState';
import { buildFileTree, countTree, displayPath, findDir, folderEntries, pathKey, searchFiles, typeBadge, typeFamily } from './fileTree';
import type { DirNode, FileNode, IndexedFileRow, SortKey, TreeEntry, TypeFamily } from './fileTree';
import { copyPath, joinPath, openInDefaultApp, showInFolder } from './fileActions';
import { FileViewer } from './FileViewer';
import { browserTimers, requestPdfMeta, usePdfMeta } from './pdfListMeta';
import { Prefetcher } from './prefetch';
import type { LibrarySource } from './sources';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const ROW_FOCUS = 'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring';

const BUTTON = cn(
  'h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text-secondary',
  'hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro disabled:opacity-50',
  FOCUS_RING,
);

/** Rows rendered at once; a filter narrows larger folders. */
const MAX_ROWS = 500;

/** Key-repeat through the list previews only where the reader pauses. */
const PREVIEW_DELAY_MS = 120;
/** Hovering a PDF row this long reads its bytes ahead. */
const HOVER_PREFETCH_MS = 200;

const LIST_WIDTH_KEY = 'shodh.library.listWidth';
const LIST_WIDTH_DEFAULT = 380;
const LIST_WIDTH_MIN = 240;
const LIST_WIDTH_MAX = 760;
/** The viewer never gets narrower than this when the list is resized. */
const VIEWER_MIN_WIDTH = 360;
const RESIZE_STEP = 24;
const RESIZE_STEP_LARGE = 96;

const SORT_LABELS: Record<SortKey, string> = { name: 'Name', type: 'Type', status: 'Status' };

const FAMILY_CLASS: Record<TypeFamily, string> = {
  pdf: 'text-shodh-error bg-shodh-error/10',
  doc: 'text-shodh-info bg-shodh-info/10',
  sheet: 'text-shodh-success bg-shodh-success/10',
  slides: 'text-shodh-warning bg-shodh-warning/10',
  text: 'text-shodh-text-secondary bg-shodh-raised-2',
  code: 'text-shodh-violet bg-shodh-violet/10',
  image: 'text-shodh-accent-text bg-shodh-accent/10',
  other: 'text-shodh-text-muted bg-shodh-raised-2',
};

type LoadState =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; rows: IndexedFileRow[] };

interface FileBrowserProps {
  source: LibrarySource;
  /** Back to the Library overview. */
  onExit: () => void;
  /** Start a chat about a file. */
  onAskAboutFile: (file: FileNode, source: LibrarySource) => void;
  /** Number of indexed files the index reports for this source. */
  onFileCount?: (count: number) => void;
}

function entryKey(entry: TreeEntry): string {
  return entry.kind === 'dir' ? `d:${entry.dir.join('/')}` : `f:${entry.path}`;
}

/** Indexed PDFs are read ahead; other files open fast enough without. */
function isPrefetchable(file: FileNode): boolean {
  return file.status === 'indexed' && file.extension === 'pdf';
}

function isEditableTarget(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('input, textarea, select, [contenteditable=""], [contenteditable="true"]') !== null;
}

/** Focus the open document's scroller (or the pane) once it is in the DOM. */
function focusViewerIn(pane: HTMLElement | null, attempts = 3): void {
  if (!pane) return;
  const scroller = pane.querySelector<HTMLElement>('[data-viewer-scroller]');
  if (scroller) scroller.focus({ preventScroll: true });
  else if (attempts > 0) requestAnimationFrame(() => focusViewerIn(pane, attempts - 1));
  else pane.focus({ preventScroll: true });
}

/**
 * The files of one Library folder as a navigable list: breadcrumbs, a name
 * filter, sorting, type badges, PDF titles and page counts, and per-file
 * index status, with the selected file previewed beside the list.
 *
 * Selecting a file (click or ↑/↓) previews it; Enter or double-click moves
 * focus into the document. Neighbouring PDFs are read ahead once the
 * current one is on screen, and on hover.
 *
 * Keyboard: ↑/↓/Home/End move, Enter opens and focuses the document,
 * Backspace goes up a folder, F toggles focus mode (document only), Ctrl+F
 * finds in the document, Esc leaves focus mode, then closes the preview (or
 * clears the filter).
 */
export function FileBrowser({ source, onExit, onAskAboutFile, onFileCount }: FileBrowserProps) {
  const [load, setLoad] = useState<LoadState>({ status: 'loading' });
  const [segments, setSegments] = useState<string[]>([]);
  const [filter, setFilter] = useState('');
  const [sortKey, setSortKey] = useState<SortKey>('name');
  const [descending, setDescending] = useState(false);
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const [previewFile, setPreviewFile] = useState<FileNode | null>(null);
  const [focusMode, setFocusMode] = useState(false);
  const [listWidth, setListWidth] = useState(() =>
    readNumberPreference(browserStorage, LIST_WIDTH_KEY, LIST_WIDTH_DEFAULT, LIST_WIDTH_MIN, LIST_WIDTH_MAX),
  );
  const [splitWidth, setSplitWidth] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const splitRef = useRef<HTMLDivElement>(null);
  const paneRef = useRef<HTMLElement>(null);
  const previewTimer = useRef<number | null>(null);
  /** Path being previewed (or about to be), read by timers. */
  const previewPath = useRef<string | null>(null);
  const headingId = useId();
  const listId = useId();

  const prefetcher = useMemo(() => new Prefetcher(browserTimers, 1), []);
  useEffect(() => () => prefetcher.cancelAll(), [prefetcher]);

  const fetchFiles = useCallback(async () => {
    setLoad({ status: 'loading' });
    try {
      const rows = await invoke<IndexedFileRow[]>('get_source_files', { sourceId: source.id });
      const list = Array.isArray(rows) ? rows : [];
      setLoad({ status: 'ready', rows: list });
      onFileCount?.(list.length);
    } catch (error) {
      setLoad({ status: 'error', message: error instanceof Error ? error.message : String(error) });
    }
  }, [source.id, onFileCount]);

  // Reload when the source finishes (re)indexing.
  useEffect(() => {
    if (source.status === 'indexing') return;
    void fetchFiles();
  }, [fetchFiles, source.status]);

  const tree = useMemo<DirNode | null>(
    () => (load.status === 'ready' ? buildFileTree(source.path, load.rows, source.failures ?? []) : null),
    [load, source.path, source.failures],
  );

  // A folder that disappeared after a reload sends the browser back to the root.
  const current = useMemo(() => (tree ? findDir(tree, segments) ?? tree : null), [tree, segments]);
  const totals = useMemo(() => (tree ? countTree(tree) : null), [tree]);

  const filtering = filter.trim().length > 0;
  const entries = useMemo<TreeEntry[]>(() => {
    if (!current) return [];
    return filtering ? searchFiles(current, filter, sortKey, descending) : folderEntries(current, sortKey, descending);
  }, [current, filtering, filter, sortKey, descending]);
  const shown = useMemo(() => (entries.length > MAX_ROWS ? entries.slice(0, MAX_ROWS) : entries), [entries]);

  // Keep a valid active row for roving focus.
  const activeIndex = Math.max(0, shown.findIndex(e => entryKey(e) === activeKey));

  const cancelPendingPreview = useCallback(() => {
    if (previewTimer.current !== null) {
      window.clearTimeout(previewTimer.current);
      previewTimer.current = null;
    }
  }, []);
  useEffect(() => cancelPendingPreview, [cancelPendingPreview]);

  /** Show `file` in the preview pane, now or after the key-repeat delay. */
  const preview = useCallback(
    (file: FileNode, delay: number) => {
      cancelPendingPreview();
      const show = () => {
        previewTimer.current = null;
        if (previewPath.current === file.path) return;
        previewPath.current = file.path;
        // Reads queued for the previous selection's neighbours are stale. A
        // read already running for this file is joined by the viewer's open.
        prefetcher.cancelAll();
        setPreviewFile(file);
      };
      if (delay <= 0) show();
      else previewTimer.current = window.setTimeout(show, delay);
    },
    [cancelPendingPreview, prefetcher],
  );

  const closePreview = useCallback(() => {
    cancelPendingPreview();
    prefetcher.cancelAll();
    previewPath.current = null;
    setPreviewFile(null);
    setFocusMode(false);
  }, [cancelPendingPreview, prefetcher]);

  const focusRow = useCallback(
    (index: number, previewIt: boolean) => {
      const entry = shown[index];
      if (!entry) return;
      setActiveKey(entryKey(entry));
      if (previewIt && entry.kind === 'file') preview(entry, PREVIEW_DELAY_MS);
      requestAnimationFrame(() => {
        listRef.current?.querySelector<HTMLElement>(`[data-row="${CSS.escape(entryKey(entry))}"]`)?.focus();
      });
    },
    [shown, preview],
  );

  const enterDir = useCallback((dir: string[]) => {
    setSegments(dir);
    setFilter('');
    setActiveKey(null);
  }, []);

  const goUp = useCallback(() => {
    if (segments.length === 0) return;
    const leaving = segments;
    setSegments(segments.slice(0, -1));
    setActiveKey(`d:${leaving.join('/')}`);
    requestAnimationFrame(() => {
      listRef.current?.querySelector<HTMLElement>(`[data-row="${CSS.escape(`d:${leaving.join('/')}`)}"]`)?.focus();
    });
  }, [segments]);

  /** Click: folders open, files preview at once. */
  const activate = useCallback(
    (entry: TreeEntry) => {
      if (entry.kind === 'dir') {
        enterDir(entry.dir);
        requestAnimationFrame(() => listRef.current?.querySelector<HTMLElement>('[data-row]')?.focus());
      } else {
        setActiveKey(entryKey(entry));
        preview(entry, 0);
      }
    },
    [enterDir, preview],
  );

  /** Enter / double-click on a file: preview it and move focus into it. */
  const openAndFocus = useCallback(
    (file: FileNode) => {
      setActiveKey(entryKey(file));
      preview(file, 0);
      requestAnimationFrame(() => focusViewerIn(paneRef.current));
    },
    [preview],
  );

  const handleListKeyDown = (e: React.KeyboardEvent<HTMLUListElement>) => {
    if (!(e.target as HTMLElement).matches('[data-row]')) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      focusRow(Math.min(shown.length - 1, activeIndex + 1), true);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      focusRow(Math.max(0, activeIndex - 1), true);
    } else if (e.key === 'Home') {
      e.preventDefault();
      focusRow(0, true);
    } else if (e.key === 'End') {
      e.preventDefault();
      focusRow(shown.length - 1, true);
    } else if (e.key === 'Enter') {
      // Handled here so the button's synthesized click does not also fire.
      e.preventDefault();
      const entry = shown[activeIndex];
      if (entry?.kind === 'file') openAndFocus(entry);
      else if (entry) activate(entry);
    } else if (e.key === 'Backspace' || (e.key === 'ArrowLeft' && e.altKey)) {
      e.preventDefault();
      if (filtering) setFilter('');
      else goUp();
    }
  };

  // Esc leaves focus mode, then closes the preview, then clears the filter.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      if (previewFile && focusMode) {
        e.preventDefault();
        setFocusMode(false);
      } else if (previewFile) {
        e.preventDefault();
        closePreview();
        focusRow(activeIndex, false);
      } else if (filtering && document.activeElement === filterRef.current) {
        e.preventDefault();
        setFilter('');
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [previewFile, focusMode, filtering, focusRow, activeIndex, closePreview]);

  const toggleFocusMode = useCallback(() => {
    const next = !focusMode;
    setFocusMode(next);
    // The list is hidden in focus mode, so focus must not stay in it.
    if (next) requestAnimationFrame(() => focusViewerIn(paneRef.current));
  }, [focusMode]);

  // Leaving focus mode returns to the selected row.
  const wasFocusMode = useRef(false);
  useEffect(() => {
    if (wasFocusMode.current && !focusMode && previewFile) {
      const index = shown.findIndex(e => e.kind === 'file' && e.path === previewFile.path);
      if (index >= 0 && paneRef.current?.contains(document.activeElement) === false) focusRow(index, false);
    }
    wasFocusMode.current = focusMode;
  }, [focusMode, previewFile, shown, focusRow]);

  // F (focus mode) and Ctrl+F (find in the document) anywhere in the browser.
  const handleRootKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.isDefaultPrevented() || !previewFile) return;
    const command = viewerCommand({
      key: e.key,
      ctrlKey: e.ctrlKey,
      metaKey: e.metaKey,
      altKey: e.altKey,
      shiftKey: e.shiftKey,
      editable: isEditableTarget(e.target),
    });
    if (command === 'toggleFocus') {
      e.preventDefault();
      toggleFocusMode();
    } else if (command === 'find') {
      const viewer = paneRef.current?.querySelector('[data-pdf-viewer]');
      if (viewer) {
        e.preventDefault();
        viewer.dispatchEvent(new Event(VIEWER_FIND_EVENT));
      }
    }
  };

  // Read ahead the PDFs before and after the one on screen.
  const neighbourState = useRef({ shown, previewFile });
  neighbourState.current = { shown, previewFile };
  const prefetchFile = useCallback(
    (file: FileNode, delay: number) => {
      prefetcher.schedule(pathKey(file.path), delay, async signal => {
        const info = await getSourceFileInfo(file.path);
        if (signal.cancelled || info.kind !== 'pdf') return;
        await prefetchPdfBytes(file.path, info.sizeBytes, signal);
      });
    },
    [prefetcher],
  );
  const handleFirstPageVisible = useCallback(() => {
    const { shown: rows, previewFile: open } = neighbourState.current;
    if (!open) return;
    const files = rows.filter((e): e is FileNode => e.kind === 'file');
    const index = files.findIndex(f => f.path === open.path);
    if (index < 0) return;
    const next = files.slice(index + 1).find(isPrefetchable);
    const prev = files.slice(0, index).reverse().find(isPrefetchable);
    // Next first: reading moves forward far more often than back.
    if (next) prefetchFile(next, 0);
    if (prev) prefetchFile(prev, 0);
  }, [prefetchFile]);

  const handleRowHover = useCallback(
    (file: FileNode, hovering: boolean) => {
      if (!isPrefetchable(file) || file.path === previewFile?.path) return;
      const key = pathKey(file.path);
      if (hovering) prefetchFile(file, HOVER_PREFETCH_MS);
      else if (prefetcher.stateOf(key) === 'waiting') prefetcher.cancel(key);
    },
    [prefetchFile, prefetcher, previewFile],
  );

  // Titles and page counts for the PDF rows on screen.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const visiblePaths = new Set<string>();
    let frame = 0;
    const observer = new IntersectionObserver(
      observed => {
        for (const entry of observed) {
          const path = (entry.target as HTMLElement).dataset.metaPath;
          if (!path) continue;
          if (entry.isIntersecting) visiblePaths.add(path);
          else visiblePaths.delete(path);
        }
        cancelAnimationFrame(frame);
        frame = requestAnimationFrame(() => requestPdfMeta([...visiblePaths]));
      },
      { root: list, rootMargin: '120px 0px' },
    );
    list.querySelectorAll<HTMLElement>('[data-meta-path]').forEach(el => observer.observe(el));
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      requestPdfMeta([]);
    };
  }, [shown, load.status]);

  // Split between the list and the document.
  useEffect(() => {
    const split = splitRef.current;
    if (!split) return;
    const update = () => setSplitWidth(split.clientWidth);
    update();
    const observer = new ResizeObserver(update);
    observer.observe(split);
    return () => observer.disconnect();
  }, []);
  const maxListWidth = Math.max(LIST_WIDTH_MIN, Math.min(LIST_WIDTH_MAX, (splitWidth || Infinity) - VIEWER_MIN_WIDTH));
  const effectiveListWidth = Math.min(listWidth, maxListWidth);
  const listWidthRef = useRef(effectiveListWidth);
  listWidthRef.current = effectiveListWidth;
  const resizeList = useCallback(
    (width: number) => setListWidth(Math.round(Math.min(maxListWidth, Math.max(LIST_WIDTH_MIN, width)))),
    [maxListWidth],
  );
  const commitListWidth = useCallback(() => writeNumberPreference(browserStorage, LIST_WIDTH_KEY, listWidthRef.current), []);

  const crumbs = [source.name, ...segments];
  const folderPath = joinPath(source.path, segments);
  const showList = !(previewFile && focusMode);

  return (
    <div className="h-full flex flex-col min-h-0" onKeyDown={handleRootKeyDown}>
      {/* Breadcrumbs and totals */}
      <div className={cn('shrink-0 px-6 pt-5 pb-3 flex items-center gap-3 min-w-0', !showList && 'hidden')}>
        <nav aria-label="Breadcrumb" className="min-w-0 flex-1">
          <ol className="flex items-center gap-1 min-w-0 text-[13px]">
            <li className="shrink-0">
              <button type="button" onClick={onExit} className={cn('px-1.5 py-0.5 rounded-md text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised', FOCUS_RING)}>
                Library
              </button>
            </li>
            {crumbs.map((crumb, i) => {
              const last = i === crumbs.length - 1;
              return (
                <li key={`${i}-${crumb}`} className={cn('flex items-center gap-1 min-w-0', last ? 'shrink' : 'shrink-[2]')}>
                  <ChevronRight className="w-3.5 h-3.5 shrink-0 text-shodh-text-faint" aria-hidden="true" />
                  {last ? (
                    <h1 id={headingId} aria-current="page" className="px-1.5 text-[15px] font-semibold text-shodh-text truncate">
                      {crumb}
                    </h1>
                  ) : (
                    <button
                      type="button"
                      onClick={() => enterDir(segments.slice(0, i))}
                      className={cn('px-1.5 py-0.5 rounded-md text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised truncate', FOCUS_RING)}
                    >
                      {crumb}
                    </button>
                  )}
                </li>
              );
            })}
          </ol>
        </nav>
        {totals && (
          <p className="shrink-0 text-[12px] text-shodh-text-faint tabular-nums">
            {`${totals.indexed.toLocaleString()} indexed`}
            {totals.failed > 0 && <span className="text-shodh-error">{` · ${totals.failed.toLocaleString()} failed`}</span>}
          </p>
        )}
      </div>

      {/* Toolbar */}
      <div className={cn('shrink-0 px-6 pb-3 flex items-center gap-2', !showList && 'hidden')}>
        <label className="relative flex-1 max-w-sm">
          <span className="sr-only">Filter files by name</span>
          <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-shodh-text-faint pointer-events-none" aria-hidden="true" />
          <input
            ref={filterRef}
            type="search"
            value={filter}
            onChange={e => { setFilter(e.target.value); setActiveKey(null); }}
            onKeyDown={e => {
              if (e.key === 'ArrowDown' && shown.length > 0) {
                e.preventDefault();
                focusRow(0, true);
              }
            }}
            placeholder={segments.length > 0 ? `Filter in ${segments[segments.length - 1]}` : `Filter in ${source.name}`}
            className={cn(
              'w-full h-8 pl-8 pr-2.5 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text placeholder:text-shodh-text-faint',
              FOCUS_RING,
            )}
          />
        </label>
        <label className="flex items-center gap-1.5 text-[12px] text-shodh-text-muted">
          Sort
          <select
            value={sortKey}
            onChange={e => setSortKey(e.target.value as SortKey)}
            className={cn('h-8 px-2 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text', FOCUS_RING)}
          >
            {(Object.keys(SORT_LABELS) as SortKey[]).map(k => (
              <option key={k} value={k}>{SORT_LABELS[k]}</option>
            ))}
          </select>
        </label>
        <button
          type="button"
          onClick={() => setDescending(d => !d)}
          aria-label={descending ? 'Sorted descending; sort ascending' : 'Sorted ascending; sort descending'}
          title={descending ? 'Descending' : 'Ascending'}
          className={cn(BUTTON, 'w-8 px-0 justify-center')}
        >
          {descending ? <ArrowUpAZ className="w-4 h-4" aria-hidden="true" /> : <ArrowDownAZ className="w-4 h-4" aria-hidden="true" />}
        </button>
        <button type="button" onClick={() => void showInFolder(folderPath)} className={BUTTON} title={folderPath}>
          <FolderSearch className="w-3.5 h-3.5" aria-hidden="true" />
          Show in folder
        </button>
      </div>

      {/* List and previewed file */}
      <div ref={splitRef} className={cn('flex-1 min-h-0 flex', showList && 'border-t border-shodh-border-subtle')}>
        <section
          id={listId}
          aria-labelledby={headingId}
          className={cn('min-h-0 flex flex-col', previewFile ? 'shrink-0' : 'flex-1', !showList && 'hidden')}
          style={previewFile ? { width: effectiveListWidth } : undefined}
        >
          {load.status === 'loading' && <ListSkeleton />}
          {load.status === 'error' && (
            <div role="alert" className="m-6 p-4 rounded-xl border border-shodh-border bg-shodh-surface flex flex-col gap-2 items-start">
              <p className="text-[13px] text-shodh-text">Could not list the files of {source.name}.</p>
              <p className="text-[12px] text-shodh-text-faint break-words">{load.message}</p>
              <button type="button" onClick={() => void fetchFiles()} className={BUTTON}>
                <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
                Try again
              </button>
            </div>
          )}
          {load.status === 'ready' && shown.length === 0 && (
            <EmptyFolder filtering={filtering} filter={filter} indexing={source.status === 'indexing'} rootEmpty={!!totals && totals.files === 0} />
          )}
          {load.status === 'ready' && shown.length > 0 && (
            <>
              <ul
                ref={listRef}
                role="list"
                aria-label={filtering ? `Files matching ${filter.trim()}` : `Contents of ${crumbs[crumbs.length - 1]}`}
                onKeyDown={handleListKeyDown}
                className="flex-1 min-h-0 overflow-y-auto overscroll-contain scrollbar-thin py-1"
              >
                {shown.map((entry, i) => (
                  <EntryRow
                    key={entryKey(entry)}
                    entry={entry}
                    rowKey={entryKey(entry)}
                    tabbable={i === activeIndex}
                    selected={entry.kind === 'file' && previewFile?.path === entry.path}
                    showFolder={filtering}
                    onFocus={() => setActiveKey(entryKey(entry))}
                    onActivate={() => activate(entry)}
                    onOpen={entry.kind === 'file' ? () => openAndFocus(entry) : undefined}
                    onHover={entry.kind === 'file' ? hovering => handleRowHover(entry, hovering) : undefined}
                    folderPath={entry.kind === 'dir' ? joinPath(source.path, entry.dir) : null}
                    filePath={entry.kind === 'file' ? displayPath(source.path, entry) : null}
                    onAsk={entry.kind === 'file' ? () => onAskAboutFile(entry, source) : undefined}
                  />
                ))}
              </ul>
              {entries.length > shown.length && (
                <p className="shrink-0 px-6 py-2 text-[12px] text-shodh-text-faint border-t border-shodh-border-subtle">
                  {`Showing ${shown.length.toLocaleString()} of ${entries.length.toLocaleString()}. Type in the filter to narrow the list.`}
                </p>
              )}
            </>
          )}
        </section>

        {previewFile && showList && (
          <ResizeHandle
            width={effectiveListWidth}
            min={LIST_WIDTH_MIN}
            max={maxListWidth}
            controls={listId}
            onResize={resizeList}
            onCommit={commitListWidth}
            onReset={() => {
              resizeList(LIST_WIDTH_DEFAULT);
              writeNumberPreference(browserStorage, LIST_WIDTH_KEY, Math.min(LIST_WIDTH_DEFAULT, maxListWidth));
            }}
          />
        )}

        {previewFile && (
          <OpenFilePane
            paneRef={paneRef}
            file={previewFile}
            shownPath={displayPath(source.path, previewFile)}
            focusMode={focusMode}
            onToggleFocus={toggleFocusMode}
            onClose={() => { closePreview(); focusRow(activeIndex, false); }}
            onAsk={() => onAskAboutFile(previewFile, source)}
            onFirstPageVisible={handleFirstPageVisible}
          />
        )}
      </div>
    </div>
  );
}

interface ResizeHandleProps {
  width: number;
  min: number;
  max: number;
  /** id of the panel being resized. */
  controls: string;
  onResize: (width: number) => void;
  /** The drag or key press ended: remember the width. */
  onCommit: () => void;
  onReset: () => void;
}

/**
 * Drag handle between the file list and the document: pointer drag, arrow
 * keys (Shift for larger steps), Home/End for the limits, double-click to
 * reset.
 */
function ResizeHandle({ width, min, max, controls, onResize, onCommit, onReset }: ResizeHandleProps) {
  const drag = useRef<{ startX: number; startWidth: number } | null>(null);
  const [dragging, setDragging] = useState(false);

  const endDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    drag.current = null;
    setDragging(false);
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    onCommit();
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize file list"
      aria-controls={controls}
      aria-valuenow={Math.round(width)}
      aria-valuemin={min}
      aria-valuemax={Math.round(max)}
      tabIndex={0}
      title="Drag to resize; double-click to reset"
      onPointerDown={e => {
        if (e.button !== 0) return;
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        drag.current = { startX: e.clientX, startWidth: width };
        setDragging(true);
      }}
      onPointerMove={e => {
        if (drag.current) onResize(drag.current.startWidth + e.clientX - drag.current.startX);
      }}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onDoubleClick={onReset}
      onKeyDown={e => {
        const step = e.shiftKey ? RESIZE_STEP_LARGE : RESIZE_STEP;
        let next: number | null = null;
        if (e.key === 'ArrowLeft') next = width - step;
        else if (e.key === 'ArrowRight') next = width + step;
        else if (e.key === 'Home') next = min;
        else if (e.key === 'End') next = max;
        if (next === null) return;
        e.preventDefault();
        onResize(next);
        requestAnimationFrame(onCommit);
      }}
      className="group relative z-10 w-px shrink-0 bg-shodh-border-subtle cursor-col-resize touch-none focus-visible:outline-none"
    >
      {/* Wider hit area than the 1px line. */}
      <span className="absolute inset-y-0 -left-1 -right-1" aria-hidden="true" />
      <span
        className={cn(
          'absolute inset-y-0 -left-px w-[3px] transition-colors duration-micro',
          dragging ? 'bg-shodh-accent' : 'bg-transparent group-hover:bg-shodh-accent/50 group-focus-visible:bg-ring',
        )}
        aria-hidden="true"
      />
    </div>
  );
}

function StatusChip({ file }: { file: FileNode }) {
  if (file.status === 'failed') {
    return (
      <span className="shrink-0 inline-flex items-center gap-1 text-[11px] px-1.5 h-5 rounded-full bg-shodh-error/10 text-shodh-error" title={file.reason || undefined}>
        <AlertTriangle className="w-3 h-3" aria-hidden="true" />
        Failed
      </span>
    );
  }
  return (
    <span className="shrink-0 inline-flex items-center gap-1 text-[11px] px-1.5 h-5 rounded-full bg-shodh-success-soft text-shodh-success">
      <span className="w-1.5 h-1.5 rounded-full bg-shodh-success" aria-hidden="true" />
      Indexed
    </span>
  );
}

function TypeBadge({ extension }: { extension: string }) {
  return (
    <span
      className={cn('shrink-0 w-11 h-5 inline-flex items-center justify-center rounded text-[10px] font-bold tracking-wide', FAMILY_CLASS[typeFamily(extension)])}
      aria-hidden="true"
    >
      {typeBadge(extension)}
    </span>
  );
}

interface EntryRowProps {
  entry: TreeEntry;
  rowKey: string;
  tabbable: boolean;
  selected: boolean;
  showFolder: boolean;
  folderPath: string | null;
  /** The file's path as shown, copied and opened. */
  filePath: string | null;
  onFocus: () => void;
  /** Click: open a folder, preview a file. */
  onActivate: () => void;
  /** Double-click on a file: preview it and focus the document. */
  onOpen?: () => void;
  onHover?: (hovering: boolean) => void;
  onAsk?: () => void;
}

function EntryRow({ entry, rowKey, tabbable, selected, showFolder, folderPath, filePath, onFocus, onActivate, onOpen, onHover, onAsk }: EntryRowProps) {
  const isDir = entry.kind === 'dir';
  const isPdf = entry.kind === 'file' && entry.extension === 'pdf';
  const meta = usePdfMeta(isPdf ? entry.path : null);
  const title = meta?.title ?? null;
  const counts = isDir ? countTree(entry) : null;
  const pages = meta ? `${meta.pages.toLocaleString()} ${meta.pages === 1 ? 'page' : 'pages'}` : null;
  const label = isDir
    ? `Folder ${entry.name}, ${counts!.files} file${counts!.files === 1 ? '' : 's'}`
    : [
        title ? `${title}, file ${entry.name}` : entry.name,
        entry.extension ? `${entry.extension.toUpperCase()} file` : null,
        pages,
        entry.status === 'failed' ? `failed to index${entry.reason ? `: ${entry.reason}` : ''}` : 'indexed',
      ]
        .filter(Boolean)
        .join(', ');
  const secondary = !isDir ? [title ? entry.name : null, showFolder && entry.dir.length > 0 ? entry.dir.join(' › ') : null].filter(Boolean).join(' · ') : '';

  return (
    <li className="relative group px-2">
      <button
        type="button"
        data-row={rowKey}
        data-meta-path={isPdf ? entry.path : undefined}
        tabIndex={tabbable ? 0 : -1}
        onFocus={onFocus}
        onClick={onActivate}
        onDoubleClick={onOpen}
        onPointerEnter={onHover ? () => onHover(true) : undefined}
        onPointerLeave={onHover ? () => onHover(false) : undefined}
        aria-label={label}
        aria-current={selected ? 'true' : undefined}
        title={title ? `${title}\n${entry.name}` : undefined}
        className={cn(
          'w-full h-10 pl-3 pr-10 flex items-center gap-3 rounded-lg text-left transition-colors duration-micro',
          selected ? 'bg-shodh-raised-2' : 'hover:bg-shodh-raised',
          ROW_FOCUS,
        )}
      >
        {isDir ? (
          <span className="shrink-0 w-11 flex justify-center" aria-hidden="true">
            <Folder className="w-4 h-4 text-shodh-text-muted" />
          </span>
        ) : (
          <TypeBadge extension={entry.extension} />
        )}
        <span className="flex-1 min-w-0 flex flex-col">
          <span className="text-[13px] text-shodh-text truncate">{title ?? entry.name}</span>
          {secondary && <span className="text-[11px] text-shodh-text-faint truncate">{secondary}</span>}
        </span>
        {isDir ? (
          <span className="shrink-0 text-[11.5px] text-shodh-text-faint tabular-nums">
            {counts!.files.toLocaleString()} file{counts!.files === 1 ? '' : 's'}
            {counts!.failed > 0 && <span className="text-shodh-error">{` · ${counts!.failed} failed`}</span>}
          </span>
        ) : (
          <>
            {meta && (
              <span className="shrink-0 text-[11.5px] text-shodh-text-faint tabular-nums" aria-hidden="true">
                {`${meta.pages.toLocaleString()} p`}
              </span>
            )}
            <StatusChip file={entry} />
          </>
        )}
      </button>
      <RowMenu
        name={entry.name}
        tabbable={tabbable}
        items={
          isDir
            ? [
                { label: 'Open', icon: Folder, run: onActivate },
                { label: 'Show in folder', icon: FolderSearch, run: () => void showInFolder(folderPath!) },
                { label: 'Copy path', icon: Copy, run: () => void copyPath(folderPath!) },
              ]
            : [
                { label: 'Open in Shodh', icon: FileText, run: onOpen ?? onActivate },
                { label: 'Open in default app', icon: ExternalLink, run: () => void openInDefaultApp(filePath!) },
                { label: 'Show in folder', icon: FolderSearch, run: () => void showInFolder(filePath!) },
                { label: 'Copy path', icon: Copy, run: () => void copyPath(filePath!) },
                ...(onAsk ? [{ label: 'Ask about this file', icon: MessageCircle, run: onAsk }] : []),
              ]
        }
      />
    </li>
  );
}

interface MenuItemSpec {
  label: string;
  icon: React.ElementType;
  run: () => void;
}

function RowMenu({ name, items, tabbable }: { name: string; items: MenuItemSpec[]; tabbable: boolean }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();

  useEffect(() => {
    if (!open) return;
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    const onPointer = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointer);
    return () => document.removeEventListener('pointerdown', onPointer);
  }, [open]);

  const close = (restore: boolean) => {
    setOpen(false);
    if (restore) buttonRef.current?.focus();
  };

  const onMenuKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const buttons = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? []);
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close(true);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      e.stopPropagation();
      buttons[(index + 1) % buttons.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      e.stopPropagation();
      buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
    } else if (e.key === 'Tab') {
      close(false);
    }
  };

  return (
    <div ref={rootRef} className="absolute right-3.5 top-1.5">
      <button
        ref={buttonRef}
        type="button"
        tabIndex={tabbable ? 0 : -1}
        onClick={() => setOpen(o => !o)}
        aria-label={`Actions for ${name}`}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        className={cn(
          'w-7 h-7 rounded-md inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised-2 hover:text-shodh-text transition-opacity duration-micro',
          open ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 group-focus-within:opacity-100',
          ROW_FOCUS,
        )}
      >
        <MoreHorizontal className="w-4 h-4" aria-hidden="true" />
      </button>
      {open && (
        <div
          id={menuId}
          ref={menuRef}
          role="menu"
          aria-label={`Actions for ${name}`}
          onKeyDown={onMenuKeyDown}
          className="shell-pop absolute right-0 top-8 z-50 min-w-[196px] py-1 rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg"
        >
          {items.map(item => {
            const Icon = item.icon;
            return (
              <button
                key={item.label}
                type="button"
                role="menuitem"
                onClick={() => { close(false); item.run(); }}
                className="w-full flex items-center gap-2 px-3 py-1.5 text-[12.5px] text-left text-shodh-text-secondary hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none"
              >
                <Icon className="w-3.5 h-3.5" aria-hidden="true" />
                {item.label}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

interface OpenFilePaneProps {
  paneRef: React.RefObject<HTMLElement | null>;
  file: FileNode;
  shownPath: string;
  focusMode: boolean;
  onToggleFocus: () => void;
  onClose: () => void;
  onAsk: () => void;
  onFirstPageVisible: () => void;
}

/**
 * The previewed file. Mounted once while files are previewed (its entrance
 * animation plays when it first appears, not on every selection); only the
 * header text and the viewer swap when the selection moves.
 */
function OpenFilePane({ paneRef, file, shownPath, focusMode, onToggleFocus, onClose, onAsk, onFirstPageVisible }: OpenFilePaneProps) {
  const titleId = useId();
  const meta = usePdfMeta(file.extension === 'pdf' ? file.path : null);
  const title = meta?.title ?? null;
  return (
    <section
      ref={paneRef}
      tabIndex={-1}
      aria-labelledby={titleId}
      className="shell-view-enter flex-1 min-w-0 min-h-0 flex flex-col bg-shodh-surface focus-visible:outline-none"
    >
      <header className="shrink-0 px-4 py-3 border-b border-shodh-border-subtle flex flex-col gap-2">
        <div className="flex items-start gap-2 min-w-0">
          <TypeBadge extension={file.extension} />
          <div className="flex-1 min-w-0">
            <h2 id={titleId} className="text-[14px] font-semibold text-shodh-text truncate" title={title ?? file.name}>{title ?? file.name}</h2>
            <p className="text-[11.5px] text-shodh-text-faint truncate" title={shownPath}>
              {title ? `${file.name}${meta ? ` · ${meta.pages.toLocaleString()} ${meta.pages === 1 ? 'page' : 'pages'}` : ''}` : shownPath}
            </p>
          </div>
          <button
            type="button"
            onClick={onToggleFocus}
            aria-pressed={focusMode}
            aria-label={focusMode ? 'Show the file list' : 'Hide the file list'}
            title={focusMode ? 'Show the file list (F)' : 'Focus on the document (F)'}
            className={cn('w-8 h-8 shrink-0 rounded-lg inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', focusMode && 'bg-shodh-raised text-shodh-text', FOCUS_RING)}
          >
            {focusMode ? <Minimize2 className="w-4 h-4" aria-hidden="true" /> : <Maximize2 className="w-4 h-4" aria-hidden="true" />}
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close file"
            title="Close (Esc)"
            className={cn('w-8 h-8 shrink-0 rounded-lg inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
          >
            <X className="w-4 h-4" aria-hidden="true" />
          </button>
        </div>
        <div className="flex flex-wrap items-center gap-1.5" role="toolbar" aria-label="File actions">
          <button type="button" onClick={onAsk} className={cn(BUTTON, 'bg-shodh-accent text-shodh-on-accent border-transparent hover:bg-shodh-accent-hover hover:text-shodh-on-accent')}>
            <MessageCircle className="w-3.5 h-3.5" aria-hidden="true" />
            Ask about this file
          </button>
          <button type="button" onClick={() => void openInDefaultApp(shownPath)} className={BUTTON}>
            <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
            Open in default app
          </button>
          <button type="button" onClick={() => void showInFolder(shownPath)} className={BUTTON}>
            <FolderSearch className="w-3.5 h-3.5" aria-hidden="true" />
            Show in folder
          </button>
          <button type="button" onClick={() => void copyPath(shownPath)} className={BUTTON}>
            <Copy className="w-3.5 h-3.5" aria-hidden="true" />
            Copy path
          </button>
        </div>
        {file.status === 'failed' && (
          <p role="note" className="text-[12px] text-shodh-error flex items-start gap-1.5">
            <AlertTriangle className="w-3.5 h-3.5 mt-px shrink-0" aria-hidden="true" />
            <span>{file.reason ? `Not indexed: ${file.reason}` : 'This file could not be indexed.'}</span>
          </p>
        )}
      </header>
      <div className="flex-1 min-h-0">
        <FileViewer path={file.path} onFirstPageVisible={onFirstPageVisible} />
      </div>
    </section>
  );
}

function ListSkeleton() {
  return (
    <div className="px-4 py-2 flex flex-col gap-1.5" aria-busy="true" aria-label="Loading files">
      {Array.from({ length: 9 }, (_, i) => (
        <div key={i} className="h-10 flex items-center gap-3 px-3">
          <span className="shell-skeleton w-11 h-5 rounded" />
          <span className="shell-skeleton h-3.5 rounded" style={{ width: `${40 + ((i * 37) % 45)}%` }} />
        </div>
      ))}
    </div>
  );
}

function EmptyFolder({ filtering, filter, indexing, rootEmpty }: { filtering: boolean; filter: string; indexing: boolean; rootEmpty: boolean }) {
  let message: string;
  if (filtering) message = `No file names here contain “${filter.trim()}”.`;
  else if (indexing) message = 'Indexing is running. Files appear here once it finishes.';
  else if (rootEmpty) message = 'Nothing from this folder is in the index yet. Index it again from the Library to add its files.';
  else message = 'This folder is empty.';
  return (
    <div className="flex-1 flex items-center justify-center p-8">
      <p role="status" className="max-w-sm text-center text-[13px] text-shodh-text-muted">{message}</p>
    </div>
  );
}
