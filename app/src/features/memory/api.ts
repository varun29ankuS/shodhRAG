/**
 * Tauri commands of Settings → Memory (`memory_commands.rs`). Every call is the user's
 * own action: edits, pins and forgets are recorded in the audit log.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { parseMemories, parseMemory } from './model';
import type { MemoryContent, MemoryRecord } from './model';
import { parseLearnStatus, parseSuggestion, parseSuggestions } from './suggestions';
import type { LearnStatus, Suggestion, SuggestionStatus } from './suggestions';

export interface ListOptions {
  /** Include superseded and expired versions. */
  includeHistory?: boolean;
}

/** Every memory (current ones, and history on request). Listing does not count as use. */
export async function listMemories(options: ListOptions = {}): Promise<MemoryRecord[]> {
  const records = await invoke<unknown>('memory_list', {
    request: { includeHistory: options.includeHistory ?? false, limit: 5000 },
  });
  return parseMemories(records);
}

/** Every version of the fact a memory belongs to, oldest first. */
export async function memoryHistory(id: string): Promise<MemoryRecord[]> {
  return parseMemories(await invoke<unknown>('memory_history', { id }));
}

/** Replace a memory with an edited version; the old one is kept as history. */
export async function updateMemory(id: string, content: MemoryContent): Promise<MemoryRecord | null> {
  const outcome = await invoke<unknown>('memory_update', { id, content });
  const memory = typeof outcome === 'object' && outcome !== null ? (outcome as { memory?: unknown }).memory : null;
  return parseMemory(memory);
}

export async function setMemoryPinned(id: string, pinned: boolean): Promise<MemoryRecord | null> {
  return parseMemory(await invoke<unknown>('memory_set_pinned', { id, pinned }));
}

/** Forget a memory and all its versions. Returns the forgotten ids. */
export async function forgetMemory(id: string): Promise<string[]> {
  const ids = await invoke<unknown>('memory_forget', { id });
  return Array.isArray(ids) ? ids.filter((x): x is string => typeof x === 'string') : [];
}

/** Write every memory (with history) as JSON to `path`. */
export async function exportMemories(path: string): Promise<{ path: string; memories: number }> {
  return invoke<{ path: string; memories: number }>('memory_export', { path });
}

/** Text of a command error. */
export function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === 'string' ? error : 'Unknown error';
}

// -- Learning from conversations (`memory_learn.rs`) --

/** Emitted with `{ pending }` when suggestions change. */
export const SUGGESTIONS_CHANGED = 'memory-suggestions-changed';

export async function learnStatus(): Promise<LearnStatus | null> {
  return parseLearnStatus(await invoke<unknown>('memory_learn_status'));
}

/** Suggestions with these statuses (all when empty), newest first. */
export async function listSuggestions(statuses: SuggestionStatus[] = [], limit = 200): Promise<Suggestion[]> {
  return parseSuggestions(await invoke<unknown>('memory_suggestions_list', { statuses, limit }));
}

/** Accept a suggestion; `edit` replaces the memory with the user's version. */
export async function acceptSuggestion(id: string, edit: MemoryContent | null = null): Promise<Suggestion | null> {
  return parseSuggestion(await invoke<unknown>('memory_suggestion_accept', { id, edit }));
}

/** Accept several suggestions as they are. */
export async function acceptSuggestions(ids: string[]): Promise<{ id: string; ok: boolean; error: string | null }[]> {
  const results = await invoke<unknown>('memory_suggestions_accept_many', { ids });
  return Array.isArray(results)
    ? results.filter(
        (r): r is { id: string; ok: boolean; error: string | null } =>
          typeof r === 'object' && r !== null && typeof (r as { id?: unknown }).id === 'string',
      )
    : [];
}

export async function rejectSuggestion(id: string): Promise<Suggestion | null> {
  return parseSuggestion(await invoke<unknown>('memory_suggestion_reject', { id }));
}

export async function undoSuggestion(id: string): Promise<Suggestion | null> {
  return parseSuggestion(await invoke<unknown>('memory_suggestion_undo', { id }));
}

/** Stop learning now: mode off, queued turns dropped, waiting suggestions rejected. */
export async function stopLearning(): Promise<number> {
  const n = await invoke<unknown>('memory_learning_stop');
  return typeof n === 'number' ? n : 0;
}

export async function consolidateNow(): Promise<void> {
  await invoke<unknown>('memory_consolidate_now');
}

/** Listen for suggestion changes. Returns the unsubscribe function. */
export function onSuggestionsChanged(handler: (pending: number) => void): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  listen<unknown>(SUGGESTIONS_CHANGED, event => {
    const p = event.payload;
    if (typeof p === 'object' && p !== null && typeof (p as { pending?: unknown }).pending === 'number') {
      handler((p as { pending: number }).pending);
    }
  })
    .then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(err => console.error(`Failed to listen for ${SUGGESTIONS_CHANGED}:`, err));
  return () => {
    disposed = true;
    if (unlisten) unlisten();
  };
}
