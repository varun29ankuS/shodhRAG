/**
 * Lazily loaded pdf.js with a bundled worker and locally served runtime data
 * (see the `shodh-pdfjs-assets` plugin in vite.config.ts). Nothing is fetched
 * from a CDN, so PDFs render offline.
 */
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
import type { PDFDocumentLoadingTask } from 'pdfjs-dist';

type PdfJs = typeof import('pdfjs-dist');

let modulePromise: Promise<PdfJs> | null = null;

export function loadPdfJs(): Promise<PdfJs> {
  if (!modulePromise) {
    modulePromise = import('pdfjs-dist')
      .then(mod => {
        mod.GlobalWorkerOptions.workerSrc = workerUrl;
        return mod;
      })
      .catch(error => {
        // Allow a later attempt (e.g. after a dev-server reload) to retry.
        modulePromise = null;
        throw error;
      });
  }
  return modulePromise;
}

function assetDir(dir: string): string {
  return new URL(`${import.meta.env.BASE_URL}pdfjs/${dir}/`, window.location.href).href;
}

type PdfWorker = InstanceType<PdfJs['PDFWorker']>;

const workers: { display: PdfWorker | null; background: PdfWorker | null } = { display: null, background: null };

/**
 * Two long-lived pdf.js workers: one for documents on screen, one for
 * background reads (file-list metadata), so background parsing never queues
 * ahead of a page render. Without them each `getDocument` spawns (and on
 * destroy terminates) its own Web Worker, so every file open paid worker
 * start-up and every cached document held a thread. Destroying a document
 * opened on a supplied worker leaves the worker running.
 */
function workerFor(pdfjs: PdfJs, lane: keyof typeof workers): PdfWorker {
  let worker = workers[lane];
  if (!worker || worker.destroyed) {
    worker = new pdfjs.PDFWorker();
    workers[lane] = worker;
  }
  return worker;
}

export interface OpenPdfOptions {
  /** Parse on the background worker (work nobody is waiting to see). */
  background?: boolean;
}

/** Open a PDF from bytes. The buffer is transferred to the worker. */
export async function openPdf(data: Uint8Array, options: OpenPdfOptions = {}): Promise<PDFDocumentLoadingTask> {
  const pdfjs = await loadPdfJs();
  return pdfjs.getDocument({
    data,
    worker: workerFor(pdfjs, options.background ? 'background' : 'display'),
    cMapUrl: assetDir('cmaps'),
    cMapPacked: true,
    standardFontDataUrl: assetDir('standard_fonts'),
    wasmUrl: assetDir('wasm'),
    iccUrl: assetDir('iccs'),
  });
}

/** True for the rejection pdf.js raises when a render is cancelled. */
export function isRenderCancelled(error: unknown): boolean {
  return typeof error === 'object' && error !== null && (error as { name?: unknown }).name === 'RenderingCancelledException';
}
