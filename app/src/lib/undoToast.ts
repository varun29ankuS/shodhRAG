/**
 * The app's undo window: `removeWithUndo` hides a record, shows a notice with
 * Undo for {@link UNDO_WINDOW_MS}, and carries out the removal when the notice
 * ends or is closed. One queue for the whole app, so `undoLast` (U in the Inbox)
 * reaches the most recent removal wherever it was made, and a card that
 * unmounts inside the window neither drops nor repeats its removal.
 */

import { toast } from 'sonner';
import { UNDO_WINDOW_MS, UndoQueue } from './undoQueue';
import type { Undoable } from './undoQueue';

export { UNDO_WINDOW_MS };

const toasts = new Map<number, string | number>();

const queue = new UndoQueue({
  set: (run, ms) => window.setTimeout(run, ms),
  clear: handle => window.clearTimeout(handle as number),
});

function closeNotice(id: number): void {
  const toastId = toasts.get(id);
  toasts.delete(id);
  if (toastId !== undefined) toast.dismiss(toastId);
}

export interface RemoveWithUndo extends Undoable {
  /** The notice, e.g. "Snippet deleted". */
  message: string;
  /** What was removed (its title). */
  description?: string;
}

/** Hide now, remove when the window ends; the notice offers Undo until then. */
export function removeWithUndo({ message, description, hide, restore, commit, onError }: RemoveWithUndo): number {
  let id = 0;
  id = queue.schedule({
    hide,
    restore,
    commit: async () => {
      // The removal runs: Undo is no longer offered.
      closeNotice(id);
      return commit();
    },
    onError,
  });
  const toastId = toast(message, {
    description,
    duration: Infinity,
    action: {
      label: 'Undo',
      onClick: () => {
        toasts.delete(id);
        queue.undo(id);
      },
    },
    onDismiss: () => {
      toasts.delete(id);
      void queue.commit(id);
    },
  });
  toasts.set(id, toastId);
  return id;
}

/** Undo the most recent removal still in its window. False when there is none. */
export function undoLast(): boolean {
  const id = queue.lastPending();
  if (id === undefined || !queue.undo(id)) return false;
  // Undone first, so closing the notice (which commits on dismiss) finds nothing to run.
  closeNotice(id);
  return true;
}
