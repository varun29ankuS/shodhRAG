/**
 * "Search all my library" for the next question of a workspace chat: shown and changed in
 * the top bar's Sources control, read by Ask when it sends. It holds for one conversation
 * and one question: switching conversations or sending turns it off.
 */
import { useCallback, useSyncExternalStore } from 'react';

let state: { conversationId: string | null; on: boolean } = { conversationId: null, on: false };
const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Whether "Search all my library" is on for `conversationId`'s next question. */
export function searchAllOn(conversationId: string | null): boolean {
  return conversationId !== null && state.conversationId === conversationId && state.on;
}

/** Turn it on or off for `conversationId` (off for every other conversation). */
export function setSearchAll(conversationId: string | null, on: boolean): void {
  if (state.conversationId === conversationId && state.on === on) return;
  state = { conversationId, on: on && conversationId !== null };
  listeners.forEach(listener => listener());
}

export function useSearchAll(conversationId: string | null): [boolean, (on: boolean) => void] {
  const on = useSyncExternalStore(subscribe, () => searchAllOn(conversationId));
  const set = useCallback((next: boolean) => setSearchAll(conversationId, next), [conversationId]);
  return [on, set];
}
