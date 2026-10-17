/**
 * When the visible chat is written to its conversation, and what is written.
 *
 * "Needs saving" lives in the same view state as the messages it describes:
 * every change that must be saved bumps `persist.seq` inside the state update
 * that makes it. A save reads one view and records the seq it wrote, so a
 * save run with an older render's view can only write that view's own data,
 * and can never mark a newer change as saved.
 *
 * Pure (no React, no Tauri), so it is unit-tested directly with Node
 * (`app/tests/chatPersistence.test.ts`).
 */

/**
 * How a change is saved: `none` (display only, e.g. a streaming frame),
 * `debounced` (the conversation store's short delay) or `immediate` (a
 * finished answer, or an answer whose text is complete and being checked).
 */
export type PersistMode = 'none' | 'debounced' | 'immediate';

/** Changes made to a view, counted for the lifetime of the chat provider. */
export interface PersistMark {
  /** Increases with every change that must be saved; never reset. */
  seq: number;
  /** The `seq` of the latest change that must be saved without delay (0: none). */
  immediateSeq: number;
}

export const NO_CHANGES: PersistMark = { seq: 0, immediateSeq: 0 };

/** The chat as shown, with what of it is still to be saved. */
export interface PersistView<M extends { id: string }, S> {
  /** Conversation the messages belong to (persistence target). */
  conversationId: string | null;
  messages: M[];
  persist: PersistMark;
  /**
   * Stored form of answers that are still running but whose text is
   * complete (being checked against its sources), by message id. Saved in
   * place of the live message until it settles.
   */
  checkpoints: Readonly<Record<string, S>>;
}

/** A save the view needs: up to `seq`, now or after the store's delay. */
export interface SaveDue {
  seq: number;
  immediate: boolean;
}

export function emptyView<M extends { id: string }, S>(): PersistView<M, S> {
  return { conversationId: null, messages: [], persist: NO_CHANGES, checkpoints: {} };
}

/** Count one change made with `mode`. */
export function markChange(mark: PersistMark, mode: PersistMode): PersistMark {
  if (mode === 'none') return mark;
  const seq = mark.seq + 1;
  return { seq, immediateSeq: mode === 'immediate' ? seq : mark.immediateSeq };
}

/**
 * Whether a view must be saved, given the latest seq already saved. A view
 * whose changes are all saved (an older render's, or a freshly loaded one)
 * needs nothing. An immediate change not yet saved keeps the save immediate
 * even when debounced changes came after it.
 */
export function saveDue(mark: PersistMark, lastSavedSeq: number): SaveDue | null {
  if (mark.seq <= lastSavedSeq) return null;
  return { seq: mark.seq, immediate: mark.immediateSeq > lastSavedSeq };
}

/** Replace the item with the same id, or append it. */
export function upsertById<T extends { id: string }>(items: readonly T[], item: T): T[] {
  return items.some(i => i.id === item.id) ? items.map(i => (i.id === item.id ? item : i)) : [...items, item];
}

/**
 * Show `message` in the view (replacing the one with its id) if the view
 * holds `conversationId`. `checkpoint` is the stored form to save while the
 * message is still running.
 */
export function publishMessage<M extends { id: string }, S>(
  view: PersistView<M, S>,
  conversationId: string,
  message: M,
  mode: PersistMode,
  checkpoint: S | null = null,
): PersistView<M, S> {
  if (view.conversationId !== conversationId) return view;
  return {
    ...view,
    messages: upsertById(view.messages, message),
    persist: markChange(view.persist, mode),
    checkpoints: checkpoint === null ? view.checkpoints : { ...view.checkpoints, [message.id]: checkpoint },
  };
}

/** Change the view's messages if it holds `conversationId`. */
export function changeMessages<M extends { id: string }, S>(
  view: PersistView<M, S>,
  conversationId: string,
  update: (messages: M[]) => M[],
  mode: PersistMode,
): PersistView<M, S> {
  if (view.conversationId !== conversationId) return view;
  return { ...view, messages: update(view.messages), persist: markChange(view.persist, mode) };
}

/**
 * Show another conversation, as stored. Loading is not a change: the mark is
 * kept, so the loaded view is not saved back (which would bump its
 * `updatedAt` and reorder the sidebar).
 */
export function loadConversation<M extends { id: string }, S>(
  view: PersistView<M, S>,
  conversationId: string,
  messages: M[],
  checkpoints: Readonly<Record<string, S>>,
): PersistView<M, S> {
  return { conversationId, messages, persist: view.persist, checkpoints };
}

/**
 * The messages to store for a view: settled messages in their stored form, a
 * running answer as its latest checkpoint, and nothing for a running answer
 * that has none yet.
 */
export function messagesToSave<M extends { id: string }, S>(
  view: PersistView<M, S>,
  isSettled: (message: M) => boolean,
  toStored: (message: M) => S,
): S[] {
  const out: S[] = [];
  for (const message of view.messages) {
    if (isSettled(message)) {
      out.push(toStored(message));
      continue;
    }
    const checkpoint = view.checkpoints[message.id];
    if (checkpoint !== undefined) out.push(checkpoint);
  }
  return out;
}
