import React, { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { AlertTriangle, CheckCircle2, FileText, Folder, Loader2, Network, Plus, RefreshCw, Scissors, Search, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { removeWithUndo } from '../../lib/undoToast';
import { researchApi, toResearchError } from '../research/api';
import { graphApi } from '../research/graphApi';
import type { ViewNode } from '../research/graphTypes';
import type { Snippet } from '../research/types';
import { workspaceError, workspacesApi } from './api';
import { SOURCE_KIND_LABELS, sourceStateLabel } from './model';
import type { NewSource, SourceHealth, SourceKind, SourceState, WorkspaceDetail, WorkspaceSource } from './types';
import { FOCUS_RING, INPUT, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON, SECTION_TITLE } from './ui';

/** A Library folder that can be added. */
export interface LibraryFolder {
  id: string;
  name: string;
  path: string;
}

const KIND_ORDER: readonly SourceKind[] = ['folder', 'file', 'snippet', 'paper'];

const KIND_ICONS: Record<SourceKind, React.ElementType> = {
  folder: Folder,
  file: FileText,
  snippet: Scissors,
  paper: Network,
};

function fileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

function StateBadge({ state }: { state: SourceState | null }) {
  if (state === null) return <span className="text-[11.5px] text-shodh-text-faint">Checking…</span>;
  const ok = state === 'indexed';
  const neutral = state === 'reference' || state === 'unknown';
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 text-[11.5px]',
        ok ? 'text-shodh-success' : neutral ? 'text-shodh-text-muted' : 'text-shodh-warning',
      )}
    >
      {ok ? <CheckCircle2 className="w-3.5 h-3.5" aria-hidden="true" /> : !neutral && <AlertTriangle className="w-3.5 h-3.5" aria-hidden="true" />}
      {sourceStateLabel(state)}
    </span>
  );
}

type AddMode = 'folders' | 'files' | 'snippets' | 'papers';

/** A checklist of things to add, with a filter box and an Add button. */
function Checklist<T>({
  items,
  keyOf,
  render,
  emptyText,
  onAdd,
  adding,
  filterPlaceholder,
  filter,
  onFilter,
}: {
  items: T[];
  keyOf: (item: T) => string;
  render: (item: T) => React.ReactNode;
  emptyText: string;
  onAdd: (items: T[]) => void;
  adding: boolean;
  filterPlaceholder?: string;
  filter?: string;
  onFilter?: (text: string) => void;
}) {
  const filterId = useId();
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  // A new list (after adding, or a new search) starts with nothing chosen.
  const itemsKey = items.map(keyOf).join('\u0000');
  useEffect(() => setChosen(new Set()), [itemsKey]);
  const selected = items.filter(i => chosen.has(keyOf(i)));
  return (
    <div className="flex flex-col gap-2">
      {onFilter && (
        <div className="relative">
          <label htmlFor={filterId} className="sr-only">{filterPlaceholder}</label>
          <Search aria-hidden="true" className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-shodh-text-muted" />
          <input
            id={filterId}
            type="search"
            className={cn(INPUT, 'pl-8 h-8')}
            value={filter ?? ''}
            onChange={e => onFilter(e.target.value)}
            placeholder={filterPlaceholder}
          />
        </div>
      )}
      {items.length === 0 ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">{emptyText}</p>
      ) : (
        <ul className="m-0 p-0 list-none max-h-[260px] overflow-y-auto scrollbar-thin rounded-lg border border-shodh-border-subtle divide-y divide-shodh-border-subtle">
          {items.map(item => {
            const key = keyOf(item);
            const id = `pick-${key}`;
            return (
              <li key={key} className="flex items-start gap-2 px-3 py-2">
                <input
                  id={id}
                  type="checkbox"
                  className="mt-1 accent-[var(--c-accent)]"
                  checked={chosen.has(key)}
                  onChange={e => setChosen(prev => {
                    const next = new Set(prev);
                    if (e.target.checked) next.add(key);
                    else next.delete(key);
                    return next;
                  })}
                />
                <label htmlFor={id} className="min-w-0 flex-1 text-[12.5px] cursor-pointer">{render(item)}</label>
              </li>
            );
          })}
        </ul>
      )}
      <div>
        <button type="button" className={PRIMARY_BUTTON} disabled={selected.length === 0 || adding} onClick={() => onAdd(selected)}>
          {adding ? <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Plus className="w-4 h-4" aria-hidden="true" />}
          {selected.length === 0 ? 'Add' : `Add ${selected.length}`}
        </button>
      </div>
    </div>
  );
}

