import { useCallback, useEffect, useRef, useSyncExternalStore } from 'react';
import { useReadiness } from './useReadiness';
import { questionGate } from './readiness';
import type { QuestionGate } from './readiness';
import { questionQueue } from './questionQueue';
import type { QueuedSend } from './questionQueue';
import { notify } from '../../lib/notify';

export interface QueuedQuestion {
  /** A question of the conversation shown waits for setup. */
  waiting: boolean;
  gate: QuestionGate;
  /**
   * Call first when the user sends: true when setup is not ready, so the question waits
   * (its text stays in the composer, where it is saved as the draft) and setup starts if
   * it was not running. `send` sends it once setup is ready, from wherever the user is
   * then (see `useQueuedQuestionSender`); it must return whether the send was accepted.
   */
  hold: (send: QueuedSend) => boolean;
  /** Don't send the waiting question (its text stays in the composer). */
  cancel: () => void;
}

/**
 * A question asked before Shodh is ready waits for setup instead of failing: the
 * composer keeps it (nothing is added to the conversation until it is really sent, so
 * a closed app never leaves a question without an answer) and the app sends it once
 * the answer engine is in place.
 */
export function useQueuedQuestion(conversationId: string | null): QueuedQuestion {
  const state = useReadiness();
  const gate = questionGate(state);
  const subscribe = useCallback((listener: () => void) => questionQueue.subscribe(listener), []);
  const waiting = useSyncExternalStore(subscribe, () => questionQueue.isWaiting(conversationId));

  const { startSetup } = state;
  const hold = useCallback((send: QueuedSend): boolean => {
    if (gate !== 'wait' || !conversationId) return false;
    questionQueue.hold(conversationId, send);
    // Sending is the user asking for it: start (or retry) whatever is missing.
    startSetup();
    return true;
  }, [gate, conversationId, startSetup]);

  const cancel = useCallback(() => {
    if (conversationId) questionQueue.cancel(conversationId);
  }, [conversationId]);

  return { waiting, gate, hold, cancel };
}

/**
 * Sends a waiting question once setup lets it go, while its conversation is shown,
 * whichever view is open. Called once by the app, inside the chat session. A send
 * refused for now (an answer still running) stays queued and is tried again when the
 * running answer ends; the user is told once that it is waiting.
 */
export function useQueuedQuestionSender(activeConversationId: string | null, busy: boolean): void {
  const state = useReadiness();
  const gate = questionGate(state);
  const subscribe = useCallback((listener: () => void) => questionQueue.subscribe(listener), []);
  const waiting = useSyncExternalStore(subscribe, () => questionQueue.isWaiting(activeConversationId));
  const told = useRef(new Set<string>());

  useEffect(() => {
    if (gate !== 'send' || !waiting || busy || !activeConversationId) return;
    const result = questionQueue.flush(activeConversationId);
    if (result === 'refused' && !told.current.has(activeConversationId)) {
      told.current.add(activeConversationId);
      notify.info('Your question is waiting', {
        description: 'Shodh is ready, but your question could not be sent yet. It stays in the composer and is tried again when the current answer ends; “Don’t send it” keeps it from going out.',
      });
    }
    if (result === 'sent') told.current.delete(activeConversationId);
  }, [gate, waiting, busy, activeConversationId]);
}
