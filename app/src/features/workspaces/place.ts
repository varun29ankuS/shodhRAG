/**
 * Where the user was in Workspaces: the open workspace, its tab, and the evidence table
 * open in each workspace. The Workspaces view unmounts when another view is shown, so this
 * is kept at module level for the app's run: "Open Settings → Model" from a blocked fill,
 * and coming back, lands on the same table. Pure (type-only imports), tested in
 * `app/tests/workspacePlace.test.ts`.
 */

import type { WorkspaceTab } from './types';

export interface WorkspacePlace {
  workspaceId: string | null;
  tab: WorkspaceTab;
}

const START: WorkspacePlace = { workspaceId: null, tab: 'overview' };

let place: WorkspacePlace = START;
const openTables = new Map<string, string>();

/** The place to show when the Workspaces view mounts. */
export function lastWorkspacePlace(): WorkspacePlace {
  return place;
}

/** Remember the open workspace and tab (no workspace: the list). */
export function rememberWorkspacePlace(next: WorkspacePlace): void {
  place = { workspaceId: next.workspaceId, tab: next.workspaceId ? next.tab : 'overview' };
}

/** The table open in a workspace's Tables tab, if any. */
export function openTableOf(workspaceId: string): string | null {
  return openTables.get(workspaceId) ?? null;
}

/** Remember the table open in a workspace's Tables tab (null: its list). */
export function rememberOpenTable(workspaceId: string, tableId: string | null): void {
  if (tableId) openTables.set(workspaceId, tableId);
  else openTables.delete(workspaceId);
}

/** Forget a workspace (deleted) or a table (deleted) wherever it is remembered. */
export function forgetPlace(ids: { workspaceId?: string; tableId?: string }): void {
  if (ids.workspaceId) {
    openTables.delete(ids.workspaceId);
    if (place.workspaceId === ids.workspaceId) place = START;
  }
  if (ids.tableId) {
    for (const [workspace, table] of openTables) if (table === ids.tableId) openTables.delete(workspace);
  }
}
