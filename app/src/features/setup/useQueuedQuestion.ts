import { useCallback, useEffect, useState } from 'react';
import type { MutableRefObject } from 'react';
import { useReadiness } from './useReadiness';
import { questionGate, queuedQuestionAction } from './readiness';
import type { QuestionGate } from './readiness';

export interface QueuedQuestion {
  /** A question of the conversation shown waits for setup. */
  waiting: boolean;
  gate: QuestionGate;
  /**
   * Call first when the user sends: true when setup is not ready, so the question
   * waits (its text stays in the composer, where it is saved as the draft) and
   * setup starts if it was not running; it is sent by `submitRef` once setup is ready.
   */
  hold: () => boolean;
  /** Don't send the waiting question (its text stays in the composer). */
  cancel: () => void;
}

/**
 * A question asked before Shodh is ready waits for setup instead of failing:
 * the composer keeps it (nothing is added to the conversation until it is
 * really sent, so a closed app never leaves a question without an answer) and
 * `submitRef.current()` sends it once the search models and the answer engine
 * are in place.
 */
export function useQueuedQuestion(
  conversationId: string | null,
  submitRef: MutableRefObject<() => void>,
): QueuedQuestion {
  const state = useReadiness();
  const gate = questionGate(state);
  const [queuedFor, setQueuedFor] = useState<string | null>(null);

  useEffect(() => {
    const action = queuedQuestionAction(queuedFor, conversationId, gate);
    if (action === 'drop') setQueuedFor(null);
    else if (action === 'send') {
      setQueuedFor(null);
      submitRef.current();
    }
  }, [queuedFor, conversationId, gate, submitRef]);

  const { startSetup } = state;
  const hold = useCallback((): boolean => {
    if (gate !== 'wait' || !conversationId) return false;
    setQueuedFor(conversationId);
    // Sending is the user asking for it: start (or retry) whatever is missing.
    startSetup();
    return true;
  }, [gate, conversationId, startSetup]);

  const cancel = useCallback(() => setQueuedFor(null), []);

  return { waiting: queuedFor !== null && queuedFor === conversationId, gate, hold, cancel };
}
