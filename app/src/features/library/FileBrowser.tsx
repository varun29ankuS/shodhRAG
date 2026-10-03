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
  MessageCircle,
  MoreHorizontal,
  RotateCcw,
  Search,
  X,
} from 'lucide-react';
import { cn } from '../../lib/utils';
import { buildFileTree, countTree, displayPath, findDir, folderEntries, searchFiles, typeBadge, typeFamily } from './fileTree';
import type { DirNode, FileNode, IndexedFileRow, SortKey, TreeEntry, TypeFamily } from './fileTree';
import { copyPath, joinPath, openInDefaultApp, showInFolder } from './fileActions';
import { FileViewer } from './FileViewer';
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

/**
 * The files of one Library folder as a navigable tree: breadcrumbs, a name
 * filter, sorting, type badges and per-file index status, with the file open
 * beside the list in the app's document viewer.
 *
 * Keyboard: ↑/↓/Home/End move, Enter opens, Backspace goes up a folder,
 * Esc closes the open file (or clears the filter).
 */
export function FileBrowser({ source, onExit, onAskAboutFile, onFileCount }: FileBrowserProps) {
  const [load, setLoad] = useState<LoadState>({ status: 'loading' });
  const [segments, setSegments] = useState<string[]>([]);
  const [filter, setFilter] = useState('');
  const [sortKey, setSortKey] = useState<SortKey>('name');
  const [descending, setDescending] = useState(false);
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const [openFile, setOpenFile] = useState<FileNode | null>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const headingId = useId();

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
  const shown = entries.length > MAX_ROWS ? entries.slice(0, MAX_ROWS) : entries;

  // Keep a valid active row for roving focus.
  const activeIndex = Math.max(0, shown.findIndex(e => entryKey(e) === activeKey));

  const focusRow = useCallback((index: number) => {
    const entry = shown[index];
    if (!entry) return;
    setActiveKey(entryKey(entry));
    requestAnimationFrame(() => {
      listRef.current?.querySelector<HTMLElement>(`[data-row="${CSS.escape(entryKey(entry))}"]`)?.focus();
    });
  }, [shown]);

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

  const activate = useCallback((entry: TreeEntry) => {
    if (entry.kind === 'dir') {
      enterDir(entry.dir);
      requestAnimationFrame(() => listRef.current?.querySelector<HTMLElement>('[data-row]')?.focus());
    } else {
      setActiveKey(entryKey(entry));
      setOpenFile(entry);
    }
  }, [enterDir]);

  const handleListKeyDown = (e: React.KeyboardEvent<HTMLUListElement>) => {
    if (!(e.target as HTMLElement).matches('[data-row]')) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      focusRow(Math.min(shown.length - 1, activeIndex + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      focusRow(Math.max(0, activeIndex - 1));
    } else if (e.key === 'Home') {
      e.preventDefault();
      focusRow(0);
    } else if (e.key === 'End') {
      e.preventDefault();
      focusRow(shown.length - 1);
    } else if (e.key === 'Backspace' || (e.key === 'ArrowLeft' && e.altKey)) {
      e.preventDefault();
      if (filtering) setFilter('');
      else goUp();
    }
  };

  // Esc closes the open file first, then clears the filter.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      if (openFile) {
        e.preventDefault();
        setOpenFile(null);
        focusRow(activeIndex);
      } else if (filtering && document.activeElement === filterRef.current) {
        e.preventDefault();
        setFilter('');
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [openFile, filtering, focusRow, activeIndex]);

  const crumbs = [source.name, ...segments];
  const folderPath = joinPath(source.path, segments);

  return (
    <div className="h-full flex flex-col min-h-0">
      {/* Breadcrumbs and totals */}
      <div className="shrink-0 px-6 pt-5 pb-3 flex items-center gap-3 min-w-0">
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
      <div className="shrink-0 px-6 pb-3 flex items-center gap-2">
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
                focusRow(0);
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

      {/* List and open file */}
      <div className="flex-1 min-h-0 flex border-t border-shodh-border-subtle">
        <section
          aria-labelledby={headingId}
          className={cn('min-h-0 flex flex-col', openFile ? 'w-[min(42%,460px)] shrink-0 border-r border-shodh-border-subtle' : 'flex-1')}
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
                className="flex-1 min-h-0 overflow-y-auto scrollbar-thin py-1"
              >
                {shown.map((entry, i) => (
                  <EntryRow
                    key={entryKey(entry)}
                    entry={entry}
                    rowKey={entryKey(entry)}
                    tabbable={i === activeIndex}
                    selected={entry.kind === 'file' && openFile?.path === entry.path}
                    showFolder={filtering}
                    onFocus={() => setActiveKey(entryKey(entry))}
                    onActivate={() => activate(entry)}
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

        {openFile && (
          <OpenFilePane
            key={openFile.path}
            file={openFile}
            shownPath={displayPath(source.path, openFile)}
            onClose={() => { setOpenFile(null); focusRow(activeIndex); }}
            onAsk={() => onAskAboutFile(openFile, source)}
          />
        )}
      </div>
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
  onActivate: () => void;
  onAsk?: () => void;
}

function EntryRow({ entry, rowKey, tabbable, selected, showFolder, folderPath, filePath, onFocus, onActivate, onAsk }: EntryRowProps) {
  const isDir = entry.kind === 'dir';
  const counts = isDir ? countTree(entry) : null;
  const label = isDir
    ? `Folder ${entry.name}, ${counts!.files} file${counts!.files === 1 ? '' : 's'}`
    : `${entry.name}, ${entry.extension ? `${entry.extension.toUpperCase()} file, ` : ''}${entry.status === 'failed' ? `failed to index${entry.reason ? `: ${entry.reason}` : ''}` : 'indexed'}`;

  return (
    <li className="relative group px-2">
      <button
        type="button"
        data-row={rowKey}
        tabIndex={tabbable ? 0 : -1}
        onFocus={onFocus}
        onClick={onActivate}
        aria-label={label}
        aria-current={selected ? 'true' : undefined}
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
          <span className="text-[13px] text-shodh-text truncate">{entry.name}</span>
          {!isDir && showFolder && entry.dir.length > 0 && (
            <span className="text-[11px] text-shodh-text-faint truncate">{entry.dir.join(' › ')}</span>
          )}
        </span>
        {isDir ? (
          <span className="shrink-0 text-[11.5px] text-shodh-text-faint tabular-nums">
            {counts!.files.toLocaleString()} file{counts!.files === 1 ? '' : 's'}
            {counts!.failed > 0 && <span className="text-shodh-error">{` · ${counts!.failed} failed`}</span>}
          </span>
        ) : (
          <StatusChip file={entry} />
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
                { label: 'Open in Shodh', icon: FileText, run: onActivate },
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

function OpenFilePane({ file, shownPath, onClose, onAsk }: { file: FileNode; shownPath: string; onClose: () => void; onAsk: () => void }) {
  const titleId = useId();
  return (
    <section aria-labelledby={titleId} className="shell-view-enter flex-1 min-w-0 min-h-0 flex flex-col bg-shodh-surface">
      <header className="shrink-0 px-4 py-3 border-b border-shodh-border-subtle flex flex-col gap-2">
        <div className="flex items-start gap-2 min-w-0">
          <TypeBadge extension={file.extension} />
          <div className="flex-1 min-w-0">
            <h2 id={titleId} className="text-[14px] font-semibold text-shodh-text truncate" title={file.name}>{file.name}</h2>
            <p className="text-[11.5px] text-shodh-text-faint truncate" title={shownPath}>{shownPath}</p>
          </div>
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
        <FileViewer path={file.path} />
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
