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
  /** A write succeeded. */
  onWritten?: (conv: C) => void;
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
  private stopped = false;

  constructor(deps: SaveDeps<C>) {
    this.deps = deps;
  }

  /**
   * Stop saving: waiting saves are dropped and later ones are refused. Used
   * when the saved conversations could not be loaded, so nothing written
   * now can overwrite the file the user's history is still in. Writes
   * already in flight finish (the backend refuses them too).
   */
  pause(): void {
    this.stopped = true;
    for (const id of [...this.pending.keys()]) this.drop(id);
  }

  /** Saving was paused ({@link pause}). */
  get paused(): boolean {
    return this.stopped;
  }

  /**
   * Save `conv` after the delay, replacing a save of the same conversation
   * still waiting; with `immediate`, write it now instead. `onSaved` runs
   * only if this save is the one written and it succeeds.
   */
  save(conv: C, onSaved?: () => void, immediate = false): void {
    if (this.stopped) return;
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
    if (this.stopped) return Promise.resolve();
    const previous = this.inFlight.get(conv.id) ?? Promise.resolve();
    const next = previous
      .then(() => this.deps.write(conv))
      .then(() => {
        this.deps.onWritten?.(conv);
        onSaved?.();
      })
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

/**
 * Whether a failed save is shown: the first failure after a success (or the
 * first ever), so a store that keeps refusing is reported once, not on
 * every save every 500 ms; a save that succeeds re-arms it.
 */
export class FailureLatch {
  private failing = false;

  /** A save failed; true when the user should be told. */
  fail(): boolean {
    if (this.failing) return false;
    this.failing = true;
    return true;
  }

  /** A save succeeded. */
  succeed(): void {
    this.failing = false;
  }
}

/** The blocking notice shown when the saved conversations cannot be loaded. */
export interface LoadFailureNotice {
  title: string;
  paragraphs: string[];
}

/**
 * What to tell the user when `load_conversations` fails, in plain words:
 * what happened, that saving is paused so nothing is overwritten, and where
 * the file is (the backend's message names it).
 */
export function loadFailureNotice(error: unknown): LoadFailureNotice {
  const detail = (error instanceof Error ? error.message : String(error ?? '')).trim();
  return {
    title: 'Your saved conversations could not be loaded',
    paragraphs: [
      'Shodh could not read the file that holds your conversations, or its backup copy.',
      'Saving is paused, so nothing you do now overwrites that file: your history may still be in it. '
        + 'New messages in this session are not saved.',
      detail.length > 0 ? `Details: ${detail}` : 'No details were given.',
      'To get your conversations back, quit Shodh, restore the file from a backup (or move it away to start fresh), and open Shodh again.',
    ],
  };
}
