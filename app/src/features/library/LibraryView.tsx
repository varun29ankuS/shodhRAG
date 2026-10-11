import React, { useCallback, useEffect, useId, useState } from 'react';
import {
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  FolderPlus,
  FolderSearch,
  Network,
  RotateCcw,
  Trash2,
} from 'lucide-react';
import { cn } from '../../lib/utils';
import { relativeTime } from '../../utils/time';
import { SearchSetupCard } from '../setup/SearchSetupCard';
import { VisualGallery } from '../visuals/VisualGallery';
import { SnippetShelf } from '../research/SnippetShelf';
import { CompareResults } from '../research/CompareResults';
import { PaperGraphView } from '../research/PaperGraphView';
import { PaperPage } from '../research/PaperPage';
import { ConceptPage } from '../research/ConceptPage';
import type { GraphPage } from '../research/graphTypes';
import { scrollBehavior } from '../ask/viewer/sourceAccess';
import { baseName } from './fileTree';
import type { FileNode } from './fileTree';
import { FileBrowser } from './FileBrowser';
import { showInFolder } from './fileActions';
import { adviceForFailures, indexingAdvice } from './indexingAdvice';
import type { IndexingAction, IndexingAdvice } from './indexingAdvice';
import { useSearchModels } from '../setup/SearchModelsContext';
import { groupSourcesByKind, progressPercent } from './sources';
import type { LibrarySource } from './sources';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const PRIMARY_BUTTON = cn(
  'h-9 px-3.5 inline-flex items-center gap-2 rounded-lg bg-shodh-accent text-shodh-on-accent text-[13px] font-semibold hover:bg-shodh-accent-hover transition-colors duration-micro',
  FOCUS_RING,
);

const QUIET_BUTTON = cn(
  'h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
  FOCUS_RING,
);

/** File types the indexer reads, for the empty state. */
const SUPPORTED_SUMMARY = 'PDF, Word, Excel, PowerPoint, CSV, Markdown, text and code files';

interface LibraryViewProps {
  sources: LibrarySource[];
  /** Documents in the index (from `get_statistics`). */
  totalDocs: number;
  /** The index is still opening; file lists wait for it. */
  indexReady: boolean;
  onAddFolder: () => void;
  onReindex: (id: string) => void;
  onToggleSource: (id: string) => void;
  onRemoveSource: (id: string, event: React.MouseEvent) => void;
  onAskAboutFile: (file: FileNode, source: LibrarySource) => void;
  /** The file browser counted a source's indexed files. */
  onFileCount: (id: string, count: number) => void;
  /** Source the agent pointed at (`show_source`): its card is scrolled to and highlighted. */
  focusedSourceId?: string | null;
}

/**
 * Library: every source Shodh can search. Today these are folders on this
 * computer, shown as cards with live indexing progress, counts, the last
 * indexing time and the files that could not be indexed. A card opens the
 * folder in the file browser.
 */
