/**
 * A request for a view that is not mounted yet ("open Settings → Memory",
 * "show the archived workspaces"): kept until the view takes it on mount,
 * and only for a short while. A request whose view never showed (the user
 * went elsewhere before it rendered) expires instead of surprising them the
 * next time they open that view by hand.
 *
 * No imports: unit-tested under Node (`app/tests/oneShot.test.ts`).
 */

/** How long a request waits for its view. Far longer than a view switch, even one waiting on a view's code. */
export const ONE_SHOT_TTL_MS = 8000;

export class OneShot<T> {
  private value: T | null = null;
  private at = 0;
  private readonly now: () => number;
  private readonly ttlMs: number;

  constructor(options: { now?: () => number; ttlMs?: number } = {}) {
    this.now = options.now ?? (() => Date.now());
    this.ttlMs = options.ttlMs ?? ONE_SHOT_TTL_MS;
  }

  /** Ask for `value` (replaces an earlier request). */
  set(value: T): void {
    this.value = value;
    this.at = this.now();
  }

  /** The request, without taking it (a first render, which may run again before it shows). */
  peek(): T | null {
    if (this.value !== null && this.now() - this.at > this.ttlMs) this.value = null;
    return this.value;
  }

  /** The request, once. */
  take(): T | null {
    const value = this.peek();
    this.value = null;
    return value;
  }
}
