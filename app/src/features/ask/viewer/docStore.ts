/**
 * Opened documents kept between file opens, so going back to a document is
 * instant and the next one in a list can be read ahead.
 *
 * Entries are keyed by path, file size and (when known) modification time,
 * and hold either prefetched bytes
 * or an opened document. Opening may consume the bytes (pdf.js transfers the
 * buffer to its worker), so an entry switches from bytes to document with
 * the same byte accounting and a buffer is never opened twice. Reads and
 * opens are shared per key: a file opened while its prefetch is in flight
 * waits for that read instead of reading again, and a prefetch whose key was
 * claimed by an open (or that was cancelled) does not insert its bytes.
 * Documents in use are leased and never destroyed under their user; evicted
 * documents are destroyed.
 *
 * Pure module (I/O is injected) so it is unit-tested directly with Node
 * (`app/tests/docStore.test.ts`); `pdfDocCache.ts` wires it to pdf.js.
 */
import { LruCache, type Lease } from './lruCache.ts';

export interface OpenedDoc<D> {
  doc: D;
  destroy(): void;
}

export interface DocStoreOptions<D> {
  /** Reads the whole file. */
  read(path: string): Promise<Uint8Array>;
  /** Opens a document from bytes (may take ownership of the buffer). */
  open(bytes: Uint8Array): Promise<OpenedDoc<D>>;
  /** Identity of a file version. */
  /** `version` is the file's modification time (ms) when known. */
  keyOf(path: string, size: number, version?: number | null): string;
  maxEntries: number;
  maxBytes: number;
  /** Larger files are not read ahead: one would evict most of the cache. */
  prefetchMaxBytes: number;
}

/** A document held open for display; release it when done. */
export interface DocLease<D> {
  doc: D;
  /** The document was already open (no read, no parse). */
  fromCache: boolean;
  release(): void;
}

type Entry<D> = { kind: 'bytes'; bytes: Uint8Array } | { kind: 'doc'; opened: OpenedDoc<D> };

export interface DocStore<D> {
  /**
   * Open a document for display. With a known `size` it is cached and
   * shared; with null (no stable identity) it is opened privately and
   * destroyed on release.
   */
  acquire(path: string, size: number | null, version?: number | null): Promise<DocLease<D>>;
  /** The open document for `path` at `size` (and `version`), only if it is already cached. */
  acquireCached(path: string, size: number, version?: number | null): DocLease<D> | null;
  /**
   * Read a file's bytes ahead of time (no parsing). Does nothing when the
   * file is cached, being opened, or too large; drops the bytes if `signal`
   * was cancelled while the read was in flight.
   */
  prefetch(path: string, size: number, signal: { readonly cancelled: boolean }, version?: number | null): Promise<void>;
  /** Mark foreground work (e.g. a first page rendering) until the returned release. */
  holdForeground(): () => void;
  /** Resolves when no foreground work is in progress. */
  whenForegroundIdle(): Promise<void>;
  /** Cached keys, least recently used first. */
  keys(): string[];
  /** Kind of the cached entry for `key`, if any. */
  kindOf(key: string): 'bytes' | 'doc' | null;
}

export function createDocStore<D>(options: DocStoreOptions<D>): DocStore<D> {
  const cache = new LruCache<Entry<D>>({
    maxEntries: options.maxEntries,
    maxBytes: options.maxBytes,
    dispose: entry => {
      if (entry.kind === 'doc') entry.opened.destroy();
    },
  });
  /** Byte reads in flight, shared by prefetches and opens. */
  const reads = new Map<string, Promise<Uint8Array>>();
  /** Opens in flight; a key here is claimed by the foreground. */
  const opens = new Map<string, Promise<void>>();
  let foreground = 0;
  let idleWaiters: Array<() => void> = [];

  const holdForeground = (): (() => void) => {
    foreground += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      foreground -= 1;
      if (foreground > 0) return;
      const waiters = idleWaiters;
      idleWaiters = [];
      for (const resolve of waiters) resolve();
    };
  };

  const readShared = (key: string, path: string): Promise<Uint8Array> => {
    let pending = reads.get(key);
    if (!pending) {
      pending = options.read(path).finally(() => reads.delete(key));
      reads.set(key, pending);
    }
    return pending;
  };

  const fromLease = (lease: Lease<Entry<D>>, fromCache: boolean): DocLease<D> => {
    const entry = lease.value;
    if (entry.kind !== 'doc') throw new Error('Cached entry is not an open document');
    return { doc: entry.opened.doc, fromCache, release: () => lease.release() };
  };

  const privateLease = (opened: OpenedDoc<D>): DocLease<D> => {
    let released = false;
    return {
      doc: opened.doc,
      fromCache: false,
      release: () => {
        if (released) return;
        released = true;
        opened.destroy();
      },
    };
  };

  const acquireCached = (path: string, size: number, version: number | null = null): DocLease<D> | null => {
    const lease = cache.acquire(options.keyOf(path, size, version));
    if (!lease) return null;
    if (lease.value.kind !== 'doc') {
      lease.release();
      return null;
    }
    return fromLease(lease, true);
  };

  const acquire = async (path: string, size: number | null, version: number | null = null): Promise<DocLease<D>> => {
    if (size === null) {
      const release = holdForeground();
      try {
        return privateLease(await options.open(await options.read(path)));
      } finally {
        release();
      }
    }

    const key = options.keyOf(path, size, version);
    for (;;) {
      const hit = acquireCached(path, size, version);
      if (hit) return hit;
      const pending = opens.get(key);
      if (!pending) break;
      await pending;
    }

    let settle: () => void = () => {};
    opens.set(
      key,
      new Promise<void>(resolve => {
        settle = resolve;
      }),
    );
    const release = holdForeground();
    try {
      let bytes: Uint8Array;
      const cached = cache.peek(key);
      if (cached?.kind === 'bytes') {
        // Taken out: opening may detach the buffer.
        cache.delete(key, false);
        bytes = cached.bytes;
      } else {
        bytes = await readShared(key, path);
      }
      // The file changed between the size lookup and the read: show it, but
      // do not cache it under a size it no longer has.
      if (bytes.byteLength !== size) return privateLease(await options.open(bytes));
      const opened = await options.open(bytes);
      return fromLease(cache.setAndAcquire(key, { kind: 'doc', opened }, size), false);
    } finally {
      opens.delete(key);
      settle();
      release();
    }
  };

  const prefetch = async (
    path: string,
    size: number,
    signal: { readonly cancelled: boolean },
    version: number | null = null,
  ): Promise<void> => {
    if (size <= 0 || size > options.prefetchMaxBytes) return;
    const key = options.keyOf(path, size, version);
    if (cache.has(key) || opens.has(key)) return;
    const bytes = await readShared(key, path);
    if (signal.cancelled || opens.has(key) || cache.has(key) || bytes.byteLength !== size) return;
    cache.set(key, { kind: 'bytes', bytes }, size);
  };

  return {
    acquire,
    acquireCached,
    prefetch,
    holdForeground,
    whenForegroundIdle: () => (foreground === 0 ? Promise.resolve() : new Promise(resolve => idleWaiters.push(resolve))),
    keys: () => cache.keys(),
    kindOf: key => cache.peek(key)?.kind ?? null,
  };
}
