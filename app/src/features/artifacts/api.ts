/**
 * The creation commands (`creation_commands.rs`). Errors arrive as `{ message }`;
 * `errorText` normalises anything thrown.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { PREVIEW_EVENT, parsePreviewEvent } from './model';
import type { CodeChange, Creation, FileChange, FileContent, FileEntry, PreviewArea, PreviewEvent, PreviewInfo } from './model';

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
  /**
   * Show a preview of a version or of the folder now in its own window over `area` (the one
   * shown before closes); `network` only from the user's switch.
   */
  preview: (id: string, version: number | null, network: boolean, area: PreviewArea) =>
    invoke<PreviewInfo>('creation_preview', { id, version, network, area }),
  /** Keep the preview window over the preview area as the layout moves. */
  placePreview: (label: string, area: PreviewArea) => invoke<null>('creation_preview_place', { label, area }),
  /** Load the preview again (its files changed, or Reload). */
  reloadPreview: (label: string) => invoke<null>('creation_preview_reload', { label }),
  /** Close the preview window; its address stops working. */
  closePreview: (label: string) => invoke<null>('creation_preview_close', { label }),
  keep: (id: string) => invoke<Creation>('creation_keep', { id }),
  /** Writes a zip into Documents/Shodh exports; returns its path. */
  exportZip: (id: string, version: number | null) => invoke<string>('creation_export', { id, version }),
  remove: (id: string) => invoke<null>('creation_delete', { id }),
  /** Code mode's branches, newest first (read only). */
  codeChanges: () => invoke<CodeChange[]>('code_changes_list'),
};

/**
 * What Rust says about the preview window (it loaded, or it was stopped from leaving its
 * files). Only Rust sends this, to the main window; the preview itself can send nothing.
 */
export function onPreviewEvent(handler: (event: PreviewEvent) => void): Promise<UnlistenFn> {
  return listen<unknown>(PREVIEW_EVENT, event => {
    const parsed = parsePreviewEvent(event.payload);
    if (parsed) handler(parsed);
  });
}

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
