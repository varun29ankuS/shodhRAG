/**
 * The computer's commands the phone's screens call, typed. Each is on the
 * phone link's allowlist (crates/shodh-link/src/allowlist.rs); the computer
 * checks every call again and answers with its own handler.
 */
import type { Shim } from './invoke';

/** One conversation in the list (`conversations_list`: no messages). */
export interface ConversationSummary {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  pinned: boolean;
  workspaceId: string | null;
  mode: string | null;
  messageCount: number;
}

export interface SavedMessage {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: string;
  transcript?: unknown;
}

/** One conversation as the computer saved it (`load_conversation`). */
export interface SavedConversation {
  id: string;
  title: string;
  messages: SavedMessage[];
  updatedAt: string;
  workspaceId?: string | null;
}

export interface FolderEntry {
  name: string;
  path: string;
}

/** A Library folder: its child folders and the indexed files in it. */
export interface FolderListing {
  folders: FolderEntry[];
  files: FolderEntry[];
}

export type FileKind = 'pdf' | 'image' | 'table' | 'text' | 'unsupported';

export interface FileInfo {
  path: string;
  fileName: string;
  extension: string;
  sizeBytes: number;
  kind: FileKind;
}

export interface PageImage {
  page: number;
  pages: number;
  width: number;
  height: number;
  /** PNG, base64. */
  png: string;
}

export interface TextPart {
  text: string;
  part: number;
  parts: number;
  truncated: boolean;
}

export interface Sheet {
  name: string;
  headers: string[];
  rows: string[][];
  totalRows: number;
  truncated: boolean;
}

export interface WorkspaceListing {
  id: string;
  name: string;
  description: string;
  color: string;
  pinned: boolean;
  chatCount: number;
  lastActiveAt: string | null;
}

/** An approval waiting on the computer (`approvals_pending`). */
export interface PendingApproval {
  sessionId: string | null;
  stepId: string | null;
  conversationId: string | null;
  tool: string | null;
  title: string;
  requestedAt: string;
  /** Whether this phone may allow it (the computer's rule); otherwise it is approved on the computer. */
  phoneMay: boolean;
}

/** One earlier turn, replayed into a session the computer started afresh. */
export interface HistoryTurn {
  role: 'user' | 'assistant';
  content: string;
}

export const conversationsList = (shim: Shim) => shim.invoke<ConversationSummary[]>('conversations_list');

export const loadConversation = (shim: Shim, conversationId: string) =>
  shim.invoke<SavedConversation | null>('load_conversation', { conversationId });

/** The linked folders (no path), or one folder's children. */
export const libraryFolders = (shim: Shim, path: string | null) =>
  shim.invoke<Partial<FolderListing>>('library_folders', path === null ? {} : { path }).then(
    (listing): FolderListing => ({ folders: listing.folders ?? [], files: listing.files ?? [] }),
  );

export const fileInfo = (shim: Shim, filePath: string) => shim.invoke<FileInfo>('get_source_file_info', { filePath });

export const readPage = (shim: Shim, filePath: string, page: number, width: number) =>
  shim.invoke<PageImage>('read_source_page', { filePath, page, width });

export const readText = (shim: Shim, filePath: string, part: number) => shim.invoke<TextPart>('read_source_text', { filePath, part });

export const readTable = (shim: Shim, filePath: string) => shim.invoke<Sheet[]>('read_source_table', { filePath });

export const workspacesList = (shim: Shim) => shim.invoke<WorkspaceListing[]>('workspaces_list');

export const pendingApprovals = (shim: Shim) => shim.invoke<PendingApproval[]>('approvals_pending');

export const approve = (shim: Shim, sessionId: string, stepId: string, approved: boolean) =>
  shim.invoke<void>('agent_approve', { sessionId, stepId, approved });

export const abortAnswer = (shim: Shim, sessionId: string) => shim.invoke<void>('agent_abort', { sessionId });

/** The computer's events the screens listen to. */
export const AGENT_EVENT = 'agent_event';
export const CONVERSATION_CHANGED = 'conversation_changed';
export const INBOX_CHANGED = 'inbox-changed';
