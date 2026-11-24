/**
 * What's new commands (`app/src-tauri/src/whats_new.rs`). Shapes are in `./types`.
 */

import { invoke } from '@tauri-apps/api/core';
import type { AlertView, FolderWatchView, TopicSuggestion, WhatsNewView } from './types';

/** Emitted when rows were found, seen or dismissed, or a folder's switch changed. */
export const WHATS_NEW_CHANGED_EVENT = 'whats-new-changed';

export const whatsNewApi = {
  list: () => invoke<WhatsNewView>('whats_new_list'),
  markSeen: (ids: string[]) => invoke<number>('whats_new_mark_seen', { ids }),
  dismiss: (id: string, restore: boolean) => invoke<boolean>('whats_new_dismiss', { id, restore }),
  setFolderWatch: (sourceId: string, on: boolean) =>
    invoke<FolderWatchView>('folder_watch_set', { sourceId, on }),
  createAlert: (words: string) => invoke<AlertView>('paper_alert_create', { words }),
  updateAlert: (id: string, change: { words?: string; enabled?: boolean }) =>
    invoke<AlertView>('paper_alert_update', { id, words: change.words ?? null, enabled: change.enabled ?? null }),
  deleteAlert: (id: string) => invoke<boolean>('paper_alert_delete', { id }),
  suggestions: () => invoke<TopicSuggestion[]>('paper_alert_suggestions'),
  addToLibrary: (itemId: string, sourceId: string) =>
    invoke<string>('whats_new_add_to_library', { itemId, sourceId }),
};
