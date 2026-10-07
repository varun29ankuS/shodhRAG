/**
 * Shapes of the workspace commands (`workspace_commands.rs`). Field names follow the
 * backend's camelCase serialisation.
 */

export type SourceKind = 'folder' | 'file' | 'snippet' | 'paper';

export type WorkspaceAuthor = 'user' | 'agent' | 'template' | 'migration';

export interface SourceCounts {
  folders: number;
  files: number;
  snippets: number;
  papers: number;
}

export interface Workspace {
  id: string;
  name: string;
  description: string;
  /** One of `WORKSPACE_ICONS`. */
  icon: string;
  /** One of `WORKSPACE_COLORS`. */
  color: string;
  /** Template id (`blank` for none). */
  template: string;
  pinned: boolean;
  archived: boolean;
  createdAt: string;
  updatedAt: string;
  lastActiveAt: string | null;
  /** Current instruction version; 0 when it never had instructions. */
  instructionsVersion: number;
  sourceCounts: SourceCounts;
}

/** A workspace as the list shows it, with its chats. */
export interface WorkspaceListing extends Workspace {
  chatCount: number;
  lastChatAt: string | null;
}

export interface WorkspaceSource {
  kind: SourceKind;
  /** Source id (folder), file path (file), snippet id or paper id. */
  ref: string;
  label: string;
  /** Folder path, file path, the snippet's file, or a library paper's PDF. */
  path: string | null;
  addedBy: WorkspaceAuthor;
  addedAt: string;
}

export interface WorkspaceDetail extends Workspace {
  instructions: string;
  sources: WorkspaceSource[];
}

export interface InstructionVersion {
  version: number;
  text: string;
  author: WorkspaceAuthor;
  note: string | null;
  createdAt: string;
}

export interface WorkspaceTemplate {
  id: string;
  name: string;
  description: string;
  icon: string;
  color: string;
  instructions: string;
}

export type SourceState = 'indexed' | 'not_indexed' | 'missing' | 'reference' | 'unknown';

export interface SourceStatus {
  kind: SourceKind;
  ref: string;
  state: SourceState;
  files: number | null;
  chunks: number | null;
}

export interface SourceHealth {
  sources: SourceStatus[];
  searchableFiles: number;
  chunks: number;
  problems: number;
}

export interface NewSource {
  kind: SourceKind;
  ref: string;
  label: string;
  path?: string | null;
}

export interface NewWorkspace {
  name: string;
  description: string;
  template: string | null;
  instructions?: string | null;
}

export interface WorkspacePatch {
  name?: string;
  description?: string;
  icon?: string;
  color?: string;
  pinned?: boolean;
}

export interface AddReport {
  added: number;
  already: number;
}

/** Tabs of a workspace page. */
export type WorkspaceTab = 'overview' | 'sources' | 'chats' | 'memory' | 'visuals' | 'results';
