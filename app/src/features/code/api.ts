import { invoke } from '@tauri-apps/api/core';
import type { FileChange } from './diffModel';
import type { ReviewRequest, StoredReview } from './reviewModel';

/** A file of a workspace's code folder (`code_file_read`). */
export interface CodeFile {
  path: string;
  language: string;
  text: string;
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
};
