/**
 * Undo instead of "are you sure?": a reversible removal is shown at once (the
 * caller hides the record) and carried out only when its undo window ends.
 * Undo drops the pending removal and gives the record back exactly as it was,
 * because nothing was changed on disk yet. If the app quits inside the window
 * the record simply survives.
 *
 * Pure (timers are injected), so Node's strip-types runner tests it directly
 * (`app/tests/undoQueue.test.ts`); `undoToast.ts` connects it to the toasts.
 */

/** How long Undo is offered (the Tasks view uses the same window). */
export const UNDO_WINDOW_MS = 6000;

/**
 * `list` with `record` back at `index` (or at the end when the list got shorter),
 * unchanged when an entry with the same id is already there.
 */
export function restoreAt<T extends { id: string }>(list: readonly T[], record: T, index: number): T[] {
  if (list.some(entry => entry.id === record.id)) return list as T[];
  const next = [...list];
  next.splice(Math.min(Math.max(index, 0), next.length), 0, record);
  return next;
}

export interface Undoable {
  /** Hide the record now (optimistic). */
  hide: () => void;
  /** Show it again, exactly as before (Undo, or the removal failed). */
  restore: () => void;
  /** Carry out the removal (the backend call). A rejection restores the record. */
  commit: () => Promise<unknown>;
  /** Told when `commit` failed, after the record was restored. */
  onError?: (error: unknown) => void;
}

export interface Timers {
  set: (run: () => void, ms: number) => unknown;
  clear: (handle: unknown) => void;
}

interface Pending {
  action: Undoable;
  timer: unknown;
}

export class UndoQueue {
  private readonly pending = new Map<number, Pending>();
  private readonly order: number[] = [];
  private nextId = 1;
  private readonly timers: Timers;
  private readonly windowMs: number;

  constructor(timers: Timers, windowMs: number = UNDO_WINDOW_MS) {
    this.timers = timers;
    this.windowMs = windowMs;
  }

  /** Hide now, remove when the window ends. Returns the id Undo and commit take. */
  schedule(action: Undoable): number {
    const id = this.nextId++;
    action.hide();
    const timer = this.timers.set(() => { void this.commit(id); }, this.windowMs);
    this.pending.set(id, { action, timer });
    this.order.push(id);
    return id;
  }

  private take(id: number): Undoable | null {
    const entry = this.pending.get(id);
    if (!entry) return null;
    this.pending.delete(id);
    this.timers.clear(entry.timer);
    const at = this.order.indexOf(id);
    if (at >= 0) this.order.splice(at, 1);
    return entry.action;
  }

  /** Undo one pending removal. False when it already ran or was undone. */
  undo(id: number): boolean {
    const action = this.take(id);
    if (!action) return false;
    action.restore();
    return true;
  }

  /** The most recent pending removal, if any. */
  lastPending(): number | undefined {
    return this.order[this.order.length - 1];
  }

  /** Undo the most recent pending removal (the U key). False when none is pending. */
  undoLast(): boolean {
    const last = this.lastPending();
    return last === undefined ? false : this.undo(last);
  }

  /** Carry out a pending removal now (the window ended or its notice was closed). */
  async commit(id: number): Promise<void> {
    const action = this.take(id);
    if (!action) return;
    try {
      await action.commit();
    } catch (error) {
      action.restore();
      action.onError?.(error);
    }
  }

  /** Carry out every pending removal (the view that owns them is closing). */
  async flush(): Promise<void> {
    await Promise.all([...this.order].map(id => this.commit(id)));
  }

  isPending(id: number): boolean {
    return this.pending.has(id);
  }

  get size(): number {
    return this.pending.size;
  }
}
