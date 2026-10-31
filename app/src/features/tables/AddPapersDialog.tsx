import React, { useEffect, useId, useMemo, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { invoke } from '@tauri-apps/api/core';
import { FileText, Folder, Loader2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import type { IndexedFileRow } from '../library/fileTree';
import type { WorkspaceDetail } from '../workspaces/types';
import { tableError, tablesApi } from './api';
import { reasonText } from './format';
import { addBatches, fileNameOf, paperKey } from './model';
import type { RowView } from './types';
import {
  CLOSE_BUTTON,
  DIALOG_BODY,
  DIALOG_CONTENT,
  DIALOG_HEADER,
  DIALOG_OVERLAY,
  FOCUS_RING,
  INPUT,
  OUTLINE_BUTTON,
  PRIMARY_BUTTON,
  QUIET_BUTTON,
  SECTION_TITLE,
} from './ui';

const CASE_INSENSITIVE_PATHS = typeof navigator !== 'undefined' && /Win/i.test(navigator.platform);

interface Candidate {
  path: string;
  label: string;
}

type FolderLoad = { status: 'idle' } | { status: 'loading' } | { status: 'ready'; files: Candidate[] } | { status: 'error'; message: string };

/**
 * Add papers: the workspace's files and library papers, and the indexed files of its
 * folders (listed when asked), each once. Papers the table has already are shown and
 * cannot be picked again.
 */
export function AddPapersDialog({
  open,
  onOpenChange,
  tableId,
  workspace,
  rows,
  onAdded,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  tableId: string;
  workspace: WorkspaceDetail;
  rows: readonly RowView[];
  onAdded: () => void;
}) {
  const ids = { desc: useId(), filter: useId() };
  const [picked, setPicked] = useState<ReadonlySet<string>>(() => new Set());
  const [folders, setFolders] = useState<Record<string, FolderLoad>>({});
  const [filter, setFilter] = useState('');
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setPicked(new Set());
    setFolders({});
    setFilter('');
    setError(null);
  }, [open]);

  const inTable = useMemo(() => new Set(rows.map(r => paperKey(r.documentId, CASE_INSENSITIVE_PATHS))), [rows]);
  const has = (path: string) => inTable.has(paperKey(path, CASE_INSENSITIVE_PATHS));

  const direct = useMemo<Candidate[]>(() => {
    const seen = new Set<string>();
    const out: Candidate[] = [];
    for (const source of workspace.sources) {
      if ((source.kind !== 'file' && source.kind !== 'paper') || !source.path) continue;
      const key = paperKey(source.path, CASE_INSENSITIVE_PATHS);
      if (seen.has(key)) continue;
      seen.add(key);
      out.push({ path: source.path, label: source.label || fileNameOf(source.path) });
    }
    return out;
  }, [workspace.sources]);

  const folderSources = useMemo(() => workspace.sources.filter(s => s.kind === 'folder'), [workspace.sources]);

  const loadFolder = async (ref: string) => {
    setFolders(f => ({ ...f, [ref]: { status: 'loading' } }));
    try {
      const list = await invoke<IndexedFileRow[]>('get_source_files', { sourceId: ref });
      const files = (Array.isArray(list) ? list : [])
        .filter(f => typeof f.file_path === 'string' && f.file_path.length > 0)
        .map(f => ({ path: f.file_path, label: f.name || fileNameOf(f.file_path) }))
        .sort((a, b) => a.label.localeCompare(b.label));
      setFolders(f => ({ ...f, [ref]: { status: 'ready', files } }));
    } catch (err) {
      setFolders(f => ({ ...f, [ref]: { status: 'error', message: err instanceof Error ? err.message : String(err) } }));
    }
  };

  const needle = filter.trim().toLowerCase();
  const shown = (c: Candidate) => !needle || c.label.toLowerCase().includes(needle) || c.path.toLowerCase().includes(needle);

  const toggle = (path: string, on: boolean) =>
    setPicked(prev => {
      const next = new Set(prev);
      if (on) next.add(path);
      else next.delete(path);
      return next;
    });

  const add = async () => {
    const paths = Array.from(picked);
    if (paths.length === 0) return;
    setAdding(true);
    setError(null);
    let added = 0;
    try {
      for (const batch of addBatches(paths)) added += (await tablesApi.addRows(tableId, batch)).length;
      const skipped = paths.length - added;
      notify.success(`Added ${added} ${added === 1 ? 'paper' : 'papers'}`, {
        description: skipped > 0 ? `${skipped} ${skipped === 1 ? 'was' : 'were'} in the table already.` : 'Fill the table to answer its columns for them.',
      });
      onAdded();
      onOpenChange(false);
    } catch (err) {
      const failure = tableError(err);
      setError(`${added > 0 ? `${added} added, then: ` : ''}${failure.message || reasonText(failure.code)}`);
      if (added > 0) onAdded();
    } finally {
      setAdding(false);
    }
  };

  const item = (c: Candidate) => {
    const already = has(c.path);
    return (
      <li key={c.path}>
        <label className={cn('flex items-center gap-2.5 py-1.5 px-1 rounded-md', already ? 'opacity-60' : 'hover:bg-shodh-raised/60 cursor-pointer')}>
          <input
            type="checkbox"
            checked={already || picked.has(c.path)}
            disabled={already || adding}
            onChange={e => toggle(c.path, e.target.checked)}
            className={cn('w-3.5 h-3.5 accent-[var(--c-accent)]', FOCUS_RING)}
          />
          <FileText className="w-3.5 h-3.5 text-shodh-text-faint shrink-0" aria-hidden="true" />
          <span className="flex-1 min-w-0 truncate text-small text-shodh-text" title={c.path}>{c.label}</span>
          {already && <span className="text-caption text-shodh-text-muted shrink-0">In the table</span>}
        </label>
      </li>
    );
  };

  const visibleDirect = direct.filter(shown);

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className={DIALOG_OVERLAY} />
        <Dialog.Content aria-describedby={ids.desc} className={DIALOG_CONTENT}>
          <header className={DIALOG_HEADER}>
            <div className="flex-1 min-w-0">
              <Dialog.Title className="text-body-lg font-semibold">Add papers</Dialog.Title>
              <Dialog.Description id={ids.desc} className="text-small text-shodh-text-muted">
                Each paper becomes a row. Papers must be indexed in the Library for the fill to read them.
              </Dialog.Description>
            </div>
            <Dialog.Close aria-label="Close" className={cn(CLOSE_BUTTON, FOCUS_RING)}>
              <X className="w-4 h-4" aria-hidden="true" />
            </Dialog.Close>
          </header>
          <div className={DIALOG_BODY}>
            <div className="flex flex-col gap-1">
              <label htmlFor={ids.filter} className="sr-only">Filter papers</label>
              <input id={ids.filter} className={INPUT} value={filter} onChange={e => setFilter(e.target.value)} placeholder="Filter by name" />
            </div>
            {direct.length === 0 && folderSources.length === 0 && (
              <p className="m-0 text-small text-shodh-text-muted">
                This workspace has no files, papers or folders yet. Add them on its Sources tab first.
              </p>
            )}
            {direct.length > 0 && (
              <section aria-label="Files and papers of this workspace" className="flex flex-col gap-1">
                <h3 className={SECTION_TITLE}>Files and papers</h3>
                {visibleDirect.length === 0 ? (
                  <p className="m-0 text-small text-shodh-text-muted">None match the filter.</p>
                ) : (
                  <ul className="m-0 p-0 list-none">{visibleDirect.map(item)}</ul>
                )}
              </section>
            )}
            {folderSources.map(folder => {
              const load: FolderLoad = folders[folder.ref] ?? { status: 'idle' };
              const files = load.status === 'ready' ? load.files : [];
              const pickable = files.filter(f => shown(f) && !has(f.path));
              return (
                <section key={folder.ref} aria-label={`Folder ${folder.label}`} className="flex flex-col gap-1">
                  <div className="flex items-center gap-2">
                    <Folder className="w-3.5 h-3.5 text-shodh-text-faint" aria-hidden="true" />
                    <h3 className={cn(SECTION_TITLE, 'flex-1 min-w-0 truncate')} title={folder.path ?? undefined}>{folder.label}</h3>
                    {load.status === 'idle' || load.status === 'error' ? (
                      <button type="button" className={OUTLINE_BUTTON} onClick={() => void loadFolder(folder.ref)}>
                        {load.status === 'error' ? 'Try again' : 'Show its files'}
                      </button>
                    ) : load.status === 'ready' && pickable.length > 0 ? (
                      <button
                        type="button"
                        className={QUIET_BUTTON}
                        disabled={adding}
                        onClick={() => {
                          const all = pickable.every(f => picked.has(f.path));
                          setPicked(prev => {
                            const next = new Set(prev);
                            for (const f of pickable) {
                              if (all) next.delete(f.path);
                              else next.add(f.path);
                            }
                            return next;
                          });
                        }}
                      >
                        {pickable.every(f => picked.has(f.path)) ? 'Unselect all' : `Select all ${pickable.length}`}
                      </button>
                    ) : null}
                  </div>
                  {load.status === 'loading' && (
                    <p role="status" className="m-0 flex items-center gap-2 text-small text-shodh-text-muted">
                      <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> Listing its indexed files…
                    </p>
                  )}
                  {load.status === 'error' && <p role="alert" className="m-0 text-small text-shodh-error">Its files could not be listed: {load.message}</p>}
                  {load.status === 'ready' && (files.length === 0 ? (
                    <p className="m-0 text-small text-shodh-text-muted">No indexed files in this folder yet.</p>
                  ) : (
                    <ul className="m-0 p-0 list-none max-h-[240px] overflow-y-auto scrollbar-thin">{files.filter(shown).map(item)}</ul>
                  ))}
                </section>
              );
            })}
            {error && <p role="alert" className="m-0 text-small text-shodh-error">{error}</p>}
            <div className="flex items-center justify-end gap-2 pt-1">
              <Dialog.Close className={QUIET_BUTTON} type="button">Cancel</Dialog.Close>
              <button type="button" className={PRIMARY_BUTTON} disabled={picked.size === 0 || adding} onClick={() => void add()}>
                {adding && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
                {picked.size === 0 ? 'Add papers' : `Add ${picked.size} ${picked.size === 1 ? 'paper' : 'papers'}`}
              </button>
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
