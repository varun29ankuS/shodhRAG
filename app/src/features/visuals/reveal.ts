/**
 * "Go to message": the Ask view scrolls to an answer when it shows the
 * conversation it belongs to. The request waits in a slot because the
 * conversation may still be loading (or the Ask view mounting) when it is
 * made.
 */

export interface MessageReveal {
  conversationId: string;
  messageId: string;
  /** Increases with every request, so repeats are distinguishable. */
  seq: number;
}

type Listener = (reveal: MessageReveal) => void;

let pending: MessageReveal | null = null;
let seq = 0;
const listeners = new Set<Listener>();

export function requestReveal(conversationId: string, messageId: string): void {
  seq += 1;
  pending = { conversationId, messageId, seq };
  for (const listener of listeners) listener(pending);
}

export function pendingReveal(): MessageReveal | null {
  return pending;
}

/** Clears the request once it was shown (only the request that was shown). */
export function clearReveal(done: MessageReveal): void {
  if (pending && pending.seq === done.seq) pending = null;
}

export function subscribeReveal(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
