/**
 * Which answers have their Reasoning panel open, by run id.
 *
 * Kept outside React: the conversation is windowed (`ConversationLog`), so
 * an answer scrolled out of view unmounts and its component state is lost.
 * A panel the reader opened stays open when the answer is scrolled back into
 * view; nothing opens a panel but the reader.
 *
 * Pure module so it is unit-tested with Node (`app/tests/reasoning.test.ts`).
 */

export interface ReasoningOpenStore {
  isOpen: (runId: string) => boolean;
  setOpen: (runId: string, open: boolean) => void;
  subscribe: (listener: () => void) => () => void;
}

export function createReasoningOpenStore(): ReasoningOpenStore {
  const open = new Set<string>();
  const listeners = new Set<() => void>();
  return {
    isOpen: runId => open.has(runId),
    setOpen: (runId, value) => {
      if (open.has(runId) === value) return;
      if (value) open.add(runId);
      else open.delete(runId);
      for (const listener of [...listeners]) listener();
    },
    subscribe: listener => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

/** The app-wide store. */
export const reasoningOpen: ReasoningOpenStore = createReasoningOpenStore();
