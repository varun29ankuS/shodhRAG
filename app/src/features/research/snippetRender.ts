/**
 * Rendering a snippet with pdf.js: the region drawn into an offscreen
 * canvas at high scale (never a screenshot of the screen), its PNG, and the
 * page text inside it. Works from an open document (the viewer) or from a
 * file path (a snippet the assistant made, drawn the first time it is
 * shown).
 */

import type { PDFDocumentProxy } from 'pdfjs-dist';
import { acquirePdf } from '../ask/viewer/pdfDocCache';
import { getSourceFileInfo } from '../ask/viewer/sourceAccess';
import { bytesToBase64 } from './api';
import { cropPlan, textBoxes, textInRect } from './snippetGeometry';
import type { SnippetRect } from './types';

/** Pixels per PDF point the snippet image is drawn at (before the caps). */
export const SNIPPET_RENDER_SCALE = 4;

export interface RenderedSnippet {
  /** Base64 PNG without a data: prefix. */
  png: string;
  width: number;
  height: number;
}

/** The page's view box (`page.view`). */
export async function pageView(doc: PDFDocumentProxy, pageNumber: number): Promise<number[]> {
  const page = await doc.getPage(pageNumber);
  return [...page.view];
}

/** The region of a page as a PNG drawn by pdf.js. */
export async function renderSnippet(doc: PDFDocumentProxy, pageNumber: number, rect: SnippetRect): Promise<RenderedSnippet> {
  const page = await doc.getPage(pageNumber);
  const unit = page.getViewport({ scale: 1 });
  const dpr = typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1;
  const plan = cropPlan(rect, page.view, unit.transform, {
    targetScale: Math.max(SNIPPET_RENDER_SCALE, dpr * 3),
    maxSide: 4096,
    maxPixels: 16_000_000,
  });
  if (!plan) throw new Error('The region is empty.');
  const viewport = page.getViewport({ scale: plan.scale, offsetX: plan.offsetX, offsetY: plan.offsetY });
  const canvas = document.createElement('canvas');
  canvas.width = plan.width;
  canvas.height = plan.height;
  const context = canvas.getContext('2d');
  if (!context) throw new Error('The image could not be drawn (no canvas).');
  // A white page behind transparent PDFs, as on screen.
  context.fillStyle = '#ffffff';
  context.fillRect(0, 0, plan.width, plan.height);
  await page.render({ canvas, canvasContext: context, viewport }).promise;
  const blob = await new Promise<Blob | null>(resolve => canvas.toBlob(resolve, 'image/png'));
  if (!blob) throw new Error('The image could not be encoded.');
  const png = bytesToBase64(new Uint8Array(await blob.arrayBuffer()));
  return { png, width: plan.width, height: plan.height };
}

/** The page text inside the region (the rule shared with the backend). */
export async function snippetText(doc: PDFDocumentProxy, pageNumber: number, rect: SnippetRect): Promise<string> {
  const page = await doc.getPage(pageNumber);
  const content = await page.getTextContent();
  return textInRect(textBoxes(content.items), rect, page.view);
}

/** Runs `use` with the file's PDF open (from the shared cache when possible). */
export async function withPdf<T>(filePath: string, use: (doc: PDFDocumentProxy) => Promise<T>): Promise<T> {
  let size: number | null = null;
  let modified: number | null = null;
  try {
    const info = await getSourceFileInfo(filePath);
    size = info.sizeBytes;
    modified = info.modifiedMs ?? null;
  } catch {
    // Opened privately below; access errors surface from the read.
  }
  const lease = await acquirePdf(filePath, size, modified);
  try {
    return await use(lease.doc);
  } finally {
    lease.release();
  }
}

/** A data: URL for a base64 PNG. */
export function pngDataUrl(png: string): string {
  return `data:image/png;base64,${png}`;
}
