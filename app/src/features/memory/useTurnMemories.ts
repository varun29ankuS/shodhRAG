/**
 * The memory lines of a conversation's turns (./turnMemories), read live from the
 * suggestions and kept current: on every `memory-suggestions-changed` about these turns
 * (a request saved, a turn learned from) and on every change that names no turns (an
 * Undo, an Accept or a forget anywhere), so the chat never shows a memory as saved after
 * it was undone or forgotten.
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { onSuggestionsChanged, suggestionsForTurns } from './api';
import { concernsTurns, groupByTurn, withStableTurns } from './turnMemories';
import type { TurnMemories } from './turnMemories';

const EMPTY: ReadonlyMap<string, TurnMemories> = new Map();

/** Lines by run id for `turnIds` (./turnMemories `turnIdsOf`) of `conversationId`. */
export function useTurnMemories(conversationId: string | null, turnIds: readonly string[]): ReadonlyMap<string, TurnMemories> {
  const [turns, setTurns] = useState<{ conversationId: string | null; byTurn: ReadonlyMap<string, TurnMemories> }>({
    conversationId: null,
    byTurn: EMPTY,
  });
  // Stands for `turnIds`' content, so a new array of the same ids reads nothing again.
  const key = turnIds.join('\n');
  const shownRef = useRef<readonly string[]>(turnIds);
  shownRef.current = turnIds;
  // Only the newest read is applied.
  const seqRef = useRef(0);

  const load = useCallback(async () => {
    const seq = ++seqRef.current;
    if (!conversationId || shownRef.current.length === 0) {
      setTurns({ conversationId, byTurn: EMPTY });
      return;
    }
    try {
      const suggestions = await suggestionsForTurns(conversationId, [...shownRef.current]);
      if (seq !== seqRef.current) return;
      setTurns(prev => ({
        conversationId,
        byTurn: withStableTurns(prev.conversationId === conversationId ? prev.byTurn : EMPTY, groupByTurn(suggestions)),
      }));
    } catch {
      // Memory is unavailable (for example before the search models are installed): the
      // chat shows no memory lines rather than stale ones.
      if (seq !== seqRef.current) return;
      setTurns({ conversationId, byTurn: EMPTY });
    }
    // `key` stands for the turns read (`shownRef`).
  }, [conversationId, key]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(
    () =>
      onSuggestionsChanged((_pending, changed) => {
        if (conversationId && concernsTurns(changed, conversationId, shownRef.current)) void load();
      }),
    [conversationId, load],
  );

  return turns.conversationId === conversationId ? turns.byTurn : EMPTY;
}