/** Adding sources: folders from the Library, files, snippets and papers of the graph. */
function AddSources({
  workspace,
  library,
  onAdded,
}: {
  workspace: WorkspaceDetail;
  library: LibraryFolder[];
  onAdded: () => void;
}) {
  const [mode, setMode] = useState<AddMode>('folders');
  const [adding, setAdding] = useState(false);
  const [snippetQuery, setSnippetQuery] = useState('');
  const [snippets, setSnippets] = useState<Snippet[] | null>(null);
  const [snippetError, setSnippetError] = useState<string | null>(null);
  const [paperQuery, setPaperQuery] = useState('');
  const [papers, setPapers] = useState<ViewNode[] | null>(null);
  const [paperError, setPaperError] = useState<string | null>(null);
  const has = useMemo(() => new Set(workspace.sources.map(s => `${s.kind}\u0000${s.ref}`)), [workspace.sources]);

  useEffect(() => {
    if (mode !== 'snippets') return;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      researchApi
        .listSnippets({ text: snippetQuery.trim() || null, limit: 100 })
        .then(list => { if (!cancelled) { setSnippets(list); setSnippetError(null); } })
        .catch(err => { if (!cancelled) setSnippetError(toResearchError(err).message); });
    }, 250);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [mode, snippetQuery]);

  useEffect(() => {
    if (mode !== 'papers' || papers !== null) return;
    let cancelled = false;
    graphApi
      .view()
      .then(view => { if (!cancelled) { setPapers(view.nodes); setPaperError(null); } })
      .catch(err => { if (!cancelled) setPaperError(toResearchError(err).message); });
    return () => { cancelled = true; };
  }, [mode, papers]);

  const add = async (sources: NewSource[]) => {
    if (sources.length === 0) return;
    setAdding(true);
    try {
      const report = await workspacesApi.addSources(workspace.id, sources);
      notify.success(
        report.added === 1 ? 'Added 1 source' : `Added ${report.added} sources`,
        report.already > 0 ? { description: `${report.already} ${report.already === 1 ? 'was' : 'were'} already in the workspace.` } : undefined,
      );
      onAdded();
    } catch (err) {
      notify.error('The sources were not added', { description: workspaceError(err).message });
    } finally {
      setAdding(false);
    }
  };

  const chooseFiles = async () => {
    const picked = await openDialog({ multiple: true, directory: false, title: `Add files to “${workspace.name}”` });
    const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
    await add(paths.map(path => ({ kind: 'file' as const, ref: path, label: fileName(path), path })));
  };

  const folders = library.filter(f => !has.has(`folder\u0000${f.id}`));
  const snippetItems = (snippets ?? []).filter(s => !has.has(`snippet\u0000${s.id}`));
  const q = paperQuery.trim().toLowerCase();
  const paperItems = (papers ?? [])
    .filter(p => !has.has(`paper\u0000${p.id}`))
    .filter(p => q === '' || p.label.toLowerCase().includes(q))
    .sort((a, b) => Number(b.inLibrary) - Number(a.inLibrary) || b.libraryCiters - a.libraryCiters)
    .slice(0, 200);

  const tabs: { id: AddMode; label: string }[] = [
    { id: 'folders', label: 'Library folders' },
    { id: 'files', label: 'Files' },
    { id: 'snippets', label: 'Snippets' },
    { id: 'papers', label: 'Papers' },
  ];

  return (
    <section aria-label="Add sources" className="flex flex-col gap-3 rounded-xl border border-shodh-border-subtle bg-shodh-surface p-4">
      <div role="group" aria-label="What to add" className="flex flex-wrap gap-1">
        {tabs.map(t => (
          <button
            key={t.id}
            type="button"
            aria-pressed={mode === t.id}
            onClick={() => setMode(t.id)}
            className={cn(
              'h-8 px-3 rounded-full text-[12.5px] border transition-colors duration-micro',
              mode === t.id ? 'bg-shodh-accent-soft border-shodh-accent text-shodh-accent-text' : 'border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised',
              FOCUS_RING,
            )}
          >
            {t.label}
          </button>
        ))}
      </div>
      {mode === 'folders' && (
        <Checklist
          items={folders}
          keyOf={f => f.id}
          render={f => (
            <>
              <span className="block font-medium text-shodh-text">{f.name}</span>
              <span className="block text-[11.5px] text-shodh-text-muted truncate">{f.path}</span>
            </>
          )}
          emptyText={library.length === 0 ? 'The Library has no folders yet. Add one in Library first.' : 'Every Library folder is already in this workspace.'}
          adding={adding}
          onAdd={chosen => void add(chosen.map(f => ({ kind: 'folder' as const, ref: f.id, label: f.name, path: f.path })))}
        />
      )}
      {mode === 'files' && (
        <div className="flex flex-col gap-2">
          <p className="m-0 text-[12.5px] text-shodh-text-muted">
            Single files, for example one paper from a large folder. Answers search a file once it is indexed: files
            outside your Library folders show as not indexed until you add their folder in Library.
          </p>
          <div>
            <button type="button" className={OUTLINE_BUTTON} onClick={() => void chooseFiles()} disabled={adding}>
              <FileText className="w-3.5 h-3.5" aria-hidden="true" /> Choose files…
            </button>
          </div>
        </div>
      )}
      {mode === 'snippets' && (
        snippetError ? (
          <p role="alert" className="m-0 text-[12.5px] text-shodh-error">Snippets could not be loaded: {snippetError}</p>
        ) : (
          <Checklist
            items={snippetItems}
            keyOf={s => s.id}
            render={s => (
              <>
                <span className="block font-medium text-shodh-text">{s.title || `${s.fileName}, page ${s.page}`}</span>
                <span className="block text-[11.5px] text-shodh-text-muted line-clamp-2">{s.text || s.note}</span>
              </>
            )}
            emptyText={snippets === null ? 'Loading snippets…' : 'No snippets match. Save snippets from the PDF viewer.'}
            adding={adding}
            filterPlaceholder="Search snippets"
            filter={snippetQuery}
            onFilter={setSnippetQuery}
            onAdd={chosen => void add(chosen.map(s => ({
              kind: 'snippet' as const,
              ref: s.id,
              label: s.title || `${s.fileName}, page ${s.page}`,
              path: s.filePath,
            })))}
          />
        )
      )}
      {mode === 'papers' && (
        paperError ? (
          <p role="alert" className="m-0 text-[12.5px] text-shodh-error">The paper graph could not be loaded: {paperError}</p>
        ) : (
          <Checklist
            items={paperItems}
            keyOf={p => p.id}
            render={p => (
              <>
                <span className="block font-medium text-shodh-text">{p.label}{p.year ? ` (${p.year})` : ''}</span>
                <span className="block text-[11.5px] text-shodh-text-muted">
                  {p.inLibrary ? `In your library: ${p.filePath ? fileName(p.filePath) : 'PDF'}` : 'Not in your library: kept as a reference, never searched'}
                </span>
              </>
            )}
            emptyText={papers === null ? 'Loading the paper graph…' : 'No papers match. Build the graph in Library → Graph.'}
            adding={adding}
            filterPlaceholder="Search papers by title"
            filter={paperQuery}
            onFilter={setPaperQuery}
            onAdd={chosen => void add(chosen.map(p => ({
              kind: 'paper' as const,
              ref: p.id,
              label: p.label,
              path: p.inLibrary ? p.filePath : null,
            })))}
          />
        )
      )}
    </section>
  );
}

