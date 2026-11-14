/**
 * Page images from the computer (`read_source_page`): a PDF page or an image,
 * rendered on the computer at the width the phone shows it. The computer
 * reads the whole file for each page, so at most two are asked for at once,
 * and the pages shown recently are kept in memory while the app runs
 * (never written to the phone, never a copy in its library).
 */
import { readPage } from './desktop';
import type { Shim } from './invoke';

/** Files the computer renders as pages; any other kind shows as its type. */
const PAGED = new Set(['pdf', 'png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'tif', 'tiff']);

export function extensionOf(path: string): string {
  const name = path.split(/[/\\]/).pop() ?? '';
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

export function hasPages(path: string): boolean {
  return PAGED.has(extensionOf(path));
}

/** Narrowest and widest page the computer renders (phone_link.rs). */
export const MIN_WIDTH = 320;
export const MAX_WIDTH = 1600;

/** A thumbnail's width in pixels: the narrowest page the computer renders. */
export const THUMB_WIDTH = MIN_WIDTH;

/** A full-width page's width in pixels on this phone. */
export function readerWidth(): number {
  const ratio = typeof window.devicePixelRatio === 'number' && window.devicePixelRatio > 0 ? window.devicePixelRatio : 2;
  return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, Math.round(window.innerWidth * ratio)));
}

export interface Page {
  /** A `data:` URL of the PNG. */
  src: string;
  width: number;
  height: number;
  pages: number;
}

const KEPT = 48;
const AT_ONCE = 2;
const kept = new Map<string, Promise<Page>>();
let running = 0;
const queue: Array<() => void> = [];

function next() {
  while (running < AT_ONCE && queue.length > 0) {
    running += 1;
    queue.shift()?.();
  }
}

function limited<T>(task: () => Promise<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    queue.push(() => {
      task()
        .then(resolve, reject)
        .finally(() => {
          running -= 1;
          next();
        });
    });
    next();
  });
}

/** Page `page` (1-based) of `path`, `width` pixels wide at most. */
export function pageImage(desktop: Shim, path: string, page: number, width: number): Promise<Page> {
  const key = `${path}\n${page}\n${width}`;
  const hit = kept.get(key);
  if (hit) {
    // Most recently used last.
    kept.delete(key);
    kept.set(key, hit);
    return hit;
  }
  const loading = limited(() => readPage(desktop, path, page, width)).then(
    (image): Page => ({ src: `data:image/png;base64,${image.png}`, width: image.width, height: image.height, pages: image.pages }),
  );
  kept.set(key, loading);
  loading.catch(() => kept.delete(key));
  while (kept.size > KEPT) {
    const oldest = kept.keys().next().value;
    if (oldest === undefined) break;
    kept.delete(oldest);
  }
  return loading;
}

/** Forget every kept page (tests, and a different computer). */
export function forgetPages() {
  kept.clear();
}
