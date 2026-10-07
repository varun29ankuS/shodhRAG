import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Archive, Layers, MessageSquare, Pin, Plus } from 'lucide-react';
import { cn } from '../../lib/utils';
import { relativeTime } from '../../utils/time';
import { useNavigationTarget } from '../agent/useNavigationTarget';
import { workspaceError, workspacesApi } from './api';
import { CreateWorkspaceDialog } from './CreateWorkspaceDialog';
import { orderTemplates, sortWorkspaces, sourceSummary, WORKSPACE_TABS } from './model';
import type { WorkspaceListing, WorkspaceTab, WorkspaceTemplate } from './types';
import { useWorkspaces } from './WorkspaceContext';
import { WorkspaceIcon } from './WorkspaceIcon';
import { WorkspacePage } from './WorkspacePage';
import type { LibraryFolder } from './SourcesTab';
import { FOCUS_RING, PRIMARY_BUTTON, QUIET_BUTTON } from './ui';

function isTab(value: string | null): value is WorkspaceTab {
  return value !== null && (WORKSPACE_TABS as readonly string[]).includes(value);
}

function lastActive(w: WorkspaceListing): string | null {
  const times = [w.lastActiveAt, w.lastChatAt].filter((t): t is string => typeof t === 'string');
  if (times.length === 0) return null;
  return times.sort().pop() ?? null;
}