/**
 * "What's in this workspace": its sources by kind with their index state, what answers
 * search in total, and adding or removing sources. Removing a source never deletes it.
 */
export function SourcesTab({
  workspace,
  library,
  onChanged,
}: {
  workspace: WorkspaceDetail;
  library: LibraryFolder[];
  onChanged: () => void;
}) {
  const [health, setHealth] = useState<SourceHealth | null>(null);
  const [healthError, setHealthError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
  const [adding, setAdding] = useState(workspace.sources.length === 0);

  const check = useCallback(async () => {
    setChecking(true);
    try {
      setHealth(await workspacesApi.health(workspace.id));
      setHealthError(null);
    } catch (err) {
      setHealthError(workspaceError(err).message);
    } finally {
      setChecking(false);
    }
  }, [workspace.id]);

  useEffect(() => {
    void check();
  }, [check, workspace.sources]);

  const stateOf = (s: WorkspaceSource): SourceState | null =>
    health?.sources.find(h => h.kind === s.kind && h.ref === s.ref)?.state ?? null;
  const statusOf = (s: WorkspaceSource) => health?.sources.find(h => h.kind === s.kind && h.ref === s.ref) ?? null;

  // Hidden at once and removed when the undo window ends, so Undo keeps the source's
  // row exactly (who added it and when).
  const remove = (s: WorkspaceSource) => {
    const key = `${s.kind}\u0000${s.ref}`;
    const workspaceId = workspace.id;
    const toggle = (on: boolean) =>
      setHidden(prev => {
        const next = new Set(prev);
        if (on) next.add(key);
        else next.delete(key);
        return next;
      });
    removeWithUndo({
      message: `Removed “${s.label}” from the workspace`,
      description: 'It stays in your Library.',
      hide: () => toggle(true),
      restore: () => toggle(false),
      commit: async () => {
        await workspacesApi.removeSource(workspaceId, s.kind, s.ref);
        onChanged();
      },
      onError: err => notify.error('The source was not removed', { description: workspaceError(err).message }),
    });
  };

  // A removed source leaves the workspace once it is gone; forget it then, so one
  // added again shows.
  useEffect(() => {
    setHidden(prev => {
      const present = new Set(workspace.sources.map(s => `${s.kind}\u0000${s.ref}`));
      const kept = [...prev].filter(key => present.has(key));
      return kept.length === prev.size ? prev : new Set(kept);
    });
  }, [workspace.sources]);

  const byKind = KIND_ORDER.map(kind => ({
    kind,
    items: workspace.sources.filter(s => s.kind === kind && !hidden.has(`${s.kind}\u0000${s.ref}`)),
  })).filter(g => g.items.length > 0);

  return (
    <div className="flex flex-col gap-5">
      <section aria-labelledby="ws-health" className="flex flex-col gap-1.5">
        <div className="flex items-center gap-2">
          <h3 id="ws-health" className={SECTION_TITLE}>What answers search</h3>
          <button type="button" className={cn(QUIET_BUTTON, 'h-7 ml-auto')} onClick={() => void check()} disabled={checking} aria-label="Check the index again">
            <RefreshCw className={cn('w-3.5 h-3.5', checking && 'animate-spin motion-reduce:animate-none')} aria-hidden="true" /> Check again
          </button>
        </div>
        {healthError ? (
          <p role="alert" className="m-0 text-[12.5px] text-shodh-error">The index could not be checked: {healthError}</p>
        ) : health === null ? (
          <p role="status" className="m-0 text-[12.5px] text-shodh-text-muted">Checking the index…</p>
        ) : workspace.sources.length === 0 ? (
          <p className="m-0 text-[12.5px] text-shodh-text-secondary">
            Nothing yet: answers in this workspace find nothing until you add sources (or turn on “Search all my
            library” for a question).
          </p>
        ) : (
          <p role="status" className="m-0 text-[12.5px] text-shodh-text-secondary">
            {`${health.searchableFiles.toLocaleString()} ${health.searchableFiles === 1 ? 'file' : 'files'}`}
            {health.chunks > 0 ? ` (${health.chunks.toLocaleString()} indexed passages)` : ''}
            {workspace.sourceCounts.snippets > 0 ? ` and ${workspace.sourceCounts.snippets} ${workspace.sourceCounts.snippets === 1 ? 'snippet' : 'snippets'}` : ''}
            {'. '}
            {health.problems > 0
              ? `${health.problems} ${health.problems === 1 ? 'source needs' : 'sources need'} attention: answers cannot search ${health.problems === 1 ? 'it' : 'them'}.`
              : 'Every source is searchable.'}
          </p>
        )}
      </section>

      {byKind.map(group => {
        const Icon = KIND_ICONS[group.kind];
        return (
          <section key={group.kind} aria-labelledby={`ws-kind-${group.kind}`} className="flex flex-col">
            <h3 id={`ws-kind-${group.kind}`} className={cn(SECTION_TITLE, 'flex items-center gap-1.5')}>
              <Icon className="w-3.5 h-3.5 text-shodh-text-muted" aria-hidden="true" />
              {SOURCE_KIND_LABELS[group.kind]} <span className="font-normal text-shodh-text-muted">({group.items.length})</span>
            </h3>
            <ul className="m-0 p-0 list-none divide-y divide-shodh-border-subtle">
              {group.items.map(s => {
                const key = `${s.kind}\u0000${s.ref}`;
                const status = statusOf(s);
                return (
                  <li key={key} className="flex items-start gap-3 py-2">
                    <div className="min-w-0 flex-1">
                      <p className="m-0 text-[13px] text-shodh-text truncate">{s.label}</p>
                      {s.path && s.path !== s.label && (
                        <p className="m-0 text-[11.5px] text-shodh-text-muted truncate" title={s.path}>{s.path}</p>
                      )}
                      <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5 mt-0.5">
                        <StateBadge state={stateOf(s)} />
                        {status?.files !== null && status?.files !== undefined && s.kind === 'folder' && (
                          <span className="text-[11.5px] text-shodh-text-muted">
                            {`${status.files.toLocaleString()} ${status.files === 1 ? 'file' : 'files'}`}
                            {status.chunks !== null ? ` · ${status.chunks.toLocaleString()} passages` : ''}
                          </span>
                        )}
                        {s.addedBy === 'agent' && <span className="text-[11.5px] text-shodh-text-muted">Added by Shodh (you approved)</span>}
                      </div>
                    </div>
                    <button
                      type="button"
                      className={cn(QUIET_BUTTON, 'h-7')}
                      onClick={() => remove(s)}
                      aria-label={`Remove ${s.label} from the workspace`}
                    >
                      <X className="w-3.5 h-3.5" aria-hidden="true" />
                      Remove
                    </button>
                  </li>
                );
              })}
            </ul>
          </section>
        );
      })}

      <div className="flex flex-col gap-2">
        <button type="button" className={cn(OUTLINE_BUTTON, 'self-start')} aria-expanded={adding} onClick={() => setAdding(a => !a)}>
          <Plus className="w-3.5 h-3.5" aria-hidden="true" /> {adding ? 'Close' : 'Add sources'}
        </button>
        {adding && <AddSources workspace={workspace} library={library} onAdded={onChanged} />}
      </div>
    </div>
  );
}
