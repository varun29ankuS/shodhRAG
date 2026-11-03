/**
 * The creation commands (`creation_commands.rs`). Errors arrive as `{ message }`;
 * `errorText` normalises anything thrown.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { Creation, FileChange, FileContent, FileEntry, PreviewInfo } from './model';

export function errorText(error: unknown): string {
  if (typeof error === 'object' && error !== null && typeof (error as { message?: unknown }).message === 'string') {
    return (error as { message: string }).message;
  }
  if (typeof error === 'string') return error;
  return error instanceof Error ? error.message : 'Something went wrong.';
}

export const creationsApi = {
  /** The conversation's creation, made with a fresh folder for its first request. */
  prepare: (conversationId: string, request: string) =>
    invoke<Creation>('creations_prepare', { conversationId, request }),
  list: (text: string | null = null) => invoke<Creation[]>('creations_list', { text }),
  get: (id: string) => invoke<Creation>('creation_get', { id }),
  forConversation: (conversationId: string) =>
    invoke<Creation | null>('creation_for_conversation', { conversationId }),
  /** A version's files, or the folder's as they are now (`version` null). */
  files: (id: string, version: number | null) => invoke<FileEntry[]>('creation_files', { id, version }),
  file: (id: string, version: number | null, path: string) =>
    invoke<FileContent>('creation_file', { id, version, path }),
  diff: (id: string, from: number, to: number) => invoke<FileChange[]>('creation_diff', { id, from, to }),
  /** A sandboxed preview of a version or of the folder now; `network` only from the user's switch. */
  preview: (id: string, version: number | null, network: boolean) =>
    invoke<PreviewInfo>('creation_preview', { id, version, network }),
  keep: (id: string) => invoke<Creation>('creation_keep', { id }),
  /** Writes a zip into Documents/Shodh exports; returns its path. */
  exportZip: (id: string, version: number | null) => invoke<string>('creation_export', { id, version }),
  remove: (id: string) => invoke<null>('creation_delete', { id }),
};

/** `creations-changed`: a creation of a conversation changed (made, saved, kept, deleted). */
export function onCreationsChanged(
  handler: (change: { conversationId: string | null; creationId: string | null }) => void,
): Promise<UnlistenFn> {
  return listen<{ conversationId?: unknown; creationId?: unknown }>('creations-changed', event => {
    const { conversationId, creationId } = event.payload ?? {};
    handler({
      conversationId: typeof conversationId === 'string' ? conversationId : null,
      creationId: typeof creationId === 'string' ? creationId : null,
    });
  });
}
