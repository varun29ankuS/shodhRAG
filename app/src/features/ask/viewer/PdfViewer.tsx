import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { PDFDocumentProxy, PDFPageProxy, RenderTask } from 'pdfjs-dist';
import { ChevronDown, ChevronUp, Loader2, Minus, MoveHorizontal, Plus, Search, X } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { pathKey } from '../../library/fileTree';
import type { PageSpan } from '../types';
import { findPassage, matchScore, prepareHaystack, type PassageMatch, type PreparedHaystack, type TextRange } from './passageMatch';
import { acquirePdf, holdForeground, readPdfMeta, type PdfDocLease } from './pdfDocCache';
import { isRenderCancelled, loadPdfJs } from './pdfjs';
import { scrollBehavior } from './sourceAccess';
import { findInText, viewerCommand } from './viewerKeys';
import { getPdfMeta, pdfViewStates, rememberPdfMeta } from './viewerStores';
import { MAX_PDF_SCALE, MIN_PDF_SCALE, type PdfViewState, type PdfZoom } from './viewState';
import type { LocateResult } from './viewerTypes';
import { VIEWER_FOCUS_RING } from './viewerTypes';

type TextContent = Awaited<ReturnType<PDFPageProxy['getTextContent']>>;
type TextLayerInstance = InstanceType<Awaited<ReturnType<typeof loadPdfJs>>['TextLayer']>;

interface PageText {
  content: TextContent;
  /** `str` of every text item, in the order pdf.js's TextLayer creates spans. */
  strings: string[];
  haystack: PreparedHaystack;
}

interface PageSize {
  width: number;
  height: number;
}

interface HighlightTarget {
  page: number;
  ranges: TextRange[];
  token: number;
}

interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

interface FindHit {
  page: number;
  start: number;
  end: number;
}

/** Dispatch on a `[data-pdf-viewer]` element to open its find bar. */
export const VIEWER_FIND_EVENT = 'shodh:viewer-find';

/** Cap on canvas backing-store pixels per page (memory guard at high DPR). */
const MAX_CANVAS_PIXELS = 16_000_000;
const MIN_SCALE = MIN_PDF_SCALE;
const MAX_SCALE = MAX_PDF_SCALE;
const ZOOM_STEP = 1.2;
const PAGE_GAP = 12;
const PAGE_PADDING = 16;
/** A page match this good ends the search early. */
const CONFIDENT_COVERAGE = 0.9;
/** The reading position is written to storage at most this often. */
const SAVE_DELAY_MS = 400;
const FIND_DEBOUNCE_MS = 150;
/** Skeleton text lines (width %) drawn on a page that is not loaded yet. */
const SKELETON_LINES = [62, 88, 94, 90, 72, 0, 91, 86, 93, 58];

/** User-timing measures (DevTools Performance panel, `performance.getEntriesByName`). */
const MEASURE_FIRST_PAINT = 'shodh:pdf open→first paint';
const MEASURE_SHARP = 'shodh:pdf open→sharp first page';

