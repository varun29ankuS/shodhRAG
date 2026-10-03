/**
 * Title and page count of the PDFs visible in the file list, loaded lazily
 * in the background and remembered across sessions.
 *
 * The viewer commands have no metadata-only read, so reading a title costs
 * a full byte read and a pdf.js open. To keep that from slowing browsing:
 * only rows on screen are requested (and dropped when scrolled away), one at
 * a time, after a short settle delay, never while a document is being opened
 * or its first page drawn, and never for files above `META_MAX_BYTES` (those
 * get their metadata when the reader opens them). Parsing runs on pdf.js's background
 * worker, so it never queues ahead of a page render. Documents already open
 * in the cache are read for free, and each file is checked once per session.
 */
import { useSyncExternalStore } from 'react';
import { acquireCachedPdf, readPdfMeta, whenForegroundIdle } from '../ask/viewer/pdfDocCache';
import { openPdf } from '../ask/viewer/pdfjs';
import { getSourceFileInfo, readSourceBytes } from '../ask/viewer/sourceAccess';
import { getPdfMeta, rememberPdfMeta, subscribePdfMeta } from '../ask/viewer/viewerStores';
import type { PdfMeta } from '../ask/viewer/viewState';
import { pathKey } from './fileTree';
import { Prefetcher, type PrefetchSignal, type PrefetchTimers } from './prefetch';

/** Larger PDFs are not read just for a list row. */
const META_MAX_BYTES = 40 * 1024 * 1024;
/** Rows must stay on screen this long before their file is read. */
const SETTLE_MS = 250;

export const browserTimers: PrefetchTimers = {
  setTimeout: (callback, ms) => window.setTimeout(callback, ms),
  clearTimeout: handle => window.clearTimeout(handle as number),
};

const queue = new Prefetcher(browserTimers, 1);
/** Files whose metadata is known to be current (or not worth reading) this session. */
const settled = new Set<string>();

async function loadMeta(path: string, signal: PrefetchSignal): Promise<void> {
  await whenForegroundIdle();
  if (signal.cancelled) return;
  const info = await getSourceFileInfo(path);
  if (signal.cancelled) return;
  const known = getPdfMeta(path);
  if (info.kind !== 'pdf' || info.sizeBytes > META_MAX_BYTES || (known && known.size === info.sizeBytes)) return;

  let meta: PdfMeta;
  const cached = acquireCachedPdf(path, info.sizeBytes);
  if (cached) {
    try {
      meta = await readPdfMeta(cached.doc, info.sizeBytes);
    } finally {
      cached.release();
    }
  } else {
    await whenForegroundIdle();
    if (signal.cancelled) return;
    const bytes = await readSourceBytes(path);
    if (signal.cancelled) return;
    const task = await openPdf(bytes, { background: true });
    try {
      meta = await readPdfMeta(await task.promise, info.sizeBytes);
    } finally {
      void task.destroy();
    }
  }
  rememberPdfMeta(path, meta);
}

/**
 * The PDFs whose rows are on screen now. Requests for rows that scrolled
 * away are cancelled.
 */
export function requestPdfMeta(paths: readonly string[]): void {
  const wanted = new Map<string, string>();
  for (const path of paths) {
    const key = pathKey(path);
    if (!settled.has(key)) wanted.set(key, path);
  }
  queue.retain(wanted.keys());
  for (const [key, path] of wanted) {
    queue.schedule(key, SETTLE_MS, async signal => {
      try {
        await loadMeta(path, signal);
        if (!signal.cancelled) settled.add(key);
      } catch {
        // Unreadable or not a PDF: the row keeps its file name.
        settled.add(key);
      }
    });
  }
}

/** Remembered metadata of a PDF row; updates when it is loaded. */
export function usePdfMeta(path: string | null): PdfMeta | null {
  return useSyncExternalStore(subscribePdfMeta, () => (path ? getPdfMeta(path) : null));
}
