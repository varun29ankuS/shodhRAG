import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { DesignCheckUpdate, StoredDesignCheck } from './designModel';
import type { FileChange } from './diffModel';
import type { ReviewRequest, StoredReview } from './reviewModel';

/** A file of a workspace's code folder (`code_file_read`). */
export interface CodeFile {
  path: string;
  language: string;
  text: string;
}

/** The model a review is sent to, and whether it is off this computer (`code_review_target`). */
export interface ReviewTarget {
  model: string;
  cloud: boolean;
}

/** The code review pane's backend (`code_review_commands.rs`). */
export const codeApi = {
  /** Run the fresh-eyes reviewer on exactly `request`. */
  review: (request: ReviewRequest, workspaceId: string | null, conversationId: string | null) =>
    invoke<StoredReview>('code_review_run', { request, workspaceId, conversationId }),
  /** A review the pane or the assistant ran (by id). */
  get: (id: string) => invoke<StoredReview>('code_review_get', { id }),
  /** A file of the workspace's code folder. */
  file: (workspaceId: string, path: string) => invoke<CodeFile>('code_file_read', { workspaceId, path }),
  /** The code changes of the workspace's code folder (the conversation's branch, else the last commit). */
  diff: (workspaceId: string, conversationId: string | null, path: string | null) =>
    invoke<FileChange[]>('code_diff', { workspaceId, conversationId, path }),
  /** The model a review would be sent to now (null: none set up). */
  target: () => invoke<ReviewTarget | null>('code_review_target'),
};

/** Which files a design check reads: a creation, or a file or folder of a project (`.` for all of it). */
export interface DesignCheckRequest {
  workspaceId?: string | null;
  conversationId?: string | null;
  creationId?: string | null;
  path?: string | null;
  /** A Code conversation's changed UI files instead of `path`. */
  changes?: boolean;
}

/** The design check's backend (`design_check_commands.rs`): deterministic, no model. */
export const designApi = {
  run: (request: DesignCheckRequest) =>
    invoke<StoredDesignCheck>('design_check_run', {
      workspaceId: request.workspaceId ?? null,
      conversationId: request.conversationId ?? null,
      creationId: request.creationId ?? null,
      path: request.path ?? null,
      changes: request.changes ?? null,
    }),
  /** A check the pane or the assistant ran (by id). */
  get: (id: string) => invoke<StoredDesignCheck>('design_check_get', { id }),
  /** The latest check of a creation, else of a conversation (its badge). */
  latest: (conversationId: string | null, creationId: string | null) =>
    invoke<StoredDesignCheck | null>('design_check_latest', { conversationId, creationId }),
};

/** `design-check-updated`: a check finished (on demand, by the assistant, or after the agent wrote files). */
export function onDesignCheckUpdated(handler: (update: DesignCheckUpdate) => void): Promise<UnlistenFn> {
  return listen<Partial<DesignCheckUpdate>>('design-check-updated', event => {
    const p = event.payload ?? {};
    if (typeof p.id !== 'string') return;
    handler({
      id: p.id,
      conversationId: typeof p.conversationId === 'string' ? p.conversationId : null,
      creationId: typeof p.creationId === 'string' ? p.creationId : null,
      workspaceId: typeof p.workspaceId === 'string' ? p.workspaceId : null,
      total: typeof p.total === 'number' ? p.total : 0,
    });
  });
}
