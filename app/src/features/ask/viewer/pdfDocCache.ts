/**
 * Loaded PDFs kept between file opens, so going back to a document is
 * instant and the next one in a list can be read ahead.
 *
 * Entries are keyed by path and file size (the viewer commands expose no
 * modification time) and hold either prefetched bytes or an opened pdf.js
 * document. Opening transfers the bytes to the worker, so an entry switches
 * from bytes to document with the same byte accounting and the same buffer
 * is never handed to pdf.js twice. Reads and opens are shared per key: a file
 * opened while its prefetch is in flight waits for that read instead of
 * reading again. Documents on screen are leased and never destroyed under
 * the viewer; evicted documents are destroyed.
 */
import type { PDFDocumentLoadingTask, PDFDocumentProxy } from 'pdfjs-dist';
import { pathKey } from '../../library/fileTree';
import { LruCache, type Lease } from './lruCache';
import { openPdf } from './pdfjs';
import { readSourceBytes } from './sourceAccess';
import { cleanPdfTitle, type PdfMeta } from './viewState';

type Entry =
  | { kind: 'bytes'; bytes: Uint8Array }
  | { kind: 'doc'; task: PDFDocumentLoadingTask; doc: PDFDocumentProxy };

const MAX_ENTRIES = 6;
const MAX_BYTES = 150 * 1024 * 1024;
/** Larger files are not read ahead: one would evict most of the cache. */
export const PREFETCH_MAX_BYTES = 48 * 1024 * 1024;

const cache = new LruCache<Entry>({
  maxEntries: MAX_ENTRIES,
  maxBytes: MAX_BYTES,
  dispose: entry => {
    if (entry.kind === 'doc') void entry.task.destroy();
  },
});

/** Byte reads in flight, shared by prefetches and opens. */
const reads = new Map<string, Promise<Uint8Array>>();
/** Opens in flight; a key here is claimed by the foreground. */
const opens = new Map<string, Promise<void>>();

let foregroundLoads = 0;
let idleWaiters: Array<() => void> = [];

function beginForeground(): void {
  foregroundLoads += 1;
}

function endForeground(): void {
  foregroundLoads -= 1;
  if (foregroundLoads > 0) return;
  const waiters = idleWaiters;
  idleWaiters = [];
  for (const resolve of waiters) resolve();
}

/** Resolves when no document is being opened for display. */
export function whenForegroundIdle(): Promise<void> {
  if (foregroundLoads === 0) return Promise.resolve();
  return new Promise(resolve => idleWaiters.push(resolve));
}

export function pdfCacheKey(path: string, size: number): string {
  return `${pathKey(path)}|${size}`;
}

function readShared(key: string, path: string): Promise<Uint8Array> {
  let pending = reads.get(key);
  if (!pending) {
    pending = readSourceBytes(path).finally(() => reads.delete(key));
    reads.set(key, pending);
  }
  return pending;
}

/** A document held open for display; release it when done. */
export interface PdfDocLease {
  doc: PDFDocumentProxy;
  /** The document was already open (no read, no parse). */
  fromCache: boolean;
  release(): void;
}

function fromLease(lease: Lease<Entry>, fromCache: boolean): PdfDocLease {
  const entry = lease.value;
  if (entry.kind !== 'doc') throw new Error('Cached entry is not an open document');
  return { doc: entry.doc, fromCache, release: () => lease.release() };
}

function uncachedLease(task: PDFDocumentLoadingTask, doc: PDFDocumentProxy): PdfDocLease {
  let released = false;
  return {
    doc,
    fromCache: false,
    release: () => {
      if (released) return;
      released = true;
      void task.destroy();
    },
  };
}

async function openUncached(bytes: Uint8Array): Promise<PdfDocLease> {
  const task = await openPdf(bytes);
  try {
    return uncachedLease(task, await task.promise);
  } catch (error) {
    void task.destroy();
    throw error;
  }
}

/** The open document for `path` at `size`, only if it is already cached. */
export function acquireCachedPdf(path: string, size: number): PdfDocLease | null {
  const lease = cache.acquire(pdfCacheKey(path, size));
  if (!lease) return null;
  if (lease.value.kind !== 'doc') {
    lease.release();
    return null;
  }
  return fromLease(lease, true);
}

/**
 * Open a PDF for display. With a known `size` the document is cached and
 * shared; with `null` (no stable identity) it is opened privately and
 * destroyed on release.
 */
export async function acquirePdf(path: string, size: number | null): Promise<PdfDocLease> {
  if (size === null) {
    beginForeground();
    try {
      return await openUncached(await readSourceBytes(path));
    } finally {
      endForeground();
    }
  }

  const key = pdfCacheKey(path, size);
  for (;;) {
    const hit = acquireCachedPdf(path, size);
    if (hit) return hit;
    const pending = opens.get(key);
    if (!pending) break;
    await pending;
  }

  let settle: () => void = () => {};
  opens.set(key, new Promise<void>(resolve => { settle = resolve; }));
  beginForeground();
  let task: PDFDocumentLoadingTask | null = null;
  try {
    let bytes: Uint8Array;
    const cached = cache.peek(key);
    if (cached?.kind === 'bytes') {
      // Taken out: pdf.js detaches the buffer it is given.
      cache.delete(key, false);
      bytes = cached.bytes;
    } else {
      bytes = await readShared(key, path);
    }
    // The file changed between the size check and the read: show it, but do
    // not cache it under a size it no longer has.
    if (bytes.byteLength !== size) return await openUncached(bytes);
    task = await openPdf(bytes);
    const doc = await task.promise;
    const lease = cache.setAndAcquire(key, { kind: 'doc', task, doc }, size);
    task = null;
    return fromLease(lease, false);
  } catch (error) {
    if (task) void task.destroy();
    throw error;
  } finally {
    opens.delete(key);
    settle();
    endForeground();
  }
}

/**
 * Read a PDF's bytes ahead of time (no parsing, no rendering). Does nothing
 * when the file is cached, being opened, or too large; drops the bytes if
 * the prefetch was cancelled while the read was in flight.
 */
export async function prefetchPdfBytes(path: string, size: number, signal: { readonly cancelled: boolean }): Promise<void> {
  if (size <= 0 || size > PREFETCH_MAX_BYTES) return;
  const key = pdfCacheKey(path, size);
  if (cache.has(key) || opens.has(key)) return;
  const bytes = await readShared(key, path);
  if (signal.cancelled || opens.has(key) || cache.has(key) || bytes.byteLength !== size) return;
  cache.set(key, { kind: 'bytes', bytes }, size);
}

function infoTitle(info: unknown): unknown {
  if (typeof info !== 'object' || info === null) return null;
  return (info as Record<string, unknown>).Title ?? null;
}

/** Title (document info), page count and first-page size of an open PDF. */
export async function readPdfMeta(doc: PDFDocumentProxy, size: number): Promise<PdfMeta> {
  const [metadata, first] = await Promise.all([doc.getMetadata().catch(() => null), doc.getPage(1)]);
  const viewport = first.getViewport({ scale: 1 });
  return {
    size,
    title: cleanPdfTitle(infoTitle(metadata?.info)),
    pages: doc.numPages,
    width: viewport.width,
    height: viewport.height,
  };
}
