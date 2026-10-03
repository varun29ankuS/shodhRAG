/**
 * Tauri commands of Settings → Memory (`memory_commands.rs`). Every call is the user's
 * own action: edits, pins and forgets are recorded in the audit log.
 */
import { invoke } from '@tauri-apps/api/core';
import { parseMemories, parseMemory } from './model';
import type { MemoryContent, MemoryRecord } from './model';

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
