/**
 * Best-effort background work keyed by a string (e.g. reading the bytes of
 * the next file in the list): each task waits a delay, then runs with bounded
 * concurrency. Scheduling a key that is already waiting or running is a
 * no-op; cancelling a waiting task drops it, and cancelling a running task
 * flags its signal so it discards its result (an IPC read cannot be aborted
 * midway, but a stale result must not displace useful cache entries).
 *
 * Pure module (timers are injected) so it is unit-tested directly with Node
 * (`app/tests/prefetch.test.ts`).
 */

export interface PrefetchTimers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

/** Read by a running task before it commits its result. */
export interface PrefetchSignal {
  readonly cancelled: boolean;
}

export type PrefetchRun = (signal: PrefetchSignal) => Promise<void>;

type TaskState = 'waiting' | 'queued' | 'running';

interface Task {
  key: string;
  run: PrefetchRun;
  state: TaskState;
  timer: unknown;
  signal: { cancelled: boolean };
}

export class Prefetcher {
  private readonly timers: PrefetchTimers;
  private readonly concurrency: number;
  private readonly tasks = new Map<string, Task>();
  private readonly queue: Task[] = [];
  private running = 0;

  constructor(timers: PrefetchTimers, concurrency = 1) {
    this.timers = timers;
    this.concurrency = Math.max(1, Math.floor(concurrency));
  }

  /** Keys of tasks not yet finished, cancelled or failed. */
  activeKeys(): string[] {
    return [...this.tasks.keys()];
  }

  stateOf(key: string): TaskState | null {
    return this.tasks.get(key)?.state ?? null;
  }

  /** Run `run` for `key` after `delayMs`, unless `key` is already scheduled. */
  schedule(key: string, delayMs: number, run: PrefetchRun): void {
    if (this.tasks.has(key)) return;
    const task: Task = { key, run, state: 'waiting', timer: null, signal: { cancelled: false } };
    this.tasks.set(key, task);
    const enqueue = () => {
      if (this.tasks.get(key) !== task) return;
      task.timer = null;
      task.state = 'queued';
      this.queue.push(task);
      this.pump();
    };
    if (delayMs <= 0) enqueue();
    else task.timer = this.timers.setTimeout(enqueue, delayMs);
  }

  /** Drop a waiting task, or flag a running one as cancelled. */
  cancel(key: string): void {
    const task = this.tasks.get(key);
    if (!task) return;
    this.tasks.delete(key);
    task.signal.cancelled = true;
    if (task.state === 'waiting' && task.timer !== null) {
      this.timers.clearTimeout(task.timer);
      task.timer = null;
    } else if (task.state === 'queued') {
      const index = this.queue.indexOf(task);
      if (index >= 0) this.queue.splice(index, 1);
    }
  }

  /** Cancel every task whose key is not in `keep`. */
  retain(keep: Iterable<string>): void {
    const wanted = new Set(keep);
    for (const key of [...this.tasks.keys()]) if (!wanted.has(key)) this.cancel(key);
  }

  cancelAll(): void {
    for (const key of [...this.tasks.keys()]) this.cancel(key);
  }

  private pump(): void {
    while (this.running < this.concurrency && this.queue.length > 0) {
      const task = this.queue.shift()!;
      task.state = 'running';
      this.running += 1;
      let outcome: Promise<void>;
      try {
        outcome = Promise.resolve(task.run(task.signal));
      } catch (error) {
        outcome = Promise.reject(error);
      }
      // Failures are not reported: prefetching is an optimisation, and the
      // foreground load of the same file surfaces any real error.
      outcome
        .catch(() => undefined)
        .then(() => {
          this.running -= 1;
          if (this.tasks.get(task.key) === task) this.tasks.delete(task.key);
          this.pump();
        });
    }
  }
}
