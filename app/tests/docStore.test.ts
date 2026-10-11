/**
 * Document store tests: sharing reads between prefetch and open, handing
 * bytes over exactly once, eviction destroying documents, leases.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/docStore.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createDocStore } from '../src/features/ask/viewer/docStore.ts';

interface FakeDoc {
  id: number;
  from: Uint8Array;
}

/** Fake I/O: reads resolve when the test says so; opens record their input. */
function harness(options: { maxEntries?: number; maxBytes?: number; prefetchMaxBytes?: number } = {}) {
  const sizes = new Map<string, number>();
  const reads: string[] = [];
  const pendingReads = new Map<string, Array<() => void>>();
  let autoRead = true;
  const opened: FakeDoc[] = [];
  const destroyed: number[] = [];
  let failNextOpen = false;
  let nextId = 1;

  const store = createDocStore<FakeDoc>({
    read: path => {
      reads.push(path);
      const bytes = new Uint8Array(sizes.get(path) ?? 0);
      if (autoRead) return Promise.resolve(bytes);
      return new Promise(resolve => {
        const list = pendingReads.get(path) ?? [];
        list.push(() => resolve(bytes));
        pendingReads.set(path, list);
      });
    },
    open: async bytes => {
      if (failNextOpen) {
        failNextOpen = false;
        throw new Error('Invalid PDF structure');
      }
      const doc = { id: nextId++, from: bytes };
      opened.push(doc);
      return { doc, destroy: () => destroyed.push(doc.id) };
    },
    keyOf: (path, size) => `${path}|${size}`,
    maxEntries: options.maxEntries ?? 4,
    maxBytes: options.maxBytes ?? 1000,
    prefetchMaxBytes: options.prefetchMaxBytes ?? 500,
  });

  return {
    store,
    reads,
    opened,
    destroyed,
    file: (path: string, size: number) => sizes.set(path, size),
    manualReads: () => {
      autoRead = false;
    },
    finishRead: (path: string) => {
      const list = pendingReads.get(path) ?? [];
      pendingReads.delete(path);
      for (const resolve of list) resolve();
    },
    failNextOpen: () => {
      failNextOpen = true;
    },
  };
}

const live = { cancelled: false };
const flush = () => new Promise<void>(resolve => setImmediate(resolve));

test('a reopened document comes from the cache: no second read or parse', async () => {
  const h = harness();
  h.file('a.pdf', 10);
  const first = await h.store.acquire('a.pdf', 10);
  assert.equal(first.fromCache, false);
  first.release();
  const again = await h.store.acquire('a.pdf', 10);
  assert.equal(again.fromCache, true);
  assert.equal(again.doc, first.doc);
  assert.deepEqual(h.reads, ['a.pdf']);
  assert.equal(h.opened.length, 1);
  again.release();
  assert.deepEqual(h.destroyed, []);
});

test('prefetched bytes are opened once and the entry becomes the document', async () => {
  const h = harness();
  h.file('next.pdf', 20);
  await h.store.prefetch('next.pdf', 20, live);
  assert.equal(h.store.kindOf('next.pdf|20'), 'bytes');
  const lease = await h.store.acquire('next.pdf', 20);
  assert.deepEqual(h.reads, ['next.pdf']);
  assert.equal(h.opened.length, 1);
  assert.equal(h.store.kindOf('next.pdf|20'), 'doc');
  lease.release();
});

test('opening a file while its prefetch read is in flight joins that read', async () => {
  const h = harness();
  h.manualReads();
  h.file('b.pdf', 30);
  const prefetching = h.store.prefetch('b.pdf', 30, live);
  const opening = h.store.acquire('b.pdf', 30);
  await flush();
  h.finishRead('b.pdf');
  const lease = await opening;
  await prefetching;
  assert.deepEqual(h.reads, ['b.pdf']);
  assert.equal(h.opened.length, 1);
  // The open claimed the bytes; the prefetch must not re-insert them.
  assert.equal(h.store.kindOf('b.pdf|30'), 'doc');
  assert.deepEqual(h.store.keys(), ['b.pdf|30']);
  lease.release();
});

test('a prefetch cancelled while reading discards its bytes', async () => {
  const h = harness();
  h.manualReads();
  h.file('c.pdf', 30);
  const signal = { cancelled: false };
  const prefetching = h.store.prefetch('c.pdf', 30, signal);
  signal.cancelled = true;
  h.finishRead('c.pdf');
  await prefetching;
  assert.deepEqual(h.store.keys(), []);
});