export function LibraryView({
  sources,
  totalDocs,
  indexReady,
  onAddFolder,
  onReindex,
  onToggleSource,
  onRemoveSource,
  onAskAboutFile,
  onFileCount,
  focusedSourceId = null,
}: LibraryViewProps) {
  const [browsingId, setBrowsingId] = useState<string | null>(null);
  const browsing = sources.find(s => s.id === browsingId) ?? null;
  /** The graph area (graph, paper, method or dataset page) with its back stack. */
  const [graphPages, setGraphPages] = useState<GraphPage[]>([]);
  const graphPage = graphPages[graphPages.length - 1] ?? null;
  const pushPage = useCallback((page: GraphPage) => setGraphPages(stack => [...stack, page]), []);
  const popPage = useCallback(() => setGraphPages(stack => stack.slice(0, -1)), []);

  const reportCount = useCallback((count: number) => {
    if (browsingId) onFileCount(browsingId, count);
  }, [browsingId, onFileCount]);

  // The browsed source was removed.
  useEffect(() => {
    if (browsingId && !browsing) setBrowsingId(null);
  }, [browsingId, browsing]);

  // The agent pointed at a source: leave the file browser so its card shows.
  useEffect(() => {
    if (focusedSourceId) setBrowsingId(null);
  }, [focusedSourceId]);

  // Bring the pointed-at card into view once it is rendered.
  const showingCards = !(browsing && indexReady);
  useEffect(() => {
    if (!focusedSourceId || !showingCards) return;
    document
      .getElementById(`library-source-${focusedSourceId}`)
      ?.scrollIntoView({ block: 'center', behavior: scrollBehavior() });
  }, [focusedSourceId, showingCards]);

  if (browsing && indexReady) {
    return (
      <div key={browsing.id} className="shell-view-enter h-full">
        <FileBrowser source={browsing} onExit={() => setBrowsingId(null)} onAskAboutFile={onAskAboutFile} onFileCount={reportCount} />
      </div>
    );
  }

  if (graphPage) {
    const openPaper = (id: string) => pushPage({ kind: 'paper', id });
    const openConcept = (kind: 'method' | 'dataset', id: string) => pushPage({ kind, id });
    return (
      <div className="h-full overflow-y-auto scrollbar-thin">
        <div className="max-w-5xl mx-auto px-8 py-7 flex flex-col gap-4">
          {graphPage.kind === 'graph' ? (
            <>
              <header className="flex items-start justify-between gap-4">
                <div className="flex flex-col gap-1">
                  <h1 className="m-0 text-2xl font-bold text-shodh-text">Paper graph</h1>
                  <p className="text-sm text-shodh-text-muted">How your papers cite each other and the work they build on, read from their bibliographies.</p>
                </div>
                <button type="button" onClick={popPage} className={QUIET_BUTTON}>Back to Library</button>
              </header>
              <PaperGraphView onOpenPaper={openPaper} />
            </>
          ) : graphPage.kind === 'paper' ? (
            <PaperPage key={graphPage.id} paperId={graphPage.id} onBack={popPage} onOpenPaper={openPaper} onOpenConcept={openConcept} />
          ) : (
            <ConceptPage key={`${graphPage.kind}:${graphPage.id}`} kind={graphPage.kind} conceptId={graphPage.id} onBack={popPage} onOpenPaper={openPaper} />
          )}
        </div>
      </div>
    );
  }

  const groups = groupSourcesByKind(sources);
  const indexing = sources.filter(s => s.status === 'indexing').length;
  const fileTotal = sources.reduce((sum, s) => sum + (s.fileCount || 0), 0);

  return (
    <div className="h-full overflow-y-auto scrollbar-thin">
      <div className="max-w-5xl mx-auto px-8 py-7 flex flex-col gap-6">
        <header className="flex items-start justify-between gap-4">
          <div className="flex flex-col gap-1">
            <h1 className="m-0 text-2xl font-bold text-shodh-text">Library</h1>
            <p className="text-sm text-shodh-text-muted">
              {sources.length === 0
                ? 'Everything Shodh can search. Nothing has been added yet.'
                : `${Math.max(totalDocs, fileTotal).toLocaleString()} files in ${sources.length} source${sources.length === 1 ? '' : 's'}${indexing > 0 ? ` · indexing ${indexing}` : ''}`}
            </p>
          </div>
          {sources.length > 0 && (
            <button type="button" onClick={onAddFolder} className={PRIMARY_BUTTON}>
              <FolderPlus className="w-4 h-4" aria-hidden="true" />
              Add folder
            </button>
          )}
        </header>

        <SearchSetupCard />

        {sources.length === 0 ? (
          <EmptyLibrary onAddFolder={onAddFolder} />
        ) : (
          groups.map(group => (
            <section key={group.id} aria-labelledby={`library-kind-${group.id}`} className="flex flex-col gap-3">
              <h2 id={`library-kind-${group.id}`} className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
                {group.label}
              </h2>
              <ul className="grid gap-3 grid-cols-[repeat(auto-fill,minmax(300px,1fr))]">
                {group.sources.map(source => (
                  <li key={source.id} className="ask-rise">
                    <FolderCard
                      source={source}
                      focused={source.id === focusedSourceId}
                      browseDisabled={!indexReady}
                      onBrowse={() => setBrowsingId(source.id)}
                      onReindex={() => onReindex(source.id)}
                      onToggle={() => onToggleSource(source.id)}
                      onRemove={e => onRemoveSource(source.id, e)}
                    />
                  </li>
                ))}
              </ul>
            </section>
          ))
        )}

        {sources.length > 0 && (
          <section aria-labelledby="library-snippets" className="flex flex-col gap-3">
            <div className="flex flex-col gap-0.5">
              <h2 id="library-snippets" className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
                Snippets
              </h2>
              <p className="text-[12.5px] text-shodh-text-muted">
                Regions saved from papers: figures, tables, equations and passages. Drag one onto the chat to ask about it.
              </p>
            </div>
            <SnippetShelf />
          </section>
        )}

        {sources.length > 0 && (
          <section aria-labelledby="library-graph" className="flex flex-col gap-3">
            <div className="flex items-end justify-between gap-3">
              <div className="flex flex-col gap-0.5">
                <h2 id="library-graph" className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
                  Graph
                </h2>
                <p className="text-[12.5px] text-shodh-text-muted">
                  Which of your papers cite each other, what they build on, and a page for every paper, method and dataset.
                </p>
              </div>
              <button type="button" onClick={() => pushPage({ kind: 'graph' })} className={cn(QUIET_BUTTON, 'text-shodh-text font-medium border border-shodh-border')}>
                <Network className="w-3.5 h-3.5" aria-hidden="true" />
                Open graph
              </button>
            </div>
          </section>
        )}

        {sources.length > 0 && (
          <section aria-labelledby="library-compare" className="flex flex-col gap-3">
            <div className="flex flex-col gap-0.5">
              <h2 id="library-compare" className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
                Compare results
              </h2>
              <p className="text-[12.5px] text-shodh-text-muted">
                Results extracted from papers’ tables, side by side. Every value opens the cell it came from.
              </p>
            </div>
            <CompareResults />
          </section>
        )}

        <section aria-labelledby="library-generated" className="flex flex-col gap-3">
          <div className="flex flex-col gap-0.5">
            <h2 id="library-generated" className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
              Generated
            </h2>
            <p className="text-[12.5px] text-shodh-text-muted">
              Diagrams, charts, sketches, plots, simulations, equations and tables from every conversation, with your snippets.
            </p>
          </div>
          <VisualGallery conversationId={null} />
        </section>
      </div>
    </div>
  );
}

