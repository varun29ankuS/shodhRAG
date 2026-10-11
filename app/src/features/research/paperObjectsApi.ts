/**
 * The `paper_objects` command (figures and equations of an indexed PDF) and
 * figure images cropped from the PDF with pdf.js.
 *
 * Both are cached: an answer re-renders on every streamed token, and the
 * paper page and the pop-out show the same figures, so a paper is read once
 * and a crop is drawn once (until the file changes, which changes the
 * backend's answer and the crop key).
 */

import { invoke } from '@tauri-apps/api/core';
import { LruCache } from '../ask/viewer/lruCache';
import { toResearchError } from './api';
import { readPaperParts } from './paperObjects';
import type { PaperParts, PdfBoxCorners } from './paperObjects';
import { boxToRect } from './snippetGeometry';
import { renderSnippet, withPdf } from './snippetRender';

const parts = new Map<string, Promise<PaperParts>>();
const MAX_PAPERS = 8;

/** Figures and equations of the indexed PDF at `path` (shared, cached per path). */
export function paperParts(path: string): Promise<PaperParts> {
  const key = path.trim();
  const cached = parts.get(key);
  if (cached) {
    parts.delete(key);
    parts.set(key, cached);
    return cached;
  }
  const request = invoke<unknown>('paper_objects', { path: key }).then(value => {
    const read = readPaperParts(value);
    if (!read) throw { code: 'storage', message: 'The paper’s figures could not be read.' };
    return read;
  });
  parts.set(key, request);
  // A failure is not kept: the next render asks again (the index may have caught up).
  request.catch(() => {
    if (parts.get(key) === request) parts.delete(key);
  });
  while (parts.size > MAX_PAPERS) {
    const oldest = parts.keys().next().value;
    if (oldest === undefined) break;
    parts.delete(oldest);
  }
  return request;
}

/** Forget a paper (it changed on disk or was re-indexed). */
export function forgetPaperParts(path?: string): void {
  if (path) parts.delete(path.trim());
  else parts.clear();
}

export interface FigureImage {
  url: string;
  width: number;
  height: number;
}

const images = new LruCache<FigureImage>({
  maxEntries: 48,
  maxBytes: 96 * 1024 * 1024,
  dispose: () => undefined,
});
const pending = new Map<string, Promise<FigureImage>>();

function imageKey(path: string, page: number, box: PdfBoxCorners, scale: number): string {
  const r = (v: number) => Math.round(v * 10) / 10;
  return `${path}|${page}|${r(box.x0)},${r(box.y0)},${r(box.x1)},${r(box.y1)}|${scale}`;
}

/**
 * The region of a page as an image (data URL), drawn by pdf.js at high
 * resolution. `scale` "thumb" draws smaller for grids.
 */
export function figureImage(path: string, page: number, box: PdfBoxCorners, scale: 'full' | 'thumb' = 'full'): Promise<FigureImage> {
  const key = imageKey(path, page, box, scale === 'full' ? 1 : 0);
  const hit = images.get(key);
  if (hit) return Promise.resolve(hit);
  const running = pending.get(key);
  if (running) return running;
  const job = withPdf(path, async doc => {
    const pdfPage = await doc.getPage(page);
    const rect = boxToRect(box, pdfPage.view);
    const rendered = await renderSnippet(doc, page, rect, scale === 'thumb' ? 2 : undefined);
    return { url: `data:image/png;base64,${rendered.png}`, width: rendered.width, height: rendered.height };
  })
    .then(image => {
      images.set(key, image, image.url.length);
      return image;
    })
    .finally(() => pending.delete(key));
  pending.set(key, job);
  return job;
}

/** A readable message for a failed figure. */
export function figureError(error: unknown): string {
  return toResearchError(error).message;
}