test('prefetch skips files that are too large, cached, or empty', async () => {
  const h = harness({ prefetchMaxBytes: 100 });
  h.file('big.pdf', 101);
  h.file('open.pdf', 10);
  await h.store.prefetch('big.pdf', 101, live);
  await h.store.prefetch('empty.pdf', 0, live);
  (await h.store.acquire('open.pdf', 10)).release();
  await h.store.prefetch('open.pdf', 10, live);
  assert.deepEqual(h.reads, ['open.pdf']);
});

test('eviction destroys documents nobody holds; held ones survive until released', async () => {
  const h = harness({ maxEntries: 1 });
  h.file('a.pdf', 10);
  h.file('b.pdf', 10);
  const a = await h.store.acquire('a.pdf', 10);
  const b = await h.store.acquire('b.pdf', 10);
  // Both in use: over the entry budget, nothing destroyed.
  assert.deepEqual(h.destroyed, []);
  a.release();
  assert.deepEqual(h.destroyed, [a.doc.id]);
  b.release();
  assert.deepEqual(h.destroyed, [a.doc.id]);
  assert.deepEqual(h.store.keys(), ['b.pdf|10']);
});

test('eviction by bytes: prefetched bytes push out the oldest unused document', async () => {
  const h = harness({ maxBytes: 100 });
  h.file('old.pdf', 60);
  h.file('next.pdf', 60);
  (await h.store.acquire('old.pdf', 60)).release();
  await h.store.prefetch('next.pdf', 60, live);
  assert.deepEqual(h.store.keys(), ['next.pdf|60']);
  assert.equal(h.destroyed.length, 1);
});

test('concurrent opens of one file share a single open', async () => {
  const h = harness();
  h.manualReads();
  h.file('d.pdf', 10);
  const one = h.store.acquire('d.pdf', 10);
  const two = h.store.acquire('d.pdf', 10);
  await flush();
  h.finishRead('d.pdf');
  const [l1, l2] = await Promise.all([one, two]);
  assert.equal(l1.doc, l2.doc);
  assert.equal(h.opened.length, 1);
  assert.equal(l2.fromCache, true);
  l1.release();
  l2.release();
});

test('a file whose size changed since the lookup is shown but not cached', async () => {
  const h = harness();
  h.file('e.pdf', 12);
  const lease = await h.store.acquire('e.pdf', 10);
  assert.deepEqual(h.store.keys(), []);
  lease.release();
  assert.deepEqual(h.destroyed, [lease.doc.id]);
});

test('without a size the document is private and destroyed on release', async () => {
  const h = harness();
  h.file('f.pdf', 10);
  const lease = await h.store.acquire('f.pdf', null);
  assert.deepEqual(h.store.keys(), []);
  lease.release();
  lease.release();
  assert.deepEqual(h.destroyed, [lease.doc.id]);
});

test('a failed open leaves nothing claimed; the next attempt reads again', async () => {
  const h = harness();
  h.file('g.pdf', 10);
  h.failNextOpen();
  await assert.rejects(h.store.acquire('g.pdf', 10), /Invalid PDF/);
  assert.deepEqual(h.store.keys(), []);
  const lease = await h.store.acquire('g.pdf', 10);
  assert.equal(h.reads.length, 2);
  lease.release();
});

test('acquireCached returns only opened documents', async () => {
  const h = harness();
  h.file('h.pdf', 10);
  await h.store.prefetch('h.pdf', 10, live);
  assert.equal(h.store.acquireCached('h.pdf', 10), null);
  (await h.store.acquire('h.pdf', 10)).release();
  const cached = h.store.acquireCached('h.pdf', 10);
  assert.ok(cached);
  assert.equal(cached.fromCache, true);
  cached.release();
});

test('foreground holds delay background work until released', async () => {
  const h = harness();
  let idle = false;
  const release = h.store.holdForeground();
  const waiting = h.store.whenForegroundIdle().then(() => {
    idle = true;
  });
  await flush();
  assert.equal(idle, false);
  release();
  release(); // idempotent: must not go negative
  await waiting;
  assert.equal(idle, true);
  const second = h.store.holdForeground();
  let idleAgain = false;
  void h.store.whenForegroundIdle().then(() => {
    idleAgain = true;
  });
  await flush();
  assert.equal(idleAgain, false);
  second();
  await flush();
  assert.equal(idleAgain, true);
});
