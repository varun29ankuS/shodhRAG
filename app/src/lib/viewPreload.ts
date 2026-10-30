/**
 * Loading the code of the lazily loaded views ahead of their first visit, so
 * opening a view never waits on its chunk. The likely views are fetched one
 * at a time while the app is idle, once the gate below lets them (never at
 * launch, never while an answer streams); any view is fetched at once when
 * its sidebar item is hovered or focused, when the command palette opens,
 * and when the assistant navigates to it.
 *
 * No imports: unit-tested under Node with injected loaders and an injected
 * idle scheduler (`app/tests/viewPreload.test.ts`).
 */

/** Runs a callback when the app is idle (requestIdleCallback, or a timer where there is none). */
export interface IdleScheduler {
  request(callback: () => void): unknown;
  cancel(handle: unknown): void;
}

export class ViewPreloader<K extends string> {
  private readonly loaders: Readonly<Record<K, () => Promise<unknown>>>;
  private readonly inFlight = new Map<K, Promise<unknown>>();
  private readonly modules = new Map<K, unknown>();

  constructor(loaders: Readonly<Record<K, () => Promise<unknown>>>) {
    this.loaders = loaders;
  }

  /** Whether `key`'s code has loaded. */
  isLoaded(key: K): boolean {
    return this.modules.has(key);
  }

  /** `key`'s module once loaded, else `undefined`. */
  loaded(key: K): unknown {
    return this.modules.get(key);
  }

  /**
   * Load `key`'s module once; concurrent calls share one load. Rejects when
   * the load fails; a failure is not remembered, so the next call tries again.
   */
  load(key: K): Promise<unknown> {
    if (this.modules.has(key)) return Promise.resolve(this.modules.get(key));
    const pending = this.inFlight.get(key);
    if (pending) return pending;
    const started = Promise.resolve()
      .then(this.loaders[key])
      .then(
        module => {
          this.modules.set(key, module);
          this.inFlight.delete(key);
          return module;
        },
        (error: unknown) => {
          this.inFlight.delete(key);
          throw error;
        },
      );
    this.inFlight.set(key, started);
    return started;
  }

  /**
   * Load `key`'s module ahead of use. Never rejects: the view's own load
   * reports a failure when it is opened.
   */
  preload(key: K): Promise<void> {
    return this.load(key).then(
      () => undefined,
      () => undefined,
    );
  }

  /**
   * Load each of `order` in turn, one per idle period, skipping any already
   * loaded. Returns a function that stops the remaining loads.
   */
  preloadWhenIdle(order: readonly K[], idle: IdleScheduler): () => void {
    let cancelled = false;
    let handle: unknown = null;
    let next = 0;
    const step = () => {
      handle = null;
      if (cancelled) return;
      while (next < order.length && this.modules.has(order[next])) next += 1;
      if (next >= order.length) return;
      const key = order[next];
      next += 1;
      void this.preload(key).then(() => {
        if (!cancelled) handle = idle.request(step);
      });
    };
    handle = idle.request(step);
    return () => {
      cancelled = true;
      if (handle !== null) idle.cancel(handle);
      handle = null;
    };
  }
}

/** Timers for the gate (injected in tests). */
export interface GateTimers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

/** Fetching the likely views' code starts this long after launch, unless an answer finished first. */
export const PRELOAD_DELAY_MS = 25_000;

/**
 * When the idle fetching of the likely views' code runs: after the first
 * answer has finished or `PRELOAD_DELAY_MS` after launch, whichever comes
 * first, and never while an answer streams (it stops when one starts and
 * resumes, skipping what loaded, when it ends). `start` begins the fetching
 * and returns the function that stops it.
 */
export class PreloadGate {
  private readonly start: () => () => void;
  private readonly timers: GateTimers;
  private due = false;
  private streaming = false;
  private stop: (() => void) | null = null;
  private timer: unknown = null;
  private disposed = false;

  constructor(start: () => () => void, timers: GateTimers, delayMs: number = PRELOAD_DELAY_MS) {
    this.start = start;
    this.timers = timers;
    this.timer = timers.setTimeout(() => {
      this.timer = null;
      this.becomeDue();
    }, delayMs);
  }

  /** Whether an answer is streaming now. The end of one makes the fetching due. */
  setStreaming(streaming: boolean): void {
    if (this.disposed || streaming === this.streaming) return;
    this.streaming = streaming;
    if (streaming) {
      this.stop?.();
      this.stop = null;
    } else {
      this.becomeDue();
    }
  }

  dispose(): void {
    this.disposed = true;
    if (this.timer !== null) this.timers.clearTimeout(this.timer);
    this.timer = null;
    this.stop?.();
    this.stop = null;
  }

  private becomeDue(): void {
    this.due = true;
    if (this.timer !== null) {
      this.timers.clearTimeout(this.timer);
      this.timer = null;
    }
    this.run();
  }

  private run(): void {
    if (this.disposed || !this.due || this.streaming || this.stop) return;
    this.stop = this.start();
  }
}

/**
 * A thenable that calls back at once with `value`. `React.lazy` reads its
 * module from the thenable its factory returns; one that calls back
 * synchronously resolves the lazy component on its first render, so a view
 * whose code was preloaded renders in the same commit instead of suspending
 * for a microtask (which would show its loading fallback for a frame).
 */
interface SyncThenable<T> {
  then(onFulfilled?: (resolved: T) => unknown): SyncThenable<T>;
}

export function settledThenable<T>(value: T): Promise<T> {
  const thenable: SyncThenable<T> = {
    then(onFulfilled) {
      if (onFulfilled) onFulfilled(value);
      return thenable;
    },
  };
  return thenable as unknown as Promise<T>;
}
