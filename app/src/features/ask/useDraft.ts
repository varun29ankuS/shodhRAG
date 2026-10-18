import { useCallback, useSyncExternalStore } from 'react';
import { composerDrafts } from './draftStore';

export type DraftChange = string | ((current: string) => string);

/** The shared draft of `conversationId` (see `draftStore`) and its setter. */
export function useDraft(conversationId: string | null): [string, (change: DraftChange) => void] {
  const subscribe = useCallback((listener: () => void) => composerDrafts.subscribe(listener), []);
  const draft = useSyncExternalStore(subscribe, () => composerDrafts.get(conversationId));
  const setDraft = useCallback((change: DraftChange) => {
    if (typeof change === 'string') composerDrafts.set(conversationId, change);
    else composerDrafts.update(conversationId, change);
  }, [conversationId]);
  return [draft, setDraft];
}
