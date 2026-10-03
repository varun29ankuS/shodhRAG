/**
 * Least-recently-used cache bounded by entry count and by total bytes, with
 * leases so a value that is in use (e.g. the PDF on screen) is never disposed
 * under its user: eviction skips leased entries, and an entry removed while
 * leased is disposed when its last lease is released.
 *
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/lruCache.test.ts`).
 */

export interface LruCacheOptions<V> {
  /** Most entries kept (leased entries can push the count over briefly). */
  maxEntries: number;
  /** Most bytes kept, summed over the entries' declared sizes. */
  maxBytes: number;
  /** Frees a value that left the cache and is no longer leased. */
  dispose: (value: V, key: string) => void;
}

/** A hold on a cached value; `release` is idempotent. */
export interface Lease<V> {
  readonly key: string;
  readonly value: V;
  release(): void;
}

interface Entry<V> {
  value: V;
  bytes: number;
  leases: number;
  /** Removed from the map while leased; dispose on the last release. */
  detached: boolean;
}

export class LruCache<V> {
  /** Map iteration order is recency order: first = least recently used. */
  private readonly entries = new Map<string, Entry<V>>();
  private readonly options: LruCacheOptions<V>;
  private totalBytes = 0;

  constructor(options: LruCacheOptions<V>) {
    if (!(options.maxEntries >= 1)) throw new RangeError('maxEntries must be at least 1');
    if (!(options.maxBytes > 0)) throw new RangeError('maxBytes must be positive');
    this.options = options;
  }

  /** Number of entries in the cache (leased-and-detached entries excluded). */
  get size(): number {
    return this.entries.size;
  }

  /** Sum of the declared sizes of the entries in the cache. */
  get bytes(): number {
    return this.totalBytes;
  }

  /** Keys from least to most recently used. */
  keys(): string[] {
    return [...this.entries.keys()];
  }

  has(key: string): boolean {
    return this.entries.has(key);
  }

  /** The value without marking it used. */
  peek(key: string): V | undefined {
    return this.entries.get(key)?.value;
  }

  /** The value, marked most recently used. */
  get(key: string): V | undefined {
    const entry = this.entries.get(key);
    if (!entry) return undefined;
    this.touch(key, entry);
    return entry.value;
  }

  /** Insert or replace a value, then evict down to the bounds. */
  set(key: string, value: V, bytes: number): void {
    this.insert(key, value, bytes);
    this.evict();
  }

  /**
   * Insert or replace a value and lease it in one step, so a value larger
   * than the byte budget is still handed to its caller (and disposed once
   * released) instead of being evicted before it can be used.
   */
  setAndAcquire(key: string, value: V, bytes: number): Lease<V> {
    const entry = this.insert(key, value, bytes);
    const lease = this.lease(key, entry);
    this.evict();
    return lease;
  }

  /** Lease the value (marking it most recently used), or null if absent. */
  acquire(key: string): Lease<V> | null {
    const entry = this.entries.get(key);
    if (!entry) return null;
    this.touch(key, entry);
    return this.lease(key, entry);
  }

  /**
   * Remove an entry. It is disposed now, or on its last release if leased,
   * unless `dispose` is false (the caller takes ownership of the value).
   */
  delete(key: string, dispose = true): boolean {
    const entry = this.entries.get(key);
    if (!entry) return false;
    this.detach(key, entry, dispose);
    return true;
  }

  /** Remove every entry (leased ones are disposed on release). */
  clear(): void {
    for (const [key, entry] of [...this.entries]) this.detach(key, entry, true);
  }

  private insert(key: string, value: V, bytes: number): Entry<V> {
    const previous = this.entries.get(key);
    if (previous) {
      if (previous.value === value) {
        this.totalBytes += Math.max(0, bytes) - previous.bytes;
        previous.bytes = Math.max(0, bytes);
        this.touch(key, previous);
        return previous;
      }
      this.detach(key, previous, true);
    }
    const entry: Entry<V> = { value, bytes: Math.max(0, bytes), leases: 0, detached: false };
    this.entries.set(key, entry);
    this.totalBytes += entry.bytes;
    return entry;
  }

  private touch(key: string, entry: Entry<V>): void {
    this.entries.delete(key);
    this.entries.set(key, entry);
  }

  private lease(key: string, entry: Entry<V>): Lease<V> {
    entry.leases += 1;
    let released = false;
    return {
      key,
      value: entry.value,
      release: () => {
        if (released) return;
        released = true;
        entry.leases -= 1;
        if (entry.leases > 0) return;
        if (entry.detached) this.options.dispose(entry.value, key);
        else this.evict();
      },
    };
  }

  private detach(key: string, entry: Entry<V>, dispose: boolean): void {
    this.entries.delete(key);
    this.totalBytes -= entry.bytes;
    if (!dispose) return;
    if (entry.leases > 0) entry.detached = true;
    else this.options.dispose(entry.value, key);
  }

  private evict(): void {
    const { maxEntries, maxBytes } = this.options;
    while (this.entries.size > maxEntries || this.totalBytes > maxBytes) {
      let victim: [string, Entry<V>] | null = null;
      for (const pair of this.entries) {
        if (pair[1].leases === 0) {
          victim = pair;
          break;
        }
      }
      // Everything left is in use: stay over budget until a release.
      if (!victim) return;
      this.detach(victim[0], victim[1], true);
    }
  }
}