function WorkspaceCard({ workspace, onOpen }: { workspace: WorkspaceListing; onOpen: () => void }) {
  const active = lastActive(workspace);
  return (
    <li>
      <button
        type="button"
        onClick={onOpen}
        className={cn(
          'w-full h-full flex flex-col gap-2 p-4 rounded-xl border border-shodh-border bg-shodh-surface text-left hover:bg-shodh-raised hover:border-shodh-border-strong transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <span className="flex items-center gap-2 min-w-0">
          <WorkspaceIcon icon={workspace.icon} color={workspace.color} className="w-4 h-4" />
          <span className="text-[14px] font-semibold text-shodh-text truncate">{workspace.name}</span>
          {workspace.pinned && <Pin className="w-3 h-3 ml-auto shrink-0 text-shodh-text-faint" aria-label="Pinned" />}
          {workspace.archived && <Archive className="w-3 h-3 ml-auto shrink-0 text-shodh-text-faint" aria-label="Archived" />}
        </span>
        <span className="text-[12.5px] text-shodh-text-secondary line-clamp-2 min-h-[2.6em]">
          {workspace.description || 'No description'}
        </span>
        <span className="mt-auto flex flex-wrap items-center gap-x-3 gap-y-1 text-[11.5px] text-shodh-text-muted">
          <span>{sourceSummary(workspace.sourceCounts)}</span>
          <span className="inline-flex items-center gap-1">
            <MessageSquare className="w-3 h-3" aria-hidden="true" />
            {workspace.chatCount} {workspace.chatCount === 1 ? 'chat' : 'chats'}
          </span>
          <span>{active ? `Active ${relativeTime(active)}` : `Created ${relativeTime(workspace.createdAt)}`}</span>
        </span>
      </button>
    </li>
  );
}

/**
 * Workspaces: the list (cards with sources, chats and last activity; new from a
 * template) and one workspace's page. Agent navigation (`open_workspace`) and the
 * sidebar open a page through the `workspace` target.
 */
export default function WorkspacesView({
  library,
  onNewChat,
  onOpenChat,
}: {
  library: LibraryFolder[];
  onNewChat: (workspaceId: string) => void;
  onOpenChat: (conversationId: string) => void;
}) {
  const { workspaces, status, error, refresh, setOpenWorkspaceId, newWorkspacePending, takeNewWorkspaceRequest } = useWorkspaces();
  const [selected, setSelected] = useState<string | null>(null);
  const [tab, setTab] = useState<WorkspaceTab>('overview');
  const [creating, setCreating] = useState(false);
  const [createTemplate, setCreateTemplate] = useState<string | null>(null);
  const [showArchived, setShowArchived] = useState(false);
  const [archived, setArchived] = useState<WorkspaceListing[]>([]);
  const [templates, setTemplates] = useState<WorkspaceTemplate[]>([]);

  useNavigationTarget('workspace', target => {
    setSelected(target.workspaceId);
    setTab(isTab(target.tab) ? target.tab : 'overview');
  });

  useEffect(() => {
    setOpenWorkspaceId(selected);
  }, [selected, setOpenWorkspaceId]);

  // "New workspace" from the palette (also when it was asked for before this view mounted).
  useEffect(() => {
    if (!newWorkspacePending) return;
    takeNewWorkspaceRequest();
    setSelected(null);
    setCreateTemplate(null);
    setCreating(true);
  }, [newWorkspacePending, takeNewWorkspaceRequest]);
  useEffect(() => () => setOpenWorkspaceId(null), [setOpenWorkspaceId]);

  useEffect(() => {
    let cancelled = false;
    workspacesApi.templates().then(list => { if (!cancelled) setTemplates(orderTemplates(list)); }).catch(() => undefined);
    return () => { cancelled = true; };
  }, []);

  const loadArchived = useCallback(async () => {
    try {
      const all = await workspacesApi.list(true);
      setArchived(all.filter(w => w.archived));
    } catch (err) {
      setArchived([]);
      console.warn('Archived workspaces could not be listed:', workspaceError(err).message);
    }
  }, []);

  useEffect(() => {
    if (showArchived) void loadArchived();
  }, [showArchived, loadArchived, workspaces]);

  const sorted = useMemo(() => sortWorkspaces(workspaces), [workspaces]);

  const startCreate = (template: string | null) => {
    setCreateTemplate(template);
    setCreating(true);
  };

  return (
    <div className="h-full overflow-y-auto scrollbar-thin bg-shodh-ground">
      <div className="max-w-5xl mx-auto px-8 py-7 flex flex-col gap-5">
        {selected ? (
          <WorkspacePage
            key={selected}
            workspaceId={selected}
            tab={tab}
            onTab={setTab}
            onBack={() => {
              setSelected(null);
              refresh();
            }}
            library={library}
            onNewChat={onNewChat}
            onOpenChat={onOpenChat}
          />
        ) : (
          <>
            <header className="flex items-start gap-3">
              <div className="flex-1 min-w-0">
                <h2 className="m-0 text-[20px] font-semibold text-shodh-text">Workspaces</h2>
                <p className="m-0 mt-1 text-[13px] text-shodh-text-secondary max-w-[640px]">
                  A workspace keeps one piece of work together: the sources its chats search (and nothing else), the
                  instructions every answer follows, what Shodh remembers about it, and its chats.
                </p>
              </div>
              <button type="button" className={PRIMARY_BUTTON} onClick={() => startCreate(null)}>
                <Plus className="w-4 h-4" aria-hidden="true" /> New workspace
              </button>
            </header>

            {status === 'error' ? (
              <p role="alert" className="m-0 text-[13px] text-shodh-error">Workspaces could not be loaded: {error}</p>
            ) : status === 'loading' ? (
              <p role="status" className="m-0 text-[13px] text-shodh-text-muted">Loading workspaces…</p>
            ) : sorted.length === 0 ? (
              <section aria-labelledby="ws-empty" className="flex flex-col gap-3 rounded-xl border border-dashed border-shodh-border p-6">
                <Layers className="w-6 h-6 text-shodh-text-muted" aria-hidden="true" />
                <h3 id="ws-empty" className="m-0 text-[15px] font-semibold text-shodh-text">No workspaces yet</h3>
                <p className="m-0 text-[13px] text-shodh-text-secondary max-w-[600px]">
                  Start one for a literature review, a grant, a paper or a client. Add its folders, files, snippets and
                  papers; chats in it answer only from them, following the workspace’s instructions.
                </p>
                <ul className="m-0 p-0 list-none grid grid-cols-1 sm:grid-cols-2 gap-2">
                  {templates.map(t => (
                    <li key={t.id}>
                      <button
                        type="button"
                        onClick={() => startCreate(t.id)}
                        className={cn('w-full flex items-start gap-2.5 p-3 rounded-xl border border-shodh-border bg-shodh-surface text-left hover:bg-shodh-raised transition-colors duration-micro', FOCUS_RING)}
                      >
                        <WorkspaceIcon icon={t.icon} color={t.color} className="w-4 h-4 mt-0.5" />
                        <span className="min-w-0">
                          <span className="block text-[13px] font-semibold text-shodh-text">{t.id === 'blank' ? 'Blank workspace' : t.name}</span>
                          <span className="block text-[12px] text-shodh-text-muted">{t.description}</span>
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              </section>
            ) : (
              <ul aria-label="Workspaces" className="m-0 p-0 list-none grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
                {sorted.map(w => (
                  <WorkspaceCard key={w.id} workspace={w} onOpen={() => { setSelected(w.id); setTab('overview'); }} />
                ))}
              </ul>
            )}

            <div className="flex flex-col gap-3">
              <button
                type="button"
                className={cn(QUIET_BUTTON, 'self-start')}
                aria-expanded={showArchived}
                onClick={() => setShowArchived(s => !s)}
              >
                <Archive className="w-3.5 h-3.5" aria-hidden="true" />
                {showArchived ? 'Hide archived workspaces' : 'Show archived workspaces'}
              </button>
              {showArchived && (
                archived.length === 0 ? (
                  <p className="m-0 text-[12.5px] text-shodh-text-muted">No archived workspaces.</p>
                ) : (
                  <ul aria-label="Archived workspaces" className="m-0 p-0 list-none grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
                    {archived.map(w => (
                      <WorkspaceCard key={w.id} workspace={w} onOpen={() => { setSelected(w.id); setTab('overview'); }} />
                    ))}
                  </ul>
                )
              )}
            </div>
          </>
        )}
      </div>
      <CreateWorkspaceDialog
        open={creating}
        onOpenChange={setCreating}
        initialTemplate={createTemplate}
        onCreated={created => {
          refresh();
          setSelected(created.id);
          setTab('sources');
        }}
      />
    </div>
  );
}
