import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { publishTarget } from '../agent/navigation';
import { onWorkspacesChanged, workspaceError, workspacesApi } from './api';
import type { WorkspaceListing, WorkspaceTab } from './types';

interface WorkspaceContextValue {
  /** Workspaces that are not archived, as listed (pinned, then most recent). */
  workspaces: WorkspaceListing[];
  status: 'loading' | 'ready' | 'error';
  error: string | null;
  refresh: () => void;
  byId: (id: string | null | undefined) => WorkspaceListing | null;
  /** The workspace whose page is showing (null on the list or another view). */
  openWorkspaceId: string | null;
  setOpenWorkspaceId: (id: string | null) => void;
  /** Show a workspace's page (switching to the Workspaces view). */
  openWorkspace: (id: string, tab?: WorkspaceTab) => void;
  /** Something asked for the "new workspace" dialog and the Workspaces view has not shown it yet. */
  newWorkspacePending: boolean;
  /** Open the Workspaces view with the "new workspace" dialog. */
  requestNewWorkspace: () => void;
  /** The Workspaces view showed the dialog. */
  takeNewWorkspaceRequest: () => void;
}

const WorkspaceContext = createContext<WorkspaceContextValue | null>(null);

/** Workspaces for the shell: the list (refreshed on `workspaces-changed`) and navigation. */
export function WorkspaceProvider({ children }: { children: React.ReactNode }) {
  const [workspaces, setWorkspaces] = useState<WorkspaceListing[]>([]);
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading');
  const [error, setError] = useState<string | null>(null);
  const [openWorkspaceId, setOpenWorkspaceId] = useState<string | null>(null);
  const [newWorkspacePending, setNewWorkspacePending] = useState(false);
  const generation = useRef(0);

  const refresh = useCallback(() => {
    const run = ++generation.current;
    workspacesApi
      .list(false)
      .then(list => {
        if (run !== generation.current) return;
        setWorkspaces(list);
        setStatus('ready');
        setError(null);
      })
      .catch(err => {
        if (run !== generation.current) return;
        setStatus('error');
        setError(workspaceError(err).message);
      });
  }, []);

  useEffect(() => {
    refresh();
    return onWorkspacesChanged(() => refresh());
  }, [refresh]);

  const byId = useCallback(
    (id: string | null | undefined) => (id ? workspaces.find(w => w.id === id) ?? null : null),
    [workspaces],
  );

  const openWorkspace = useCallback((id: string, tab?: WorkspaceTab) => {
    publishTarget({ kind: 'workspace', workspaceId: id, tab: tab ?? null });
    window.dispatchEvent(new CustomEvent('switchTab', { detail: 'workspaces' }));
  }, []);

  const requestNewWorkspace = useCallback(() => {
    setNewWorkspacePending(true);
    window.dispatchEvent(new CustomEvent('switchTab', { detail: 'workspaces' }));
  }, []);
  const takeNewWorkspaceRequest = useCallback(() => setNewWorkspacePending(false), []);

  const value = useMemo<WorkspaceContextValue>(
    () => ({
      workspaces,
      status,
      error,
      refresh,
      byId,
      openWorkspaceId,
      setOpenWorkspaceId,
      openWorkspace,
      newWorkspacePending,
      requestNewWorkspace,
      takeNewWorkspaceRequest,
    }),
    [workspaces, status, error, refresh, byId, openWorkspaceId, openWorkspace, newWorkspacePending, requestNewWorkspace, takeNewWorkspaceRequest],
  );
  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspaces(): WorkspaceContextValue {
  const value = useContext(WorkspaceContext);
  if (!value) throw new Error('useWorkspaces must be used inside WorkspaceProvider');
  return value;
}
