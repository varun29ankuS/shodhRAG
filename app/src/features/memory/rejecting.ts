/**
 * Memory suggestions whose rejection waits out its undo window, shared by every surface
 * that can reject one (Settings → Memory's Reject, the Inbox's Dismiss) and every surface
 * that can accept several at once ("Accept N non-sensitive"). Until the window ends the
 * suggestion is still pending in the backend, so a batch accept must leave it out wherever
 * it was rejected. An id is added when the rejection starts and removed on Undo, when the
 * rejection fails, or once it is carried out. Import-free, so Node's strip-types runner
 * tests it (`app/tests/batchAccept.test.ts`).
 */
import { markRejecting } from './batchAccept.ts';

type Listener = () => void;

/** A set of suggestion ids being rejected, with change notification. */
export class RejectingStore {
  private ids: ReadonlySet<string> = new Set();
  private readonly listeners = new Set<Listener>();

  /** The ids now (a new set after every change, so it can be compared by identity). */
  readonly get = (): ReadonlySet<string> => this.ids;

  /** Calls `listener` after every change; returns the unsubscribe function. */
  readonly subscribe = (listener: Listener): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** `id`'s rejection started (`on`) or ended (undone, failed or carried out). */
  mark(id: string, on: boolean): void {
    const next = markRejecting(this.ids, id, on);
    if (next === this.ids) return;
    this.ids = next;
    for (const listener of [...this.listeners]) listener();
  }

  /**
   * Hooks for `removeWithUndo` rejecting suggestion `id`: `hide`/`restore` keep it in the
   * set, `commit` runs `reject` and takes it out once the rejection is carried out (a
   * failed rejection is restored by the undo queue).
   */
  around(id: string, reject: () => Promise<unknown>) {
    return {
      hide: () => this.mark(id, true),
      restore: () => this.mark(id, false),
      commit: async () => {
        await reject();
        this.mark(id, false);
      },
    };
  }
}

/** The app's suggestions being rejected. */
export const pendingRejections = new RejectingStore();
