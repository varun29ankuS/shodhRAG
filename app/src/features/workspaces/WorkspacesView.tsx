import React, { useEffect, useMemo, useState } from 'react';
import { Archive, Layers, MessageSquare, Pin, Plus } from 'lucide-react';
import { cn } from '../../lib/utils';
import { relativeTime } from '../../utils/time';
import { useNavigationTarget } from '../agent/useNavigationTarget';
import { workspacesApi } from './api';
import { CreateWorkspaceDialog } from './CreateWorkspaceDialog';
import { orderTemplates, sortWorkspaces, sourceSummary, WORKSPACE_TABS } from './model';
import type { WorkspaceListing, WorkspaceTab, WorkspaceTemplate } from './types';
import { useWorkspaces } from './WorkspaceContext';
import { SHOW_ARCHIVED_WORKSPACES, peekArchivedRequest, takeArchivedRequest } from './archivedRequest';
import { WorkspaceIcon } from './WorkspaceIcon';
import { WorkspacePage } from './WorkspacePage';
import { lastWorkspacePlace, rememberWorkspacePlace } from './place';
import type { LibraryFolder } from './SourcesTab';
import { FOCUS_RING, PRIMARY_BUTTON, QUIET_BUTTON } from './ui';
import { SlackAnswersBadge, useSlackAnswerSource, type SlackAnswerSource } from '../connectors/SlackAnswersBadge';
import { PageFrame } from '../../components/shell/PageFrame';
import { ViewState } from '../../components/shell/ViewState';

function isTab(value: string | null): value is WorkspaceTab {
  return value !== null && (WORKSPACE_TABS as readonly string[]).includes(value);
}

function lastActive(w: WorkspaceListing): string | null {
  const times = [w.lastActiveAt, w.lastChatAt].filter((t): t is string => typeof t === 'string');
  if (times.length === 0) return null;
  return times.sort().pop() ?? null;
}

function WorkspaceCard({
  workspace,
  slack,
  onOpen,
}: {
  workspace: WorkspaceListing;
  slack: SlackAnswerSource | null;
  onOpen: () => void;
}) {
  const active = lastActive(workspace);
  return (
    <li>
      <button
        type="button"
        onClick={onOpen}
        className={cn(
          'w-full h-full flex flex-col gap-2 p-4 rounded-xl border border-shodh-border bg-shodh-surface text-start hover:bg-shodh-raised hover:border-shodh-border-strong transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <span className="flex items-center gap-2 min-w-0">
          <WorkspaceIcon icon={workspace.icon} color={workspace.color} className="w-4 h-4" />
          <span className="text-body font-semibold text-shodh-text truncate">{workspace.name}</span>
          <SlackAnswersBadge source={slack} workspaceId={workspace.id} />
          {workspace.pinned && <Pin className="w-3 h-3 ms-auto shrink-0 text-shodh-text-faint" aria-label="Pinned" />}
          {workspace.archived && <Archive className="w-3 h-3 ms-auto shrink-0 text-shodh-text-faint" aria-label="Archived" />}
        </span>
        <span className="text-small text-shodh-text-secondary line-clamp-2 min-h-[2.6em]">
          {workspace.description || 'No description'}
        </span>
        <span className="mt-auto flex flex-wrap items-center gap-x-3 gap-y-1 text-caption text-shodh-text-muted">
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
  const { workspaces, allWorkspaces, status, error, refresh, setOpenWorkspaceId, newWorkspacePending, takeNewWorkspaceRequest } = useWorkspaces();
  // Back where the user was when this view was last shown (a trip to Settings and back).
  const [selected, setSelected] = useState<string | null>(() => lastWorkspacePlace().workspaceId);
  const [tab, setTab] = useState<WorkspaceTab>(() => lastWorkspacePlace().tab);
  useEffect(() => {
    rememberWorkspacePlace({ workspaceId: selected, tab });
  }, [selected, tab]);
  const [creating, setCreating] = useState(false);
  const [createTemplate, setCreateTemplate] = useState<string | null>(null);
  // Also opened from the sidebar when every chat is in an archived workspace.
  // Read, not taken, during render: the first render may run again before it shows.
  const [showArchived, setShowArchived] = useState(() => peekArchivedRequest());
  useEffect(() => {
    // The request made before the view mounted (shown by the first render) is taken here.
    takeArchivedRequest();
    const show = () => {
      if (!takeArchivedRequest()) return;
      setSelected(null);
      setShowArchived(true);
    };
    window.addEventListener(SHOW_ARCHIVED_WORKSPACES, show);
    return () => window.removeEventListener(SHOW_ARCHIVED_WORKSPACES, show);
  }, []);
  const archived = useMemo(() => sortWorkspaces(allWorkspaces.filter(w => w.archived)), [allWorkspaces]);
  const slack = useSlackAnswerSource();
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

  const sorted = useMemo(() => sortWorkspaces(workspaces), [workspaces]);

  const startCreate = (template: string | null) => {
    setCreateTemplate(template);
    setCreating(true);
  };

  return (
    <>
      <PageFrame
        title="Workspaces"
        lede="Each keeps its own sources, instructions, memory and chats"
        heading={!selected}
        action={
          <button type="button" className={PRIMARY_BUTTON} onClick={() => startCreate(null)}>
            <Plus className="w-4 h-4" aria-hidden="true" /> New workspace
          </button>
        }
      >
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
            {status === 'error' ? (
              <ViewState kind="error" title="Workspaces could not be loaded" detail={error} />
            ) : status === 'loading' ? (
              <ViewState kind="loading" title="Loading workspaces" />
            ) : sorted.length === 0 ? (
              <section aria-labelledby="ws-empty" className="flex flex-col gap-3 rounded-xl border border-dashed border-shodh-border p-6">
                <Layers className="w-6 h-6 text-shodh-text-muted" aria-hidden="true" />
                <h3 id="ws-empty" className="m-0 text-body-lg font-semibold text-shodh-text">No workspaces yet</h3>
                <p className="m-0 text-small text-shodh-text-secondary">Chats here answer from its sources only.</p>
                <ul className="m-0 p-0 list-none grid grid-cols-1 sm:grid-cols-2 gap-2">
                  {templates.map(t => (
                    <li key={t.id}>
                      <button
                        type="button"
                        onClick={() => startCreate(t.id)}
                        className={cn('w-full flex items-start gap-2.5 p-3 rounded-xl border border-shodh-border bg-shodh-surface text-start hover:bg-shodh-raised transition-colors duration-micro', FOCUS_RING)}
                      >
                        <WorkspaceIcon icon={t.icon} color={t.color} className="w-4 h-4 mt-0.5" />
                        <span className="min-w-0">
                          <span className="block text-small font-semibold text-shodh-text">{t.id === 'blank' ? 'Blank workspace' : t.name}</span>
                          <span className="block text-caption text-shodh-text-muted">{t.description}</span>
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              </section>
            ) : (
              <ul aria-label="Workspaces" className="m-0 p-0 list-none grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
                {sorted.map(w => (
                  <WorkspaceCard key={w.id} workspace={w} slack={slack} onOpen={() => { setSelected(w.id); setTab('overview'); }} />
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
                  <p className="m-0 text-small text-shodh-text-muted">No archived workspaces.</p>
                ) : (
                  <ul aria-label="Archived workspaces" className="m-0 p-0 list-none grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
                    {archived.map(w => (
                      <WorkspaceCard key={w.id} workspace={w} slack={slack} onOpen={() => { setSelected(w.id); setTab('overview'); }} />
                    ))}
                  </ul>
                )
              )}
            </div>
          </>
        )}
      </PageFrame>
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
    </>
  );
}
