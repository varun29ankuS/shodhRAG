/**
 * What's new commands (`app/src-tauri/src/whats_new.rs`). Shapes are in `./types`.
 */

import { invoke } from '@tauri-apps/api/core';
import type { FolderWatchView, WhatsNewView } from './types';

/** Emitted when rows were found, seen or dismissed, or a folder's switch changed. */
export const WHATS_NEW_CHANGED_EVENT = 'whats-new-changed';

export const whatsNewApi = {
  list: () => invoke<WhatsNewView>('whats_new_list'),
  markSeen: (ids: string[]) => invoke<number>('whats_new_mark_seen', { ids }),
  dismiss: (id: string, restore: boolean) => invoke<boolean>('whats_new_dismiss', { id, restore }),
  setFolderWatch: (sourceId: string, on: boolean) =>
    invoke<FolderWatchView>('folder_watch_set', { sourceId, on }),
};
