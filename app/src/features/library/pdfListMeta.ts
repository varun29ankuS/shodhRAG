/**
 * Title and page count of the PDFs visible in the file list, loaded lazily
 * in the background and remembered across sessions.
 *
 * Titles come from `get_pdf_info`, which reads the PDF's object tree in the
 * backend (no page content, no text) instead of sending the bytes to pdf.js.
 * Only rows on screen are requested (and dropped when scrolled away), one at
 * a time, after a short settle delay, never while a document is being opened
 * or its first page drawn, and never for files above `META_MAX_BYTES` (those
 * get their metadata when the reader opens them). Documents already open in
 * the cache are read for free, and each file is checked once per session.
 * A PDF the backend cannot read (malformed files pdf.js tolerates) falls back
 * to a background pdf.js open. Remembered metadata is reused only while the
 * file's size and modification time are unchanged.
 */
import { useSyncExternalStore } from 'react';
import { acquireCachedPdf, readPdfMeta, whenForegroundIdle } from '../ask/viewer/pdfDocCache';
import { openPdf } from '../ask/viewer/pdfjs';
import { getPdfInfo, getSourceFileInfo, readSourceBytes } from '../ask/viewer/sourceAccess';
import { getPdfMeta, rememberPdfMeta, subscribePdfMeta } from '../ask/viewer/viewerStores';
import { cleanPdfTitle } from '../ask/viewer/viewState';
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
  const current = known && known.size === info.sizeBytes && known.modified === info.modifiedMs;
  if (info.kind !== 'pdf' || info.sizeBytes > META_MAX_BYTES || current) return;

  let meta: PdfMeta | null = null;
  const cached = acquireCachedPdf(path, info.sizeBytes, info.modifiedMs);
  if (cached) {
    try {
      meta = await readPdfMeta(cached.doc, info.sizeBytes, info.modifiedMs);
    } finally {
      cached.release();
    }
  } else {
    meta = await backendMeta(path, info.sizeBytes, info.modifiedMs);
  }
  if (signal.cancelled) return;
  if (!meta) {
    await whenForegroundIdle();
    if (signal.cancelled) return;
    const bytes = await readSourceBytes(path);
    if (signal.cancelled) return;
    const task = await openPdf(bytes, { background: true });
    try {
      meta = await readPdfMeta(await task.promise, info.sizeBytes, info.modifiedMs);
    } finally {
      void task.destroy();
    }
  }
  rememberPdfMeta(path, meta);
}

/** Metadata from the backend's PDF reader, or null when it cannot supply all of it. */
async function backendMeta(path: string, size: number, modified: number | null): Promise<PdfMeta | null> {
  try {
    const info = await getPdfInfo(path);
    const { firstPageWidth: width, firstPageHeight: height } = info;
    if (info.pageCount < 1 || !width || !height || width <= 0 || height <= 0) return null;
    return { size, modified, title: cleanPdfTitle(info.title), pages: info.pageCount, width, height };
  } catch {
    return null;
  }
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