function EmptyLibrary({ onAddFolder }: { onAddFolder: () => void }) {
  return (
    <section aria-labelledby="library-empty" className="rounded-2xl border border-dashed border-shodh-border-strong bg-shodh-surface px-8 py-10 flex flex-col items-center text-center gap-4">
      <span className="w-12 h-12 rounded-xl bg-shodh-accent-soft inline-flex items-center justify-center" aria-hidden="true">
        <FolderPlus className="w-6 h-6 text-shodh-accent-text" />
      </span>
      <div className="flex flex-col gap-1.5 max-w-md">
        <h2 id="library-empty" className="text-[17px] font-semibold text-shodh-text">Add a folder to start searching it</h2>
        <p className="text-[13px] leading-relaxed text-shodh-text-muted">
          Shodh reads the {SUPPORTED_SUMMARY} in the folder and its subfolders, and indexes them on this computer. Nothing
          is uploaded. Answers in Ask then cite the exact passage they came from.
        </p>
      </div>
      <button type="button" onClick={onAddFolder} className={PRIMARY_BUTTON}>
        <FolderPlus className="w-4 h-4" aria-hidden="true" />
        Add folder
      </button>
      <p className="text-[12px] text-shodh-text-faint">You can also drop a folder or file anywhere on this window.</p>
    </section>
  );
}

interface FolderCardProps {
  source: LibrarySource;
  /** The agent pointed at this source. */
  focused: boolean;
  browseDisabled: boolean;
  onBrowse: () => void;
  onReindex: () => void;
  onToggle: () => void;
  onRemove: (e: React.MouseEvent) => void;
}

