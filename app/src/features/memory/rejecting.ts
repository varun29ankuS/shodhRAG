/**
 * Memory suggestions whose rejection waits out its undo window, shared by every surface
 * that can reject one (Settings → Memory's Reject, the Inbox's Dismiss) and every surface
 * that can accept several at once ("Accept N non-sensitive"). Until the window ends the
 * suggestion is still pending in the backend, so a batch accept must leave it out wherever
 * it was rejected. An id is added when the rejection starts and removed on Undo or when the
 * rejection fails; once it is carried out the id stays until a reloaded pending list no
 * longer has it (`settle`), so a list loaded before the rejection never shows it or
 * counts it again. Import-free, so Node's strip-types runner
 * tests it (`app/tests/batchAccept.test.ts`).
 */
import { markRejecting } from './batchAccept.ts';

type Listener = () => void;

/** A set of suggestion ids being rejected, with change notification. */
export class RejectingStore {
  private ids: ReadonlySet<string> = new Set();
  /** Ids whose rejection was carried out, kept until a reloaded list confirms it. */
  private readonly done = new Set<string>();
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
    if (!on) this.done.delete(id);
    const next = markRejecting(this.ids, id, on);
    if (next === this.ids) return;
    this.ids = next;
    for (const listener of [...this.listeners]) listener();
  }

  /**
   * The full list of waiting suggestions was reloaded: rejections carried out whose
   * suggestion it no longer has are done with. Pass every waiting suggestion's id, not a
   * filtered view.
   */
  settle(waitingIds: readonly string[]): void {
    const waiting = new Set(waitingIds);
    for (const id of [...this.done]) {
      if (!waiting.has(id)) this.mark(id, false);
    }
  }

  /**
   * Hooks for `removeWithUndo` rejecting suggestion `id`: `hide`/`restore` keep it in the
   * set, `commit` runs `reject` and keeps it until a reload no longer lists it (`settle`;
   * a failed rejection is restored by the undo queue).
   */
  around(id: string, reject: () => Promise<unknown>) {
    return {
      hide: () => this.mark(id, true),
      restore: () => this.mark(id, false),
      commit: async () => {
        await reject();
        if (this.ids.has(id)) this.done.add(id);
      },
    };
  }
}

/** The app's suggestions being rejected. */
export const pendingRejections = new RejectingStore();
