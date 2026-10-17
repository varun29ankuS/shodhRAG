/**
 * Saving conversation records: debounced per conversation, or at once for a
 * change that must not wait (a finished answer), and always in order per
 * conversation. Pure (the write and the timer are passed in), so it is
 * unit-tested directly with Node (`app/tests/conversationSaves.test.ts`).
 */

/** Event the backend sends before the app exits (`shutdown.rs`); answered with `quit_ready`. */
export const APP_QUITTING_EVENT = 'app-quitting';

/** The request id of an `app-quitting` payload, or null when it is malformed. */
export function quitRequestId(payload: unknown): number | null {
  if (typeof payload !== 'object' || payload === null) return null;
  const id = (payload as Record<string, unknown>).requestId;
  return typeof id === 'number' && Number.isSafeInteger(id) && id >= 0 ? id : null;
}

/**
 * Apply `change` to the record with `id`. Returns the new list and the
 * changed record (null, and the list unchanged, when there is none), so the
 * caller saves exactly the record it put in the list, outside any state
 * updater.
 */
export function updateRecord<C extends { id: string }>(
  list: C[],
  id: string,
  change: (record: C) => C,
): { list: C[]; updated: C | null } {
  const index = list.findIndex(c => c.id === id);
  if (index < 0) return { list, updated: null };
  const updated = change(list[index]);
  const next = list.slice();
  next[index] = updated;
  return { list: next, updated };
}

export interface SaveDeps<C extends { id: string }> {
  /** Store the record (`save_conversation`). */
  write: (conv: C) => Promise<unknown>;
  /** Run `run` after `ms`; returns a function that cancels it. */
  schedule: (run: () => void, ms: number) => () => void;
  /** A write failed. */
  onError: (conv: C, error: unknown) => void;
  delayMs: number;
}

interface Pending<C> {
  conv: C;
  onSaved?: () => void;
  cancel: () => void;
}

export class ConversationSaves<C extends { id: string }> {
  private readonly deps: SaveDeps<C>;
  private readonly pending = new Map<string, Pending<C>>();
  private readonly inFlight = new Map<string, Promise<void>>();

  constructor(deps: SaveDeps<C>) {
    this.deps = deps;
  }

  /**
   * Save `conv` after the delay, replacing a save of the same conversation
   * still waiting; with `immediate`, write it now instead. `onSaved` runs
   * only if this save is the one written and it succeeds.
   */
  save(conv: C, onSaved?: () => void, immediate = false): void {
    this.drop(conv.id);
    if (immediate) {
      void this.write(conv, onSaved);
      return;
    }
    const cancel = this.deps.schedule(() => {
      this.pending.delete(conv.id);
      void this.write(conv, onSaved);
    }, this.deps.delayMs);
    this.pending.set(conv.id, { conv, onSaved, cancel });
  }

  /** Write every waiting save now (page hidden or closing, provider unmounting). */
  flush(): void {
    const waiting = [...this.pending.values()];
    for (const p of waiting) this.drop(p.conv.id);
    for (const p of waiting) void this.write(p.conv, p.onSaved);
  }

  /**
   * Write every waiting save now and resolve once every write, earlier ones
   * included, has finished (the app is quitting). Per-conversation order is
   * kept: a flushed save still goes after the write of its conversation in
   * flight. Never rejects.
   */
  async drain(): Promise<void> {
    this.flush();
    await Promise.all([...this.inFlight.values()]);
  }

  /**
   * Write `conv` once every earlier write of the same conversation has
   * finished, so two saves made a few ms apart reach the store in the order
   * they were made. Never rejects.
   */
  write(conv: C, onSaved?: () => void): Promise<void> {
    const previous = this.inFlight.get(conv.id) ?? Promise.resolve();
    const next = previous
      .then(() => this.deps.write(conv))
      .then(() => onSaved?.())
      .catch(error => this.deps.onError(conv, error));
    this.inFlight.set(conv.id, next);
    void next.then(() => {
      if (this.inFlight.get(conv.id) === next) this.inFlight.delete(conv.id);
    });
    return next;
  }

  private drop(id: string): void {
    const p = this.pending.get(id);
    if (!p) return;
    p.cancel();
    this.pending.delete(id);
  }
}