function clampScale(scale: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

function isEditableTarget(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('input, textarea, select, [contenteditable=""], [contenteditable="true"]') !== null;
}

/** Record a measure from `startMark`, keeping only the latest of its name. */
function measureFrom(name: string, startMark: string, detail: Record<string, unknown>): void {
  if (typeof performance === 'undefined' || performance.getEntriesByName(startMark, 'mark').length === 0) return;
  performance.clearMeasures(name);
  performance.measure(name, { start: startMark, detail });
}

const sameHit = (a: FindHit | null, b: FindHit) => a !== null && a.page === b.page && a.start === b.start;

/** Merge per-span rects that sit on the same line and touch. */
function mergeRects(rects: Rect[]): Rect[] {
  const sorted = [...rects].sort((a, b) => a.top - b.top || a.left - b.left);
  const out: Rect[] = [];
  for (const r of sorted) {
    const prev = out[out.length - 1];
    if (
      prev &&
      Math.abs(prev.top - r.top) < Math.max(2, prev.height * 0.4) &&
      Math.abs(prev.height - r.height) < Math.max(3, prev.height * 0.5) &&
      r.left <= prev.left + prev.width + 6
    ) {
      const right = Math.max(prev.left + prev.width, r.left + r.width);
      const bottom = Math.max(prev.top + prev.height, r.top + r.height);
      prev.left = Math.min(prev.left, r.left);
      prev.top = Math.min(prev.top, r.top);
      prev.width = right - prev.left;
      prev.height = bottom - prev.top;
    } else {
      out.push({ ...r });
    }
  }
  return out;
}

interface PdfPageViewProps {
  /** Null while the document is loading: the page shows a skeleton. */
  doc: PDFDocumentProxy | null;
  pageNumber: number;
  scale: number;
  size: PageSize;
  active: boolean;
  /** Draw a low-resolution pass first (read when rendering starts). */
  quick: boolean;
  getText: (page: number) => Promise<PageText>;
  ranges: TextRange[] | null;
  highlightToken: number | null;
  /** `top` is the first highlight's offset in the page, or null if none could be drawn. */
  onHighlightRendered: (page: number, token: number, top: number | null) => void;
  onPainted: (page: number, sharp: boolean) => void;
  registerElement: (page: number, el: HTMLDivElement | null) => void;
}

function PdfPageView({
  doc,
  pageNumber,
  scale,
  size,
  active,
  quick,
  getText,
  ranges,
  highlightToken,
  onHighlightRendered,
  onPainted,
  registerElement,
}: PdfPageViewProps) {
  const pageRef = useRef<HTMLDivElement>(null);
  const canvasHostRef = useRef<HTMLDivElement>(null);
  const textHostRef = useRef<HTMLDivElement>(null);
  const [layer, setLayer] = useState<{ divs: HTMLElement[]; strings: string[] } | null>(null);
  /** Canvas state: drawing (spinner), drawn, or failed (message). */
  const [stage, setStage] = useState<'idle' | 'drawing' | 'drawn' | 'failed'>('idle');
  /** Text layer could not be used for highlighting (failed or misaligned). */
  const [layerUnusable, setLayerUnusable] = useState(false);
  /** null until computed for the current layer and ranges. */
  const [rects, setRects] = useState<Rect[] | null>(null);
  const quickRef = useRef(quick);
  quickRef.current = quick;
  const hasCanvas = useRef(false);

  const setPageRef = useCallback(
    (el: HTMLDivElement | null) => {
      pageRef.current = el;
      registerElement(pageNumber, el);
    },
    [pageNumber, registerElement],
  );

  useEffect(() => {
    const canvasHost = canvasHostRef.current;
    const textHost = textHostRef.current;
    if (!active || !doc || !canvasHost || !textHost) {
      canvasHost?.replaceChildren();
      textHost?.replaceChildren();
      hasCanvas.current = false;
      setLayer(null);
      setLayerUnusable(false);
      setStage('idle');
      return;
    }
    let cancelled = false;
    let renderTask: RenderTask | null = null;
    let textLayer: TextLayerInstance | null = null;
    let drawn = false;
    setStage(prev => (prev === 'drawn' ? 'drawn' : 'drawing'));
    setLayerUnusable(false);

    (async () => {
      const pdfjs = await loadPdfJs();
      const page = await doc.getPage(pageNumber);
      if (cancelled) return;
      const viewport = page.getViewport({ scale });
      const sharpRatio = Math.max(
        0.5,
        Math.min(window.devicePixelRatio || 1, Math.sqrt(MAX_CANVAS_PIXELS / Math.max(1, viewport.width * viewport.height))),
      );
      // A fresh canvas per render: pdf.js refuses concurrent renders on one
      // canvas, and swapping on completion avoids a blank flash while zooming.
      const draw = async (ratio: number): Promise<HTMLCanvasElement> => {
        const canvas = document.createElement('canvas');
        canvas.width = Math.max(1, Math.floor(viewport.width * ratio));
        canvas.height = Math.max(1, Math.floor(viewport.height * ratio));
        canvas.setAttribute('aria-hidden', 'true');
        const context = canvas.getContext('2d');
        if (!context) throw new Error('Canvas 2D context unavailable');
        const task = page.render({
          canvas,
          canvasContext: context,
          viewport,
          transform: ratio !== 1 ? [ratio, 0, 0, ratio, 0, 0] : undefined,
        });
        renderTask = task;
        await task.promise;
        return canvas;
      };

      // Progressive first paint: a low-resolution pass shows the page as soon
      // as its operator list is ready; the sharp pass reuses that list and
      // only rasterizes again.
      if (quickRef.current && !hasCanvas.current) {
        const preview = await draw(Math.min(sharpRatio / 2, 0.75));
        if (cancelled) return;
        canvasHost.replaceChildren(preview);
        hasCanvas.current = true;
        drawn = true;
        setStage('drawn');
        onPainted(pageNumber, false);
      }

      const canvas = await draw(sharpRatio);
      if (cancelled) return;
      canvasHost.replaceChildren(canvas);
      hasCanvas.current = true;
      drawn = true;
      setStage('drawn');
      onPainted(pageNumber, true);

      const text = await getText(pageNumber);
      if (cancelled) return;
      const container = document.createElement('div');
      container.className = 'pdf-text-layer';
      textLayer = new pdfjs.TextLayer({ textContentSource: text.content, container, viewport });
      await textLayer.render();
      if (cancelled) return;
      textHost.replaceChildren(container);
      const divs = textLayer.textDivs;
      const strings = textLayer.textContentItemsStr;
      // Highlight offsets are computed on `text.strings`; only trust the
      // layer when it produced exactly the same runs.
      const aligned = divs.length === strings.length && strings.length === text.strings.length && strings.every((s, i) => s === text.strings[i]);
      setLayer(aligned ? { divs, strings } : null);
      setLayerUnusable(!aligned);
    })().catch(error => {
      if (cancelled || isRenderCancelled(error)) return;
      // A text-layer failure after the canvas was drawn leaves the page
      // readable; only highlighting is lost.
      if (drawn) {
        setLayer(null);
        setLayerUnusable(true);
      } else {
        setStage('failed');
      }
    });

    return () => {
      cancelled = true;
      (renderTask as RenderTask | null)?.cancel();
      textLayer?.cancel();
    };
  }, [active, doc, pageNumber, scale, getText, onPainted]);

  useLayoutEffect(() => {
    const pageEl = pageRef.current;
    if (!layer || !ranges || ranges.length === 0 || !pageEl) {
      setRects(null);
      return;
    }
    const origin = pageEl.getBoundingClientRect();
    const domRange = document.createRange();
    const found: Rect[] = [];
    let offset = 0;
    layer.strings.forEach((str, index) => {
      const start = offset;
      const end = offset + str.length;
      offset = end;
      const node = layer.divs[index]?.firstChild;
      if (!node || node.nodeType !== Node.TEXT_NODE) return;
      const length = (node as Text).length;
      for (const r of ranges) {
        const a = Math.max(r.start, start);
        const b = Math.min(r.end, end);
        if (a >= b) continue;
        domRange.setStart(node, Math.min(a - start, length));
        domRange.setEnd(node, Math.min(b - start, length));
        for (const rect of Array.from(domRange.getClientRects())) {
          if (rect.width < 0.5 || rect.height < 0.5) continue;
          found.push({ left: rect.left - origin.left, top: rect.top - origin.top, width: rect.width, height: rect.height });
        }
      }
    });
    domRange.detach();
    setRects(mergeRects(found));
  }, [layer, ranges]);

  useEffect(() => {
    if (highlightToken === null || !ranges || ranges.length === 0) return;
    if (layerUnusable) {
      onHighlightRendered(pageNumber, highlightToken, null);
    } else if (rects !== null) {
      onHighlightRendered(pageNumber, highlightToken, rects.length > 0 ? rects.reduce((m, r) => Math.min(m, r.top), Infinity) : null);
    }
  }, [rects, layerUnusable, ranges, highlightToken, pageNumber, onHighlightRendered]);

  return (
    <div
      ref={setPageRef}
      data-page={pageNumber}
      className="pdf-page"
      style={{ width: size.width, height: size.height, ['--total-scale-factor' as string]: String(scale) }}
      role="region"
      aria-label={`Page ${pageNumber}`}
      aria-busy={!doc || stage === 'drawing' ? true : undefined}
    >
      {!doc && (
        <div
          className="absolute inset-0 flex flex-col pointer-events-none"
          style={{ padding: '9% 11%', gap: Math.max(4, size.height * 0.018) }}
          aria-hidden="true"
        >
          {SKELETON_LINES.map((width, i) => (
            <span key={i} className="pdf-skeleton-line" style={{ width: `${width}%`, height: Math.max(3, size.height * 0.011) }} />
          ))}
        </div>
      )}
      <div ref={canvasHostRef} className="absolute inset-0" />
      <div ref={textHostRef} className="absolute inset-0" />
      {rects && rects.length > 0 && (
        <div className="pdf-highlight-layer" aria-hidden="true">
          {rects.map((r, i) => (
            <div key={i} className="pdf-highlight" style={{ left: r.left - 1, top: r.top - 1, width: r.width + 2, height: r.height + 2 }} />
          ))}
        </div>
      )}
      {doc && active && stage === 'drawing' && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none" aria-hidden="true">
          <Loader2 className="w-5 h-5 animate-spin motion-reduce:animate-none text-zinc-500" />
        </div>
      )}
      {stage === 'failed' && (
        <div className="absolute inset-0 flex items-center justify-center p-4 text-center text-[12.5px] text-zinc-700">
          Page {pageNumber} could not be rendered.
        </div>
      )}
    </div>
  );
}

