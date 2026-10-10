/**
 * Tauri commands of the Workspaces view (`workspace_commands.rs`). Every call is the
 * user's own action; changes are broadcast as `workspaces-changed`.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  AddReport,
  InstructionVersion,
  NewSource,
  NewWorkspace,
  SourceHealth,
  SourceKind,
  Workspace,
  WorkspaceDetail,
  WorkspaceListing,
  WorkspacePatch,
  WorkspaceTemplate,
} from './types';

/** Emitted with `{ workspaceId }` (null when several changed). */
export const WORKSPACES_CHANGED = 'workspaces-changed';

export const workspacesApi = {
  list: (includeArchived = false) => invoke<WorkspaceListing[]>('workspaces_list', { includeArchived }),
  templates: () => invoke<WorkspaceTemplate[]>('workspaces_templates'),
  get: (id: string) => invoke<WorkspaceDetail>('workspaces_get', { id }),
  create: (workspace: NewWorkspace) => invoke<Workspace>('workspaces_create', { workspace }),
  update: (id: string, patch: WorkspacePatch) => invoke<Workspace>('workspaces_update', { id, patch }),
  setArchived: (id: string, archived: boolean) => invoke<Workspace>('workspaces_set_archived', { id, archived }),
  setInstructions: (id: string, text: string, expectedVersion: number, note: string | null = null) =>
    invoke<InstructionVersion>('workspaces_set_instructions', { id, text, expectedVersion, note }),
  history: (id: string) => invoke<InstructionVersion[]>('workspaces_instruction_history', { id }),
  addSources: (id: string, sources: NewSource[]) => invoke<AddReport>('workspaces_add_sources', { id, sources }),
  removeSource: (id: string, kind: SourceKind, reference: string) =>
    invoke<boolean>('workspaces_remove_source', { id, kind, reference }),
  remove: (id: string) => invoke<boolean>('workspaces_delete', { id }),
  health: (id: string) => invoke<SourceHealth>('workspaces_source_health', { id }),
};

/** Error text of a workspace command (`{ code, message }` or a string). */
export function workspaceError(error: unknown): { code: string; message: string } {
  if (typeof error === 'object' && error !== null) {
    const e = error as { code?: unknown; message?: unknown };
    if (typeof e.message === 'string') return { code: typeof e.code === 'string' ? e.code : 'unknown', message: e.message };
  }
  if (typeof error === 'string') return { code: 'unknown', message: error };
  return { code: 'unknown', message: error instanceof Error ? error.message : 'Workspaces could not be reached.' };
}

/** Listen for workspace changes. Returns the unsubscribe function. */
export function onWorkspacesChanged(handler: (workspaceId: string | null) => void): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  listen<{ workspaceId?: unknown }>(WORKSPACES_CHANGED, event => {
    const id = event.payload?.workspaceId;
    handler(typeof id === 'string' ? id : null);
  })
    .then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(err => console.error(`Failed to listen for ${WORKSPACES_CHANGED}:`, err));
  return () => {
    disposed = true;
    if (unlisten) unlisten();
  };
}