/** One line on what to do about an indexing failure, with the button that does it. */
function AdviceLine({ advice, onAction }: { advice: IndexingAdvice; onAction: (action: IndexingAction) => void }) {
  return (
    <p className="m-0 flex flex-wrap items-center gap-x-2 gap-y-1 text-[12px] text-shodh-text-secondary">
      <span className="min-w-0">{advice.hint}</span>
      {advice.actionLabel && (
        <button
          type="button"
          onClick={() => onAction(advice.action)}
          className={cn('h-6 px-2 rounded-md bg-shodh-raised-2 text-[11.5px] font-medium text-shodh-text hover:bg-shodh-pressed transition-colors duration-micro', FOCUS_RING)}
        >
          {advice.actionLabel}
        </button>
      )}
    </p>
  );
}

function FolderCard({ source, focused, browseDisabled, onBrowse, onReindex, onToggle, onRemove }: FolderCardProps) {
  const [showFailures, setShowFailures] = useState(false);
  const failuresId = useId();
  const nameId = useId();
  const failures = source.failures ?? [];
  const percent = progressPercent(source);
  const { install } = useSearchModels();
  const act = (action: IndexingAction) => {
    if (action === 'reindex') onReindex();
    else if (action === 'show_in_folder') void showInFolder(source.path);
    else if (action === 'install_search_models') void install();
  };
  const failureAdvice = source.status === 'indexing' ? null : adviceForFailures(failures.map(f => f.reason));

  let statusLine: React.ReactNode;
  if (source.status === 'indexing') {
    const total = source.fileCount || 0;
    const done = source.processedCount ?? 0;
    statusLine = (
      <div className="flex flex-col gap-1.5">
        <div className="flex items-center justify-between text-[12px] text-shodh-text-secondary">
          <span>{total > 0 ? `Indexing ${done.toLocaleString()} of ${total.toLocaleString()} files` : 'Preparing files…'}</span>
          <span className="font-mono text-[11px]">{percent}%</span>
        </div>
        <div
          role="progressbar"
          aria-labelledby={nameId}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
          aria-valuetext={total > 0 ? `${done} of ${total} files, ${percent}%` : 'Preparing files'}
          className="h-1.5 rounded-full bg-shodh-raised-2 overflow-hidden"
        >
          <div
            className={cn('h-full w-full origin-left bg-shodh-warning transition-transform duration-panel ease-standard', total === 0 && 'ask-breathe')}
            style={{ transform: `scaleX(${total === 0 ? 0.12 : percent / 100})` }}
          />
        </div>
        {source.currentFile && (
          <p className="text-[11.5px] font-mono text-shodh-text-faint truncate" title={source.currentFile}>
            {baseName(source.currentFile)}
          </p>
        )}
      </div>
    );
  } else if (source.status === 'error') {
    statusLine = (
      <div className="flex flex-col gap-1">
        <p className="m-0 text-[12.5px] text-shodh-error flex items-start gap-1.5">
          <AlertTriangle className="w-3.5 h-3.5 mt-px shrink-0" aria-hidden="true" />
          <span className="break-words">{source.lastError ? `Indexing failed: ${source.lastError}` : 'Indexing failed.'}</span>
        </p>
        <AdviceLine advice={indexingAdvice(source.lastError)} onAction={act} />
      </div>
    );
  } else if (source.status === 'interrupted') {
    statusLine = (
      <p className="text-[12.5px] text-shodh-warning flex items-start gap-1.5">
        <AlertTriangle className="w-3.5 h-3.5 mt-px shrink-0" aria-hidden="true" />
        <span>Indexing stopped before it finished (the app was closed). Index it again to finish.</span>
      </p>
    );
  } else {
    statusLine = (
      <div className="flex flex-col gap-1">
        <p className="text-[12.5px] text-shodh-text-secondary">
          {`${(source.fileCount || 0).toLocaleString()} file${source.fileCount === 1 ? '' : 's'}`}
          {source.indexedAt && <span className="text-shodh-text-faint">{` · indexed ${relativeTime(source.indexedAt)}`}</span>}
        </p>
        {source.syncError && (
          <p className="m-0 text-[12px] text-shodh-warning flex items-start gap-1.5">
            <AlertTriangle className="w-3.5 h-3.5 mt-px shrink-0" aria-hidden="true" />
            <span className="break-words">{`Not kept in sync: ${source.syncError}`}</span>
          </p>
        )}
      </div>
    );
  }

  return (
    <article
      id={`library-source-${source.id}`}
      aria-labelledby={nameId}
      aria-current={focused ? 'true' : undefined}
      className={cn(
        'h-full rounded-xl border bg-shodh-surface p-4 flex flex-col gap-3 transition-colors duration-micro',
        focused ? 'border-shodh-accent ring-1 ring-shodh-accent' : 'border-shodh-border hover:border-shodh-border-strong',
      )}
    >
      <div className="flex items-start gap-3 min-w-0">
        <span className="w-9 h-9 shrink-0 rounded-lg bg-shodh-raised inline-flex items-center justify-center" aria-hidden="true">
          {source.status === 'indexing' ? <FolderOpen className="w-[18px] h-[18px] text-shodh-warning" /> : <Folder className="w-[18px] h-[18px] text-shodh-text-muted" />}
        </span>
        <div className="flex-1 min-w-0">
          <h3 id={nameId} className="text-[14px] font-semibold text-shodh-text truncate" title={source.name}>{source.name}</h3>
          <p className="text-[11.5px] text-shodh-text-faint truncate" title={source.path}>{source.path}</p>
        </div>
        <label
          className="shrink-0 flex items-center gap-1.5 text-[11.5px] text-shodh-text-muted cursor-pointer select-none"
          title="Include this folder when answering in Ask"
        >
          <input
            type="checkbox"
            checked={source.selected}
            onChange={onToggle}
            className={cn('w-3.5 h-3.5 accent-[var(--c-accent)] rounded', FOCUS_RING)}
          />
          Use in Ask
        </label>
      </div>

      {statusLine}

      {failures.length > 0 && source.status !== 'indexing' && (
        <div className="flex flex-col gap-1.5">
          <button
            type="button"
            onClick={() => setShowFailures(v => !v)}
            aria-expanded={showFailures}
            aria-controls={failuresId}
            className={cn('self-start inline-flex items-center gap-1 text-[12px] text-shodh-error hover:underline rounded', FOCUS_RING)}
          >
            {showFailures ? <ChevronDown className="w-3.5 h-3.5" aria-hidden="true" /> : <ChevronRight className="w-3.5 h-3.5" aria-hidden="true" />}
            {`${failures.length} file${failures.length === 1 ? '' : 's'} could not be indexed`}
          </button>
          {failureAdvice && <AdviceLine advice={failureAdvice} onAction={act} />}
          {showFailures && (
            <ul id={failuresId} className="max-h-40 overflow-y-auto scrollbar-thin flex flex-col gap-1 rounded-lg bg-shodh-raised p-2">
              {failures.map(f => (
                <li key={f.file} className="text-[11.5px] leading-snug">
                  <span className="text-shodh-text-secondary break-all">{baseName(f.file)}</span>
                  {f.reason && <span className="block text-shodh-text-faint break-words">{f.reason}</span>}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <div className="mt-auto pt-1 flex items-center gap-1 -mx-1">
        <button
          type="button"
          onClick={onBrowse}
          disabled={browseDisabled}
          title={browseDisabled ? 'Available once the index has opened' : undefined}
          className={cn(QUIET_BUTTON, 'text-shodh-text font-medium disabled:opacity-50 disabled:cursor-not-allowed')}
        >
          <FolderOpen className="w-3.5 h-3.5" aria-hidden="true" />
          Browse files
        </button>
        {(source.status === 'error' || source.status === 'interrupted' || source.status === 'ready') && (
          <button type="button" onClick={onReindex} className={QUIET_BUTTON} aria-label={`Index ${source.name} again`}>
            <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
            {source.status === 'ready' ? 'Re-index' : 'Index again'}
          </button>
        )}
        <button type="button" onClick={() => void showInFolder(source.path)} className={QUIET_BUTTON} aria-label={`Show ${source.name} in folder`} title="Show in folder">
          <FolderSearch className="w-3.5 h-3.5" aria-hidden="true" />
        </button>
        <button
          type="button"
          onClick={onRemove}
          aria-label={`Remove ${source.name} from the Library`}
          title="Remove from Library"
          className={cn(QUIET_BUTTON, 'ml-auto text-shodh-error hover:text-shodh-error')}
        >
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
        </button>
      </div>
    </article>
  );
}