interface PdfViewerProps {
  filePath: string;
  /**
   * Size of the file in bytes. A number caches the opened document under
   * path + size (instant reopen, prefetch reuse); `'pending'` shows the
   * remembered page skeleton and waits; null or omitted opens privately.
   */
  fileSize?: number | 'pending' | null;
  passage: string;
  citedPages: PageSpan | null;
  onLocate: (result: LocateResult) => void;
  onFatal: (error: unknown) => void;
  /** Restore and remember the reading position and zoom for this file. */
  rememberView?: boolean;
  /** Called once, when the first page is on screen. */
  onFirstPageVisible?: () => void;
  /** The page the reader is on changed (scrolling, keys or the page field). */
  onPageChange?: (page: number) => void;
}

/**
 * Renders a PDF with pdf.js (canvas + text layer), lazily per visible page,
 * and highlights the cited passage on the page it was found. Documents come
 * from a shared cache, the remembered page layout shows as a skeleton before
 * the file is read, and the first page paints at low resolution first.
 *
 * Keyboard (focus inside the viewer): PageUp/PageDown page, Home/End first
 * and last page, + / - zoom, 0 fit width, Ctrl/⌘+F find, F3 next match.
 */
export function PdfViewer({
  filePath,
  fileSize = null,
  passage,
  citedPages,
  onLocate,
  onFatal,
  rememberView = false,
  onFirstPageVisible,
  onPageChange,
}: PdfViewerProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const findInputRef = useRef<HTMLInputElement>(null);
  const pageEls = useRef(new Map<number, HTMLDivElement>());
  const textCache = useRef(new Map<number, Promise<PageText>>());
  const viewKey = pathKey(filePath);
  const pageInputId = useId();
  const findInputId = useId();

  // Where to start: the remembered position when browsing a file; nothing
  // when a citation decides the page.
  const [initialView] = useState<PdfViewState | null>(() =>
    rememberView && !passage.trim() && !citedPages ? pdfViewStates.get(viewKey) : null,
  );
  // The remembered page count and size draw the skeleton before the file is read.
  const [sizes, setSizes] = useState<PageSize[]>(() => {
    const meta = getPdfMeta(filePath);
    return meta ? Array.from({ length: meta.pages }, () => ({ width: meta.width, height: meta.height })) : [];
  });
  const [doc, setDoc] = useState<PDFDocumentProxy | null>(null);
  const [zoom, setZoom] = useState<PdfZoom>(() => initialView?.zoom ?? { mode: 'fit' });
  const [fitScale, setFitScale] = useState(1);
  const [fitReady, setFitReady] = useState(false);
  const [visible, setVisible] = useState<Set<number>>(() => new Set());
  const [currentPage, setCurrentPage] = useState(() => initialView?.page ?? 1);
  const [pageInput, setPageInput] = useState(String(initialView?.page ?? 1));
  const [target, setTarget] = useState<HighlightTarget | null>(null);
  const [firstPainted, setFirstPainted] = useState(false);
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState('');
  const [findHits, setFindHits] = useState<FindHit[]>([]);
  const [findSelected, setFindSelected] = useState<FindHit | null>(null);
  const [findStatus, setFindStatus] = useState<'idle' | 'searching' | 'done'>('idle');
  const locateToken = useRef(0);
  const scrolledToken = useRef<number | null>(null);
  /** What the current highlight is: the cited passage or a find match. */
  const targetKind = useRef<'passage' | 'find'>('passage');
  const anchorPage = useRef<number | null>(null);
  const observerRef = useRef<IntersectionObserver | null>(null);
  const restored = useRef(initialView === null);
  const fromCache = useRef(false);
  const firstPaintedRef = useRef(false);
  const sharpMeasured = useRef(false);
  const openMark = useRef(`shodh:pdf-open:${Math.random().toString(36).slice(2)}`);
  /** Background reads wait from mount until the first sharp page is drawn. */
  const releaseForeground = useRef<(() => void) | null>(null);
  const pendingSave = useRef<PdfViewState | null>(null);
  const saveTimer = useRef<number | null>(null);

  const onLocateRef = useRef(onLocate);
  const onFatalRef = useRef(onFatal);
  const onFirstPageVisibleRef = useRef(onFirstPageVisible);
  const onPageChangeRef = useRef(onPageChange);
  useEffect(() => {
    onLocateRef.current = onLocate;
    onFatalRef.current = onFatal;
    onFirstPageVisibleRef.current = onFirstPageVisible;
    onPageChangeRef.current = onPageChange;
  }, [onLocate, onFatal, onFirstPageVisible, onPageChange]);

  const scale = zoom.mode === 'fit' ? fitScale : zoom.scale;
  const numPages = sizes.length;
  const initialPage = initialView ? Math.min(initialView.page, Math.max(1, numPages)) : citedPages?.start ?? 1;

  const currentPageRef = useRef(currentPage);
  const zoomRef = useRef(zoom);
  const fitInitialized = useRef(false);
  useEffect(() => {
    currentPageRef.current = currentPage;
    setPageInput(String(currentPage));
    onPageChangeRef.current?.(currentPage);
  }, [currentPage]);
  useEffect(() => {
    zoomRef.current = zoom;
  }, [zoom]);

  // Opening starts when the viewer mounts (the file was selected).
  useLayoutEffect(() => {
    const mark = openMark.current;
    performance.mark(mark);
    const release = holdForeground();
    releaseForeground.current = release;
    return () => {
      performance.clearMarks(mark);
      release();
      releaseForeground.current = null;
    };
  }, []);

  // Load the document (once per file), from the shared cache when possible.
  useEffect(() => {
    if (fileSize === 'pending') return;
    let cancelled = false;
    let lease: PdfDocLease | null = null;
    textCache.current = new Map();
    fitInitialized.current = false;
    setDoc(null);
    setTarget(null);

    (async () => {
      const acquired = await acquirePdf(filePath, typeof fileSize === 'number' ? fileSize : null);
      if (cancelled) {
        acquired.release();
        return;
      }
      lease = acquired;
      fromCache.current = acquired.fromCache;
      const loaded = acquired.doc;
      const first = (await loaded.getPage(1)).getViewport({ scale: 1 });
      if (cancelled) return;
      // Keep the skeleton's array (and so every offset) when it already matches.
      setSizes(prev =>
        prev.length === loaded.numPages && prev.every(s => s.width === first.width && s.height === first.height)
          ? prev
          : Array.from({ length: loaded.numPages }, () => ({ width: first.width, height: first.height })),
      );
      setDoc(loaded);
    })().catch(error => {
      if (!cancelled) onFatalRef.current(error);
    });

    return () => {
      cancelled = true;
      lease?.release();
    };
  }, [filePath, fileSize]);

  // After the first page is on screen: remember the metadata and resolve
  // every page's real size (mixed-size documents get correct placeholders).
  // Waiting keeps this work off the worker while the first page renders.
  useEffect(() => {
    if (!doc || !firstPainted) return;
    let cancelled = false;
    (async () => {
      if (typeof fileSize === 'number') {
        const meta = await readPdfMeta(doc, fileSize);
        if (cancelled) return;
        rememberPdfMeta(filePath, meta);
      }
      const first = (await doc.getPage(1)).getViewport({ scale: 1 });
      const resolved: PageSize[] = Array.from({ length: doc.numPages }, () => ({ width: first.width, height: first.height }));
      let differs = false;
      for (let n = 2; n <= doc.numPages; n += 1) {
        const vp = (await doc.getPage(n)).getViewport({ scale: 1 });
        if (cancelled) return;
        resolved[n - 1] = { width: vp.width, height: vp.height };
        if (vp.width !== first.width || vp.height !== first.height) differs = true;
        if (differs && (n % 50 === 0 || n === doc.numPages)) {
          anchorPage.current = currentPageRef.current;
          setSizes([...resolved]);
        }
      }
    })().catch(() => {
      // Placeholders keep the first page's size; rendering still works.
    });
    return () => {
      cancelled = true;
    };
  }, [doc, firstPainted, fileSize, filePath]);

  const onPainted = useCallback(
    (page: number, sharp: boolean) => {
      const detail = { file: filePath, page, cached: fromCache.current };
      if (!firstPaintedRef.current) {
        firstPaintedRef.current = true;
        measureFrom(MEASURE_FIRST_PAINT, openMark.current, detail);
        setFirstPainted(true);
        onFirstPageVisibleRef.current?.();
      }
      if (sharp && !sharpMeasured.current) {
        sharpMeasured.current = true;
        measureFrom(MEASURE_SHARP, openMark.current, detail);
        releaseForeground.current?.();
        releaseForeground.current = null;
      }
    },
    [filePath],
  );

  const getText = useCallback(
    (page: number): Promise<PageText> => {
      if (!doc) return Promise.reject(new Error('Document not loaded'));
      let cached = textCache.current.get(page);
      if (!cached) {
        cached = doc
          .getPage(page)
          .then(p => p.getTextContent())
          .then(content => {
            const strings: string[] = [];
            for (const item of content.items) {
              if ('str' in item) strings.push(item.str);
            }
            return { content, strings, haystack: prepareHaystack(strings.join('')) };
          });
        cached.catch(() => textCache.current.delete(page));
        textCache.current.set(page, cached);
      }
      return cached;
    },
    [doc],
  );

  // Fit-to-width scale follows the panel width (including expand/collapse).
  // A layout effect, so the first frame is already at the fitted size.
  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller || sizes.length === 0) return;
    const base = sizes[0];
    const update = () => {
      const width = scroller.clientWidth - PAGE_PADDING * 2;
      if (width <= 0) return;
      const next = clampScale(width / base.width);
      setFitScale(prev => {
        // Keep the reader's page in place when the panel is resized, but not
        // for the first fit (the restore or the locator positions the document).
        if (fitInitialized.current && zoomRef.current.mode === 'fit' && Math.abs(next - prev) > 0.001) {
          anchorPage.current = currentPageRef.current;
        }
        return next;
      });
      fitInitialized.current = true;
      setFitReady(true);
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(scroller);
    return () => observer.disconnect();
  }, [sizes.length > 0 ? sizes[0].width : 0]);

  // Lazy rendering: pages within one screen of the viewport are active.
  useEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller || !doc) return;
    const observer = new IntersectionObserver(
      entries => {
        setVisible(prev => {
          const next = new Set(prev);
          for (const entry of entries) {
            const page = Number((entry.target as HTMLElement).dataset.page);
            if (entry.isIntersecting) next.add(page);
            else next.delete(page);
          }
          return next;
        });
      },
      { root: scroller, rootMargin: '100% 0px' },
    );
    observerRef.current = observer;
    pageEls.current.forEach(el => observer.observe(el));
    return () => {
      observer.disconnect();
      observerRef.current = null;
    };
  }, [doc]);

  const registerElement = useCallback((page: number, el: HTMLDivElement | null) => {
    const previous = pageEls.current.get(page);
    if (previous && previous !== el) observerRef.current?.unobserve(previous);
    if (el) {
      pageEls.current.set(page, el);
      observerRef.current?.observe(el);
    } else {
      pageEls.current.delete(page);
    }
  }, []);

  const scrollToPage = useCallback((page: number, behavior: ScrollBehavior = 'auto') => {
    const scroller = scrollerRef.current;
    const el = pageEls.current.get(page);
    if (!scroller || !el) return;
    scroller.scrollTo({ top: Math.max(0, el.offsetTop - PAGE_GAP), behavior });
  }, []);

  // Keep the page the reader was on in place across zoom and size changes.
  useLayoutEffect(() => {
    if (anchorPage.current !== null && restored.current) {
      scrollToPage(anchorPage.current);
      anchorPage.current = null;
    }
  }, [scale, sizes, scrollToPage]);

  // Restore the remembered position once the page layout is final (sizes
  // known, fit scale applied). The skeleton has the same geometry as the
  // rendered pages, so this usually happens before the file is even read.
  useLayoutEffect(() => {
    if (restored.current || !initialView || sizes.length === 0) return;
    if (zoom.mode === 'fit' && !fitReady) return;
    const scroller = scrollerRef.current;
    const page = Math.min(initialView.page, sizes.length);
    const el = pageEls.current.get(page);
    if (!scroller || !el) return;
    scroller.scrollTop = el.offsetTop + initialView.offset * el.offsetHeight;
    restored.current = true;
    anchorPage.current = null;
    setCurrentPage(page);
  }, [initialView, sizes.length, scale, fitReady, zoom.mode]);

  /** The reading position at the top edge of the viewport. */
  const readViewState = useCallback((): PdfViewState | null => {
    const scroller = scrollerRef.current;
    const count = pageEls.current.size;
    if (!scroller || count === 0) return null;
    const top = scroller.scrollTop;
    let lo = 1;
    let hi = count;
    while (lo < hi) {
      const mid = Math.ceil((lo + hi) / 2);
      const el = pageEls.current.get(mid);
      if (el && el.offsetTop <= top + 1) lo = mid;
      else hi = mid - 1;
    }
    const el = pageEls.current.get(lo);
    if (!el || el.offsetHeight === 0) return null;
    const offset = Math.min(1, Math.max(0, (top - el.offsetTop) / el.offsetHeight));
    return { page: lo, offset, zoom: zoomRef.current };
  }, []);

  const flushSave = useCallback(() => {
    if (saveTimer.current !== null) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    if (pendingSave.current) {
      pdfViewStates.set(viewKey, pendingSave.current);
      pendingSave.current = null;
    }
  }, [viewKey]);

  const queueSave = useCallback(() => {
    if (!rememberView || !restored.current) return;
    const state = readViewState();
    if (!state) return;
    pendingSave.current = state;
    if (saveTimer.current === null) saveTimer.current = window.setTimeout(flushSave, SAVE_DELAY_MS);
  }, [rememberView, readViewState, flushSave]);

  useEffect(() => () => flushSave(), [flushSave]);

  // Zoom is part of the remembered state.
  useEffect(() => {
    if (doc) queueSave();
  }, [zoom, doc, queueSave]);

  // Track the page at the top third of the viewport, and remember the position.
  useEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller || numPages === 0) return;
    let frame = 0;
    const onScroll = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const probe = scroller.scrollTop + scroller.clientHeight / 3;
        let lo = 1;
        let hi = numPages;
        while (lo < hi) {
          const mid = Math.ceil((lo + hi) / 2);
          const el = pageEls.current.get(mid);
          if (el && el.offsetTop <= probe) lo = mid;
          else hi = mid - 1;
        }
        setCurrentPage(lo);
        queueSave();
      });
    };
    scroller.addEventListener('scroll', onScroll, { passive: true });
    return () => {
      cancelAnimationFrame(frame);
      scroller.removeEventListener('scroll', onScroll);
    };
  }, [numPages, queueSave]);

  const citedStart = citedPages?.start ?? null;
  const citedEnd = citedPages?.end ?? null;

  // Locate the passage: cited page(s) first, then every page by distance.
  useEffect(() => {
    if (!doc) return;
    const token = ++locateToken.current;
    const total = doc.numPages;
    const isCurrent = () => token === locateToken.current;
    targetKind.current = 'passage';
    setTarget(null);
    scrolledToken.current = null;

    const start = citedStart !== null ? Math.min(Math.max(1, citedStart), total) : null;
    const end = start !== null && citedEnd !== null ? Math.min(Math.max(start, citedEnd), total) : start;
    const citedList: number[] = [];
    if (start !== null && end !== null) for (let p = start; p <= end; p += 1) citedList.push(p);
    // Without a cited page the document stays where it is: the top, or the
    // remembered position.
    if (start !== null) {
      requestAnimationFrame(() => {
        if (isCurrent()) scrollToPage(start);
      });
    }

    const passageText = passage.trim();
    if (!passageText) {
      onLocateRef.current({
        status: 'notFound',
        message: start !== null ? `No passage text was stored for this source; showing page ${start}.` : 'No passage text was stored for this source; showing the document from the start.',
      });
      return;
    }

    const search = async (pages: number[], onProgress?: (done: number) => void) => {
      let best: { page: number; match: PassageMatch } | null = null;
      for (let i = 0; i < pages.length; i += 1) {
        if (!isCurrent()) return null;
        const text = await getText(pages[i]);
        const match = findPassage(passageText, text.haystack);
        if (match && (!best || matchScore(match) > matchScore(best.match))) best = { page: pages[i], match };
        if (best && best.match.coverage >= CONFIDENT_COVERAGE && !best.match.partial) break;
        if (onProgress && i % 10 === 9) onProgress(i + 1);
      }
      return best;
    };

    (async () => {
      onLocateRef.current({ status: 'searching', message: start !== null ? `Finding the passage on page ${start}…` : 'Finding the passage in this PDF…' });
      let best = citedList.length > 0 ? await search(citedList) : null;
      if (!isCurrent()) return;
      if (!best) {
        const anchor = start ?? 1;
        const rest: number[] = [];
        for (let p = 1; p <= total; p += 1) if (!citedList.includes(p)) rest.push(p);
        rest.sort((a, b) => Math.abs(a - anchor) - Math.abs(b - anchor) || a - b);
        best = await search(rest, done => {
          if (isCurrent()) onLocateRef.current({ status: 'searching', message: `Searching the PDF for the passage… (${done} of ${rest.length} pages)` });
        });
        if (!isCurrent()) return;
      }

      if (!best) {
        onLocateRef.current({
          status: 'notFound',
          message:
            start !== null
              ? `The cited passage could not be pinpointed in the PDF text; showing page ${start}.`
              : 'The cited passage could not be found in the PDF text; showing the document from the start.',
        });
        return;
      }

      setTarget({ page: best.page, ranges: best.match.ranges, token });
      scrollToPage(best.page);
      const moved = start !== null && (best.page < start || best.page > (end ?? start));
      const where = `page ${best.page}`;
      if (best.match.partial || best.match.coverage < 0.6) {
        onLocateRef.current({
          status: 'approximate',
          message: `Highlighted the closest match on ${where}; the retrieved text differs from the PDF text in places.`,
        });
      } else {
        onLocateRef.current({
          status: 'found',
          message: moved ? `Cited passage highlighted on ${where} (indexed as page ${start}).` : `Cited passage highlighted on ${where}.`,
        });
      }
    })().catch(() => {
      if (!isCurrent()) return;
      onLocateRef.current({
        status: 'notFound',
        message: start !== null ? `The PDF text could not be searched; showing page ${start}.` : 'The PDF text could not be searched; showing the document from the start.',
      });
    });
  }, [doc, passage, citedStart, citedEnd, getText, scrollToPage]);

  const onHighlightRendered = useCallback((page: number, token: number, top: number | null) => {
    if (scrolledToken.current === token || token !== locateToken.current) return;
    const scroller = scrollerRef.current;
    const el = pageEls.current.get(page);
    if (!scroller || !el) return;
    scrolledToken.current = token;
    if (top === null) {
      // The match is in the page text but could not be outlined on the
      // rendered page (text layer unavailable or out of step with it).
      if (targetKind.current === 'passage') {
        onLocateRef.current({
          status: 'approximate',
          message: `The cited passage is on page ${page}, but it could not be outlined on the rendered page.`,
        });
      }
      return;
    }
    scroller.scrollTo({ top: Math.max(0, el.offsetTop + top - scroller.clientHeight / 3), behavior: scrollBehavior() });
  }, []);

  // Find in document: search the page text once the query settles, from the
  // current page onward, revealing the first match found there.
  useEffect(() => {
    const query = findQuery.trim();
    setFindSelected(null);
    if (!findOpen || !doc || !query) {
      setFindHits([]);
      setFindStatus('idle');
      return;
    }
    let cancelled = false;
    const startPage = currentPageRef.current;
    const timer = window.setTimeout(() => {
      setFindStatus('searching');
      const total = doc.numPages;
      const order: number[] = [];
      for (let p = startPage; p <= total; p += 1) order.push(p);
      for (let p = 1; p < startPage; p += 1) order.push(p);
      const perPage = new Map<number, FindHit[]>();
      const flatten = () => [...perPage.keys()].sort((a, b) => a - b).flatMap(p => perPage.get(p)!);
      let revealed = false;
      (async () => {
        for (let i = 0; i < order.length; i += 1) {
          const page = order[i];
          const text = await getText(page);
          if (cancelled) return;
          const spans = findInText(text.strings.join(''), query);
          if (spans.length > 0) perPage.set(page, spans.map(s => ({ page, start: s.start, end: s.end })));
          if (!revealed && spans.length > 0) {
            revealed = true;
            setFindHits(flatten());
            setFindSelected(perPage.get(page)![0]);
          } else if (i % 20 === 19) {
            setFindHits(flatten());
          }
        }
        const hits = flatten();
        setFindHits(hits);
        setFindStatus('done');
      })().catch(() => {
        if (!cancelled) setFindStatus('done');
      });
    }, FIND_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [findOpen, findQuery, doc, getText]);

  const findIndex = useMemo(() => (findSelected ? findHits.findIndex(h => sameHit(findSelected, h)) : -1), [findHits, findSelected]);

  // Highlight and scroll to the selected match.
  useEffect(() => {
    if (!findSelected) return;
    const token = ++locateToken.current;
    targetKind.current = 'find';
    scrolledToken.current = null;
    setTarget({ page: findSelected.page, ranges: [{ start: findSelected.start, end: findSelected.end }], token });
    scrollToPage(findSelected.page);
  }, [findSelected, scrollToPage]);

  const openFind = useCallback(() => {
    setFindOpen(true);
    requestAnimationFrame(() => {
      findInputRef.current?.focus();
      findInputRef.current?.select();
    });
  }, []);

  const closeFind = useCallback(() => {
    setFindOpen(false);
    if (targetKind.current === 'find') {
      locateToken.current += 1;
      setTarget(null);
    }
    scrollerRef.current?.focus({ preventScroll: true });
  }, []);

  const stepFind = useCallback(
    (delta: number) => {
      if (!findOpen) {
        openFind();
        return;
      }
      if (findHits.length === 0) return;
      const next = findIndex < 0 ? (delta > 0 ? 0 : findHits.length - 1) : (findIndex + delta + findHits.length) % findHits.length;
      setFindSelected(findHits[next]);
    },
    [findOpen, findHits, findIndex, openFind],
  );

  // Opened from outside the viewer (Ctrl+F while the file list has focus).
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    root.addEventListener(VIEWER_FIND_EVENT, openFind);
    return () => root.removeEventListener(VIEWER_FIND_EVENT, openFind);
  }, [openFind]);

  const zoomBy = (factor: number) => {
    anchorPage.current = currentPage;
    setZoom({ mode: 'manual', scale: clampScale(scale * factor) });
  };

  // Ctrl/⌘ + wheel (and trackpad pinch, which arrives as a ctrl wheel) zooms
  // the pages instead of the window. Native listener: React's onWheel is
  // passive and cannot cancel the window zoom. Steps are folded per frame so
  // a pinch re-renders the pages once per frame, not once per event.
  const scaleRef = useRef(scale);
  scaleRef.current = scale;
  useEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller) return;
    let pending = 1;
    let frame: number | null = null;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      const pixels = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaMode === 2 ? e.deltaY * 400 : e.deltaY;
      pending *= Math.min(2, Math.max(0.5, Math.exp(-pixels * 0.0025)));
      if (frame !== null) return;
      frame = requestAnimationFrame(() => {
        frame = null;
        const factor = pending;
        pending = 1;
        anchorPage.current = currentPageRef.current;
        setZoom({ mode: 'manual', scale: clampScale(scaleRef.current * factor) });
      });
    };
    scroller.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      scroller.removeEventListener('wheel', onWheel);
      if (frame !== null) cancelAnimationFrame(frame);
    };
  }, []);
  const fitWidth = () => {
    anchorPage.current = currentPage;
    setZoom({ mode: 'fit' });
  };
  const goToPage = (page: number) => {
    if (!numPages) return;
    const clamped = Math.min(Math.max(1, page), numPages);
    setCurrentPage(clamped);
    scrollToPage(clamped, scrollBehavior());
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.isDefaultPrevented()) return;
    const command = viewerCommand({
      key: e.key,
      ctrlKey: e.ctrlKey,
      metaKey: e.metaKey,
      altKey: e.altKey,
      shiftKey: e.shiftKey,
      editable: isEditableTarget(e.target),
    });
    // Focus mode belongs to the surrounding browser (if any).
    if (!command || command === 'toggleFocus') return;
    if (command === 'find') openFind();
    else if (command === 'findNext') stepFind(1);
    else if (command === 'findPrev') stepFind(-1);
    else if (!doc) return;
    else if (command === 'nextPage') goToPage(currentPage + 1);
    else if (command === 'prevPage') goToPage(currentPage - 1);
    else if (command === 'firstPage') goToPage(1);
    else if (command === 'lastPage') goToPage(numPages);
    else if (command === 'zoomIn') zoomBy(ZOOM_STEP);
    else if (command === 'zoomOut') zoomBy(1 / ZOOM_STEP);
    else if (command === 'fitWidth') fitWidth();
    // Also keeps Ctrl+= / Ctrl+- / Ctrl+0 from zooming the whole window.
    e.preventDefault();
  };

  const toolButton = cn(
    'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-40 disabled:cursor-not-allowed transition-colors duration-micro',
    VIEWER_FOCUS_RING,
  );

  let findSummary = '';
  if (findQuery.trim()) {
    if (findHits.length > 0) findSummary = `${findIndex >= 0 ? findIndex + 1 : '–'} of ${findHits.length}${findStatus === 'searching' ? '+' : ''}`;
    else findSummary = findStatus === 'done' ? 'No matches' : 'Searching…';
  }

  return (
    <div ref={rootRef} data-pdf-viewer="" className="flex-1 min-h-0 flex flex-col" onKeyDown={handleKeyDown}>
      <div role="toolbar" aria-label="PDF controls" className="flex items-center gap-1 px-3 py-1.5 border-b border-shodh-border-subtle text-[12.5px] text-shodh-text-secondary">
        <button type="button" className={toolButton} onClick={() => goToPage(currentPage - 1)} disabled={!doc || currentPage <= 1} aria-label="Previous page" title="Previous page (PageUp)">
          <ChevronUp className="w-4 h-4" aria-hidden="true" />
        </button>
        <button type="button" className={toolButton} onClick={() => goToPage(currentPage + 1)} disabled={!doc || currentPage >= numPages} aria-label="Next page" title="Next page (PageDown)">
          <ChevronDown className="w-4 h-4" aria-hidden="true" />
        </button>
        <form
          className="flex items-center gap-1.5 ml-1"
          onSubmit={e => {
            e.preventDefault();
            const n = Number(pageInput);
            if (Number.isInteger(n) && n > 0) goToPage(n);
            else setPageInput(String(currentPage));
          }}
        >
          <label htmlFor={pageInputId} className="sr-only">
            Page number
          </label>
          <span aria-hidden="true">Page</span>
          <input
            id={pageInputId}
            inputMode="numeric"
            value={pageInput}
            onChange={e => setPageInput(e.target.value.replace(/[^0-9]/g, ''))}
            onFocus={e => e.target.select()}
            onBlur={() => setPageInput(String(currentPage))}
            disabled={!doc}
            className={cn('w-12 h-7 px-1.5 rounded-md border border-shodh-border-strong bg-shodh-surface-2 text-center tabular-nums text-shodh-text', VIEWER_FOCUS_RING)}
          />
          <span className="tabular-nums">of {numPages || '–'}</span>
        </form>
        <div className="flex-1" />
        <button
          type="button"
          className={cn(toolButton, findOpen && 'bg-shodh-raised text-shodh-text')}
          onClick={() => (findOpen ? closeFind() : openFind())}
          disabled={!doc}
          aria-label="Find in document"
          aria-expanded={findOpen}
          title="Find (Ctrl+F)"
        >
          <Search className="w-4 h-4" aria-hidden="true" />
        </button>
        <button type="button" className={toolButton} onClick={() => zoomBy(1 / ZOOM_STEP)} disabled={!doc || scale <= MIN_SCALE} aria-label="Zoom out" title="Zoom out (-)">
          <Minus className="w-4 h-4" aria-hidden="true" />
        </button>
        <span className="w-12 text-center tabular-nums" aria-label={`Zoom ${Math.round(scale * 100)} percent`}>
          {Math.round(scale * 100)}%
        </span>
        <button type="button" className={toolButton} onClick={() => zoomBy(ZOOM_STEP)} disabled={!doc || scale >= MAX_SCALE} aria-label="Zoom in" title="Zoom in (+)">
          <Plus className="w-4 h-4" aria-hidden="true" />
        </button>
        <button
          type="button"
          className={cn(toolButton, 'w-auto px-2 gap-1.5', zoom.mode === 'fit' && 'bg-shodh-raised text-shodh-text')}
          onClick={fitWidth}
          disabled={!doc}
          aria-pressed={zoom.mode === 'fit'}
          title="Fit width (0)"
        >
          <MoveHorizontal className="w-4 h-4" aria-hidden="true" />
          Fit width
        </button>
      </div>

      {findOpen && (
        <div role="search" className="flex items-center gap-1.5 px-3 py-1.5 border-b border-shodh-border-subtle text-[12.5px] text-shodh-text-secondary">
          <label htmlFor={findInputId} className="sr-only">
            Find in document
          </label>
          <input
            ref={findInputRef}
            id={findInputId}
            type="search"
            value={findQuery}
            onChange={e => setFindQuery(e.target.value)}
            onKeyDown={e => {
              if (e.key === 'Enter') {
                e.preventDefault();
                stepFind(e.shiftKey ? -1 : 1);
              } else if (e.key === 'Escape') {
                // Closes find only; the browser's Esc (close file) must not also fire.
                e.preventDefault();
                closeFind();
              }
            }}
            placeholder="Find in document"
            className={cn(
              'flex-1 min-w-0 h-7 px-2 rounded-md border border-shodh-border-strong bg-shodh-surface-2 text-shodh-text placeholder:text-shodh-text-faint',
              VIEWER_FOCUS_RING,
            )}
          />
          <span className="min-w-[76px] text-right tabular-nums text-shodh-text-muted" role="status" aria-live="polite">
            {findSummary}
          </span>
          <button type="button" className={toolButton} onClick={() => stepFind(-1)} disabled={findHits.length === 0} aria-label="Previous match" title="Previous match (Shift+Enter)">
            <ChevronUp className="w-4 h-4" aria-hidden="true" />
          </button>
          <button type="button" className={toolButton} onClick={() => stepFind(1)} disabled={findHits.length === 0} aria-label="Next match" title="Next match (Enter)">
            <ChevronDown className="w-4 h-4" aria-hidden="true" />
          </button>
          <button type="button" className={toolButton} onClick={closeFind} aria-label="Close find" title="Close (Esc)">
            <X className="w-4 h-4" aria-hidden="true" />
          </button>
        </div>
      )}

      <div
        ref={scrollerRef}
        tabIndex={0}
        data-viewer-scroller=""
        aria-label="PDF pages"
        className={cn('relative flex-1 min-h-0 overflow-auto overscroll-contain scrollbar-thin bg-shodh-raised-2', VIEWER_FOCUS_RING)}
        style={{ padding: PAGE_PADDING }}
      >
        {sizes.length === 0 ? (
          <div className="h-full flex items-center justify-center gap-2 text-[13px] text-shodh-text-muted" role="status">
            <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            Loading PDF…
          </div>
        ) : (
          <div className="flex flex-col items-center" style={{ gap: PAGE_GAP }}>
            {sizes.map((size, index) => {
              const pageNumber = index + 1;
              const isTarget = target?.page === pageNumber;
              return (
                <PdfPageView
                  key={pageNumber}
                  doc={doc}
                  pageNumber={pageNumber}
                  scale={scale}
                  size={{ width: Math.floor(size.width * scale), height: Math.floor(size.height * scale) }}
                  active={visible.has(pageNumber) || isTarget}
                  quick={!firstPainted && pageNumber === initialPage}
                  getText={getText}
                  ranges={isTarget ? target.ranges : null}
                  highlightToken={isTarget ? target.token : null}
                  onHighlightRendered={onHighlightRendered}
                  onPainted={onPainted}
                  registerElement={registerElement}
                />
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
