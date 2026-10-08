/**
 * The app's PDF document cache: `docStore` wired to pdf.js and the source
 * viewer commands. Up to 4 entries (opened documents and prefetched bytes
 * share the count) and 64 MB of file bytes (each open document's parsed
 * state in the pdf.js worker is a multiple of its file size), keyed by path,
 * file size and modification time (`SourceFileInfo.modifiedMs`), so an
 * edited file is never served from the cache even when its size did not
 * change.
 */
import type { PDFDocumentProxy } from 'pdfjs-dist';
import { pathKey } from '../../library/fileTree';
import { createDocStore, type DocLease } from './docStore';
import { openPdf } from './pdfjs';
import { readSourceBytes } from './sourceAccess';
import { cleanPdfTitle, type PdfMeta } from './viewState';

export type PdfDocLease = DocLease<PDFDocumentProxy>;

export const PREFETCH_MAX_BYTES = 32 * 1024 * 1024;

export function pdfCacheKey(path: string, size: number, modifiedMs: number | null = null): string {
  return `${pathKey(path)}|${size}|${modifiedMs ?? ''}`;
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
  maxEntries: 4,
  maxBytes: 64 * 1024 * 1024,
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
export async function readPdfMeta(doc: PDFDocumentProxy, size: number, modified: number | null = null): Promise<PdfMeta> {
  const [metadata, first] = await Promise.all([doc.getMetadata().catch(() => null), doc.getPage(1)]);
  const viewport = first.getViewport({ scale: 1 });
  return {
    size,
    modified,
    title: cleanPdfTitle(infoTitle(metadata?.info)),
    pages: doc.numPages,
    width: viewport.width,
    height: viewport.height,
  };
}
