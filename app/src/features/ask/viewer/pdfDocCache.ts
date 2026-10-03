/**
 * The app's PDF document cache: `docStore` wired to pdf.js and the source
 * viewer commands. Up to 6 entries (opened documents and prefetched bytes
 * share the count) and 150 MB, keyed by path and file size (the viewer
 * commands expose no modification time).
 */
import type { PDFDocumentProxy } from 'pdfjs-dist';
import { pathKey } from '../../library/fileTree';
import { createDocStore, type DocLease } from './docStore';
import { openPdf } from './pdfjs';
import { readSourceBytes } from './sourceAccess';
import { cleanPdfTitle, type PdfMeta } from './viewState';

export type PdfDocLease = DocLease<PDFDocumentProxy>;

export const PREFETCH_MAX_BYTES = 48 * 1024 * 1024;

export function pdfCacheKey(path: string, size: number): string {
  return `${pathKey(path)}|${size}`;
}

const store = createDocStore<PDFDocumentProxy>({
  read: readSourceBytes,
  open: async bytes => {
    const task = await openPdf(bytes);
    try {
      return { doc: await task.promise, destroy: () => void task.destroy() };
    } catch (error) {
      void task.destroy();
      throw error;
    }
  },
  keyOf: pdfCacheKey,
  maxEntries: 6,
  maxBytes: 150 * 1024 * 1024,
  prefetchMaxBytes: PREFETCH_MAX_BYTES,
});

/** Open a PDF for display (cached when `size` is known). */
export const acquirePdf = store.acquire;
/** The open document, only if it is already cached. */
export const acquireCachedPdf = store.acquireCached;
/** Read a PDF's bytes ahead (no parsing, no rendering). */
export const prefetchPdfBytes = store.prefetch;
/** Mark display work (opening, first paint) so background loads wait. */
export const holdForeground = store.holdForeground;
export const whenForegroundIdle = store.whenForegroundIdle;

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
