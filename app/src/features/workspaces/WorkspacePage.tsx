import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ask } from '@tauri-apps/plugin-dialog';
import { Archive, ArchiveRestore, ArrowLeft, MessageSquarePlus, MoreHorizontal, Pin, PinOff, Trash2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { relativeTime } from '../../utils/time';
import { useChatSession } from '../ask/ChatSessionContext';
import MemorySettings from '../../components/MemorySettings';
import { VisualGallery } from '../visuals/VisualGallery';
import { CompareResults } from '../research/CompareResults';
import { PaperGraphView } from '../research/PaperGraphView';
import { PaperPage } from '../research/PaperPage';
import { ConceptPage } from '../research/ConceptPage';
import type { GraphPage, ViewNode } from '../research/graphTypes';
import { onWorkspacesChanged, workspaceError, workspacesApi } from './api';
import { OverviewTab } from './OverviewTab';
import { SourcesTab } from './SourcesTab';
import type { LibraryFolder } from './SourcesTab';
import { pathWithin, sourceSummary, WORKSPACE_TAB_LABELS, WORKSPACE_TABS } from './model';
import type { WorkspaceDetail, WorkspaceTab } from './types';
import { WorkspaceIcon } from './WorkspaceIcon';
import { FOCUS_RING, PRIMARY_BUTTON, QUIET_BUTTON, SECTION_TITLE } from './ui';

const CASE_INSENSITIVE_PATHS = typeof navigator !== 'undefined' && /Win/i.test(navigator.platform);

/** Whether a library file belongs to the workspace (a file or paper of it, or under a folder of it). */
function fileFilter(workspace: WorkspaceDetail): (filePath: string) => boolean {
  const files = workspace.sources.filter(s => (s.kind === 'file' || s.kind === 'paper') && s.path).map(s => s.path as string);
  const folders = workspace.sources.filter(s => s.kind === 'folder' && s.path).map(s => s.path as string);
  return (filePath: string) =>
    files.some(f => pathWithin(filePath, f, CASE_INSENSITIVE_PATHS) && pathWithin(f, filePath, CASE_INSENSITIVE_PATHS))
    || folders.some(folder => pathWithin(filePath, folder, CASE_INSENSITIVE_PATHS));
}

function ChatsTab({ workspace, onNewChat, onOpenChat }: { workspace: WorkspaceDetail; onNewChat: () => void; onOpenChat: (id: string) => void }) {
  const { conversations } = useChatSession();
  const chats = useMemo(
    () => conversations
      .filter(c => c.workspaceId === workspace.id)
      .sort((a, b) => Number(b.pinned) - Number(a.pinned) || new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime()),
    [conversations, workspace.id],
  );
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <h3 className={SECTION_TITLE}>
          Chats <span className="font-normal text-shodh-text-muted">({chats.length})</span>
        </h3>
        <button type="button" className={cn(PRIMARY_BUTTON, 'ml-auto h-8')} onClick={onNewChat}>
          <MessageSquarePlus className="w-4 h-4" aria-hidden="true" /> New chat
        </button>
      </div>
      {chats.length === 0 ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">
          No chats yet. A chat started here searches only this workspace’s sources and follows its instructions. Move
          an existing chat here from its menu in the sidebar.
        </p>
      ) : (
        <ul className="m-0 p-0 list-none divide-y divide-shodh-border-subtle">
          {chats.map(c => (
            <li key={c.id}>
              <button
                type="button"
                onClick={() => onOpenChat(c.id)}
                className={cn('w-full flex items-center gap-3 py-2 px-1 text-left rounded-md hover:bg-shodh-raised/60', FOCUS_RING)}
              >
                {c.pinned && <Pin className="w-3 h-3 text-shodh-text-faint shrink-0" aria-label="Pinned" />}
                <span className="flex-1 min-w-0 text-[13px] text-shodh-text truncate">{c.title}</span>
                <span className="text-[11.5px] text-shodh-text-muted shrink-0">{relativeTime(c.updatedAt)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function ResultsTab({ workspace }: { workspace: WorkspaceDetail }) {
  const [page, setPage] = useState<GraphPage[]>([{ kind: 'graph' }]);
  const current = page[page.length - 1];
  const push = (next: GraphPage) => setPage(stack => [...stack, next]);
  const pop = () => setPage(stack => (stack.length > 1 ? stack.slice(0, -1) : stack));
  const allows = useMemo(() => fileFilter(workspace), [workspace]);
  const paperIds = useMemo(() => new Set(workspace.sources.filter(s => s.kind === 'paper').map(s => s.ref)), [workspace.sources]);
  const restrict = useCallback(
    (node: ViewNode) => paperIds.has(node.id) || (node.filePath !== null && allows(node.filePath)),
    [paperIds, allows],
  );
  const openPaper = (id: string) => push({ kind: 'paper', id });
  const openConcept = (kind: 'method' | 'dataset', id: string) => push({ kind, id });
  if (current.kind === 'paper') {
    return <PaperPage key={current.id} paperId={current.id} onBack={pop} onOpenPaper={openPaper} onOpenConcept={openConcept} />;
  }
  if (current.kind === 'method' || current.kind === 'dataset') {
    return <ConceptPage key={`${current.kind}:${current.id}`} kind={current.kind} conceptId={current.id} onBack={pop} onOpenPaper={openPaper} />;
  }
  return (
    <div className="flex flex-col gap-6">
      <section aria-labelledby="ws-results" className="flex flex-col gap-2">
        <h3 id="ws-results" className={SECTION_TITLE}>Results across this workspace’s papers</h3>
        <CompareResults paperFilter={allows} />
      </section>
      <section aria-labelledby="ws-graph" className="flex flex-col gap-2">
        <h3 id="ws-graph" className={SECTION_TITLE}>Citation graph of this workspace’s papers</h3>
        <p className="m-0 text-[12.5px] text-shodh-text-muted">
          Its library papers and the works they cite. Library papers outside the workspace are left out.
        </p>
        <PaperGraphView onOpenPaper={openPaper} restrict={restrict} />
      </section>
    </div>
  );
}

/**
 * One workspace: its header (name, what it searches, new chat, pin, archive, delete) and
 * tabs for the overview and instructions, sources, chats, memory, visuals, and results
 * and the citation graph.
 */
export function WorkspacePage({
  workspaceId,
  tab,
  onTab,
  onBack,
  library,
  onNewChat,
  onOpenChat,
}: {
  workspaceId: string;
  tab: WorkspaceTab;
  onTab: (tab: WorkspaceTab) => void;
  onBack: () => void;
  library: LibraryFolder[];
  onNewChat: (workspaceId: string) => void;
  onOpenChat: (conversationId: string) => void;
}) {
  const { conversations, switchConversation, detachWorkspace } = useChatSession();
  const [detail, setDetail] = useState<WorkspaceDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const tabRefs = useRef<Partial<Record<WorkspaceTab, HTMLButtonElement | null>>>({});
  const menuRef = useRef<HTMLDivElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);

  const load = useCallback(async () => {
    try {
      setDetail(await workspacesApi.get(workspaceId));
      setError(null);
    } catch (err) {
      setError(workspaceError(err).message);
    }
  }, [workspaceId]);

  useEffect(() => {
    setDetail(null);
    void load();
  }, [load]);

  // Changes from elsewhere (an assistant edit the user approved, another window).
  useEffect(
    () => onWorkspacesChanged(id => {
      if (id === null || id === workspaceId) void load();
    }),
    [load, workspaceId],
  );

  useEffect(() => {
    if (!menuOpen) return;
    const close = (e: PointerEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuOpen(false);
    };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, [menuOpen]);

  const chatIds = useMemo(() => conversations.filter(c => c.workspaceId === workspaceId).map(c => c.id), [conversations, workspaceId]);

  if (error) {
    return (
      <div className="flex flex-col gap-3">
        <button type="button" className={cn(QUIET_BUTTON, 'self-start')} onClick={onBack}>
          <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" /> All workspaces
        </button>
        <p role="alert" className="m-0 text-[13px] text-shodh-error">This workspace could not be opened: {error}</p>
      </div>
    );
  }
  if (!detail) {
    return <p role="status" className="m-0 text-[13px] text-shodh-text-muted">Opening the workspace…</p>;
  }

  const setPinned = async () => {
    setMenuOpen(false);
    try {
      await workspacesApi.update(detail.id, { pinned: !detail.pinned });
      void load();
    } catch (err) {
      notify.error('The workspace was not changed', { description: workspaceError(err).message });
    }
  };

  const setArchived = async () => {
    setMenuOpen(false);
    try {
      await workspacesApi.setArchived(detail.id, !detail.archived);
      notify.success(detail.archived ? `“${detail.name}” is back in the list` : `Archived “${detail.name}”`, {
        description: detail.archived ? undefined : 'Its chats are hidden from the sidebar until you unarchive it.',
      });
      void load();
    } catch (err) {
      notify.error('The workspace was not changed', { description: workspaceError(err).message });
    }
  };

  const remove = async () => {
    setMenuOpen(false);
    const confirmed = await ask(
      `Delete the workspace “${detail.name}”? Its instructions and their history and its list of sources are deleted. Its ${chatIds.length} ${chatIds.length === 1 ? 'chat moves' : 'chats move'} to No workspace; your files, snippets and memories are kept.`,
      { title: 'Delete workspace', kind: 'warning', okLabel: 'Delete', cancelLabel: 'Keep' },
    );
    if (!confirmed) return;
    try {
      await workspacesApi.remove(detail.id);
      detachWorkspace(detail.id);
      notify.success(`Deleted “${detail.name}”`);
      onBack();
    } catch (err) {
      notify.error('The workspace was not deleted', { description: workspaceError(err).message });
    }
  };

  const moveTab = (e: React.KeyboardEvent, current: WorkspaceTab) => {
    const index = WORKSPACE_TABS.indexOf(current);
    let next: WorkspaceTab | null = null;
    if (e.key === 'ArrowRight') next = WORKSPACE_TABS[(index + 1) % WORKSPACE_TABS.length];
    else if (e.key === 'ArrowLeft') next = WORKSPACE_TABS[(index - 1 + WORKSPACE_TABS.length) % WORKSPACE_TABS.length];
    else if (e.key === 'Home') next = WORKSPACE_TABS[0];
    else if (e.key === 'End') next = WORKSPACE_TABS[WORKSPACE_TABS.length - 1];
    if (!next) return;
    e.preventDefault();
    onTab(next);
    tabRefs.current[next]?.focus();
  };

  const menuItem = 'w-full flex items-center gap-2 px-3 py-1.5 text-[12.5px] text-left hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none';

  return (
    <div className="flex flex-col gap-5">
      <button type="button" className={cn(QUIET_BUTTON, 'self-start -ml-2')} onClick={onBack}>
        <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" /> All workspaces
      </button>
      <header className="flex items-start gap-3">
        <WorkspaceIcon icon={detail.icon} color={detail.color} className="w-6 h-6 mt-1" />
        <div className="flex-1 min-w-0">
          <h2 className="m-0 text-[20px] font-semibold text-shodh-text truncate">
            {detail.name}
            {detail.archived && <span className="ml-2 align-middle text-[11px] font-medium px-1.5 py-0.5 rounded-full bg-shodh-raised text-shodh-text-muted">Archived</span>}
          </h2>
          {detail.description && <p className="m-0 mt-0.5 text-[13px] text-shodh-text-secondary">{detail.description}</p>}
          <p className="m-0 mt-1 text-[12px] text-shodh-text-muted">
            {sourceSummary(detail.sourceCounts)} · {chatIds.length} {chatIds.length === 1 ? 'chat' : 'chats'}
            {detail.instructionsVersion > 0 ? ' · has instructions' : ' · no instructions'}
          </p>
        </div>
        <button type="button" className={cn(PRIMARY_BUTTON, 'shrink-0')} onClick={() => onNewChat(detail.id)}>
          <MessageSquarePlus className="w-4 h-4" aria-hidden="true" /> New chat
        </button>
        <div ref={menuRef} className="relative shrink-0">
          <button
            ref={menuButtonRef}
            type="button"
            className={cn(QUIET_BUTTON, 'w-9 h-9 justify-center px-0')}
            aria-label="Workspace options"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen(o => !o)}
          >
            <MoreHorizontal className="w-4 h-4" aria-hidden="true" />
          </button>
          {menuOpen && (
            <div
              role="menu"
              aria-label="Workspace options"
              className="shell-pop absolute right-0 top-10 z-50 min-w-[180px] py-1 rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg"
              onKeyDown={e => {
                const items = Array.from(e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
                const index = items.indexOf(document.activeElement as HTMLButtonElement);
                if (e.key === 'Escape') {
                  e.preventDefault();
                  setMenuOpen(false);
                  menuButtonRef.current?.focus();
                } else if (e.key === 'ArrowDown') {
                  e.preventDefault();
                  items[(index + 1) % items.length]?.focus();
                } else if (e.key === 'ArrowUp') {
                  e.preventDefault();
                  items[(index - 1 + items.length) % items.length]?.focus();
                } else if (e.key === 'Tab') {
                  setMenuOpen(false);
                }
              }}
            >
              <button type="button" role="menuitem" className={cn(menuItem, 'text-shodh-text-secondary')} onClick={() => void setPinned()} autoFocus>
                {detail.pinned ? <PinOff className="w-3.5 h-3.5" aria-hidden="true" /> : <Pin className="w-3.5 h-3.5" aria-hidden="true" />}
                {detail.pinned ? 'Unpin' : 'Pin to top'}
              </button>
              <button type="button" role="menuitem" className={cn(menuItem, 'text-shodh-text-secondary')} onClick={() => void setArchived()}>
                {detail.archived ? <ArchiveRestore className="w-3.5 h-3.5" aria-hidden="true" /> : <Archive className="w-3.5 h-3.5" aria-hidden="true" />}
                {detail.archived ? 'Unarchive' : 'Archive'}
              </button>
              <div role="separator" className="my-1 border-t border-shodh-border" />
              <button type="button" role="menuitem" className={cn(menuItem, 'text-shodh-error')} onClick={() => void remove()}>
                <Trash2 className="w-3.5 h-3.5" aria-hidden="true" /> Delete…
              </button>
            </div>
          )}
        </div>
      </header>

      <div role="tablist" aria-label="Workspace sections" className="flex flex-wrap gap-1 border-b border-shodh-border-subtle">
        {WORKSPACE_TABS.map(t => (
          <button
            key={t}
            ref={el => { tabRefs.current[t] = el; }}
            type="button"
            role="tab"
            id={`ws-tab-${t}`}
            aria-selected={tab === t}
            aria-controls={`ws-panel-${t}`}
            tabIndex={tab === t ? 0 : -1}
            onClick={() => onTab(t)}
            onKeyDown={e => moveTab(e, t)}
            className={cn(
              '-mb-px h-9 px-3 text-[13px] border-b-2 transition-colors duration-micro',
              tab === t ? 'border-shodh-accent text-shodh-text font-semibold' : 'border-transparent text-shodh-text-muted hover:text-shodh-text',
              FOCUS_RING,
            )}
          >
            {WORKSPACE_TAB_LABELS[t]}
            {t === 'sources' && ` (${detail.sources.length})`}
            {t === 'chats' && ` (${chatIds.length})`}
          </button>
        ))}
      </div>

      <div role="tabpanel" id={`ws-panel-${tab}`} aria-labelledby={`ws-tab-${tab}`} tabIndex={0} className="focus:outline-none">
        {tab === 'overview' && <OverviewTab workspace={detail} onChanged={() => void load()} />}
        {tab === 'sources' && <SourcesTab workspace={detail} library={library} onChanged={() => void load()} />}
        {tab === 'chats' && <ChatsTab workspace={detail} onNewChat={() => onNewChat(detail.id)} onOpenChat={onOpenChat} />}
        {tab === 'memory' && (
          <MemorySettings
            scope={`workspace:${detail.id}`}
            conversationTitle={id => conversations.find(c => c.id === id)?.title}
            onOpenConversation={id => { switchConversation(id); onOpenChat(id); }}
            workspaceName={id => (id === detail.id ? detail.name : undefined)}
          />
        )}
        {tab === 'visuals' && (
          <VisualGallery
            conversationId={null}
            conversationIds={chatIds}
            emptyText="No visuals in this workspace’s chats yet. Diagrams, charts, plots, equations and tables from its answers appear here."
          />
        )}
        {tab === 'results' && <ResultsTab workspace={detail} />}
      </div>
    </div>
  );
}
